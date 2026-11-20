//! What a moderator can do to a member from the moderator panel: silence, suspend, lift, clean up
//! after a spammer, and ban an address. Warnings have their own route (`warnings.rs`).
//!
//! The rules are the admin tools' with limits for moderators: never on an administrator, on
//! oneself, or (for anyone but an administrator) on another moderator; restrictions last at most
//! a year; clean-up is for new accounts; bans cover single addresses or small networks. Every
//! action is logged with its reason, tells the member, and (for restrictions) records the EU DSA
//! statement of reasons, as the admin panel's restrictions do.

use crate::http::render::DbResult;
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Form, Router};
use bf_core::AppError;
use bf_db::accounts::users::{self, User};
use bf_db::admin::members::{self, UserChange};
use bf_db::moderation::bans;
use serde::Deserialize;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/modcp/act", post(act))
        .route("/modcp/ban", post(add_ban))
        .route("/modcp/ban/{id}/remove", post(remove_ban))
}

/// Where to go back to: a panel address and nothing else.
pub(crate) fn back_to(back: &str, done: &str) -> String {
    let safe = back.starts_with("/modcp") && !back.contains(['\r', '\n', '\\']) && !back.starts_with("//");
    let base = if safe { back } else { "/modcp?tab=members" };
    let base = base.split(['#']).next().unwrap_or(base);
    // A finished action's own notice replaces any earlier one.
    let base = base.split("&done=").next().unwrap_or(base).split("?done=").next().unwrap_or(base);
    let join = if base.contains('?') { '&' } else { '?' };
    format!("{base}{join}done={done}")
}

/// The member an action is about, when this viewer may act on them.
pub(crate) async fn member(state: &AppState, v: &Viewer, id: i64) -> Result<User, AppError> {
    let me = v.require_user()?;
    if !is_staff(v) {
        return Err(AppError::Forbidden);
    }
    let target = users::user_by_id(state.db()?, id.into()).await.db()?.ok_or(AppError::NotFound)?;
    if target.id == me.id || target.is_admin {
        return Err(AppError::Validation("You cannot act on yourself or an administrator.".into()));
    }
    if !v.actor.is_admin && target.groups.contains(&bf_core::ids::GroupId::MODERATORS) {
        return Err(AppError::Validation("Only an administrator can act on a moderator.".into()));
    }
    Ok(target)
}

#[derive(Deserialize)]
struct ActForm {
    csrf: String,
    user: i64,
    action: String,
    #[serde(default)]
    days: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    ban: String,
    #[serde(default)]
    confirm: String,
    #[serde(default)]
    back: String,
}

async fn log(state: &AppState, actor: i64, action: &str, user: i64, details: serde_json::Value) -> Result<(), AppError> {
    bf_db::admin::audit::record(state.db()?, bf_db::admin::audit::Entry::new(action).by(actor).on(format!("user:{user}")).details(details))
        .await
        .db()
}

async fn act(State(state): State<AppState>, v: Viewer, Form(f): Form<ActForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let target = member(&state, &v, f.user).await?;
    let me = v.require_user()?.id.get();
    let pool = state.db()?;
    let reason = f.reason.trim();
    match f.action.as_str() {
        kind @ ("silence" | "suspend") => {
            let days: i64 = f.days.trim().parse().unwrap_or(0);
            if !(1..=365).contains(&days) {
                return Err(AppError::Validation("Choose 1 to 365 days.".into()));
            }
            if reason.is_empty() || reason.chars().count() > 500 {
                return Err(AppError::Validation("Give a reason of up to 500 characters.".into()));
            }
            let until = chrono::Utc::now() + chrono::Duration::days(days);
            let mut change = UserChange::default();
            if kind == "silence" {
                change.silenced_until = Some(Some(until));
            } else {
                change.suspended_until = Some(Some(until));
            }
            if !members::update_user(pool, target.id.get(), &change, me).await.db()? {
                return Err(AppError::NotFound);
            }
            let (notice, action) = if kind == "silence" { ("silenced", "silenced") } else { ("suspended", "suspended") };
            log(&state, me, &format!("mod.{kind}"), target.id.get(), json!({ "reason": reason, "days": days })).await?;
            crate::community::notifications::send(&state, me, &[target.id.get()], notice, &json!({ "url": "/settings" })).await;
            crate::moderation::dsa::record(
                &state,
                crate::moderation::dsa::Restriction {
                    user_id: target.id.get(),
                    target_type: "account",
                    target_id: target.id.get(),
                    action,
                    ground: "terms",
                    category: "scope_of_platform_service",
                    facts: reason,
                    automated_detection: false,
                    automated_decision: false,
                    notice_id: None,
                    moderator_id: Some(me),
                },
            )
            .await;
            crate::platform::webhooks::moderated(
                &state,
                kind,
                me,
                "user",
                target.id.get(),
                json!({ "reason": reason, "days": days }),
            )
            .await;
            state.microcache.clear();
            Ok(Redirect::to(&back_to(&f.back, kind)).into_response())
        }
        "lift" => {
            // A moderator does not undo what an administrator imposed (the statement of reasons
            // names who restricted the account; the latest one decides).
            if !v.actor.is_admin {
                let by_admin: bool = sqlx::query_scalar(
                    "SELECT coalesce((SELECT EXISTS (SELECT 1 FROM users u JOIN groups g ON g.is_admin
                         AND (g.id = u.primary_group_id
                              OR g.id IN (SELECT group_id FROM user_secondary_groups WHERE user_id = u.id))
                         WHERE u.id = s.moderator_id)
                       FROM moderation_statements s
                       WHERE s.user_id = $1 AND s.target_type = 'account' AND s.action IN ('suspended', 'silenced')
                       ORDER BY s.id DESC LIMIT 1), false)",
                )
                .bind(target.id.get())
                .fetch_one(pool)
                .await
                .db()?;
                if by_admin {
                    return Err(AppError::Validation("Only an administrator can lift this restriction.".into()));
                }
            }
            let change = UserChange { suspended_until: Some(None), silenced_until: Some(None), ..Default::default() };
            if !members::update_user(pool, target.id.get(), &change, me).await.db()? {
                return Err(AppError::NotFound);
            }
            log(&state, me, "mod.lift", target.id.get(), json!({})).await?;
            crate::community::notifications::send(&state, me, &[target.id.get()], "restored", &json!({ "url": "/settings" })).await;
            crate::platform::webhooks::moderated(&state, "lift", me, "user", target.id.get(), json!({})).await;
            state.microcache.clear();
            Ok(Redirect::to(&back_to(&f.back, "lift")).into_response())
        }
        "spammer" => {
            if f.confirm != "1" {
                return Err(AppError::Validation("Confirm that this member is a spammer.".into()));
            }
            let topics = clean_up_spammer(&state, &v, &target, f.ban == "1").await?;
            log(&state, me, "mod.spammer", target.id.get(), json!({ "reason": "spam", "topics": topics, "banned": f.ban == "1" })).await?;
            crate::platform::webhooks::moderated(
                &state,
                "spammer",
                me,
                "user",
                target.id.get(),
                json!({ "topics": topics, "banned": f.ban == "1" }),
            )
            .await;
            Ok(Redirect::to(&back_to(&f.back, "spammer")).into_response())
        }
        _ => Err(AppError::BadRequest("unknown action".into())),
    }
}

/// Deletes what a new member posted, suspends the account and, if asked, bans their email address
/// and last address. Wiping a member's history is for new accounts; an old member goes to an
/// administrator. Returns how many topics were touched.
pub(crate) async fn clean_up_spammer(state: &AppState, v: &Viewer, target: &User, ban: bool) -> Result<i64, AppError> {
    let me = v.require_user()?.id.get();
    if !v.actor.is_admin && target.trust_level > 1 {
        return Err(AppError::Validation(
            "Clean-up is for new members (trust level 0 or 1). Ask an administrator.".into(),
        ));
    }
    let pool = state.db()?;
    let id = target.id.get();
    let last_ip: Option<String> = sqlx::query_scalar(
        "SELECT host(ip) FROM sessions WHERE user_id = $1 AND ip IS NOT NULL ORDER BY last_used_at DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .db()?;
    let topics = bf_db::moderation::topics::delete_spammer(pool, id, Some(me)).await.db()?;
    if ban {
        let email = target.email.trim().to_lowercase();
        if email.contains('@') {
            bans::add(pool, "email", &email, Some("spammer"), Some(me)).await.db()?;
        }
        // Never an internal address, never the moderator's own, never one on the allow list.
        if let Some(ip) = last_ip.filter(|ip| {
            ip.parse::<std::net::IpAddr>().is_ok_and(|a| !crate::platform::outbound::forbidden_ip(a) && Some(a) != v.ip)
        }) && !bans::ip_allowed(pool, ip.parse().unwrap_or(std::net::IpAddr::from([0, 0, 0, 0]))).await.unwrap_or(false)
        {
            bans::add(pool, "ip", &ip, Some("spammer"), Some(me)).await.db()?;
        }
    }
    crate::moderation::topics::refresh(&state);
    Ok(topics)
}

#[derive(Deserialize)]
struct BanForm {
    csrf: String,
    kind: String,
    value: String,
    #[serde(default)]
    reason: String,
    /// Days until the ban lapses; empty for a ban until lifted.
    #[serde(default)]
    days: String,
}

/// Whether a moderator may ban this: one address or a small network, or one email address.
pub(crate) fn moderator_ban_ok(kind: &str, value: &str, admin: bool) -> Result<(), &'static str> {
    match kind {
        "ip" => {
            let net: sqlx::types::ipnetwork::IpNetwork =
                value.parse().map_err(|_| "Enter an IP address or network.")?;
            let widest = if net.is_ipv4() { 24 } else { 48 };
            if !admin && net.prefix() < widest {
                return Err("A network can be at most a /24 (IPv4) or /48 (IPv6).");
            }
            Ok(())
        }
        "ip_allow" if admin => {
            value.parse::<sqlx::types::ipnetwork::IpNetwork>().map_err(|_| "Enter an IP address or network.")?;
            Ok(())
        }
        "email" => {
            if value.len() > 254 || value.contains(char::is_whitespace) || !value.contains('@') {
                return Err("Enter an email address.");
            }
            if !admin && (value.starts_with('@') || value.starts_with("*@")) {
                return Err("Only an administrator can ban a whole domain.");
            }
            Ok(())
        }
        _ => Err("Choose what to ban."),
    }
}

async fn add_ban(State(state): State<AppState>, v: Viewer, Form(f): Form<BanForm>) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let me = v.require_user()?.id.get();
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let value = f.value.trim().to_lowercase();
    moderator_ban_ok(&f.kind, &value, v.actor.is_admin).map_err(|m| AppError::Validation(m.into()))?;
    if f.kind == "ip"
        && let (Ok(net), Some(mine)) = (value.parse::<sqlx::types::ipnetwork::IpNetwork>(), v.ip)
        && net.contains(mine)
    {
        return Err(AppError::Validation("That would ban your own address.".into()));
    }
    // An address the administrators put on the allow list is never banned from here.
    if f.kind == "ip"
        && let Ok(net) = value.parse::<sqlx::types::ipnetwork::IpNetwork>()
        && bans::allow_listed(state.db()?, net).await.db()?
    {
        return Err(AppError::Validation("That address is on the allow list and cannot be banned.".into()));
    }
    let reason: String = f.reason.trim().chars().take(200).collect();
    let expires = f.days.trim().parse::<i64>().ok().filter(|d| *d > 0).map(|d| chrono::Utc::now() + chrono::Duration::days(d.min(3650)));
    let ban = bans::add_until(
        state.db()?,
        &f.kind,
        &value,
        Some(&reason).filter(|r| !r.is_empty()).map(String::as_str),
        Some(me),
        expires,
    )
        .await
        .db()?;
    crate::platform::webhooks::moderated(&state, "ban", me, "ban", ban, json!({ "kind": f.kind, "value": value })).await;
    Ok(Redirect::to("/modcp?tab=bans&done=ban").into_response())
}

#[derive(Deserialize)]
struct CsrfOnly {
    csrf: String,
}

async fn remove_ban(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<CsrfOnly>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let me = v.require_user()?.id.get();
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT kind, value FROM bans WHERE id = $1").bind(id).fetch_optional(pool).await.db()?;
    // A moderator lifts only what a moderator may add: not an administrator's wide network,
    // whole-domain or operator ban, and not an entry of the allow list.
    if let Some((kind, value)) = &row
        && !v.actor.is_admin
        && moderator_ban_ok(kind, value, false).is_err()
    {
        return Err(AppError::Validation("Only an administrator can lift this ban.".into()));
    }
    let what: Option<String> = row.map(|(kind, value)| format!("{kind} {value}"));
    if !bans::remove(pool, id).await.db()? {
        return Err(AppError::NotFound);
    }
    bf_db::admin::audit::record(pool, bf_db::admin::audit::Entry::new("mod.unban").by(me).on(format!("ban:{id}")).details(json!({ "reason": what })))
        .await
        .db()?;
    crate::platform::webhooks::moderated(&state, "unban", me, "ban", id, json!({ "what": what })).await;
    Ok(Redirect::to("/modcp?tab=bans&done=unban").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_panel_addresses_are_followed() {
        assert_eq!(back_to("/modcp?tab=members&find=ann", "lift"), "/modcp?tab=members&find=ann&done=lift");
        assert_eq!(back_to("https://evil.example/", "lift"), "/modcp?tab=members&done=lift");
        assert_eq!(back_to("//evil.example", "ban"), "/modcp?tab=members&done=ban");
        assert_eq!(back_to("/modcp?tab=members&done=silence", "lift"), "/modcp?tab=members&done=lift");
        assert_eq!(back_to("/modcp\r\nSet-Cookie: x", "lift"), "/modcp?tab=members&done=lift");
    }

    #[test]
    fn moderators_cannot_make_wide_bans() {
        assert!(moderator_ban_ok("ip", "203.0.113.7", false).is_ok());
        assert!(moderator_ban_ok("ip", "203.0.113.0/24", false).is_ok());
        assert!(moderator_ban_ok("ip", "203.0.0.0/16", false).is_err());
        assert!(moderator_ban_ok("ip", "0.0.0.0/0", false).is_err());
        assert!(moderator_ban_ok("ip", "2001:db8::/32", false).is_err());
        assert!(moderator_ban_ok("ip", "203.0.0.0/8", true).is_ok());
        assert!(moderator_ban_ok("email", "spam@example.com", false).is_ok());
        assert!(moderator_ban_ok("email", "@gmail.com", false).is_err());
        assert!(moderator_ban_ok("email", "@gmail.com", true).is_ok());
        assert!(moderator_ban_ok("email", "no at sign", false).is_err());
    }
}
