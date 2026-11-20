//! Data export, account deletion and account merging.
//!
//! [`REGISTRY`] is the one list of everything that points at a member. Export and erasure both
//! read it, and a test compares it with the schema, so the two cannot drift apart or miss a table.

use crate::admin::audit::{Entry, record};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;

type Error = sqlx::Error;

/// What happens to the rows that point at a member when the account is anonymized.
#[derive(Debug, Clone, Copy)]
pub enum Erase {
    /// The row is about the member alone: it is deleted.
    Delete,
    /// The row stays but forgets the member (the column is set to NULL).
    Null,
    /// The row stays because accounting needs it; these columns are set to NULL on it (an
    /// order loses buyer name, address and tax id and keeps amounts and invoice number).
    Scrub(&'static [&'static str]),
    /// The row stays and keeps pointing at the account, which by then is only "deleted-123":
    /// content other people read, and moderation records. The text is the policy.
    Keep(&'static str),
}

/// How a table reaches the data export.
#[derive(Debug, Clone, Copy)]
pub enum Export {
    /// Every row as JSON, minus these columns (secrets and digests).
    Rows(&'static [&'static str]),
    /// Written out by hand in [`export`] under its own key.
    Curated,
    /// Not exported, with the reason.
    No(&'static str),
}

/// One column that references `users`, and what erasure and export do with its rows.
#[derive(Debug, Clone, Copy)]
pub struct Personal {
    pub table: &'static str,
    pub column: &'static str,
    pub erase: Erase,
    pub export: Export,
}

const fn p(table: &'static str, column: &'static str, erase: Erase, export: Export) -> Personal {
    Personal { table, column, erase, export }
}

/// Columns never exported, whatever the table: rendered copies and search vectors.
const EXPORT_NEVER: [&str; 2] = ["cooked", "search_vector"];
const EXPORT_LIMIT: usize = 50_000;

use Erase::{Delete, Keep, Null};
use Export::{Curated, No, Rows};

/// Every column referencing `users` (the `personal_data_registry_matches_schema` test checks it
/// against the migrations).
pub const REGISTRY: &[Personal] = &[
    p(
        "users",
        "referred_by",
        Keep("points at the inviting member, now a pseudonym"),
        No("another member's invitation"),
    ),
    p("user_secondary_groups", "user_id", Delete, Rows(&[])),
    p("sessions", "user_id", Delete, Rows(&["token_digest"])),
    p("sessions", "impersonated_by", Null, No("the administrator behind an impersonated session; the link only")),
    p("club_bans", "user_id", Delete, No("a club owner's ban list")),
    p("mass_messages", "sender_id", Null, No("staff action; the message record stays")),
    p(
        "plugin_review_items",
        "submitted_by",
        Keep("a staff review entry; the member shows as deleted-N"),
        No("staff queue"),
    ),
    p("plugin_review_items", "decided_by", Keep("moderation record"), No("staff action")),
    p("post_notes", "author_id", Keep("a staff note; the author shows as deleted-N"), No("staff note")),
    p("email_tokens", "user_id", Delete, No("one-time secrets")),
    p("trusted_devices", "user_id", Delete, No("device tokens")),
    p("node_moderators", "user_id", Delete, Rows(&[])),
    p("items", "assigned_to", Null, No("staff workflow")),
    p("items", "author_id", Keep("the topic stays, shown as deleted-N"), Rows(&[])),
    p("items", "deleted_by", Keep("moderation record"), No("staff action")),
    p("items", "last_poster_id", Keep("shown as deleted-N"), No("derived")),
    p("posts", "author_id", Keep("the post stays, shown as deleted-N"), Curated),
    p("posts", "deleted_by", Keep("moderation record"), No("staff action")),
    p("posts", "last_editor_id", Keep("shown as deleted-N"), No("derived")),
    p(
        "posts",
        "real_author_id",
        Null,
        No("anonymous replies are written under the shared account; the posts export does not list them yet"),
    ),
    p("post_revisions", "editor_id", Keep("history of posts that stay"), Rows(&[])),
    p("post_likes", "user_id", Keep("keeps like counts true; no identity is left on the row"), Curated),
    p("drafts", "user_id", Delete, Rows(&[])),
    p("read_markers", "user_id", Delete, Rows(&[])),
    p("user_visits", "user_id", Delete, Rows(&[])),
    p("node_read_markers", "user_id", Delete, Rows(&[])),
    p("follows", "user_id", Delete, Rows(&[])),
    p("notifications", "user_id", Delete, Rows(&[])),
    p("notification_prefs", "user_id", Delete, Rows(&[])),
    p("ignored_users", "ignored_user_id", Delete, No("another member's list")),
    p("ignored_users", "user_id", Delete, Rows(&[])),
    p("uploads", "user_id", Keep("files embedded in posts stay; unreferenced ones are swept"), Curated),
    p("reports", "reporter_id", Keep("moderation record"), Rows(&[])),
    p("reviewables", "claimed_by", Keep("moderation record"), No("staff action")),
    p("reviewables", "resolved_by", Keep("moderation record"), No("staff action")),
    p("user_legal_acceptances", "user_id", Delete, Curated),
    p("pending_logins", "user_id", Delete, No("one-time secrets")),
    p("user_identities", "user_id", Delete, Rows(&[])),
    p("user_field_values", "user_id", Delete, Curated),
    p("passkeys", "user_id", Delete, Rows(&["public_key"])),
    p("webauthn_challenges", "user_id", Delete, No("one-time secrets")),
    // Whoever tried to sign in (typed name, address) is not this member's data: counts and dates stay.
    p("failed_logins", "user_id", Delete, Rows(&["login", "ip"])),
    p("backup_codes", "user_id", Delete, No("secrets")),
    p("username_history", "user_id", Delete, Rows(&[])),
    p("user_notes", "author_id", Keep("staff note, attributed to a pseudonym"), No("staff-only")),
    p("user_notes", "user_id", Delete, No("staff-only notes about the member")),
    p("unsubscribe_tokens", "user_id", Delete, No("one-time secrets")),
    p("bans", "created_by", Keep("moderation record"), No("staff action")),
    p("bookmarks", "user_id", Delete, Curated),
    p("search_logs", "user_id", Null, Rows(&[])),
    p("invites", "created_by", Keep("invite records, shown as a pseudonym"), Rows(&["code_digest", "note", "used_by"])),
    p("invites", "used_by", Keep("shown as a pseudonym"), No("another member's invite")),
    p("muted_tags", "user_id", Delete, Rows(&[])),
    p("api_keys", "user_id", Delete, Rows(&["key_digest"])),
    p("oauth_codes", "user_id", Delete, No("one-time secrets")),
    p("plugins", "bot_user_id", Keep("a plugin's bot account is not a member"), No("not personal")),
    p("conversations", "starter_id", Keep("other participants keep the conversation"), Rows(&[])),
    p("conversation_messages", "author_id", Keep("other participants keep the conversation"), Curated),
    p("conversation_participants", "user_id", Delete, Rows(&[])),
    p("warnings", "moderator_id", Keep("moderation record"), No("staff action")),
    p("warnings", "user_id", Delete, Curated),
    p("item_timers", "created_by", Keep("staff schedule"), No("staff action")),
    p("clubs", "owner_id", Keep("the club stays; an admin reassigns it"), Rows(&[])),
    p("conversation_reports", "reporter_id", Keep("moderation record"), Rows(&[])),
    p("conversation_reports", "resolved_by", Keep("moderation record"), No("staff action")),
    p("user_follows", "target_id", Delete, No("other members' follows")),
    p("user_follows", "user_id", Delete, Rows(&[])),
    p("group_requests", "user_id", Delete, Rows(&[])),
    p("profile_visits", "profile_id", Delete, No("other members' visits")),
    p("profile_visits", "visitor_id", Delete, Rows(&[])),
    p("push_subscriptions", "user_id", Delete, Rows(&["p256dh", "auth"])),
    p("referral_rewards", "user_id", Delete, Rows(&[])),
    p("ai_usage", "user_id", Null, No("a counter for the provider bill and quotas")),
    p("upload_quota", "user_id", Delete, No("an operational counter of the last day")),
    p("user_badges", "user_id", Delete, Rows(&[])),
    p("poll_votes", "user_id", Delete, Rows(&[])),
    p("member_statuses", "user_id", Delete, Curated),
    p("member_status_replies", "author_id", Delete, Curated),
    p("club_invites", "user_id", Delete, Rows(&[])),
    p("chat_channels", "created_by", Keep("the channel stays"), No("staff action")),
    p("chat_members", "user_id", Delete, Rows(&[])),
    p("chat_messages", "deleted_by", Keep("moderation record"), No("staff action")),
    p("chat_messages", "user_id", Keep("other members keep the conversation"), Rows(&[])),
    p("chat_reactions", "user_id", Delete, Rows(&[])),
    p("chat_flags", "user_id", Delete, Rows(&[])),
    p("legal_notices", "decided_by", Keep("legal record (notices and actions)"), No("staff action")),
    p("legal_notices", "reporter_id", Keep("legal record (notices and actions)"), No("someone else's notice")),
    p("moderation_statements", "appeal_decided_by", Keep("legal record (statements of reasons)"), No("staff action")),
    p("moderation_statements", "moderator_id", Keep("legal record (statements of reasons)"), No("staff action")),
    p("moderation_statements", "user_id", Keep("legal record (statements of reasons)"), Rows(&[])),
    // Only a remote actor's shadow account is linked here, never a member's own federated actor
    // (that one is the user row itself; its key is removed by `purge_orphan_actor_keys`).
    p("ap_actors", "user_id", Null, No("a remote actor's shadow account")),
    p("member_reports", "reporter_id", Keep("moderation record"), Rows(&[])),
    p("member_reports", "resolved_by", Keep("moderation record"), No("staff action")),
    p("member_reports", "user_id", Delete, No("reports about the member are staff-only")),
    p("held_content", "resolved_by", Keep("moderation record"), No("staff action")),
    p("held_content", "user_id", Delete, Rows(&[])),
    p("event_rsvps", "user_id", Delete, Rows(&[])),
    p("post_votes", "user_id", Delete, Rows(&[])),
    p("topic_votes", "user_id", Delete, Rows(&[])),
    p("explorer_queries", "created_by", Keep("staff tool"), No("staff action")),
    p("live_sessions", "created_by", Keep("staff schedule"), No("staff action")),
    p("live_questions", "author_id", Keep("the session record stays"), Rows(&[])),
    p("live_question_votes", "user_id", Delete, Rows(&[])),
    p("content_translations", "translator_id", Keep("translated content stays"), No("not personal")),
];

/// Everything the site holds that this member wrote or chose. Password hashes and secrets are
/// left out, and so are staff notes. Each kind is limited to 50,000 rows; `truncated` names the
/// kinds that were cut. Besides the hand-written sections, `data` has one entry per table in
/// [`REGISTRY`] marked [`Export::Rows`].
pub async fn export(pool: &PgPool, user: i64) -> Result<Value, Error> {
    let mut data = serde_json::Map::new();
    let mut cut: Vec<Value> = Vec::new();
    for entry in REGISTRY {
        let Export::Rows(hidden) = entry.export else { continue };
        let hidden: Vec<String> = hidden.iter().chain(EXPORT_NEVER.iter()).map(|c| (*c).to_owned()).collect();
        // Table and column names come from the constant registry; every value is bound.
        let sql = format!(
            "SELECT coalesce(jsonb_agg(r), '[]'::jsonb) FROM (SELECT to_jsonb(t) - $2::text[] AS r FROM {} t \
             WHERE t.{} = $1 LIMIT {}) s",
            entry.table,
            entry.column,
            EXPORT_LIMIT + 1
        );
        let mut rows: Value =
            sqlx::query_scalar(sqlx::AssertSqlSafe(sql)).bind(user).bind(hidden).fetch_one(pool).await?;
        if let Some(list) = rows.as_array_mut()
            && list.len() > EXPORT_LIMIT
        {
            list.truncate(EXPORT_LIMIT);
            cut.push(Value::from(entry.table));
        }
        data.insert(entry.table.to_owned(), rows);
    }
    let mut out: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
           'account', (SELECT jsonb_build_object(
               'username', username, 'display_name', display_name, 'email', email, 'title', title,
               'about', about_md, 'location', location, 'website', website, 'timezone', timezone,
               'locale', locale, 'joined_at', joined_at, 'last_seen_at', last_seen_at,
               'post_count', post_count, 'reputation', reputation, 'trust_level', trust_level)
             FROM users WHERE id = $1),
           'profile_fields', coalesce((SELECT jsonb_agg(to_jsonb(v) - 'user_id') FROM user_field_values v WHERE v.user_id = $1), '[]'),
           'posts', coalesce((SELECT jsonb_agg(jsonb_build_object('topic', p.item_id, 'number', p.post_number, 'text', p.raw, 'created_at', p.created_at, 'ip', host(p.ip)))
                              FROM (SELECT * FROM posts WHERE author_id = $1 ORDER BY id LIMIT 50000) p), '[]'),
           'messages_sent', coalesce((SELECT jsonb_agg(jsonb_build_object('conversation', m.conversation_id, 'text', m.raw, 'created_at', m.created_at))
                              FROM (SELECT * FROM conversation_messages WHERE author_id = $1 ORDER BY id LIMIT 50000) m), '[]'),
           'reactions', coalesce((SELECT jsonb_agg(jsonb_build_object('post', l.post_id, 'created_at', l.created_at))
                              FROM (SELECT * FROM post_likes WHERE user_id = $1 LIMIT 50000) l), '[]'),
           'status_updates', coalesce((SELECT jsonb_agg(jsonb_build_object('text', s.body, 'created_at', s.created_at))
                              FROM (SELECT * FROM member_statuses WHERE user_id = $1 LIMIT 50000) s), '[]'),
           'status_replies', coalesce((SELECT jsonb_agg(jsonb_build_object('status', r.status_id, 'text', r.body, 'created_at', r.created_at))
                              FROM (SELECT * FROM member_status_replies WHERE author_id = $1 LIMIT 50000) r), '[]'),
           'bookmarks', coalesce((SELECT jsonb_agg(jsonb_build_object('post', b.post_id, 'created_at', b.created_at)) FROM bookmarks b WHERE b.user_id = $1), '[]'),
           'uploads', coalesce((SELECT jsonb_agg(to_jsonb(u) - 'user_id') FROM (SELECT * FROM uploads WHERE user_id = $1 LIMIT 50000) u), '[]'),
           'warnings', coalesce((SELECT jsonb_agg(jsonb_build_object('points', w.points, 'reason', w.reason, 'created_at', w.created_at)) FROM warnings w WHERE w.user_id = $1), '[]'),
           'legal_acceptances', coalesce((SELECT jsonb_agg(to_jsonb(a) - 'user_id') FROM user_legal_acceptances a WHERE a.user_id = $1), '[]'),
           'truncated', coalesce((SELECT jsonb_agg(k) FROM (VALUES
               ('posts', (SELECT count(*) FROM posts WHERE author_id = $1)),
               ('messages_sent', (SELECT count(*) FROM conversation_messages WHERE author_id = $1)),
               ('reactions', (SELECT count(*) FROM post_likes WHERE user_id = $1)),
               ('status_updates', (SELECT count(*) FROM member_statuses WHERE user_id = $1)),
               ('status_replies', (SELECT count(*) FROM member_status_replies WHERE author_id = $1)),
               ('uploads', (SELECT count(*) FROM uploads WHERE user_id = $1))) v(k, n) WHERE n > 50000), '[]'))",
    )
    .bind(user)
    .fetch_one(pool)
    .await?;
    if let Some(obj) = out.as_object_mut() {
        if let Some(Value::Array(list)) = obj.get_mut("truncated") {
            list.extend(cut);
        }
        obj.insert("data".to_owned(), Value::Object(data));
        obj.insert("plugins".to_owned(), crate::platform::plugins::export_user(pool, user).await?);
    }
    Ok(out)
}

/// A member's uploaded files: (storage key, original file name, size), oldest first.
pub async fn upload_files(pool: &PgPool, user: i64, limit: i64) -> Result<Vec<(String, String, i64)>, Error> {
    sqlx::query_as("SELECT storage_key, filename, size FROM uploads WHERE user_id = $1 ORDER BY id LIMIT $2")
        .bind(user)
        .bind(limit)
        .fetch_all(pool)
        .await
}

pub async fn request_deletion(pool: &PgPool, user: i64, days: i32) -> Result<(), Error> {
    sqlx::query("UPDATE users SET delete_after = now() + make_interval(days => $2) WHERE id = $1")
        .bind(user)
        .bind(days)
        .execute(pool)
        .await
        .map(|_| ())
}

crate::sql::set_col!(
/// Whether the member's posts go with the account (`true`) or stay under a pseudonym.
    set_delete_content, "users", "delete_content", i64, bool);

pub async fn delete_content_chosen(pool: &PgPool, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT delete_content FROM users WHERE id = $1").bind(user).fetch_one(pool).await
}

pub async fn cancel_deletion(pool: &PgPool, user: i64) -> Result<(), Error> {
    sqlx::query("UPDATE users SET delete_after = NULL WHERE id = $1").bind(user).execute(pool).await.map(|_| ())
}

pub async fn deletion_date(pool: &PgPool, user: i64) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT delete_after FROM users WHERE id = $1").bind(user).fetch_one(pool).await
}

async fn anonymize_in(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, user: i64) -> Result<(), Error> {
    // The member chose to take their posts along: they are deleted (counters follow) and their
    // text is overwritten, so nothing readable stays even for staff.
    let delete_content: bool =
        sqlx::query_scalar("SELECT delete_content FROM users WHERE id = $1").bind(user).fetch_one(&mut **tx).await?;
    if delete_content {
        let posts: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM posts WHERE author_id = $1 AND state <> 'deleted' ORDER BY id")
                .bind(user)
                .fetch_all(&mut **tx)
                .await?;
        for id in posts {
            crate::forum::posts::set_post_state_with_reason(tx, id, "deleted", None, Some("account_erased")).await?;
        }
        // Notifications of other members that carry the title of a topic they started: the title
        // is blanked (the text falls back) and the slug in the link no longer spells it. The
        // notification itself stays, the least destructive way to drop the erased text.
        sqlx::query(
            "UPDATE notifications n SET data = n.data || jsonb_build_object('title', '')
               || CASE WHEN n.data ? 'url' THEN jsonb_build_object('url', regexp_replace(n.data->>'url', '^/t/[^/]+/', '/t/deleted/'))
                       ELSE '{}'::jsonb END
             WHERE n.user_id <> $1 AND jsonb_typeof(n.data->'url') = 'string' AND n.data ? 'title'
               AND EXISTS (SELECT 1 FROM items i WHERE i.author_id = $1
                           AND n.data->>'url' ~ ('^/t/[^/]+/' || i.id || '([#/?]|$)'))",
        )
        .bind(user)
        .execute(&mut **tx)
        .await?;
        sqlx::query("UPDATE posts SET raw = '', cooked = '' WHERE author_id = $1")
            .bind(user)
            .execute(&mut **tx)
            .await?;
        // Earlier versions of those posts are the same text: they go too.
        sqlx::query("DELETE FROM post_revisions WHERE post_id IN (SELECT id FROM posts WHERE author_id = $1)")
            .bind(user)
            .execute(&mut **tx)
            .await?;
    }
    // Other members' notifications that name this member ("ann replied") keep their meaning but
    // say "deleted-N", like the posts do. The name is read before it is changed below.
    sqlx::query(
        "UPDATE notifications n SET data = n.data || coalesce((
           SELECT jsonb_object_agg(k, v) FROM (
             SELECT 'by' AS k, to_jsonb(u.pseudonym) AS v WHERE n.data->>'by' = u.username
             UNION ALL SELECT 'author', to_jsonb(u.pseudonym) WHERE n.data->>'author' = u.username
             UNION ALL SELECT 'users', (SELECT jsonb_agg(CASE WHEN e #>> '{}' = u.username THEN to_jsonb(u.pseudonym) ELSE e END)
                                         FROM jsonb_array_elements(n.data->'users') e)
               WHERE jsonb_typeof(n.data->'users') = 'array' AND n.data->'users' @> to_jsonb(u.username)
           ) x), '{}'::jsonb)
         FROM (SELECT username, 'deleted-' || id AS pseudonym FROM users WHERE id = $1) u
         WHERE n.user_id <> $1 AND (n.data->>'by' = u.username OR n.data->>'author' = u.username
            OR (jsonb_typeof(n.data->'users') = 'array' AND n.data->'users' @> to_jsonb(u.username)))",
    )
    .bind(user)
    .execute(&mut **tx)
    .await?;
    // Topics they voted for lose the votes below; their counters are recounted afterwards.
    let voted: Vec<i64> = sqlx::query_scalar("SELECT item_id FROM topic_votes WHERE user_id = $1")
        .bind(user)
        .fetch_all(&mut **tx)
        .await?;
    for entry in REGISTRY {
        // Table and column names come from the constant registry; the member id is bound.
        let sql = match entry.erase {
            Erase::Keep(_) => continue,
            Erase::Delete => format!("DELETE FROM {} WHERE {} = $1", entry.table, entry.column),
            Erase::Null => format!("UPDATE {t} SET {c} = NULL WHERE {c} = $1", t = entry.table, c = entry.column),
            Erase::Scrub(columns) => {
                let set: Vec<String> = columns.iter().map(|c| format!("{c} = NULL")).collect();
                format!("UPDATE {} SET {} WHERE {} = $1", entry.table, set.join(", "), entry.column)
            }
        };
        sqlx::query(sqlx::AssertSqlSafe(sql)).bind(user).execute(&mut **tx).await?;
    }
    sqlx::query(
        "UPDATE items SET vote_count = (SELECT count(*) FROM topic_votes WHERE item_id = items.id) WHERE id = ANY($1)",
    )
    .bind(&voted)
    .execute(&mut **tx)
    .await?;
    // The federated actor: remote servers stop hearing about it (the Delete activity is queued by
    // the caller before this runs) and its private key goes once that is delivered.
    sqlx::query("DELETE FROM ap_followers WHERE local_type = 'user' AND local_id = $1")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    // Plugins' rows that name this member (their declared `user_column`) and `user:<id>:` keys.
    crate::platform::plugins::purge_user(tx, user).await?;
    sqlx::query("UPDATE posts SET ip = NULL WHERE author_id = $1").bind(user).execute(&mut **tx).await?;
    sqlx::query("DELETE FROM mail_outbox WHERE sent_at IS NULL AND to_addr = (SELECT email FROM users WHERE id = $1)")
        .bind(user)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE users SET username = 'deleted-' || id, username_lower = 'deleted-' || id, username_skeleton = NULL,
           email = 'deleted-' || id || '@deleted.invalid', email_lower = 'deleted-' || id || '@deleted.invalid',
           display_name = NULL, email_verified_at = NULL, password_hash = NULL, primary_group_id = 2, title = NULL,
           about_md = NULL, about_html = NULL, timezone = NULL, locale = NULL, location = NULL, website = NULL,
           avatar_upload_id = NULL, cover_upload_id = NULL, totp_secret = NULL, totp_last_step = NULL,
           last_seen_at = NULL, theme_id = NULL, color_scheme = NULL, digest = 'none',
           flags = '{}', mail_post_key = NULL, delete_after = NULL, delete_content = false, allow_pm_from = 'everyone', signature_md = NULL, signature_html = NULL,
           ap_also_known_as = '{}', ap_moved_to = NULL,
           newsletter = false, newsletter_changed_at = CASE WHEN newsletter THEN now() ELSE newsletter_changed_at END
         WHERE id = $1",
    )
    .bind(user)
    .execute(&mut **tx)
    .await?;
    // Separately: the trigger on changing the address would clear this in the same statement.
    sqlx::query("UPDATE users SET email_bad = true WHERE id = $1").bind(user).execute(&mut **tx).await?;
    Ok(())
}

/// Removes who someone was but keeps their posts, shown as "deleted-123", so threads still read.
/// Later messages in their conversations stay for the other people in them.
pub async fn anonymize(pool: &PgPool, user: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    anonymize_in(&mut tx, user).await?;
    record(&mut *tx, Entry::new("user.anonymize").on(user.to_string())).await?;
    tx.commit().await
}

/// Deletes the federation private keys of accounts that no longer exist as members, once no
/// queued delivery still needs them to be signed (the actor Delete activity is one).
pub async fn purge_orphan_actor_keys(pool: &PgPool) -> Result<u64, Error> {
    Ok(sqlx::query(
        "DELETE FROM ap_keys k WHERE k.owner_type = 'user'
           AND EXISTS (SELECT 1 FROM users u WHERE u.id = k.owner_id AND u.email_lower LIKE 'deleted-%@deleted.invalid')
           AND NOT EXISTS (SELECT 1 FROM ap_deliveries d WHERE d.signer_type = 'user' AND d.signer_id = k.owner_id)",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Accounts whose grace period is over.
pub async fn due_deletions(pool: &PgPool) -> Result<Vec<i64>, Error> {
    // Promoted during the grace period: an admin is not removed by a timer.
    sqlx::query_scalar(
        "SELECT u.id FROM users u WHERE u.delete_after <= now()
           AND NOT EXISTS (SELECT 1 FROM groups g WHERE g.is_admin AND (g.id = u.primary_group_id
                OR g.id IN (SELECT group_id FROM user_secondary_groups s WHERE s.user_id = u.id)))
         LIMIT 100",
    )
    .fetch_all(pool)
    .await
}

/// Deletes reactions and takes back what they added: the post's and topic's like counts and the
/// reputation of the post's author. `$1` is the member whose reactions go, `$2` the one they merge into.
macro_rules! drop_likes {
    ($found:literal) => {
        concat!(
            "WITH gone AS (DELETE FROM post_likes l ", $found, " RETURNING l.post_id, l.reaction_id),
                  per AS (SELECT g.post_id, count(*)::int AS n, coalesce(sum(r.score), 0)::bigint AS score
                          FROM gone g LEFT JOIN reactions r ON r.id = g.reaction_id GROUP BY g.post_id),
                  pu AS (UPDATE posts SET like_count = greatest(0, like_count - per.n) FROM per
                         WHERE posts.id = per.post_id RETURNING posts.item_id, posts.author_id, per.n, per.score),
                  iu AS (UPDATE items SET like_count = greatest(0, like_count - s.n)
                         FROM (SELECT item_id, sum(n)::int AS n FROM pu GROUP BY item_id) s WHERE items.id = s.item_id)
             UPDATE users SET reputation = reputation - s.score
             FROM (SELECT author_id, sum(score)::bigint AS score FROM pu GROUP BY author_id) s WHERE users.id = s.author_id"
        )
    };
}
const DROP_LIKES_OWN: &str =
    drop_likes!("USING posts p WHERE l.post_id = p.id AND l.user_id = $1 AND p.author_id = $2");
const DROP_LIKES_DUPLICATE: &str =
    drop_likes!("USING post_likes b WHERE l.post_id = b.post_id AND l.user_id = $1 AND b.user_id = $2");

/// Moves content, reactions, follows and conversations to `into`, then anonymizes `from`.
pub async fn merge(pool: &PgPool, from: i64, into: i64, actor: Option<i64>) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for sql in [
        "UPDATE items SET author_id = $2 WHERE author_id = $1",
        "UPDATE items SET last_poster_id = $2 WHERE last_poster_id = $1",
        "UPDATE posts SET author_id = $2 WHERE author_id = $1",
        "UPDATE posts SET last_editor_id = $2 WHERE last_editor_id = $1",
        "UPDATE uploads SET user_id = $2 WHERE user_id = $1",
        "UPDATE conversations SET starter_id = $2 WHERE starter_id = $1",
        "UPDATE conversation_messages SET author_id = $2 WHERE author_id = $1",
        "INSERT INTO conversation_participants (conversation_id, user_id, last_read_id, archived, left_at)
           SELECT conversation_id, $2, last_read_id, archived, left_at FROM conversation_participants WHERE user_id = $1
           ON CONFLICT DO NOTHING",
        "INSERT INTO bookmarks (user_id, post_id, created_at, remind_at)
           SELECT $2, post_id, created_at, remind_at FROM bookmarks WHERE user_id = $1 ON CONFLICT DO NOTHING",
        // A reaction to your own post would break the rule against it, so those are dropped, and
        // with them the counts and the author's reputation they gave.
        DROP_LIKES_OWN,
        DROP_LIKES_DUPLICATE,
        "INSERT INTO topic_votes (item_id, user_id, created_at)
           SELECT item_id, $2, created_at FROM topic_votes WHERE user_id = $1 ON CONFLICT DO NOTHING",
        "UPDATE post_likes SET user_id = $2 WHERE user_id = $1",
        "UPDATE users t SET post_count = t.post_count + s.post_count, reputation = t.reputation + s.reputation
           FROM users s WHERE t.id = $2 AND s.id = $1",
        "UPDATE users SET post_count = 0, reputation = 0 WHERE id = $1",
        "UPDATE member_statuses SET user_id = $2 WHERE user_id = $1",
        "UPDATE member_status_replies SET author_id = $2 WHERE author_id = $1",
        "UPDATE chat_messages SET user_id = $2 WHERE user_id = $1",
        "INSERT INTO user_badges (user_id, badge_id, earned_at)
           SELECT $2, badge_id, earned_at FROM user_badges WHERE user_id = $1 ON CONFLICT DO NOTHING",
        "INSERT INTO poll_votes (item_id, poll_id, user_id, option_id, rank)
           SELECT item_id, poll_id, $2, option_id, rank FROM poll_votes WHERE user_id = $1 ON CONFLICT DO NOTHING",
        "INSERT INTO user_follows (user_id, target_id, created_at)
           SELECT $2, target_id, created_at FROM user_follows WHERE user_id = $1 AND target_id <> $2 ON CONFLICT DO NOTHING",
        "INSERT INTO user_follows (user_id, target_id, created_at)
           SELECT user_id, $2, created_at FROM user_follows WHERE target_id = $1 AND user_id <> $2 ON CONFLICT DO NOTHING",
        "INSERT INTO event_rsvps (item_id, user_id, status, created_at)
           SELECT item_id, $2, status, created_at FROM event_rsvps WHERE user_id = $1 ON CONFLICT DO NOTHING",
    ] {
        sqlx::query(sql).bind(from).bind(into).execute(&mut *tx).await?;
    }
    anonymize_in(&mut tx, from).await?;
    record(
        &mut *tx,
        Entry::new("user.merge").by(actor).on(from.to_string()).details(serde_json::json!({ "into": into })),
    )
    .await?;
    tx.commit().await
}
