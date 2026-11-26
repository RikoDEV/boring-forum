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

#[tokio::test]
#[ignore = "needs Postgres"]
async fn held_mail_about_a_removed_post_is_not_sent() {
    let db = Scratch::new("removed").await;
    let author = users::create_user(&db.pool, "ann", "ann@example.test", "x").await.unwrap();
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
    for (number, state) in [(1_i32, "visible"), (2, "deleted")] {
        sqlx::query(
            "INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version, state)
             VALUES ($1, $2, $3, 'words', '<p>words</p>', 1, $4)",
        )
        .bind(topic)
        .bind(author)
        .bind(number)
        .bind(state)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO mail_outbox (to_addr, subject, body, not_before, unless_read_user, unless_read_url)
             VALUES ('b@example.test', $1, '> words', now() + interval '1 hour', $2, $3)",
        )
        .bind(format!("post {number}"))
        .bind(author)
        .bind(format!("/t/t/{topic}#post-{number}"))
        .execute(&db.pool)
        .await
        .unwrap();
    }
    assert_eq!(bf_db::community::notifications::visible_post_raw(&db.pool, topic, 1).await.unwrap().unwrap(), "words");
    assert_eq!(bf_db::community::notifications::visible_post_raw(&db.pool, topic, 2).await.unwrap(), None);
    mail::skip_mail_to_bad_addresses(&db.pool).await.unwrap();
    let waiting: Vec<String> =
        sqlx::query_scalar("SELECT subject FROM mail_outbox WHERE sent_at IS NULL").fetch_all(&db.pool).await.unwrap();
    assert_eq!(waiting, ["post 1"]);
    db.drop_db().await;
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn summary_cards_count_only_what_the_member_may_see() {
    use bf_db::platform::digests;
    let db = Scratch::new("cards").await;
    let reader = users::create_user(&db.pool, "reader", "r@example.test", "x").await.unwrap().get();
    let ann = users::create_user(&db.pool, "ann", "a@example.test", "x").await.unwrap().get();
    let troll = users::create_user(&db.pool, "troll", "t@example.test", "x").await.unwrap().get();
    sqlx::query("INSERT INTO ignored_users (user_id, ignored_user_id) VALUES ($1, $2)")
        .bind(reader)
        .bind(troll)
        .execute(&db.pool)
        .await
        .unwrap();
    let node: i64 =
        sqlx::query_scalar("INSERT INTO nodes (kind, slug, title) VALUES ('forum', 'f', 'Guides') RETURNING id")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let mut topics = Vec::new();
    for (title, author) in [("one", ann), ("two", ann), ("by troll", troll)] {
        let topic: i64 = sqlx::query_scalar(
            "INSERT INTO items (node_id, author_id, title, slug, state, like_count, reply_count)
             VALUES ($1, $2, $3, 's', 'visible', 2, 1) RETURNING id",
        )
        .bind(node)
        .bind(author)
        .bind(title)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        topics.push(topic);
    }
    let mut post_ids = Vec::new();
    for (topic, author, number, raw) in [
        (topics[0], ann, 1, "first"),
        (topics[0], reader, 2, "mine"),
        (topics[0], troll, 3, "noise"),
        (topics[1], ann, 1, "second"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO posts (item_id, author_id, post_number, raw, cooked, cooked_version)
             VALUES ($1, $2, $3, $4, '', 1) RETURNING id",
        )
        .bind(topic)
        .bind(author)
        .bind(number)
        .bind(raw)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        post_ids.push(id);
    }
    // A like on the reader's own reply counts for them; their like on another's post does not.
    for (post, user) in [(post_ids[1], ann), (post_ids[0], reader)] {
        sqlx::query("INSERT INTO post_likes (post_id, user_id) VALUES ($1, $2)")
            .bind(post)
            .bind(user)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let since = chrono::Utc::now() - chrono::Duration::days(1);
    assert_eq!(digests::digest_new_count(&db.pool, reader, &[node], since).await.unwrap(), 2, "not the ignored one");
    assert_eq!(digests::likes_received(&db.pool, reader, since).await.unwrap(), 1);
    // In the order asked; a topic with no visible first post has no card.
    let cards = digests::digest_cards(&db.pool, &[topics[1], topics[2], topics[0]]).await.unwrap();
    assert_eq!(cards.iter().map(|c| c.title.as_str()).collect::<Vec<_>>(), ["two", "one"]);
    assert_eq!(
        (cards[1].forum.as_str(), cards[1].author.as_deref(), cards[1].raw.as_str()),
        ("Guides", Some("ann"), "first")
    );
    let people = digests::digest_participants(&db.pool, reader, &topics, 5).await.unwrap();
    assert_eq!(people, [(topics[0], "reader".to_owned())], "repliers, without the ignored one");
    assert!(!digests::uploaded_by(&db.pool, "nothing.png", ann).await.unwrap());
    // The summary travels with its queued mail.
    let summary = serde_json::json!({ "new_topics": 2 });
    digests::mark_digest_sent(&db.pool, "r@example.test", "s", Some("b"), "https://f.test/u", Some(&summary))
        .await
        .unwrap();
    assert_eq!(mail::unsent_mail(&db.pool, 5).await.unwrap()[0].summary, Some(summary));
    db.drop_db().await;
}
