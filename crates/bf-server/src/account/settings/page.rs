//! The settings page itself and the small helpers it draws with.

use crate::http::render::{DbResult, ago, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bf_core::{AppError, totp};
use bf_db::accounts::{security, sessions};
use bf_web::pages::{SessionView, SettingsPage, TotpSetup, render};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct SettingsQuery {
    pub(super) setup: Option<String>,
    /// A member just unmuted from the list: offered an undo.
    pub(super) unmuted: Option<i64>,
}

pub(super) async fn settings_page(
    state: &AppState,
    v: &Viewer,
    setup: bool,
    message: Option<String>,
    error: Option<String>,
) -> Result<Response, AppError> {
    settings_page_ex(state, v, setup, message, error, None, None, None).await
}

/// `new_backup_codes`: codes this request just generated, shown once. `retry_secret`:
/// the secret just submitted to `/settings/2fa/enable` and rejected for a bad code — reused
/// instead of generating (and showing) a different one, so a mistyped code doesn't silently
/// invalidate what the member already scanned.
pub(super) async fn settings_page_ex(
    state: &AppState,
    v: &Viewer,
    setup: bool,
    message: Option<String>,
    error: Option<String>,
    new_backup_codes: Option<Vec<String>>,
    retry_secret: Option<String>,
    unmuted: Option<i64>,
) -> Result<Response, AppError> {
    // Messages come from many places, some marked English (`en`): shown in the viewer's language.
    let (message, error) = (message.map(|m| v.t().msg(&m)), error.map(|e| v.t().msg(&e)));
    let user = v.require_user()?;
    let pool = state.db()?;
    let acct = security::account(pool, user.id).await.db()?.ok_or(AppError::NotFound)?;
    let sessions = sessions::sessions(pool, user.id).await.db()?;
    let profile = bf_db::accounts::preferences::profile_fields(pool, user.id).await.db()?;
    let prefs = crate::account::settings::prefs_view(pool, user.id.get()).await?;
    let (quiet_start, quiet_end) = bf_db::accounts::preferences::quiet_hours(pool, user.id.get()).await.db()?;
    let fields = custom_field_inputs(pool, v, user.id).await?;
    let digest = bf_db::platform::digests::digest_setting(pool, user.id.get()).await.db()?;
    let mailing_list = state
        .settings()
        .bool("mailing_list_mode")
        .then_some(bf_db::community::notifications::mailing_list_on(pool, user.id.get()).await.db()?);
    let newsletter = if crate::platform::newsletter::enabled(state) {
        Some(bf_db::platform::newsletter::subscribed(pool, user.id.get()).await.db()?)
    } else {
        None
    };
    let passkeys = crate::auth::passkeys::list(state, user.id).await?;
    let totp_setup = (setup && acct.totp_secret.is_none()).then(|| {
        let secret = retry_secret.unwrap_or_else(totp::new_secret);
        let uri = totp::otpauth_uri(state.settings().str("site_name"), &acct.username, &secret);
        let qr = totp::qr_svg(&uri);
        TotpSetup { secret, uri, qr }
    });
    let backup_codes_left =
        if acct.totp_secret.is_some() { Some(security::backup_codes_left(pool, user.id).await.db()?) } else { None };
    let privacy =
        crate::account::settings::privacy::view(state, user.id, acct.password_hash.is_some(), passkeys.len()).await?;
    let undo_unmute = match unmuted {
        Some(id) if !privacy.muted_users.iter().any(|(m, _)| *m == id) => {
            bf_db::accounts::users::user_by_id(pool, id.into()).await.db()?.map(|u| (id, u.username))
        }
        _ => None,
    };
    let (mut l, set) = layout(state, v, Some(&v.t().msg("Account settings")));
    l.noindex = true;
    let status = if error.is_some() { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK };
    let body = SettingsPage {
        layout: l,
        username: acct.username.clone(),
        message,
        error,
        email: mask_email(&acct.email),
        email_verified: acct.email_verified,
        totp_enabled: acct.totp_secret.is_some(),
        totp_setup,
        backup_codes_left,
        new_backup_codes,
        display_name: profile.display_name.clone().unwrap_or_default(),
        hide_last_seen: profile.hide_last_seen,
        about: profile.about_md.unwrap_or_default(),
        location: profile.location.unwrap_or_default(),
        website: profile.website.unwrap_or_default(),
        timezone: profile.timezone.unwrap_or_default(),
        profile_visibility: profile.profile_visibility,
        muted_tags: bf_db::forum::tags::muted_tag_names(pool, user.id.get()).await.db()?.join(", "),
        theme_choices: bf_web::theme::choices(&v.actor.groups.iter().map(|g| g.get()).collect::<Vec<_>>()),
        theme_id: user.theme_id,
        schemes: layout_schemes(v).0,
        default_scheme: layout_schemes(v).1,
        keeps_visitors: bf_db::accounts::users::profile(pool, user.id).await.db()?.is_some_and(|p| p.show_visitors),
        deletion_pending: bf_db::accounts::lifecycle::deletion_date(pool, user.id.get())
            .await
            .db()?
            .map(|d| d.format("%Y-%m-%d").to_string()),
        api_keys: api_key_rows(pool, v, user.id.get()).await?,
        plugin_scopes: bf_db::platform::registry_content::api_scopes(pool)
            .await
            .db()?
            .into_iter()
            .map(|(scope, label, _)| (scope, label))
            .collect(),
        prefs,
        quiet_start: quiet_start.map_or_else(String::new, |h| h.to_string()),
        quiet_end: quiet_end.map_or_else(String::new, |h| h.to_string()),
        fields,
        digest,
        mailing_list,
        newsletter,
        passkeys,
        identities: privacy.identities,
        identity_removable: privacy.identity_removable,
        connectable: privacy.connectable,
        allow_pm_from: privacy.allow_pm_from,
        muted_users: privacy.muted_users,
        undo_unmute,
        trusted_devices: privacy.trusted_devices,
        locales: bf_core::i18n::locales(),
        sessions: sessions
            .into_iter()
            .map(|s| SessionView {
                browser_icon: session_icons(s.user_agent.as_deref()).0.to_owned(),
                os_icon: session_icons(s.user_agent.as_deref()).1.to_owned(),
                current: v.session_digest.as_deref() == Some(s.token_digest.as_str()),
                id: s.token_digest,
                created: ago(s.created_at),
                last_used: ago(s.last_used_at),
                ip: s.ip.map(|i| i.ip().to_string()),
                device: s.user_agent.as_deref().map(bf_core::auth::device_label),
                country: s.country.map(|c| c.to_lowercase()).filter(|c| bf_web::assets::flag(c).is_some()),
            })
            .collect(),
    };
    Ok(page((status, render(&body)).into_response(), set, false))
}

pub(super) async fn settings(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<SettingsQuery>,
) -> Result<Response, AppError> {
    settings_page_ex(&state, &v, q.setup.as_deref() == Some("2fa"), None, None, None, None, q.unmuted).await
}

/// The profile fields the admin defined, each with this member's answer.
async fn custom_field_inputs(
    pool: &sqlx::PgPool,
    v: &Viewer,
    user: bf_core::ids::UserId,
) -> Result<Vec<bf_web::pages::FieldInput>, AppError> {
    let values = bf_db::accounts::fields::values(pool, user).await.db()?;
    Ok(bf_db::accounts::fields::fields(pool)
        .await
        .db()?
        .into_iter()
        .map(|f| {
            let value = values.iter().find(|(id, _)| *id == f.id).map(|(_, v)| v.clone()).unwrap_or_default();
            let hint = f.hint(&v.t());
            bf_web::pages::FieldInput {
                id: f.id,
                label: f.label,
                kind: f.kind,
                options: f.options,
                required: f.required,
                value,
                hint,
                shorthand: f.url_shorthand,
            }
        })
        .collect())
}

/// The member's API keys as the settings table shows them: name, scope, last use and expiry.
async fn api_key_rows(
    pool: &sqlx::PgPool,
    v: &Viewer,
    user: i64,
) -> Result<Vec<(i64, String, String, bf_web::ago::Ago, String)>, AppError> {
    Ok(bf_db::platform::api::keys(pool, user)
        .await
        .db()?
        .into_iter()
        .map(|k| {
            let expires = k.expires_at.map_or_else(
                || v.t().msg("no expiry"),
                |at| {
                    if at < chrono::Utc::now() {
                        v.t().msg("expired")
                    } else {
                        v.t().msgf("expires {date}", &[("date", &at.format("%Y-%m-%d").to_string())])
                    }
                },
            );
            (
                k.id,
                k.name,
                if k.plugin_scopes.is_empty() {
                    k.scope
                } else {
                    format!("{} + {}", k.scope, k.plugin_scopes.join(", "))
                },
                k.last_used_at.map_or_else(|| bf_web::ago::Ago::plain("never"), crate::http::render::ago),
                expires,
            )
        })
        .collect())
}

/// Hides an address on pages, leaving enough to recognise it: `alice@example.com` becomes
/// `a•••@e•••.com`. The full address stays out of the HTML, so a screenshot, a shared screen or
/// an unattended session does not give it away.
pub(super) fn mask_email(email: &str) -> String {
    let hide = |part: &str| part.chars().next().map_or_else(String::new, |c| format!("{c}•••"));
    let Some((local, domain)) = email.rsplit_once('@') else { return hide(email) };
    match domain.rsplit_once('.') {
        Some((host, tld)) => format!("{}@{}.{tld}", hide(local), hide(host)),
        None => format!("{}@{}", hide(local), hide(domain)),
    }
}

/// Sprite ids for a session's browser and system; anything unknown gets a plain globe.
pub(super) fn session_icons(ua: Option<&str>) -> (&'static str, &'static str) {
    let (browser, system) = ua.map_or(("", ""), bf_core::auth::device_parts);
    let b = match browser {
        "Chrome" => "b-chrome",
        "Firefox" => "b-firefox",
        "Safari" => "b-safari",
        "Opera" => "b-opera",
        _ => "globe",
    };
    let o = match system {
        "Windows" => "os-windows",
        "Android" => "os-android",
        "iPhone" | "iPad" | "macOS" => "os-apple",
        "Linux" => "os-linux",
        _ => "globe",
    };
    (b, o)
}

/// The schemes to pick from besides the site's default, and that default: (address name, name).
pub(super) fn layout_schemes(v: &Viewer) -> (Vec<(String, String)>, (String, String)) {
    let (choices, light, dark) = bf_web::schemes::choices();
    // What a member who picked nothing sees: the light default stands for it in the picker.
    // When light and dark have the same default, that option is named after it and not listed
    // twice. When they differ it is just "the site's default" (an empty name).
    let shown = if light == dark {
        choices
            .iter()
            .find(|c| c.slug == light)
            .map_or_else(|| (light.clone(), light.clone()), |c| (c.slug.clone(), c.name.clone()))
    } else {
        (light.clone(), String::new())
    };
    let mut list: Vec<(String, String)> =
        choices.into_iter().filter(|c| light != dark || c.slug != light).map(|c| (c.slug, c.name)).collect();
    // A theme can bring schemes of its own, named by their address.
    let groups: Vec<i64> = v.actor.groups.iter().map(|g| g.get()).collect();
    let theme = bf_web::theme::resolve(&[v.user.as_ref().and_then(|u| u.theme_id)], &groups);
    list.extend(theme.iter().flat_map(|t| t.schemes.iter().map(|n| (n.clone(), n.clone()))));
    (list, shown)
}

#[cfg(test)]
mod mask_tests {
    use super::mask_email;

    #[test]
    fn hides_the_address_but_keeps_it_recognisable() {
        assert_eq!(mask_email("alice@example.com"), "a•••@e•••.com");
        assert_eq!(mask_email("ż@mail.example.co"), "ż•••@m•••.co");
        assert_eq!(mask_email("root@localhost"), "r•••@l•••");
        assert_eq!(mask_email("nonsense"), "n•••");
        assert!(!mask_email("alice@example.com").contains("alice"));
    }
}
