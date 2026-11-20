//! Data Explorer: read-only SQL reports for administrators.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::{DbResult, db_err};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::admin::explorer::{self, SavedQuery};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/explorer", get(overview))
        .route("/api/v1/admin/explorer/run", post(run))
        .route("/api/v1/admin/explorer/queries", post(create))
        .route("/api/v1/admin/explorer/queries/{id}", put(update).delete(remove))
}

/// Whether it can run, the views and their columns, and the saved queries.
async fn overview(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    let pool = state.db()?;
    // The Explorer runs on its own connection as the restricted `bf_explorer` login role; it is
    // available only when that connection is configured.
    let available = state.explorer_pool.is_some();
    let schema: Vec<_> = explorer::schema(pool)
        .await
        .db()?
        .into_iter()
        .map(|(name, columns)| json!({ "name": name, "columns": columns }))
        .collect();
    let queries = explorer::saved(pool).await.db()?;
    Ok(Json(json!({ "available": available, "schema": schema, "queries": queries, "max_rows": explorer::MAX_ROWS }))
        .into_response())
}

#[derive(Deserialize)]
struct RunBody {
    sql: String,
}

/// A write: it needs a recent password confirmation, like other admin writes, because it reads
/// a lot of the forum at once. Each run is in the audit log.
async fn run(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(b): Json<RunBody>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    // Untrusted admin SQL runs only on the dedicated `bf_explorer` login connection, never on the
    // application's own (privileged) pool. Fail closed when it is not configured.
    let Some(explorer_pool) = state.explorer_pool.as_ref() else {
        return Err(AppError::Validation(
            "The Data Explorer is not configured. Set BF_EXPLORER_DATABASE_URL to a connection that logs in as the restricted bf_explorer role (see deploy/postgres/roles.sql).".into(),
        ));
    };
    let sql = b.sql.trim();
    if sql.is_empty() || sql.len() > 20_000 {
        return Err(AppError::Validation("Write a SELECT of up to 20,000 characters.".into()));
    }
    let started = std::time::Instant::now();
    let (rows, truncated) = match explorer::run(explorer_pool, sql, 10_000).await {
        Ok(r) => r,
        // The database's own message is what an admin needs to fix the query.
        Err(sqlx::Error::Database(e)) => {
            return Err(AppError::invalid("The query failed: {n}", &[("n", &e.message())]));
        }
        Err(e) => return Err(db_err(e)),
    };
    let ms = started.elapsed().as_millis();
    // The whole query (at most 20,000 characters): a cut one could hide what was read behind padding.
    let details = json!({ "sql": sql, "ms": ms as i64 });
    crate::admin::api::audit_log(pool, who, "explorer.run", "explorer", details).await?;
    // `rows` is a JSON array straight from Postgres: kept as text so columns stay in order.
    let body = format!("{{\"rows\":{rows},\"truncated\":{truncated},\"ms\":{ms}}}");
    let mut res = (StatusCode::OK, body).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

fn check(q: &SavedQuery) -> Result<(), AppError> {
    if q.name.trim().is_empty() || q.name.chars().count() > 100 || q.sql.trim().is_empty() || q.sql.len() > 20_000 {
        return Err(AppError::Validation("A saved query needs a name and a query.".into()));
    }
    Ok(())
}

async fn create(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(q): Json<SavedQuery>,
) -> Result<Response, AppError> {
    check(&q)?;
    let pool = state.db()?;
    let id = explorer::save(pool, None, &q, who).await.db()?;
    let target = format!("explorer_query:{}", id.unwrap_or(0));
    crate::admin::api::audit_log(pool, who, "explorer_query.create", &target, json!({ "name": q.name, "sql": q.sql }))
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn update(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
    Json(q): Json<SavedQuery>,
) -> Result<Response, AppError> {
    check(&q)?;
    let pool = state.db()?;
    if explorer::save(pool, Some(id), &q, who).await.db()?.is_none() {
        return Err(AppError::NotFound);
    }
    let target = format!("explorer_query:{id}");
    crate::admin::api::audit_log(pool, who, "explorer_query.update", &target, json!({ "name": q.name, "sql": q.sql }))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn remove(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    explorer::delete(pool, id).await.found()?;
    crate::admin::api::audit_log(pool, who, "explorer_query.delete", &format!("explorer_query:{id}"), json!({})).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
