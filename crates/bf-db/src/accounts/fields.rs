//! Custom profile fields, group limits, topic prefixes and email digests.

use bf_core::i18n::T;
use bf_core::ids::UserId;
use serde::{Deserialize, Serialize};
pub use sqlx::Error;
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Field {
    #[serde(default)]
    pub id: i64,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub show_on_register: bool,
    #[serde(default = "yes")]
    pub public: bool,
    #[serde(default)]
    pub position: i32,
    /// An icon from the site's sprite, shown beside the field.
    #[serde(default)]
    pub icon: Option<String>,
    /// Link fields: the value must start with one of these; empty allows any http(s) link.
    #[serde(default)]
    pub url_prefixes: Vec<String>,
    /// Link fields: a value without `://` is the part after the first prefix (a username).
    #[serde(default)]
    pub url_shorthand: bool,
    /// Link fields: only https:// links.
    #[serde(default)]
    pub https_only: bool,
    /// The plugin that declared this field: it is read-only in the admin panel.
    #[serde(default, skip_deserializing)]
    pub plugin: Option<String>,
    /// False: members cannot change the answer (the plugin sets it).
    #[serde(default = "yes")]
    pub editable: bool,
}

fn yes() -> bool {
    true
}

pub const KINDS: [&str; 4] = ["text", "textarea", "url", "select"];

/// Characters a link may never hold: they would end an attribute or start markup.
fn unsafe_char(c: char) -> bool {
    c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '\\')
}

/// A link prefix as staff may set it: a full http(s) address start, no spaces or quotes.
fn prefix_ok(p: &str, https_only: bool) -> bool {
    let scheme = p.starts_with("https://") || (!https_only && p.starts_with("http://"));
    let host = p.split_once("://").map_or("", |(_, rest)| rest);
    scheme && !host.is_empty() && !host.starts_with('/') && p.len() <= 100 && !p.chars().any(unsafe_char)
}

fn starts_with_ignore_case(value: &str, prefix: &str) -> bool {
    value.len() >= prefix.len()
        && value.is_char_boundary(prefix.len())
        && value[..prefix.len()].eq_ignore_ascii_case(prefix)
}

impl Field {
    /// Cleans a definition from the admin panel; `Err` holds a message for the form.
    pub fn normalise(&mut self) -> Result<(), String> {
        self.label = self.label.trim().to_owned();
        if self.label.is_empty() || self.label.chars().count() > 80 {
            return Err("Give the field a label of up to 80 characters.".into());
        }
        if !KINDS.contains(&self.kind.as_str()) {
            return Err("Choose a type: short text, long text, link or choice.".into());
        }
        self.icon = self.icon.take().map(|i| i.trim().to_owned()).filter(|i| !i.is_empty());
        if self.icon.as_deref().is_some_and(|i| {
            i.len() > 40 || !i.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        }) {
            return Err("Choose an icon from the list.".into());
        }
        self.options = if self.kind == "select" {
            let mut seen: Vec<String> = Vec::new();
            for o in self.options.iter().map(|o| o.trim()).filter(|o| !o.is_empty()) {
                if !seen.iter().any(|x| x == o) {
                    seen.push(o.to_owned());
                }
            }
            if seen.len() < 2 || seen.len() > 50 || seen.iter().any(|o| o.chars().count() > 80) {
                return Err("A choice needs 2 to 50 options of up to 80 characters.".into());
            }
            seen
        } else {
            Vec::new()
        };
        if self.kind == "url" {
            let mut prefixes: Vec<String> = Vec::new();
            for p in self.url_prefixes.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
                if !prefix_ok(p, self.https_only) {
                    let scheme = if self.https_only { "https://" } else { "https:// or http://" };
                    return Err(format!("\"{p}\" is not a link start: use {scheme}, a host and an optional path."));
                }
                if !prefixes.iter().any(|x| x.eq_ignore_ascii_case(p)) {
                    prefixes.push(p.to_owned());
                }
            }
            if prefixes.len() > 10 {
                return Err("Up to 10 allowed link starts.".into());
            }
            if self.url_shorthand && prefixes.is_empty() {
                return Err("Short entries need an allowed link start to add them to.".into());
            }
            self.url_prefixes = prefixes;
        } else {
            self.url_prefixes.clear();
            self.url_shorthand = false;
            self.https_only = false;
        }
        Ok(())
    }

    /// A short line for members under the input: what a link field accepts.
    pub fn hint(&self, t: &T) -> Option<String> {
        if self.kind != "url" {
            return None;
        }
        let scheme = if self.https_only { "https://" } else { "http(s)://" };
        Some(match (self.url_prefixes.as_slice(), self.url_shorthand) {
            ([], _) => t.msgf("A link starting with {n}", &[("n", scheme)]),
            ([one], true) => t.msgf("Your name on {n}… or the full link", &[("n", one)]),
            ([one], false) => t.msgf("A link starting with {n}", &[("n", one)]),
            (many, true) => t.msgf(
                "Your name on {n}… or a link starting with {n2}",
                &[("n", &many[0]), ("n2", &many[1..].join(", "))],
            ),
            (many, false) => t.msgf("A link starting with {n}", &[("n", &many.join(", "))]),
        })
    }

    /// Normalises a submitted value; `Err` holds a message for the form.
    pub fn check(&self, raw: &str) -> Result<Option<String>, String> {
        self.check_in(&T::default(), raw)
    }

    /// `check`, with its message in `t`'s language.
    pub fn check_in(&self, t: &T, raw: &str) -> Result<Option<String>, String> {
        let label = &self.label;
        let v = raw.trim();
        if v.is_empty() {
            return if self.required { Err(t.msgf("{label} is required.", &[("label", label)])) } else { Ok(None) };
        }
        let max = if self.kind == "textarea" { 2000 } else { 200 };
        if v.chars().count() > max {
            return Err(t.msgf("{label} is too long.", &[("label", label)]));
        }
        match self.kind.as_str() {
            "url" => self.check_link(t, v).map(Some),
            "select" if !self.options.iter().any(|o| o == v) => {
                Err(t.msgf("Choose a value for {label}.", &[("label", label)]))
            }
            _ => Ok(Some(v.to_owned())),
        }
    }

    fn check_link(&self, t: &T, v: &str) -> Result<String, String> {
        let label = &self.label;
        // `octocat`, `@octocat` or `/octocat` becomes the first prefix plus the name.
        let full = match self.url_prefixes.first() {
            Some(first) if self.url_shorthand && !v.contains("://") => {
                let name = v.trim_start_matches(['@', '/']);
                if name.is_empty() {
                    return Err(t.msgf("{label}: enter a name or a link.", &[("label", label)]));
                }
                format!("{first}{name}")
            }
            _ => v.to_owned(),
        };
        if full.chars().any(unsafe_char) {
            return Err(t.msgf("{label} cannot contain spaces or quotes.", &[("label", label)]));
        }
        let https = starts_with_ignore_case(&full, "https://");
        if !(https || (!self.https_only && starts_with_ignore_case(&full, "http://"))) {
            return Err(if self.https_only {
                t.msgf("{label} must start with https://", &[("label", label)])
            } else {
                t.msgf("{label} must start with https:// or http://", &[("label", label)])
            });
        }
        if full.split_once("://").is_some_and(|(_, rest)| rest.is_empty() || rest.starts_with('/')) {
            return Err(t.msgf("{label} needs a full address.", &[("label", label)]));
        }
        if !self.url_prefixes.is_empty() {
            match self.url_prefixes.iter().find(|p| starts_with_ignore_case(&full, p)) {
                // Just the prefix is not a profile: something must follow it.
                Some(p) if full.len() == p.len() => {
                    return Err(t.msgf("{label}: add your name after {p}", &[("label", label), ("p", p)]));
                }
                Some(_) => {}
                None => {
                    return Err(t.msgf(
                        "{label} must start with {p}",
                        &[("label", label), ("p", &self.url_prefixes.join(&format!(" {} ", t.msg("or"))))],
                    ));
                }
            }
        }
        if full.chars().count() > 200 {
            return Err(t.msgf("{label} is too long.", &[("label", label)]));
        }
        Ok(full)
    }
}

macro_rules! cols {
    () => {
        "id, label, kind, options, required, show_on_register, public, position, icon, url_prefixes, url_shorthand, https_only, plugin, editable"
    };
}

pub async fn fields(pool: &PgPool) -> Result<Vec<Field>, Error> {
    // Only the constant COLS is interpolated.
    sqlx::query_as(concat!("SELECT ", cols!(), " FROM profile_fields WHERE active ORDER BY position, id"))
        .fetch_all(pool)
        .await
}

/// Adds a field at the end of the list.
pub async fn create_field(pool: &PgPool, f: &Field) -> Result<i64, Error> {
    sqlx::query_scalar(
        "INSERT INTO profile_fields (label, kind, options, required, show_on_register, public, position, icon, url_prefixes, url_shorthand, https_only)
         VALUES ($1, $2, $3, $4, $5, $6, (SELECT coalesce(max(position), -1) + 1 FROM profile_fields), $7, $8, $9, $10) RETURNING id",
    )
    .bind(&f.label)
    .bind(&f.kind)
    .bind(&f.options)
    .bind(f.required)
    .bind(f.show_on_register)
    .bind(f.public)
    .bind(&f.icon)
    .bind(&f.url_prefixes)
    .bind(f.url_shorthand)
    .bind(f.https_only)
    .fetch_one(pool)
    .await
}

/// Changes a field's definition. Its type stays: members' answers were checked against it.
pub async fn update_field(pool: &PgPool, id: i64, f: &Field) -> Result<bool, Error> {
    Ok(sqlx::query(
        "UPDATE profile_fields SET label = $2, options = $3, required = $4, show_on_register = $5, public = $6,
                icon = $7, url_prefixes = $8, url_shorthand = $9, https_only = $10
         WHERE id = $1 AND plugin IS NULL",
    )
    .bind(id)
    .bind(&f.label)
    .bind(&f.options)
    .bind(f.required)
    .bind(f.show_on_register)
    .bind(f.public)
    .bind(&f.icon)
    .bind(&f.url_prefixes)
    .bind(f.url_shorthand)
    .bind(f.https_only)
    .execute(pool)
    .await?
    .rows_affected()
        > 0)
}

/// Puts the listed fields in this order.
pub async fn reorder_fields(pool: &PgPool, ids: &[i64]) -> Result<(), Error> {
    sqlx::query(
        "UPDATE profile_fields f SET position = o.n - 1
         FROM unnest($1::bigint[]) WITH ORDINALITY AS o(id, n) WHERE f.id = o.id",
    )
    .bind(ids)
    .execute(pool)
    .await
    .map(|_| ())
}

/// How many members answered each field: (field id, count).
pub async fn answer_counts(pool: &PgPool) -> Result<Vec<(i64, i64)>, Error> {
    sqlx::query_as("SELECT field_id, count(*) FROM user_field_values GROUP BY field_id").fetch_all(pool).await
}

pub async fn delete_field(pool: &PgPool, id: i64) -> Result<bool, Error> {
    Ok(sqlx::query("DELETE FROM profile_fields WHERE id = $1 AND plugin IS NULL")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected()
        > 0)
}

pub async fn values(pool: &PgPool, user_id: UserId) -> Result<Vec<(i64, String)>, Error> {
    sqlx::query_as("SELECT field_id, value FROM user_field_values WHERE user_id = $1")
        .bind(user_id)
        .fetch_all(pool)
        .await
}

/// Replaces a user's values for the given fields (`None` clears one).
pub async fn save_values(pool: &PgPool, user_id: UserId, values: &[(i64, Option<String>)]) -> Result<(), Error> {
    let mut tx = pool.begin().await?;
    for (field, value) in values {
        match value {
            Some(v) => {
                sqlx::query(
                    "INSERT INTO user_field_values (user_id, field_id, value) VALUES ($1, $2, $3)
                     ON CONFLICT (user_id, field_id) DO UPDATE SET value = EXCLUDED.value",
                )
                .bind(user_id)
                .bind(field)
                .bind(v)
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM user_field_values WHERE user_id = $1 AND field_id = $2")
                    .bind(user_id)
                    .bind(field)
                    .execute(&mut *tx)
                    .await?;
            }
        }
    }
    tx.commit().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: &str, required: bool) -> Field {
        Field {
            id: 1,
            label: "Field".into(),
            kind: kind.into(),
            options: vec!["Red".into(), "Blue".into()],
            required,
            show_on_register: false,
            public: true,
            position: 0,
            icon: None,
            url_prefixes: vec![],
            url_shorthand: false,
            https_only: false,
            plugin: None,
            editable: true,
        }
    }

    #[test]
    fn link_rules() {
        let mut f = field("url", false);
        f.url_prefixes = vec!["https://github.com/".into()];
        f.url_shorthand = true;
        f.https_only = true;
        assert_eq!(f.check("octocat"), Ok(Some("https://github.com/octocat".into())));
        assert_eq!(f.check("@octocat"), Ok(Some("https://github.com/octocat".into())));
        assert_eq!(f.check("HTTPS://GitHub.com/octocat"), Ok(Some("HTTPS://GitHub.com/octocat".into())));
        assert!(f.check("https://gitlab.com/octocat").is_err());
        assert!(f.check("http://github.com/octocat").is_err());
        assert!(f.check("https://github.com/").is_err());
        assert!(f.check("octo cat").is_err());
        let mut any = field("url", false);
        any.normalise().unwrap();
        assert!(any.check("https:///x").is_err());
        assert_eq!(any.check("http://example.com"), Ok(Some("http://example.com".into())));
    }

    #[test]
    fn definitions() {
        let mut f = field("url", false);
        f.url_prefixes = vec!["javascript:alert(1)".into()];
        assert!(f.normalise().is_err());
        let mut f = field("url", false);
        f.url_shorthand = true;
        assert!(f.normalise().is_err());
        let mut f = field("text", false);
        f.url_prefixes = vec!["https://x.example/".into()];
        f.normalise().unwrap();
        assert!(f.url_prefixes.is_empty());
    }

    #[test]
    fn field_validation() {
        assert_eq!(field("text", false).check("  "), Ok(None));
        assert!(field("text", true).check("").is_err());
        assert_eq!(field("text", false).check(" hi "), Ok(Some("hi".into())));
        assert!(field("url", false).check("javascript:x").is_err());
        assert!(field("select", false).check("Green").is_err());
        assert_eq!(field("select", false).check("Blue"), Ok(Some("Blue".into())));
        assert!(field("text", false).check(&"x".repeat(201)).is_err());
    }
}
