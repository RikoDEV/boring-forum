//! Admin maintenance: offline mode, recount, re-cook, cache clearing,
//! settings export/import and the permission check tool.

use crate::admin::api::{Admin, AdminRead, AdminWrite};
use crate::http::render::DbResult;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::perms::{Actor, Perms, effective};
use bf_db::admin::maintenance;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/maintenance/{task}", post(run))
        .route("/api/v1/admin/settings-export", get(export))
        .route("/api/v1/admin/site-export", post(site_export))
        .route("/api/v1/admin/settings-import", post(import))
        .route("/api/v1/admin/permission-check", get(permission_check))
}

async fn run(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Path(task): Path<String>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let detail = match task.as_str() {
        "recount" => {
            bf_db::admin::maintenance::recount_all(pool).await.db()?;
            json!({ "recounted": true })
        }
        "recook" => {
            let n = bf_db::admin::maintenance::mark_all_for_recook(pool).await.db()?;
            // The re-cook job starts at boot and exits when done; start a pass now.
            tokio::spawn(crate::state::recook(state.clone()));
            json!({ "queued": n })
        }
        "rebuild-search" => {
            // The search columns are generated, so only the indexes can drift.
            let ddl = bf_db::ddl_pool(pool).await.db()?;
            for index in ["items_search", "posts_search"] {
                sqlx::query(sqlx::AssertSqlSafe(format!("REINDEX INDEX {index}"))).execute(&ddl).await.db()?;
            }
            json!({ "rebuilt": true })
        }
        "housekeeping" => crate::admin::maintenance::housekeeping(&state).await?,
        "clear-caches" => {
            state.forum.invalidate();
            state.microcache.clear();
            json!({ "cleared": true })
        }
        _ => return Err(AppError::NotFound),
    };
    crate::admin::api::audit_log(pool, Some(who.unwrap_or(0)), &format!("maintenance.{task}"), "site", detail.clone())
        .await?;
    state.forum.invalidate();
    state.microcache.clear();
    Ok(Json(detail).into_response())
}

/// The changed settings without the keys and secrets among them: an export is a file that gets
/// passed around, and those are entered again on the site that imports it.
fn exportable(state: &AppState) -> std::collections::BTreeMap<&'static str, Value> {
    let mut changed = state.settings().overrides();
    changed.retain(|key, _| !crate::admin::settings::is_secret(key));
    changed
}

/// Settings that differ from the defaults, as JSON (export). Keys and secrets are left out.
async fn export(State(state): State<AppState>, Admin(..): AdminRead) -> Result<Response, AppError> {
    Ok(Json(exportable(&state)).into_response())
}

/// Imports settings from an export. All values are validated before any is saved.
async fn import(
    State(state): State<AppState>,
    Admin(who, ..): AdminWrite,
    Json(mut body): Json<serde_json::Map<String, Value>>,
) -> Result<Response, AppError> {
    // An empty key or secret keeps the stored one, as on the Settings page.
    body.retain(|k, val| !(crate::admin::settings::is_secret(k) && val.as_str().is_some_and(str::is_empty)));
    let mut next = (*state.settings()).clone();
    for (k, val) in &body {
        next.set(k, val.clone()).map_err(|e| AppError::Validation(e.to_string()))?;
    }
    let pool = state.db()?;
    for (k, val) in &body {
        bf_db::settings::save(pool, k, val).await.db()?;
    }
    let keys = json!({ "keys": body.keys().collect::<Vec<_>>() });
    crate::admin::api::audit_log(pool, Some(who.unwrap_or(0)), "settings.import", "site", keys).await?;
    state.replace_settings(next);
    Ok(Json(json!({ "imported": body.len() })).into_response())
}

#[derive(Deserialize)]
struct CheckQuery {
    user: String,
    forum: i64,
}

/// "Why can/can't this member …": each group's grant, the result, and the
/// rules that changed it (admin, scoped moderator, silenced, hidden parent).
async fn permission_check(
    State(state): State<AppState>,
    Admin(..): AdminRead,
    Query(q): Query<CheckQuery>,
) -> Result<Response, AppError> {
    let pool = state.db()?;
    let u = bf_db::accounts::users::user_by_login(pool, q.user.trim()).await.db()?.ok_or(AppError::NotFound)?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(q.forum).ok_or(AppError::NotFound)?;
    let actor = Actor {
        user_id: Some(u.id),
        groups: u.groups.clone(),
        is_admin: u.is_admin,
        silenced: u.silenced || !u.approved,
        moderated_nodes: u.moderated_nodes.clone(),
        ..Default::default()
    };
    let own = tree.grants_of(q.forum).clone();
    let names = |p: Perms| -> Vec<&'static str> {
        [
            ("view", Perms::VIEW),
            ("read", Perms::READ),
            ("create", Perms::CREATE),
            ("reply", Perms::REPLY),
            ("attach", Perms::ATTACH),
            ("download", Perms::DOWNLOAD),
            ("moderate", Perms::MODERATE),
        ]
        .into_iter()
        .filter(|(_, b)| p.has(*b))
        .map(|(n, _)| n)
        .collect()
    };
    let by_group: Vec<Value> = u
        .groups
        .iter()
        .map(|g| json!({ "group": g, "grants": names(own.get(g).copied().unwrap_or(Perms::NONE)) }))
        .collect();
    let result = tree.perms(&actor, q.forum);
    let mut reasons = Vec::new();
    if u.is_admin {
        reasons.push("administrator: every permission");
    }
    if u.moderated_nodes.contains(&q.forum.into()) {
        reasons.push("scoped moderator of this forum");
    }
    if u.silenced {
        reasons.push("silenced: read-only");
    }
    if !u.approved {
        reasons.push("awaiting account approval: read-only");
    }
    let raw = effective(&actor, q.forum.into(), &own);
    if raw.has(Perms::VIEW) && !result.has(Perms::VIEW) {
        reasons.push("a parent category is hidden from this member");
    }
    Ok(Json(json!({
        "user": u.username, "forum": node.title, "groups": by_group, "result": names(result), "reasons": reasons,
    }))
    .into_response())
}

/// Fills in the confusable skeleton of new and renamed accounts, a batch per call.
pub async fn backfill_skeletons(pool: &sqlx::PgPool) {
    let mut after = 0i64;
    for _ in 0..20 {
        let rows = match bf_db::accounts::users::names_without_skeleton(pool, after, 500).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "skeleton backfill failed");
                return;
            }
        };
        if rows.is_empty() {
            return;
        }
        for (id, name) in rows {
            after = id;
            if let Err(e) =
                bf_db::accounts::users::set_skeleton(pool, id.into(), &bf_core::auth::username_skeleton(&name)).await
            {
                // A look-alike of an older account (the unique index refuses it): leave it unset
                // and go on, instead of stopping on it every time.
                tracing::warn!(error = %e, user = id, "skeleton update failed");
            }
        }
    }
}

/// Background: ends idle sessions every 10 minutes when `session_idle_minutes` is set.
pub fn spawn_session_sweeper(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(
            std::env::var("BF_TICK_SECONDS").ok().and_then(|s| s.parse().ok()).filter(|s| *s > 0).unwrap_or(600),
        ));
        loop {
            tick.tick().await;
            let Some(pool) = &state.pool else { return };
            // One node at a time runs the tick: a session lock on a connection held for its length.
            let mut lock_conn = match pool.acquire().await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "maintenance tick: no connection");
                    continue;
                }
            };
            match sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
                .bind(MAINTENANCE_LOCK)
                .fetch_one(&mut *lock_conn)
                .await
            {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, "maintenance tick: lock failed");
                    continue;
                }
            }
            backfill_skeletons(pool).await;
            let s = state.settings();
            let levels = s.trust_levels();
            match bf_db::accounts::users::demote_inactive(
                pool,
                (0..=levels.max_level())
                    .map(|n| levels.get(n).map_or(0, |l| if l.auto { l.recent_posts } else { 0 }))
                    .collect(),
                s.int("trust_recent_days") as i32,
                levels.max_auto(),
            )
            .await
            {
                Ok(lowered) => {
                    for (user, level) in lowered {
                        crate::community::notifications::trust_lowered(&state, user, level).await;
                        crate::platform::webhooks::fire(
                            &state,
                            "trust.changed",
                            serde_json::json!({ "user_id": user, "to": level, "by": "auto" }),
                        );
                    }
                }
                Err(e) => tracing::warn!(error = %e, "trust demotion failed"),
            }
            // During a restore or maintenance window only housekeeping runs; nothing that mails,
            // posts or calls out.
            let offline = state.settings().bool("offline");
            if !offline {
                crate::admin::bulk_mail::resume_stalled(&state).await;
            }
            let _ = bf_db::platform::mail::fail_stale_mass_messages(pool).await;
            if !offline {
                crate::community::trust::run_promotion(&state).await;
            }
            if !offline {
                crate::community::notifications::run_tick(&state).await;
            }
            crate::community::chat::run_retention(&state).await;
            if !offline {
                crate::platform::plugins::dispatch(&state, crate::platform::plugins::SCHEDULED, &serde_json::json!({}));
                crate::forum::bookmarks::send_reminders(&state).await;
            }
            if !offline {
                crate::community::badges::run_awards(&state).await;
            }
            crate::forum::topic_tools::run_timers(&state).await;
            if !offline {
                crate::platform::automation::run_scheduled(&state).await;
                crate::ai::run_tick(&state).await;
            }
            crate::platform::ads::run_tick(&state).await;
            if !offline {
                crate::community::referrals::run_tick(&state).await;
            }
            if !offline {
                crate::platform::newsletter::run_tick(&state).await;
            }
            crate::admin::backups::run_tick(&state).await;
            crate::account::lifecycle::run_deletions(&state).await;
            if !offline {
                match bf_db::community::promotion::run_rules(pool).await {
                    Ok(joined) => {
                        for (user, group) in joined {
                            crate::community::notifications::added_to_group(&state, user, group).await;
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "promotion rules failed"),
                }
            }
            if let Err(e) = bf_db::forum::topics::expire_pins(pool).await {
                tracing::warn!(error = %e, "pin expiry failed");
            }
            let minutes = state.settings().int("session_idle_minutes");
            if let Err(e) = bf_db::accounts::sessions::expire_idle_sessions(pool, minutes).await {
                tracing::warn!(error = %e, "session sweep failed");
            }
            let _ = sqlx::query("SELECT pg_advisory_unlock($1)").bind(MAINTENANCE_LOCK).execute(&mut *lock_conn).await;
        }
    });
}

/// Advisory-lock key of the maintenance tick (shared by every node on the database).
const MAINTENANCE_LOCK: i64 = 0x0062_666d_6169_6e74;

/// Daily housekeeping: revision retention and orphaned uploads, each logged.
pub fn spawn_housekeeping(state: AppState) {
    tokio::spawn(async move {
        // Not during startup, but not a whole day later either: a server restarted daily would
        // otherwise never get here. Every step is safe to repeat.
        tokio::time::sleep(Duration::from_secs(600)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(86_400));
        loop {
            tick.tick().await;
            if let Err(e) = housekeeping(&state).await {
                tracing::warn!(error = %e, "housekeeping failed");
            }
        }
    });
}

pub(crate) async fn housekeeping(state: &AppState) -> Result<serde_json::Value, AppError> {
    let pool = state.db()?;
    let days = state.settings().int("edit_history_retention_days");
    let revisions = if days > 0 { maintenance::purge_revisions(pool, days).await.db()? } else { 0 };
    let mut files = 0;
    // One step failing must not stop the retention steps after it.
    match maintenance::remove_orphan_uploads(pool).await {
        Ok(keys) => {
            for key in keys {
                if !maintenance::upload_key_used(pool, &key).await.db()? {
                    files += crate::forum::uploads::delete_files(&key).await;
                }
            }
        }
        Err(e) => tracing::warn!(error = %e, "orphan upload sweep failed"),
    }
    let operational_days = state.settings().int("operational_retention_days");
    let notification_days = state.settings().int("notification_retention_days");
    if let Err(e) = maintenance::purge_operational(pool, operational_days, notification_days).await {
        tracing::warn!(error = %e, "operational table cleanup failed");
    }
    crate::ai::housekeeping(state).await;
    // Keys of deleted members' federated actors, once their Delete activity has gone out.
    if let Err(e) = bf_db::accounts::lifecycle::purge_orphan_actor_keys(pool).await {
        tracing::warn!(error = %e, "federation key cleanup failed");
    }
    let deleted_days = state.settings().int("deleted_retention_days");
    let purged = if deleted_days > 0 { bf_db::forum::posts::purge_deleted(pool, deleted_days).await.db()? } else { 0 };
    // Audit IPs age out on their own horizon, ahead of the events themselves.
    let ip_days = state.settings().int("ip_retention_days");
    let ips_cleared = if ip_days > 0 { bf_db::admin::audit::clear_old_ips(pool, ip_days).await.db()? } else { 0 };
    // The same horizon for the addresses kept beside posts, sessions and failed sign-ins.
    let request_ips_cleared = if ip_days > 0 {
        match maintenance::clear_old_request_ips(pool, ip_days).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "request IP cleanup failed");
                0
            }
        }
    } else {
        0
    };
    let audit_days = state.settings().int("audit_retention_days");
    let audit_pruned = if audit_days > 0 { bf_db::admin::audit::prune(pool, audit_days).await.db()? } else { 0 };
    let detail = json!({
        "revisions_purged": revisions,
        "files_deleted": files,
        "deleted_purged": purged,
        "audit_ips_cleared": ips_cleared,
        "request_ips_cleared": request_ips_cleared,
        "audit_pruned": audit_pruned,
    });
    maintenance::log_task(pool, "housekeeping", detail.clone()).await.db()?;
    Ok(detail)
}

/// What the site export holds, one section after another: the table name in the file, then its
/// query. The posts and topics are in the neutral format `boring-forum import` reads, so an export
/// can be loaded into a fresh site. Password hashes, sessions, keys and 2FA secrets are never in it.
const EXPORT_SECTIONS: [(&str, &str); 5] = [
    ("groups", "SELECT id, name FROM groups ORDER BY id"),
    ("users", "SELECT id, username, email, primary_group_id AS group_id, joined_at FROM users ORDER BY id"),
    ("forums", "SELECT id, parent_id, title, kind, slug, position FROM nodes ORDER BY id"),
    (
        "topics",
        "SELECT id, node_id AS forum_id, author_id, title, slug, state::text AS state, created_at,
                '/t/' || slug || '/' || id AS url FROM items ORDER BY id",
    ),
    (
        "posts",
        "SELECT id, item_id AS topic_id, post_number, author_id, raw AS body, 'markdown' AS format,
                state::text AS state, created_at FROM posts WHERE action IS NULL ORDER BY item_id, post_number",
    ),
];

/// Writes the export into a channel, a row at a time, so the site never holds it in memory. A
/// failure ends the stream with an error, which cuts the response off instead of closing the JSON.
async fn write_export(pool: sqlx::PgPool, settings: Value, tx: tokio::sync::mpsc::Sender<std::io::Result<Vec<u8>>>) {
    use futures_util::StreamExt;
    let send = |bytes: Vec<u8>| {
        let tx = tx.clone();
        async move { tx.send(Ok(bytes)).await.is_ok() }
    };
    let head = format!(
        "{{\"format\":\"boring-forum-export\",\"version\":1,\"exported_at\":{},\"settings\":{}",
        json!(chrono::Utc::now().to_rfc3339()),
        settings
    );
    if !send(head.into_bytes()).await {
        return;
    }
    for (name, sql) in EXPORT_SECTIONS {
        if !send(format!(",\"{name}\":[").into_bytes()).await {
            return;
        }
        let wrapped = format!("SELECT row_to_json(t) FROM ({sql}) t");
        let mut rows = sqlx::query_scalar::<_, Value>(sqlx::AssertSqlSafe(wrapped)).fetch(&pool);
        let mut first = true;
        let mut batch = Vec::with_capacity(64 * 1024);
        while let Some(row) = rows.next().await {
            match row {
                Ok(row) => {
                    if !first {
                        batch.push(b',');
                    }
                    first = false;
                    batch.extend_from_slice(row.to_string().as_bytes());
                    if batch.len() >= 64 * 1024 && !send(std::mem::take(&mut batch)).await {
                        return;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, section = name, "site export failed");
                    let _ = tx.send(Err(std::io::Error::other("export failed"))).await;
                    return;
                }
            }
        }
        batch.push(b']');
        if !send(batch).await {
            return;
        }
    }
    let _ = send(b"}".to_vec()).await;
}

/// Downloads the whole site (members, forums, topics, posts, groups and the changed settings) as
/// one JSON file, streamed. An administrator's session with a recent sudo and the CSRF token, or
/// an admin key; recorded in the audit log.
#[utoipa::path(post, path = "/api/v1/admin/site-export", tag = "admin", security(("admin_token" = [])),
    responses((status = 200, content_type = "application/json", body = Object, description = "boring-forum-export: settings, groups, users, forums, topics, posts"),
        (status = 403, body = bf_core::error::Problem)))]
pub(crate) async fn site_export(
    State(state): State<AppState>,
    // A write check: sessions need the CSRF token and a recent sudo, since this is every member's
    // address and every post.
    Admin(who, ..): AdminWrite,
) -> Result<Response, AppError> {
    let pool = state.db()?.clone();
    crate::admin::api::audit_log(&pool, Some(who.unwrap_or(0)), "site.export", "site", json!({})).await?;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(write_export(pool, json!(exportable(&state)), tx));
    let body = axum::body::Body::from_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }));
    let name = format!("boring-forum-export-{}.json", chrono::Utc::now().format("%Y%m%d"));
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, "application/json".to_owned()),
            (axum::http::header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"")),
            (axum::http::header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        body,
    )
        .into_response())
}
