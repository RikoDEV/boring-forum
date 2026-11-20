//! Calls into the forum. Each needs a capability in the plugin's manifest, approved by an
//! administrator; without it the call returns [`Error::Denied`].
//!
//! Off WebAssembly (when you unit-test plugin logic natively) every call returns
//! [`Error::Unavailable`].

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The capability for this call was not approved, or a per-call limit was used up.
    Denied,
    /// The host refused the request, with its reason.
    Failed(String),
    /// Not running inside the forum.
    Unavailable,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => f.write_str("not allowed"),
            Self::Failed(why) => f.write_str(why),
            Self::Unavailable => f.write_str("not running in the forum"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(target_arch = "wasm32")]
mod raw {
    #[link(wasm_import_module = "bf")]
    unsafe extern "C" {
        pub fn log(ptr: *const u8, len: i32);
        pub fn kv_get(ptr: *const u8, len: i32) -> i64;
        pub fn kv_set(kp: *const u8, kl: i32, vp: *const u8, vl: i32) -> i32;
        pub fn kv_delete(ptr: *const u8, len: i32) -> i32;
        pub fn kv_keys(ptr: *const u8, len: i32) -> i64;
        pub fn request(ptr: *const u8, len: i32) -> i64;
        pub fn http_get(ptr: *const u8, len: i32) -> i64;
    }
}

/// Bytes the host wrote into this module's memory: `(ptr << 32) | len`.
#[cfg(target_arch = "wasm32")]
fn take(r: i64) -> Result<Option<Vec<u8>>, Error> {
    match r {
        -1 => Err(Error::Denied),
        0 => Ok(None),
        r => {
            let (ptr, len) = ((r >> 32) as usize, (r & 0xffff_ffff) as usize);
            // SAFETY: the host just copied `len` bytes to `ptr`, memory it got from our `alloc`.
            Ok(Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) }.to_vec()))
        }
    }
}

/// Writes a line to the server log (`info`, with the plugin's id).
pub fn log(message: &str) {
    #[cfg(target_arch = "wasm32")]
    // SAFETY: the pointer and length describe `message`, which lives through the call.
    unsafe {
        raw::log(message.as_ptr(), message.len() as i32);
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = message;
}

/// Capability `kv`: the plugin's private key-value store (values up to 64 KB).
pub fn kv_get(key: &str) -> Result<Option<String>, Error> {
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: as in `log`.
        let r = unsafe { raw::kv_get(key.as_ptr(), key.len() as i32) };
        Ok(take(r)?.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Err(Error::Unavailable)
    }
}

pub fn kv_set(key: &str, value: &str) -> Result<(), Error> {
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: as in `log`.
        let r = unsafe { raw::kv_set(key.as_ptr(), key.len() as i32, value.as_ptr(), value.len() as i32) };
        if r == 0 { Ok(()) } else { Err(Error::Denied) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
        Err(Error::Unavailable)
    }
}

pub fn kv_delete(key: &str) -> Result<(), Error> {
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: as in `log`.
        let r = unsafe { raw::kv_delete(key.as_ptr(), key.len() as i32) };
        if r == 0 { Ok(()) } else { Err(Error::Denied) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Err(Error::Unavailable)
    }
}

/// Keys that start with `prefix`.
pub fn kv_keys(prefix: &str) -> Result<Vec<String>, Error> {
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: as in `log`.
        let r = unsafe { raw::kv_keys(prefix.as_ptr(), prefix.len() as i32) };
        Ok(take(r)?.and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = prefix;
        Err(Error::Unavailable)
    }
}

/// `bf.request`: a JSON operation (`{"op": "...", ...}`), answered `{"ok": ...}` or `{"error": ...}`.
/// The helpers below build the usual ones; `op`s are listed in `docs/plugins/abi.md`.
pub fn request(op: &Value) -> Result<Value, Error> {
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = serde_json::to_vec(op).map_err(|e| Error::Failed(e.to_string()))?;
        // SAFETY: as in `log`.
        let r = unsafe { raw::request(bytes.as_ptr(), bytes.len() as i32) };
        let answer: Value =
            serde_json::from_slice(&take(r)?.unwrap_or_default()).map_err(|e| Error::Failed(e.to_string()))?;
        match answer.get("error").and_then(Value::as_str) {
            Some(e) => Err(Error::Failed(e.to_owned())),
            None => Ok(answer.get("ok").cloned().unwrap_or(Value::Null)),
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = op;
        Err(Error::Unavailable)
    }
}

/// Capability `http:<host>`: GET a page (at most 3 a call, 64 KB, 5 seconds). `Ok(None)` when the
/// request failed; [`Error::Denied`] for a host that was not approved.
pub fn http_get(url: &str) -> Result<Option<Vec<u8>>, Error> {
    #[cfg(target_arch = "wasm32")]
    {
        // SAFETY: as in `log`.
        take(unsafe { raw::http_get(url.as_ptr(), url.len() as i32) })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
        Err(Error::Unavailable)
    }
}

/// `http_get` for JSON answers.
pub fn http_get_json(url: &str) -> Result<Option<Value>, Error> {
    Ok(http_get(url)?.and_then(|b| serde_json::from_slice(&b).ok()))
}

/// Capability `tables`: rows of a table the manifest declares. `filter` is `{column: value}`.
pub fn db_select(table: &str, filter: &Value, order: Option<&str>, limit: i64) -> Result<Vec<Value>, Error> {
    let mut op = json!({ "op": "db", "action": "select", "table": table, "where": filter, "limit": limit });
    if let Some(order) = order {
        op["order"] = json!(order);
    }
    Ok(request(&op)?.as_array().cloned().unwrap_or_default())
}

pub fn db_insert(table: &str, values: &Value) -> Result<(), Error> {
    request(&json!({ "op": "db", "action": "insert", "table": table, "values": values })).map(drop)
}

pub fn db_update(table: &str, filter: &Value, values: &Value) -> Result<(), Error> {
    request(&json!({ "op": "db", "action": "update", "table": table, "where": filter, "values": values })).map(drop)
}

pub fn db_delete(table: &str, filter: &Value) -> Result<(), Error> {
    request(&json!({ "op": "db", "action": "delete", "table": table, "where": filter })).map(drop)
}

/// Capability `tables`: `select` with every option: `where` (a value, or an object of `eq`, `ne`,
/// `gt`, `gte`, `lt`, `lte`, `in`, `prefix`, `contains`, `isnull`), `order` (a column, or a list of
/// up to three with `-` for descending), `limit` (500) and `offset`.
pub fn db_query(table: &str, options: &Value) -> Result<Vec<Value>, Error> {
    let mut op = options.clone();
    op["op"] = json!("db");
    op["action"] = json!("select");
    op["table"] = json!(table);
    Ok(request(&op)?.as_array().cloned().unwrap_or_default())
}

/// Capability `tables`: how many rows match `filter` (the filters of [`db_query`]).
pub fn db_count(table: &str, filter: &Value) -> Result<i64, Error> {
    let out = request(&json!({ "op": "db", "action": "count", "table": table, "where": filter }))?;
    Ok(out.get(0).and_then(|r| r.get("count")).and_then(Value::as_i64).unwrap_or_default())
}

/// Capability `tables`: inserts a row and returns its id.
pub fn db_insert_id(table: &str, values: &Value) -> Result<i64, Error> {
    let out = request(&json!({ "op": "db", "action": "insert", "table": table, "values": values }))?;
    Ok(out.get(0).and_then(|r| r.get("id")).and_then(Value::as_i64).unwrap_or_default())
}

/// Capability `files` and `http:<host>`: stores what the host at `url` serves as `path` (up to
/// 8 MB). `content_type` is yours to name, from the allowed list.
pub fn file_fetch(url: &str, path: &str, content_type: &str) -> Result<String, Error> {
    let out = request(&json!({ "op": "file.fetch", "url": url, "path": path, "content_type": content_type }))?;
    Ok(out.get("url").and_then(Value::as_str).unwrap_or_default().to_owned())
}

/// Capability `content_types`: offers an entry of a declared searchable `[[content_types]]` type to
/// site search (a title, plain text, a path on this site). Index again to update.
pub fn search_index(
    kind: &str,
    reference: &str,
    title: &str,
    body: &str,
    url: &str,
    author: Option<&str>,
) -> Result<(), Error> {
    let mut op =
        json!({ "op": "search.index", "type": kind, "ref": reference, "title": title, "body": body, "url": url });
    if let Some(author) = author {
        op["author"] = json!(author);
    }
    request(&op).map(drop)
}

/// Capability `content_types`: takes an entry out of site search.
pub fn search_remove(kind: &str, reference: &str) -> Result<(), Error> {
    request(&json!({ "op": "search.remove", "type": kind, "ref": reference })).map(drop)
}

/// Capability `content_types`: holds an entry for a moderator (`/review/apps`); the decision comes
/// back as the `content.review` hook. `false` when it already waits or the plugin has too many waiting.
pub fn review_hold(
    kind: &str,
    reference: &str,
    title: &str,
    excerpt: &str,
    url: &str,
    user_id: Option<i64>,
) -> Result<bool, Error> {
    let mut op =
        json!({ "op": "review.hold", "type": kind, "ref": reference, "title": title, "excerpt": excerpt, "url": url });
    if let Some(user_id) = user_id {
        op["user_id"] = json!(user_id);
    }
    Ok(request(&op)?.get("held").and_then(Value::as_bool).unwrap_or(false))
}

/// Capability `content_types`: withdraws a waiting entry (its author deleted it).
pub fn review_withdraw(kind: &str, reference: &str) -> Result<(), Error> {
    request(&json!({ "op": "review.withdraw", "type": kind, "ref": reference })).map(drop)
}

/// Capability `content_types`: whether a member holds the permission of a declared content type.
pub fn content_can(user_id: i64, kind: &str) -> Result<bool, Error> {
    let out = request(&json!({ "op": "content.can", "user_id": user_id, "type": kind }))?;
    Ok(out.get("allowed").and_then(Value::as_bool).unwrap_or(false))
}

/// Capability `content.read`: a topic a guest can read, with its posts (cooked HTML). A topic the
/// plugin owns is readable whatever its forum allows; the answer says `owned`, `state` and `node_id`.
pub fn content_read(topic: i64, from: i32, limit: i32) -> Result<Value, Error> {
    request(&json!({ "op": "content.read", "topic": topic, "from": from, "limit": limit }))
}

/// Capability `content.write`: replies in a topic as the plugin's own account.
pub fn content_reply(topic: i64, body: &str) -> Result<i64, Error> {
    let out = request(&json!({ "op": "content.reply", "topic": topic, "body": body }))?;
    Ok(out.get("number").and_then(Value::as_i64).unwrap_or_default())
}

/// Capability `jobs`: runs `job.run` with `payload` after `delay` seconds (0 to 86400).
pub fn enqueue_job(payload: &Value, delay_seconds: i64) -> Result<(), Error> {
    request(&json!({ "op": "job", "payload": payload, "delay": delay_seconds })).map(drop)
}

/// Capability `notify`: a notification of one of the plugin's own `[[notification_kinds]]`, which
/// members can switch off in their settings.
pub fn notify_kind(user_ids: &[i64], kind: &str, text: &str, url: &str) -> Result<(), Error> {
    request(&json!({ "op": "notify", "kind": kind, "user_ids": user_ids, "text": text, "url": url })).map(drop)
}

/// Capability `notify`: a notification to up to 50 members; `url` is a path on this site.
pub fn notify(user_ids: &[i64], text: &str, url: &str) -> Result<(), Error> {
    request(&json!({ "op": "notify", "user_ids": user_ids, "text": text, "url": url })).map(drop)
}

/// Capability `users.read`: `{id, username, display_name, trust_level, created_at, groups}`.
pub fn user_get(user_id: i64) -> Result<Value, Error> {
    request(&json!({ "op": "user.get", "user_id": user_id }))
}

/// Capability `users.read`: whether a member holds `perm` (`view`, `read`, `create`, `reply`,
/// `attach`, `moderate`) in a forum.
pub fn user_can(user_id: i64, node_id: i64, perm: &str) -> Result<bool, Error> {
    let out = request(&json!({ "op": "user.can", "user_id": user_id, "node_id": node_id, "perm": perm }))?;
    Ok(out.get("allowed").and_then(Value::as_bool).unwrap_or(false))
}

/// Capability `settings.read`: one public site setting, or `None`.
pub fn settings_get(key: &str) -> Result<Option<Value>, Error> {
    match request(&json!({ "op": "settings.get", "key": key })) {
        Ok(v) => Ok(v.get("value").cloned()),
        Err(Error::Failed(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Capability `jobs`: a recurring `job.run` every `every_minutes` (5 to 43200), carrying `payload`
/// and `"schedule": name`. Scheduling the same name again replaces it.
pub fn schedule(name: &str, every_minutes: i64, payload: &Value) -> Result<(), Error> {
    request(&json!({ "op": "schedule", "name": name, "every_minutes": every_minutes, "payload": payload })).map(drop)
}

/// A text of the plugin's own `locales/` in any language (no capability needed).
pub fn i18n_t(locale: &str, key: &str, vars: &Value) -> Result<String, Error> {
    let out = request(&json!({ "op": "i18n.t", "locale": locale, "key": key, "vars": vars }))?;
    Ok(out.get("text").and_then(Value::as_str).unwrap_or(key).to_owned())
}

/// Outbound HTTP with any method to a host the plugin holds `http:<host>` for. `request` takes
/// `{url, method?, headers?, json? | body?}` and answers `{status, body}`.
pub fn http_request(request_body: &Value) -> Result<Value, Error> {
    let mut op = request_body.clone();
    op["op"] = json!("http_request");
    request(&op)
}

/// Capability `profile_fields`: the answer to one of the plugin's own `[[profile_fields]]`.
pub fn profile_set(user_id: i64, key: &str, value: &str) -> Result<(), Error> {
    request(&json!({ "op": "profile.set", "user_id": user_id, "key": key, "value": value })).map(drop)
}

/// Capability `groups`: puts a member in one of the plugin's own `[[groups]]`.
pub fn group_add(user_id: i64, key: &str) -> Result<(), Error> {
    request(&json!({ "op": "group.add", "user_id": user_id, "key": key })).map(drop)
}

/// Capability `groups`: takes a member out of one of the plugin's own groups.
pub fn group_remove(user_id: i64, key: &str) -> Result<(), Error> {
    request(&json!({ "op": "group.remove", "user_id": user_id, "key": key })).map(drop)
}

/// Capability `badges`: gives a member one of the plugin's own `[[badges]]`. `true` when it was
/// new to them.
pub fn badge_grant(user_id: i64, key: &str) -> Result<bool, Error> {
    let out = request(&json!({ "op": "badge.grant", "user_id": user_id, "key": key }))?;
    Ok(out.get("granted").and_then(Value::as_bool).unwrap_or(false))
}

/// Capability `tags`: sets which of the plugin's own `[[tags]]` (by key) a topic has.
pub fn tags_set(topic_id: i64, keys: &[&str]) -> Result<(), Error> {
    request(&json!({ "op": "tags.set", "topic_id": topic_id, "keys": keys })).map(drop)
}

/// Capability `nodes`: a forum under one of the plugin's own `[[nodes]]` (named by `parent`);
/// returns its id.
pub fn nodes_create(parent: &str, title: &str, description: &str) -> Result<i64, Error> {
    let out = request(&json!({ "op": "nodes.create", "parent": parent, "title": title, "description": description }))?;
    Ok(out.get("id").and_then(Value::as_i64).unwrap_or_default())
}

/// Capability `meta`: the plugin's metadata on a `topic`, `post` or `user` (one key, or all).
pub fn meta_get(target: &str, id: i64, key: Option<&str>) -> Result<Value, Error> {
    let mut op = json!({ "op": "meta.get", "target": target, "id": id });
    if let Some(key) = key {
        op["key"] = json!(key);
    }
    Ok(request(&op)?.get("meta").cloned().unwrap_or(Value::Null))
}

/// Capability `meta`: sets one declared `[[meta]]` key (up to 8 KB of JSON); `Value::Null` removes it.
pub fn meta_set(target: &str, id: i64, key: &str, value: &Value) -> Result<(), Error> {
    request(&json!({ "op": "meta.set", "target": target, "id": id, "key": key, "value": value })).map(drop)
}

/// Capability `content.write`: a new topic in a forum, as the plugin's own account.
pub fn content_create_topic(node_id: i64, title: &str, body: &str) -> Result<Value, Error> {
    request(&json!({ "op": "content.create_topic", "node_id": node_id, "title": title, "body": body }))
}

/// Capability `content.write`: edits a post the plugin's own account wrote (the old text is kept
/// as a revision). `title` is for a topic's first post.
pub fn content_edit(
    topic: i64,
    number: i64,
    body: &str,
    reason: Option<&str>,
    title: Option<&str>,
) -> Result<(), Error> {
    let mut op = json!({ "op": "content.edit", "topic": topic, "number": number, "body": body });
    if let Some(reason) = reason {
        op["reason"] = json!(reason);
    }
    if let Some(title) = title {
        op["title"] = json!(title);
    }
    request(&op).map(drop)
}

/// Capability `files`: stores a text file, served at `/plugin-files/<id>/<path>`; returns its
/// address. A request holds 64 KB, so a bigger file (up to 1 MB) is sent in pieces with `append`.
pub fn file_put_text(path: &str, content_type: &str, text: &str, append: bool) -> Result<String, Error> {
    let out = request(
        &json!({ "op": "file.put", "path": path, "content_type": content_type, "text": text, "append": append }),
    )?;
    Ok(out.get("url").and_then(Value::as_str).unwrap_or_default().to_owned())
}

/// Capability `files`: stores binary data (`base64`).
pub fn file_put_base64(path: &str, content_type: &str, base64: &str, append: bool) -> Result<String, Error> {
    let out = request(
        &json!({ "op": "file.put", "path": path, "content_type": content_type, "base64": base64, "append": append }),
    )?;
    Ok(out.get("url").and_then(Value::as_str).unwrap_or_default().to_owned())
}

/// Capability `files`: a stored file as `{content_type, base64, size}`.
pub fn file_get(path: &str) -> Result<Value, Error> {
    request(&json!({ "op": "file.get", "path": path }))
}

/// Capability `files`: deletes a stored file; `true` when there was one.
pub fn file_delete(path: &str) -> Result<bool, Error> {
    let out = request(&json!({ "op": "file.delete", "path": path }))?;
    Ok(out.get("deleted").and_then(Value::as_bool).unwrap_or(false))
}

/// Capability `email`: queues an email to a member with a verified address, in their language
/// when `subject_key` and `text_key` name messages of the plugin's `locales/`.
pub fn email_send(user_id: i64, subject_key: &str, text_key: &str, vars: &Value) -> Result<(), Error> {
    request(&json!({ "op": "email.send", "user_id": user_id, "subject_key": subject_key, "text_key": text_key, "vars": vars }))
        .map(drop)
}

/// Capability `realtime`: pushes an event to members' open pages (`plugin.<id>.<event>`).
pub fn realtime_push(user_ids: &[i64], event: &str, data: &Value) -> Result<(), Error> {
    request(&json!({ "op": "realtime.push", "user_ids": user_ids, "event": event, "data": data })).map(drop)
}

/// No capability: the current time as `{unix, iso}`.
pub fn time_now() -> Result<Value, Error> {
    request(&json!({ "op": "time.now" }))
}

/// No capability: `n` random bytes (1 to 1024), as hex.
pub fn random_hex(n: u32) -> Result<String, Error> {
    let out = request(&json!({ "op": "random.bytes", "n": n }))?;
    Ok(out.get("hex").and_then(Value::as_str).unwrap_or_default().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_calls_are_unavailable_natively() {
        assert_eq!(kv_get("a"), Err(Error::Unavailable));
        assert_eq!(request(&json!({})), Err(Error::Unavailable));
        log("fine");
    }
}
