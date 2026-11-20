//! Daily report series for the admin dashboard.

use sqlx::PgPool;

type Error = sqlx::Error;

/// Daily counts for the last 30 days, by report name.
pub async fn report(pool: &PgPool, name: &str) -> Result<Option<Vec<(String, i64)>>, Error> {
    let source = match name {
        "signups" => "SELECT joined_at AS t FROM users",
        "posts" => "SELECT created_at AS t FROM posts WHERE action IS NULL",
        "topics" => "SELECT created_at AS t FROM items",
        "flags" => "SELECT created_at AS t FROM reports",
        "searches" => "SELECT created_at AS t FROM search_logs",
        "emails" => "SELECT sent_at AS t FROM mail_outbox WHERE sent_at IS NOT NULL",
        _ => return Ok(None),
    };
    let rows = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT to_char(t, 'YYYY-MM-DD'), count(*) FROM ({source}) s WHERE t > now() - interval '30 days' GROUP BY 1 ORDER BY 1"
    )))
    .fetch_all(pool)
    .await?;
    Ok(Some(rows))
}
