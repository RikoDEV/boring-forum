//! Users: lookup, creation, renames, approval and trust promotion.

use crate::admin::audit::{Entry, record};
use bf_core::ids::{ForumId, GroupId, UserId};
use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub password_hash: Option<String>,
    pub primary_group_id: GroupId,
    pub is_admin: bool,
    pub silenced: bool,
    pub hide_last_seen: bool,
    pub post_count: i32,
    pub email_verified: bool,
    pub approved: bool,
    /// A newer version of the terms or privacy policy awaits acceptance.
    pub legal_pending: bool,
    /// A group requires two-factor and the user has no authenticator (TOTP). A passkey on the
    /// account does not count: a session opened with the password alone would pass for it, and a
    /// session does not record how it was opened.
    pub needs_2fa: bool,
    /// This session recently proved who it is. Only meaningful for session lookups.
    #[sqlx(default)]
    pub sudo: bool,
    /// This session was opened by an administrator as this member. Only for session lookups.
    #[sqlx(default)]
    pub impersonated: bool,
    /// The administrator behind an impersonated session. Only for session lookups.
    #[sqlx(default)]
    pub impersonated_by: Option<i64>,
    pub locale: Option<String>,
    pub text_size: i16,
    pub density: String,
    pub infinite_scroll: bool,
    pub date_format: String,
    pub skin_tone: i16,
    pub trust_level: i16,
    pub unread_notifications: i64,
    pub unread_messages: i64,
    pub unread_chat: i64,
    pub theme_id: Option<i64>,
    pub color_scheme: Option<String>,
    pub groups: Vec<GroupId>,
    pub moderated_nodes: Vec<ForumId>,
    /// A new member whose getting-started banner is still wanted: an account under two weeks old,
    /// not dismissed, with a step left. Only for lookups that select `USER_COLS`.
    #[sqlx(default)]
    pub welcome_open: bool,
}

macro_rules! user_cols {
    () => {
        "u.id, u.username, u.email, u.password_hash, u.primary_group_id,
    EXISTS (SELECT 1 FROM groups g WHERE g.is_admin AND (g.id = u.primary_group_id
        OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id))) AS is_admin,
    coalesce(u.silenced_until > now(), false) AS silenced, u.hide_last_seen, u.post_count, u.email_verified_at IS NOT NULL AS email_verified, u.approved,
    EXISTS (SELECT 1 FROM (SELECT DISTINCT ON (kind) id FROM legal_documents WHERE kind IN ('terms', 'privacy')
                           ORDER BY kind, version DESC) d
            WHERE NOT EXISTS (SELECT 1 FROM user_legal_acceptances a WHERE a.user_id = u.id AND a.document_id = d.id)) AS legal_pending,
    (EXISTS (SELECT 1 FROM groups g WHERE g.require_2fa AND (g.id = u.primary_group_id
        OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id)))
     AND u.totp_secret IS NULL) AS needs_2fa,
    u.locale, u.text_size, u.density, u.infinite_scroll, u.date_format, u.skin_tone, u.trust_level, u.theme_id, u.color_scheme,
    (SELECT count(*) FROM notifications n WHERE n.user_id = u.id AND NOT n.read) AS unread_notifications,
    (SELECT count(*) FROM conversation_participants p JOIN conversations c ON c.id = p.conversation_id
       WHERE p.user_id = u.id AND p.left_at IS NULL AND NOT p.archived
         AND c.last_message_id > p.last_read_id) AS unread_messages,
    coalesce((SELECT sum((SELECT count(*) FROM chat_messages x WHERE x.channel_id = m.channel_id
                AND x.id > m.last_read_id AND x.deleted_at IS NULL AND x.user_id IS DISTINCT FROM u.id))
              FROM chat_members m WHERE m.user_id = u.id AND NOT m.muted), 0)::bigint AS unread_chat,
    array_prepend(u.primary_group_id, coalesce((SELECT array_agg(group_id) FROM user_secondary_groups WHERE user_id = u.id), '{}')) AS groups,
    coalesce((SELECT array_agg(node_id) FROM node_moderators WHERE user_id = u.id), '{}') AS moderated_nodes,
    (u.welcome_dismissed_at IS NULL AND u.joined_at > now() - interval '14 days'
     AND NOT (u.avatar_upload_id IS NOT NULL AND coalesce(u.about_md, '') <> '' AND u.post_count > 0
              AND u.email_verified_at IS NOT NULL)) AS welcome_open"
    };
}
pub(crate) const USER_COLS: &str = user_cols!();

/// Serialises every account creation and rename, so two look-alike names cannot both pass the
/// skeleton check before either is stored.
pub(crate) const SKELETON_LOCK: i64 = 0x6266_5f73_6b65_6c65;

/// The message of the error returned when a name looks like another member's.
const SKELETON_TAKEN: &str = "username skeleton taken";

/// Did this error come from a confusable name (a look-alike of another account)?
pub fn is_skeleton_taken(e: &Error) -> bool {
    matches!(e, Error::Protocol(m) if m == SKELETON_TAKEN)
        || matches!(e, Error::Database(d) if d.is_unique_violation() && d.constraint() == Some("users_username_skeleton_key"))
}

/// Takes the name lock and checks, inside the caller's transaction, that no other local account
/// has a name with this skeleton. Returns the skeleton to store with the name.
pub(crate) async fn claim_skeleton(
    tx: &mut sqlx::PgConnection,
    username: &str,
    except_user: Option<i64>,
) -> Result<String, Error> {
    let skeleton = bf_core::auth::username_skeleton(username);
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(SKELETON_LOCK).execute(&mut *tx).await?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE username_skeleton = $1 AND NOT remote AND id <> COALESCE($2, 0))",
    )
    .bind(&skeleton)
    .bind(except_user)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Err(Error::Protocol(SKELETON_TAKEN.into()));
    }
    Ok(skeleton)
}

/// What an account is, for the checks it goes through when it is made.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    /// A person: the look-alike check applies.
    Member,
    /// A plugin's or persona's account: no sign-in, but a local name like any other.
    Bot,
    /// A remote actor's shadow account: named `name@domain`, outside the look-alike check.
    Remote,
}

/// Everything needed to make an account. This is the only way one is made: the name is claimed
/// under the name lock, its skeleton is stored with it, and a trigger refuses a local account
/// without one (see the `users_require_skeleton` migration).
pub struct NewUser<'a> {
    pub username: &'a str,
    pub email: &'a str,
    pub password_hash: Option<&'a str>,
    pub display_name: Option<&'a str>,
    /// `None`: members, or administrators for the very first account of a site.
    pub primary_group: Option<i64>,
    pub joined_at: Option<DateTime<Utc>>,
    pub verified: bool,
    pub kind: AccountKind,
    /// Try `name1`, `name2`... when the name is taken or looks like another member's, instead of
    /// failing with that error.
    pub fit_name: bool,
}

/// Makes an account inside the caller's transaction and returns its id and the name it got.
pub async fn insert_user(tx: &mut sqlx::PgConnection, u: &NewUser<'_>) -> Result<(UserId, String), Error> {
    use sqlx::Acquire;
    for n in 0..(if u.fit_name { 50 } else { 1 }) {
        let name = if n == 0 { u.username.to_owned() } else { format!("{}{n}", u.username) };
        // A savepoint per try: a taken name aborts the statement, not the whole transaction.
        let mut attempt = tx.begin().await?;
        let skeleton = if u.kind == AccountKind::Remote {
            None
        } else {
            match claim_skeleton(&mut attempt, &name, None).await {
                Ok(s) => Some(s),
                Err(e) if u.fit_name && is_skeleton_taken(&e) => {
                    attempt.rollback().await?;
                    continue;
                }
                Err(e) => return Err(e),
            }
        };
        let made: Result<UserId, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO users (username, username_lower, display_name, email, email_lower, password_hash,
                                primary_group_id, joined_at, email_verified_at, remote, username_skeleton)
             VALUES ($1, lower($1), $2, $3, lower($3), $4,
                     COALESCE($5::bigint, CASE WHEN EXISTS (SELECT 1 FROM users) THEN 2 ELSE 4 END),
                     COALESCE($6::timestamptz, now()), CASE WHEN $7 THEN now() END, $8, $9)
             RETURNING id",
        )
        .bind(&name)
        .bind(u.display_name)
        .bind(u.email)
        .bind(u.password_hash)
        .bind(u.primary_group)
        .bind(u.joined_at)
        .bind(u.verified)
        .bind(u.kind == AccountKind::Remote)
        .bind(skeleton)
        .fetch_one(&mut *attempt)
        .await;
        match made {
            Err(sqlx::Error::Database(e))
                if u.fit_name && e.is_unique_violation() && e.constraint() == Some("users_username_lower_key") =>
            {
                attempt.rollback().await?;
            }
            Ok(id) => {
                attempt.commit().await?;
                if u.kind == AccountKind::Member {
                    crate::community::chat::join_auto_channels(&mut *tx, id.get()).await?;
                }
                return Ok((id, name));
            }
            Err(e) => return Err(e),
        }
    }
    Err(Error::Protocol("no free username".into()))
}

/// Would this display name pass for a staff member or a reserved name? Compared by skeleton, so
/// "Admin1strator" and one with a Cyrillic letter count. `except_user` is the member renaming
/// themselves.
pub async fn display_name_impersonates(
    pool: &PgPool,
    display_name: &str,
    except_user: Option<i64>,
) -> Result<bool, Error> {
    let skeleton = bf_core::auth::username_skeleton(display_name);
    if skeleton.is_empty() {
        return Ok(false);
    }
    if bf_core::auth::is_reserved_skeleton(&skeleton) {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users u LEFT JOIN groups g ON g.id = u.primary_group_id
                        WHERE u.username_skeleton = $1 AND NOT u.remote AND u.id <> COALESCE($2, 0)
                          AND (g.is_staff OR g.is_moderator OR u.primary_group_id IN (3, 4)))",
    )
    .bind(&skeleton)
    .bind(except_user)
    .fetch_one(pool)
    .await
}

pub async fn create_user(pool: &PgPool, username: &str, email: &str, password_hash: &str) -> Result<UserId, Error> {
    // The first account becomes an administrator so a fresh install can be configured.
    // Under a lock, so two registrations on an empty site cannot both see "no users yet".
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(crate::accounts::security::FIRST_USER_LOCK)
        .execute(&mut *tx)
        .await?;
    let (id, _) = insert_user(
        &mut tx,
        &NewUser {
            username,
            email,
            password_hash: Some(password_hash),
            display_name: None,
            primary_group: None,
            joined_at: None,
            verified: false,
            kind: AccountKind::Member,
            fit_name: false,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(id)
}

pub async fn user_by_login(pool: &PgPool, login: &str) -> Result<Option<User>, Error> {
    // Only the constant USER_COLS is interpolated; every value is bound.
    sqlx::query_as(concat!(
        "SELECT ",
        user_cols!(),
        " FROM users u WHERE (u.username_lower = lower($1) OR u.email_lower = lower($1))
         AND (u.suspended_until IS NULL OR u.suspended_until < now())"
    ))
    .bind(login)
    .fetch_optional(pool)
    .await
}

#[derive(Debug, Clone, FromRow)]
pub struct Profile {
    pub id: UserId,
    pub username: String,
    pub title: Option<String>,
    pub about_html: Option<String>,
    pub post_count: i32,
    pub joined_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub group_name: String,
    pub location: Option<String>,
    pub website: Option<String>,
    pub display_name: Option<String>,
    pub hide_last_seen: bool,
    pub profile_visibility: String,
    pub cover_key: Option<String>,
    pub cover_y: i16,
    pub likes_received: i64,
    pub reputation: i64,
    pub solved_count: i64,
    pub status_text: Option<String>,
    pub show_visitors: bool,
    pub profile_views: i64,
    pub hide_followers: bool,
    pub group_colour: Option<String>,
    pub group_icon: Option<String>,
    pub group_image: Option<String>,
    pub trust_level: i16,
}

pub async fn profile(pool: &PgPool, id: UserId) -> Result<Option<Profile>, Error> {
    sqlx::query_as(
        "SELECT u.id, u.username, u.title, u.about_html, u.post_count, u.joined_at, u.last_seen_at, g.name AS group_name, u.location, u.website,
                u.display_name, u.hide_last_seen, u.profile_visibility, cu.storage_key AS cover_key,
                u.reputation, u.show_visitors, u.profile_views, u.hide_followers, u.cover_y,
                (SELECT s.body FROM member_statuses s WHERE s.user_id = u.id ORDER BY s.id DESC LIMIT 1) AS status_text,
                (SELECT count(*) FROM items si JOIN posts sp ON sp.item_id = si.id AND sp.post_number = si.solved_post_number
                 WHERE sp.author_id = u.id AND si.state = 'visible') AS solved_count,
                coalesce((SELECT sum(like_count) FROM posts WHERE author_id = u.id AND state = 'visible'), 0)::bigint AS likes_received,
                g.colour AS group_colour, g.icon AS group_icon, g.image AS group_image, u.trust_level
         FROM users u JOIN groups g ON g.id = u.primary_group_id LEFT JOIN uploads cu ON cu.id = u.cover_upload_id
         WHERE u.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

pub async fn user_id_by_name(pool: &PgPool, username: &str) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE username_lower = lower($1)")
        .bind(username)
        .fetch_optional(pool)
        .await
}

#[derive(Debug, Clone, FromRow)]
pub struct ActivityRow {
    pub item_id: i64,
    pub title: String,
    pub slug: String,
    pub post_number: i32,
    pub cooked: String,
    pub created_at: DateTime<Utc>,
}

/// A member's recent posts, limited to forums the viewer can read.
pub async fn user_posts(
    pool: &PgPool,
    user_id: UserId,
    visible_nodes: &[i64],
    limit: i64,
) -> Result<Vec<ActivityRow>, Error> {
    sqlx::query_as(
        "SELECT i.id AS item_id, i.title, i.slug, p.post_number, p.cooked, p.created_at
         FROM posts p JOIN items i ON i.id = p.item_id
         WHERE p.author_id = $1 AND p.state = 'visible' AND p.action IS NULL
           AND i.state = 'visible' AND i.listed AND i.node_id = ANY($2)
         ORDER BY p.created_at DESC LIMIT $3",
    )
    .bind(user_id)
    .bind(visible_nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Several users, with the same columns as a session lookup.
pub async fn users_by_ids(pool: &PgPool, ids: &[i64]) -> Result<Vec<User>, Error> {
    // Only the constant USER_COLS is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", user_cols!(), " FROM users u WHERE u.id = ANY($1)"))
        .bind(ids)
        .fetch_all(pool)
        .await
}

pub async fn user_ids_by_names(pool: &PgPool, names_lower: &[String]) -> Result<Vec<UserId>, Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE username_lower = ANY($1)").bind(names_lower).fetch_all(pool).await
}

crate::sql::set_col!(set_approved, "users", "approved", UserId, bool);

/// Topics a member started, limited to forums the viewer can read (profile "Topics" tab).
pub async fn user_topics(
    pool: &PgPool,
    user_id: UserId,
    visible_nodes: &[i64],
    limit: i64,
) -> Result<Vec<(i64, String, String, i32, DateTime<Utc>)>, Error> {
    sqlx::query_as(
        "SELECT id, slug, title, reply_count, created_at FROM items
         WHERE author_id = $1 AND state = 'visible' AND listed AND node_id = ANY($2) ORDER BY created_at DESC LIMIT $3",
    )
    .bind(user_id)
    .bind(visible_nodes)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Staff change a topic's author (its first post goes with it); counters follow.
pub async fn change_author(pool: &PgPool, item_id: i64, new_author: i64, actor: i64) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    let old: Option<i64> = sqlx::query_scalar("SELECT author_id FROM items WHERE id = $1 FOR UPDATE")
        .bind(item_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE items SET author_id = $2 WHERE id = $1")
        .bind(item_id)
        .bind(new_author)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE posts SET author_id = $2 WHERE item_id = $1 AND post_number = 1")
        .bind(item_id)
        .bind(new_author)
        .execute(&mut *tx)
        .await?;
    // The same formula as everywhere else: visible post in a visible topic of a forum that counts posts.
    sqlx::query(
        "UPDATE users u SET post_count = (SELECT count(*) FROM posts p JOIN items i ON i.id = p.item_id JOIN nodes n ON n.id = i.node_id
            WHERE p.author_id = u.id AND p.state = 'visible' AND p.action IS NULL AND i.state = 'visible' AND n.counts_posts)
         WHERE u.id = $1 OR u.id = $2",
    )
    .bind(new_author)
    .bind(old)
    .execute(&mut *tx)
    .await?;
    // `last_poster_id` may have been the old author (a topic with only its first post).
    crate::moderation::topics::recount_item(&mut tx, item_id).await?;
    record(
        &mut *tx,
        Entry::new("topic.change_author")
            .by(actor)
            .on(format!("item:{item_id}"))
            .details(serde_json::json!({ "from": old, "to": new_author })),
    )
    .await?;
    tx.commit().await
}

/// A member suggested for an `@mention`.
#[derive(Debug, Clone, FromRow)]
pub struct MentionCandidate {
    pub username: String,
    pub display_name: Option<String>,
    /// Posted in the topic being replied to.
    pub in_topic: bool,
}

/// Members for the `@mention` picker: matched on the username or display name, people already in
/// `topic` first, then those seen most recently. An empty prefix lists the topic's participants.
/// Members who keep their profile private are listed only to staff, or when they posted in `topic`.
pub async fn mention_candidates(
    pool: &PgPool,
    prefix: &str,
    topic: Option<i64>,
    exclude: i64,
    limit: i64,
    staff: bool,
) -> Result<Vec<MentionCandidate>, Error> {
    let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    sqlx::query_as(
        "SELECT u.username, u.display_name,
                EXISTS (SELECT 1 FROM posts p WHERE p.item_id = $2 AND p.author_id = u.id AND p.state = 'visible') AS in_topic
         FROM users u
         WHERE u.id <> $3 AND (u.suspended_until IS NULL OR u.suspended_until < now())
           AND (u.approved OR $5)
           AND (u.username ILIKE $1 || '%' OR u.display_name ILIKE $1 || '%')
           AND ($5 OR u.profile_visibility <> 'nobody'
                OR EXISTS (SELECT 1 FROM posts p WHERE p.item_id = $2 AND p.author_id = u.id AND p.state = 'visible'))
           AND ($1 <> '' OR EXISTS (SELECT 1 FROM posts p WHERE p.item_id = $2 AND p.author_id = u.id AND p.state = 'visible'))
         ORDER BY in_topic DESC, u.username ILIKE $1 || '%' DESC,
                  CASE WHEN u.hide_last_seen THEN NULL ELSE u.last_seen_at END DESC NULLS LAST, u.username
         LIMIT $4",
    )
    .bind(escaped)
    .bind(topic.unwrap_or(0))
    .bind(exclude)
    .bind(limit)
    .bind(staff)
    .fetch_all(pool)
    .await
}

/// The same for search results, which must not list members who keep their profile private.
pub async fn visible_usernames_starting(
    pool: &PgPool,
    prefix: &str,
    limit: i64,
    member: bool,
    staff: bool,
) -> Result<Vec<String>, Error> {
    let escaped = prefix.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    sqlx::query_scalar(
        "SELECT username FROM users WHERE approved AND username ILIKE $1 || '%'
           AND ($4 OR profile_visibility = 'everyone' OR ($3 AND profile_visibility = 'members'))
         ORDER BY username LIMIT $2",
    )
    .bind(escaped)
    .bind(limit)
    .bind(member)
    .bind(staff)
    .fetch_all(pool)
    .await
}

crate::sql::set_col!(set_infinite_scroll, "users", "infinite_scroll", UserId, bool);

pub async fn user_count(pool: &PgPool) -> Result<i64, Error> {
    sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(pool).await
}

/// The measures every trust requirement is checked against, as columns of `u`. `recent` is the
/// SQL for the recent-activity window in days (a bound parameter such as `$4`).
fn metric_columns(recent: &str) -> String {
    format!(
        "u.post_count::bigint AS posts, extract(day FROM now() - u.joined_at)::bigint AS days, u.reputation,
         (SELECT count(*) FROM read_markers rm WHERE rm.user_id = u.id AND NOT rm.bulk) + u.read_credit AS topics_read,
         (SELECT count(*) FROM posts p WHERE p.author_id = u.id AND p.state = 'visible' AND p.action IS NULL
            AND p.created_at > now() - make_interval(days => {recent})) AS recent_posts,
         coalesce((SELECT sum(p.like_count) FROM posts p WHERE p.author_id = u.id AND p.state = 'visible'), 0)::bigint
            AS likes_received,
         (SELECT count(*) FROM post_likes l WHERE l.user_id = u.id) AS likes_given,
         (SELECT count(*) FROM user_visits v WHERE v.user_id = u.id) + u.visit_credit AS days_visited"
    )
}

/// A member and the measures their trust level is judged by.
#[derive(Debug, sqlx::FromRow)]
pub struct TrustCandidate {
    pub id: i64,
    pub trust_level: i16,
    pub posts: i64,
    pub days: i64,
    pub topics_read: i64,
    pub reputation: i64,
    pub recent_posts: i64,
    pub likes_received: i64,
    pub likes_given: i64,
    pub days_visited: i64,
    /// The next level this member has already been told they are close to.
    pub nudged_level: i16,
    /// An administrator or moderator account, which level 3 demotion skips.
    pub staff: bool,
}

impl TrustCandidate {
    pub fn metrics(&self) -> bf_core::trust::Metrics {
        bf_core::trust::Metrics {
            posts: self.posts,
            days: self.days,
            topics_read: self.topics_read,
            reputation: self.reputation,
            recent_posts: self.recent_posts,
            likes_received: self.likes_received,
            likes_given: self.likes_given,
            days_visited: self.days_visited,
        }
    }
}

const TRUST_ELIGIBLE: &str = "u.approved AND NOT u.remote AND u.email_lower NOT LIKE '%@plugins.invalid'
    AND NOT u.trust_level_locked AND (u.suspended_until IS NULL OR u.suspended_until < now())
    AND (u.silenced_until IS NULL OR u.silenced_until < now())";

/// Members below the highest earned level whose cheap numbers already reach `posts`, `days` and
/// `reputation` for their next level (element `trust_level` of each list: what level
/// `trust_level + 1` needs), with every measure the evaluator needs. The caller passes the full
/// requirements to find who is ready, or a share of them to find who is close. Staff-set levels,
/// suspended and silenced members, remote and plugin accounts are left out.
pub async fn trust_candidates(
    pool: &PgPool,
    posts: Vec<i64>,
    days: Vec<i32>,
    reputation: Vec<i64>,
    recent_days: i32,
) -> Result<Vec<TrustCandidate>, Error> {
    if posts.is_empty() {
        return Ok(vec![]);
    }
    let cols = metric_columns("$4");
    // Only constant SQL is interpolated; every value is bound.
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.id, u.trust_level, u.trust_nudged_level AS nudged_level, u.primary_group_id IN (3, 4) AS staff, {cols}
         FROM users u
         WHERE u.trust_level < cardinality($1::bigint[]) AND {TRUST_ELIGIBLE}
           AND u.post_count >= ($1::bigint[])[u.trust_level + 1]
           AND u.joined_at <= now() - make_interval(days => ($2::int[])[u.trust_level + 1])
           AND u.reputation >= ($3::bigint[])[u.trust_level + 1]"
    )))
    .bind(posts)
    .bind(days)
    .bind(reputation)
    .bind(recent_days)
    .fetch_all(pool)
    .await
}

/// Every member whose level activity can change (not set by hand, not staff-only accounts), for
/// an admin's preview of new requirements. `max_auto` is the highest level activity earns.
pub async fn trust_population(pool: &PgPool, recent_days: i32, max_auto: i16) -> Result<Vec<TrustCandidate>, Error> {
    let cols = metric_columns("$1");
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.id, u.trust_level, u.trust_nudged_level AS nudged_level, u.primary_group_id IN (3, 4) AS staff, {cols}
         FROM users u WHERE u.trust_level <= $2 AND {TRUST_ELIGIBLE}"
    )))
    .bind(recent_days)
    .bind(max_auto)
    .fetch_all(pool)
    .await
}

/// Counts of members at each level.
pub async fn trust_level_counts(pool: &PgPool) -> Result<Vec<(i16, i64)>, Error> {
    sqlx::query_as(
        "SELECT u.trust_level, count(*) FROM users u
         WHERE u.approved AND NOT u.remote AND u.email_lower NOT LIKE '%@plugins.invalid'
         GROUP BY u.trust_level ORDER BY u.trust_level",
    )
    .fetch_all(pool)
    .await
}

/// Remembers who was told they are close to which level, so each is told once.
pub async fn mark_trust_nudged(pool: &PgPool, marks: &[(i64, i16)]) -> Result<(), Error> {
    if marks.is_empty() {
        return Ok(());
    }
    let ids: Vec<i64> = marks.iter().map(|m| m.0).collect();
    let levels: Vec<i16> = marks.iter().map(|m| m.1).collect();
    sqlx::query(
        "UPDATE users SET trust_nudged_level = n.level FROM unnest($1::bigint[], $2::smallint[]) AS n(id, level)
         WHERE users.id = n.id AND users.trust_nudged_level < n.level",
    )
    .bind(ids)
    .bind(levels)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Raises each member to the level given, never lowering one or touching a level set by hand.
/// Returns who actually moved up, with the new level.
pub async fn raise_trust(pool: &PgPool, raises: &[(i64, i16)]) -> Result<Vec<(i64, i16)>, Error> {
    if raises.is_empty() {
        return Ok(vec![]);
    }
    let ids: Vec<i64> = raises.iter().map(|r| r.0).collect();
    let levels: Vec<i16> = raises.iter().map(|r| r.1).collect();
    sqlx::query_as(
        "UPDATE users SET trust_level = n.level FROM unnest($1::bigint[], $2::smallint[]) AS n(id, level)
         WHERE users.id = n.id AND users.trust_level < n.level AND NOT users.trust_level_locked
         RETURNING users.id, users.trust_level",
    )
    .bind(ids)
    .bind(levels)
    .fetch_all(pool)
    .await
}

/// One member's level, whether staff fixed it, whether their account is paused, and the
/// measures the requirements are checked against.
#[derive(Debug, sqlx::FromRow)]
pub struct TrustStanding {
    pub trust_level: i16,
    pub locked: bool,
    pub paused: bool,
    pub posts: i64,
    pub days: i64,
    pub topics_read: i64,
    pub reputation: i64,
    pub recent_posts: i64,
    pub likes_received: i64,
    pub likes_given: i64,
    pub days_visited: i64,
}

impl TrustStanding {
    pub fn metrics(&self) -> bf_core::trust::Metrics {
        bf_core::trust::Metrics {
            posts: self.posts,
            days: self.days,
            topics_read: self.topics_read,
            reputation: self.reputation,
            recent_posts: self.recent_posts,
            likes_received: self.likes_received,
            likes_given: self.likes_given,
            days_visited: self.days_visited,
        }
    }
}

pub async fn trust_standing(pool: &PgPool, id: UserId, recent_days: i32) -> Result<Option<TrustStanding>, Error> {
    let cols = metric_columns("$2");
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT u.trust_level, u.trust_level_locked AS locked,
                coalesce(u.suspended_until > now() OR u.silenced_until > now(), false) AS paused, {cols}
         FROM users u WHERE u.id = $1"
    )))
    .bind(id)
    .bind(recent_days)
    .fetch_optional(pool)
    .await
}

/// An earned level can be kept only with recent activity: members without it go down one
/// level. `needed[n]` is the posts level `n` asks for in the last `days` days (0 is no
/// requirement); only levels up to `max_auto` count. Levels staff set by hand (locked) and staff
/// accounts stay. Returns who went down, with the level they have now.
pub async fn demote_inactive(
    pool: &PgPool,
    needed: Vec<i64>,
    days: i32,
    max_auto: i16,
) -> Result<Vec<(i64, i16)>, Error> {
    if needed.iter().all(|n| *n <= 0) {
        return Ok(vec![]);
    }
    sqlx::query_as(
        "UPDATE users SET trust_level = trust_level - 1 WHERE id IN (
           SELECT u.id FROM users u
           WHERE u.trust_level BETWEEN 1 AND $3 AND NOT u.trust_level_locked AND u.primary_group_id NOT IN (3, 4)
             AND ($1::bigint[])[u.trust_level + 1] > 0
             AND (SELECT count(*) FROM posts p WHERE p.author_id = u.id AND p.state = 'visible'
                    AND p.action IS NULL AND p.created_at > now() - make_interval(days => $2))
                 < ($1::bigint[])[u.trust_level + 1])
         RETURNING id, trust_level",
    )
    .bind(needed)
    .bind(days)
    .bind(max_auto)
    .fetch_all(pool)
    .await
}

/// After a level was removed: members above `top` drop to it, and forums that asked for more
/// ask for `top`. Returns how many members moved.
pub async fn cap_trust_level(pool: &PgPool, top: i16) -> Result<u64, Error> {
    let moved = sqlx::query(
        "UPDATE users SET trust_level = $1, trust_nudged_level = least(trust_nudged_level, $1) WHERE trust_level > $1",
    )
    .bind(top)
    .execute(pool)
    .await?
    .rows_affected();
    sqlx::query("UPDATE nodes SET min_trust_level = $1 WHERE min_trust_level > $1").bind(top).execute(pool).await?;
    Ok(moved)
}

/// A user by id, with the same columns as a session lookup.
pub async fn user_by_id(pool: &PgPool, id: UserId) -> Result<Option<User>, Error> {
    // Only the constant USER_COLS is interpolated; every value is bound.
    sqlx::query_as(concat!("SELECT ", user_cols!(), " FROM users u WHERE u.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

/// Starts an email change: the new address is stored in the token's payload until confirmed.
pub async fn change_email(pool: &PgPool, user_id: UserId, new_email: &str) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let taken: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE email_lower = lower($1) AND id <> $2)")
            .bind(new_email)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
    if taken {
        return Ok(false);
    }
    sqlx::query("UPDATE users SET email = $2, email_lower = lower($2), email_verified_at = now() WHERE id = $1")
        .bind(user_id)
        .bind(new_email)
        .execute(&mut *tx)
        .await?;
    // Links already mailed to the old address must stop working.
    sqlx::query("UPDATE email_tokens SET used_at = now() WHERE user_id = $1 AND used_at IS NULL")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    record(&mut *tx, Entry::new("account.email").by(user_id).on(format!("user:{user_id}"))).await?;
    tx.commit().await?;
    // Reply-by-email keys opened for the old address must not work for whoever holds them now.
    crate::accounts::security::reset_mail_keys(pool, user_id).await?;
    Ok(true)
}

/// Sets the name shown next to the username (the provider's full name, by `auth_overrides_name`).
pub async fn set_display_name(pool: &PgPool, user_id: UserId, name: &str) -> Result<(), Error> {
    sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
        .bind(user_id)
        .bind(name)
        .execute(pool)
        .await
        .map(|_| ())
}

/// A name someone gave up in the last year: old mentions and links still mean them.
pub async fn name_reserved(pool: &PgPool, name: &str) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM username_history WHERE old_lower = lower($1) AND changed_at > now() - interval '1 year')",
    )
    .bind(name)
    .fetch_one(pool)
    .await
}

/// Renames a user, keeping the old name for redirects. Returns false if the name is taken.
pub async fn rename(pool: &PgPool, user_id: UserId, new_name: &str) -> Result<bool, Error> {
    let mut tx = pool.begin().await?;
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE username_lower = lower($1) AND id <> $2)
             OR EXISTS (SELECT 1 FROM username_history WHERE old_lower = lower($1) AND user_id <> $2
                          AND changed_at > now() - interval '1 year')",
    )
    .bind(new_name)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if taken {
        return Ok(false);
    }
    // A look-alike of another member's name is an error the caller maps to a message.
    let skeleton = claim_skeleton(&mut tx, new_name, Some(user_id.get())).await?;
    sqlx::query("INSERT INTO username_history (user_id, old_username, old_lower) SELECT id, username, username_lower FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE users SET username = $2, username_lower = lower($2), username_skeleton = $3 WHERE id = $1")
        .bind(user_id)
        .bind(new_name)
        .bind(&skeleton)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn last_rename(pool: &PgPool, user_id: UserId) -> Result<Option<DateTime<Utc>>, Error> {
    sqlx::query_scalar("SELECT max(changed_at) FROM username_history WHERE user_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
}

/// The current id for a former username (redirects from old profile and mention links).
pub async fn user_by_old_name(pool: &PgPool, name: &str) -> Result<Option<UserId>, Error> {
    sqlx::query_scalar(
        "SELECT user_id FROM username_history WHERE old_lower = lower($1) ORDER BY changed_at DESC LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await
}

/// The address of an account whose email is the same mailbox as this alias key (see
/// `bf_core::auth::email_alias_key`): the same address, or one differing by a `+tag` or Gmail dots.
pub async fn email_alias_owner(pool: &PgPool, local: &str, domain: &str) -> Result<Option<String>, Error> {
    sqlx::query_scalar(
        "SELECT email FROM users
         WHERE split_part(email_lower, '@', 2) IN ($1, CASE WHEN $1 = 'gmail.com' THEN 'googlemail.com' END)
           AND CASE WHEN $1 = 'gmail.com' THEN replace(split_part(split_part(email_lower, '@', 1), '+', 1), '.', '')
                    ELSE split_part(split_part(email_lower, '@', 1), '+', 1) END = $2
         LIMIT 1",
    )
    .bind(domain)
    .bind(local)
    .fetch_optional(pool)
    .await
}

/// Whether another account's name has this confusable skeleton.
pub async fn skeleton_taken(pool: &PgPool, skeleton: &str, except_user: Option<i64>) -> Result<bool, Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM users WHERE username_skeleton = $1 AND NOT remote AND id <> COALESCE($2, 0))",
    )
    .bind(skeleton)
    .bind(except_user)
    .fetch_one(pool)
    .await
}

/// Accounts whose skeleton is missing or stale (new accounts, renames), a batch at a time.
pub async fn names_without_skeleton(pool: &PgPool, after: i64, limit: i64) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as("SELECT id, username FROM users WHERE username_skeleton IS NULL AND id > $1 ORDER BY id LIMIT $2")
        .bind(after)
        .bind(limit)
        .fetch_all(pool)
        .await
}

crate::sql::set_col!(set_skeleton, "users", "username_skeleton", UserId, &str);

/// Has staff approved this account (or did it never need approval)?
pub async fn is_approved(pool: &PgPool, user: i64) -> Result<bool, Error> {
    Ok(sqlx::query_scalar("SELECT approved FROM users WHERE id = $1")
        .bind(user)
        .fetch_optional(pool)
        .await?
        .unwrap_or(false))
}

pub async fn is_suspended(pool: &PgPool, user: i64) -> Result<bool, Error> {
    sqlx::query_scalar("SELECT coalesce(suspended_until > now(), false) FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(pool)
        .await
}
