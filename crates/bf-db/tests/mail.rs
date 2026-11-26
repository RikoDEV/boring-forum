//! The mail queue and notification mail. Needs Postgres:
//! `BF_DATABASE_URL=... cargo test -p bf-db --test mail -- --ignored --test-threads=1`.
//! Each test makes its own scratch database on that server and drops it afterwards.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_db::accounts::users;
use bf_db::community::notifications;
use bf_db::platform::mail::{self, MAX_ATTEMPTS, RETRY_MINUTES};
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
        let name = format!("qa_mail_{tag}_{}", std::process::id());
        let admin = PgPoolOptions::new().max_connections(1).connect_with(base.clone()).await.expect("connect");
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE IF EXISTS {name}")))
            .execute(&admin)
            .await
            .expect("drop");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}"))).execute(&admin).await.expect("create");
        let opts = base.database(&name).disable_statement_logging();
        let pool = PgPoolOptions::new().max_connections(4).connect_with(opts).await.expect("connect scratch");
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

/// (attempts, minutes until the next try, rounded) of the only queued mail.
async fn state(pool: &PgPool) -> (i16, i32) {
    sqlx::query_as("SELECT attempts, round(extract(epoch FROM not_before - now()) / 60)::int FROM mail_outbox")
        .fetch_one(pool)
        .await
        .expect("one mail")
}

async fn make_due(pool: &PgPool) {
    sqlx::query("UPDATE mail_outbox SET not_before = now() - interval '1 second'").execute(pool).await.expect("due");
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn failed_mail_is_retried_for_two_days_and_then_given_up() {
    let db = Scratch::new("retry").await;
    mail::queue_mail(&db.pool, "a@example.test", "s", "b").await.unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM mail_outbox").fetch_one(&db.pool).await.unwrap();
    for (tried, wait) in RETRY_MINUTES.iter().enumerate() {
        assert_eq!(mail::unsent_mail(&db.pool, 5).await.unwrap().len(), 1, "try {tried}");
        mail::mail_failed(&db.pool, id, "down", false).await.unwrap();
        assert_eq!(state(&db.pool).await, (i16::try_from(tried).unwrap() + 1, *wait));
        make_due(&db.pool).await;
    }
    assert!(RETRY_MINUTES.iter().sum::<i32>() >= 45 * 60, "about two days");
    // The last try fails too: nothing is left to send.
    assert_eq!(mail::unsent_mail(&db.pool, 5).await.unwrap().len(), 1);
    mail::mail_failed(&db.pool, id, "down", false).await.unwrap();
    make_due(&db.pool).await;
    assert!(mail::unsent_mail(&db.pool, 5).await.unwrap().is_empty());
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn an_address_that_cannot_be_read_is_tried_once() {
    let db = Scratch::new("permanent").await;
    mail::queue_mail(&db.pool, "not an address", "s", "b").await.unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM mail_outbox").fetch_one(&db.pool).await.unwrap();
    mail::mail_failed(&db.pool, id, "bad address", true).await.unwrap();
    assert_eq!(state(&db.pool).await.0, MAX_ATTEMPTS);
    make_due(&db.pool).await;
    assert!(mail::unsent_mail(&db.pool, 5).await.unwrap().is_empty());
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn the_contact_form_cap_counts_its_own_mail_whatever_the_subject() {
    let db = Scratch::new("contact").await;
    mail::queue_contact_mail(&db.pool, "staff@example.test", "Formularz kontaktowy - ANYForum", "x").await.unwrap();
    mail::queue_mail(&db.pool, "a@example.test", "Contact form message", "not from the form").await.unwrap();
    assert_eq!(mail::contact_mails_last_hour(&db.pool).await.unwrap(), 1);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn notification_mail_has_a_translated_footer_and_account_notices_have_none() {
    let db = Scratch::new("footer").await;
    let from = users::create_user(&db.pool, "boss", "boss@example.test", "x").await.unwrap();
    let to = users::create_user(&db.pool, "ola", "ola@example.test", "x").await.unwrap();
    sqlx::query("UPDATE users SET email_verified_at = now(), approved = true, locale = 'pl' WHERE id = $1")
        .bind(to)
        .execute(&db.pool)
        .await
        .unwrap();
    let node: i64 = sqlx::query_scalar("INSERT INTO nodes (kind, slug, title) VALUES ('forum', 'f', 'F') RETURNING id")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let topic: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, title, slug, state) VALUES ($1, 't', 't', 'visible') RETURNING id",
    )
    .bind(node)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mandatory = bf_core::notifications::KINDS.iter().find(|k| k.mandatory && k.email).expect("a kind").key;
    let optional = bf_core::notifications::KINDS.iter().find(|k| !k.mandatory && k.email).expect("a kind").key;
    for kind in [optional, mandatory] {
        let data = serde_json::json!({ "url": format!("/t/t/{topic}"), "topic_id": topic });
        notifications::deliver(&db.pool, "https://f.test", from.get(), &[to.get()], kind, &data, kind, "Body", 0)
            .await
            .unwrap();
    }
    let sent = |subject: &'static str| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT body, unsubscribe_url FROM mail_outbox WHERE subject = $1",
            )
            .bind(subject)
            .fetch_one(&pool)
            .await
            .expect("queued")
        }
    };
    let pl = bf_core::i18n::T::new("pl");
    let (body, url) = sent(optional).await;
    let url = url.expect("a one-click link");
    assert_eq!(
        body,
        format!(
            "Body\n{} {url}\n{} {url}/topic",
            pl.msg("To stop these emails, open:"),
            pl.msg("To mute this topic, open:")
        )
    );
    assert_ne!(pl.msg("To stop these emails, open:"), "To stop these emails, open:", "translated");
    assert_eq!(sent(mandatory).await, ("Body".to_owned(), None));
    db.drop_db().await;
}
