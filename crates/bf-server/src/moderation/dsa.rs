//! EU Digital Services Act.
//!
//! * Art. 16: anyone can send a notice about illegal content at `/legal/notice`, gets an
//!   acknowledgement, and is told the decision.
//! * Art. 17: every restriction of a member's content or account by staff (or by the report
//!   threshold) records a statement of reasons, which the member is pointed to.
//! * Art. 20: the member can appeal from the statement; the appeal is decided by someone other
//!   than the original moderator, and a reversal undoes the restriction.
//! * Art. 15/24: `/transparency` reports the counts per half-year.

use crate::http::i18n::{tr, trf};
use crate::http::render::{DbResult, layout, page};
use crate::http::viewer::{Viewer, is_staff};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use bf_core::AppError;
use bf_core::i18n::en;
use bf_db::moderation::dsa;
use bf_web::pages::{
    AppealView, DsaNoticePage, DsaReviewPage, NoticeView, StatementPage, StatementView, TransparencyPage,
    TransparencyView, render,
};
use chrono::{Datelike, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/legal/notice", get(notice_form).post(send_notice))
        .route("/statements/{id}", get(statement_page))
        .route("/statements/{id}/appeal", post(appeal))
        .route("/review/dsa", get(review))
        .route("/review/dsa/notice/{id}", post(decide_notice))
        .route("/review/dsa/appeal/{id}", post(decide_appeal))
        .route("/transparency", get(transparency))
}

/// The statement categories of the EU DSA Transparency Database, shortened.
pub const CATEGORIES: &[(&str, &str)] = &[
    ("illegal_or_harmful_speech", en("Illegal or harmful speech (hate speech, defamation)")),
    ("violence", en("Violence or incitement")),
    ("protection_of_minors", en("Protection of minors, including child sexual abuse material")),
    ("scams_and_fraud", en("Scams and fraud")),
    ("intellectual_property", en("Intellectual property infringement")),
    ("data_protection_and_privacy", en("Data protection and privacy violations")),
    ("non_consensual_behaviour", en("Non-consensual behaviour (e.g. intimate images shared without consent)")),
    ("pornography_or_sexualized_content", en("Pornography or sexualised content")),
    ("self_harm", en("Self-harm")),
    ("unsafe_products", en("Unsafe or illegal products")),
    ("risk_for_public_security", en("Risk to public security (e.g. terrorist content)")),
    ("negative_effects_on_civic_discourse", en("Negative effects on civic discourse or elections")),
    ("animal_welfare", en("Animal welfare")),
    ("scope_of_platform_service", en("Outside the scope of this site (terms of service)")),
    ("other", en("Something else")),
];

fn category_label(key: &str) -> String {
    tr(CATEGORIES.iter().find(|(k, _)| *k == key).map_or(en("Something else"), |(_, l)| l))
}

/// What a moderation action restricts; the statement records and explains it.
pub(crate) struct Restriction<'a> {
    pub user_id: i64,
    pub target_type: &'a str,
    pub target_id: i64,
    pub action: &'a str,
    pub ground: &'a str,
    pub category: &'a str,
    pub facts: &'a str,
    pub automated_detection: bool,
    pub automated_decision: bool,
    pub notice_id: Option<i64>,
    pub moderator_id: Option<i64>,
}

/// Records an Art. 17 statement and points the member to it. A failure is logged, never fatal:
/// the moderation action itself has already happened.
pub(crate) async fn record(state: &AppState, r: Restriction<'_>) -> Option<i64> {
    let (user_id, moderator_id, target_type) = (r.user_id, r.moderator_id, r.target_type);
    let id = record_silent(state, r).await?;
    let what = match target_type {
        "account" => "your account".to_owned(),
        _ => "one of your posts".to_owned(),
    };
    let data = json!({ "title": what, "url": format!("/statements/{id}") });
    crate::community::notifications::send(state, moderator_id.unwrap_or(0), &[user_id], "statement", &data).await;
    Some(id)
}

/// Records the statement without a notification of its own, for callers that already tell the
/// member (their message then links to `/statements/{id}`).
pub(crate) async fn record_silent(state: &AppState, r: Restriction<'_>) -> Option<i64> {
    let pool = state.pool.as_ref()?;
    let facts: String = r.facts.trim().chars().take(2000).collect();
    let facts = if facts.is_empty() { "No further details were given.".to_owned() } else { facts };
    let id = dsa::insert_statement(
        pool,
        &dsa::NewStatement {
            user_id: r.user_id,
            target_type: r.target_type,
            target_id: r.target_id,
            action: r.action,
            ground: r.ground,
            category: r.category,
            facts: &facts,
            automated_detection: r.automated_detection,
            automated_decision: r.automated_decision,
            notice_id: r.notice_id,
            moderator_id: r.moderator_id,
        },
    )
    .await;
    match id {
        Ok(id) => {
            metrics::counter!("bf_dsa_statements_total", "action" => r.action.to_owned()).increment(1);
            Some(id)
        }
        Err(e) => {
            tracing::error!(error = %e, "statement of reasons not recorded");
            None
        }
    }
}

fn altcha_ready(state: &AppState) -> bool {
    state.settings().str("captcha_provider") == "altcha" && std::env::var("BF_CAPTCHA_SECRET").is_ok()
}

fn notice_page(state: &AppState, v: &Viewer, url: String, error: Option<String>, done: Option<String>) -> Response {
    let (mut l, _) = layout(state, v, Some(&v.t().msg("Report illegal content")));
    // Guests may send notices too, so they get a CSRF token like members do.
    let (token, set) = crate::http::viewer::csrf_for(v, true);
    l.csrf = token;
    l.noindex = true;
    let body = DsaNoticePage {
        layout: l,
        categories: CATEGORIES
            .iter()
            .filter(|(k, _)| *k != "scope_of_platform_service")
            .map(|(k, l)| (*k, tr(l)))
            .collect(),
        url,
        error,
        done,
        altcha: altcha_ready(state),
        contact: state.settings().str("dsa_contact_email").to_owned(),
    };
    page(render(&body), set, false)
}

#[derive(Deserialize)]
struct NoticeQuery {
    #[serde(default)]
    url: String,
}

async fn notice_form(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<NoticeQuery>,
) -> Result<Response, AppError> {
    state.require_feature("dsa_notices_enabled")?;
    Ok(notice_page(&state, &v, q.url.chars().take(2000).collect(), None, None))
}

#[derive(Deserialize)]
struct NoticeForm {
    csrf: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    explanation: String,
    #[serde(default)]
    legal_ground: String,
    #[serde(default)]
    notifier_name: String,
    #[serde(default)]
    notifier_email: String,
    #[serde(default)]
    good_faith: String,
    #[serde(default)]
    website_url: String,
    #[serde(flatten)]
    extra: HashMap<String, String>,
}

/// The post a URL on this site points at, if any.
async fn post_of(state: &AppState, url: &str) -> Option<i64> {
    let pool = state.pool.as_ref()?;
    let base = state.config.base_url.trim_end_matches('/');
    let path = url.strip_prefix(base)?;
    if let Some(id) = path.strip_prefix("/p/").and_then(|r| r.split(['/', '?', '#']).next()?.parse::<i64>().ok()) {
        return Some(id);
    }
    let topic = crate::forum::onebox::internal_topic(url, base)?;
    let number = url.split_once("#post-").and_then(|(_, n)| n.parse::<i32>().ok()).unwrap_or(1);
    dsa::post_for_topic(pool, topic, number).await.ok().flatten()
}

/// A notice's address as a mail may repeat it: up to its first space, and no longer than a link is.
fn mail_url(url: &str) -> String {
    url.split_whitespace().next().unwrap_or_default().chars().take(300).collect()
}

/// Public notices per address and hour: the form works without a challenge when none is configured.
static NOTICE_BY_IP: std::sync::LazyLock<crate::http::ratelimit::RateLimiter> =
    std::sync::LazyLock::new(|| crate::http::ratelimit::RateLimiter::new(10, std::time::Duration::from_secs(3600)));

async fn send_notice(
    State(state): State<AppState>,
    v: Viewer,
    Form(f): Form<NoticeForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    state.require_feature("dsa_notices_enabled")?;
    let url: String = f.url.trim().chars().take(2000).collect();
    let fail = |m: &str| Ok(notice_page(&state, &v, url.clone(), Some(m.to_owned()), None));
    // Bots fill in the hidden field; they are told it worked and nothing is stored.
    if !f.website_url.is_empty() {
        return Ok(notice_page(
            &state,
            &v,
            String::new(),
            None,
            Some(v.t().msg("Thank you. Your notice was received.")),
        ));
    }
    if let Some(ip) = v.ip
        && NOTICE_BY_IP.check(ip, std::time::Instant::now()).is_err()
    {
        return fail(&tr("You have sent several notices recently. Please try again later."));
    }
    if altcha_ready(&state) {
        let pool = state.db()?;
        let ok = match (f.extra.get("altcha"), std::env::var("BF_CAPTCHA_SECRET")) {
            (Some(payload), Ok(secret)) => {
                match crate::auth::altcha::verify(&secret, payload, Utc::now().timestamp()) {
                    Some((sig, exp)) => {
                        bf_db::accounts::challenges::spend_captcha(pool, &sig, exp).await.unwrap_or(false)
                    }
                    None => false,
                }
            }
            _ => false,
        };
        if !ok {
            return fail(&tr("Please complete the challenge and try again."));
        }
    }
    let base = state.config.base_url.trim_end_matches('/');
    if !url.starts_with(&format!("{base}/")) {
        return fail(&trf(
            "Give the full address of the content on this site, starting with {base}/.",
            &[("base", base)],
        ));
    }
    let Some(category) =
        CATEGORIES.iter().map(|(k, _)| *k).find(|k| *k == f.category && *k != "scope_of_platform_service")
    else {
        return fail(&tr("Choose what kind of illegal content it is."));
    };
    let explanation: String = f.explanation.trim().chars().take(5000).collect();
    if explanation.chars().count() < 10 {
        return fail(&tr("Explain why the content is illegal."));
    }
    if f.good_faith != "1" {
        return fail(&tr("Confirm that you send this notice in good faith."));
    }
    let name: String = f.notifier_name.trim().chars().take(200).collect();
    let email: String = f.notifier_email.trim().chars().take(254).collect();
    // Art. 16(2)(c): name and email are required, except for child sexual abuse material.
    if category != "protection_of_minors" && (name.is_empty() || email.is_empty()) {
        return fail(&tr("Give your name and email address."));
    }
    if !email.is_empty() && !email.split_once('@').is_some_and(|(a, d)| !a.is_empty() && d.contains('.')) {
        return fail(&tr("Enter a valid email address."));
    }
    let pool = state.db()?;
    // Each notice mails the address given and alerts every reviewer: a few a day per address.
    if !email.is_empty() && dsa::notices_from_email_today(pool, &email).await.db()? >= 5 {
        return fail(&tr("You have sent several notices recently. Please try again later."));
    }
    let post_id = post_of(&state, &url).await;
    let legal: String = f.legal_ground.trim().chars().take(300).collect();
    let id = dsa::insert_notice(
        pool,
        &dsa::NewNotice {
            url: &url,
            post_id,
            category,
            explanation: &explanation,
            legal_ground: &legal,
            notifier_name: &name,
            notifier_email: &email,
            reporter_id: (v.user.as_ref().map(|u| u.id)).map(bf_core::ids::UserId::get),
            good_faith: true,
        },
    )
    .await
    .db()?;
    metrics::counter!("bf_dsa_notices_total").increment(1);
    // Art. 16(4): an acknowledgement of receipt.
    let site = state.settings().str("site_name").to_owned();
    if !email.is_empty() {
        let n = id.to_string();
        // The address given is anyone's, so the mail carries no text of the sender's choosing:
        // the link to the post when the address named one, otherwise the address up to its first
        // space.
        let shown = match post_id {
            Some(p) => format!("{base}/p/{p}"),
            None => mail_url(&url),
        };
        let body = v.t().msgf(
            "We received your notice (number {id}) about:\n\n{url}\n\nWe will review it and tell you our decision.\n\n— {site}\n",
            &[("id", &n), ("url", &shown), ("site", &site)],
        );
        if let Err(e) = bf_db::platform::mail::queue_mail(
            pool,
            &email,
            &v.t().msgf("Your notice {id} - {site}", &[("id", &n), ("site", &site)]),
            &body,
        )
        .await
        {
            tracing::warn!(error = %e, "notice acknowledgement not queued");
        }
    }
    // Staff who review: the forum's moderators when the post is known, otherwise everyone.
    let node = match post_id {
        Some(p) => bf_db::forum::posts::post_ref(pool, p).await.ok().flatten().map_or(0, |p| p.node_id),
        None => 0,
    };
    crate::community::notifications::tell_reviewers(&state, node, 0).await;
    let n = id.to_string();
    let done = if email.is_empty() {
        trf("Thank you. Your notice was received as number {id}.", &[("id", &n)])
    } else {
        trf(
            "Thank you. Your notice was received as number {id}; we have sent a confirmation to {email}.",
            &[("id", &n), ("email", &email)],
        )
    };
    Ok(notice_page(&state, &v, String::new(), None, Some(done)))
}

/// Short names for the report tables and the review queue.
fn measure_label(action: &str) -> String {
    match action {
        "hidden" => tr("Content hidden"),
        "deleted" => tr("Content removed"),
        "suspended" => tr("Account suspended"),
        "silenced" => tr("Account made read-only"),
        "removed_from_federation" => tr("No longer federated"),
        _ => tr("Other restriction"),
    }
}

fn ground_label(ground: &str) -> String {
    if ground == "illegal" { tr("Illegal content") } else { tr("Terms of service") }
}

fn action_text(action: &str) -> String {
    match action {
        "hidden" => tr("The content was hidden from other members."),
        "deleted" => tr("The content was removed."),
        "suspended" => tr("The account was suspended."),
        "silenced" => tr("The account was restricted to reading."),
        "removed_from_federation" => tr("The content is no longer shared with other sites."),
        _ => tr("The content or account was restricted."),
    }
}

async fn statement_page(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let user = v.require_user()?;
    let pool = state.db()?;
    let s = dsa::statement(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    // The affected member and staff only; anyone else gets the same answer as a missing one.
    if s.user_id != Some(user.id.get()) && !is_staff(&v) {
        return Err(AppError::NotFound);
    }
    let (target_text, target_link) = match s.target_type.as_str() {
        "account" => (tr("Your account"), Some("/settings".to_owned())),
        "topic" => (tr("A topic"), Some(format!("/t/-/{}", s.target_id))),
        _ => (tr("A post"), Some(format!("/p/{}", s.target_id))),
    };
    let can_appeal = s.user_id == Some(user.id.get())
        && s.appealed_at.is_none()
        && Utc::now() - s.created_at < chrono::Duration::days(182);
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Statement of reasons")));
    l.noindex = true;
    let body = StatementPage {
        layout: l,
        created: bf_web::ago::Ago::stamp(s.created_at),
        action_text: action_text(&s.action),
        target_text,
        target_link,
        category_text: category_label(&s.category),
        can_appeal,
        s: StatementView {
            id: s.id,
            ground: s.ground,
            facts: s.facts,
            automated_detection: s.automated_detection,
            automated_decision: s.automated_decision,
            notice_id: s.notice_id,
            appealed_at: s.appealed_at,
            appeal_outcome: s.appeal_outcome,
        },
    };
    Ok(page(render(&body), set, false))
}

#[derive(Deserialize)]
struct AppealForm {
    csrf: String,
    text: String,
}

async fn appeal(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AppealForm>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    let text: String = f.text.trim().chars().take(5000).collect();
    if text.is_empty() {
        return Err(AppError::Validation("Say why the decision should change.".into()));
    }
    let pool = state.db()?;
    if !dsa::appeal(pool, id, user.id.get(), &text).await.db()? {
        return Err(AppError::NotFound);
    }
    crate::community::notifications::tell_reviewers(&state, 0, user.id.get()).await;
    Ok(Redirect::to(&format!("/statements/{id}")).into_response())
}

async fn review(State(state): State<AppState>, v: Viewer) -> Result<Response, AppError> {
    let user = v.require_user()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let pool = state.db()?;
    let notices = dsa::open_notices(pool, 100).await.db()?;
    let appeals = dsa::pending_appeals(pool, 100).await.db()?;
    let (mut l, set) = layout(&state, &v, Some(&v.t().msg("Legal notices and appeals")));
    l.noindex = true;
    let body = DsaReviewPage {
        layout: l,
        me: Some(user.id.get()),
        notices: notices
            .into_iter()
            .map(|n| NoticeView {
                id: n.id,
                url: n.url,
                category: category_label(&n.category),
                explanation: n.explanation,
                legal_ground: n.legal_ground,
                notifier_name: n.notifier_name,
                notifier_email: n.notifier_email,
                created_at: bf_web::ago::Ago::stamp(n.created_at),
            })
            .collect(),
        appeals: appeals
            .into_iter()
            .map(|a| AppealView {
                id: a.id,
                action: measure_label(&a.action),
                target_type: a.target_type,
                username: a.username,
                category: category_label(&a.category),
                facts: a.facts,
                appeal_text: a.appeal_text,
                moderator_id: a.moderator_id,
            })
            .collect(),
    };
    Ok(page(render(&body), set, false))
}

#[derive(Deserialize)]
struct NoticeDecision {
    csrf: String,
    decision: String,
    reason: String,
}

async fn decide_notice(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<NoticeDecision>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    let reason: String = f.reason.trim().chars().take(2000).collect();
    if reason.is_empty() {
        return Err(AppError::Validation("Give the reasons for the decision.".into()));
    }
    let pool = state.db()?;
    let n = dsa::notice(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    let (status, ground) = match f.decision.as_str() {
        "remove" => ("actioned", "illegal"),
        "terms" => ("actioned", "terms"),
        "reject" => ("rejected", ""),
        _ => return Err(AppError::Validation("Unknown decision.".into())),
    };
    if !dsa::decide_notice(pool, id, status, &reason, user.id.get()).await.db()? {
        return Err(AppError::Validation("That notice was already decided.".into()));
    }
    if status == "actioned"
        && let Some(post) = n.post_id
        && let Some(p) = bf_db::forum::posts::post_ref(pool, post).await.db()?
    {
        if p.state == "visible" {
            bf_db::forum::posts::set_hidden(pool, post, true, Some("Hidden after a legal notice"), user.id.get())
                .await
                .db()?;
            crate::moderation::topics::refresh(&state);
        }
        if let Some(author) = p.author_id {
            let category = if ground == "terms" { "scope_of_platform_service" } else { n.category.as_str() };
            record(
                &state,
                Restriction {
                    user_id: author,
                    target_type: "post",
                    target_id: post,
                    action: "hidden",
                    ground,
                    category,
                    facts: &reason,
                    automated_detection: false,
                    automated_decision: false,
                    notice_id: Some(id),
                    moderator_id: Some(user.id.get()),
                },
            )
            .await;
        }
    }
    // Art. 16(5): the notifier is told the decision and the means of redress.
    if !n.notifier_email.is_empty() {
        let site = state.settings().str("site_name").to_owned();
        let outcome = if status == "actioned" { "We have acted on it." } else { "We have decided not to act on it." };
        let body = format!(
            "We have reviewed your notice {id} about:\n\n{}\n\n{outcome}\n\nReasons: {reason}\n\nIf you disagree, you may turn to a certified out-of-court dispute settlement body or to the courts.\n\n— {site}\n",
            mail_url(&n.url)
        );
        if let Err(e) = bf_db::platform::mail::queue_mail(
            pool,
            &n.notifier_email,
            &format!("Decision on your notice {id} - {site}"),
            &body,
        )
        .await
        {
            tracing::warn!(error = %e, "notice decision not queued");
        }
    }
    bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("dsa.notice").by(user.id.get()).on(format!("notice:{id}")).details(json!({ "decision": f.decision })),
    )
    .await
    .db()?;
    Ok(Redirect::to("/review/dsa").into_response())
}

#[derive(Deserialize)]
struct AppealDecision {
    csrf: String,
    outcome: String,
}

async fn decide_appeal(
    State(state): State<AppState>,
    v: Viewer,
    Path(id): Path<i64>,
    Form(f): Form<AppealDecision>,
) -> Result<Response, AppError> {
    v.check_csrf(&f.csrf)?;
    let user = v.require_user()?;
    if !is_staff(&v) {
        return Err(AppError::Forbidden);
    }
    if !matches!(f.outcome.as_str(), "upheld" | "reversed") {
        return Err(AppError::Validation("Unknown outcome.".into()));
    }
    let pool = state.db()?;
    let s = dsa::statement(pool, id).await.db()?.ok_or(AppError::NotFound)?;
    // A moderator does not overrule an administrator, nor decide about a fellow member of staff.
    if !v.actor.is_admin {
        let by_admin = match s.moderator_id {
            Some(m) => bf_db::accounts::users::user_by_id(pool, m.into()).await.db()?.is_some_and(|u| u.is_admin),
            None => false,
        };
        let about_staff = match s.user_id {
            Some(u) => bf_db::accounts::users::user_by_id(pool, u.into())
                .await
                .db()?
                .is_some_and(|u| u.is_admin || u.groups.contains(&bf_core::ids::GroupId::MODERATORS)),
            None => false,
        };
        if by_admin || about_staff {
            return Err(AppError::Validation("Only an administrator can decide this appeal.".into()));
        }
    }
    if !dsa::decide_appeal(pool, id, &f.outcome, user.id.get()).await.db()? {
        return Err(AppError::Validation("That appeal was already decided, or you made the original decision.".into()));
    }
    if f.outcome == "reversed" {
        // The statement does not say until when the account was restricted, so a reversal lifts
        // the restriction only while it is still this one: not when the account was restricted
        // again afterwards (a later statement, a spammer clean-up, a known-image match or an
        // administrator's edit), which this appeal is not about.
        let again: bool = if s.target_type == "account" {
            sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM moderation_statements
                                WHERE target_type = 'account' AND target_id = $1 AND action = $2 AND id > $3)
                     OR EXISTS (SELECT 1 FROM audit_log WHERE target = $4 AND created_at > $5
                                AND (action IN ('user.delete_spammer', 'mod.spammer', 'upload.known_image')
                                     OR (action = 'user.update' AND details ->> $6 IS NOT NULL)))",
            )
            .bind(s.target_id)
            .bind(&s.action)
            .bind(id)
            .bind(format!("user:{}", s.target_id))
            .bind(s.created_at)
            .bind(format!("{}_until", s.action))
            .fetch_one(pool)
            .await
            .db()?
        } else {
            false
        };
        match (s.target_type.as_str(), s.action.as_str()) {
            ("account", _) if again => {}
            ("post", "hidden") => {
                bf_db::forum::posts::set_hidden(pool, s.target_id, false, None, user.id.get()).await.db()?;
            }
            ("post", "deleted") => {
                bf_db::moderation::topics::restore_post_override(pool, s.target_id, user.id.get(), true).await.db()?;
            }
            ("account", "suspended") => {
                sqlx::query("UPDATE users SET suspended_until = NULL WHERE id = $1")
                    .bind(s.target_id)
                    .execute(pool)
                    .await
                    .db()?;
            }
            ("account", "silenced") => {
                sqlx::query("UPDATE users SET silenced_until = NULL WHERE id = $1")
                    .bind(s.target_id)
                    .execute(pool)
                    .await
                    .db()?;
            }
            _ => {}
        }
        crate::moderation::topics::refresh(&state);
        state.sessions.invalidate_all();
    }
    if let Some(member) = s.user_id {
        let data = json!({ "title": if f.outcome == "reversed" { "reversed" } else { "upheld" }, "url": format!("/statements/{id}") });
        crate::community::notifications::send(&state, user.id.get(), &[member], "statement_decided", &data).await;
    }
    bf_db::admin::audit::record(
        pool,
        bf_db::admin::audit::Entry::new("dsa.appeal").by(user.id.get()).on(format!("statement:{id}")).details(json!({ "outcome": f.outcome })),
    )
    .await
    .db()?;
    Ok(Redirect::to("/review/dsa").into_response())
}

#[derive(Deserialize)]
struct PeriodQuery {
    #[serde(default)]
    period: String,
}

/// `2026-H2` as [start, end) in UTC.
fn period_bounds(key: &str) -> Option<(chrono::DateTime<Utc>, chrono::DateTime<Utc>)> {
    let (y, h) = key.split_once("-H")?;
    let year: i32 = y.parse().ok()?;
    let (start, end) = match h {
        "1" => {
            (Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single()?, Utc.with_ymd_and_hms(year, 7, 1, 0, 0, 0).single()?)
        }
        "2" => (
            Utc.with_ymd_and_hms(year, 7, 1, 0, 0, 0).single()?,
            Utc.with_ymd_and_hms(year + 1, 1, 1, 0, 0, 0).single()?,
        ),
        _ => return None,
    };
    Some((start, end))
}

async fn transparency(
    State(state): State<AppState>,
    v: Viewer,
    Query(q): Query<PeriodQuery>,
) -> Result<Response, AppError> {
    let now = Utc::now();
    let current = format!("{}-H{}", now.year(), if now.month() <= 6 { 1 } else { 2 });
    let mut periods = Vec::new();
    let (mut y, mut h) = (now.year(), if now.month() <= 6 { 1 } else { 2 });
    for _ in 0..6 {
        periods.push((
            format!("{y}-H{h}"),
            format!("{} {y}", if h == 1 { tr("January–June") } else { tr("July–December") }),
        ));
        if h == 1 {
            y -= 1;
            h = 2;
        } else {
            h = 1;
        }
    }
    let period = if period_bounds(&q.period).is_some() { q.period.clone() } else { current };
    let (from, to) = period_bounds(&period).ok_or(AppError::NotFound)?;
    let r = dsa::transparency(state.db()?, from, to).await.db()?;
    let (l, set) = layout(&state, &v, Some(&v.t().msg("Transparency report")));
    let hours = |h: Option<f64>| h.map(|h| format!("{h:.1}"));
    let body = TransparencyPage {
        layout: l,
        from: from.format("%Y-%m-%d").to_string(),
        to: (to - chrono::Duration::days(1)).format("%Y-%m-%d").to_string(),
        period,
        periods,
        notice_hours: hours(r.notice_median_hours),
        appeal_hours: hours(r.appeal_median_hours),
        contact: state.settings().str("dsa_contact_email").to_owned(),
        r: TransparencyView {
            notices: r.notices,
            notices_by_category: r.notices_by_category.into_iter().map(|(k, n)| (category_label(&k), n)).collect(),
            notices_actioned: r.notices_actioned,
            notices_rejected: r.notices_rejected,
            statements: r.statements,
            by_action: r.by_action.into_iter().map(|(k, n)| (measure_label(&k), n)).collect(),
            by_ground: r.by_ground.into_iter().map(|(k, n)| (ground_label(&k), n)).collect(),
            by_category: r.by_category.into_iter().map(|(k, n)| (category_label(&k), n)).collect(),
            automated_detection: r.automated_detection,
            automated_decision: r.automated_decision,
            appeals: r.appeals,
            appeals_reversed: r.appeals_reversed,
            appeals_upheld: r.appeals_upheld,
        },
    };
    Ok(page(render(&body), set, true))
}

#[cfg(test)]
mod tests {
    #[test]
    fn half_years() {
        let (a, b) = super::period_bounds("2026-H2").unwrap();
        assert_eq!(a.to_rfc3339(), "2026-07-01T00:00:00+00:00");
        assert_eq!(b.to_rfc3339(), "2027-01-01T00:00:00+00:00");
        assert!(super::period_bounds("2026-H3").is_none());
        assert!(super::period_bounds("x").is_none());
    }
}
