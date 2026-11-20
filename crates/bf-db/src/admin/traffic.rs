//! Site traffic: page views as the middleware records them, and the dashboard's report on them.

use chrono::{DateTime, Utc};
use serde::Serialize;
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

/// One page view to store.
#[derive(Debug, Clone)]
pub struct View {
    pub at: DateTime<Utc>,
    pub path: String,
    pub referrer: Option<String>,
    pub country: Option<String>,
    pub kind: &'static str,
    pub visitor: i64,
}

pub async fn insert(pool: &PgPool, views: &[View]) -> Result<(), Error> {
    if views.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO page_views (at, path, referrer, country, kind, visitor)
         SELECT * FROM unnest($1::timestamptz[], $2::text[], $3::text[], $4::text[], $5::text[], $6::bigint[])",
    )
    .bind(views.iter().map(|v| v.at).collect::<Vec<_>>())
    .bind(views.iter().map(|v| v.path.clone()).collect::<Vec<_>>())
    .bind(views.iter().map(|v| v.referrer.clone()).collect::<Vec<_>>())
    .bind(views.iter().map(|v| v.country.clone()).collect::<Vec<_>>())
    .bind(views.iter().map(|v| v.kind.to_owned()).collect::<Vec<_>>())
    .bind(views.iter().map(|v| v.visitor).collect::<Vec<_>>())
    .execute(pool)
    .await
    .map(|_| ())
}

#[derive(Debug, Serialize, FromRow)]
pub struct Day {
    pub day: chrono::NaiveDate,
    pub member: i64,
    pub anonymous: i64,
    pub crawler: i64,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Share {
    pub name: String,
    pub views: i64,
}

#[derive(Debug, Serialize)]
pub struct Traffic {
    pub days: i32,
    pub daily: Vec<Day>,
    /// Views by people (not crawlers) in the period, and in the one before it.
    pub views: i64,
    pub views_before: i64,
    /// Visits by people (a visitor's views on one day): what the lists below are shares of.
    pub visits: i64,
    /// Percent of people's views by signed-in members.
    pub member_share: Option<i32>,
    /// Percent of people's visits that arrived with no referring site.
    pub direct_share: Option<i32>,
    /// Percent of visits that looked at one page only.
    pub bounce_rate: Option<i32>,
    /// Average seconds from a visit's first view to its last.
    pub avg_session_seconds: Option<i32>,
    pub referrers: Vec<Share>,
    pub entries: Vec<Share>,
    pub countries: Vec<Share>,
}

#[derive(FromRow)]
struct Totals {
    views: i64,
    views_before: i64,
    member_views: i64,
    visits: i64,
    direct: i64,
    bounces: i64,
    seconds: Option<f64>,
}

fn pct(part: i64, whole: i64) -> Option<i32> {
    (whole > 0).then(|| ((part as f64 / whole as f64) * 100.0).round() as i32)
}

/// The report for the last `days` days (7 to 90), people and crawlers told apart.
pub async fn report(pool: &PgPool, days: i32) -> Result<Traffic, Error> {
    let days = days.clamp(7, 90);
    let daily = sqlx::query_as(
        "SELECT d::date AS day,
                count(*) FILTER (WHERE v.kind = 'member') AS member,
                count(*) FILTER (WHERE v.kind = 'anonymous') AS anonymous,
                count(*) FILTER (WHERE v.kind = 'crawler') AS crawler
         FROM generate_series(date_trunc('day', now()) - make_interval(days => $1 - 1), date_trunc('day', now()), interval '1 day') d
         LEFT JOIN page_views v ON v.at >= d AND v.at < d + interval '1 day'
         GROUP BY d ORDER BY d",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    // A visit is one visitor's views on one day.
    let t: Totals = sqlx::query_as(
        "WITH cur AS (
           SELECT * FROM page_views WHERE kind <> 'crawler' AND at >= date_trunc('day', now()) - make_interval(days => $1 - 1)
         ), visits AS (
           SELECT visitor, at::date AS day, count(*) AS n,
                  extract(epoch FROM max(at) - min(at))::float8 AS secs,
                  (array_agg(referrer ORDER BY at))[1] IS NULL AS direct
           FROM cur GROUP BY visitor, at::date
         )
         SELECT (SELECT count(*) FROM cur) AS views,
                (SELECT count(*) FROM page_views WHERE kind <> 'crawler'
                   AND at >= date_trunc('day', now()) - make_interval(days => 2 * $1 - 1)
                   AND at < date_trunc('day', now()) - make_interval(days => $1 - 1)) AS views_before,
                (SELECT count(*) FROM cur WHERE kind = 'member') AS member_views,
                (SELECT count(*) FROM visits) AS visits,
                (SELECT count(*) FROM visits WHERE direct) AS direct,
                (SELECT count(*) FROM visits WHERE n = 1) AS bounces,
                (SELECT avg(secs)::float8 FROM visits) AS seconds",
    )
    .bind(days)
    .fetch_one(pool)
    .await?;
    let referrers = sqlx::query_as(
        "WITH visits AS (
           SELECT (array_agg(referrer ORDER BY at))[1] AS name FROM page_views
           WHERE kind <> 'crawler' AND at >= date_trunc('day', now()) - make_interval(days => $1 - 1)
           GROUP BY visitor, at::date)
         SELECT name, count(*) AS views FROM visits WHERE name IS NOT NULL GROUP BY name ORDER BY views DESC, name LIMIT 5",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let entries = sqlx::query_as(
        "WITH visits AS (
           SELECT (array_agg(path ORDER BY at))[1] AS name FROM page_views
           WHERE kind <> 'crawler' AND at >= date_trunc('day', now()) - make_interval(days => $1 - 1)
           GROUP BY visitor, at::date)
         SELECT name, count(*) AS views FROM visits GROUP BY name ORDER BY views DESC, name LIMIT 5",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    let countries = sqlx::query_as(
        "SELECT upper(country) AS name, count(*) AS views FROM page_views
         WHERE kind <> 'crawler' AND country IS NOT NULL AND at >= date_trunc('day', now()) - make_interval(days => $1 - 1)
         GROUP BY upper(country) ORDER BY views DESC, name LIMIT 5",
    )
    .bind(days)
    .fetch_all(pool)
    .await?;
    Ok(Traffic {
        days,
        daily,
        views: t.views,
        views_before: t.views_before,
        visits: t.visits,
        member_share: pct(t.member_views, t.views),
        direct_share: pct(t.direct, t.visits),
        bounce_rate: pct(t.bounces, t.visits),
        avg_session_seconds: t.seconds.map(|s| s.round() as i32),
        referrers,
        entries,
        countries,
    })
}
