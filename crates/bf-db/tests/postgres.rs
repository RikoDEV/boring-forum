//! Needs Postgres: `BF_DATABASE_URL=... cargo test -p bf-db -- --ignored --test-threads=1`.
//! CI runs these against a service container; locally `tools/devdb` (PGlite) works.

#![expect(clippy::expect_used, reason = "test setup: a missing database or failed migration is the failure report")]

use bf_core::ids::{ForumId, GroupId};
use bf_db::accounts::{sessions, users};
use bf_db::admin::members;
use bf_db::community::notifications;
use bf_db::forum::{nodes, posts, reading, search, topics};
use bf_db::moderation::{review, watched_words};
use bf_db::platform::mail;
use serde_json::json;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

async fn pool() -> sqlx::PgPool {
    let url = std::env::var("BF_DATABASE_URL").expect("BF_DATABASE_URL must be set for --ignored tests");
    // PGlite runs all connections on one backend, so named prepared statements from
    // different connections collide; unnamed statements work on both it and Postgres.
    let opts = PgConnectOptions::from_str(&url)
        .expect("BF_DATABASE_URL must be a valid Postgres URL")
        .statement_cache_capacity(0);
    let pool = PgPoolOptions::new()
        .max_connections(if std::env::var("BF_DEVDB").is_ok() { 1 } else { 4 })
        .connect_with(opts)
        .await
        .expect("the test database must accept connections");
    bf_db::migrate(&pool).await.expect("migrations must apply to the test database");
    pool
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn migrations_are_idempotent() {
    let pool = pool().await;
    bf_db::migrate(&pool).await.unwrap();
    bf_db::ping(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn forum_flow_counters_revisions_search() {
    use bf_core::perms::Perms;

    let pool = pool().await;
    let tag = format!("t{}", std::process::id());
    let uid = users::create_user(&pool, &tag, &format!("{tag}@example.com"), "x").await.unwrap();
    let user = users::user_by_login(&pool, &tag.to_uppercase()).await.unwrap().unwrap();
    assert_eq!(user.id, uid);
    assert!(user.groups.contains(&user.primary_group_id));

    let digest = bf_core::auth::token_digest(&tag);
    sessions::create_session(&pool, &digest, uid, None).await.unwrap();
    assert_eq!(sessions::session_user(&pool, &digest).await.unwrap().unwrap().id, uid);

    let cat = nodes::create_node(&pool, None, "category", "Cat", "cat", "", &[(1, Perms::VIEW)]).await.unwrap();
    let forum = nodes::create_node(&pool, Some(cat), "forum", "Forum", "forum", "", &[(1, Perms::ALL)]).await.unwrap();
    assert_eq!(nodes::node_grants(&pool).await.unwrap()[&ForumId(forum)][&GroupId(1)], Perms::ALL);

    let post = |raw: &'static str| posts::NewPost {
        author_id: uid.get(),
        raw,
        cooked: raw,
        cooked_version: 1,
        ip: None,
        pending: false,
    };
    let item =
        posts::create_topic(&pool, forum, "Hello zebra", "hello-zebra", &post("first zebra post")).await.unwrap();
    assert_eq!(posts::create_reply(&pool, item, forum, &post("second")).await.unwrap(), 2);
    assert_eq!(posts::create_reply(&pool, item, forum, &post("third")).await.unwrap(), 3);

    let t = topics::topic(&pool, item).await.unwrap().unwrap();
    assert_eq!(t.reply_count, 2);
    let node = nodes::nodes(&pool).await.unwrap().into_iter().find(|n| n.id == forum).unwrap();
    assert_eq!((node.topic_count, node.post_count), (1, 3));

    let page = posts::topic_posts(&pool, item, 2, 10, false, None).await.unwrap();
    assert_eq!(page.iter().map(|p| p.post_number).collect::<Vec<_>>(), [2, 3]);

    let second = page[0].id;
    posts::edit_post(&pool, second, uid.get(), "second, edited", "second, edited", 1, Some("typo")).await.unwrap();
    let revs: i64 = sqlx::query_scalar("SELECT count(*) FROM post_revisions WHERE post_id = $1")
        .bind(second)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(revs, 1);

    posts::soft_delete_post(&pool, second, uid.get()).await.unwrap();
    let t = topics::topic(&pool, item).await.unwrap().unwrap();
    assert_eq!(t.reply_count, 1, "deleting a reply recounts the topic");
    let node = nodes::nodes(&pool).await.unwrap().into_iter().find(|n| n.id == forum).unwrap();
    assert_eq!(node.post_count, 2, "and the forum");
    assert_eq!(posts::topic_posts(&pool, item, 1, 10, false, None).await.unwrap().len(), 2);
    assert_eq!(posts::topic_posts(&pool, item, 1, 10, true, None).await.unwrap().len(), 3);

    topics::set_topic_flag(&pool, item, topics::TopicFlag::Locked, true, uid.get()).await.unwrap();
    assert!(topics::topic(&pool, item).await.unwrap().unwrap().locked);

    let hits = search::search(&pool, "zebra", &[forum], &search::SearchFilter::default(), 10, 0).await.unwrap();
    assert_eq!(hits.first().map(|h| h.item_id), Some(item));
    assert!(search::search(&pool, "zebra", &[], &search::SearchFilter::default(), 10, 0).await.unwrap().is_empty());

    reading::mark_read(&pool, uid.get(), item, 3).await.unwrap();
    reading::mark_read(&pool, uid.get(), item, 1).await.unwrap();
    let read: i32 =
        sqlx::query_scalar("SELECT last_read_post_number FROM read_markers WHERE user_id = $1 AND item_id = $2")
            .bind(uid)
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(read, 3, "read markers never move backwards");

    let p = posts::post_ref(&pool, page[1].id).await.unwrap().unwrap();
    assert_eq!(review::report(&pool, uid.get(), &p, "spam", None, 1.0).await.unwrap(), 1.0);
    assert_eq!(
        review::report(&pool, uid.get(), &p, "off_topic", None, 1.0).await.unwrap(),
        1.0,
        "repeat report does not stack"
    );
    let other = users::create_user(&pool, &format!("{tag}x"), &format!("{tag}x@example.com"), "x").await.unwrap();
    review::report(&pool, other.get(), &p, "spam", Some("ads"), 1.0).await.unwrap();
    let score: f32 = sqlx::query_scalar("SELECT score FROM reviewables WHERE target_type = 'post' AND target_id = $1")
        .bind(p.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(score, 2.0);

    notifications::notify(&pool, &[uid.get()], "mention", &json!({"by": "x"})).await.unwrap();
    assert_eq!(notifications::notifications(&pool, uid.get()).await.unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn settings_round_trip_and_notify() {
    let pool = pool().await;
    // tools/devdb (PGlite) has one backend, so cross-connection NOTIFY cannot be observed there.
    let devdb = std::env::var("BF_DEVDB").is_ok_and(|v| v == "1");
    let mut listener = if devdb {
        None
    } else {
        let mut l = sqlx::postgres::PgListener::connect_with(&pool).await.unwrap();
        l.listen("bf_settings").await.unwrap();
        Some(l)
    };

    bf_db::settings::save(&pool, "topics_per_page", &json!(42)).await.unwrap();
    assert_eq!(bf_db::settings::load(&pool).await.unwrap().int("topics_per_page"), 42);

    if let Some(l) = listener.as_mut() {
        let note = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv()).await.unwrap().unwrap();
        assert_eq!(note.payload(), "topics_per_page");
    }

    sqlx::query("DELETE FROM site_settings WHERE key = 'topics_per_page'").execute(&pool).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn pending_posts_review_unread_and_views() {
    use bf_core::perms::Perms;

    let pool = pool().await;
    let tag = format!("r{}", std::process::id());
    let mk = |n: &str| format!("{tag}{n}");
    let author = users::create_user(&pool, &mk("a"), &format!("{}@example.com", mk("a")), "x").await.unwrap();
    let reader = users::create_user(&pool, &mk("b"), &format!("{}@example.com", mk("b")), "x").await.unwrap();
    let forum = nodes::create_node(&pool, None, "forum", "Queue", "queue", "", &[(2, Perms::ALL)]).await.unwrap();
    let post = |raw: &'static str, pending| posts::NewPost {
        author_id: author.get(),
        raw,
        cooked: raw,
        cooked_version: 1,
        ip: None,
        pending,
    };

    // A pending topic is invisible and uncounted until approved.
    let item = posts::create_topic(&pool, forum, "Held", "held", &post("held body", true)).await.unwrap();
    let t = topics::topic(&pool, item).await.unwrap().unwrap();
    assert_eq!(t.state, "pending");
    let count = |pool: sqlx::PgPool| async move {
        nodes::nodes(&pool).await.unwrap().into_iter().find(|n| n.id == forum).unwrap().topic_count
    };
    assert_eq!(count(pool.clone()).await, 0);
    let queue = review::review_queue(&pool, &[forum], None, false).await.unwrap();
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].kind, "pending_post");
    assert_eq!(review::reviewable_node(&pool, queue[0].id).await.unwrap(), Some(forum));
    review::resolve_review(&pool, queue[0].id, review::Decision::Keep, author.get()).await.unwrap();
    assert_eq!(topics::topic(&pool, item).await.unwrap().unwrap().state, "visible");
    assert_eq!(count(pool.clone()).await, 1);
    assert!(review::review_queue(&pool, &[forum], None, false).await.unwrap().is_empty());

    // A rejected pending reply never counts.
    posts::create_reply(&pool, item, forum, &post("spam reply", true)).await.unwrap();
    let r = review::review_queue(&pool, &[forum], None, false).await.unwrap().remove(0);
    review::resolve_review(&pool, r.id, review::Decision::Remove, author.get()).await.unwrap();
    assert_eq!(topics::topic(&pool, item).await.unwrap().unwrap().reply_count, 0);

    // Unread: a new visible reply shows as unread for the reader until read or marked.
    posts::create_reply(&pool, item, forum, &post("real reply", false)).await.unwrap();
    let unread = |pool: sqlx::PgPool| async move {
        topics::latest_topics(&pool, &[forum], Some(reader.get()), true, 10).await.unwrap().len()
    };
    assert_eq!(unread(pool.clone()).await, 1, "topic created after the reader joined is unread");
    let row = topics::forum_topics(
        &pool,
        forum,
        topics::Sort::default(),
        topics::TopicFilter::default(),
        Some(reader.get()),
        10,
        0,
    )
    .await
    .unwrap()
    .remove(0);
    assert_eq!(row.unread(), Some(row.highest_post_number));
    reading::mark_read(&pool, reader.get(), item, row.highest_post_number).await.unwrap();
    assert_eq!(unread(pool.clone()).await, 0);
    posts::create_reply(&pool, item, forum, &post("another reply", false)).await.unwrap();
    assert_eq!(unread(pool.clone()).await, 1);
    nodes::mark_nodes_read(&pool, reader.get(), &[forum]).await.unwrap();
    assert_eq!(unread(pool.clone()).await, 0, "mark forum read clears it");
    let guest = topics::latest_topics(&pool, &[forum], None, false, 10).await.unwrap().remove(0);
    assert_eq!(guest.unread(), None, "guests have no unread state");

    // Batched views.
    reading::add_views(&pool, &[item], &[3]).await.unwrap();
    reading::add_views(&pool, &[item], &[2]).await.unwrap();
    assert_eq!(topics::topic(&pool, item).await.unwrap().unwrap().view_count, 5);

    // Re-cook job queries: old cooker versions are found and replaced.
    sqlx::query("UPDATE posts SET cooked_version = 0 WHERE item_id = $1").bind(item).execute(&pool).await.unwrap();
    let stale = posts::stale_cooked(&pool, 1, 100).await.unwrap();
    assert!(!stale.is_empty());
    let (ids, raws): (Vec<i64>, Vec<String>) = stale.into_iter().unzip();
    let html: Vec<String> = raws.iter().map(|r| format!("<p>{r}</p>")).collect();
    posts::save_cooked(&pool, &ids, &html, 1).await.unwrap();
    assert!(posts::stale_cooked(&pool, 1, 100).await.unwrap().is_empty());

    // Digests: due after a day, topics limited to the given forums, clock advances.
    bf_db::platform::digests::set_digest(&pool, reader.get(), "daily").await.unwrap();
    sqlx::query("UPDATE users SET last_digest_at = now() - interval '2 days', email_verified_at = now() WHERE id = $1")
        .bind(reader)
        .execute(&pool)
        .await
        .unwrap();
    let due = bf_db::platform::digests::due_digests(&pool, 1000).await.unwrap();
    let me = due.iter().find(|d| d.id == reader).expect("reader is due");
    assert!(me.groups.contains(&2));
    let topics = bf_db::platform::digests::digest_topics(&pool, reader.get(), &[forum], me.since, 10).await.unwrap();
    assert!(topics.iter().any(|t| t.0 == item));
    assert!(bf_db::platform::digests::digest_topics(&pool, reader.get(), &[], me.since, 10).await.unwrap().is_empty());
    bf_db::platform::digests::mark_digest_sent(&pool, "r@example.com", "s", Some("b"), "https://x.test/unsubscribe/t").await.unwrap();
    // The claim above moved the clock, so a second pass (another node) does not see them.
    assert!(!bf_db::platform::digests::due_digests(&pool, 1000).await.unwrap().iter().any(|d| d.id == reader));

    // Group limits: the most generous group wins; any group without a limit means none.
    let helpers = sqlx::query_scalar::<_, i64>(
        "INSERT INTO groups (name, slug, limits) VALUES ($1, $1, '{\"posts_per_day\": 5}') RETURNING id",
    )
    .bind(mk("g"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let strict = sqlx::query_scalar::<_, i64>(
        "INSERT INTO groups (name, slug, limits) VALUES ($1, $1, '{\"posts_per_day\": 2}') RETURNING id",
    )
    .bind(mk("h"))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        bf_db::accounts::limits::limits(&pool, &[GroupId(helpers), GroupId(strict)]).await.unwrap().posts_per_day,
        Some(5)
    );
    assert_eq!(
        bf_db::accounts::limits::limits(&pool, &[GroupId(strict), GroupId(2)]).await.unwrap().posts_per_day,
        None
    );

    // Watched words round trip.
    let id = watched_words::add_watched_word(&pool, "block", &mk("BAD"), None).await.unwrap();
    assert!(watched_words::watched_words(&pool).await.unwrap().iter().any(|w| w.id == id && w.pattern == mk("bad")));
    assert!(watched_words::delete_watched_word(&pool, id).await.unwrap());
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn scheduled_jobs_and_delayed_mail() {
    let pool = pool().await;
    let tag = std::process::id();
    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, username_lower, email, email_lower, password_hash, username_skeleton) VALUES ($1, $1, $2, $2, 'x', $1) RETURNING id",
    )
    .bind(format!("jobs{tag}"))
    .bind(format!("jobs{tag}@example.com"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE users SET post_count = 60 WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
    let raised = bf_db::accounts::users::raise_trust(&pool, &[(uid, 2)]).await.unwrap();
    assert_eq!(raised, vec![(uid, 2)]);
    let level: i16 =
        sqlx::query_scalar("SELECT trust_level FROM users WHERE id = $1").bind(uid).fetch_one(&pool).await.unwrap();
    assert_eq!(level, 2);
    assert!(bf_db::accounts::users::raise_trust(&pool, &[(uid, 1)]).await.unwrap().is_empty(), "never lowered");
    let to = format!("delayed{tag}@example.com");
    // Held until its time; then dropped because the notification was read.
    sqlx::query(
        "INSERT INTO mail_outbox (to_addr, subject, body, not_before, unless_read_user, unless_read_url)
         VALUES ($1, 'read', 'b', now() + interval '1 hour', $2, '/x'), ($1, 'due', 'b', now() - interval '1 minute', $2, '/x'),
                ($1, 'unread', 'b', now() - interval '1 minute', $2, '/other')",
    )
    .bind(&to)
    .bind(uid)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO notifications (user_id, kind, data, read) VALUES ($1, 'reply', '{\"url\":\"/x\"}', true)")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    mail::skip_mail_to_bad_addresses(&pool).await.unwrap();
    let sendable: Vec<String> = mail::unsent_mail(&pool, 1000)
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.to_addr == to)
        .map(|m| m.subject)
        .collect();
    assert_eq!(sendable, ["unread"]);

    let forum: i64 =
        sqlx::query_scalar("INSERT INTO nodes (kind, slug, title) VALUES ('forum', $1, 'Jobs') RETURNING id")
            .bind(format!("jobs-{tag}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let topic = |state: &'static str, pinned: bool, deleted_days: i32| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO items (node_id, title, slug, state, pinned, pinned_until, deleted_at)
                 VALUES ($1, 't', 't', $2, $3, CASE WHEN $3 THEN now() - interval '1 hour' END, now() - make_interval(days => $4)) RETURNING id",
            )
            .bind(forum)
            .bind(state)
            .bind(pinned)
            .bind(deleted_days)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let pinned = topic("visible", true, 0).await;
    let old = topic("deleted", false, 40).await;
    let recent = topic("deleted", false, 1).await;
    assert!(bf_db::forum::topics::expire_pins(&pool).await.unwrap() >= 1);
    let still: bool =
        sqlx::query_scalar("SELECT pinned FROM items WHERE id = $1").bind(pinned).fetch_one(&pool).await.unwrap();
    assert!(!still);
    bf_db::forum::posts::purge_deleted(&pool, 30).await.unwrap();
    let left = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM items WHERE id = $1)")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert!(!left(old).await);
    assert!(left(recent).await);
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn event_capacity_rsvp_outcomes_poll_zone_and_held_mail() {
    use bf_db::community::{events, polls};
    use chrono::NaiveDate;

    let pool = pool().await;
    let tag = std::process::id();
    let mut users = Vec::new();
    for i in 0..3 {
        users.push(
            sqlx::query_scalar::<_, i64>(
                "INSERT INTO users (username, username_lower, email, email_lower, password_hash, username_skeleton)
                 VALUES ($1, $1, $2, $2, 'x', $1) RETURNING id",
            )
            .bind(format!("cf{tag}_{i}"))
            .bind(format!("cf{tag}_{i}@example.com"))
            .fetch_one(&pool)
            .await
            .unwrap(),
        );
    }
    let forum: i64 =
        sqlx::query_scalar("INSERT INTO nodes (kind, slug, title) VALUES ('forum', $1, 'Comm') RETURNING id")
            .bind(format!("comm-{tag}"))
            .fetch_one(&pool)
            .await
            .unwrap();
    let item: i64 = sqlx::query_scalar(
        "INSERT INTO items (node_id, title, slug, state) VALUES ($1, 't', 't', 'visible') RETURNING id",
    )
    .bind(forum)
    .fetch_one(&pool)
    .await
    .unwrap();

    // RSVP to a topic with no event is told apart from a full event.
    assert_eq!(events::rsvp(&pool, item, users[0], Some("going")).await.unwrap(), events::Rsvp::NoEvent);
    let new = |capacity| events::NewEvent {
        date: NaiveDate::from_ymd_opt(2030, 1, 1).unwrap(),
        start_time: Some("18:00"),
        end_time: None,
        timezone: "UTC",
        location: None,
        recurrence: "none",
        repeat_until: None,
        capacity,
    };
    assert!(events::set_event(&pool, item, &new(Some(2))).await.unwrap());
    assert_eq!(events::rsvp(&pool, item, users[0], Some("going")).await.unwrap(), events::Rsvp::Saved);
    assert_eq!(events::rsvp(&pool, item, users[1], Some("going")).await.unwrap(), events::Rsvp::Saved);
    assert_eq!(events::rsvp(&pool, item, users[2], Some("going")).await.unwrap(), events::Rsvp::Full);
    // The capacity cannot drop below the two going; equal is fine.
    assert!(!events::set_event(&pool, item, &new(Some(1))).await.unwrap());
    assert_eq!(events::event(&pool, item).await.unwrap().unwrap().capacity, Some(2));
    assert!(events::set_event(&pool, item, &new(None)).await.unwrap());

    // A poll's wall-clock closing time is read in the member's zone.
    let local = NaiveDate::from_ymd_opt(2030, 7, 1).unwrap().and_hms_opt(12, 0, 0).unwrap();
    let warsaw = polls::local_to_utc(&pool, local, "Europe/Warsaw").await.unwrap().unwrap();
    assert_eq!(warsaw.to_rfc3339(), "2030-07-01T10:00:00+00:00");
    assert!(polls::local_to_utc(&pool, local, "Not/AZone").await.unwrap().is_none());

    // Mail for a member in "do not disturb" is held until it ends, not dropped.
    sqlx::query("UPDATE users SET dnd_until = now() + interval '3 hours' WHERE id = $1")
        .bind(users[0])
        .execute(&pool)
        .await
        .unwrap();
    // Mail goes only to verified addresses.
    sqlx::query("UPDATE users SET email_verified_at = now() WHERE id = ANY($1)")
        .bind(&users[..2])
        .execute(&pool)
        .await
        .unwrap();
    notifications::deliver(
        &pool,
        "https://x.test",
        0,
        &users[..2],
        "mention",
        &json!({ "url": "/t/1" }),
        &format!("held{tag}"),
        "body",
        0,
    )
    .await
    .unwrap();
    let held: Vec<(String, bool)> = sqlx::query_as(
        "SELECT to_addr, coalesce(not_before > now() + interval '2 hours', false) FROM mail_outbox WHERE subject = $1 ORDER BY to_addr",
    )
    .bind(format!("held{tag}"))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(held.len(), 2, "both get mail");
    assert_eq!(held.iter().map(|h| h.1).collect::<Vec<_>>(), [true, false]);
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn forum_fixes_edit_base_anonymous_likes_invites_split() {
    use bf_core::perms::Perms;
    use bf_db::forum::{anonymous, reactions};
    use bf_db::moderation::{held, topics as mod_topics};

    let pool = pool().await;
    let tag = format!("ff{}", std::process::id());
    let uid = users::create_user(&pool, &tag, &format!("{tag}@example.com"), "x").await.unwrap();
    let other = users::create_user(&pool, &format!("{tag}o"), &format!("{tag}o@example.com"), "x").await.unwrap();
    let cat = nodes::create_node(&pool, None, "category", "Cat2", &format!("cat-{tag}"), "", &[(1, Perms::VIEW)])
        .await
        .unwrap();
    let forum =
        nodes::create_node(&pool, Some(cat), "forum", "F2", &format!("f-{tag}"), "", &[(1, Perms::ALL)]).await.unwrap();
    let post = |raw: &'static str| posts::NewPost {
        author_id: uid.get(),
        raw,
        cooked: raw,
        cooked_version: 1,
        ip: None,
        pending: false,
    };
    let item = posts::create_topic(&pool, forum, "Fix topic", &format!("fix-{tag}"), &post("first")).await.unwrap();
    posts::create_reply(&pool, item, forum, &post("second")).await.unwrap();
    let second: i64 = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = 2")
        .bind(item)
        .fetch_one(&pool)
        .await
        .unwrap();

    // FORUM-6: two saves from the same base: only the first applies, and no revision is doubled.
    assert!(posts::edit_post_if(&pool, second, uid.get(), "a", "a", 1, None, Some(0)).await.unwrap());
    assert!(!posts::edit_post_if(&pool, second, uid.get(), "b", "b", 1, None, Some(0)).await.unwrap());
    let raw: String =
        sqlx::query_scalar("SELECT raw FROM posts WHERE id = $1").bind(second).fetch_one(&pool).await.unwrap();
    assert_eq!(raw, "a");
    let revs: i64 = sqlx::query_scalar("SELECT count(*) FROM post_revisions WHERE post_id = $1")
        .bind(second)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(revs, 1);

    // FORUM-3: an anonymous reply never carries the real name, and still counts for the real member.
    let (n, anon) = posts::create_anonymous_reply(&pool, item, forum, &post("hidden hand")).await.unwrap();
    assert_eq!(anon, anonymous::anonymous_account(&pool).await.unwrap(), "one shared account");
    let (author, real): (i64, Option<i64>) =
        sqlx::query_as("SELECT author_id, real_author_id FROM posts WHERE item_id = $1 AND post_number = $2")
            .bind(item)
            .bind(n)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((author, real), (anon, Some(uid.get())));
    assert!(posts::seconds_since_last_post(&pool, uid.get()).await.unwrap().is_some());
    assert!(bf_db::accounts::limits::posts_today(&pool, uid).await.unwrap() >= 3);

    // FORUM-5: the like is found by its flag, and toggles.
    let like = reactions::like_reaction_id(&pool).await.unwrap();
    let flagged: bool =
        sqlx::query_scalar("SELECT is_like FROM reactions WHERE id = $1").bind(like).fetch_one(&pool).await.unwrap();
    assert!(flagged);
    assert!(reactions::toggle_like(&pool, second, other.get()).await.unwrap().0);
    assert!(!reactions::toggle_like(&pool, second, other.get()).await.unwrap().0);

    // FORUM-10: one invitation per member and topic.
    assert_eq!(topics::claim_invite(&pool, uid.get(), other.get(), item).await.unwrap(), topics::Invite::Allowed);
    assert_eq!(topics::claim_invite(&pool, uid.get(), other.get(), item).await.unwrap(), topics::Invite::Already);

    // FORUM-11: a private profile is not offered to a non-staff viewer.
    sqlx::query("UPDATE users SET profile_visibility = 'nobody' WHERE id = $1")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    let prefix = format!("{tag}o");
    let shown = users::mention_candidates(&pool, &prefix, None, uid.get(), 8, false).await.unwrap();
    assert!(shown.is_empty());
    assert_eq!(users::mention_candidates(&pool, &prefix, None, uid.get(), 8, true).await.unwrap().len(), 1);

    // FORUM-8: a decided item can be put back in the queue.
    let h = held::hold(&pool, "status", uid.get(), None, &json!({ "body": "x" })).await.unwrap();
    assert!(held::resolve(&pool, h, true, uid.get()).await.unwrap());
    assert!(held::get(&pool, h).await.unwrap().is_none());
    held::reopen(&pool, h).await.unwrap();
    assert!(held::get(&pool, h).await.unwrap().is_some());

    // FORUM-20: a split topic dates from its first moved post and keeps the tags.
    bf_db::forum::tags::set_tags(&pool, item, &[("fixtag".to_owned(), "fixtag".to_owned())], true).await.unwrap();
    let third = posts::create_reply(&pool, item, forum, &post("third")).await.unwrap();
    let third_id: i64 = sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(item)
        .bind(third)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE posts SET created_at = now() - interval '3 days' WHERE id = $1")
        .bind(third_id)
        .execute(&pool)
        .await
        .unwrap();
    let new_id = mod_topics::split_posts(&pool, item, &[third_id], "Split off", &format!("split-{tag}"), uid.get())
        .await
        .unwrap();
    let (age_ok, tags): (bool, i64) = sqlx::query_as(
        "SELECT created_at < now() - interval '2 days', (SELECT count(*) FROM item_tags WHERE item_id = $1) FROM items WHERE id = $1",
    )
    .bind(new_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(age_ok);
    assert_eq!(tags, 1);
}

#[tokio::test]
#[ignore = "needs Postgres"]
async fn input_fixes_quota_links_search_sitemap() {
    use bf_core::perms::Perms;
    use bf_db::forum::{feeds, links, uploads};

    let pool = pool().await;
    let tag = format!("in{}", std::process::id());
    let uid = users::create_user(&pool, &format!("x_y{tag}"), &format!("{tag}@example.com"), "x").await.unwrap();
    let other = users::create_user(&pool, &format!("xay{tag}"), &format!("{tag}b@example.com"), "x").await.unwrap();

    // Upload quota: every upload counts, also of bytes that are already stored, and the check
    // and the count are one step.
    assert!(uploads::reserve_quota(&pool, uid.get(), 600, 1000).await.unwrap());
    assert!(!uploads::reserve_quota(&pool, uid.get(), 600, 1000).await.unwrap(), "a repeat counts again");
    assert_eq!(uploads::bytes_today(&pool, uid.get()).await.unwrap(), 600);
    assert!(uploads::reserve_quota(&pool, uid.get(), 400, 1000).await.unwrap());
    let mut racers = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let pool = pool.clone();
        racers.spawn(async move { uploads::reserve_quota(&pool, other.get(), 400, 1000).await.unwrap() });
    }
    let mut won = 0;
    while let Some(ok) = racers.join_next().await {
        won += usize::from(ok.unwrap());
    }
    assert_eq!(won, 2, "parallel uploads cannot all pass the check");

    // Forums: one public, one private.
    let cat =
        nodes::create_node(&pool, None, "category", "C", &format!("c{tag}"), "", &[(1, Perms::VIEW)]).await.unwrap();
    let open = nodes::create_node(&pool, Some(cat), "forum", "Open", &format!("o{tag}"), "", &[(1, Perms::ALL)])
        .await
        .unwrap();
    let hidden = nodes::create_node(&pool, Some(cat), "forum", "Hidden", &format!("h{tag}"), "", &[(2, Perms::ALL)])
        .await
        .unwrap();
    let post = |raw: &'static str| posts::NewPost {
        author_id: uid.get(),
        raw,
        cooked: raw,
        cooked_version: 1,
        ip: None,
        pending: false,
    };
    let public_topic = posts::create_topic(&pool, open, "Open one", "open-one", &post("a")).await.unwrap();
    let private_topic = posts::create_topic(&pool, hidden, "Hidden one", "hidden-one", &post("b")).await.unwrap();
    let (public_url, private_url) = (format!("https://pub.example/{tag}"), format!("https://priv.example/{tag}"));
    links::record_links(&pool, public_topic, 1, std::slice::from_ref(&public_url)).await.unwrap();
    links::record_links(&pool, private_topic, 1, std::slice::from_ref(&private_url)).await.unwrap();
    let guest_forums = [open];
    assert!(links::is_posted_link(&pool, &public_url, &guest_forums).await.unwrap());
    assert!(!links::is_posted_link(&pool, &private_url, &guest_forums).await.unwrap(), "private forum");
    sqlx::query("UPDATE posts SET state = 'deleted' WHERE item_id = $1")
        .bind(public_topic)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!links::is_posted_link(&pool, &public_url, &guest_forums).await.unwrap(), "deleted post");

    // Sitemap: counted and paged newest first.
    let second = posts::create_topic(&pool, open, "Open two", "open-two", &post("c")).await.unwrap();
    let third = posts::create_topic(&pool, open, "Open three", "open-three", &post("d")).await.unwrap();
    assert!(feeds::sitemap_topic_count(&pool, &[open]).await.unwrap() >= 2);
    let page = feeds::sitemap_topics(&pool, &[open], 0, 2).await.unwrap();
    assert_eq!(page.iter().map(|t| t.0).collect::<Vec<_>>(), [third, second]);
    let rest = feeds::sitemap_topics(&pool, &[open], 2, 10).await.unwrap();
    assert!(rest.iter().all(|t| t.0 < second));

    // Member search: `_` and `%` are characters, not wildcards.
    let filter = |q| members::UserFilter { q, ..Default::default() };
    let name = format!("x_y{tag}");
    let found = members::users(&pool, &filter(&name), 10, 0).await.unwrap();
    assert_eq!(found.iter().map(|u| u.id).collect::<Vec<_>>(), [uid], "underscore matched another name");
    assert!(members::users(&pool, &filter("%"), 10, 0).await.unwrap().is_empty());
}
