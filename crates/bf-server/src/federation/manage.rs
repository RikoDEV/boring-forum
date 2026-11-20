//! Admin mirrors, and members' aliases and account moves.

use crate::http::render::DbResult;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use bf_core::AppError;
use bf_db::federation;
use serde::Deserialize;
use serde_json::json;

use super::*;

pub(super) async fn admin_mirrors(
    State(state): State<AppState>,
    crate::admin::api::Admin(..): crate::admin::api::AdminRead,
) -> Result<Response, AppError> {
    let rows = federation::mirrors(state.db()?).await.db()?;
    Ok(Json(
        rows.into_iter()
            .map(|(node, uri, handle, accepted)| json!({ "node_id": node, "actor": uri, "handle": handle, "accepted": accepted }))
            .collect::<Vec<_>>(),
    )
    .into_response())
}

#[derive(Deserialize)]
pub(super) struct MirrorBody {
    node_id: i64,
    #[serde(default)]
    handle: String,
    #[serde(default)]
    actor: String,
}

pub(super) async fn admin_add_mirror(
    State(state): State<AppState>,
    crate::admin::api::Admin(..): crate::admin::api::AdminWrite,
    Json(b): Json<MirrorBody>,
) -> Result<Response, AppError> {
    if !enabled(&state) {
        return Err(AppError::Validation("Turn federation on first.".into()));
    }
    extended::add_mirror(&state, b.node_id, &b.handle).await?;
    Ok(StatusCode::ACCEPTED.into_response())
}

pub(super) async fn admin_remove_mirror(
    State(state): State<AppState>,
    crate::admin::api::Admin(..): crate::admin::api::AdminWrite,
    Json(b): Json<MirrorBody>,
) -> Result<Response, AppError> {
    extended::remove_mirror(&state, b.node_id, &b.actor).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) async fn fediverse_page(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
) -> Result<Response, AppError> {
    use crate::http::i18n::trh;
    use crate::http::ui::{Shell, begin, finish, hidden_csrf};
    use bf_core::html::esc;
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    let (aka, moved) = federation::aliases(state.db()?, user.id.get()).await.db()?;
    let ctx = begin(&state, &v);
    let me = actor_uri(&state, "user", user.id.get());
    let handle = format!("@{}@{}", user.username, host(&state));
    let code = |text: &str| format!("<code>{}</code>", esc(text));
    let html = format!(
        r#"<section class="bf-box block"><h2>{t1}</h2><p><code>{}</code></p><p class="small muted">{t2}</p></section>
         <section class="bf-box block"><h2>{t3}</h2><p class="small muted">{t4}</p><form method="post" action="/settings/fediverse/aliases" class="stack">{}<label class="bf-field"><textarea name="aliases" rows="3" placeholder=" ">{}</textarea><span class="bf-field-label">{t5}</span></label><button class="btn">{t6}</button></form></section>
         <section class="bf-box block"><h2>{t7}</h2>{}<form method="post" action="/settings/fediverse/move" class="stack" data-confirm="{t8}">{}<label class="bf-field"><input name="target" required placeholder=" "><span class="bf-field-label">{t9}</span></label><p class="small muted">{t10}</p><button class="btn danger">{t11}</button></form></section>"#,
        esc(&handle),
        hidden_csrf(&ctx.token),
        esc(&aka.join("\n")),
        moved
            .map(|m| format!(r#"<p class="bf-notice">{}</p>"#, trh("This account has moved to {account}.").replace("{account}", &code(&m))))
            .unwrap_or_default(),
        hidden_csrf(&ctx.token),
        t1 = trh("Your fediverse address"),
        t2 = trh("People on Mastodon and other servers can follow you by this name. Your actor is {actor}.").replace("{actor}", &code(&me)),
        t3 = trh("Other accounts of yours"),
        t4 = trh("To move an account from elsewhere to here, add it below first, then start the move on the other server."),
        t5 = trh("Account addresses (one per line, like https://mastodon.example/users/me)"),
        t6 = trh("Save"),
        t7 = trh("Move to another server"),
        t8 = trh("Tell your followers you have moved?"),
        t9 = trh("The new account (name@server)"),
        t10 = trh("On the new account, first add {actor} as an alias. Your followers elsewhere then follow the new account; your posts here stay.").replace("{actor}", &code(&me)),
        t11 = trh("Move"),
    );
    let shell = Shell {
        title: v.t().msg("Fediverse"),
        crumbs: vec![("/settings".into(), v.t().msg("Settings"))],
        noindex: true,
        ..Shell::default()
    };
    Ok(finish(&state, &v, ctx, shell, html))
}

#[derive(Deserialize)]
pub(super) struct AliasForm {
    csrf: String,
    #[serde(default)]
    aliases: String,
}

pub(super) async fn save_aliases(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    axum::Form(f): axum::Form<AliasForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    let list: Vec<String> = f
        .aliases
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(10)
        .map(|l| {
            if l.starts_with("https://") && url::Url::parse(l).is_ok() {
                Ok(l.to_owned())
            } else {
                Err(AppError::invalid("{l} is not an account address (https://…).", &[("l", &l)]))
            }
        })
        .collect::<Result<_, _>>()?;
    let pool = state.db()?;
    let (_, moved) = federation::aliases(pool, user.id.get()).await.db()?;
    federation::set_aliases(pool, user.id.get(), &list, moved.as_deref()).await.db()?;
    Ok(Redirect::to("/settings/fediverse").into_response())
}

#[derive(Deserialize)]
pub(super) struct MoveForm {
    csrf: String,
    target: String,
}

pub(super) async fn move_account(
    State(state): State<AppState>,
    v: crate::http::viewer::Viewer,
    axum::Form(f): axum::Form<MoveForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    if !enabled(&state) {
        return Err(AppError::NotFound);
    }
    let user = v.require_user()?;
    v.require_sudo()?;
    let target = extended::resolve(&state, &f.target)
        .await
        .map_err(|e| AppError::invalid("Could not find that account: {e}", &[("e", &e)]))?;
    extended::move_away(&state, user.id.get(), &target).await?;
    Ok(Redirect::to("/settings/fediverse").into_response())
}
