//! Regression tests for the authentication audit fixes. Needs Postgres:
//! `BF_DATABASE_URL=... cargo test -p bf-db --test auth_fixes -- --ignored --test-threads=1`.
//! Each test makes its own scratch database on that server and drops it afterwards, so nothing
//! in the database named by the URL is read or written.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_core::ids::UserId;
use bf_db::accounts::{security, sessions, users};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};
use std::net::IpAddr;
use std::str::FromStr;

struct Scratch {
    admin: PgPool,
    name: String,
    pool: PgPool,
}

impl Scratch {
    async fn new(tag: &str) -> Self {
        let url = std::env::var("BF_DATABASE_URL").expect("BF_DATABASE_URL must be set for --ignored tests");
        let base = PgConnectOptions::from_str(&url).expect("BF_DATABASE_URL must be a valid Postgres URL");
        let name = format!("qa_auth_{tag}_{}", std::process::id());
        let admin = PgPoolOptions::new().max_connections(1).connect_with(base.clone()).await.expect("connect");
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {name}")))
            .execute(&admin)
            .await
            .expect("drop");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}"))).execute(&admin).await.expect("create");
        let opts = base.database(&name).disable_statement_logging();
        let pool = PgPoolOptions::new().max_connections(16).connect_with(opts).await.expect("connect scratch");
        bf_db::migrate(&pool).await.expect("migrations must apply");
        Self { admin, name, pool }
    }

    async fn drop_db(self) {
        self.pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name)))
            .execute(&self.admin)
            .await
            .expect("drop scratch database");
    }
}

async fn member(pool: &PgPool, name: &str) -> UserId {
    users::create_user(pool, name, &format!("{name}@example.test"), "x").await.expect("create user")
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn concurrent_first_registrations_make_one_administrator() {
    let db = Scratch::new("first").await;
    let mut tasks = Vec::new();
    for i in 0..8 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            if i % 2 == 0 {
                users::create_user(&pool, &format!("u{i}"), &format!("u{i}@example.test"), "x").await
            } else {
                security::create_external_user(&pool, &format!("u{i}"), &format!("u{i}@example.test"), true).await
            }
        }));
    }
    for t in tasks {
        t.await.expect("task").expect("registration");
    }
    let admins: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE primary_group_id = 4").fetch_one(&db.pool).await.unwrap();
    assert_eq!(admins, 1, "only the first account is an administrator");
    let first_is_admin: bool = sqlx::query_scalar("SELECT primary_group_id = 4 FROM users ORDER BY id LIMIT 1")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(first_is_admin);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn failed_logins_lock_the_attacker_not_the_owner() {
    let db = Scratch::new("lock").await;
    let owner = member(&db.pool, "owner").await;
    let attacker: IpAddr = "203.0.113.9".parse().unwrap();
    let home: IpAddr = "198.51.100.7".parse().unwrap();
    assert_eq!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(attacker)).await.unwrap(), None);
    let mut locked = None;
    for _ in 0..5 {
        locked = security::record_failed_login(&db.pool, "owner", Some(owner), Some(attacker)).await.unwrap();
    }
    assert_eq!(locked, Some(15), "the fifth failure from one address locks that address");
    assert!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(attacker)).await.unwrap().is_some());
    assert_eq!(
        security::login_blocked_until(&db.pool, Some(owner), "owner", Some(home)).await.unwrap(),
        None,
        "the owner's own address is not locked out by someone else's failures"
    );
    assert!(security::locked_until(&db.pool, owner).await.unwrap().is_none(), "the account itself is not locked");

    // A success, a reset link or a sign-in link ends the run.
    security::clear_login_lockouts(&db.pool, owner).await.unwrap();
    assert_eq!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(attacker)).await.unwrap(), None);

    // Many addresses together do lock the account, for a bounded time, and clearing lifts it.
    for i in 0..30 {
        let ip: IpAddr = format!("192.0.2.{}", i + 1).parse().unwrap();
        security::record_failed_login(&db.pool, "owner", Some(owner), Some(ip)).await.unwrap();
    }
    let until = security::locked_until(&db.pool, owner).await.unwrap().expect("locked by 30 failures");
    assert!(until < chrono::Utc::now() + chrono::Duration::minutes(16), "the lock does not double");
    assert!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(home)).await.unwrap().is_some());
    security::clear_login_lockouts(&db.pool, owner).await.unwrap();
    assert_eq!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(home)).await.unwrap(), None);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn unknown_logins_are_limited_like_real_ones_and_stored_bounded() {
    let db = Scratch::new("unknown").await;
    let ip: IpAddr = "203.0.113.50".parse().unwrap();
    let long = "x".repeat(5000);
    for _ in 0..5 {
        security::record_failed_login(&db.pool, &long, None, Some(ip)).await.unwrap();
    }
    let stored: i32 =
        sqlx::query_scalar("SELECT max(length(login)) FROM failed_logins").fetch_one(&db.pool).await.unwrap();
    assert_eq!(stored as usize, security::LOGIN_MAX_CHARS);
    let typed: String = long.chars().take(security::LOGIN_MAX_CHARS).collect();
    assert!(
        security::login_blocked_until(&db.pool, None, &typed, Some(ip)).await.unwrap().is_some(),
        "a name nobody holds shows the same lock after five failures"
    );
    assert_eq!(security::login_blocked_until(&db.pool, None, "someone-else", Some(ip)).await.unwrap(), None);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn ipv6_failures_count_per_64() {
    let db = Scratch::new("v6").await;
    let owner = member(&db.pool, "owner").await;
    for i in 0..5 {
        let ip: IpAddr = format!("2001:db8:1:2::{i:x}").parse().unwrap();
        security::record_failed_login(&db.pool, "owner", Some(owner), Some(ip)).await.unwrap();
    }
    let sibling: IpAddr = "2001:db8:1:2:ffff::1".parse().unwrap();
    let other_net: IpAddr = "2001:db8:1:3::1".parse().unwrap();
    assert!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(sibling)).await.unwrap().is_some());
    assert_eq!(security::login_blocked_until(&db.pool, Some(owner), "owner", Some(other_net)).await.unwrap(), None);

    // The per-address cap across accounts is per /64 too.
    for i in 0..50 {
        let ip: IpAddr = format!("2001:db8:9:9::{:x}", i + 1).parse().unwrap();
        security::record_failed_login(&db.pool, "nobody", None, Some(ip)).await.unwrap();
    }
    assert!(security::ip_login_blocked(&db.pool, "2001:db8:9:9:1::1".parse().unwrap()).await.unwrap());
    assert!(!security::ip_login_blocked(&db.pool, "2001:db8:9:8::1".parse().unwrap()).await.unwrap());
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn parallel_second_factor_guesses_are_capped_by_the_lock() {
    let db = Scratch::new("mfa").await;
    let owner = member(&db.pool, "owner").await;
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move { security::begin_mfa_attempt(&pool, owner).await.unwrap() }));
    }
    let mut allowed = 0;
    for t in tasks {
        allowed += usize::from(t.await.unwrap());
    }
    assert_eq!(allowed, 5, "five guesses get through, however many arrive at once");
    assert!(security::mfa_locked(&db.pool, owner).await.unwrap());
    assert!(!security::begin_mfa_attempt(&db.pool, owner).await.unwrap());
    security::clear_mfa_failures(&db.pool, owner).await.unwrap();
    assert!(security::begin_mfa_attempt(&db.pool, owner).await.unwrap());
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn new_email_links_retire_the_oldest_instead_of_being_refused() {
    let db = Scratch::new("tokens").await;
    let owner = member(&db.pool, "owner").await;
    for i in 0..7 {
        security::create_email_token(&db.pool, &format!("digest{i}"), owner, "reset", None, 60 + i).await.unwrap();
    }
    let live: i64 = sqlx::query_scalar("SELECT count(*) FROM email_tokens WHERE used_at IS NULL")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(live, 5);
    assert!(security::consume_email_token(&db.pool, "digest6", "reset").await.unwrap().is_some(), "newest works");
    assert!(security::consume_email_token(&db.pool, "digest0", "reset").await.unwrap().is_none(), "oldest retired");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn an_impersonation_session_never_has_sudo() {
    let db = Scratch::new("imp").await;
    let target = member(&db.pool, "target").await;
    sessions::create_session(&db.pool, "plain", target, None).await.unwrap();
    let plain = sessions::session_user(&db.pool, "plain").await.unwrap().unwrap();
    assert!(plain.sudo && !plain.impersonated);

    let admin = member(&db.pool, "admin_x").await;
    sessions::create_impersonation_session(&db.pool, "imp", target, admin.get(), None).await.unwrap();
    let imp = sessions::session_user(&db.pool, "imp").await.unwrap().unwrap();
    assert!(imp.impersonated && !imp.sudo, "no sudo from the row, whatever cookies the browser holds");
    sessions::confirm_session(&db.pool, "imp", "imp2").await.unwrap();
    assert!(sessions::session_user(&db.pool, "imp").await.unwrap().is_some(), "confirming did not move it");
    assert!(sessions::session_user(&db.pool, "imp2").await.unwrap().is_none());
    db.drop_db().await;
}
