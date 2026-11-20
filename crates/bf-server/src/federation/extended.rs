//! The rest of following remote forums, account moves, polls and direct messages.
//!
//! * Mirrors: a local forum follows a remote `Group` (a Lemmy community, another forum's board).
//!   What the group announces becomes topics and replies here, written by shadow accounts.
//! * Move: a remote account that moves has its follows here passed on; a member who moves
//!   away sends `Move` to their followers, with `movedTo` and `alsoKnownAs` on their actor.
//! * Polls: a topic's first poll goes out as a `Question`; remote votes (a `Note` with only a
//!   `name`) count like members' votes.
//! * Direct messages (`federation_dms`): a non-public `Note` to a member becomes a private
//!   conversation; replies here go back to the remote people in it.

use super::*;

// --- Mirrors ---------------------------------------------------------------------------------

/// `acct:name@host`, `@name@host`, `name@host` or an actor URL, resolved to an actor.
pub(crate) async fn resolve(state: &AppState, handle: &str) -> Result<federation::RemoteActor, String> {
    let h = handle.trim();
    if h.starts_with("https://") {
        return remote_actor(state, h, true).await;
    }
    let acct = h.trim_start_matches("acct:").trim_start_matches('@');
    let (_, host) = acct.split_once('@').ok_or("use name@host")?;
    if host.is_empty() || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) {
        return Err("that is not a host name".into());
    }
    let url = format!("https://{host}/.well-known/webfinger?resource=acct:{acct}");
    if !crate::platform::outbound::federation_url_ok(&url) || blocked(state, &url) {
        return Err("address not allowed".into());
    }
    let headers = vec![("accept".to_owned(), "application/jrd+json, application/json".to_owned())];
    let (status, body) =
        crate::platform::outbound::request_public(reqwest::Method::GET, &url, &headers, None, 64 * 1024).await?;
    if !(200..300).contains(&status) {
        return Err(format!("WebFinger answered {status}"));
    }
    let doc: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    let href = doc["links"]
        .as_array()
        .and_then(|l| {
            l.iter().find(|x| {
                x["rel"] == "self"
                    && x["type"].as_str().is_some_and(|t| t.contains("activity+json") || t.contains("ld+json"))
            })
        })
        .and_then(|x| x["href"].as_str())
        .ok_or("no ActivityPub actor for that name")?;
    remote_actor(state, href, true).await
}

/// Starts following a remote group into a local forum.
pub(crate) async fn add_mirror(state: &AppState, node: i64, handle: &str) -> Result<(), AppError> {
    if !public_forum(state, node).await {
        return Err(AppError::Validation("Choose a forum guests can read: what arrives there is public.".into()));
    }
    let actor =
        resolve(state, handle).await.map_err(|e| AppError::invalid("Could not find that group: {e}", &[("e", &e)]))?;
    if !matches!(actor.kind.as_str(), "Group" | "Organization" | "Service" | "Application") {
        return Err(AppError::Validation("That is a person, not a group or community.".into()));
    }
    let me = actor_uri(state, "forum", node);
    let follow_id = format!("{me}/follows/{}/{}", actor.id, chrono::Utc::now().timestamp());
    federation::add_mirror(state.db()?, node, actor.id, &follow_id).await.db()?;
    let follow = json!({ "@context": context(), "id": follow_id, "type": "Follow", "actor": me, "object": actor.uri });
    enqueue(state, std::slice::from_ref(&actor.inbox), ("forum", node), &follow).await;
    Ok(())
}

pub(crate) async fn remove_mirror(state: &AppState, node: i64, actor_uri_: &str) -> Result<(), AppError> {
    let Some((follow_id, inbox)) = federation::remove_mirror(state.db()?, node, actor_uri_).await.db()? else {
        return Err(AppError::NotFound);
    };
    let me = actor_uri(state, "forum", node);
    let undo = json!({
        "@context": context(), "id": format!("{follow_id}/undo"), "type": "Undo", "actor": me,
        "object": { "id": follow_id, "type": "Follow", "actor": me, "object": actor_uri_ },
    });
    enqueue(state, &[inbox], ("forum", node), &undo).await;
    Ok(())
}

/// `Accept` or `Reject` of a Follow we sent.
pub(crate) async fn answered(
    state: &AppState,
    actor: &federation::RemoteActor,
    a: &Value,
    accepted: bool,
) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let object = &a["object"];
    let follow_id = object.as_str().or_else(|| object["id"].as_str()).ok_or("answer to nothing")?;
    federation::mirror_answer(pool, follow_id, actor.id, accepted).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// An object named by URI or inlined, fetched from its own server so the sender cannot forge it.
async fn authentic(state: &AppState, object: &Value) -> Result<Value, String> {
    let id = object.as_str().or_else(|| object["id"].as_str()).ok_or("object without id")?;
    let fetched = fetch(state, id).await?;
    if fetched["id"].as_str() != Some(id) {
        return Err("fetched object has another id".into());
    }
    Ok(fetched)
}

/// What a mirrored group announces: new posts (topics here) and replies to them.
pub(crate) async fn announced(state: &AppState, group: &federation::RemoteActor, object: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let nodes = federation::mirror_nodes(pool, group.id).await.map_err(|e| e.to_string())?;
    let Some(&node) = nodes.first() else { return Ok(()) };
    // Lemmy wraps the post in a Create; others announce the object itself.
    let inner = if object["type"] == "Create" || object["type"] == "Update" { &object["object"] } else { object };
    let note = authentic(state, inner).await?;
    if !matches!(note["type"].as_str(), Some("Note" | "Page" | "Article" | "Question")) {
        return Ok(());
    }
    let author_uri = match &note["attributedTo"] {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .find_map(|x| x.as_str().map(str::to_owned).or_else(|| x["id"].as_str().map(str::to_owned)))
            .ok_or("no author")?,
        v => v["id"].as_str().ok_or("no author")?.to_owned(),
    };
    if blocked(state, &author_uri) {
        return Ok(());
    }
    let author = remote_actor(state, &author_uri, false).await?;
    if domain_of(note["id"].as_str().unwrap_or_default()).as_deref() != Some(author.domain.as_str()) {
        return Err("object not on its author's server".into());
    }
    if note.get("inReplyTo").and_then(Value::as_str).is_some() {
        // A reply: the usual path finds the mirrored topic through the object table.
        return create_reply(state, &author, &note).await;
    }
    mirror_topic(state, node, &author, &note).await
}

async fn mirror_topic(
    state: &AppState,
    node: i64,
    author: &federation::RemoteActor,
    note: &Value,
) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let id = note["id"].as_str().ok_or("object without id")?;
    if federation::object(pool, id).await.map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    let limit = state.settings().int("federation_posts_per_hour");
    if federation::objects_from_domain_last_hour(pool, &author.domain).await.map_err(|e| e.to_string())? >= limit {
        return Err("instance over its hourly limit".into());
    }
    let mut raw = remote_text(note).unwrap_or_default();
    // A link post (Lemmy) has its link in `attachment`.
    if let Some(link) = note["attachment"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["type"] == "Link"))
        .and_then(|x| x["href"].as_str())
        && link.starts_with("https://")
    {
        raw = format!("{link}\n\n{raw}");
    }
    if raw.trim().is_empty() {
        return Err("empty".into());
    }
    let title: String = note["name"]
        .as_str()
        .or_else(|| note["summary"].as_str())
        .map_or_else(|| raw.lines().next().unwrap_or_default().to_owned(), strip_html)
        .chars()
        .take(200)
        .collect();
    let title = if title.trim().is_empty() { "Untitled".to_owned() } else { title.trim().to_owned() };
    let user = federation::ensure_shadow_user(pool, author).await.map_err(|e| e.to_string())?;
    let screened = crate::forum::posting::screen_federated(state, user, node, None, &raw).await?;
    crate::forum::posting::validate_title(state, &title).map_err(|_| "bad title".to_owned())?;
    let raw = screened.text;
    let flagged = screened.pending;
    // Remote text and titles go through the plugins' filters too; a refusal drops the topic.
    let raw = crate::platform::plugins::before_post_system(state, user, &raw).await.ok_or("refused by a plugin")?;
    let title = crate::platform::plugins::filter_text(state, "item.before_create", "title", &title)
        .await
        .map_err(|_| "refused by a plugin".to_owned())?;
    let cooked = crate::platform::plugins::cook(state, &raw).await;
    crate::forum::posting::check_federated_cooked(state, &screened.user, &raw, &cooked).await?;
    let post = bf_db::forum::posts::NewPost {
        author_id: user,
        raw: &raw,
        cooked: &cooked.html,
        cooked_version: bf_cook::COOK_VERSION,
        ip: None,
        pending: flagged,
    };
    let slug = bf_core::slug::slugify(&title);
    let slug = if slug.is_empty() { "topic".to_owned() } else { slug };
    let item = bf_db::forum::posts::create_topic(pool, node, &title, &slug, &post).await.map_err(|e| e.to_string())?;
    let post_id = federation::post_id_at(pool, item, 1).await.map_err(|e| e.to_string())?.ok_or("post not found")?;
    federation::add_object(pool, id, post_id, author.id).await.map_err(|e| e.to_string())?;
    state.forum.invalidate();
    if !flagged {
        let by = format!("{}@{}", author.username, author.domain);
        crate::forum::posting::after_system_post(state, user, &by, &title, &slug, item, node, 1).await;
    }
    metrics::counter!("bf_federation_mirrored_total").increment(1);
    Ok(())
}

// --- Moves -----------------------------------------------------------------------------------

/// A remote account moved: its follows go to the new one, if that one claims the old.
pub(crate) async fn moved(state: &AppState, actor: &federation::RemoteActor, a: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    if str_of(a, "object") != Some(actor.uri.as_str()) {
        return Err("a Move of someone else".into());
    }
    let target = str_of(a, "target").ok_or("Move without target")?;
    let doc = fetch(state, target).await?;
    let claims =
        doc["alsoKnownAs"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str() == Some(actor.uri.as_str())));
    if !claims {
        return Err("the new account does not name the old one".into());
    }
    let new = remote_actor(state, target, true).await?;
    federation::move_actor(pool, actor.id, new.id, &new.uri).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// The member's aliases and new home, for their actor document.
pub(crate) async fn decorate_actor(state: &AppState, user: i64, doc: &mut Value) {
    let Some(pool) = &state.pool else { return };
    if let Ok((aka, moved_to)) = federation::aliases(pool, user).await {
        if !aka.is_empty() {
            doc["alsoKnownAs"] = json!(aka);
        }
        if let Some(m) = moved_to {
            doc["movedTo"] = json!(m);
        }
    }
}

/// A member moves away: `movedTo` is set and their followers are told.
pub(crate) async fn move_away(state: &AppState, user: i64, target: &federation::RemoteActor) -> Result<(), AppError> {
    let pool = state.db()?;
    let me = actor_uri(state, "user", user);
    let doc = fetch(state, &target.uri)
        .await
        .map_err(|e| AppError::invalid("Could not read the new account: {e}", &[("e", &e)]))?;
    let claims = doc["alsoKnownAs"].as_array().is_some_and(|l| l.iter().any(|x| x.as_str() == Some(me.as_str())));
    if !claims {
        return Err(AppError::invalid(
            "First add {me} as an alias (\"also known as\") on the new account.",
            &[("me", &me)],
        ));
    }
    let (aka, _) = federation::aliases(pool, user).await.db()?;
    federation::set_aliases(pool, user, &aka, Some(&target.uri)).await.db()?;
    let activity = json!({
        "@context": context(), "id": format!("{me}/move/{}", chrono::Utc::now().timestamp()), "type": "Move",
        "actor": me, "object": me, "target": target.uri,
    });
    let inboxes = federation::all_follower_inboxes(pool, user).await.db()?;
    enqueue(state, &inboxes, ("user", user), &activity).await;
    Ok(())
}

// --- Polls -----------------------------------------------------------------------------------

/// Turns a first post's Note into a Question when the topic has a poll.
pub(crate) async fn decorate_note(state: &AppState, item: i64, note: &mut Value) {
    let Some(pool) = &state.pool else { return };
    let Ok(polls) = bf_db::community::polls::polls_of(pool, item).await else { return };
    // Only the plain kind of poll has a form the fediverse knows.
    let Some(p) = polls.first().filter(|p| p.kind == "choice") else { return };
    let Ok(options) = bf_db::community::polls::options_of(pool, item, None).await else { return };
    let choices: Vec<Value> = options
        .iter()
        .filter(|o| o.0 == p.id)
        .map(|o| json!({ "type": "Note", "name": o.2, "replies": { "type": "Collection", "totalItems": o.3 } }))
        .collect();
    note["type"] = json!("Question");
    note[if p.max_choices > 1 { "anyOf" } else { "oneOf" }] = json!(choices);
    note["votersCount"] = json!(p.voters);
    if let Some(end) = p.closes_at {
        note["endTime"] = json!(end.to_rfc3339());
        if end <= chrono::Utc::now() {
            note["closed"] = json!(end.to_rfc3339());
        }
    }
}

/// A vote: a Note with a `name` (the option) and no content, answering our Question.
pub(crate) fn is_vote(object: &Value) -> bool {
    object["type"] == "Note"
        && object["name"].is_string()
        && object.get("content").and_then(Value::as_str).is_none_or(str::is_empty)
}

pub(crate) async fn vote(state: &AppState, actor: &federation::RemoteActor, object: &Value) -> Result<(), String> {
    let pool = state.pool.as_ref().ok_or("no database")?;
    let target = str_of(object, "inReplyTo").ok_or("vote on nothing")?;
    let post = local_post_for(state, target).await.ok_or("not a poll here")?;
    // The post and its topic must be visible, in a forum guests can read.
    let (p, _) = federable_post(state, post).await.ok_or("not a public poll")?;
    let polls = bf_db::community::polls::polls_of(pool, p.item_id).await.map_err(|e| e.to_string())?;
    // The poll belongs to this very post: the topic's (no post number) or one made in a reply.
    let poll = polls.iter().find(|q| q.post_number.unwrap_or(1) == p.post_number).ok_or("no poll")?;
    if poll.kind != "choice" || !poll.groups.is_empty() || poll.min_trust_level > 0 {
        return Err("this poll limits who may vote".into());
    }
    let locked: bool = sqlx::query_scalar("SELECT locked OR archived FROM items WHERE id = $1")
        .bind(p.item_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
    if poll.closed(locked, chrono::Utc::now()) {
        return Err("poll closed".into());
    }
    let name = object["name"].as_str().unwrap_or_default();
    let options = bf_db::community::polls::options_of(pool, p.item_id, None).await.map_err(|e| e.to_string())?;
    let option = options.iter().find(|o| o.0 == poll.id && o.2 == name).ok_or("no such option")?;
    let user = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
    // A suspended or silenced shadow account does not vote, as it does not reply.
    if inbox::restricted_here(pool, user).await? {
        return Err("account restricted here".into());
    }
    // A multiple-choice vote arrives as one Note per option: keep the choices already made.
    let mut chosen: Vec<i64> = bf_db::community::polls::options_of(pool, p.item_id, Some(user))
        .await
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|o| o.0 == poll.id && o.4)
        .map(|o| o.1)
        .collect();
    if !chosen.contains(&option.1) {
        chosen.push(option.1);
    }
    if chosen.len() > usize::try_from(poll.max_choices.max(1)).unwrap_or(1) {
        chosen = vec![option.1];
    }
    bf_db::community::polls::vote(pool, poll, &chosen, user).await.map_err(|e| e.to_string())?;
    state.microcache.clear();
    Ok(())
}

// --- Direct messages -------------------------------------------------------------------------

fn dm_uri(state: &AppState, message: i64) -> String {
    format!("{}/ap/m/{message}", base(state))
}

fn addressees(object: &Value) -> Vec<String> {
    ["to", "cc", "bto", "bcc"]
        .iter()
        .flat_map(|k| match &object[*k] {
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
            Value::String(s) => vec![s.clone()],
            _ => vec![],
        })
        .collect()
}

/// A non-public Note: a direct message to members here, or nothing for a forum.
pub(crate) async fn inbound_dm(
    state: &AppState,
    actor: &federation::RemoteActor,
    object: &Value,
) -> Result<(), String> {
    if !state.settings().bool("federation_dms") {
        return Err("direct messages are off".into());
    }
    let pool = state.pool.as_ref().ok_or("no database")?;
    let id = object["id"].as_str().ok_or("object without id")?;
    if domain_of(id).as_deref() != Some(actor.domain.as_str())
        || str_of(object, "attributedTo") != Some(actor.uri.as_str())
    {
        return Err("object not by its sender".into());
    }
    if federation::dm_conversation(pool, id).await.map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    // Messages count toward the instance's hourly limit together with its replies.
    let limit = state.settings().int("federation_posts_per_hour");
    let lately = federation::objects_from_domain_last_hour(pool, &actor.domain).await.map_err(|e| e.to_string())?
        + federation::dm_objects_from_domain_last_hour(pool, &actor.domain).await.map_err(|e| e.to_string())?;
    if lately >= limit {
        return Err("instance over its hourly limit".into());
    }
    let mut named: Vec<i64> = addressees(object)
        .iter()
        .filter_map(|to| match local_target(state, to) {
            Some(("user", uid)) => Some(uid),
            _ => None,
        })
        .collect();
    named.sort_unstable();
    named.dedup();
    // As many people as a member could put in one conversation, the sender included.
    if named.len() as i64 + 1 > state.settings().int("pm_max_participants") {
        return Err("addressed to too many people here".into());
    }
    let mut recipients: Vec<i64> = Vec::new();
    for uid in named {
        if federation::local_user(pool, uid).await.ok().flatten().is_some() {
            recipients.push(uid);
        }
    }
    if recipients.is_empty() {
        return Err("not addressed to anyone here".into());
    }
    let raw = remote_text(object)?;
    let tree = state.forum.get(pool).await.map_err(|e| e.to_string())?;
    let raw = match bf_core::watched::apply(&tree.words, &raw) {
        bf_core::watched::Outcome::Blocked(_) => return Err("blocked by watched words".into()),
        // A word that needs approval would hold a member's message; nobody could approve one that
        // has already arrived from elsewhere, so it is dropped quietly.
        bf_core::watched::Outcome::Allow { needs_approval: true, .. } => return Ok(()),
        bf_core::watched::Outcome::Allow { text, .. } => text,
    };
    let user = federation::ensure_shadow_user(pool, actor).await.map_err(|e| e.to_string())?;
    if inbox::restricted_here(pool, user).await? {
        return Err("account restricted here".into());
    }
    // The standing a member needs to write privately.
    let level = bf_db::accounts::users::user_by_id(pool, user.into())
        .await
        .map_err(|e| e.to_string())?
        .map_or(0, |u| i64::from(u.trust_level));
    if level < state.settings().int("pm_min_trust_level") {
        return Ok(());
    }
    // Text filters apply to remote private messages too; a refusal drops the message.
    let raw = crate::platform::plugins::before_post_system(state, user, &raw).await.ok_or("refused by a plugin")?;
    // Members who take messages from nobody (or only from people they follow) and those who
    // ignore the sender are left out, as they would be for a member's message.
    let refusing =
        bf_db::community::messages::restricting(pool, user, &recipients).await.map_err(|e| e.to_string())?;
    let mut allowed = Vec::new();
    for r in recipients {
        if !refusing.contains(&r) && !federation::ignores(pool, r, user).await.map_err(|e| e.to_string())? {
            allowed.push(r);
        }
    }
    if allowed.is_empty() {
        return Ok(());
    }
    let by = format!("{}@{}", actor.username, actor.domain);
    let earlier = match str_of(object, "inReplyTo") {
        Some(parent) => match federation::dm_conversation(pool, parent).await.map_err(|e| e.to_string())? {
            Some(c) => Some(c),
            None => local_message_conversation(state, parent).await,
        },
        None => None,
    };
    let conversation = match earlier {
        Some(c)
            if bf_db::community::messages::participant_of(pool, c, user)
                .await
                .map_err(|e| e.to_string())?
                .is_some() =>
        {
            crate::community::messages::deliver_reply(state, c, (user, &by), &raw)
                .await
                .map_err(|e| format!("{e:?}"))?;
            c
        }
        _ => {
            let subject: String = object["summary"]
                .as_str()
                .map(strip_html)
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| format!("Message from {by}"))
                .chars()
                .take(120)
                .collect();
            crate::community::messages::deliver_new(state, (user, &by), &subject, &raw, &allowed)
                .await
                .map_err(|e| format!("{e:?}"))?
        }
    };
    if let Ok(Some((msg, _, _))) = federation::dm_latest_by(pool, conversation, user).await {
        federation::add_dm_object(pool, id, msg, conversation).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A direct message as a Note, addressed to the remote people in its conversation.
#[allow(clippy::too_many_arguments)]
fn dm_note(
    state: &AppState,
    conversation: i64,
    msg: i64,
    author: i64,
    cooked: &str,
    at: chrono::DateTime<chrono::Utc>,
    to: &[&str],
    in_reply_to: Option<&str>,
) -> Value {
    let mentions: Vec<Value> = to.iter().map(|uri| json!({ "type": "Mention", "href": uri })).collect();
    json!({
        "@context": context(),
        "id": dm_uri(state, msg), "type": "Note", "attributedTo": actor_uri(state, "user", author), "to": to, "cc": [],
        "tag": mentions, "content": absolute(state, cooked), "mediaType": "text/html", "published": at.to_rfc3339(),
        "inReplyTo": in_reply_to, "context": format!("{}/ap/conversations/{conversation}", base(state)),
    })
}

/// Checks a signed GET: `(request-target)`, `host` and `date` signed by a key we can fetch.
async fn verify_get(state: &AppState, uri: &Uri, headers: &HeaderMap) -> Result<federation::RemoteActor, String> {
    let sig = headers.get("signature").and_then(|v| v.to_str().ok()).ok_or("unsigned")?;
    let p = parse_signature(sig).ok_or("bad signature header")?;
    for required in ["(request-target)", "host", "date"] {
        if !p.headers.iter().any(|h| h == required) {
            return Err(format!("{required} not signed"));
        }
    }
    let date = headers.get("date").and_then(|v| v.to_str().ok()).ok_or("no date")?;
    inbox::date_fresh(date, chrono::Utc::now())?;
    if !inbox::host_matches(headers.get("host").and_then(|v| v.to_str().ok()), &host(state)) {
        return Err("signed for another host".into());
    }
    // The key may have to be fetched from the host in `keyId`: the same budget as the inbox's
    // for a host whose signatures failed before, so this cannot be used to make fetches at will.
    let key_host = domain_of(&p.key_id).unwrap_or_default();
    if !inbox::unverified_allowed(&key_host) {
        return Err("too many unverified requests".into());
    }
    let checked = signed_by(state, &p, uri, headers).await;
    if checked.is_err() {
        inbox::unverified_failed(&key_host);
    }
    checked
}

/// The actor whose key made the signature of a GET.
async fn signed_by(
    state: &AppState,
    p: &super::signatures::SigParams,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<federation::RemoteActor, String> {
    let msg = signing_string(p, &Method::GET, uri, headers).ok_or("signed header missing")?;
    let pool = state.pool.as_ref().ok_or("no database")?;
    let owner = p.key_id.split('#').next().unwrap_or_default().to_owned();
    let actor = match federation::actor_by_key(pool, &p.key_id).await.ok().flatten() {
        Some(a) => a,
        None => remote_actor(state, &owner, true).await?,
    };
    if verify_with(&actor.public_pem, &msg, &p.signature) {
        return Ok(actor);
    }
    let fresh = remote_actor(state, &actor.uri, true).await?;
    if verify_with(&fresh.public_pem, &msg, &p.signature) { Ok(fresh) } else { Err("signature does not verify".into()) }
}

/// `GET /ap/m/{id}`: a direct message, for the servers of the people in its conversation only.
pub(crate) async fn fetch_dm(state: &AppState, id: i64, uri: &Uri, headers: &HeaderMap) -> Response {
    if !enabled(state) || !state.settings().bool("federation_dms") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(pool) = &state.pool else { return StatusCode::NOT_FOUND.into_response() };
    let fetcher = match verify_get(state, uri, headers).await {
        Ok(a) => a,
        Err(e) => {
            tracing::info!(error = %e, "federation: unsigned or unverified message fetch");
            // Not found rather than unauthorized: nothing says the message exists.
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let row: Option<(i64, Option<i64>, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT conversation_id, author_id, cooked, created_at FROM conversation_messages WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((conversation, Some(author), cooked, at)) = row else { return StatusCode::NOT_FOUND.into_response() };
    if federation::is_remote_user(pool, author).await.unwrap_or(true) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let remote = federation::dm_remote_participants(pool, conversation).await.unwrap_or_default();
    let allowed = dm_fetch_allowed(&remote, &fetcher.uri);
    if !allowed {
        return StatusCode::NOT_FOUND.into_response();
    }
    let in_reply_to = match federation::dm_last_before(pool, conversation, id).await {
        Ok(Some((prev, uri))) => Some(uri.unwrap_or_else(|| dm_uri(state, prev))),
        _ => None,
    };
    let to: Vec<&str> = remote.iter().map(|(u, _)| u.as_str()).collect();
    let mut res = activity_json(dm_note(state, conversation, id, author, &cooked, at, &to, in_reply_to.as_deref()));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    res
}

/// `/ap/m/{id}`: one of our own messages, by its id.
async fn local_message_conversation(state: &AppState, uri: &str) -> Option<i64> {
    let rest = uri.strip_prefix(&format!("{}/ap/m/", base(state)))?;
    let id: i64 = rest.split(['/', '#', '?']).next()?.parse().ok()?;
    let pool = state.pool.as_ref()?;
    sqlx::query_scalar("SELECT conversation_id FROM conversation_messages WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()?
}

/// After a member writes in a conversation with remote people: the message goes to them.
pub(crate) async fn outbound_dm(state: &AppState, conversation: i64, author: i64) {
    if !enabled(state) || !state.settings().bool("federation_dms") {
        return;
    }
    let Some(pool) = &state.pool else { return };
    // Remote shadow accounts never write here themselves.
    if federation::is_remote_user(pool, author).await.unwrap_or(true) {
        return;
    }
    let Ok(remote) = federation::dm_remote_participants(pool, conversation).await else { return };
    if remote.is_empty() || federation::local_user(pool, author).await.ok().flatten().is_none() {
        return;
    }
    let Ok(Some((msg, cooked, at))) = federation::dm_latest_by(pool, conversation, author).await else { return };
    let in_reply_to = match federation::dm_last(pool, conversation, msg).await {
        Ok(Some((prev, uri))) => Some(uri.unwrap_or_else(|| dm_uri(state, prev))),
        _ => None,
    };
    let me = actor_uri(state, "user", author);
    let to: Vec<&str> = remote.iter().map(|(uri, _)| uri.as_str()).collect();
    let uri = dm_uri(state, msg);
    let note = dm_note(state, conversation, msg, author, &cooked, at, &to, in_reply_to.as_deref());
    let create = json!({
        "@context": context(), "id": format!("{uri}/activity"), "type": "Create", "actor": me,
        "to": to, "cc": [], "published": at.to_rfc3339(), "object": note,
    });
    let inboxes: Vec<String> = remote.into_iter().map(|(_, inbox)| inbox).collect();
    enqueue(state, &inboxes, ("user", author), &create).await;
    let _ = federation::add_dm_object(pool, &uri, msg, conversation).await;
}

/// A direct message is shown only to the remote people in its conversation, by actor, not by server.
fn dm_fetch_allowed(participants: &[(String, String)], fetcher: &str) -> bool {
    participants.iter().any(|(actor, _)| actor == fetcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_participant_may_fetch_a_message() {
        let people = vec![("https://a.example/users/bob".to_owned(), "https://a.example/inbox".to_owned())];
        assert!(dm_fetch_allowed(&people, "https://a.example/users/bob"));
        assert!(!dm_fetch_allowed(&people, "https://a.example/users/mallory"));
        assert!(!dm_fetch_allowed(&[], "https://a.example/users/bob"));
    }

    #[test]
    fn votes_are_notes_with_only_a_name() {
        assert!(is_vote(&json!({ "type": "Note", "name": "Yes", "inReplyTo": "https://x/ap/p/1" })));
        assert!(!is_vote(&json!({ "type": "Note", "name": "Yes", "content": "<p>hi</p>" })));
        assert!(!is_vote(&json!({ "type": "Note", "content": "<p>hi</p>" })));
    }

    #[test]
    fn addressees_cover_every_field() {
        let o = json!({ "to": ["a"], "cc": "b", "bcc": ["c"] });
        assert_eq!(addressees(&o), vec!["a", "b", "c"]);
    }
}
