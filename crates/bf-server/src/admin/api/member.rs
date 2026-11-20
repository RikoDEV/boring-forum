//! Everything the member details page needs beyond the account fields: what a member has done,
//! the warnings and staff notes about them, the addresses and connections tied to the account,
//! and the actions an administrator takes from there (change name or email, send a password
//! reset, clear profile fields, warn, ban an address, revoke a key, export their data).
//!
//! Every action is logged under the administrator and needs a recent password confirmation, like
//! the other account edits.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::{AppError, auth};
use bf_db::accounts::users;
use bf_db::moderation::bans;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use super::actor;

/// One query that answers with a JSON array of rows.
async fn rows(pool: &PgPool, sql: &str, id: i64) -> Result<Value, AppError> {
    // Only the literal queries below are wrapped.
    let sql = format!("SELECT coalesce(jsonb_agg(t), '[]') FROM ({sql}) t");
    sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(sql)).bind(id).fetch_one(pool).await.db()
}

async fn audit(pool: &PgPool, who: Option<i64>, action: &str, id: i64, details: Value) -> Result<(), AppError> {
    super::audit_log(pool, who, action, &format!("user:{id}"), details).await
}

/// `GET /api/v1/admin/users/{id}/overview`.
pub(super) async fn overview(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let email: Option<(String, i32, bool)> =
        sqlx::query_as("SELECT email_lower, email_bounces, email_bad FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?;
    let Some((email, bounces, bad)) = email else { return Err(AppError::NotFound) };
    let activity: Value = sqlx::query_scalar(
        "SELECT jsonb_build_object(
            'likes_received', coalesce((SELECT sum(like_count) FROM posts WHERE author_id = $1 AND state = 'visible'), 0),
            'likes_given', (SELECT count(*) FROM post_likes WHERE user_id = $1),
            'solved', (SELECT count(*) FROM items i JOIN posts p ON p.item_id = i.id AND p.post_number = i.solved_post_number
                       WHERE p.author_id = $1),
            'last_post_at', (SELECT max(created_at) FROM posts WHERE author_id = $1 AND action IS NULL),
            'reports_filed', (SELECT count(*) FROM reports WHERE reporter_id = $1),
            'reports_against', (SELECT count(*) FROM reports r JOIN posts p ON r.target_type = 'post' AND r.target_id = p.id
                                WHERE p.author_id = $1)
                             + (SELECT count(*) FROM member_reports WHERE user_id = $1))",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .db()?;
    let recent_posts = rows(
        pool,
        "SELECT p.id, p.post_number = 1 AS is_topic, i.title, p.state, p.created_at, left(p.raw, 160) AS excerpt,
                  '/t/' || i.id || '-' || i.slug || '/post/' || p.post_number AS url
           FROM posts p JOIN items i ON i.id = p.item_id
           WHERE p.author_id = $1 AND p.action IS NULL ORDER BY p.created_at DESC LIMIT 10",
        id,
    )
    .await?;
    let reports_against = rows(
        pool,
        "SELECT r.id, r.reason, r.status, r.created_at, i.title,
                  '/t/' || i.id || '-' || i.slug || '/post/' || p.post_number AS url
           FROM reports r JOIN posts p ON r.target_type = 'post' AND r.target_id = p.id
                JOIN items i ON i.id = p.item_id
           WHERE p.author_id = $1 ORDER BY r.created_at DESC LIMIT 5",
        id,
    )
    .await?;
    let badges = rows(
        pool,
        "SELECT b.name, b.icon, ub.earned_at FROM user_badges ub JOIN badges b ON b.id = ub.badge_id
           WHERE ub.user_id = $1 ORDER BY ub.earned_at DESC LIMIT 40",
        id,
    )
    .await?;
    let warnings = rows(
        pool,
        "SELECT w.id, m.username AS moderator, w.points, w.reason, w.created_at, w.expires_at, w.expires_at > now() AS active
           FROM warnings w LEFT JOIN users m ON m.id = w.moderator_id WHERE w.user_id = $1 ORDER BY w.id DESC LIMIT 50",
        id,
    )
    .await?;
    let notes = rows(
        pool,
        "SELECT n.id, a.username AS author, n.body, n.created_at FROM user_notes n
           LEFT JOIN users a ON a.id = n.author_id WHERE n.user_id = $1 ORDER BY n.id DESC LIMIT 100",
        id,
    )
    .await?;
    // Every address the member posted or signed in from, and how many other accounts used it.
    let ips = rows(
        pool,
        "SELECT host(m.ip) AS ip, max(m.at) AS last_used, sum(m.n)::bigint AS uses,
                  (SELECT count(DISTINCT o.uid) FROM (
                       SELECT author_id AS uid FROM posts WHERE ip = m.ip
                       UNION SELECT user_id FROM sessions WHERE ip = m.ip) o WHERE o.uid <> $1) AS others,
                  EXISTS (SELECT 1 FROM bans b WHERE b.kind = 'ip' AND b.value = lower(host(m.ip)) AND (b.expires_at IS NULL OR b.expires_at > now())) AS banned
           FROM (SELECT ip, max(created_at) AS at, count(*) AS n FROM posts
                   WHERE author_id = $1 AND ip IS NOT NULL GROUP BY ip
                 UNION ALL SELECT ip, max(last_used_at), count(*) FROM sessions
                   WHERE user_id = $1 AND ip IS NOT NULL GROUP BY ip) m
           GROUP BY m.ip ORDER BY max(m.at) DESC LIMIT 50",
        id,
    )
    .await?;
    let same_ip = rows(
        pool,
        "SELECT u.id, u.username, host(x.ip) AS ip, max(x.at) AS last_used FROM (
               SELECT author_id AS uid, ip, created_at AS at FROM posts
                 WHERE ip IN (SELECT ip FROM posts WHERE author_id = $1 AND ip IS NOT NULL
                              UNION SELECT ip FROM sessions WHERE user_id = $1 AND ip IS NOT NULL)
               UNION ALL SELECT user_id, ip, last_used_at FROM sessions
                 WHERE ip IN (SELECT ip FROM posts WHERE author_id = $1 AND ip IS NOT NULL
                              UNION SELECT ip FROM sessions WHERE user_id = $1 AND ip IS NOT NULL)) x
             JOIN users u ON u.id = x.uid WHERE u.id <> $1 GROUP BY u.id, u.username, x.ip
             ORDER BY max(x.at) DESC LIMIT 20",
        id,
    )
    .await?;
    let identities = rows(
        pool,
        "SELECT provider, email, created_at FROM user_identities WHERE user_id = $1 ORDER BY created_at DESC",
        id,
    )
    .await?;
    let api_keys = rows(
        pool,
        "SELECT id, name, scope, oauth_client, last_used_at, created_at, expires_at FROM api_keys
           WHERE user_id = $1 ORDER BY created_at DESC LIMIT 50",
        id,
    )
    .await?;
    let passkeys = rows(
        pool,
        "SELECT name, created_at, last_used_at FROM passkeys WHERE user_id = $1 ORDER BY created_at DESC",
        id,
    )
    .await?;
    let sessions = rows(
        pool,
        "SELECT host(ip) AS ip, country, user_agent, created_at, last_used_at FROM sessions
           WHERE user_id = $1 AND expires_at > now() ORDER BY last_used_at DESC LIMIT 10",
        id,
    )
    .await?;
    let mail: Value = sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(t), '[]') FROM (
           SELECT subject, created_at, sent_at, attempts, last_error FROM mail_outbox
           WHERE lower(to_addr) = $1 ORDER BY id DESC LIMIT 15) t",
    )
    .bind(&email)
    .fetch_one(pool)
    .await
    .db()?;
    Ok(Json(json!({
        "activity": activity,
        "recent_posts": recent_posts,
        "reports_against": reports_against,
        "badges": badges,
        "warnings": warnings,
        "notes": notes,
        "ips": ips,
        "same_ip": same_ip,
        "connections": {
            "identities": identities,
            "api_keys": api_keys,
            "passkeys": passkeys,
            "sessions": sessions,
        },
        "mail": { "recent": mail, "bounces": bounces, "bad": bad },
    }))
    .into_response())
}

#[derive(Deserialize)]
pub(super) struct IdentityBody {
    username: Option<String>,
    email: Option<String>,
    #[serde(default, deserialize_with = "super::accounts::double_option")]
    display_name: Option<Option<String>>,
}

/// `PATCH /api/v1/admin/users/{id}/identity`: change the username, email address or display name.
pub(super) async fn set_identity(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<IdentityBody>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    let user = users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    if let Some(name) = b.username.as_deref().map(auth::normalize_username).filter(|n| *n != user.username) {
        let unicode = state.settings().bool("unicode_usernames");
        auth::validate_username_for(&name, unicode).map_err(|e| AppError::Validation(e.into()))?;
        if users::skeleton_taken(pool, &auth::username_skeleton(&name), Some(id)).await.db()? {
            return Err(AppError::Validation("That username looks too much like another member's.".into()));
        }
        match users::rename(pool, id.into(), &name).await {
            Ok(true) => {}
            Ok(false) => return Err(AppError::Validation("That username is taken.".into())),
            Err(e) if users::is_skeleton_taken(&e) => {
                return Err(AppError::Validation("That username looks too much like another member's.".into()));
            }
            Err(e) => return Err(crate::http::render::db_err(e)),
        }
        audit(pool, who, "user.rename", id, json!({ "from": super::mask_name(&user.username), "to": super::mask_name(&name) }))
            .await?;
    }
    if let Some(raw) = b.email.as_deref().map(str::trim).filter(|e| !e.eq_ignore_ascii_case(&user.email)) {
        let email = auth::normalize_email(raw)
            .ok_or_else(|| AppError::Validation("Enter a valid email address.".into()))?;
        let email = email.as_str();
        if !users::change_email(pool, id.into(), email).await.db()? {
            return Err(AppError::Validation("That email address is already used by another member.".into()));
        }
        crate::platform::webhooks::fire(&state, "user.email_changed", json!({ "user_id": id }));
        // The log is append-only and outlives the account: it holds addresses only in masked form.
        audit(pool, who, "user.email", id, json!({ "from": super::mask_email(&user.email), "to": super::mask_email(email) }))
            .await?;
    }
    if let Some(name) = b.display_name {
        let name = name.map(|n| n.trim().chars().take(60).collect::<String>()).filter(|n| !n.is_empty());
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(id)
            .bind(&name)
            .execute(pool)
            .await
            .db()?;
        audit(pool, who, "user.display_name", id, json!({ "to": name.as_deref().map(super::mask_name) })).await?;
    }
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/admin/users/{id}/send-password-reset`: mails the member a reset link.
pub(super) async fn send_reset(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    let user = users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    crate::auth::recovery::send_token_mail(
        &state,
        pool,
        bf_core::mail::Kind::Reset,
        user.id,
        &user.username,
        &user.email,
    )
    .await?;
    audit(pool, who, "user.password_reset_sent", id, json!({})).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct ClearBody {
    what: Vec<String>,
}

/// `POST /api/v1/admin/users/{id}/clear-profile`: empties profile fields (a picture or text a
/// member should not have).
pub(super) async fn clear_profile(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<ClearBody>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    let mut sets: Vec<&'static str> = Vec::new();
    for w in &b.what {
        sets.push(match w.as_str() {
            "avatar" => "avatar_upload_id = NULL",
            "cover" => "cover_upload_id = NULL",
            "about" => "about_md = NULL, about_html = NULL",
            "signature" => "signature_md = NULL, signature_html = NULL",
            "location" => "location = NULL",
            "website" => "website = NULL",
            "title" => "title = NULL",
            _ => return Err(AppError::BadRequest("unknown field".into())),
        });
    }
    if sets.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    // Only the fixed fragments above are interpolated.
    let sql = format!("UPDATE users SET {} WHERE id = $1", sets.join(", "));
    sqlx::query(sqlx::AssertSqlSafe(sql)).bind(id).execute(pool).await.found()?;
    audit(pool, who, "user.clear_profile", id, json!({ "fields": b.what })).await?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct WarnBody {
    points: i32,
    reason: String,
}

/// `POST /api/v1/admin/users/{id}/warn`.
pub(super) async fn warn(
    State(state): State<AppState>,
    Admin(_, v): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<WarnBody>,
) -> Result<StatusCode, AppError> {
    crate::moderation::warnings::issue(&state, &v, id, b.points, &b.reason).await?;
    state.microcache.clear();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct NoteBody {
    body: String,
}

/// `POST /api/v1/admin/users/{id}/notes`: a private note about the member, for staff.
pub(super) async fn add_note(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<NoteBody>,
) -> Result<StatusCode, AppError> {
    let body = b.body.trim();
    if body.is_empty() || body.chars().count() > 2000 {
        return Err(AppError::Validation("A note is 1 to 2000 characters.".into()));
    }
    let pool = state.db()?;
    if users::user_by_id(pool, id.into()).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    bf_db::moderation::topics::add_note(pool, id, actor(who), body).await.db()?;
    Ok(StatusCode::CREATED)
}

/// `DELETE /api/v1/admin/users/{id}/notes/{note}`.
pub(super) async fn delete_note(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path((id, note)): Path<(i64, i64)>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    // The note's words go to the log with it: a deleted note is otherwise gone without a trace.
    let body: Option<String> =
        sqlx::query_scalar("DELETE FROM user_notes WHERE id = $1 AND user_id = $2 RETURNING body")
            .bind(note)
            .bind(id)
            .fetch_optional(pool)
            .await
            .db()?;
    if let Some(body) = body {
        audit(pool, who, "user.note_delete", id, json!({ "note": note, "body": body })).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub(super) struct BanBody {
    /// `ip` (one of the member's own addresses, in `value`) or `email` (their current address).
    kind: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    reason: String,
}

/// `POST /api/v1/admin/users/{id}/ban`: bans one of the member's addresses, or their email.
/// Never an internal address, the administrator's own, or one on the allow list.
pub(super) async fn ban(
    State(state): State<AppState>,
    Admin(who, v): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<BanBody>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    let user = users::user_by_id(pool, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    let reason = Some(b.reason.trim()).filter(|r| !r.is_empty()).map(|r| r.chars().take(200).collect::<String>());
    match b.kind.as_str() {
        "email" => {
            let email = user.email.trim().to_lowercase();
            bans::add(pool, "email", &email, reason.as_deref(), who).await.db()?;
        }
        "ip" => {
            let ip: std::net::IpAddr =
                b.value.trim().parse().map_err(|_| AppError::Validation("That is not an address.".into()))?;
            let known: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM posts WHERE author_id = $1 AND ip = $2::inet)
                     OR EXISTS (SELECT 1 FROM sessions WHERE user_id = $1 AND ip = $2::inet)",
            )
            .bind(id)
            .bind(ip.to_string())
            .fetch_one(pool)
            .await
            .db()?;
            if !known {
                return Err(AppError::Validation("That address was not used by this member.".into()));
            }
            if crate::platform::outbound::forbidden_ip(ip)
                || Some(ip) == v.ip
                || bans::ip_allowed(pool, ip).await.unwrap_or(false)
            {
                return Err(AppError::Validation("That address cannot be banned.".into()));
            }
            bans::add(pool, "ip", &ip.to_string(), reason.as_deref(), who).await.db()?;
        }
        _ => return Err(AppError::BadRequest("unknown kind".into())),
    }
    // The log outlives the account: an address or email goes in masked.
    let shown = if b.kind == "email" { super::mask_email(&user.email) } else { super::mask_ip(b.value.trim()) };
    audit(pool, who, "user.ban", id, json!({ "kind": b.kind, "value": shown })).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/admin/users/{id}/api-keys/{key}`: revokes a key or an app's token.
pub(super) async fn revoke_key(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path((id, key)): Path<(i64, i64)>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    sqlx::query("DELETE FROM api_keys WHERE id = $1 AND user_id = $2")
        .bind(key)
        .bind(id)
        .execute(pool)
        .await
        .found()?;
    audit(pool, who, "user.revoke_key", id, json!({ "key": key })).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/v1/admin/users/{id}/identities/{provider}`: unlinks a social sign-in.
pub(super) async fn unlink_identity(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path((id, provider)): Path<(i64, String)>,
) -> Result<StatusCode, AppError> {
    let pool = state.db()?;
    sqlx::query("DELETE FROM user_identities WHERE user_id = $1 AND provider = $2")
        .bind(id)
        .bind(&provider)
        .execute(pool)
        .await
        .found()?;
    audit(pool, who, "user.unlink_identity", id, json!({ "provider": provider })).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/admin/users/{id}/export`: everything the site holds about the member, as the
/// member's own export gives it. The password confirmation guards it; the download is logged.
pub(super) async fn export(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let data = bf_db::accounts::lifecycle::export(pool, id).await.db()?;
    audit(pool, who, "user.export", id, json!({})).await?;
    Ok(([(axum::http::header::CACHE_CONTROL, "no-store")], Json(data)).into_response())
}
