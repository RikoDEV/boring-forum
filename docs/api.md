# The JSON API

Everything the site does for a signed-in member can be done over a JSON API under `/api/v1`.
The admin panel uses the same API (under `/api/v1/admin`). It is described by an OpenAPI
document, which is the reference for routes, parameters and responses:

- `GET /api/v1/openapi.json` on any running forum.
- `boring-forum openapi` prints the same document without starting the server.
- `web/api` in the repository holds a generated TypeScript client.

This page covers what the document does not repeat: how to authenticate and how requests behave.

## Authentication

A request is made as a member, in one of two ways.

**A session cookie**, as the site's own pages use. Writes also send the CSRF token in an
`X-CSRF-Token` header.

**An API key**, for scripts and apps. Send it as a bearer token:

```sh
curl -H "Authorization: Bearer bf_…" https://forum.example.com/api/v1/me
```

`GET /api/v1/me` answers who the key belongs to and what it may do, which makes it a good first
call.

### Creating a key

A member creates a key from a signed-in session with `POST /api/v1/me/api-keys` (and confirms their
password first). The token is **shown once**; only a digest is kept, so a lost key is replaced, not
recovered. List and revoke keys with `GET /api/v1/me/api-keys` and
`DELETE /api/v1/me/api-keys/{id}`.

```json
{ "name": "nightly export", "scope": "read", "expires_days": 30,
  "resources": ["read:topics", "read:search"] }
```

- **`scope`** is `read`, `write` or `admin`. A key can never do more than the member who made it;
  `admin` is for administrators only.
- **`expires_days`** is 1 to 365 and defaults to 90. An `admin` key lasts at most 30 days.
- **`resources`** narrows the key to parts of the API, as `read:<resource>` or
  `write:<resource>`. The resources are `topics`, `posts`, `users`, `search`, `notifications`,
  `messages`, `chat`, `uploads` and `bookmarks`. A key with none is not narrowed.
- **`plugin_scopes`** grants scopes that installed plugins declare, as `plugin:<id>:<key>`.

A key cannot create other keys.

## MCP server

`POST /mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server (Streamable HTTP,
stateless, one JSON answer per request), built into the forum, so an AI assistant can read it and,
for an administrator, manage it. Point a client at `https://forum.example.com/mcp`, with the key as a
bearer token. It is on by default; **Admin, Settings, MCP server** has the switches.

**Reading** (`mcp_enabled`). With no key the assistant reads what a guest can; with
`Authorization: Bearer bf_…` it reads as that member, and the key's scope and `resources` apply as
they do for the API. Cookies are never used, so a browser session cannot act through `/mcp`. The tools
are `whoami`, `list_forums`, `list_topics`, `get_topic`, `get_post`, `search`, `get_user`,
`user_activity` and `related_topics`. Each is one `GET` of the JSON API, run inside the server, so
permissions, private forums and rate limits are the API's own.

**Posting** (`mcp_posting_enabled`, **off** by default). A member's key with the `write` scope also gets
`create_topic` and `reply_to_topic`, which post as that member. They use the forum's own forms, so
everything that applies to a post made by hand applies here: what the member may post where, the daily
and new-member limits, spam screening, locked topics and the queue for a first post (the tool then
says it is waiting for a moderator). A key limited to resources needs `write:topics` or `write:posts`.
Posting twice posts twice; there is no idempotency key for it.

**Managing** (`mcp_admin_enabled`, **off** until an administrator turns it on). A key with the `admin`
scope, or the `BF_ADMIN_TOKEN`, also gets three tools for the admin API:

| Tool | Does |
| --- | --- |
| `admin_routes` | lists the admin API's routes (method, path, what it does), filtered by a word |
| `admin_get` | reads one of them: users, settings, forums, reports, logs, statistics, ... |
| `admin_write` | `POST`, `PUT`, `PATCH` or `DELETE` one of them |

The agent finds a route, reads it to see its shape, then writes. Nothing here widens the API: the key
is passed to it, and the API's own checks decide, including the two-factor rule for administrators.
On top of that:

- Never available through MCP: the shell, impersonating a member, the SQL explorer's run, installing
  plugins and themes, restoring or linking a backup, the exports of the site and of a person, importing
  settings, and managing a member's API keys.
- An agent cannot change `mcp_enabled` or `mcp_admin_enabled`.
- Every write, successful or refused, is a line in the audit log (`mcp.admin_write`, with the route,
  the method and the status), under the administrator who owns the key.
- Answers have anything named like a secret, token or password hidden before they reach the model.
- An `admin` key lasts at most 30 days. Give an agent its own key, and revoke it when done.

A refused call comes back as a tool error such as `404 Not found`. A request that names an `Origin`
other than the forum's own is refused, as the protocol asks. 
## How requests behave

**Errors** are `application/problem+json`, with `type`, `title`, `status` and usually a
`detail` in the reader's language:

```json
{ "type": "not-found", "title": "Not found", "status": 404 }
```

**Writes are safe to retry.** Send an `Idempotency-Key` header (1 to 255 visible ASCII
characters) with any write. The first answer is stored for 24 hours and repeated for the same key
and the same request. Reusing a key with a different request is `422`; sending it again while the
first is still running is `409`.

**Reads can be cached.** Responses carry an `ETag`. Send it back as `If-None-Match` and an
unchanged resource answers `304` with no body.

**Rate limits.** Requests are limited per client address (`BF_RATE_LIMIT_PER_MIN`, 600 by
default), and some actions have their own limits. A refused request is `429` with a
`Retry-After` header and the IETF `RateLimit` and `RateLimit-Policy` header fields saying when
to try again.

## Webhooks

Webhooks tell another service what happens on the forum. Add one in **Admin, Webhooks and apps**
with an `https` address, the events to send (or `*` for all) and, if you like, the forums to limit
it to. The forum generates the signing secret and shows it **once**, when the webhook is created:

| Event | When |
| --- | --- |
| `topic.created` | a topic is started |
| `post.created` | a reply is posted |
| `post.edited` | a post is edited |
| `post.deleted` | a post is deleted |
| `topic.moved` | a topic is moved to another forum |
| `topic.deleted` | a topic is deleted |
| `user.registered` | a member signs up |
| `moderation.action` | a moderator acts |

Private events (messages, chat, sign-ins, erasure) are never sent through a webhook.

Each delivery is a `POST` with a JSON body, `{ "event": …, "delivery": …, "data": … }`, and these
headers:

| Header | Holds |
| --- | --- |
| `X-BF-Event` | the event name |
| `X-BF-Delivery` | the delivery id |
| `X-BF-Timestamp` | the send time, in Unix seconds |
| `X-BF-Signature` | `t=<timestamp>,sha256=<hex>` |

**Verify the signature.** It is an HMAC-SHA256, keyed with your secret, over the timestamp, a dot
and the raw body. Recompute it and compare in constant time, and refuse a timestamp more than about
five minutes from your own clock, so a captured delivery cannot be replayed later:

```python
import hashlib, hmac, time

def valid(secret: str, timestamp: str, body: bytes, header: str) -> bool:
    mac = hmac.new(secret.encode(), timestamp.encode() + b"." + body, hashlib.sha256).hexdigest()
    expected = f"t={timestamp},sha256={mac}"
    return hmac.compare_digest(expected, header) and abs(time.time() - int(timestamp)) < 300
```

A delivery that does not answer with a `2xx` within ten seconds is retried with growing delays
(2, 4, 8 minutes and so on) up to eight attempts, then dropped. Redirects are not followed, and
addresses that point at internal networks are refused.

## Other formats

- **Feeds**: the latest topics, each forum, each topic and each tag are available as RSS 2.0, Atom
  1.0 and JSON Feed 1.1 (`/latest.rss`, `/c/{id}/feed.atom`, `/t/{id}/feed.json`, …).
- **`/sitemap.xml`**, `robots.txt`, `/llms.txt` and `/.well-known/security.txt` for crawlers.
- **ActivityPub**, when federation is on ([Features and settings](features.md#federation)).
