use serde_json::{Value, json};

fn s() -> Value {
    json!({ "type": "string" })
}

fn int() -> Value {
    json!({ "type": "integer", "format": "int64" })
}

/// A chat message as every message endpoint returns it.
fn message() -> Value {
    json!({
        "type": "object",
        "required": ["id", "channel_id", "user_id", "username", "raw", "cooked", "reply_to_id", "reply_author",
                     "reply_text", "pinned", "edited", "deleted", "created_at", "reactions"],
        "properties": {
            "id": int(),
            "channel_id": int(),
            "user_id": { "type": ["integer", "null"], "format": "int64", "description": "Null when the author's account is gone." },
            "username": { "type": ["string", "null"] },
            "raw": { "type": "string", "description": "Markdown source; empty for a deleted message." },
            "cooked": { "type": "string", "description": "Rendered HTML; empty for a deleted message." },
            "reply_to_id": { "type": ["integer", "null"], "format": "int64" },
            "reply_author": { "type": ["string", "null"], "description": "Username of the replied-to message's author." },
            "reply_text": { "type": ["string", "null"], "description": "First 120 characters of the replied-to message; null when it was deleted." },
            "pinned": { "type": "boolean" },
            "edited": { "type": "boolean" },
            "deleted": { "type": "boolean" },
            "created_at": { "type": "string", "format": "date-time" },
            "reactions": {
                "type": "array",
                "description": "One entry per emoji, ordered by emoji.",
                "items": {
                    "type": "object",
                    "required": ["emoji", "count", "users"],
                    "properties": {
                        "emoji": s(),
                        "count": { "type": "integer" },
                        "users": { "type": "array", "items": s(), "description": "Usernames who reacted." }
                    }
                }
            }
        }
    })
}

fn held() -> Value {
    json!({
        "type": "object",
        "required": ["held", "message"],
        "description": "Returned with status 202 instead of the normal body when a watched-word rule holds the content for moderator approval.",
        "properties": { "held": { "type": "boolean", "const": true }, "message": s() }
    })
}

fn one_of_held(ok: Value) -> Value {
    json!({ "oneOf": [ok, held()], "description": "The normal body, or (HTTP 202) the held-for-approval notice." })
}

fn id_only() -> Value {
    json!({ "type": "object", "required": ["id"], "properties": { "id": int() } })
}

pub(super) fn docs() -> Vec<Value> {
    let on = "Needs the chat feature enabled (otherwise 404). ";
    let see = "A channel the viewer may not see (invite-only or direct message they are not in, group-restricted without a matching group) answers 404, the same as an unknown id. ";
    let write = "Silenced members are refused (403) and archived channels reject with a validation error. ";
    vec![
        json!({
            "method": "get", "path": "/api/v1/chat/channels", "auth": "member",
            "description": format!("{on}Every channel the viewer can open: public channels they may read (open, or group-restricted to a group they are in; admins see all) and channels they have joined, including direct messages. Ordered by kind (public first), then name."),
            "response": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "kind", "name", "description", "archived", "access", "created_by", "joined", "muted", "unread", "people"],
                    "properties": {
                        "id": int(),
                        "kind": { "type": "string", "enum": ["public", "dm"] },
                        "name": s(),
                        "description": s(),
                        "archived": { "type": "boolean" },
                        "access": { "type": "string", "enum": ["open", "groups", "invite"] },
                        "created_by": { "type": ["integer", "null"], "format": "int64" },
                        "joined": { "type": "boolean", "description": "Whether the viewer is a member." },
                        "muted": { "type": "boolean" },
                        "unread": { "type": "integer", "format": "int64", "description": "Messages after the viewer's read marker, excluding their own and deleted ones; 0 when not joined." },
                        "people": { "type": "array", "items": s(), "description": "For a direct message, the other participants' usernames; empty otherwise." }
                    }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels", "auth": "member", "status": "201",
            "description": format!("{on}Create an invite-only channel; the creator becomes a member. Requires CSRF. Members need trust level `chat_create_min_trust_level` (staff are exempt); silenced members get 403. The account must be ready (e.g. confirmed). Name and description go through the watched-word list, which may block (validation error) or hold (202 `held`) them."),
            "request": {
                "type": "object", "required": ["name"],
                "properties": {
                    "name": { "type": "string", "minLength": 1, "maxLength": 60, "description": "Trimmed; 1 to 60 characters." },
                    "description": { "type": "string", "default": "" }
                }
            },
            "response": one_of_held(id_only())
        }),
        json!({
            "method": "get", "path": "/api/v1/chat/channels/{id}/members", "auth": "member",
            "description": format!("{on}Member list of an invite-only public channel, for its creator or staff. Other channel kinds answer a validation error; other members get 403. {see}"),
            "response": {
                "type": "array",
                "items": {
                    "type": "object", "required": ["id", "username"],
                    "properties": { "id": int(), "username": s() }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/members", "auth": "member", "status": "204",
            "description": format!("{on}Add a member to an invite-only channel; only its creator or staff. Requires CSRF. {write}Fails when the username does not exist or the person ignores the viewer. The added member is notified. {see}"),
            "request": {
                "type": "object", "required": ["username"],
                "properties": { "username": { "type": "string", "description": "Case-insensitive; a leading @ is accepted." } }
            }
        }),
        json!({
            "method": "delete", "path": "/api/v1/chat/channels/{id}/members/{user}", "auth": "member", "status": "204",
            "description": format!("{on}Remove a member (path `user` is the numeric user id) from an invite-only channel; only its creator or staff. Requires CSRF. The creator cannot be removed (validation error). {see}")
        }),
        json!({
            "method": "get", "path": "/api/v1/chat/me", "auth": "member",
            "description": format!("{on}The viewer's chat state and upload allowances."),
            "response": {
                "type": "object",
                "required": ["id", "ignored", "uploads", "upload_types", "max_files"],
                "properties": {
                    "id": int(),
                    "ignored": { "type": "array", "items": s(), "description": "Usernames the viewer ignores." },
                    "uploads": { "type": "boolean", "description": "Whether the attach button should show for this viewer." },
                    "upload_types": { "type": "array", "items": s(), "description": "File kinds chat accepts." },
                    "max_files": { "type": "integer", "description": "Files per message; 20 for staff, else `chat_max_files_per_message`." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/dm", "auth": "member",
            "description": format!("{on}Open (or reuse) a direct message with 1 to `chat_max_dm_users` other members. Requires CSRF. Silenced members get 403; non-staff need trust level `chat_dm_min_trust_level`. Fails when no listed member exists or one of them ignores the viewer."),
            "request": {
                "type": "object", "required": ["usernames"],
                "properties": {
                    "usernames": { "type": "array", "items": s(), "minItems": 1, "description": "Usernames, case-insensitive, a leading @ is accepted. The viewer is added automatically." }
                }
            },
            "response": id_only()
        }),
        json!({
            "method": "get", "path": "/api/v1/chat/channels/{id}/messages", "auth": "member",
            "description": format!("{on}A page of at most 50 messages, newest first (descending id). Page back by passing the smallest id seen as `before`. {see}"),
            "query": [
                { "name": "before", "required": false, "description": "Cursor: only messages with an id lower than this.", "schema": int() },
                { "name": "q", "required": false, "description": "Case-insensitive substring search in message text (deleted messages never match). Blank means no search.", "schema": { "type": "string", "default": "" } },
                { "name": "pinned", "required": false, "description": "Only pinned, non-deleted messages.", "schema": { "type": "boolean", "default": false } }
            ],
            "response": { "type": "array", "maxItems": 50, "items": message() }
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/messages", "auth": "member", "status": "201",
            "description": format!("{on}Send a message, joining the channel if needed. Requires CSRF. {write}{see}Message must be 1 to `chat_max_message_length` characters. Non-staff are limited by trust level `chat_min_trust_level`, file-attachment rules, `chat_messages_per_minute`, `chat_wait_seconds`, a duplicate window, new-member link limits and spam checks (validation errors). Blocked watched words are refused; \"require approval\" words hold the message (202 `held`)."),
            "request": {
                "type": "object", "required": ["raw"],
                "properties": {
                    "raw": { "type": "string", "description": "Markdown text." },
                    "reply_to": { "type": ["integer", "null"], "format": "int64", "description": "Id of a message in the same channel; a reply to any other message is dropped." }
                }
            },
            "response": one_of_held(message())
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/join", "auth": "member", "status": "204",
            "description": format!("{on}Join a channel the viewer can see. Requires CSRF. Cannot admit anyone to a channel they may not see. {see}")
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/leave", "auth": "member", "status": "204",
            "description": format!("{on}Leave a channel. Requires CSRF. {see}")
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/mute", "auth": "member", "status": "204",
            "description": format!("{on}Mute or unmute a channel for the viewer (a muted channel sends no unread badge updates). Requires CSRF. {see}"),
            "request": {
                "type": "object", "required": ["muted"],
                "properties": { "muted": { "type": "boolean" } }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/channels/{id}/read", "auth": "member", "status": "204",
            "description": format!("{on}Mark the channel read up to a message. Requires CSRF. {see}"),
            "request": {
                "type": "object", "required": ["id"],
                "properties": { "id": { "type": "integer", "format": "int64", "description": "Id of the last message read." } }
            }
        }),
        json!({
            "method": "put", "path": "/api/v1/chat/messages/{id}", "auth": "member",
            "description": format!("{on}Edit the viewer's own message (others' and deleted messages: 403). Requires CSRF. {write}Same length, word-list and filter checks as sending; a held edit answers 202 `held`. Returns the updated message and publishes it to the channel. Unseen messages answer 404."),
            "request": {
                "type": "object", "required": ["raw"],
                "properties": { "raw": { "type": "string", "description": "New Markdown text, 1 to `chat_max_message_length` characters." } }
            },
            "response": one_of_held(message())
        }),
        json!({
            "method": "delete", "path": "/api/v1/chat/messages/{id}", "auth": "member",
            "description": format!("{on}Delete a message: its author or site staff only (else 403). Requires CSRF. Returns the message as now stored (deleted, with empty text). Unseen messages answer 404."),
            "response": message()
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/messages/{id}/react", "auth": "member",
            "description": format!("{on}Toggle the viewer's reaction with an emoji (a second call removes it). Requires CSRF. {write}Deleted messages and non-emoji text (over 8 characters, or letters/digits other than keycaps) are refused with a validation error. Returns the updated message."),
            "request": {
                "type": "object", "required": ["emoji"],
                "properties": { "emoji": { "type": "string", "maxLength": 8, "description": "A single emoji; trimmed." } }
            },
            "response": message()
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/messages/{id}/pin", "auth": "member",
            "description": format!("{on}Toggle a message's pinned state. Staff only (403 for other members). Requires CSRF. No request body is read. Returns the updated message."),
            "response": message()
        }),
        json!({
            "method": "post", "path": "/api/v1/chat/messages/{id}/flag", "auth": "member", "status": "204",
            "description": format!("{on}Report a message to staff. Requires CSRF. Your own and deleted messages cannot be flagged (validation error). Reviewers are notified; once flags reach `flag_hide_score` the message is hidden until staff decide, and an author with several hidden messages in a day may be silenced."),
            "request": {
                "type": "object",
                "properties": { "reason": { "type": "string", "default": "", "maxLength": 500, "description": "Trimmed and cut to 500 characters." } }
            }
        }),
    ]
}
