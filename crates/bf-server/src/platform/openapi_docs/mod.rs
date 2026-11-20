//! Typed request and response documentation for the member routes listed in `ROUTES`
//! (`openapi.rs`), written by reading each handler. One entry per `(method, path)`:
//!
//! ```text
//! { "method": "get", "path": "/api/v1/topics/{id}/card",
//!   "auth": "viewer",                       // "anyone" | "viewer" | "member": who the handler lets in
//!   "query": [ { "name": "n", "required": false, "description": "…", "schema": { "type": "integer" } } ],
//!   "request": { JSON Schema },             // body, when the route takes one
//!   "response": { JSON Schema },            // the 200 body
//!   "status": "200" }                       // optional; "201", "204" …
//! ```
//! `openapi::Documented` merges these over the generic rows.

mod ai_me;
mod auth_push;
mod chat;
mod content;

pub(super) fn all() -> Vec<serde_json::Value> {
    [content::docs(), ai_me::docs(), chat::docs(), auth_push::docs()].concat()
}
