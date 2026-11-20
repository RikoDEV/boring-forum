//! The shared application state every handler receives: configuration, the database pool, settings,
//! caches, the rate limiter and the realtime hub.

use crate::http::ratelimit::RateLimiter;
use bf_core::config::Config;
use bf_core::settings::Settings;
use bf_web::microcache::Microcache;
use sqlx::PgPool;
use std::sync::{Arc, RwLock};
use std::time::Duration;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: Option<PgPool>,
    /// Dedicated connection for the Data Explorer, logging in as the restricted `bf_explorer`
    /// role (`BF_EXPLORER_DATABASE_URL`). `None` disables the Explorer.
    pub explorer_pool: Option<PgPool>,
    settings: Arc<RwLock<Arc<Settings>>>,
    pub microcache: Microcache,
    pub rate_limiter: RateLimiter,
    pub hub: bf_realtime::Hub,
    pub forum: crate::forum::tree::ForumCache,
    pub views: crate::forum::views::ViewCounter,
    /// Signed-in users by session digest, for a few seconds; emptied by any write.
    pub sessions: moka::sync::Cache<String, bf_db::accounts::users::User>,
    /// Rendered widgets with when they were made; each widget has its own lifetime.
    /// Rendered widgets by id and language (their empty states are translated).
    pub widgets: moka::sync::Cache<(i64, &'static str), (std::time::Instant, String)>,
    /// Who was on each page lately; see `browsing`.
    pub browsing: moka::sync::Cache<String, crate::community::browsing::Visitors>,
    /// Gravatar pictures by username: the image and its type, or None when there is none.
    pub gravatars: moka::sync::Cache<String, Option<(axum::body::Bytes, &'static str)>>,
    /// The widget list itself, briefly: every home page render needs it.
    pub widget_list: moka::sync::Cache<(), Arc<Vec<bf_db::admin::themes::Widget>>>,
}

impl AppState {
    /// The 404 page when the on/off setting `key` is off.
    pub(crate) fn require_feature(&self, key: &str) -> Result<(), bf_core::AppError> {
        if self.settings().bool(key) { Ok(()) } else { Err(bf_core::AppError::NotFound) }
    }

    /// The database pool, or the internal error when the server runs without one.
    pub(crate) fn db(&self) -> Result<&PgPool, bf_core::AppError> {
        self.pool.as_ref().ok_or_else(|| bf_core::AppError::internal("no database"))
    }

    pub async fn new(config: Config, pool: PgPool) -> Result<Self, sqlx::Error> {
        let settings = bf_db::settings::load(&pool).await?;
        let explorer_pool = match bf_db::explorer_pool().await {
            Ok(p) => p,
            // A bad explorer URL must not stop the forum from starting; the Explorer stays off.
            Err(e) => {
                tracing::error!(error = %e, "BF_EXPLORER_DATABASE_URL could not connect; Data Explorer disabled");
                None
            }
        };
        if let Some(ep) = &explorer_pool {
            match bf_db::admin::explorer::check_connection(ep).await {
                Ok(role) => tracing::info!(role = %role, "Data Explorer enabled on a confined connection"),
                Err(e) => {
                    tracing::error!(reason = %e, "Data Explorer disabled: its connection is not confined");
                    return Ok(Self::build_with(config, Some(pool), None, settings));
                }
            }
        }
        Ok(Self::build_with(config, Some(pool), explorer_pool, settings))
    }

    /// State without a database, for tests and the OpenAPI dump.
    pub fn build(config: Config, pool: Option<PgPool>, settings: Settings) -> Self {
        Self::build_with(config, pool, None, settings)
    }

    fn build_with(config: Config, pool: Option<PgPool>, explorer_pool: Option<PgPool>, settings: Settings) -> Self {
        Self {
            microcache: Microcache::new(bf_core::scaled(Duration::from_millis(config.microcache_ttl_ms))),
            rate_limiter: RateLimiter::new(config.rate_limit_per_min, Duration::from_secs(60)),
            hub: bf_realtime::Hub::default(),
            forum: crate::forum::tree::ForumCache::default(),
            views: crate::forum::views::ViewCounter::default(),
            widgets: moka::sync::Cache::builder().max_capacity(1000).build(),
            browsing: moka::sync::Cache::builder().max_capacity(20_000).time_to_idle(Duration::from_secs(1800)).build(),
            gravatars: moka::sync::Cache::builder()
                .max_capacity(5_000)
                .time_to_live(Duration::from_secs(6 * 3600))
                .build(),
            widget_list: moka::sync::Cache::builder().time_to_live(bf_core::scaled(Duration::from_secs(5))).build(),
            sessions: moka::sync::Cache::builder()
                .max_capacity(20_000)
                // `forget_sessions_of`: drop one member's cached sessions, all devices.
                .support_invalidation_closures()
                .time_to_live(bf_core::scaled(Duration::from_secs(5)))
                .build(),
            config: Arc::new(config),
            pool,
            explorer_pool,
            settings: Arc::new(RwLock::new(Arc::new(settings))),
        }
    }

    pub fn settings(&self) -> Arc<Settings> {
        let s = self.settings.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        bf_cook::set_bbcode(s.bool("bbcode_enabled"));
        bf_cook::set_math(s.bool("math_enabled"));
        s
    }

    pub fn replace_settings(&self, s: Settings) {
        *self.settings.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(s);
        self.microcache.clear();
    }

    pub async fn reload_settings(&self) {
        let Some(pool) = &self.pool else { return };
        match bf_db::settings::load(pool).await {
            Ok(s) => self.replace_settings(s),
            Err(e) => tracing::error!(error = %e, "settings reload failed"),
        }
    }

    /// Loads everything kept in memory from the database: themes, texts, announcements, reactions,
    /// translations, ads and the rest. At start-up, and again after a restore replaced the data.
    pub async fn reload_site_data(&self) {
        let Some(pool) = &self.pool else { return };
        if let Err(e) = crate::admin::themes::reload(self).await {
            tracing::warn!(error = %e, "theme load failed");
        }
        crate::community::legal::load_published(self).await;
        if let Err(e) = crate::moderation::actions::load_mod_actions(self).await {
            tracing::warn!(error = %e, "moderator action load failed");
        }
        if let Err(e) = crate::admin::announcements::load_announcements(self).await {
            tracing::warn!(error = %e, "announcement load failed");
        }
        if let Err(e) = crate::forum::reactions::load_reactions(self).await {
            tracing::warn!(error = %e, "reaction load failed");
        }
        crate::forum::translations::load(self).await;
        crate::admin::pages::load_paths(self).await;
        if let Err(e) = crate::platform::ads::reload(self).await {
            tracing::warn!(error = ?e, "ad load failed");
        }
        if let Err(e) = crate::moderation::report_reasons::load(self).await {
            tracing::warn!(error = %e, "report reason load failed");
        }
        if let Err(e) = crate::platform::plugins::reload(self).await {
            tracing::warn!(error = %e, "plugin load failed");
        }
        if let Err(e) = crate::admin::text_overrides::load_overrides(pool).await {
            tracing::warn!(error = %e, "text overrides load failed");
        }
        if let Err(e) = crate::admin::emoji::load_custom_emoji(pool).await {
            tracing::warn!(error = %e, "custom emoji load failed");
        }
    }

    pub fn spawn_background_tasks(&self) {
        let limiter = self.rate_limiter.clone();
        let hub = self.hub.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(60));
            loop {
                tick.tick().await;
                limiter.sweep();
                hub.sweep();
            }
        });

        if let Some(pool) = self.pool.clone() {
            crate::platform::mail::spawn(pool.clone());
            crate::platform::digest::spawn(self.clone());
            crate::admin::maintenance::spawn_session_sweeper(self.clone());
            crate::admin::maintenance::spawn_housekeeping(self.clone());
            crate::platform::webhooks::spawn_webhooks(self.clone());
            crate::platform::audit_sink::spawn(self.clone());
            let load_state = self.clone();
            tokio::spawn(async move { load_state.reload_site_data().await });
            let recook_state = self.clone();
            tokio::spawn(async move {
                // Plugins first, or this pass would drop what post.after_cook adds.
                if let Err(e) = crate::platform::plugins::reload(&recook_state).await {
                    tracing::warn!(error = %e, "plugin load failed");
                }
                recook(recook_state).await;
            });
            crate::platform::plugins::spawn_jobs(self.clone());
            crate::platform::push::spawn_worker(self.clone());
            crate::platform::imap_in::spawn(self.clone());
            crate::federation::spawn_worker(self.clone());
            crate::platform::builtin_plugins::register(self);
            let views = self.views.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(bf_core::scaled(Duration::from_secs(5)));
                loop {
                    tick.tick().await;
                    views.flush(&pool).await;
                }
            });
        }

        crate::platform::cluster::spawn(self);

        // Other nodes announce settings changes over LISTEN/NOTIFY. tools/devdb serves one
        // connection at a time, so a held listener connection would block every query.
        if std::env::var("BF_DEVDB").is_ok_and(|v| v == "1") {
            return;
        }
        let Some(pool) = self.pool.clone() else { return };
        let state = self.clone();
        tokio::spawn(async move {
            loop {
                match listen_settings(&pool, &state).await {
                    Ok(()) => {}
                    Err(e) => tracing::warn!(error = %e, "settings listener stopped; retrying"),
                }
                tokio::time::sleep(bf_core::scaled(Duration::from_secs(5))).await;
            }
        });
    }
}

async fn listen_settings(pool: &PgPool, state: &AppState) -> Result<(), sqlx::Error> {
    let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
    listener.listen("bf_settings").await?;
    // Changes made while we were disconnected are picked up by reloading once.
    state.reload_settings().await;
    loop {
        listener.recv().await?;
        state.reload_settings().await;
        // Another node may have changed the default theme or the site name.
        if let Err(e) = crate::admin::themes::reload(state).await {
            tracing::warn!(error = ?e, "themes not reloaded");
        }
        if let Err(e) = crate::platform::plugins::reload(state).await {
            tracing::warn!(error = ?e, "plugins not reloaded");
        }
    }
}

/// Re-cooks posts saved by an older cooker, a batch at a time, until none are left.
pub(crate) async fn recook(state: AppState) {
    let Some(pool) = state.pool.clone() else { return };
    let microcache = state.microcache.clone();
    loop {
        let batch = match bf_db::forum::posts::stale_cooked(&pool, bf_cook::COOK_VERSION, 200).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "re-cook query failed");
                tokio::time::sleep(Duration::from_secs(60)).await;
                continue;
            }
        };
        if batch.is_empty() {
            return;
        }
        let (ids, raws): (Vec<i64>, Vec<String>) = batch.into_iter().unzip();
        let cooked = tokio::task::spawn_blocking(move || raws.iter().map(|r| bf_cook::cook(r)).collect::<Vec<_>>())
            .await
            .unwrap_or_default();
        if cooked.len() != ids.len() {
            return;
        }
        let mut finished = Vec::with_capacity(cooked.len());
        for mut c in cooked {
            crate::platform::plugins::finish_callbacks(&state, &mut c).await;
            finished.push(crate::platform::plugins::after_cook(&state, c.html).await);
        }
        let html = finished;
        if let Err(e) = bf_db::forum::posts::save_cooked(&pool, &ids, &html, bf_cook::COOK_VERSION).await {
            tracing::warn!(error = %e, "re-cook save failed");
            return;
        }
        tracing::info!(posts = ids.len(), "re-cooked");
        microcache.clear();
    }
}
