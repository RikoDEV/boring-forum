//! Topic, post, composer-helper and profile-card routes.

use serde_json::{Value, json};

fn s() -> Value {
    json!({ "type": "string" })
}
fn sn() -> Value {
    json!({ "type": ["string", "null"] })
}
fn i() -> Value {
    json!({ "type": "integer" })
}
fn b() -> Value {
    json!({ "type": "boolean" })
}
fn d(mut v: Value, text: &str) -> Value {
    v["description"] = json!(text);
    v
}
fn dt() -> Value {
    json!({ "type": "string", "format": "date-time" })
}
fn arr(item: Value) -> Value {
    json!({ "type": "array", "items": item })
}
fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}
fn q(name: &str, required: bool, description: &str, schema: Value) -> Value {
    json!({ "name": name, "required": required, "description": description, "schema": schema })
}

/// Free-form fields plugins add to a response.
fn plugins_note() -> Value {
    d(json!({ "type": "object", "additionalProperties": true }), "Fields added by plugins, when any do.")
}

fn forum_topic_row() -> Value {
    obj(
        json!({
            "id": i(), "title": s(), "slug": s(), "url": s(), "author": sn(), "forum_id": i(),
            "pinned": b(), "locked": b(), "replies": i(), "views": i(),
            "last_posted_at": dt(), "created_at": dt(), "tags": arr(s()),
            "meta": d(json!({ "type": "object", "additionalProperties": true }), "Public plugin metadata, present only when a plugin keeps some for this topic."),
        }),
        &["id", "title", "slug", "url", "author", "forum_id", "pinned", "locked", "replies", "views", "last_posted_at", "created_at", "tags"],
    )
}

fn post_list_row() -> Value {
    obj(
        json!({
            "id": i(), "number": i(), "author": sn(),
            "html": d(s(), "The post as the topic page shows it (sanitized HTML)."),
            "likes": i(), "created_at": dt(), "state": d(s(), "visible, hidden, deleted or pending."),
            "meta": d(json!({ "type": "object", "additionalProperties": true }), "Public plugin metadata, present only when a plugin keeps some for this post."),
        }),
        &["id", "number", "author", "html", "likes", "created_at", "state"],
    )
}

fn topic_card() -> Value {
    obj(
        json!({
            "id": i(), "title": s(), "url": s(), "prefix": sn(), "forum": s(), "forum_url": s(),
            "author": d(sn(), "Null for a deleted member."),
            "started": d(s(), "Relative time text."), "started_at": dt(),
            "last_poster": sn(), "last_posted": s(), "last_posted_at": dt(),
            "replies": i(), "views": i(), "likes": d(i(), "Likes on the first post."),
            "participants": i(),
            "votes": d(json!({ "type": ["integer", "null"] }), "Vote count, only in ideas forums."),
            "excerpt": d(sn(), "Opening of the first post, up to 280 characters."),
            "gist": d(sn(), "One-line AI summary, when there is one."),
            "image": d(sn(), "First uploaded picture of the first post."),
            "pinned": b(), "locked": b(), "solved": b(), "tags": arr(s()),
            "unread": d(json!({ "type": ["integer", "null"] }), "Unread posts for a signed-in reader; null for guests and fully read topics."),
            "unread_url": sn(),
        }),
        &["id", "title", "url", "prefix", "forum", "forum_url", "author", "started", "started_at", "last_poster", "last_posted", "last_posted_at", "replies", "views", "likes", "participants", "votes", "excerpt", "gist", "image", "pinned", "locked", "solved", "tags", "unread", "unread_url"],
    )
}

fn user_card() -> Value {
    let group = obj(json!({ "name": s(), "colour": sn(), "slug": s() }), &["name", "colour", "slug"]);
    let badge = obj(
        json!({ "name": s(), "tier": s(), "icon": s(), "color": sn(), "level": json!({}), "rare": b() }),
        &["name", "tier", "icon", "color", "level", "rare"],
    );
    obj(
        json!({
            "id": i(), "username": s(), "display_name": sn(), "title": sn(), "url": s(),
            "group": d(sn(), "Primary group name."),
            "group_colour": d(sn(), "Hex colour such as #aabbcc, or null."),
            "groups": d(arr(group), "Secondary groups."),
            "rank": sn(), "rank_tier": d(s(), "bronze, silver or gold."), "rank_color": d(i(), "Colour step 0 to 9."),
            "trust": d(json!({ "type": ["object", "null"], "properties": { "level": i(), "name": s() } }), "Null unless the site shows trust levels, or the viewer is the member or staff."),
            "cover": sn(), "cover_y": i(), "status": sn(), "bio": d(sn(), "Plain-text excerpt of the about text, up to 180 characters."),
            "location": sn(), "joined": d(s(), "Month and year, e.g. Oct 2026."), "joined_at": dt(),
            "last_seen": sn(), "last_seen_at": json!({ "type": ["string", "null"], "format": "date-time" }),
            "online": d(b(), "Seen within the last 15 minutes."),
            "posts": i(), "likes": i(), "solved": i(), "reputation": i(),
            "followers": d(json!({ "type": ["integer", "null"] }), "Null when the member hides their followers."),
            "badges": d(arr(badge), "The member's four most recent badges."),
            "own": d(b(), "The viewer is this member."),
            "following": d(json!({ "type": ["boolean", "null"] }), "Null for guests and for the member's own card."),
            "can_message": b(),
            "slot_html": d(s(), "HTML drawn by plugins in the card slot; present only when a plugin adds some."),
            "plugins": plugins_note(),
        }),
        &["id", "username", "display_name", "title", "url", "group", "group_colour", "groups", "rank", "rank_tier", "rank_color", "trust", "cover", "cover_y", "status", "bio", "location", "joined", "joined_at", "last_seen", "last_seen_at", "online", "posts", "likes", "solved", "reputation", "followers", "badges", "own", "following", "can_message"],
    )
}

pub(super) fn docs() -> Vec<Value> {
    vec![
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/related", "auth": "viewer",
            "response": d(arr(obj(
                json!({ "id": i(), "title": s(), "url": s(), "forum": s(), "score": d(json!({ "type": "number" }), "Cosine similarity, 0.45 or more, rounded to two decimals.") }),
                &["id", "title", "url", "forum", "score"],
            )), "Up to 5 readable topics close in meaning, best first. Empty when AI embeddings or related topics are off, or the topic is not indexed yet."),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/card", "auth": "viewer",
            "response": topic_card(),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/links", "auth": "viewer",
            "response": arr(obj(json!({ "post": d(i(), "Post id."), "url": s(), "clicks": i() }), &["post", "url", "clicks"])),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/state", "auth": "viewer",
            "response": obj(
                json!({
                    "highest_post_number": d(i(), "Newest post number the viewer may see (moderators also count hidden and deleted posts)."),
                    "plugins": plugins_note(),
                }),
                &["highest_post_number"],
            ),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/dates", "auth": "viewer",
            "response": d(arr(json!({
                "type": "array", "prefixItems": [i(), json!({ "type": "string", "format": "date" })],
                "minItems": 2, "maxItems": 2,
            })), "Pairs of [post number, date as YYYY-MM-DD], spaced along the topic."),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/posts/{n}", "auth": "viewer",
            "response": obj(
                json!({
                    "html": d(s(), "The post's cooked, sanitized HTML. Only visible, non-action posts are returned (otherwise 404)."),
                    "author": d(sn(), "Username; null for a deleted member."),
                    "plugins": plugins_note(),
                }),
                &["html", "author"],
            ),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/posts", "auth": "viewer",
            "query": [
                q("from", true, "First post number to return (values below 1 count as 1).", i()),
                q("count", false, "How many posts, clamped to 1..50. Default 1.", json!({ "type": "integer", "default": 1, "minimum": 1, "maximum": 50 })),
                q("seen", false, "The reader has the page in front of them: marks the posts read for a signed-in viewer. Default false.", json!({ "type": "boolean", "default": false })),
            ],
            "response": d(s(), "Not JSON: an HTML fragment (text/html) of the posts as the topic page renders them for this viewer. Posts the viewer may not see are left out."),
        }),
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/post-list", "auth": "viewer",
            "query": [
                q("from", false, "First post number (values below 1 count as 1). Default 1.", json!({ "type": "integer", "default": 1 })),
                q("count", false, "How many posts, clamped to 1..50. Default 20.", json!({ "type": "integer", "default": 20, "minimum": 1, "maximum": 50 })),
            ],
            "response": arr(post_list_row()),
        }),
        json!({
            "method": "get", "path": "/api/v1/forums/{id}/topics", "auth": "viewer",
            "query": [q("page", false, "Zero-based page of 30 topics, newest activity first. Default 0.", json!({ "type": "integer", "default": 0, "minimum": 0 }))],
            "response": arr(forum_topic_row()),
        }),
        json!({
            "method": "get", "path": "/api/v1/posts/{id}/source", "auth": "member",
            "response": obj(
                json!({
                    "raw": d(s(), "The post's Markdown source."),
                    "base": d(i(), "The post's edit count; send it back with an edit to detect clashes."),
                    "reason_required": d(b(), "The site requires an edit reason."),
                }),
                &["raw", "base", "reason_required"],
            ),
        }),
        json!({
            "method": "get", "path": "/api/v1/posts/{id}/raw", "auth": "viewer",
            "response": obj(
                json!({
                    "raw": d(s(), "Markdown source; hidden-from-non-repliers parts are stripped unless the viewer is a moderator, the author or has replied."),
                    "author": sn(), "post_number": i(),
                    "url": d(s(), "Topic path with #post-N."),
                }),
                &["raw", "author", "post_number", "url"],
            ),
        }),
        json!({
            "method": "get", "path": "/api/v1/posts/{id}/revisions", "auth": "viewer",
            "response": d(arr(obj(
                json!({
                    "n": i(), "hidden": b(),
                    "raw": d(sn(), "Null when the revision is hidden and the viewer is not staff."),
                    "editor": sn(), "reason": sn(), "created_at": dt(),
                }),
                &["n", "hidden", "raw", "editor", "reason", "created_at"],
            )), "Earlier versions, newest first; revision n holds the text before edit n. Refused (403) when the site limits edit history to members, the author or staff and the viewer is none of those."),
        }),
        json!({
            "method": "get", "path": "/api/v1/posts/{id}/reactions", "auth": "viewer",
            "response": obj(
                json!({
                    "groups": d(arr(obj(
                        json!({
                            "id": i(), "icon": s(), "name": s(),
                            "users": d(arr(obj(json!({ "name": s(), "at": dt() }), &["name", "at"])), "Newest first."),
                        }),
                        &["id", "icon", "name", "users"],
                    )), "The reactions in the order the site lists them, only those somebody used."),
                }),
                &["groups"],
            ),
        }),
        json!({
            "method": "post", "path": "/api/v1/preview", "auth": "member",
            "request": obj(json!({ "raw": d(s(), "Markdown to render; at most the site's max_post_length characters.") }), &["raw"]),
            "response": obj(json!({ "html": d(s(), "Cooked HTML, as saving would produce it.") }), &["html"]),
        }),
        json!({
            "method": "get", "path": "/api/v1/autocomplete", "auth": "member",
            "query": [
                q("kind", true, "What to complete: user (@mention), tag, or node (forums and tags). Anything else is 404.", json!({ "type": "string", "enum": ["user", "tag", "node"] })),
                q("q", false, "The text typed so far. Default empty.", s()),
                q("topic", false, "For kind=user: a readable topic whose participants are listed first.", i()),
            ],
            "response": d(arr(obj(
                json!({
                    "label": d(s(), "Username (user), group name, tag name or forum title."),
                    "name": d(sn(), "kind=user, members only: display name."),
                    "in_topic": d(b(), "kind=user, members only: participates in `topic`."),
                    "group": d(b(), "kind=user: true for a mentionable group (at most 4, only when q is not empty)."),
                    "url": d(s(), "kind=node: forum or tag path."),
                    "tag": d(b(), "kind=node: true for a tag entry."),
                }),
                &["label"],
            )), "Up to 8 members plus 4 groups (user), 8 tags (tag), or 6 forums plus 4 tags (node). Which fields appear depends on kind."),
        }),
        json!({
            "method": "get", "path": "/api/v1/onebox", "auth": "viewer",
            "query": [q("url", true, "A link already posted in a public post, or one of this site's own topic addresses.", s())],
            "response": obj(
                json!({
                    "title": s(), "description": d(s(), "Empty for the site's own topics."),
                    "image": d(s(), "Empty when there is none."), "site": d(s(), "Site name."),
                }),
                &["title", "description", "image", "site"],
            ),
        }),
        json!({
            "method": "post", "path": "/api/v1/links/click", "auth": "viewer", "status": "204",
            "request": obj(json!({ "post": d(i(), "Post id."), "url": d(s(), "A link stored for that post.") }), &["post", "url"]),
            "response": d(json!({ "type": "string" }), "No body. Meant for navigator.sendBeacon: the request must carry a Sec-Fetch-Site header of same-origin, same-site or none, otherwise 403. No CSRF token needed."),
        }),
        json!({
            "method": "post", "path": "/api/v1/links/seen", "auth": "member",
            "request": obj(json!({ "urls": d(arr(s()), "Links to look up; only the first 20 are used.") }), &["urls"]),
            "response": d(arr(obj(
                json!({ "url": s(), "title": d(s(), "Title of the topic where it was posted."), "topic_url": d(s(), "Topic path with #post-N.") }),
                &["url", "title", "topic_url"],
            )), "Where these links were already posted, in forums the member can read."),
        }),
        json!({
            "method": "get", "path": "/api/v1/uploads/counts", "auth": "anyone",
            "query": [q("keys", false, "Comma-separated upload keys, at most 100. Default empty.", s())],
            "response": arr(obj(json!({ "key": s(), "downloads": i() }), &["key", "downloads"])),
        }),
        json!({
            "method": "get", "path": "/api/v1/emoji", "auth": "anyone",
            "response": d(arr(obj(
                json!({
                    "name": s(), "url": s(),
                    "set": d(s(), "Plugin emoji only: the set it belongs to."),
                    "label": d(s(), "Plugin emoji only: display label."),
                }),
                &["name", "url"],
            )), "Custom emoji, then emoji and stickers from plugins. Cached publicly for 60 seconds."),
        }),
        json!({
            "method": "get", "path": "/api/v1/quotes/deleted", "auth": "viewer",
            "query": [q("refs", false, "Comma-separated `topic:post_number` pairs, at most 100 (and at most 20 distinct topics are checked). Default empty.", s())],
            "response": d(arr(s()), "The requested `topic:post_number` pairs whose posts are gone, in readable topics only."),
        }),
        json!({
            "method": "get", "path": "/api/v1/users/{name}/card", "auth": "viewer",
            "response": user_card(),
        }),
        json!({
            "method": "get", "path": "/api/v1/online", "auth": "anyone",
            "query": [q("names", false, "Comma-separated usernames, at most 100. Default empty.", s())],
            "response": d(arr(s()), "The usernames among those given who were seen in the last 15 minutes and do not hide their last-seen time."),
        }),
        json!({
            "method": "get", "path": "/api/v1/live", "auth": "viewer",
            "query": [q("channels", true, "Comma-separated channels, at most 10: global; user (members); conversation:ID (members, participants); chat:ID; item:ID (a readable topic); forums (every readable forum's activity); forum:ID. An unknown or forbidden channel is refused. Send Last-Event-ID to resume.", s())],
            "response": d(s(), "Not JSON: a server-sent events stream (text/event-stream). Each event is named by its type and its data is {\"channel\": string, \"data\": object}; the connection ends when the viewer loses access."),
        }),
    ]
}
