//! What a slot's plugin is told about the thing it sits on: the member whose profile or card it
//! is (with their badges and public profile fields, including the ones plugins declare), the
//! topic, or the forum. Only what a guest could see, apart from members-only profiles that a
//! signed-in viewer may see.

use crate::state::AppState;
use bf_db::accounts::{fields, users};
use serde_json::{Value, json};

/// A member, by id. `None` when the profile is hidden from this viewer.
pub(crate) async fn user(state: &AppState, id: i64, viewer_signed_in: bool) -> Option<Value> {
    let pool = state.pool.as_ref()?;
    let uid = bf_core::ids::UserId::from(id);
    let p = users::profile(pool, uid).await.ok()??;
    match p.profile_visibility.as_str() {
        "nobody" => return None,
        "members" if !viewer_signed_in => return None,
        _ => {}
    }
    if !users::is_approved(pool, id).await.unwrap_or(false) {
        return None;
    }
    let badges: Vec<Value> = bf_db::community::badges::recent_badges(pool, id, 8)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(_, name, tier, icon, color, level, rare)| {
            json!({ "name": name, "tier": tier, "icon": icon, "color": color, "level": level, "rare": rare })
        })
        .collect();
    let defs = fields::fields(pool).await.unwrap_or_default();
    let answers = fields::values(pool, uid).await.unwrap_or_default();
    let shown: Vec<Value> = defs
        .iter()
        .filter(|f| f.public)
        .filter_map(|f| {
            let value =
                answers.iter().find(|(fid, _)| *fid == f.id).map(|(_, v)| v.clone()).filter(|v| !v.is_empty())?;
            Some(json!({ "label": f.label, "kind": f.kind, "value": value, "icon": f.icon, "plugin": f.plugin }))
        })
        .collect();
    Some(json!({
        "kind": "user", "id": id, "username": p.username, "display_name": p.display_name,
        "title": p.title, "group": p.group_name, "badges": badges, "fields": shown,
    }))
}

/// The thing a page is about, from its address: `/u/<name>`, `/t/<slug>/<id>`, `/c/<slug>/<id>`.
pub(crate) async fn of_page(state: &AppState, page: &str, viewer_signed_in: bool) -> Option<Value> {
    let pool = state.pool.as_ref()?;
    let mut parts = page.trim_start_matches('/').split('/');
    match (parts.next()?, parts.next(), parts.next()) {
        ("u", Some(name), _) => {
            let id = users::user_id_by_name(pool, name).await.ok()??;
            user(state, id.get(), viewer_signed_in).await
        }
        ("t", Some(_), Some(id)) => {
            let t = super::host::public_topic(state, id.parse().ok()?).await.ok()?;
            Some(json!({ "kind": "topic", "id": t.id, "title": t.title, "forum_id": t.node_id }))
        }
        ("c", Some(_), Some(id)) => {
            let id: i64 = id.parse().ok()?;
            let tree = state.forum.get(pool).await.ok()?;
            let guest = bf_core::perms::Actor { groups: vec![1.into()], ..Default::default() };
            let node = tree.node(id)?;
            tree.perms(&guest, id)
                .has(bf_core::perms::Perms::READ)
                .then(|| json!({ "kind": "forum", "id": id, "title": node.title, "slug": node.slug }))
        }
        _ => None,
    }
}
