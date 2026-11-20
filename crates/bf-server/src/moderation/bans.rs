//! IP, email and network bans.

use crate::admin::api::admin;
use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/bans", get(list_bans).post(add_ban))
        .route("/api/v1/admin/bans/{id}", axum::routing::delete(remove_ban))
}

async fn list_bans(State(state): State<AppState>, v: Viewer, headers: HeaderMap) -> Result<Response, AppError> {
    admin(&state, &v, &headers, false)?;
    Ok(Json(bf_db::moderation::bans::list(state.db()?).await.db()?).into_response())
}

#[derive(Deserialize)]
struct NewBan {
    kind: String,
    value: String,
    reason: Option<String>,
    /// The ban lapses after this many days; leave out for a ban until lifted.
    #[serde(default)]
    days: Option<i64>,
}

/// `kind`: "ip" (an address or CIDR network) or "email" (an address, or `@domain`).
async fn add_ban(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Json(b): Json<NewBan>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let value = b.value.trim();
    let ok = match b.kind.as_str() {
        "ip" | "ip_allow" => {
            value.parse::<sqlx::types::ipnetwork::IpNetwork>().is_ok() || value.parse::<std::net::IpAddr>().is_ok()
        }
        "asn" => value.trim_start_matches("AS").chars().all(|c| c.is_ascii_digit()) && !value.is_empty(),
        "email" => value.contains('@') && !value.contains(char::is_whitespace) && value.len() <= 254,
        _ => false,
    };
    if !ok {
        return Err(AppError::Validation(
            "Use kind ip or ip_allow with an address or network, or kind email with an address or @domain.".into(),
        ));
    }
    // Never let an admin ban the address they are using.
    if b.kind == "ip"
        && let Some(me) = v.ip
    {
        let net: sqlx::types::ipnetwork::IpNetwork =
            value.parse().map_err(|_| AppError::Validation("bad network".into()))?;
        if net.contains(me) {
            return Err(AppError::Validation("That would ban your own address.".into()));
        }
    }
    let expires = b.days.filter(|d| *d > 0).map(|d| chrono::Utc::now() + chrono::Duration::days(d.min(3650)));
    let id = bf_db::moderation::bans::add_until(state.db()?, &b.kind, value, b.reason.as_deref(), who, expires)
        .await
        .db()?;
    crate::platform::webhooks::moderated(
        &state,
        "ban",
        who.unwrap_or(0),
        "ban",
        id,
        json!({ "kind": b.kind, "value": value }),
    )
    .await;
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))).into_response())
}

async fn remove_ban(
    State(state): State<AppState>,
    v: Viewer,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let who = admin(&state, &v, &headers, true)?;
    let before = bf_db::moderation::bans::list(state.db()?).await.db()?.into_iter().find(|b| b.id == id);
    if !bf_db::moderation::bans::remove(state.db()?, id).await.db()? {
        return Err(AppError::NotFound);
    }
    crate::moderation::audit(
        &state,
        who,
        "ban.remove",
        format!("ban:{id}"),
        json!({ "before": before.map(|b| json!({ "kind": b.kind, "value": crate::admin::api::mask_ban_value(&b.kind, &b.value), "reason": b.reason })), "after": null }),
    )
    .await;
    Ok(StatusCode::NO_CONTENT.into_response())
}
