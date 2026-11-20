//! Regression tests for the security and logic fixes in the database layer. Needs Postgres:
//! `BF_DATABASE_URL=... cargo test -p bf-db --test hardening -- --ignored --test-threads=1`.
//! Each test makes its own scratch database on that server and drops it afterwards.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_core::ids::UserId;
use bf_db::accounts::users::{self, AccountKind, NewUser};
use bf_db::admin::audit::{self, Entry};
use bf_db::community::groups::{self, Grant};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};
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
        let name = format!("qa_hard_{tag}_{}", std::process::id());
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

/// The first account of an empty database is the administrator, so call this first.
async fn admin(pool: &PgPool, name: &str) -> UserId {
    let id = users::create_user(pool, name, &format!("{name}@example.test"), "x").await.expect("create admin");
    let group: i64 = sqlx::query_scalar("SELECT primary_group_id FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("group");
    assert_eq!(group, 4, "the first account is an administrator");
    id
}

async fn member(pool: &PgPool, name: &str) -> UserId {
    users::create_user(pool, name, &format!("{name}@example.test"), "x").await.expect("create user")
}

/// Makes a member through `insert_user`, returning the name it got.
async fn insert(pool: &PgPool, name: &str, email: &str, fit_name: bool) -> Result<String, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let (_, got) = users::insert_user(
        &mut tx,
        &NewUser {
            username: name,
            email,
            password_hash: Some("x"),
            display_name: None,
            primary_group: Some(2),
            joined_at: None,
            verified: false,
            kind: AccountKind::Member,
            fit_name,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(got)
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn fit_name_appends_a_number_when_the_name_is_taken() {
    let db = Scratch::new("fit_taken").await;
    admin(&db.pool, "boss").await;
    assert_eq!(insert(&db.pool, "someone", "a1@example.test", false).await.unwrap(), "someone");
    let got = insert(&db.pool, "someone", "a2@example.test", true).await.unwrap();
    assert_eq!(got, "someone1");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn fit_name_steps_past_a_lookalike_skeleton() {
    let db = Scratch::new("fit_look").await;
    admin(&db.pool, "boss").await;
    insert(&db.pool, "hello", "a1@example.test", false).await.unwrap();
    // "he1lo" has the same UTS #39 skeleton as "hello" (1 reads as l).
    let got = insert(&db.pool, "he1lo", "a2@example.test", true).await.unwrap();
    assert_eq!(got, "he1lo1");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn without_fit_name_a_lookalike_is_refused_as_skeleton_taken() {
    let db = Scratch::new("nofit").await;
    admin(&db.pool, "boss").await;
    insert(&db.pool, "hello", "a1@example.test", false).await.unwrap();
    let err = insert(&db.pool, "he1lo", "a2@example.test", false).await.expect_err("look-alike refused");
    assert!(users::is_skeleton_taken(&err), "{err:?}");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn raw_insert_of_a_local_user_without_a_skeleton_is_refused_by_the_trigger() {
    let db = Scratch::new("trigger").await;
    let raw = sqlx::query(
        "INSERT INTO users (username, username_lower, email, email_lower, password_hash, primary_group_id)
         VALUES ('Raw', 'raw', 'raw@example.test', 'raw@example.test', 'x', 2)",
    )
    .execute(&db.pool)
    .await;
    assert!(raw.is_err(), "a local user without a username skeleton must be refused");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE username_lower = 'raw'")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn automatic_grant_cannot_reach_the_administrators_group() {
    let db = Scratch::new("grant").await;
    admin(&db.pool, "boss").await;
    let target = member(&db.pool, "target").await.get();
    let mut tx = db.pool.begin().await.unwrap();
    let refused = groups::add_members(&mut tx, &[target], 4, Grant::Automatic).await;
    assert!(refused.is_err(), "Automatic into group 4 must fail");
    drop(tx);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_secondary_groups WHERE user_id = $1 AND group_id = 4")
        .bind(target)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn automatic_grant_works_for_an_ordinary_group_and_staff_grant_for_group_4() {
    let db = Scratch::new("grant_ok").await;
    admin(&db.pool, "boss").await;
    let target = member(&db.pool, "target").await.get();
    let plain: i64 = sqlx::query_scalar("INSERT INTO groups (name, slug) VALUES ('Plain', 'plain') RETURNING id")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    assert_eq!(groups::add_members(&mut tx, &[target], plain, Grant::Automatic).await.unwrap(), vec![target]);
    assert_eq!(groups::add_members(&mut tx, &[target], 4, Grant::Staff).await.unwrap(), vec![target]);
    tx.commit().await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM user_secondary_groups WHERE user_id = $1")
        .bind(target)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn display_name_impersonating_an_administrator_is_detected() {
    let db = Scratch::new("impersonate").await;
    admin(&db.pool, "boss").await;
    // "b0ss" has the skeleton of the administrator "boss".
    assert!(users::display_name_impersonates(&db.pool, "b0ss", None).await.unwrap());
    assert!(users::display_name_impersonates(&db.pool, "Admin", None).await.unwrap(), "reserved name");
    assert!(!users::display_name_impersonates(&db.pool, "Gardener", None).await.unwrap());
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn audit_record_writes_actor_target_details_and_ip() {
    let db = Scratch::new("audit").await;
    let actor = admin(&db.pool, "boss").await.get();
    let ip: std::net::IpAddr = "203.0.113.9".parse().unwrap();
    audit::record(
        &db.pool,
        Entry::new("user.test")
            .by(actor)
            .on("user:42")
            .details(serde_json::json!({"why": "because"}))
            .from_ip(Some(ip)),
    )
    .await
    .unwrap();
    let (a, target, details, got_ip): (Option<i64>, Option<String>, serde_json::Value, Option<String>) =
        sqlx::query_as("SELECT actor_id, target, details, host(ip) FROM audit_log WHERE action = 'user.test'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(a, Some(actor));
    assert_eq!(target.as_deref(), Some("user:42"));
    assert_eq!(details["why"], "because");
    assert_eq!(got_ip.as_deref(), Some("203.0.113.9"));
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn concurrent_votes_on_a_single_choice_poll_leave_one_ballot() {
    use bf_db::community::polls;
    let db = Scratch::new("poll").await;
    admin(&db.pool, "boss").await;
    let voter = member(&db.pool, "voter").await.get();
    let node: i64 = sqlx::query_scalar("INSERT INTO nodes (kind, slug, title) VALUES ('forum', 'f', 'F') RETURNING id")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let item: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, title, slug, state) VALUES ($1, 't', 't', 'visible') RETURNING id",
    )
    .bind(node)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let poll_id: i64 =
        sqlx::query_scalar("INSERT INTO polls (item_id, question, max_choices) VALUES ($1, 'q', 1) RETURNING id")
            .bind(item)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let mut options = Vec::new();
    for (i, label) in ["a", "b"].iter().enumerate() {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO poll_options (item_id, poll_id, position, label) VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(item)
        .bind(poll_id)
        .bind(i as i32)
        .bind(*label)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        options.push(id);
    }
    let poll = polls::Poll {
        id: poll_id,
        item_id: item,
        question: "q".into(),
        max_choices: 1,
        closes_at: None,
        results: "always".into(),
        voters: 0,
        kind: "choice".into(),
        is_public: false,
        vote_change: true,
        min_trust_level: 0,
        post_number: None,
        groups: vec![],
        group_names: vec![],
    };
    for _ in 0..5 {
        let (a, b) = tokio::join!(
            polls::vote(&db.pool, &poll, &options[..1], voter),
            polls::vote(&db.pool, &poll, &options[1..], voter)
        );
        a.unwrap();
        b.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM poll_votes WHERE poll_id = $1 AND user_id = $2")
            .bind(poll_id)
            .bind(voter)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "a single-choice poll holds one ballot per member");
    }
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn mandatory_notification_reaches_a_recipient_who_ignores_the_sender() {
    use bf_db::community::notifications;
    assert!(bf_core::notifications::is_mandatory("suspended"));
    assert!(!bf_core::notifications::is_mandatory("mention"));
    let db = Scratch::new("mandatory").await;
    admin(&db.pool, "boss").await;
    let sender = member(&db.pool, "sender").await.get();
    let recipient = member(&db.pool, "recipient").await.get();
    sqlx::query("INSERT INTO ignored_users (user_id, ignored_user_id) VALUES ($1, $2)")
        .bind(recipient)
        .bind(sender)
        .execute(&db.pool)
        .await
        .unwrap();
    let data = serde_json::json!({});
    let got = notifications::deliver(&db.pool, "http://x.test", sender, &[recipient], "suspended", &data, "s", "b", 0)
        .await
        .unwrap();
    assert_eq!(got, vec![recipient], "a mandatory kind ignores the ignore list");
    let got = notifications::deliver(&db.pool, "http://x.test", sender, &[recipient], "mention", &data, "s", "b", 0)
        .await
        .unwrap();
    assert!(got.is_empty(), "an ordinary kind honours the ignore list");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn save_draft_keeps_at_most_fifty_drafts_per_user() {
    use bf_db::forum::uploads;
    let db = Scratch::new("drafts").await;
    let user = admin(&db.pool, "boss").await.get();
    for i in 0..55 {
        uploads::save_draft(&db.pool, user, &format!("ctx{i}"), &serde_json::json!({ "i": i })).await.unwrap();
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM drafts WHERE user_id = $1")
        .bind(user)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 50);
    assert!(uploads::draft(&db.pool, user, "ctx54").await.unwrap().is_some(), "the latest draft survives");
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn unsubscribe_without_a_pref_row_turns_email_off_and_keeps_push_and_app() {
    use bf_db::platform::mail;
    let db = Scratch::new("unsub").await;
    let user = admin(&db.pool, "boss").await.get();
    mail::unsubscribe(&db.pool, user, "mention").await.unwrap();
    let channels: i16 =
        sqlx::query_scalar("SELECT channels FROM notification_prefs WHERE user_id = $1 AND kind = 'mention'")
            .bind(user)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(channels & bf_core::notifications::EMAIL, 0, "email is off");
    assert_ne!(channels & bf_core::notifications::PUSH, 0, "push stays on");
    assert_ne!(channels & bf_core::notifications::APP, 0, "the app channel stays on");
    db.drop_db().await;
}
