//! Realtime fan-out: named channels (`item:12`, `user:5`, `global`) backed by
//! tokio broadcast, streamed to browsers as SSE with `Last-Event-ID` resume.
//! Cross-node delivery (LISTEN/NOTIFY) feeds `Hub::publish` on each node.

use axum::response::sse::{Event, KeepAlive, Sse};
use dashmap::DashMap;
use futures_util::stream::{self, Stream, StreamExt};
use serde::Serialize;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Recent messages kept per channel for `Last-Event-ID` resume.
const BACKLOG: usize = 64;
const CHANNEL_CAPACITY: usize = 256;
/// How long an idle channel keeps its backlog after its last receiver left or its last message.
const BACKLOG_TTL: Duration = Duration::from_secs(600);
/// Sent in place of messages a slow client missed: it should reload what it shows.
pub const RESYNC: &str = "resync";

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Message {
    pub id: u64,
    pub channel: String,
    pub event: String,
    pub data: serde_json::Value,
}

struct Channel {
    tx: broadcast::Sender<Arc<Message>>,
    backlog: Mutex<VecDeque<Arc<Message>>>,
    /// The last publish or subscribe, for the sweep.
    touched: Mutex<Instant>,
}

impl Channel {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(CHANNEL_CAPACITY).0,
            backlog: Mutex::new(VecDeque::new()),
            touched: Mutex::new(Instant::now()),
        }
    }

    fn touch(&self) {
        *self.touched.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.touched.lock().unwrap_or_else(std::sync::PoisonError::into_inner).elapsed()
    }
}

/// Called with every locally published message; set only when several nodes run.
type Bridge = Box<dyn Fn(&Message) + Send + Sync>;

#[derive(Clone, Default)]
pub struct Hub {
    channels: Arc<DashMap<String, Arc<Channel>>>,
    next_id: Arc<AtomicU64>,
    /// Forwards local publishes to the other nodes (Postgres NOTIFY). Unset on a single node,
    /// where `publish` does exactly what it did before.
    bridge: Arc<OnceLock<Bridge>>,
}

impl Hub {
    fn channel(&self, name: &str) -> Arc<Channel> {
        self.channels.entry(name.to_owned()).or_insert_with(|| Arc::new(Channel::new())).clone()
    }

    /// A receiver and the backlog after `last_id`, taken while the map entry is held: `sweep`
    /// cannot remove the channel in between, and no message falls between the two (`publish`
    /// stores and sends under the backlog lock).
    fn attach(&self, name: &str, last_id: Option<u64>) -> (broadcast::Receiver<Arc<Message>>, Vec<Arc<Message>>) {
        let entry = self.channels.entry(name.to_owned()).or_insert_with(|| Arc::new(Channel::new()));
        let ch = entry.value();
        ch.touch();
        let b = ch.backlog.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let missed = last_id.map(|last| b.iter().filter(|m| m.id > last).cloned().collect()).unwrap_or_default();
        (ch.tx.subscribe(), missed)
    }

    /// Forwards every later `publish` to `f` too (once; a second call is ignored).
    pub fn set_bridge(&self, f: impl Fn(&Message) + Send + Sync + 'static) {
        let _ = self.bridge.set(Box::new(f));
    }

    /// Publishes to this node's subscribers and to the other nodes, if a bridge is set.
    pub fn publish(&self, channel: &str, event: &str, data: serde_json::Value) -> u64 {
        let (id, msg) = self.publish_local(channel, event, data);
        if let Some(b) = self.bridge.get() {
            b(&msg);
        }
        id
    }

    /// A message that came from another node: delivered here only, never sent back out.
    pub fn publish_remote(&self, channel: &str, event: &str, data: serde_json::Value) -> u64 {
        self.publish_local(channel, event, data).0
    }

    fn publish_local(&self, channel: &str, event: &str, data: serde_json::Value) -> (u64, Arc<Message>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = Arc::new(Message { id, channel: channel.to_owned(), event: event.to_owned(), data });
        let ch = self.channel(channel);
        ch.touch();
        let mut b = ch.backlog.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if b.len() == BACKLOG {
            b.pop_front();
        }
        b.push_back(msg.clone());
        // No receivers is fine: nobody is watching this channel right now.
        let _ = ch.tx.send(msg.clone());
        drop(b);
        (id, msg)
    }

    /// Live subscriptions across all channels, for the metrics endpoint.
    pub fn subscriber_count(&self) -> usize {
        self.channels.iter().map(|c| c.tx.receiver_count()).sum()
    }

    /// Drops channels with no subscribers that have been quiet for `BACKLOG_TTL`: until then
    /// their backlog still serves a `Last-Event-ID` resume.
    pub fn sweep(&self) {
        self.sweep_older_than(BACKLOG_TTL);
    }

    fn sweep_older_than(&self, ttl: Duration) {
        self.channels.retain(|_, c| c.tx.receiver_count() > 0 || c.idle_for() < ttl);
    }

    /// Missed messages after `last_id` followed by live ones, across all channels.
    /// The caller must have checked that the actor may read every channel.
    pub fn subscribe(&self, channels: &[String], last_id: Option<u64>) -> impl Stream<Item = Arc<Message>> + use<> {
        let mut missed = Vec::new();
        let mut live = Vec::new();
        for name in channels {
            let (rx, backlog) = self.attach(name, last_id);
            missed.extend(backlog);
            let name = name.clone();
            live.push(
                BroadcastStream::new(rx)
                    .map(move |r| match r {
                        Ok(m) => m,
                        // The client fell behind and messages were dropped: tell it to reload.
                        Err(_) => Arc::new(Message {
                            id: 0,
                            channel: name.clone(),
                            event: RESYNC.to_owned(),
                            data: serde_json::Value::Null,
                        }),
                    })
                    .boxed(),
            );
        }
        missed.sort_by_key(|m| m.id);
        stream::iter(missed).chain(stream::select_all(live))
    }

    /// `keep` drops messages this viewer must not see, on channels shared by viewers with
    /// different access (the forum activity channel).
    pub fn sse<K>(
        &self,
        channels: &[String],
        last_id: Option<u64>,
        keep: K,
    ) -> Sse<impl Stream<Item = Result<Event, Infallible>> + use<K>>
    where
        K: Fn(&Message) -> bool + Send + 'static,
    {
        let s = self
            .subscribe(channels, last_id)
            .filter(move |m| std::future::ready(m.event == RESYNC || keep(m)))
            .map(|m| {
                let e = Event::default().event(&m.event);
                // A resync is not a message in the sequence: it must not move `Last-Event-ID`.
                let e = if m.id == 0 { e } else { e.id(m.id.to_string()) };
                Ok(e.json_data(serde_json::json!({ "channel": m.channel, "data": m.data })).unwrap_or_default())
            });
        Sse::new(s).keep_alive(KeepAlive::new().interval(Duration::from_secs(25)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn live_delivery_only_to_subscribed_channels() {
        let hub = Hub::default();
        let mut s = Box::pin(hub.subscribe(&["item:1".into()], None));
        hub.publish("item:2", "post", json!(2));
        hub.publish("item:1", "post", json!(1));
        let m = s.next().await.unwrap();
        assert_eq!((m.channel.as_str(), &m.data), ("item:1", &json!(1)));
    }

    #[tokio::test]
    async fn resume_replays_missed_messages_in_order() {
        let hub = Hub::default();
        let first = hub.publish("user:5", "notification", json!("a"));
        hub.publish("user:5", "notification", json!("b"));
        hub.publish("global", "settings", json!("c"));
        let mut s = Box::pin(hub.subscribe(&["global".into(), "user:5".into()], Some(first)));
        let got: Vec<_> = vec![s.next().await.unwrap().data.clone(), s.next().await.unwrap().data.clone()];
        assert_eq!(got, [json!("b"), json!("c")]);
    }

    #[test]
    fn backlog_is_bounded_and_sweep_drops_idle_channels() {
        let hub = Hub::default();
        for i in 0..200 {
            hub.publish("x", "e", json!(i));
        }
        assert_eq!(hub.channel("x").backlog.lock().unwrap().len(), BACKLOG);
        hub.sweep_older_than(Duration::ZERO);
        assert!(hub.channels.is_empty());
    }

    #[tokio::test]
    async fn backlog_survives_a_sweep_within_the_ttl() {
        let hub = Hub::default();
        let first = hub.publish("item:9", "post", json!(1));
        hub.publish("item:9", "post", json!(2));
        // A default sweep keeps a channel that was just used, though nobody listens.
        hub.sweep();
        let mut s = Box::pin(hub.subscribe(&["item:9".into()], Some(first)));
        assert_eq!(s.next().await.unwrap().data, json!(2));
        // Still listened to: kept even when it is past the TTL.
        hub.sweep_older_than(Duration::ZERO);
        assert_eq!(hub.channels.len(), 1);
        drop(s);
        hub.sweep_older_than(Duration::ZERO);
        assert!(hub.channels.is_empty());
    }

    #[tokio::test]
    async fn subscriber_survives_sweeps_and_still_receives() {
        let hub = Hub::default();
        let mut s = Box::pin(hub.subscribe(&["user:1".into()], None));
        hub.sweep_older_than(Duration::ZERO);
        hub.publish("user:1", "notification", json!("x"));
        assert_eq!(s.next().await.unwrap().data, json!("x"));
    }

    #[tokio::test]
    async fn a_slow_client_is_told_to_resync() {
        let hub = Hub::default();
        let mut s = Box::pin(hub.subscribe(&["activity".into()], None));
        for i in 0..(CHANNEL_CAPACITY + 10) {
            hub.publish("activity", "topic", json!(i));
        }
        let m = s.next().await.unwrap();
        assert_eq!((m.event.as_str(), m.channel.as_str(), m.id), (RESYNC, "activity", 0));
        // And the stream carries on with the messages that were kept.
        assert_eq!(s.next().await.unwrap().event, "topic");
    }
}
