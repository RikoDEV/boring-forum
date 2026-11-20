//! Who may fetch an uploaded file: those who may see a place it is used (a post, a private
//! message, a chat message), listed in `upload_uses`; for a post's attached files also those the
//! forum lets download. A file used nowhere (an avatar, an emoji, a
//! picture not yet posted) stays open, as does anything a guest could already read. A post in a
//! topic that is not visible counts as not visible, and a file left only in a post's edit history
//! is for its uploader and that forum's moderators.

use crate::http::render::DbResult;
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::http::{HeaderMap, header};
use bf_core::AppError;
use bf_core::perms::{Actor, Perms};
use bf_db::forum::uploads::{self, Upload};

/// Where a file with no current use is named in edit history (forum ids), by upload id, and
/// whether it was used before (`uploads.had_use`) and is no avatar, cover or emoji. Looking that up
/// reads the history's text, and avatars and emoji are fetched all the time, so the answer is kept
/// for two minutes.
static REVISION_HOMES: std::sync::LazyLock<moka::sync::Cache<i64, (Vec<i64>, bool)>> = std::sync::LazyLock::new(|| {
    moka::sync::Cache::builder().max_capacity(100_000).time_to_live(std::time::Duration::from_secs(120)).build()
});

pub(crate) struct Access {
    /// This viewer may have the file.
    pub allowed: bool,
    /// A guest may too, so caches may keep and share it.
    pub public: bool,
    /// The file is part of a post, a message or a chat: it can go away or change who may see it, so
    /// it is not cached "for good". An avatar, a cover or an emoji is not.
    pub in_content: bool,
}

pub(crate) async fn access(state: &AppState, v: &Viewer, up: &Upload) -> Result<Access, AppError> {
    let pool = state.db()?;
    let uses = uploads::uses(pool, up.id).await.db()?;
    if uses.is_empty() {
        // A file edited out of a post lives on in the post's history only: that is not "used
        // nowhere". It stays with its uploader and with those who can read that history.
        let (old, orphaned) = match REVISION_HOMES.get(&up.id) {
            Some(found) => found,
            None => {
                let nodes = uploads::revision_homes(pool, up.id, &up.storage_key).await.db()?;
                // Used by a post, message or chat before and by nothing now (its post was deleted
                // or its author erased): until the sweep removes it, only its uploader and admins
                // have it, not everyone with the address.
                let orphaned: bool = sqlx::query_scalar(
                    "SELECT u.had_use
                        AND NOT EXISTS (SELECT 1 FROM users x WHERE x.avatar_upload_id = u.id OR x.cover_upload_id = u.id)
                        AND NOT EXISTS (SELECT 1 FROM custom_emoji e WHERE e.url LIKE '%' || u.storage_key || '%')
                     FROM uploads u WHERE u.id = $1",
                )
                .bind(up.id)
                .fetch_optional(pool)
                .await
                .db()?
                .unwrap_or(false);
                REVISION_HOMES.insert(up.id, (nodes.clone(), orphaned));
                (nodes, orphaned)
            }
        };
        if old.is_empty() && !orphaned {
            return Ok(Access { allowed: true, public: true, in_content: false });
        }
        let Some(user) = &v.user else { return Ok(Access { allowed: false, public: false, in_content: true }) };
        let tree = state.forum.get(pool).await?;
        let allowed = up.user_id == Some(user.id.get())
            || v.actor.is_admin
            || old.iter().any(|node| tree.perms(&v.actor, *node).has(Perms::MODERATE));
        return Ok(Access { allowed, public: false, in_content: true });
    }
    let ids = |kind: &str| uses.iter().filter(|(k, _)| k == kind).map(|(_, id)| *id).collect::<Vec<_>>();
    let tree = state.forum.get(pool).await?;
    let homes = uploads::post_homes(pool, &ids("post")).await.db()?;
    let guest = Actor { groups: vec![1.into()], ..Default::default() };
    let members_only = state.settings().bool("uploads_members_only");
    // Opening an attached file (not looking at a picture in the post) needs the forum's download right.
    let needs_download = !up.mime.starts_with("image/");
    let may_open = |actor: &Actor, node: i64| {
        let p = tree.perms(actor, node);
        p.has(Perms::READ) && (!needs_download || p.has(Perms::DOWNLOAD) || p.has(Perms::MODERATE))
    };
    let public =
        !members_only && homes.iter().any(|(_, node, st)| st == "visible" && may_open(&guest, *node));
    if public {
        return Ok(Access { allowed: true, public: true, in_content: true });
    }
    let Some(user) = &v.user else { return Ok(Access { allowed: false, public: false, in_content: true }) };
    let me = user.id.get();
    if up.user_id == Some(me) || v.actor.is_admin {
        return Ok(Access { allowed: true, public: false, in_content: true });
    }
    // A post: readable, and visible (or the viewer moderates the forum).
    let post = homes.iter().any(|(_, node, st)| {
        may_open(&v.actor, *node) && (st == "visible" || tree.perms(&v.actor, *node).has(Perms::MODERATE))
    });
    if post {
        return Ok(Access { allowed: true, public: false, in_content: true });
    }
    for conversation in uploads::message_homes(pool, &ids("message")).await.db()? {
        if bf_db::community::messages::participant_of(pool, conversation, me).await.db()?.is_some() {
            return Ok(Access { allowed: true, public: false, in_content: true });
        }
    }
    for channel in uploads::chat_homes(pool, &ids("chat")).await.db()? {
        if crate::community::chat::viewable(state, v, channel).await.is_ok() {
            return Ok(Access { allowed: true, public: false, in_content: true });
        }
    }
    Ok(Access { allowed: false, public: false, in_content: true })
}

/// Is a request from another site's page (an `<img>`, a download link in a script, a hotlink)?
/// Browsers say so in `Sec-Fetch-Site`; older ones in `Referer`. A request with neither (a feed
/// reader, a link preview, another server) is not a page embedding the file, so it passes, and
/// so does someone following a link (a top-level navigation).
pub(crate) fn embedded_elsewhere(headers: &HeaderMap) -> bool {
    let get = |name: &str| headers.get(name).and_then(|h| h.to_str().ok());
    if let Some(site) = get("sec-fetch-site") {
        return site == "cross-site" && !matches!(get("sec-fetch-dest"), Some("document" | "iframe" | "frame"));
    }
    let (Some(referer), Some(host)) = (get("referer"), get(header::HOST.as_str())) else { return false };
    let from = referer.split_once("://").map_or(referer, |(_, rest)| rest).split(['/', '?', '#']).next().unwrap_or("");
    !from.eq_ignore_ascii_case(host)
}

/// Is the request a person following a link, as opposed to a page loading the file?
pub(crate) fn is_navigation(headers: &HeaderMap) -> bool {
    let get = |name: &str| headers.get(name).and_then(|h| h.to_str().ok());
    get("sec-fetch-mode") == Some("navigate") || matches!(get("sec-fetch-dest"), Some("document" | "iframe" | "frame"))
}

/// The expiry of a valid `?x=` signature on this request, if it has one.
pub(crate) fn signature_until(query: Option<&str>, sha: &str, now: i64) -> Option<i64> {
    let value = query?.split('&').find_map(|p| p.strip_prefix("x="))?;
    super::upload_links::verify(sha, value, now).then(|| value.split('.').next()?.parse().ok())?
}

/// The site's rule for other sites using its files (`hotlink_protection`). For the files it
/// covers: a page on another site may not load them, and someone not signed in needs a link the
/// site signed (they expire) unless they are following a link in the browser.
pub(crate) fn link_refused(covered: bool, headers: &HeaderMap, signed_in: bool, signed: bool) -> bool {
    covered && (embedded_elsewhere(headers) || (!signed_in && !signed && !is_navigation(headers)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn h(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.insert(*k, HeaderValue::from_static(v));
        }
        m
    }

    #[test]
    fn embedding_is_told_from_following_a_link() {
        assert!(embedded_elsewhere(&h(&[("sec-fetch-site", "cross-site"), ("sec-fetch-dest", "image")])));
        assert!(embedded_elsewhere(&h(&[("sec-fetch-site", "cross-site")])));
        assert!(!embedded_elsewhere(&h(&[("sec-fetch-site", "cross-site"), ("sec-fetch-dest", "document")])));
        assert!(!embedded_elsewhere(&h(&[("sec-fetch-site", "same-origin"), ("sec-fetch-dest", "image")])));
        assert!(!embedded_elsewhere(&h(&[("sec-fetch-site", "none")])));
        assert!(!embedded_elsewhere(&h(&[])));
        // Without Fetch Metadata the Referer decides.
        assert!(embedded_elsewhere(&h(&[("referer", "https://evil.example/page"), ("host", "forum.test")])));
        assert!(!embedded_elsewhere(&h(&[("referer", "https://forum.test/t/1"), ("host", "forum.test")])));
        assert!(!embedded_elsewhere(&h(&[("host", "forum.test")])));
    }
}
