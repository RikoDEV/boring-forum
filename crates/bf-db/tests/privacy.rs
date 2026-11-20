//! Personal data: the export and erasure registry against the real schema, erasure itself, and the
//! retention jobs. Needs Postgres: `BF_DATABASE_URL=... cargo test -p bf-db --test privacy -- --ignored`.
//! Use a throwaway database; the tests only touch rows they create.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_db::accounts::lifecycle::{self, Erase, Export, REGISTRY};
use bf_db::admin::{audit, maintenance};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::BTreeSet;
use std::str::FromStr;

async fn pool() -> PgPool {
    let url = std::env::var("BF_DATABASE_URL").expect("BF_DATABASE_URL must be set for --ignored tests");
    let opts = PgConnectOptions::from_str(&url)
        .expect("BF_DATABASE_URL must be a valid Postgres URL")
        .statement_cache_capacity(0);
    let pool =
        PgPoolOptions::new().max_connections(4).connect_with(opts).await.expect("database must accept connections");
    bf_db::migrate(&pool).await.expect("migrations must apply to the test database");
    pool
}

/// A member in the ordinary group (the first account of an empty database would be an admin).
async fn member(pool: &PgPool, name: &str) -> i64 {
    let tag = format!("{name}{}", std::process::id());
    let id = bf_db::accounts::users::create_user(pool, &tag, &format!("{tag}@example.com"), "x")
        .await
        .expect("a test member can be created");
    sqlx::query("UPDATE users SET primary_group_id = 2 WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("the member can be demoted");
    id.get()
}

async fn count(pool: &PgPool, sql: &str, user: i64) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned())).bind(user).fetch_one(pool).await.expect("the count runs")
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn personal_data_registry_matches_schema() {
    let pool = pool().await;
    let actual: BTreeSet<(String, String)> = sqlx::query_as(
        "SELECT c.conrelid::regclass::text, a.attname::text FROM pg_constraint c
         JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = ANY (c.conkey)
         WHERE c.contype = 'f' AND c.confrelid = 'public.users'::regclass AND c.connamespace = 'public'::regnamespace",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .collect();
    let registered: BTreeSet<(String, String)> =
        REGISTRY.iter().map(|e| (e.table.to_owned(), e.column.to_owned())).collect();
    assert_eq!(registered.len(), REGISTRY.len(), "a column is listed twice");
    let missing: Vec<_> = actual.difference(&registered).collect();
    let stale: Vec<_> = registered.difference(&actual).collect();
    assert!(
        missing.is_empty(),
        "tables with a user column that REGISTRY (accounts/lifecycle.rs) does not decide on: {missing:?}"
    );
    assert!(stale.is_empty(), "REGISTRY names columns that do not reference users: {stale:?}");

    // What an export adds as raw rows must not carry secrets, whatever column is added later.
    let secret = ["secret", "hash", "digest", "private", "password", "p256dh", "public_key"];
    for e in REGISTRY {
        let Export::Rows(hidden) = e.export else { continue };
        let columns: Vec<String> = sqlx::query_scalar(
            "SELECT column_name::text FROM information_schema.columns WHERE table_schema = 'public' AND table_name = $1",
        )
        .bind(e.table)
        .fetch_all(&pool)
        .await
        .unwrap();
        for c in columns.iter().filter(|c| !hidden.contains(&c.as_str())) {
            assert!(
                !secret.iter().any(|s| c.contains(s)) && c != "auth",
                "{}.{c} looks secret but is exported",
                e.table
            );
        }
    }
    // Scrubbed columns exist (a typo would only show up when somebody is deleted).
    for e in REGISTRY {
        if let Erase::Scrub(columns) = e.erase {
            for c in columns {
                let n: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM information_schema.columns WHERE table_name = $1 AND column_name = $2",
                )
                .bind(e.table)
                .bind(*c)
                .fetch_one(&pool)
                .await
                .unwrap();
                assert!(n > 0, "{}.{c} does not exist", e.table);
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn erasure_leaves_no_personal_rows() {
    let pool = pool().await;
    let user = member(&pool, "erase").await;
    let digest = format!("digest-erase-{user}");
    bf_db::accounts::sessions::create_session(&pool, &digest, user.into(), Some("203.0.113.9".parse().unwrap()))
        .await
        .unwrap();
    sqlx::query("INSERT INTO failed_logins (login, user_id, ip) VALUES ('x', $1, '203.0.113.9')")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO notifications (user_id, kind, data) VALUES ($1, 'mention', '{}')")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ap_keys (owner_type, owner_id, public_pem, private_pem) VALUES ('user', $1, 'pub', 'sealed')",
    )
    .bind(user)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET newsletter = true, ap_moved_to = 'https://x/y' WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    // The export covers the tables beyond the hand-written sections, and never a session token.
    let export = lifecycle::export(&pool, user).await.unwrap();
    assert_eq!(export["data"]["notifications"].as_array().unwrap().len(), 1);
    assert!(export["data"]["sessions"][0].get("token_digest").is_none());
    assert!(export["truncated"].as_array().unwrap().is_empty());

    lifecycle::anonymize(&pool, user).await.unwrap();

    for e in REGISTRY {
        let sql = format!("SELECT count(*) FROM {} WHERE {} = $1", e.table, e.column);
        let left = count(&pool, &sql, user).await;
        match e.erase {
            Erase::Delete | Erase::Null => assert_eq!(left, 0, "{}.{} still holds the member", e.table, e.column),
            Erase::Scrub(columns) => {
                for c in columns {
                    let sql = format!("SELECT count(*) FROM {} WHERE {} = $1 AND {c} IS NOT NULL", e.table, e.column);
                    assert_eq!(count(&pool, &sql, user).await, 0, "{}.{c} was not scrubbed", e.table);
                }
            }
            Erase::Keep(_) => {}
        }
    }
    let (newsletter, moved): (bool, Option<String>) =
        sqlx::query_as("SELECT newsletter, ap_moved_to FROM users WHERE id = $1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!newsletter && moved.is_none());

    // The private key stays until the actor Delete queued for followers has been delivered.
    sqlx::query("INSERT INTO ap_deliveries (inbox, signer_type, signer_id, activity) VALUES ('https://r.example/inbox', 'user', $1, '{}')")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    lifecycle::purge_orphan_actor_keys(&pool).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM ap_keys WHERE owner_type = 'user' AND owner_id = $1", user).await, 1);
    sqlx::query("DELETE FROM ap_deliveries WHERE signer_type = 'user' AND signer_id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    lifecycle::purge_orphan_actor_keys(&pool).await.unwrap();
    assert_eq!(count(&pool, "SELECT count(*) FROM ap_keys WHERE owner_type = 'user' AND owner_id = $1", user).await, 0);
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn avatar_file_is_swept_after_deletion() {
    let pool = pool().await;
    let user = member(&pool, "avatar").await;
    let key = format!("avatar-{user}.png");
    let upload: i64 = sqlx::query_scalar(
        "INSERT INTO uploads (sha256, user_id, storage_key, filename, mime, size, created_at)
         VALUES ($1, $2, $3, 'a.png', 'image/png', 10, now() - interval '30 days') RETURNING id",
    )
    .bind(format!("{user:0>64}"))
    .bind(user)
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET avatar_upload_id = $2 WHERE id = $1")
        .bind(user)
        .bind(upload)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!maintenance::remove_orphan_uploads(&pool).await.unwrap().contains(&key), "in use while the avatar is set");
    lifecycle::anonymize(&pool, user).await.unwrap();
    assert!(maintenance::remove_orphan_uploads(&pool).await.unwrap().contains(&key), "unreferenced after deletion");
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn deletion_grace_cancel_and_protected_admin() {
    let pool = pool().await;
    let user = member(&pool, "grace").await;
    lifecycle::request_deletion(&pool, user, 14).await.unwrap();
    assert!(lifecycle::deletion_date(&pool, user).await.unwrap().is_some());
    assert!(!lifecycle::due_deletions(&pool).await.unwrap().contains(&user), "still in the grace period");
    sqlx::query("UPDATE users SET delete_after = now() - interval '1 minute' WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(lifecycle::due_deletions(&pool).await.unwrap().contains(&user), "grace over");
    lifecycle::cancel_deletion(&pool, user).await.unwrap();
    assert!(!lifecycle::due_deletions(&pool).await.unwrap().contains(&user), "cancelled");

    // Promoted during the grace period: skipped, never removed by the timer.
    sqlx::query("UPDATE users SET delete_after = now() - interval '1 minute', primary_group_id = 4 WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!lifecycle::due_deletions(&pool).await.unwrap().contains(&user));
    sqlx::query("UPDATE users SET primary_group_id = 2, delete_after = NULL WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn retention_jobs_clear_old_ips_and_expired_rows() {
    let pool = pool().await;
    let user = member(&pool, "retain").await;
    let old_session = format!("old-session-{user}");
    let new_session = format!("new-session-{user}");
    let expired_session = format!("expired-session-{user}");
    for d in [&old_session, &new_session, &expired_session] {
        bf_db::accounts::sessions::create_session(&pool, d, user.into(), Some("198.51.100.7".parse().unwrap()))
            .await
            .unwrap();
    }
    sqlx::query("UPDATE sessions SET last_used_at = now() - interval '400 days' WHERE token_digest = $1")
        .bind(&old_session)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE sessions SET expires_at = now() - interval '1 day' WHERE token_digest = $1")
        .bind(&expired_session)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO failed_logins (login, ip, created_at) VALUES ($1, '198.51.100.7', now() - interval '400 days')",
    )
    .bind(format!("retain{user}"))
    .execute(&pool)
    .await
    .unwrap();

    let cleared = maintenance::clear_old_request_ips(&pool, 180).await.unwrap();
    assert!(cleared >= 2, "the old session and the old failed sign-in lose their address");
    let ip_of = |digest: String| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, Option<sqlx::types::ipnetwork::IpNetwork>>(
                "SELECT ip FROM sessions WHERE token_digest = $1",
            )
            .bind(digest)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert!(ip_of(old_session.clone()).await.is_none());
    assert!(ip_of(new_session.clone()).await.is_some(), "recent addresses stay");

    maintenance::purge_operational(&pool, 90, 365).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE token_digest = $1")
        .bind(&expired_session)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "expired sessions are deleted");

    // Audit rows are append-only; the test backdates its own row the way the retention job may.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('bf.audit_maintenance', 'on', true)").execute(&mut *tx).await.unwrap();
    let id: i64 = sqlx::query_scalar("INSERT INTO audit_log (action, target, ip, created_at) VALUES ('test.retention', 'x', '198.51.100.7', now() - interval '1000 days') RETURNING id")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(audit::clear_old_ips(&pool, 180).await.unwrap() >= 1);
    let ip: Option<sqlx::types::ipnetwork::IpNetwork> =
        sqlx::query_scalar("SELECT ip FROM audit_log WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert!(ip.is_none());
    assert!(audit::prune(&pool, 365).await.unwrap() >= 1);
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();
    assert_eq!(rows, 0);

    // The idle sweep ends sessions unused for longer than the setting; 0 is off.
    assert_eq!(bf_db::accounts::sessions::expire_idle_sessions(&pool, 0).await.unwrap(), 0);
    bf_db::accounts::sessions::expire_idle_sessions(&pool, 60 * 24 * 100).await.unwrap();
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE token_digest = $1")
        .bind(&old_session)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "idle for 400 days");
}
