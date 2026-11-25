//! What the admin suggestions are computed from, and which of them were dismissed.

use sqlx::PgPool;

type Error = sqlx::Error;

pub async fn dismissed(pool: &PgPool) -> Result<Vec<String>, Error> {
    sqlx::query_scalar("SELECT id FROM admin_suggestion_dismissals").fetch_all(pool).await
}

pub async fn dismiss(pool: &PgPool, id: &str) -> Result<(), Error> {
    sqlx::query("INSERT INTO admin_suggestion_dismissals (id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(id)
        .execute(pool)
        .await
        .map(|_| ())
}

pub async fn restore(pool: &PgPool, id: &str) -> Result<(), Error> {
    sqlx::query("DELETE FROM admin_suggestion_dismissals WHERE id = $1").bind(id).execute(pool).await.map(|_| ())
}

/// Counts for the rules: members, visible topics, administrators with no second factor, and the
/// sessions of the last 30 days with how many came from `networks` (CIDR strings).
#[derive(Debug, Default, Clone)]
pub struct Counts {
    pub members: i64,
    pub topics: i64,
    pub admins_without_2fa: i64,
    pub recent_sessions: i64,
    pub sessions_in_networks: i64,
}

pub async fn counts(pool: &PgPool, networks: &[&str]) -> Result<Counts, Error> {
    let nets: Vec<String> = networks.iter().map(|n| (*n).to_owned()).collect();
    let (members, topics, admins_without_2fa, recent_sessions, sessions_in_networks): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT (SELECT count(*) FROM users),
                    (SELECT count(*) FROM items WHERE state = 'visible'),
                    (SELECT count(*) FROM users WHERE primary_group_id = 4 AND totp_secret IS NULL
                       AND NOT EXISTS (SELECT 1 FROM passkeys p WHERE p.user_id = users.id)),
                    (SELECT count(*) FROM sessions WHERE ip IS NOT NULL AND created_at > now() - interval '30 days'),
                    (SELECT count(*) FROM sessions WHERE ip <<= ANY($1::inet[]) AND created_at > now() - interval '30 days')",
        )
        .bind(&nets)
        .fetch_one(pool)
        .await?;
    Ok(Counts { members, topics, admins_without_2fa, recent_sessions, sessions_in_networks })
}
