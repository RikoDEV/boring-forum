//! Topic summaries, and the preview card shown over topic links.

use crate::forum::topics::readable_topic;
use crate::http::render::{DbResult, ago, layout, page};
use crate::http::viewer::Viewer;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bf_core::AppError;
use bf_core::slug::{forum_path, topic_path};
use bf_db::forum::summary;
use bf_web::pages::render;
use serde_json::json;

pub fn router() -> Router<AppState> {
    Router::new().route("/t/{id}/summary", get(topic_summary)).route("/api/v1/topics/{id}/card", get(topic_card))
}

async fn topic_summary(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let (t, tree) = (t, state.forum.get(state.db()?).await?);
    let pool = state.db()?;
    let rows = summary::top_replies(pool, id).await.db()?;
    let (participants, reactions, chars) = summary::topic_totals(pool, id).await.db()?;
    let posters = summary::topic_posters(pool, id).await.db()?;
    let links = summary::topic_shared_links(pool, id).await.db()?;
    let node = tree.node(t.node_id).ok_or(AppError::NotFound)?;
    let topic_url = topic_path(t.id, &t.slug);
    let title = format!("Top replies: {}", t.title);
    let (mut l, set) = layout(&state, &v, Some(&title));
    l.noindex = true;
    let body = bf_web::pages::TopicSummaryPage {
        layout: l,
        title: t.title.clone(),
        forum_title: node.title.clone(),
        forum_url: bf_core::slug::forum_path(node.id, &node.slug),
        author: t.author.clone().unwrap_or_else(|| "deleted".into()),
        started: crate::http::render::ago(t.created_at),
        last_posted: crate::http::render::ago(t.last_posted_at),
        last_poster: t.last_poster.clone().unwrap_or_else(|| "deleted".into()),
        replies: t.reply_count,
        views: t.view_count,
        participants,
        reactions,
        // About 1,000 characters a minute, and never "0 min".
        read_minutes: (chars / 1000).max(1),
        top: rows
            .into_iter()
            .map(|(n, who, cooked, likes)| {
                let url = format!("{topic_url}#post-{n}");
                (n, who.unwrap_or_else(|| "deleted".into()), cooked, likes, url)
            })
            .collect(),
        posters,
        links,
        topic_url,
    };
    Ok(page(render(&body), set, false))
}

/// The preview card over a topic link: what it is about (the opening of its first post and its
/// first picture), where it is, who started it, and how lively it is. The same access rule as
/// the topic page.
async fn topic_card(State(state): State<AppState>, v: Viewer, Path(id): Path<i64>) -> Result<Response, AppError> {
    let (t, _, _) = readable_topic(&state, &v, id).await?;
    let pool = state.db()?;
    let tree = state.forum.get(pool).await?;
    let node = tree.node(t.node_id).ok_or(AppError::NotFound)?;
    let first = summary::first_post(pool, id).await.db()?;
    let (participants, _, _) = summary::topic_totals(pool, id).await.db()?;
    let excerpt = first.as_ref().map(|(cooked, _)| bf_seo::excerpt(cooked, 280)).filter(|e| !e.is_empty());
    // A one-line gist made by the AI, when there is one, says more than the opening words.
    let gist = crate::ai::summary::gist(&state, id, v.locale).await;
    let image = first.as_ref().and_then(|(cooked, _)| first_upload_image(cooked));
    let ideas = node.mode == "ideas";
    let mut card = json!({
        "id": t.id,
        "title": t.title,
        "url": topic_path(t.id, &t.slug),
        "prefix": t.prefix,
        "forum": node.title,
        "forum_url": forum_path(node.id, &node.slug),
        // Null for a deleted member: the island writes the translated stand-in.
        "author": t.author,
        "started": ago(t.created_at),
        "started_at": t.created_at.to_rfc3339(),
        "last_poster": t.last_poster,
        "last_posted": ago(t.last_posted_at),
        "last_posted_at": t.last_posted_at.to_rfc3339(),
        "replies": t.reply_count,
        "views": t.view_count,
        "likes": first.as_ref().map_or(0, |f| f.1),
        "participants": participants,
        "votes": ideas.then_some(t.vote_count),
        "excerpt": excerpt,
        "gist": gist,
        "image": image,
        "pinned": t.pinned,
        "locked": t.locked,
        "solved": t.solved_post_number.is_some(),
        "tags": t.tag_pairs.iter().filter_map(|p| p.split_once('|')).map(|(n, _)| n).collect::<Vec<_>>(),
        "unread": t.unread(),
        "unread_url": t.unread().map(|_| format!("{}/unread", topic_path(t.id, &t.slug))),
    });
    crate::platform::plugins::add_plugin_fields(&state, "topic", t.id, &mut card).await;
    let mut res = Json(card).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=30"));
    Ok(res)
}

/// The first picture of a post, when it is one of the site's own uploads.
fn first_upload_image(cooked: &str) -> Option<String> {
    let at = cooked.find("<img")?;
    let rest = &cooked[at..];
    let src = rest.split("src=\"").nth(1)?.split('"').next()?;
    (src.starts_with("/uploads/") && !src.contains(['<', '>', '\'', ' '])).then(|| src.to_owned())
}
