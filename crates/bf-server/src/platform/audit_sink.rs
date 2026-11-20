//! Ships security events to an external log system.
//!
//! An audit log only helps after an incident if the attacker cannot reach it. The table is
//! append-only, but anyone with the database can still drop it, so events are
//! copied off the host as they are written.
//!
//! Two sinks, either or both:
//!
//! * `BF_AUDIT_SINK_STDOUT=true` writes each event to the `audit` tracing target. With
//!   `BF_LOG_JSON=true` that is one JSON object per line on stdout, which any log agent collects
//!   without the forum needing to know about the agent.
//! * `BF_AUDIT_SINK_URL=https://…` POSTs batches as newline-delimited JSON, signed the same way
//!   as outgoing webhooks when `BF_AUDIT_SINK_SECRET` is set, with `BF_AUDIT_SINK_TOKEN` as an
//!   optional bearer token. The address goes through the webhook SSRF guards.
//!
//! A cursor in the database records how far shipping has got, so a restart resumes instead of
//! re-sending, and it only moves after the sink has accepted the batch: a failing sink makes
//! events arrive late, never silently not at all.

use crate::state::AppState;
use std::time::Duration;

const BATCH: i64 = 200;

fn sink_url() -> Option<String> {
    std::env::var("BF_AUDIT_SINK_URL").ok().filter(|u| crate::platform::outbound::acceptable_target(u))
}

fn to_stdout() -> bool {
    std::env::var("BF_AUDIT_SINK_STDOUT").is_ok_and(|v| v == "true" || v == "1")
}

/// Starts the shipper when a sink is configured. Silent and costless otherwise.
pub fn spawn(state: AppState) {
    let url = sink_url();
    let stdout = to_stdout();
    if url.is_none() && !stdout {
        return;
    }
    if std::env::var("BF_AUDIT_SINK_URL").is_ok() && url.is_none() {
        tracing::error!("BF_AUDIT_SINK_URL is not an allowed https address; audit forwarding is off");
    }
    tracing::info!(http = url.is_some(), stdout, "audit log forwarding enabled");
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(bf_core::scaled(Duration::from_secs(30)));
        loop {
            tick.tick().await;
            if let Err(e) = ship(&state, url.as_deref(), stdout).await {
                // Warn rather than give up: the next tick retries from the same cursor.
                tracing::warn!(error = %e, "audit log forwarding failed");
            }
        }
    });
}

/// Sends everything past the cursor, a batch at a time, advancing only on success.
async fn ship(state: &AppState, url: Option<&str>, stdout: bool) -> Result<(), String> {
    let Some(pool) = state.pool.as_ref() else { return Ok(()) };
    loop {
        let after = bf_db::admin::audit::forward_cursor(pool).await.map_err(|e| e.to_string())?;
        let events = bf_db::admin::audit::events_after(pool, after, BATCH).await.map_err(|e| e.to_string())?;
        let Some(last) = events.last().map(|e| e.id) else { return Ok(()) };

        if stdout {
            for event in &events {
                tracing::info!(
                    target: "audit",
                    id = event.id,
                    actor_id = event.actor_id,
                    action = %event.action,
                    target_ref = event.target.as_deref().unwrap_or(""),
                    details = %event.details,
                    at = %event.created_at,
                );
            }
        }

        if let Some(url) = url {
            let mut body = Vec::new();
            for event in &events {
                serde_json::to_writer(&mut body, event).map_err(|e| e.to_string())?;
                body.push(b'\n');
            }
            let now = chrono::Utc::now().timestamp();
            let signature = std::env::var("BF_AUDIT_SINK_SECRET")
                .ok()
                .filter(|s| s.len() >= 32)
                .map(|secret| bf_core::auth::webhook_signature(&secret, now, &body));
            let bearer = std::env::var("BF_AUDIT_SINK_TOKEN").ok().map(|t| format!("Bearer {t}"));
            let timestamp = now.to_string();
            let mut headers = vec![("content-type", "application/x-ndjson"), ("x-bf-timestamp", timestamp.as_str())];
            if let Some(s) = signature.as_deref() {
                headers.push(("x-bf-signature", s));
            }
            if let Some(b) = bearer.as_deref() {
                headers.push(("authorization", b));
            }
            let status = crate::platform::outbound::post_public(url, &headers, body).await?;
            if !(200..300).contains(&status) {
                return Err(format!("sink answered {status}"));
            }
        }

        bf_db::admin::audit::set_forward_cursor(pool, last).await.map_err(|e| e.to_string())?;
        // A short batch means the log is caught up; stop until the next tick.
        if (events.len() as i64) < BATCH {
            return Ok(());
        }
    }
}
