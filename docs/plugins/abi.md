# Plugin ABI reference

Host API `1.3` (`api = "^1.0"` in the manifest). This page is the contract; the code it describes is
`crates/bf-server/src/platform/plugins.rs` and the files beside it. The Rust SDK is
`crates/bf-plugin-api` (typed bindings for everything below); [getting-started.md](getting-started.md)
is the tutorial.

## Calling convention

A module exports:

| Export | Signature | Meaning |
| --- | --- | --- |
| `memory` | linear memory | The host reads and writes JSON here. |
| `alloc` | `(len: i32) -> i32` | Returns a pointer to `len` writable bytes. Called for the input, and by the host to hand answers back. |
| `handle` | `(ptr: i32, len: i32) -> i64` | Reads the request at `ptr`, returns `(out_ptr << 32) \| out_len`. |

A fresh instance runs for every call; nothing survives between calls except what a plugin stores
through `kv`, its tables, files or metadata.

**Compatibility (1.3).** Everything in 1.0 to 1.2 works unchanged: a plugin built for `^1.0`, `^1.1`
or `^1.2` still installs and runs. 1.3 only adds (the `pages` and `content_types` capabilities, the
`page.request` and `content.review` hooks, `[[routes]]` and `[[content_types]]`, the `count` action,
the `offset`, `order` list and `where` operators of `db`, `unique` and `float` for tables,
`file.fetch`, `search.*`, `review.*`, `content.can`, and the optional fields of `content.edit`).
Three existing behaviours widened and cannot break a plugin: `file.put` accepts 8 MB (was 1 MB),
1000 files (was 200) and more content types; `/plugin-files/` answers `Range`, `ETag` and sends
non-media types as a download; `content.read` and `content.edit` also work on topics the plugin
owns. A plugin that asks for `^1.3` is refused by a host older than 1.3, so it never calls an op
that does not exist. `sitemap.urls` entries may now be objects with a `lastmod`.

Request (UTF-8 JSON): `{"hook": "post.before_create", "data": {...}, "settings": {...}}`.
`settings` holds what an administrator saved from the plugin's settings and admin pages, with the
declared defaults filled in. The answer is JSON of at most 1 MB; its shape depends on the hook
(below). A module that cannot be linked (any import outside module `bf`, so no WASI) or traps is a
failed call.

### Imports (module `bf`)

All pointer and length arguments are `i32` into the module's own memory; any single argument or value
is limited to 64 KB. Results of `i64` type: `-1` means not allowed (the capability was not approved,
a per-call limit was used up, or the argument was unreadable), `0` means nothing, anything else is
`(ptr << 32) | len` of bytes the host wrote through your `alloc`.

| Import | Signature | Capability | Notes |
| --- | --- | --- | --- |
| `log` | `(ptr, len)` | none | Server log, level info, tagged with the plugin id. |
| `kv_get` | `(ptr, len) -> i64` | `kv` | Value as bytes; `0` when unset. |
| `kv_set` | `(kp, kl, vp, vl) -> i32` | `kv` | `0` ok, `-1` refused or the plugin is over `plugin_storage_mb`. |
| `kv_delete` | `(ptr, len) -> i32` | `kv` | `0` ok, `-1` refused. |
| `kv_keys` | `(ptr, len) -> i64` | `kv` | JSON array of keys under the prefix. |
| `http_get` | `(ptr, len) -> i64` | `http:<host>` | GET through the SSRF-safe fetcher: exact host match on the parsed URL, no credentials in the URL, 3 calls a request, 5 s each, body up to 64 KB. `0` when the fetch failed. |
| `request` | `(ptr, len) -> i64` | per `op` | JSON `{"op": ..., ...}` answered `{"ok": ...}` or `{"error": "..."}`. |

`kv` keys starting `user:<id>:` are removed with that member's account (erasure) and exported with
their data; a table column declared with `user_column` is too.

### `bf.request` operations

Everything except the reads (`content.read`, `user.get`, `user.can`, `settings.get`, `meta.get`,
`file.get`, `content.can`, `db` selects and counts and the three no-capability ops) counts as a write: 5 a call.

| `op` | Capability | Request fields | `ok` answer |
| --- | --- | --- | --- |
| `db` | `tables` | `action` (`select`, `count`, `insert`, `update`, `delete`), `table`, `where` (see [Table queries](#table-queries)), `values` `{col: value}`, `order` (a column, or up to three, `-` for descending), `desc`, `limit` (max 500), `offset` (max 100000) | rows (select), `[{count}]` (count), `[{id}]` (insert) or `{affected}`. Only tables declared in the manifest, with their declared column types. |
| `content.read` | `content.read` | `topic`, `from` (post number), `limit` (max 100) | `{id, title, slug, locked, state, node_id, owned, posts: [{number, author, html, created_at}]}`. Only what a guest can read, except a topic the plugin owns (in a forum it declared or created, or one its account opened): that one in any state but deleted. |
| `content.reply` | `content.write` | `topic`, `body` (up to 20000 chars) | `{number}`. Posts as the plugin's own account; `plugin_posts_per_hour` applies. Plugin `post.created` hooks do not run for it. |
| `content.create_topic` | `content.write` | `node_id`, `title` (3 to 200), `body` | the new topic. |
| `content.edit` | `content.write` | `topic`, `number`, `body`, `reason`, `title`, `locked`, `listed` (each optional, at least one) | `{edited}`. `body`: only posts the plugin's own account wrote; the old text is kept as a revision. `title`, `locked`, `listed`: only on a topic the plugin owns. Never another member's words. |
| `job` | `jobs` | `payload`, `delay` seconds (0 to 86400) | `{queued}`. Runs `job.run`; three attempts. |
| `schedule` | `jobs` | `name`, `every_minutes` (5 to 43200), `payload`; or `name` and `cancel: true` | Recurring `job.run` carrying `"schedule": name`. Capped by `plugin_max_schedules`. |
| `notify` | `notify` | `user_ids` (1 to 50), `text` (up to 200 chars), `url` (a path on this site), `kind` (one of the plugin's `[[notification_kinds]]`) | `{sent}`. With a `kind`, members' own settings for it apply. |
| `email.send` | `email` | `user_id`, `subject` or `subject_key`, `text` or `text_key`, `vars` | `{queued}`. Goes to a verified address, in the member's language when `*_key` names a message of your `locales/`; `plugin_emails_per_hour` applies. |
| `realtime.push` | `realtime` | `user_ids` (1 to 50), `event`, `data` (up to 4 KB) | `{sent}`. The browser sees `plugin.<id>.<event>`. |
| `user.get` | `users.read` | `user_id` | `{id, username, display_name, trust_level, created_at, groups}` |
| `user.can` | `users.read` | `user_id`, `node_id`, `perm` (`view`, `read`, `create`, `reply`, `attach`, `moderate`, or a key of the plugin's own `[[permissions]]`) | `{allowed}` |
| `settings.get` | `settings.read` | `key`, or none for all | public settings, and the plugin's own `[[settings]]` (`plugin:<id>:<key>`) |
| `profile.set` | `profile_fields` | `user_id`, `key`, `value` | `{saved}`. One of the plugin's own `[[profile_fields]]`, checked like a member's entry. |
| `group.add`, `group.remove` | `groups` | `user_id`, `key` | `{ok}`. One of the plugin's own `[[groups]]`. |
| `badge.grant` | `badges` | `user_id`, `key` | `{granted}`. One of the plugin's own `[[badges]]`. |
| `tags.set` | `tags` | `topic_id`, `keys` | `{tags}`. The plugin's own `[[tags]]` on a topic; other tags are left alone. |
| `nodes.create` | `nodes` | `parent` (key of one of the plugin's `[[nodes]]`), `title`, `description` | `{id}` |
| `meta.get` | `meta` | `target` (`topic`, `post` or `user`), `id`, `key` | `{meta}` |
| `meta.set` | `meta` | `target`, `id`, `key` (a declared `[[meta]]` key), `value` (up to 8 KB of JSON; `null` removes) | `{ok}` |
| `file.put` | `files` | `path`, `content_type`, `text` or `base64`, `append` | `{url, size}`. Served at `/plugin-files/<id>/<path>` (with `Range`, `ETag`, `nosniff`); up to 8 MB a file (send 64 KB pieces with `append`, which adds in the database without reading the file back), 1000 files, all counted in `plugin_storage_mb` (a write that would pass it is refused); no content type a browser would run as a page. |
| `file.fetch` | `files` and `http:<host>` | `url`, `path`, `content_type` | `{url, size}`. Stores what an approved host serves (up to 8 MB, 10 s, SSRF-safe fetcher, `plugin_http_per_minute`); the type is the plugin's to name, never the remote server's. Counts as a write and an outbound request. |
| `file.get` | `files` | `path` | `{content_type, base64, size}` |
| `file.delete` | `files` | `path` | `{deleted}` |
| `search.index` | `content_types` | `type`, `ref` (up to 100), `title`, `body` (up to 60000 chars), `url` (a path), `author` | `{indexed}`. Offers an entry to site search (page 1 of a plain search). The type must be a searchable `[[content_types]]`; its `permission` decides who finds it. 600 a minute, 50000 documents, counted in the quota. |
| `search.remove` | `content_types` | `type`, `ref` | `{removed}` |
| `review.hold` | `content_types` | `type`, `ref`, `title`, `excerpt` (up to 500), `url` (a path), `user_id` | `{held, id}`. Puts an entry in the queue at `/review/apps` (a type with `review = true`). 120 an hour, 500 waiting; `held: false` when already waiting. |
| `review.withdraw` | `content_types` | `type`, `ref` | `{withdrawn}` |
| `content.can` | `content_types` | `type`, `user_id` | `{allowed}`: the member holds the type's `permission` (always true for a type without one). |
| `http_request` | `http:<host>` | `url`, `method`, `headers`, `json` or `body` | `{status, body}`; `plugin_http_per_minute` applies. |
| `time.now` | none | | `{unix, iso}` |
| `random.bytes` | none | `n` (1 to 1024) | `{hex}` |
| `i18n.t` | none | `locale`, `key`, `vars` | `{text}`; your own `locales/` for that language, then the base language, then English. |

## Capabilities

`kv`, `api_route`, `tables`, `content.read`, `content.write`, `jobs`, `notify`, `login`, `client`
(browser code on this site's origin; outside the sandbox), `users.read`, `settings.read`, `email`,
`realtime`, `files`, `pages` (routes under `/app/<id>/`), `content_types` (search, review queue and
permission of a content type; see [Content apps](#content-apps)), the registry capabilities `profile_fields`, `groups`, `permissions`, `badges`,
`notification_kinds`, `reactions`, `flag_reasons`, `settings`, `api_scopes`, `tags`, `nodes`, `meta`
(each needed for the manifest section of the same name), `http:<host>` (exact lowercase host) and
`connect:<origin>` (an `https` or `wss` origin the browser code may contact; added to the page's
`connect-src`). Anything else fails the install. An administrator approves them when enabling; an
upgrade asks only about the new ones. A package with only style sheets, translations, assets,
slots, buttons, emoji, forms and cook rules needs none.

## Limits

| What | Limit |
| --- | --- |
| Instructions per call | 200 million (fuel) |
| Wall-clock per call | 15 s (including waits in host calls) |
| Memory | 32 MB, one memory, one instance, 10,000 table elements |
| Answer size | 1 MB |
| Value or argument | 64 KB |
| Writes per call | 5 |
| Routes, content types | 40, 10 a plugin; POST body 32 KB (route may ask up to 256 KB); page HTML 400000 characters; feed 100 items |
| Files | 8 MB a file, 1000 files; with search documents (50000) and held entries (500 waiting), all within `plugin_storage_mb` |
| Outbound requests per call | 3, 5 s each |
| Consecutive failures before it is disabled | 5 (not counted for visitor-driven hooks) |
| Button presses | `plugin_actions_per_minute` (30) per member and plugin |
| Package | 32 MB, 500 files; `assets/` 1 MB a file, 3 MB and 100 files in all; style sheet 256 KB; locale file 256 KB, 20 languages |
| Cook rules | 16 a plugin (32 in all), markers 1 to 32 characters, template 2 KB, 100 matches a text, 20 callbacks a post |
| Emoji, widgets, admin pages, buttons | 200, 20, 20, 10 a plugin |
| Registry sections | 50 entries each |

## Hooks

"Visitor-driven" hooks run on a member's request with input of their choosing, so a failure there
does not count towards disabling the plugin.

### Filters

Run in order; an answer of `{"reject": "message"}` stops the action and shows the message.

| Hook | `data` | Answer |
| --- | --- | --- |
| `post.before_create` | `{raw, author_id}` | `{"data": {"raw": "..."}}` to change the text |
| `post.before_cook` | `{raw, author_id}` | same |
| `item.before_create` | `{title}` | `{"data": {"title": "..."}}` |
| `search.query` | `{q}` | `{"data": {"q": "..."}}` |
| `user.before_register` | `{username, email}` | `{"reject": ...}` |
| `signup.validate` | `{username, email, fields}` (`fields`: `<plugin>:<key>` and `field:<id>` to answers) | `{"reject": ...}` |
| `user.can` | `{user_id, groups}` | `{"deny": [action, ...], "deny_nodes": {"<forum id>": [action, ...]}}`: see [The `user.can` filter](#the-usercan-filter) |
| `post.after_cook` | `{html}` | `{"data": {"html": "..."}}` (sanitized again) |
| `admin.form.save` | `{values}` (the settings or admin page just saved, already checked) | `{"reject": ...}` |

### The `user.can` filter

The hook is asked once per member (the answer is kept for 10 seconds) and only while a plugin
listens; it costs nothing otherwise. It can only take rights away, never grant them, and
administrators are exempt. Answer with `deny` (everywhere) and `deny_nodes` (in one forum). Unknown
names are ignored.

| Action | What it stops |
| --- | --- |
| `view`, `read`, `create`, `reply`, `attach`, `moderate` | The forum permissions, wherever the core checks them (lists, topic pages, posting). |
| `edit`, `delete`, `react`, `flag`, `report`, `vote`, `bookmark`, `follow`, `comment` | The matching request, in forums, chat and accounts. |
| `chat`, `pm` | Writing in chat; private messages. |
| `upload`, `download` | Uploading files and downloading them. |
| `write` | Every state-changing request that is not exempt: a frozen account. |

Requests are named by their method and path (`actions_of` in `platform/plugins/gate.rs`); a
state-changing route nobody named counts as `write`, and a test fails when a route is neither named
nor exempt. Signing in and out, registration and recovery, the admin panel, federation and plugin
endpoints are exempt. A denied request answers 403.

### Rendering and requests

| Hook | `data` | Answer |
| --- | --- | --- |
| `panel.render` (visitor-driven) | `{area, key, viewer_id, profile_id, locale, dir, strings}` | `{"html": "..."}` (sanitized). |
| `widget.render` | `{key, settings}` (the values filled in from the widget's `fields`) | `{"html": "..."}`; cached for everyone, so no viewer. |
| `slot.render` (visitor-driven) | `{slot, page, post_id, viewer_id, subject, locale, dir, strings}` | `{"html": "..."}`; asked for the slots in `dynamic_slots`. `subject` is the thing the slot sits on (see below). |
| `action.run` (visitor-driven) | `{key, kind, post_id, selection, viewer_id, locale, dir, strings}` | by the button's `result`: `{"toast": "..."}`, `{"insert": "..."}` or `{"html": "..."}`; `{"error": "..."}` shows a message. |
| `cook.rule` | `{rule, text}` | `{"html": "..."}` for a `callback` cook rule (sanitized like a post); nothing leaves the text as written. |
| `onebox.resolve` (visitor-driven) | `{url}` | `{title, description, image, site}` as text; `image` must be `https://`. Only for hosts the plugin holds `http:<host>` for. |
| `api.request` (visitor-driven) | `{method, path, body, user_id, api_key, scopes}` | `{"status": 200, "json": ...}`; served at `/api/p/<id>/<path>`, needs `api_route`, writes need the CSRF token. `scopes` are the plugin's own `[[api_scopes]]` the calling key holds. |
| `api.serialize` | `{kind, id}` | `{"fields": {...}}`, added to API answers under the plugin's id. |
| `sitemap.urls` | `{}` | `{"urls": ["/path", {"path": "/path", "lastmod": "2026-11-01T10:00:00Z"}, ...]}` (local paths, up to 5000) |
| `page.request` (visitor-driven) | `{route, method, path, params, query, form, viewer, permissions, ...}` | A page, a redirect or a feed for one of the plugin's `[[routes]]`, served at `/app/<id>/<path>`; needs `pages`: see [Content apps](#content-apps). |
| `content.review` | `{type, ref, decision, by}` | ignored; a moderator decided an entry the plugin held with `review.hold`. |
| `login.begin` / `login.callback` (visitor-driven) | `{key, state, callback_url}` / `{key, query}` | `{"redirect": "https://..."}` / `{subject, email, email_verified, name}`; needs `login`. |
| `member.filter` | `{filter}` | `{"user_ids": [...]}` |
| `job.run` | `{payload}` (and `schedule` for a recurring one) | ignored |
| `scheduled.tick` | `{}` | ignored; every ten minutes |

`strings` is the plugin's `locales/<locale>.json` over `locales/en.json`; `dir` is `ltr` or `rtl`.
`kind` is `post` or `composer`.

`subject` in `slot.render`, when the host could tell from the page (`/u/<name>`, `/t/<slug>/<id>`,
`/c/<slug>/<id>`; the member for `user_card`):

```json
{ "kind": "user", "id": 3, "username": "ann", "display_name": null, "title": null, "group": "Members",
  "badges": [{ "name": "Rookie", "tier": "bronze", "icon": "star", "level": 1 }],
  "fields": [{ "label": "Website", "kind": "link", "value": "https://…", "plugin": null }] }
{ "kind": "topic", "id": 12, "title": "…", "forum_id": 4 }
{ "kind": "forum", "id": 4, "title": "…", "slug": "queue" }
```

For a member: only what the profile shows (their latest badges and public profile fields, the ones
plugins declare with `[[profile_fields]]` included; `plugin` names the declaring plugin). It is
absent for a hidden or unapproved profile.

### Events (fire and forget, run in the background)

Answers are ignored. Payloads are documented on `HOOKS` in `plugins.rs`; the SDK has a struct for
each (`bf_plugin_api::hooks`): `post.created`, `topic.created`, `post.edited`, `post.deleted`,
`topic.deleted`, `topic.moved`, `user.registered`, `user.login`, `user.logout`,
`user.password_changed`, `user.email_changed`, `profile.updated`, `user.erased` (awaited, so purge
what you hold for that member), `reaction.changed`, `vote.cast`, `poll.voted`, `flag.created`,
`moderation.action`, `upload.created`, `follow.changed`, `badge.awarded`, `trust.changed`,
`chat.message`, `message.created`.

## Manifest

```toml
id = "my-plugin"            # required: lowercase, digits, dashes; no "--"; up to 60
name = "My plugin"          # required
version = "1.0.0"           # required
api = "^1.0"                # required: host API major
kind = "wasm"               # the only installable kind
capabilities = ["kv"]
hooks = ["post.before_create"]
requires = ["other-plugin ^1.0"]   # plugins that must be enabled first
islands = ["my-island"]            # client code loads only on pages with one of these (empty: every page)
dynamic_slots = ["post_footer"]    # slots drawn per viewer by slot.render; "user_card" is the hover card

[server]  module = "plugin.wasm"
[client]  entry = "client.js"      # optional ES module (needs the client capability)
          css = ["style.css"]      # optional style sheets

[[nav]]                     # a link: header bar, footer, sidebar
label = "Docs"              # or label_key; English text, translated from locales/
label_key = "nav.docs"
url = "/plugin/my-plugin/docs"
groups = [3]                # who sees it (group ids; empty: everyone; administrators see all)
icon = "coffee"             # a name from the icon sprite
menu_key = "menu.tools"     # or menu = "Tools": links with the same menu share a drop-down
order = 10                  # lower first

[[panels]]                  # a page: settings | modcp | profile | admin | content | login
area = "profile"
key = "stats"
label_key = "panel.stats"   # or label

[[tables]]                  # needs the tables capability
name = "scores"
columns = { user = "int", points = "int" }
user_column = "user"        # rows go when the member is erased
unique = ["user"]           # no two rows with the same value (optional)

[[post_actions]]            # a button on each post for signed-in members
key = "pin"
label_key = "button.pin"
icon = "pin"
result = "toast"            # toast | insert | panel

[[composer_buttons]]
key = "shrug"
label = "Shrug"
icon = "smile"
result = "insert"

[[cook_rules]]              # marker pair -> template; {} is the escaped text between them
open = "||"
close = "||"
html = "<span class=\"plug-spoiler\">{}</span>"   # classes in posts must start with plug-
block = false               # true: the paragraph must be exactly open ... close

[[cook_rules]]              # or the plugin decides: cook.rule gets {rule: "shout", text}
open = "!!"
close = "!!"
callback = "shout"

[[emoji]]                   # :key: in posts and the composer picker, from assets/
key = "party"
label = "Party"
file = "assets/party.svg"
set = "My emoji"            # a heading in the picker

[[widgets]]                 # a block for the page builder
key = "count"
label_key = "widget.title"
description_key = "widget.description"
fields = [{ key = "heading", type = "text", label = "Heading", default = "Hello" }]

[[admin_pages]]             # a form page in the admin panel
key = "limits"
label_key = "page.limits"
intro_key = "page.limits_intro"
fields = [{ key = "max", type = "number", label = "Most", min = 1, max = 100, default = 20 }]

[[routes]]                  # a page under /app/my-plugin/ (needs pages); see Content apps
key = "entry"
path = "entries/{slug}"
methods = ["GET"]

[[content_types]]           # needs content_types: search, review queue, permission
key = "entry"
label = "Entry"
search = true
review = true
```

### Forms

`settings.json`, `[[widgets]]` and `[[admin_pages]]` share one description of a field:
`{key, type, label, label_key, help, help_key, default, options, min, max, rows}`, where `type` is
`text`, `textarea`, `number`, `switch`, `select` (with `options: [{value, label, label_key}]`),
`url`, `email`, `date` or `color` (entered as hex text). `settings.json` may instead be a JSON Schema
(`properties` with `type`, `title`, `description`, `default`, `enum`, `minimum`, `maximum`, plus
`title_key`, `description_key`, `enum_labels` and `enum_label_keys` for translations). The admin
panel draws the form with its own controls; the server checks every saved value against the
declaration (number ranges, choices, link shapes) and then asks `admin.form.save`. The plugin sees
the saved values, defaults filled in, as `settings` in every call (and `data.settings` for a
widget).

### Registries

Sections that add rows to the forum's own lists, each needing the capability of the same name, owned
by the plugin (read-only in the core panels, removed with the plugin), keys unique per plugin and
`[a-z0-9_-]` up to 40, at most 50 entries:

| Section | Fields | Used by |
| --- | --- | --- |
| `[[profile_fields]]` | `key`, `label`, `type` (`text`, `url`, `select`, ...), `options`, `required`, `show_on_signup`, `editable`, `visible`, `icon`, `url_prefixes`, `url_shorthand`, `https_only` | `profile.set`; shown on profiles and in `subject.fields` |
| `[[groups]]` | `key`, `name`, `colour` | `group.add`, `group.remove`; never staff |
| `[[permissions]]` | `key`, `label` | `user.can` with `perm = key`; groups chosen on the plugin's admin card |
| `[[badges]]` | `key`, `name`, `description`, `tier`, `icon`, `color`, `level`, `points` | `badge.grant` |
| `[[notification_kinds]]` | `key`, `label`, `icon`, `tab`, `email` | `notify` with `kind` |
| `[[reactions]]` | `key`, `name`, `icon` (one emoji), `score` (-1, 0, 1) | members react like the built-in ones |
| `[[flag_reasons]]` | `key`, `label`, `hint`, `weight`, `needs_note` | the report form |
| `[[settings]]` | `key`, `type` (`bool`, `int`, `string`), `default`, `min`, `max`, `description` | the site Settings page; `settings.get` |
| `[[api_scopes]]` | `key`, `label` | scopes on members' API keys; `scopes` in `api.request` |
| `[[tags]]` | `key`, `name` | `tags.set` |
| `[[nodes]]` | `key`, `title`, `kind`, `description`, `parent` | `nodes.create` |
| `[[meta]]` | `key`, `target` (`topic`, `post`, `user`), `public` | `meta.get`, `meta.set`; public ones in API output under `meta.<plugin>.<key>` |

Other files in the zip: `settings.json`, `templates/<slot>.html` (static slot fragments),
`locales/<code>.json`, `assets/**`, `signature.txt`. Slot names: `header_after`, `header_icons`,
`home_top`, `topic_header`, `topic_after`, `post_after_content`, `post_footer`,
`topic_row_after_title`, `profile_header`, `profile_block`, `profile_sidebar`, `category_header`,
`footer`, and `user_card` (dynamic only).

### Cooked output and re-cooking

Cook rules and emoji change what a post cooks to. When the set changes (a plugin enabled, upgraded
or removed), every post is marked and the background job re-cooks them through the same path as a
new post, so old posts pick the rules up. Cook rules work on plain text and across inline formatting
(`||a **b** c||`); inside code and links they do nothing. Callback rules run `cook.rule` where posts
are cooked through the plugin-aware path (posting, editing, re-cooking), at most 20 a post.

## Content apps

Since Host API 1.3 a plugin can host a whole content section of the site (a blog, a gallery, a
download library): pages of its own, forms, SEO, a feed, search, a review queue and files. The
reference is [`docs/examples/plugins/notes-app`](../examples/plugins/notes-app) (a small blog, built
with `sh build.sh`); the host does every check that matters before and after the plugin runs.

### Routes and pages (`pages`)

```toml
capabilities = ["pages", "tables", "content_types", "permissions"]
hooks = ["page.request"]

[[routes]]
key = "entry"                 # names the route in page.request
path = "entries/{slug}"       # under /app/<plugin>/ ; "" is /app/<plugin> itself
methods = ["GET"]             # GET (default) and/or POST
login = false                 # true: members only. Always so when POST is allowed
permission = "write"          # optional: a key of the plugin's own [[permissions]]
max_body = 32768              # POST body in bytes (default 32 KB, at most 256 KB)
kind = "page"                 # or "feed"
```

Paths have up to 8 segments, each a literal (`[a-z0-9._-]`) or `{param}`; a literal beats a
parameter, and two routes may not share a shape and a method. Before the plugin runs, the host
checks `login` and `permission` (a guest or a member without the right gets the site's 403 page;
administrators hold every permission), and for POST the body size, a form content type
(`application/x-www-form-urlencoded`), the CSRF token (the `_csrf` field the host adds to every
form that posts, or the `x-csrf-token` header) and `plugin_actions_per_minute` per member. A frozen
account (`write` taken away by a `user.can` filter) cannot POST.

`page.request` `data`: `{route, method, path, params, query, form, viewer_id, viewer: {id, username,
is_admin, staff}, permissions, locale, dir, strings, base_url}`. `form` and `query` map a name to text
(a name sent twice becomes a list; 100 fields of 20000 characters at most). The answer:

| Field | Meaning |
| --- | --- |
| `html` | The body, up to 400000 characters. Sanitized (below) and put in the site's page shell. |
| `title` | The heading and `<title>` (up to 200). |
| `description` | `<meta name="description">` and the cards' description (up to 300). |
| `canonical` | A path on this site; the request's own path when absent. |
| `noindex` | `true` adds `robots noindex`. Always set for a status other than 200 and for a route behind `login` or `permission` or taking POST. |
| `og` | `{"type": "article"` (or `website`, `profile`)`, "image": "https://…" or a path}`. |
| `jsonld` | An object, or a list of up to 3, each up to 20 KB. Written with `<` escaped, so it cannot close the script. |
| `breadcrumbs` | `[["Label", "/path"], …]`, up to 5. |
| `feed` | The path of the plugin's `.rss` route, advertised in the page head. |
| `status` | 200, 400, 403, 404, 410 or 422. |
| `redirect` | A path on this site: answers `303 See Other` and nothing else is used. |

A route of `kind = "feed"` is a public GET route whose path ends `.rss`; the same route also
answers `.atom` and `.json` beside it. The plugin answers `{"feed": {title, link, description,
items: [{title, link, html, published}]}}` (links are paths; up to 100 items) and the host writes
RSS, Atom and JSON Feed, escaping everything and sanitizing item HTML.

**What the sanitizer keeps on a plugin page** (`bf_web::theme::clean_page_html`): what a slot may
contain, plus `form`, `input`, `textarea`, `select`, `option`, `optgroup`, `label`, `button`,
`fieldset` and `legend` with their usual attributes. Never scripts, event handlers, `style`
(the CSP blocks inline styles anyway), `formaction`, `file` or `password` inputs. A form's `method` is
`get` or `post` and its `action` must be inside `/app/<plugin>/`. Field names that start with `_` or
that would shadow a form property (`action`, `submit`, `method`, ...) are dropped. `id` and `for`
values are prefixed `pp-`. `data-*` attributes are kept **only as `data-app-*`** (and `aria-*`):
the site's own `data-*` attributes drive its scripts and must not be forgeable from a page.

### Content types (`content_types`)

```toml
capabilities = ["content_types", "permissions"]
hooks = ["content.review"]

[[permissions]]
key = "read-private"
label = "Read private entries"

[[content_types]]
key = "entry"          # [a-z0-9_-], up to 10 a plugin
label_key = "type.entry"
search = true          # entries may be offered to site search
review = true          # entries may be held for a moderator
permission = "read-private"   # optional: only members whose group holds it find the entry
```

- **Search.** `search.index` puts an entry in the site's search (plain searches, first page, below the
  topics). The host applies the type's `permission` when someone searches; the plugin never chooses
  it per request. `search.remove` takes it out. Index only what is published.
- **Held for review.** `review.hold` queues an entry for moderators and administrators at
  `/review/apps` (linked from the review queue and counted in its badge). The decision arrives as the
  `content.review` hook `{type, ref, decision: "approve" | "reject", by}`; the plugin publishes or
  drops the entry then. The decision is recorded even if the plugin fails to answer.
- **Permission gate.** A route's `permission` is checked by the host; `page.request` carries the
  keys the viewer holds; `content.can` answers for any member (jobs, hooks).
- **Sitemap.** `sitemap.urls` may answer paths or `{"path", "lastmod"}` objects; it is how entries
  reach the sitemap.

### Table queries

`where` takes a plain value (equality) or, per column, an object of operators, all values bound:
`eq`, `ne`, `gt`, `gte`, `lt`, `lte` (not on `bool`), `in` (1 to 50 values), `isnull`, and for
`text` columns `prefix` (starts with) and `contains` (case-insensitive substring). `order` is a
column (with `desc`) or a list of up to three, `"-col"` for descending; ties break on `id`, so pages
are stable. `offset` skips rows (up to 100000), `limit` is at most 500. `{"action": "count", ...}`
answers `[{"count": n}]`. Column types: `text`, `int`, `float`, `bool`, `json`, `timestamp` (ISO
text in and out). A table may declare `unique = ["slug"]`: an insert that repeats a value fails with
"That value already exists." and writes nothing. Selects and counts share `plugin_db_reads_per_minute`.

### Files

`/plugin-files/<id>/<path>` answers `Range` (one range; `206`, `416` when it cannot be met),
`ETag`/`If-None-Match`, `Accept-Ranges: bytes`, `X-Content-Type-Options: nosniff` and a sandboxing
CSP. Images, audio, video, text, PDF and JSON show in place; archives, binaries and fonts download
(`Content-Disposition: attachment`), and `?download=1` asks for that for any file. Files, documents
in search and held entries all count toward `plugin_storage_mb`, the plugin's quota; 8 MB a file,
1000 files. The allowed types are fixed (no HTML, no script, no XML).

### Security rules that apply

- Every host call is behind a capability the administrator approved, counts as a write or a read,
  and has a rate or size limit above.
- Plugin output is always sanitized; the plugin cannot set a header, a cookie or an inline style.
- A plugin reaches only its own tables, files, documents and queue entries: names are checked
  against its declaration and every value is bound. There is no call that reads another plugin's data.
- Outbound requests (`file.fetch`, `http_request`) go through the SSRF-safe fetcher, to hosts
  approved as `http:<host>` only.

### Out of scope

Plugin-owned topics do not replace the forum's own: a content app keeps its entries in its tables.
Not offered: multipart uploads from visitors (a form cannot carry a file; a plugin stores files from
an approved host or from an admin page), comments threaded into the topic system, per-entry
notification kinds beyond `notify`, entries in the topic-based search filters (`#tag`, `@author`),
theming a plugin page beyond the site's classes and the plugin's own style sheet, and more than
the first page of search for plugin entries.

## Upgrades and rollback

Installing a package whose `id` exists is an upgrade. A lower `version` is refused. If the new
version asks only for capabilities already approved, it keeps running and its tables are migrated at
once; otherwise it stays off until the new ones are approved. The package it replaced is kept:
**Roll back** (`POST /api/v1/admin/plugins/<id>/rollback`) puts it back, and it runs again at once if
everything it asked for was approved before. `requires = ["other ^1.0"]` is checked when enabling.

## API endpoints for lists

Besides the topic card and a single post, plugins' public `meta` appears (under `meta.<plugin>.<key>`)
in `GET /api/v1/forums/{id}/topics?page=`, `GET /api/v1/topics/{id}/post-list?from=&count=`, search
results (`GET /api/v1/search`) and user cards, one query per page. A plugin's `scopes` for an API key
arrive in the `api.request` data as `scopes` (its own keys) with `api_key: true`.

## Testing against local servers

`http_request` and `http_get` refuse loopback and private addresses. To test against a server on
`127.0.0.1`, start the forum with `BF_WEBHOOK_ALLOW_LOOPBACK=1` (never in production) and approve the
host `http:127.0.0.1`. Mail can be captured to files with `BF_MAIL_DIR=<dir>`.

## Browser code

`[client] entry` is an ES module loaded as `/plugins/<id>.<hash>.js`. It registers with
`(window.bfPlugins ??= []).push((api) => { ... })`:

| `api` | |
| --- | --- |
| `registerIsland(name, fn)` | Mount code for `[data-island="name"]` elements. |
| `decorateCooked(selector, fn)` | Runs for matching elements in posts, now and later. |
| `addPostMenuButton(label, onClick)` | `onClick({id, element})` for the post the button sits on. |
| `addComposerToolbarButton(label, onClick, icon?)` | `onClick(ctx)`; `ctx.insert(text)`, `ctx.selection()`, `ctx.text()`, `ctx.wrap(before, after?)`, `ctx.replace(text)`. With an `icon` name the button is an icon with the label as its tooltip. |
| `on(name, fn)` | `posts-added`, or any `bf:<name>` event. Returns an unsubscribe function. |
| `fetch(url, opts)` | `fetch` with the CSRF header. |

The page's Content Security Policy allows scripts from this site only; talking to other origins
needs `connect:<origin>`.
