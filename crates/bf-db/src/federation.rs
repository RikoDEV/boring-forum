//! ActivityPub storage: actor keys, remote actors and their shadow accounts,
//! followers, remote objects that became posts, and the delivery queue.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

pub async fn key(pool: &PgPool, owner_type: &str, owner_id: i64) -> Result<Option<(String, String)>, Error> {
    sqlx::query_as("SELECT public_pem, private_pem FROM ap_keys WHERE owner_type = $1 AND owner_id = $2")
        .bind(owner_type)
        .bind(owner_id)
        .fetch_optional(pool)
        .await
}

/// Stores a new key pair unless another request stored one first; returns the stored pair.
pub async fn insert_key(
    pool: &PgPool,
    owner_type: &str,
    owner_id: i64,
    public_pem: &str,
    private_sealed: &str,
) -> Result<(String, String), Error> {
    sqlx::query(
        "INSERT INTO ap_keys (owner_type, owner_id, public_pem, private_pem) VALUES ($1, $2, $3, $4)
         ON CONFLICT (owner_type, owner_id) DO NOTHING",
    )
    .bind(owner_type)
    .bind(owner_id)
    .bind(public_pem)
    .bind(private_sealed)
    .execute(pool)
    .await?;
    sqlx::query_as("SELECT public_pem, private_pem FROM ap_keys WHERE owner_type = $1 AND owner_id = $2")
        .bind(owner_type)
        .bind(owner_id)
        .fetch_one(pool)
        .await
}

#[derive(Debug, Clone, FromRow)]
pub struct RemoteActor {
    pub id: i64,
    pub uri: String,
    pub domain: String,
    pub kind: String,
    pub username: String,
    pub display_name: Option<String>,
    pub url: Option<String>,
    pub inbox: String,
    pub shared_inbox: Option<String>,
    pub key_id: String,
    pub public_pem: String,
    pub user_id: Option<i64>,
    pub fetched_at: DateTime<Utc>,
}

macro_rules! actor_cols {
    () => {
        "id, uri, domain, kind, username, display_name, url, inbox, shared_inbox, key_id, public_pem, user_id, fetched_at"
    };
}

pub struct NewActor<'a> {
    pub uri: &'a str,
    pub domain: &'a str,
    pub kind: &'a str,
    pub username: &'a str,
    pub display_name: Option<&'a str>,
    pub url: Option<&'a str>,
    pub inbox: &'a str,
    pub shared_inbox: Option<&'a str>,
    pub key_id: &'a str,
    pub public_pem: &'a str,
}

pub async fn upsert_actor(pool: &PgPool, a: &NewActor<'_>) -> Result<RemoteActor, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("INSERT INTO ap_actors (uri, domain, kind, username, display_name, url, inbox, shared_inbox, key_id, public_pem)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (uri) DO UPDATE SET kind = $3, username = $4, display_name = $5, url = $6, inbox = $7,
             shared_inbox = $8, key_id = $9, public_pem = $10, fetched_at = now()
         RETURNING ", actor_cols!()))
    .bind(a.uri)
    .bind(a.domain)
    .bind(a.kind)
    .bind(a.username)
    .bind(a.display_name)
    .bind(a.url)
    .bind(a.inbox)
    .bind(a.shared_inbox)
    .bind(a.key_id)
    .bind(a.public_pem)
    .fetch_one(pool)
    .await
}

pub async fn actor_by_uri(pool: &PgPool, uri: &str) -> Result<Option<RemoteActor>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", actor_cols!(), " FROM ap_actors WHERE uri = $1"))
        .bind(uri)
        .fetch_optional(pool)
        .await
}

pub async fn actor_by_key(pool: &PgPool, key_id: &str) -> Result<Option<RemoteActor>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", actor_cols!(), " FROM ap_actors WHERE key_id = $1"))
        .bind(key_id)
        .fetch_optional(pool)
        .await
}

pub async fn actor_by_user(pool: &PgPool, user_id: i64) -> Result<Option<RemoteActor>, Error> {
    // Only a fixed column list or select prefix (a constant) is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", actor_cols!(), " FROM ap_actors WHERE user_id = $1"))
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

/// The local account a remote actor posts and reacts as. It can never sign in: no password,
/// an `.invalid` address, and `remote` set, which every sign-in path refuses.
pub async fn ensure_shadow_user(pool: &PgPool, actor: &RemoteActor) -> Result<i64, Error> {
    if let Some(id) = actor.user_id {
        return Ok(id);
    }
    let mut tx = pool.begin().await?;
    // `name@domain`, cut to fit; a clash (two long names) gets the actor id appended.
    let mut name: String = format!("{}@{}", actor.username, actor.domain).chars().take(60).collect();
    let taken: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE username_lower = lower($1))")
        .bind(&name)
        .fetch_one(&mut *tx)
        .await?;
    if taken {
        name = format!("{}-{}", name.chars().take(48).collect::<String>(), actor.id);
    }
    let email = format!("actor-{}@remote.invalid", actor.id);
    // A remote actor may call itself anything; it does not get to pass for staff here.
    let shown: Option<String> = actor
        .display_name
        .as_deref()
        .map(|d| bf_core::html::strip_hidden_chars(d).trim().chars().take(40).collect::<String>())
        .filter(|d| !d.is_empty());
    let shown = match shown {
        Some(d) if crate::accounts::users::display_name_impersonates(pool, &d, None).await? => None,
        other => other,
    };
    let (id, _) = crate::accounts::users::insert_user(
        &mut tx,
        &crate::accounts::users::NewUser {
            username: &name,
            email: &email,
            password_hash: None,
            display_name: shown.as_deref(),
            primary_group: Some(2),
            joined_at: None,
            verified: true,
            kind: crate::accounts::users::AccountKind::Remote,
            fit_name: false,
        },
    )
    .await?;
    let id: i64 = id.get();
    sqlx::query("UPDATE ap_actors SET user_id = $2 WHERE id = $1").bind(actor.id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn add_follower(
    pool: &PgPool,
    local_type: &str,
    local_id: i64,
    actor_id: i64,
    follow_id: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ap_followers (local_type, local_id, actor_id, follow_id) VALUES ($1, $2, $3, $4)
         ON CONFLICT (local_type, local_id, actor_id) DO UPDATE SET follow_id = $4",
    )
    .bind(local_type)
    .bind(local_id)
    .bind(actor_id)
    .bind(follow_id)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn remove_follower(pool: &PgPool, local_type: &str, local_id: i64, actor_id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM ap_followers WHERE local_type = $1 AND local_id = $2 AND actor_id = $3")
        .bind(local_type)
        .bind(local_id)
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn remove_actor_follows(pool: &PgPool, actor_id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM ap_followers WHERE actor_id = $1").bind(actor_id).execute(pool).await.map(|_| ())
}

pub async fn follower_count(pool: &PgPool, local_type: &str, local_id: i64) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM ap_followers WHERE local_type = $1 AND local_id = $2")
        .bind(local_type)
        .bind(local_id)
        .fetch_one(pool)
        .await
}

/// One inbox per follower's server where it has a shared inbox, so a post reaches a big
/// instance once rather than once per follower there.
pub async fn follower_inboxes(pool: &PgPool, local_type: &str, local_id: i64) -> Result<Vec<String>, Error> {
    sqlx::query_scalar(
        "SELECT DISTINCT coalesce(a.shared_inbox, a.inbox) FROM ap_followers f JOIN ap_actors a ON a.id = f.actor_id
         WHERE f.local_type = $1 AND f.local_id = $2",
    )
    .bind(local_type)
    .bind(local_id)
    .fetch_all(pool)
    .await
}

pub async fn add_object(pool: &PgPool, uri: &str, post_id: i64, actor_id: i64) -> Result<(), Error> {
    sqlx::query("INSERT INTO ap_objects (uri, post_id, actor_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
        .bind(uri)
        .bind(post_id)
        .bind(actor_id)
        .execute(pool)
        .await
        .map(|_| ())
}

/// (post id, actor id) of a remote object that became a post.
pub async fn object(pool: &PgPool, uri: &str) -> Result<Option<(i64, i64)>, Error> {
    sqlx::query_as("SELECT post_id, actor_id FROM ap_objects WHERE uri = $1").bind(uri).fetch_optional(pool).await
}

/// The remote object uri of a post, if it came from elsewhere.
pub async fn object_of_post(pool: &PgPool, post_id: i64) -> Result<Option<(String, i64)>, Error> {
    sqlx::query_as("SELECT uri, actor_id FROM ap_objects WHERE post_id = $1").bind(post_id).fetch_optional(pool).await
}

pub async fn objects_from_domain_last_hour(pool: &PgPool, domain: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM ap_objects o JOIN ap_actors a ON a.id = o.actor_id
         WHERE a.domain = $1 AND o.created_at > now() - interval '1 hour'",
    )
    .bind(domain)
    .fetch_one(pool)
    .await
}

/// Direct messages that arrived from one instance in the last hour.
pub async fn dm_objects_from_domain_last_hour(pool: &PgPool, domain: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM ap_dm_objects o JOIN conversation_messages m ON m.id = o.message_id
         JOIN ap_actors a ON a.user_id = m.author_id
         WHERE a.domain = $1 AND o.created_at > now() - interval '1 hour'",
    )
    .bind(domain)
    .fetch_one(pool)
    .await
}

/// Reports filed by the shadow accounts of one instance in the last hour.
pub async fn flags_from_domain_last_hour(pool: &PgPool, domain: &str) -> Result<i64, Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM reports r JOIN ap_actors a ON a.user_id = r.reporter_id
         WHERE a.domain = $1 AND r.created_at > now() - interval '1 hour'",
    )
    .bind(domain)
    .fetch_one(pool)
    .await
}

/// True the first time an activity id is seen.
pub async fn first_sight(pool: &PgPool, activity_id: &str) -> Result<bool, Error> {
    let n = sqlx::query("INSERT INTO ap_seen (activity_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(activity_id)
        .execute(pool)
        .await?;
    Ok(n.rows_affected() == 1)
}

pub async fn enqueue(
    pool: &PgPool,
    inbox: &str,
    signer_type: &str,
    signer_id: i64,
    activity: &serde_json::Value,
) -> Result<(), Error> {
    sqlx::query("INSERT INTO ap_deliveries (inbox, signer_type, signer_id, activity) VALUES ($1, $2, $3, $4)")
        .bind(inbox)
        .bind(signer_type)
        .bind(signer_id)
        .bind(activity)
        .execute(pool)
        .await
        .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct Delivery {
    pub id: i64,
    pub inbox: String,
    pub signer_type: String,
    pub signer_id: i64,
    pub activity: serde_json::Value,
    pub attempts: i32,
}

/// Claims due deliveries by pushing them forward, so two nodes do not send the same one.
pub async fn due(pool: &PgPool, limit: i64) -> Result<Vec<Delivery>, Error> {
    sqlx::query_as(
        "UPDATE ap_deliveries SET next_at = now() + interval '5 minutes'
         WHERE id IN (SELECT id FROM ap_deliveries WHERE next_at <= now() ORDER BY next_at LIMIT $1 FOR UPDATE SKIP LOCKED)
         RETURNING id, inbox, signer_type, signer_id, activity, attempts",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Gives a claimed delivery back so it is due again at once.
pub async fn release(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("UPDATE ap_deliveries SET next_at = now() WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

/// Forgets an activity id, so a resend of an activity that failed is handled again.
pub async fn forget_seen(pool: &PgPool, activity_id: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM ap_seen WHERE activity_id = $1").bind(activity_id).execute(pool).await.map(|_| ())
}

pub async fn delivered(pool: &PgPool, id: i64) -> Result<(), Error> {
    sqlx::query("DELETE FROM ap_deliveries WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

pub async fn retry(pool: &PgPool, id: i64, after_seconds: i64, error: &str) -> Result<(), Error> {
    sqlx::query(
        "UPDATE ap_deliveries SET attempts = attempts + 1, next_at = now() + make_interval(secs => $2), last_error = $3
         WHERE id = $1",
    )
    .bind(id)
    .bind(after_seconds as f64)
    .bind(error)
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Clone, FromRow)]
pub struct LocalUser {
    pub id: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub about_html: Option<String>,
    pub joined_at: DateTime<Utc>,
}

/// A member who may appear as an ActivityPub actor: local, active, approved, not a system
/// account, and with a profile that anyone may see (the actor document shows the same things).
pub async fn local_user(pool: &PgPool, id: i64) -> Result<Option<LocalUser>, Error> {
    sqlx::query_as(
        "SELECT id, username, display_name, about_html, joined_at FROM users
         WHERE id = $1 AND NOT remote AND coalesce(suspended_until <= now(), true)
           AND approved AND profile_visibility = 'everyone'
           AND username_lower NOT LIKE 'deleted-%' AND username_lower NOT LIKE 'plugin-%'
           AND email_lower NOT LIKE '%.invalid'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn local_user_by_name(pool: &PgPool, name: &str) -> Result<Option<LocalUser>, Error> {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username_lower = lower($1)")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    match id {
        Some(id) => local_user(pool, id).await,
        None => Ok(None),
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct ApPost {
    pub id: i64,
    pub item_id: i64,
    pub node_id: i64,
    pub post_number: i32,
    pub author_id: Option<i64>,
    pub cooked: String,
    pub state: String,
    pub topic_state: String,
    pub title: String,
    pub slug: String,
    pub reply_to_post_id: Option<i64>,
    pub first_post_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub edit_count: i32,
}

pub async fn post(pool: &PgPool, id: i64) -> Result<Option<ApPost>, Error> {
    sqlx::query_as(
        "SELECT p.id, p.item_id, i.node_id, p.post_number, p.author_id, p.cooked, p.state::text AS state,
                i.state::text AS topic_state, i.title, i.slug, p.reply_to_post_id,
                (SELECT f.id FROM posts f WHERE f.item_id = p.item_id AND f.post_number = 1) AS first_post_id,
                p.created_at, p.updated_at, p.edit_count
         FROM posts p JOIN items i ON i.id = p.item_id WHERE p.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn post_id_at(pool: &PgPool, topic: i64, number: i32) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT id FROM posts WHERE item_id = $1 AND post_number = $2")
        .bind(topic)
        .bind(number)
        .fetch_optional(pool)
        .await
}

/// (total, active in the last month, active in the last six months, local posts) for NodeInfo.
pub async fn usage(pool: &PgPool) -> Result<(i64, i64, i64, i64), Error> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE true),
                count(*) FILTER (WHERE last_seen_at > now() - interval '30 days'),
                count(*) FILTER (WHERE last_seen_at > now() - interval '180 days'),
                (SELECT count(*) FROM posts p JOIN users a ON a.id = p.author_id WHERE NOT a.remote AND p.state = 'visible')
         FROM users WHERE NOT remote AND username_lower NOT LIKE 'deleted-%' AND email_lower NOT LIKE '%.invalid'",
    )
    .fetch_one(pool)
    .await
}

pub async fn is_remote_user(pool: &PgPool, user_id: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT coalesce((SELECT remote FROM users WHERE id = $1), false)")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

// --- Mirrors, moves and direct messages ---------------------------------------

/// (node, actor uri, accepted) of every forum that follows a remote group.
pub async fn mirrors(pool: &PgPool) -> Result<Vec<(i64, String, String, bool)>, Error> {
    sqlx::query_as(
        "SELECT m.node_id, a.uri, a.username || '@' || a.domain, m.accepted FROM ap_mirrors m
         JOIN ap_actors a ON a.id = m.actor_id ORDER BY m.node_id, a.uri",
    )
    .fetch_all(pool)
    .await
}

pub async fn add_mirror(pool: &PgPool, node: i64, actor: i64, follow_id: &str) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ap_mirrors (node_id, actor_id, follow_id) VALUES ($1, $2, $3)
         ON CONFLICT (node_id, actor_id) DO UPDATE SET follow_id = $3, accepted = false",
    )
    .bind(node)
    .bind(actor)
    .bind(follow_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Removes a mirror and returns its follow id, for the Undo.
pub async fn remove_mirror(pool: &PgPool, node: i64, actor_uri: &str) -> Result<Option<(String, String)>, Error> {
    sqlx::query_as(
        "DELETE FROM ap_mirrors m USING ap_actors a WHERE m.actor_id = a.id AND m.node_id = $1 AND a.uri = $2
         RETURNING m.follow_id, a.inbox",
    )
    .bind(node)
    .bind(actor_uri)
    .fetch_optional(pool)
    .await
}

/// A remote group answered our Follow.
pub async fn mirror_answer(pool: &PgPool, follow_id: &str, actor: i64, accepted: bool) -> Result<bool, Error> {
    let sql = if accepted {
        "UPDATE ap_mirrors SET accepted = true WHERE follow_id = $1 AND actor_id = $2"
    } else {
        "DELETE FROM ap_mirrors WHERE follow_id = $1 AND actor_id = $2"
    };
    Ok(sqlx::query(sql).bind(follow_id).bind(actor).execute(pool).await?.rows_affected() > 0)
}

/// The local forums mirroring a remote group, once it accepted.
pub async fn mirror_nodes(pool: &PgPool, actor: i64) -> Result<Vec<i64>, Error> {
    sqlx::query_scalar("SELECT node_id FROM ap_mirrors WHERE actor_id = $1 AND accepted")
        .bind(actor)
        .fetch_all(pool)
        .await
}

/// A member's other accounts and where they moved.
pub async fn aliases(pool: &PgPool, user: i64) -> Result<(Vec<String>, Option<String>), Error> {
    sqlx::query_as("SELECT ap_also_known_as, ap_moved_to FROM users WHERE id = $1").bind(user).fetch_one(pool).await
}

pub async fn set_aliases(pool: &PgPool, user: i64, aliases: &[String], moved_to: Option<&str>) -> Result<(), Error> {
    sqlx::query("UPDATE users SET ap_also_known_as = $2, ap_moved_to = $3 WHERE id = $1")
        .bind(user)
        .bind(aliases)
        .bind(moved_to)
        .execute(pool)
        .await
        .map(|_| ())
}

/// A remote account moved: its follows pass to the new account, which is remembered.
pub async fn move_actor(pool: &PgPool, old: i64, new: i64, target_uri: &str) -> Result<u64, Error> {
    let mut tx = pool.begin().await?;
    let moved = sqlx::query(
        "INSERT INTO ap_followers (local_type, local_id, actor_id, follow_id)
         SELECT local_type, local_id, $2, follow_id || '#moved' FROM ap_followers WHERE actor_id = $1
         ON CONFLICT DO NOTHING",
    )
    .bind(old)
    .bind(new)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    sqlx::query("DELETE FROM ap_followers WHERE actor_id = $1").bind(old).execute(&mut *tx).await?;
    sqlx::query("UPDATE ap_actors SET moved_to = $2 WHERE id = $1")
        .bind(old)
        .bind(target_uri)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(moved)
}

/// Inboxes of everyone who follows a member, for an outgoing Move.
pub async fn all_follower_inboxes(pool: &PgPool, user: i64) -> Result<Vec<String>, Error> {
    follower_inboxes(pool, "user", user).await
}

pub async fn add_dm_object(pool: &PgPool, uri: &str, message: i64, conversation: i64) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO ap_dm_objects (uri, message_id, conversation_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(uri)
    .bind(message)
    .bind(conversation)
    .execute(pool)
    .await
    .map(|_| ())
}

/// The conversation a remote object belongs to.
pub async fn dm_conversation(pool: &PgPool, uri: &str) -> Result<Option<i64>, Error> {
    sqlx::query_scalar("SELECT conversation_id FROM ap_dm_objects WHERE uri = $1").bind(uri).fetch_optional(pool).await
}

/// The newest message of a conversation and its remote object, if any: what a reply answers.
pub async fn dm_last(pool: &PgPool, conversation: i64, except: i64) -> Result<Option<(i64, Option<String>)>, Error> {
    sqlx::query_as(
        "SELECT m.id, o.uri FROM conversation_messages m LEFT JOIN ap_dm_objects o ON o.message_id = m.id
         WHERE m.conversation_id = $1 AND m.id <> $2 ORDER BY m.id DESC LIMIT 1",
    )
    .bind(conversation)
    .bind(except)
    .fetch_optional(pool)
    .await
}

/// The message before one in its conversation, and its remote object if it came from elsewhere.
pub async fn dm_last_before(
    pool: &PgPool,
    conversation: i64,
    before: i64,
) -> Result<Option<(i64, Option<String>)>, Error> {
    sqlx::query_as(
        "SELECT m.id, o.uri FROM conversation_messages m LEFT JOIN ap_dm_objects o ON o.message_id = m.id
         WHERE m.conversation_id = $1 AND m.id < $2 ORDER BY m.id DESC LIMIT 1",
    )
    .bind(conversation)
    .bind(before)
    .fetch_optional(pool)
    .await
}

/// Remote people in a conversation: (actor uri, inbox).
pub async fn dm_remote_participants(pool: &PgPool, conversation: i64) -> Result<Vec<(String, String)>, Error> {
    sqlx::query_as(
        "SELECT a.uri, a.inbox FROM conversation_participants p JOIN ap_actors a ON a.user_id = p.user_id
         WHERE p.conversation_id = $1 AND p.left_at IS NULL",
    )
    .bind(conversation)
    .fetch_all(pool)
    .await
}

/// The latest message by one member in a conversation: (id, cooked, created).
pub async fn dm_latest_by(
    pool: &PgPool,
    conversation: i64,
    author: i64,
) -> Result<Option<(i64, String, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT id, cooked, created_at FROM conversation_messages WHERE conversation_id = $1 AND author_id = $2
         ORDER BY id DESC LIMIT 1",
    )
    .bind(conversation)
    .bind(author)
    .fetch_optional(pool)
    .await
}

/// Whether a member ignores another (who then cannot message them).
pub async fn ignores(pool: &PgPool, user: i64, other: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM ignored_users WHERE user_id = $1 AND ignored_user_id = $2)")
        .bind(user)
        .bind(other)
        .fetch_one(pool)
        .await
}
