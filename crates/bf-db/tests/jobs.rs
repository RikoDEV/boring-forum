//! Background-job queues on a real Postgres: each unit of work goes to one claimer, however many
//! nodes ask at once. Needs Postgres: `BF_DATABASE_URL=... cargo test -p bf-db --test jobs -- --ignored --test-threads=1`.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_db::platform::{ads, api, digests, push};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::HashSet;
use std::str::FromStr;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("BF_DATABASE_URL").expect("BF_DATABASE_URL must be set for --ignored tests");
    let opts = PgConnectOptions::from_str(&url).expect("BF_DATABASE_URL must be a valid Postgres URL");
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .expect("the test database must accept connections");
    bf_db::migrate(&pool).await.expect("migrations must apply to the test database");
    pool
}

async fn user(pool: &sqlx::PgPool, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, username_lower, email, email_lower, password_hash, username_skeleton) VALUES ($1, $1, $2, $2, 'x', $1) RETURNING id",
    )
    .bind(name)
    .bind(format!("{name}@example.com"))
    .fetch_one(pool)
    .await
    .expect("a user can be created")
}

fn disjoint(a: &[i64], b: &[i64]) -> bool {
    let a: HashSet<_> = a.iter().collect();
    b.iter().all(|x| !a.contains(x))
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn webhook_deliveries_go_to_one_claimer() {
    let pool = pool().await;
    let tag = std::process::id();
    let hook = api::create_webhook(&pool, "https://example.com/hook", "secret", &[format!("jobs.{tag}")], &[])
        .await
        .expect("webhook");
    for i in 0..30 {
        api::enqueue_event(&pool, &format!("jobs.{tag}"), &serde_json::json!({ "n": i })).await.expect("enqueue");
    }
    let (a, b) = tokio::join!(api::due_deliveries(&pool, 20), api::due_deliveries(&pool, 20));
    let ids = |v: Vec<api::Due>| v.into_iter().map(|d| d.id).collect::<Vec<_>>();
    let (a, b) = (ids(a.expect("claim a")), ids(b.expect("claim b")));
    assert!(disjoint(&a, &b), "a delivery went to both claimers");
    assert!(a.windows(2).all(|w| w[0] < w[1]), "oldest first");
    let mine: i64 =
        sqlx::query_scalar("SELECT count(*) FROM webhook_deliveries WHERE webhook_id = $1 AND next_at > now()")
            .bind(hook)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(mine as usize, a.len() + b.len(), "claimed rows are leased");
    // Leased rows are not handed out again until the lease ends; a failure schedules the retry.
    let again = api::due_deliveries(&pool, 100).await.expect("claim again");
    assert!(again.iter().all(|d| !a.contains(&d.id) && !b.contains(&d.id)));
    api::delivery_failed(&pool, a[0], Some(500), "status 500", Some(0)).await.expect("failed");
    let retry = api::due_deliveries(&pool, 100).await.expect("claim retry");
    assert!(retry.iter().any(|d| d.id == a[0]), "a failed delivery is retried");
    sqlx::query("DELETE FROM webhooks WHERE id = $1").bind(hook).execute(&pool).await.expect("cleanup");
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn digests_are_claimed_once_and_only_for_verified_addresses() {
    let pool = pool().await;
    let tag = std::process::id();
    let mut verified = Vec::new();
    for i in 0..6 {
        let id = user(&pool, &format!("dg{tag}v{i}")).await;
        sqlx::query("UPDATE users SET digest = 'daily', last_digest_at = now() - interval '3 days', email_verified_at = now() WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .expect("setup");
        verified.push(id);
    }
    let unverified = user(&pool, &format!("dg{tag}u")).await;
    let bad = user(&pool, &format!("dg{tag}b")).await;
    sqlx::query("UPDATE users SET digest = 'daily', last_digest_at = now() - interval '3 days' WHERE id = ANY($1)")
        .bind([unverified, bad])
        .execute(&pool)
        .await
        .expect("setup");
    sqlx::query("UPDATE users SET email_verified_at = now(), email_bad = true WHERE id = $1")
        .bind(bad)
        .execute(&pool)
        .await
        .expect("setup");

    let (a, b) = tokio::join!(digests::due_digests(&pool, 3), digests::due_digests(&pool, 1000));
    let (a, b) = (a.expect("claim a"), b.expect("claim b"));
    let (a_ids, b_ids): (Vec<i64>, Vec<i64>) = (a.iter().map(|d| d.id).collect(), b.iter().map(|d| d.id).collect());
    assert!(disjoint(&a_ids, &b_ids), "a member's digest went to both claimers");
    let all: HashSet<i64> = a_ids.iter().chain(&b_ids).copied().collect();
    assert!(verified.iter().all(|id| all.contains(id)), "every verified member is claimed by someone");
    assert!(!all.contains(&unverified), "unverified address");
    assert!(!all.contains(&bad), "address marked bad");

    // A failed build gives the claim back with the old clock.
    let first = a.first().or(b.first()).expect("one claimed");
    digests::release_digest(&pool, first.id, first.since).await.expect("release");
    let again = digests::due_digests(&pool, 1000).await.expect("claim again");
    assert!(again.iter().any(|d| d.id == first.id), "released claim is due again");
    assert_eq!(again.iter().find(|d| d.id == first.id).map(|d| d.since), Some(first.since));
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn push_is_queued_per_device_limited_by_notification_and_retried() {
    let pool = pool().await;
    let tag = std::process::id();
    let u = user(&pool, &format!("ps{tag}")).await;
    let endpoints: Vec<String> = (0..3).map(|i| format!("https://push.example/{tag}/{i}")).collect();
    for e in &endpoints {
        push::add(&pool, u, e, "k", "a").await.expect("subscribe");
    }
    let mut ns = Vec::new();
    for i in 0..3 {
        ns.push(
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO notifications (user_id, kind, data) VALUES ($1, 'reply', $2) RETURNING id",
            )
            .bind(u)
            .bind(serde_json::json!({ "url": format!("/{i}") }))
            .fetch_one(&pool)
            .await
            .expect("notify"),
        );
    }
    sqlx::query("UPDATE push_cursor SET last_id = $1").bind(ns[0] - 1).execute(&pool).await.expect("cursor");
    let mine = |_: ()| async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM push_queue WHERE endpoint = ANY($1)")
            .bind(&endpoints)
            .fetch_one(&pool)
            .await
            .expect("count")
    };

    // Two notifications is the limit: all six messages for them are queued, none for the third.
    assert!(push::fan_out(&pool, 2).await.expect("fan out") >= 2);
    assert_eq!(mine(()).await, 6, "every device of the first two notifications, not a slice of them");
    let cursor: i64 = sqlx::query_scalar("SELECT last_id FROM push_cursor").fetch_one(&pool).await.expect("cursor");
    assert!(cursor >= ns[1] && cursor < ns[2]);
    push::fan_out(&pool, 2).await.expect("fan out");
    assert_eq!(mine(()).await, 9);
    assert_eq!(push::fan_out(&pool, 2).await.expect("fan out"), 0, "nothing is queued twice");

    // Two nodes claiming at once never share a message.
    let (a, b) = tokio::join!(push::due(&pool, 5), push::due(&pool, 5));
    let ids = |v: Vec<push::Due>| {
        v.into_iter().filter(|d| endpoints.contains(&d.endpoint)).map(|d| d.queue_id).collect::<Vec<_>>()
    };
    let (a, b) = (ids(a.expect("claim a")), ids(b.expect("claim b")));
    assert!(disjoint(&a, &b), "a push went to both claimers");

    // Transient failure: kept with a later retry, dropped for good after MAX_ATTEMPTS.
    let q = *a.first().or(b.first()).expect("claimed one");
    push::push_failed(&pool, q).await.expect("failed");
    let (attempts, later): (i32, bool) =
        sqlx::query_as("SELECT attempts, next_at > now() FROM push_queue WHERE id = $1")
            .bind(q)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert!(attempts == 1 && later, "retried later, not dropped");
    for _ in 1..push::MAX_ATTEMPTS {
        push::push_failed(&pool, q).await.expect("failed");
    }
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM push_queue WHERE id = $1")
        .bind(q)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(left, 0, "given up after the last attempt");

    // A sent message leaves the queue; a dropped subscription takes its messages with it.
    push::push_done(&pool, a.get(1).or(b.first()).copied().unwrap_or(q)).await.expect("done");
    push::remove_endpoint(&pool, &endpoints[0]).await.expect("gone");
    let on_gone: i64 = sqlx::query_scalar("SELECT count(*) FROM push_queue WHERE endpoint = $1")
        .bind(&endpoints[0])
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(on_gone, 0);
    sqlx::query("DELETE FROM users WHERE id = $1").bind(u).execute(&pool).await.expect("cleanup");
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn referral_rewards_are_granted_once_and_skip_suspended_accounts() {
    use bf_db::community::referrals;
    let pool = pool().await;
    let tag = std::process::id();
    let group: i64 = sqlx::query_scalar("INSERT INTO groups (name, slug) VALUES ($1, $1) RETURNING id")
        .bind(format!("ref{tag}"))
        .fetch_one(&pool)
        .await
        .expect("group");
    let referrer = user(&pool, &format!("rf{tag}")).await;
    let mut referred = Vec::new();
    for i in 0..2 {
        let id = user(&pool, &format!("rf{tag}c{i}")).await;
        sqlx::query("UPDATE users SET referred_by = $2, trust_level = 1 WHERE id = $1")
            .bind(id)
            .bind(referrer)
            .execute(&pool)
            .await
            .expect("setup");
        referred.push(id);
    }
    // One of them is suspended: only one qualifies, so two are not enough.
    sqlx::query("UPDATE users SET suspended_until = now() + interval '1 day' WHERE id = $1")
        .bind(referred[1])
        .execute(&pool)
        .await
        .expect("suspend");
    assert!(!referrals::reward(&pool, 2, group).await.expect("reward").contains(&referrer));
    assert!(!referrals::referrals(&pool, referrer).await.expect("list").iter().all(|r| r.3));
    sqlx::query("UPDATE users SET suspended_until = NULL WHERE id = $1")
        .bind(referred[1])
        .execute(&pool)
        .await
        .expect("lift");

    assert_eq!(referrals::reward(&pool, 2, group).await.expect("reward"), [referrer]);
    // An administrator takes the group away; the next tick must not hand it back.
    sqlx::query("DELETE FROM user_secondary_groups WHERE user_id = $1 AND group_id = $2")
        .bind(referrer)
        .bind(group)
        .execute(&pool)
        .await
        .expect("remove");
    assert!(referrals::reward(&pool, 2, group).await.expect("reward").is_empty());
    let back: i64 =
        sqlx::query_scalar("SELECT count(*) FROM user_secondary_groups WHERE user_id = $1 AND group_id = $2")
            .bind(referrer)
            .bind(group)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(back, 0);
    sqlx::query("DELETE FROM groups WHERE id = $1").bind(group).execute(&pool).await.expect("cleanup");
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn ad_clicks_only_count_for_live_ads() {
    let pool = pool().await;
    let tag = std::process::id();
    // `ended_hours_ago`: None for an ad with no end.
    let mk = |active: bool, ended_hours_ago: Option<i32>| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO ads (name, placement, html, url, active, ends_at)
                 VALUES ($1, 'footer', '<b>x</b>', 'https://example.com/', $2, now() - make_interval(hours => $3)) RETURNING id",
            )
            .bind(format!("ad{tag}{active}{ended_hours_ago:?}"))
            .bind(active)
            .bind(ended_hours_ago)
            .fetch_one(&pool)
            .await
            .expect("ad")
        }
    };
    let live = mk(true, None).await;
    let off = mk(false, None).await;
    let expired = mk(true, Some(1)).await;
    assert_eq!(ads::click(&pool, live, true).await.expect("click").as_deref(), Some("https://example.com/"));
    assert!(ads::click(&pool, off, true).await.expect("click").is_none(), "inactive");
    assert!(ads::click(&pool, expired, true).await.expect("click").is_none(), "ended");
    // Not counted (a crawler, or over the per-address limit), but the visitor still gets through.
    assert!(ads::click(&pool, live, false).await.expect("click").is_some());
    let clicks = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT clicks FROM ads WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("clicks")
        }
    };
    assert_eq!((clicks(live).await, clicks(off).await, clicks(expired).await), (1, 0, 0));
    sqlx::query("DELETE FROM ads WHERE id = ANY($1)").bind([live, off, expired]).execute(&pool).await.expect("cleanup");
}
