//! Outside logins: the OAuth clients of the services members can sign in with, one generic OpenID
//! Connect provider and a DiscourseConnect login site. A login appears on the sign-in page as soon as
//! its client ID and secret are set. Secrets are stored sealed (see `bf_db::settings::is_secret`).
//! Each login also has an on/off switch, so an administrator can turn one off and on again without
//! deleting its credentials.

use super::super::{SettingDef, SettingType::*};
use super::def;
use crate::i18n::en;
use serde_json::json;

/// A login's on/off switch: on unless an administrator turns it off.
fn switch(key: &'static str, description: &'static str) -> SettingDef {
    SettingDef { default: json!(true), ..def(key, Bool, false, description) }
}

/// An empty text setting.
fn text(key: &'static str, description: &'static str) -> SettingDef {
    SettingDef { default: json!(""), ..def(key, String, false, description) }
}

pub(super) fn defs() -> Vec<SettingDef> {
    vec![
        switch(
            "login_google_enabled",
            en("Google sign-in is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_github_enabled",
            en("GitHub sign-in is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_discord_enabled",
            en("Discord sign-in is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_microsoft_enabled",
            en("Microsoft sign-in is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_twitch_enabled",
            en("Twitch sign-in is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch("login_x_enabled", en("X sign-in is on (turn it off to hide it without deleting its credentials)")),
        switch(
            "login_apple_enabled",
            en("Sign in with Apple is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_oidc_enabled",
            en("The other OpenID Connect login is on (turn it off to hide it without deleting its credentials)"),
        ),
        switch(
            "login_sso_enabled",
            en("The DiscourseConnect login is on (turn it off to hide it without deleting its credentials)"),
        ),
        text(
            "login_google_id",
            en("Google sign-in: OAuth client ID. The redirect address is your site's /auth/google/callback"),
        ),
        text("login_google_secret", en("Google sign-in: OAuth client secret")),
        text(
            "login_github_id",
            en("GitHub sign-in: OAuth client ID. The redirect address is your site's /auth/github/callback"),
        ),
        text("login_github_secret", en("GitHub sign-in: OAuth client secret")),
        text(
            "login_discord_id",
            en("Discord sign-in: OAuth client ID. The redirect address is your site's /auth/discord/callback"),
        ),
        text("login_discord_secret", en("Discord sign-in: OAuth client secret")),
        text(
            "login_microsoft_id",
            en(
                "Microsoft sign-in: application (client) ID. The redirect address is your site's /auth/microsoft/callback",
            ),
        ),
        text("login_microsoft_secret", en("Microsoft sign-in: client secret")),
        text(
            "login_twitch_id",
            en("Twitch sign-in: client ID. The redirect address is your site's /auth/twitch/callback"),
        ),
        text("login_twitch_secret", en("Twitch sign-in: client secret")),
        text("login_x_id", en("X sign-in: OAuth 2.0 client ID. The redirect address is your site's /auth/x/callback")),
        text("login_x_secret", en("X sign-in: OAuth 2.0 client secret")),
        text("login_apple_id", en("Sign in with Apple: Services ID")),
        text("login_apple_team_id", en("Sign in with Apple: team ID")),
        text("login_apple_key_id", en("Sign in with Apple: key ID")),
        text("login_apple_key", en("Sign in with Apple: contents of the .p8 private key")),
        text("login_oidc_name", en("Other OpenID Connect provider: name shown on the button")),
        text("login_oidc_authorize_url", en("Other OpenID Connect provider: authorization address")),
        text("login_oidc_token_url", en("Other OpenID Connect provider: token address")),
        text("login_oidc_userinfo_url", en("Other OpenID Connect provider: user info address")),
        text("login_oidc_client_id", en("Other OpenID Connect provider: client ID")),
        text("login_oidc_client_secret", en("Other OpenID Connect provider: client secret")),
        text("login_sso_url", en("DiscourseConnect login site: its address (https)")),
        text("login_sso_secret", en("DiscourseConnect login site: shared secret, at least 32 characters")),
        text("login_sso_name", en("DiscourseConnect login site: name shown on the button")),
    ]
}
