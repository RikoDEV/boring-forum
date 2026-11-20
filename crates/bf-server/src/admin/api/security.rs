//! Site-wide security switches.

use crate::admin::api::{Admin, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::AppError;
use serde::Deserialize;
use serde_json::json;

use super::actor;

#[derive(Deserialize)]
pub(super) struct Require2fa {
    pub(super) on: bool,
}

pub(super) async fn require_2fa(
    State(state): State<AppState>,
    Admin(who, v): AdminWrite,
    Path(id): Path<i64>,
    Json(b): Json<Require2fa>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    // Do not let an admin lock themselves out of the panel. The same test as `needs_2fa`: an
    // authenticator, since a passkey on the account does not make a password sign-in two-factor.
    if let (true, Some(me)) = (b.on, &v.user) {
        let has_2fa: bool = sqlx::query_scalar("SELECT totp_secret IS NOT NULL FROM users WHERE id = $1")
            .bind(me.id)
            .fetch_one(pool)
            .await
            .db()?;
        if me.groups.contains(&id.into()) && !has_2fa {
            return Err(AppError::Validation(
                "Set up two-factor for yourself before requiring it for your own group.".into(),
            ));
        }
    }
    sqlx::query("UPDATE groups SET require_2fa = $2 WHERE id = $1").bind(id).bind(b.on).execute(pool).await.found()?;
    super::audit_log(pool, Some(actor(who)), "group.require_2fa", &format!("group:{id}"), json!({ "on": b.on }))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
