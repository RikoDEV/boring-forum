//! Member preferences: profile fields, interface, quiet hours, signature, do-not-disturb.

use bf_core::ids::UserId;
use sqlx::{FromRow, PgPool};

type Error = sqlx::Error;

#[derive(Debug, Clone, FromRow)]
pub struct ProfileFields {
    pub display_name: Option<String>,
    pub hide_last_seen: bool,
    pub profile_visibility: String,
    pub about_md: Option<String>,
    pub location: Option<String>,
    pub website: Option<String>,
    pub timezone: Option<String>,
}

pub async fn profile_fields(pool: &PgPool, user_id: UserId) -> Result<ProfileFields, Error> {
    sqlx::query_as(
        "SELECT display_name, hide_last_seen, profile_visibility, about_md, location, website, timezone FROM users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
}

pub async fn save_profile(
    pool: &PgPool,
    user_id: UserId,
    f: &ProfileFields,
    about_html: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users SET about_md = $2, about_html = $3, location = $4, website = $5, timezone = $6,
           display_name = $7, hide_last_seen = $8 WHERE id = $1",
    )
    .bind(user_id)
    .bind(&f.about_md)
    .bind(about_html)
    .bind(&f.location)
    .bind(&f.website)
    .bind(&f.timezone)
    .bind(&f.display_name)
    .bind(f.hide_last_seen)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Notification channels bitset: 1 = in app, 2 = email.
pub const APP: i16 = bf_core::notifications::APP;

pub const EMAIL: i16 = bf_core::notifications::EMAIL;

pub const PUSH: i16 = bf_core::notifications::PUSH;

#[allow(clippy::too_many_arguments)]
pub async fn save_interface(
    pool: &PgPool,
    user_id: UserId,
    locale: &str,
    text_size: i16,
    density: &str,
    date_format: Option<&str>,
    skin_tone: Option<i16>,
    profile_visibility: Option<&str>,
) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users SET locale = $2, text_size = $3, density = $4, date_format = coalesce($5, date_format),
           skin_tone = coalesce($6, skin_tone), profile_visibility = coalesce($7, profile_visibility) WHERE id = $1",
    )
    .bind(user_id)
    .bind(locale)
    .bind(text_size)
    .bind(density)
    .bind(date_format)
    .bind(skin_tone)
    .bind(profile_visibility)
    .execute(pool)
    .await
    .map(|_| ())
}

pub async fn set_quiet_hours(pool: &PgPool, user: i64, start: Option<i16>, end: Option<i16>) -> Result<(), Error> {
    sqlx::query("UPDATE users SET quiet_start = $2, quiet_end = $3 WHERE id = $1")
        .bind(user)
        .bind(start)
        .bind(end)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn quiet_hours(pool: &PgPool, user: i64) -> Result<(Option<i16>, Option<i16>), Error> {
    sqlx::query_as("SELECT quiet_start, quiet_end FROM users WHERE id = $1").bind(user).fetch_one(pool).await
}

pub async fn set_signature(pool: &PgPool, user: i64, raw: &str, html: &str) -> Result<(), Error> {
    sqlx::query("UPDATE users SET signature_md = NULLIF($2, ''), signature_html = NULLIF($3, '') WHERE id = $1")
        .bind(user)
        .bind(raw)
        .bind(html)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn set_dnd(pool: &PgPool, user: i64, hours: i32) -> Result<(), Error> {
    sqlx::query(
        "UPDATE users SET dnd_until = CASE WHEN $2 > 0 THEN now() + make_interval(hours => $2) END WHERE id = $1",
    )
    .bind(user)
    .bind(hours)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Who may start a private message with this member: `everyone`, `following` or `nobody`.
pub async fn allow_pm_from(pool: &PgPool, user_id: UserId) -> Result<String, Error> {
    sqlx::query_scalar("SELECT allow_pm_from FROM users WHERE id = $1").bind(user_id).fetch_one(pool).await
}

crate::sql::set_col!(set_allow_pm_from, "users", "allow_pm_from", UserId, &str);
