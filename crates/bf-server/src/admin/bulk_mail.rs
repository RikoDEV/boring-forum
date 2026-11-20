//! Mass messages: an administrator writes to many members at once, as a notice with an email
//! (members can switch it off under Settings → Notifications, and each email carries a one-click
//! unsubscribe), as a private message to each, or both. The members are chosen by filters, and
//! the preview says how many it reaches before anything is sent. Delivery runs in the
//! background, a little at a time, and each message is recorded with how far it got.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bf_core::AppError;
use bf_db::platform::mail::{self, Audience};
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/mass-message", post(send).get(history))
        .route("/api/v1/admin/mass-message/preview", post(preview))
}

#[derive(Deserialize)]
struct Preview {
    #[serde(default)]
    audience: Audience,
}

/// `POST /api/v1/admin/mass-message/preview`: how many members the filters reach.
async fn preview(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Json(p): Json<Preview>,
) -> Result<Response, AppError> {
    let (recipients, emails) = mail::audience_counts(state.db()?, &p.audience).await.db()?;
    Ok(Json(json!({ "recipients": recipients, "emails": emails })).into_response())
}

/// `GET /api/v1/admin/mass-message`: the latest messages and how they went.
async fn history(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(mail::mass_messages(state.db()?, 30).await.db()?).into_response())
}

#[derive(Deserialize)]
struct Send {
    subject: String,
    body: String,
    #[serde(default)]
    audience: Audience,
    /// A notice, and an email to members who have not switched it off.
    #[serde(default = "yes")]
    notice: bool,
    /// A private message from the sender to each member.
    #[serde(default)]
    conversation: bool,
}

fn yes() -> bool {
    true
}

/// The most members one message can reach: a bigger audience is split by its filters.
const MAX_RECIPIENTS: usize = 50_000;

/// `POST /api/v1/admin/mass-message`.
async fn send(
    State(state): State<AppState>,
    Admin(who, v): AdminWrite,
    Json(b): Json<Send>,
) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let subject = b.subject.trim().to_owned();
    let body = b.body.trim().to_owned();
    if subject.is_empty() || subject.chars().count() > 150 || subject.chars().any(char::is_control) || body.is_empty() {
        return Err(AppError::Validation(
            "A mass message needs a subject of up to 150 characters and some text.".into(),
        ));
    }
    if body.chars().count() > 20_000 {
        return Err(AppError::Validation("The text is too long.".into()));
    }
    if !b.notice && !b.conversation {
        return Err(AppError::Validation(
            "Choose how to send it: a notice with email, a private message, or both.".into(),
        ));
    }
    let pool = state.db()?;
    let ids = mail::audience_ids(pool, &b.audience).await.db()?;
    if ids.is_empty() {
        return Err(AppError::Validation("No member matches those filters.".into()));
    }
    if ids.len() > MAX_RECIPIENTS {
        return Err(AppError::invalid(
            "That reaches {n} members; one message can reach at most {max}. Narrow the filters.",
            &[("n", &ids.len()), ("max", &MAX_RECIPIENTS)],
        ));
    }
    let job =
        mail::start_mass_message(pool, who, &subject, &body, &b.audience, (b.notice, b.conversation), ids.len() as i32)
            .await
            .db()?;
    let details = json!({ "recipients": ids.len(), "job": job, "notice": b.notice, "conversation": b.conversation });
    crate::admin::api::audit_log(pool, who, "mail.bulk", &subject, details).await?;
    let total = ids.len();
    let (state2, sender) = (state.clone(), (user.id.get(), user.username.clone()));
    let (notice, conversation) = (b.notice, b.conversation);
    tokio::spawn(async move { deliver(state2, job, sender, subject, body, ids, notice, conversation, 0).await });
    Ok((StatusCode::ACCEPTED, Json(json!({ "id": job, "recipients": total }))).into_response())
}

/// Picks up deliveries that stopped when a server did, where they were. Called from the
/// maintenance tick; a delivery that is alive keeps its heartbeat fresh and is left alone.
pub(crate) async fn resume_stalled(state: &AppState) {
    let Some(pool) = &state.pool else { return };
    let jobs = match mail::claim_stalled_mass_messages(pool).await {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(error = %e, "stalled mass messages lookup failed");
            return;
        }
    };
    for (job, sender, subject, body, audience, notice, conversation, delivered) in jobs {
        let Some(sender) = sender else { continue };
        let name = match bf_db::accounts::users::user_by_id(pool, sender.into()).await {
            Ok(Some(u)) => u.username,
            _ => continue,
        };
        let audience: Audience = serde_json::from_value(audience).unwrap_or_default();
        let Ok(ids) = mail::audience_ids(pool, &audience).await else { continue };
        let rest: Vec<i64> = ids.into_iter().skip(usize::try_from(delivered).unwrap_or(0)).collect();
        tracing::info!(job, left = rest.len(), "resuming a mass message");
        let state = state.clone();
        tokio::spawn(async move {
            deliver(state, job, (sender, name), subject, body, rest, notice, conversation, delivered).await;
        });
    }
}

/// Sends the message in batches, keeping the record's count up to date.
#[allow(clippy::too_many_arguments)]
async fn deliver(
    state: AppState,
    job: i64,
    sender: (i64, String),
    subject: String,
    body: String,
    ids: Vec<i64>,
    notice: bool,
    conversation: bool,
    already: i32,
) {
    let Some(pool) = &state.pool else { return };
    let mut done = already;
    let mut failed = false;
    for chunk in ids.chunks(200) {
        if notice {
            let sent = bf_db::community::notifications::deliver(
                pool,
                &state.config.base_url,
                sender.0,
                chunk,
                "bulk",
                &json!({ "message": subject, "url": "/notifications" }),
                &subject,
                &body,
                0,
            )
            .await;
            if let Err(e) = sent {
                tracing::error!(error = %e, job, "mass message delivery failed");
                failed = true;
                break;
            }
        }
        if conversation {
            for id in chunk.iter().filter(|id| **id != sender.0) {
                if let Err(e) =
                    crate::community::messages::deliver_new(&state, (sender.0, &sender.1), &subject, &body, &[*id])
                        .await
                {
                    tracing::warn!(error = %e, job, member = id, "mass message conversation failed");
                }
            }
        }
        done += chunk.len() as i32;
        let _ = mail::mass_message_progress(pool, job, done, "running").await;
        // A pause between batches keeps the mail queue and the database responsive.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let _ = mail::mass_message_progress(pool, job, done, if failed { "failed" } else { "done" }).await;
    state.microcache.clear();
}
