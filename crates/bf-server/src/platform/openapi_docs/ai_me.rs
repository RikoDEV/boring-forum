use serde_json::{Value, json};

fn topic_card() -> Value {
    json!({
        "type": "object",
        "required": ["id", "title", "url", "forum", "score"],
        "properties": {
            "id": { "type": "integer", "format": "int64" },
            "title": { "type": "string" },
            "url": { "type": "string", "description": "Path of the topic page." },
            "forum": { "type": "string", "description": "Title of the forum the topic is in (empty when unknown)." },
            "score": { "type": "number", "description": "Cosine similarity, rounded to two decimals." }
        }
    })
}

fn saved_reply() -> Value {
    json!({
        "type": "object",
        "required": ["id", "title", "body"],
        "properties": {
            "id": { "type": "integer", "format": "int64" },
            "title": { "type": "string" },
            "body": { "type": "string" }
        }
    })
}

fn saved_body() -> Value {
    json!({
        "type": "object",
        "required": ["title", "body"],
        "properties": {
            "title": { "type": "string", "maxLength": 60, "description": "Not blank." },
            "body": { "type": "string", "maxLength": 10000, "description": "Not blank." }
        }
    })
}

fn draft_context() -> Value {
    json!({
        "name": "context",
        "required": true,
        "description": "Where the draft belongs: `new-topic:{forum id}` or `reply:{topic id}`. One draft per place and member. (The query parameter is `context`, not `key`.)",
        "schema": { "type": "string", "pattern": "^(new-topic|reply):[0-9]{1,18}$" }
    })
}

pub(super) fn docs() -> Vec<Value> {
    vec![
        json!({
            "method": "get", "path": "/api/v1/topics/{id}/ai-summary", "auth": "viewer",
            "description": "The stored summary of a topic for the viewer's language, for anyone who can read the topic. Makes nothing and costs nothing. Returns 404 when AI is not configured or summaries are off, and 204 with no body when no summary exists yet. `stale` is true once enough posts came after it (fewer than max(3, up_to/10) new posts is fresh).",
            "response": {
                "type": "object",
                "required": ["summary", "up_to", "stale"],
                "properties": {
                    "summary": { "type": "string" },
                    "up_to": { "type": "integer", "description": "Post number the summary covers up to." },
                    "stale": { "type": "boolean" }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/topics/{id}/ai-summary", "auth": "viewer",
            "description": "Summarize a topic. Without `since` it returns the stored summary when still fresh (guests may get it) and otherwise generates and stores a new one, which needs a signed-in member. With `since: true` it summarizes only posts the member has not read (member only; nothing is stored). Returns 404 when AI is not configured or summaries are off; 422 when the topic has fewer replies than the site minimum, when everything is read, or when the forum is private and `ai_public_only` is on. The body is optional.",
            "request": {
                "type": "object",
                "properties": {
                    "since": { "type": "boolean", "default": false, "description": "Only what the member has not read yet." }
                }
            },
            "response": {
                "type": "object",
                "required": ["summary", "cached"],
                "properties": {
                    "summary": { "type": "string" },
                    "cached": { "type": "boolean" },
                    "up_to": { "type": "integer", "description": "Post number the summary covers up to (absent when `since` was used)." },
                    "since": { "type": "integer", "description": "Last post number the member had read (only when `since` was used)." },
                    "posts": { "type": "integer", "description": "How many unread posts were summarized (only when `since` was used)." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/posts/{id}/ai-translate", "auth": "member",
            "description": "Machine-translate a post into a language and store the result. Returns 404 when AI translation or content translation is off, or the post is not visible to the viewer; 422 for an unsupported language or a private forum with `ai_public_only`. A stored translation is returned without calling the provider. The body is optional.",
            "request": {
                "type": "object",
                "properties": {
                    "to": { "type": "string", "description": "Language code (en, pl, ar, ...). The member's own language when left out or empty." }
                }
            },
            "response": {
                "type": "object",
                "required": ["cooked", "machine", "cached"],
                "properties": {
                    "cooked": { "type": "string", "description": "The translated post as HTML." },
                    "machine": { "type": "boolean", "description": "True when the translation was made by machine." },
                    "cached": { "type": "boolean", "description": "True when it was already stored." }
                }
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/ai/similar", "auth": "member",
            "description": "Topics close in meaning to a title being typed, among forums the member can read, best first (at most 5, similarity at least 0.5). Returns an empty array when embeddings or related topics are off, or when `q` is shorter than 12 characters. Cached privately for 60 seconds.",
            "query": [
                { "name": "q", "required": false, "description": "The text to compare (a title). Fewer than 12 characters gives an empty list.", "schema": { "type": "string" } }
            ],
            "response": { "type": "array", "items": topic_card() }
        }),
        json!({
            "method": "post", "path": "/api/v1/ai/helper", "auth": "member",
            "description": "The composer writing helper; works on the member's own draft and changes nothing. Returns 404 when AI or the helper is off, 403 below the helper's minimum trust level (admins excepted), 422 when the text is empty or over 8000 characters or the forum is private with `ai_public_only`, and 400 for an unknown action. The response shape depends on `action`.",
            "request": {
                "type": "object",
                "required": ["action", "text"],
                "properties": {
                    "action": { "type": "string", "enum": ["proofread", "friendly", "formal", "shorten", "explain", "translate", "titles", "tags", "forum"] },
                    "text": { "type": "string", "maxLength": 8000, "description": "The draft; trimmed, must not be empty." },
                    "to": { "type": "string", "description": "Language code for `translate`; the member's language when unsupported or empty." },
                    "forum": { "type": ["integer", "null"], "format": "int64", "description": "The forum the draft is for, when known." },
                    "topic": { "type": ["integer", "null"], "format": "int64", "description": "The topic it replies to; takes precedence over `forum` for the privacy check." }
                }
            },
            "response": {
                "oneOf": [
                    {
                        "type": "object", "description": "For proofread, friendly, formal, shorten, explain and translate.",
                        "required": ["text"],
                        "properties": { "text": { "type": "string" } }
                    },
                    {
                        "type": "object", "description": "For `titles`: up to 3 suggestions.",
                        "required": ["titles"],
                        "properties": { "titles": { "type": "array", "items": { "type": "string" }, "maxItems": 3 } }
                    },
                    {
                        "type": "object", "description": "For `tags`: up to 5 suggestions.",
                        "required": ["tags"],
                        "properties": { "tags": { "type": "array", "items": { "type": "string" }, "maxItems": 5 } }
                    },
                    {
                        "type": "object", "description": "For `forum`: the suggested forum, null when the member can post in none or the model picked none.",
                        "required": ["forum"],
                        "properties": {
                            "forum": {
                                "type": ["object", "null"],
                                "properties": {
                                    "id": { "type": "integer", "format": "int64" },
                                    "title": { "type": "string" }
                                }
                            }
                        }
                    }
                ]
            }
        }),
        json!({
            "method": "put", "path": "/api/v1/me/infinite-scroll", "auth": "member", "status": "204",
            "description": "Turn infinite scroll on or off for the signed-in member. No response body.",
            "request": {
                "type": "object",
                "required": ["on"],
                "properties": { "on": { "type": "boolean" } }
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/saved-replies", "auth": "member",
            "description": "All saved replies (site-wide canned replies), ordered by title. Staff only: signed-in members who are not staff get 403.",
            "response": { "type": "array", "items": saved_reply() }
        }),
        json!({
            "method": "post", "path": "/api/v1/saved-replies", "auth": "member", "status": "201",
            "description": "Create a saved reply. Staff only (403 for other members). 422 when the title is blank or over 60 characters, or the body is blank or over 10000 characters.",
            "request": saved_body(),
            "response": {
                "type": "object",
                "required": ["id"],
                "properties": { "id": { "type": "integer", "format": "int64" } }
            }
        }),
        json!({
            "method": "put", "path": "/api/v1/saved-replies/{id}", "auth": "member", "status": "204",
            "description": "Replace a saved reply's title and body. Staff only (403 for other members). 404 when it does not exist; no response body.",
            "request": saved_body()
        }),
        json!({
            "method": "delete", "path": "/api/v1/saved-replies/{id}", "auth": "member", "status": "204",
            "description": "Delete a saved reply. Staff only (403 for other members). 404 when it does not exist; no response body."
        }),
        json!({
            "method": "get", "path": "/api/v1/drafts", "auth": "member",
            "description": "The member's stored composer draft for a place. 204 with no body when there is none; 400 for a malformed `context`. Never cached.",
            "query": [draft_context()],
            "response": {
                "type": "object",
                "required": ["data", "updated_at"],
                "properties": {
                    "data": { "type": "object", "description": "The draft exactly as it was stored (a JSON object)." },
                    "updated_at": { "type": "string", "format": "date-time" }
                }
            }
        }),
        json!({
            "method": "put", "path": "/api/v1/drafts", "auth": "member", "status": "204",
            "description": "Store (replace) the member's draft for a place. No response body. 400 for a malformed `context`; 422 when the body is not a JSON object or is over 200 KB.",
            "query": [draft_context()],
            "request": { "type": "object", "description": "Any JSON object, up to 200 KB serialized; the editor decides its fields.", "additionalProperties": true }
        }),
        json!({
            "method": "delete", "path": "/api/v1/drafts", "auth": "member", "status": "204",
            "description": "Discard the member's draft for a place. No response body; succeeds when there was none.",
            "query": [draft_context()]
        }),
        json!({
            "method": "post", "path": "/api/v1/uploads", "auth": "member", "status": "201",
            "description": "Upload one file as multipart/form-data, with the CSRF token in the X-CSRF-Token header. The type is decided from the file's leading bytes (PNG, JPEG, GIF, WebP, PDF, ZIP, plain .txt and the other types the site allows), never from its name. Needs the site's minimum trust level (staff excepted); silenced members get 403; 422 for an empty file, a type that is not allowed, a file over the size limit or daily quota; 429 when too many uploads are being read or the rate limit is hit. Images have location metadata stripped.",
            "query": [
                { "name": "for", "required": false, "description": "`chat` applies the chat upload rules (chat must allow uploads, and only types both the site and chat accept). Any other value means a post.", "schema": { "type": "string", "enum": ["chat"] } }
            ],
            "request": {
                "type": "object",
                "required": ["file"],
                "properties": {
                    "file": { "type": "string", "format": "binary", "description": "The file. Other form fields are ignored." }
                }
            },
            "response": {
                "type": "object",
                "required": ["url", "markdown", "filename", "size", "width", "height", "is_image"],
                "properties": {
                    "url": { "type": "string", "description": "Path of the stored file, /uploads/{sha256}.{ext}." },
                    "markdown": { "type": "string", "description": "Ready-to-insert Markdown: an image embed (with #WxH) or a link with the human size." },
                    "filename": { "type": "string", "description": "The cleaned name the file was uploaded under." },
                    "size": { "type": "integer", "format": "int64", "description": "Bytes stored (after metadata scrubbing)." },
                    "width": { "type": ["integer", "null"], "description": "Pixels; null for non-images." },
                    "height": { "type": ["integer", "null"], "description": "Pixels; null for non-images." },
                    "is_image": { "type": "boolean" }
                }
            }
        }),
    ]
}
