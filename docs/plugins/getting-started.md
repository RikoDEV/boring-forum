# Writing a plugin

A plugin is a zip with a `plugin.toml`, and usually a WebAssembly module. The forum runs the module
in a sandbox, once per hook, with limits on instructions, memory and time. It can reach only the
host calls its manifest asks for and an administrator approved. This page builds one from nothing;
[abi.md](abi.md) lists every hook, host call, limit and manifest field.

Content apps such as blogs, galleries and downloads are no longer part of the core. They are meant
to be built as plugins.

Content apps such as blogs, galleries and downloads are no longer part of the core. They are meant
to be built as plugins.

## 1. What you need

```sh
rustup target add wasm32-unknown-unknown
```

Plugins are written in Rust with the SDK crate `crates/bf-plugin-api`, but any language that
compiles to WebAssembly and exports `memory`, `alloc` and `handle` works (the ABI is plain JSON).
A plugin with no server code at all (only slot templates, a style sheet, translations, assets or
declarative buttons and syntax) needs no compiler.

## 2. The worked example

`docs/examples/plugins/community-kit` is a complete plugin: a word filter, a page on every profile, a
widget, a per-viewer line under every post, a composer button, `||spoiler||` syntax, a style sheet
and a translation into Polish and Arabic. Build and install it:

```sh
scripts/plugin.sh pack docs/examples/plugins/community-kit      # writes docs/examples/plugins/community-kit/dist/plugin.zip
```

Then Admin, Plugins, Install a plugin, and approve the capabilities it lists (`kv`).

Layout of the folder:

```
community-kit/
  plugin.toml          the manifest
  Cargo.toml, src/     the module (not packed; only the built .wasm goes in the zip)
  settings.json        JSON Schema of the settings an administrator edits
  style.css            listed in [client] css
  assets/leaf.svg      images and fonts, referenced from the style sheet as url(assets/leaf.svg)
  locales/en|pl|ar.json
  templates/<slot>.html   server-rendered slot fragments (optional)
```

## 3. The module

```rust
use bf_plugin_api::{export, Reply, Request};

fn on_hook(req: Request) -> Reply {
    match req.hook.as_str() {
        "post.before_create" => {
            let post: bf_plugin_api::hooks::PostBeforeCreate = req.payload().unwrap();
            if post.raw.contains("spam") { Reply::reject("No spam, please.") } else { Reply::none() }
        }
        _ => Reply::none(),
    }
}
export!(on_hook);
```

`Cargo.toml` needs `crate-type = ["cdylib"]`, a path or git dependency on `bf-plugin-api`, and an
empty `[workspace]` table if the folder sits inside another workspace. `export!` writes the two
functions the host calls (`alloc` and `handle`); the SDK turns JSON into the typed structs in
`bf_plugin_api::hooks` and back through `Reply`. Calls into the forum are in `bf_plugin_api::host`:
`kv_get`, `kv_set`, `db_select`, `notify`, `http_get`, `enqueue_job`, and the generic `request`.

A call that was not approved returns `Error::Denied`; it never panics. A panic or trap fails the
call; five failures in a row disable the plugin and tell administrators (hooks a visitor drives,
like `slot.render`, `panel.render` and `action.run`, do not count).

## 4. The manifest

```toml
id = "community-kit"          # lowercase, digits and dashes; table names and URLs are built from it
name = "Community kit"
version = "1.0.0"
api = "^1.0"                  # the host API major; this forum has 1.x
capabilities = ["kv"]         # what an administrator is asked to approve
hooks = ["post.before_create", "panel.render"]

[server]
module = "plugin.wasm"

[client]
entry = "client.js"           # optional ES module, loaded only on pages with one of `islands`
css = ["style.css"]           # optional style sheets
```

Everything else is optional and listed in [abi.md](abi.md#manifest). Packages are read in memory:
nothing is unpacked to disk, every file has a size cap, and `plugin.toml` mistakes are reported with
the message you see at install time.

## 5. Showing things

| You want | You write |
| --- | --- |
| A fragment in a fixed place (footer, topic end, profile sidebar) | `templates/<slot>.html`; slot names are in `theme::SLOTS` (`header_after`, `header_icons`, `home_top`, `topic_header`, `topic_after`, `post_after_content`, `post_footer`, `topic_row_after_title`, `profile_header`, `profile_block`, `profile_sidebar`, `category_header`, `footer`). Fragments are rendered once when the plugin loads and see `{{ site_name }}`. |
| A fragment that depends on who is looking | `hooks = ["slot.render"]` and `dynamic_slots = ["post_footer"]`. The page holds a placeholder; the browser asks `/plugin-slot/<slot>` and the `slot.render` hook answers with HTML for that viewer, language and page. Answers are cached for 30 seconds per viewer, language, page and post. The payload has a `subject`: the member (with latest badges and public profile fields, the plugins' own included), topic or forum the page is about, so a `profile_header` can show badges and fields. `user_card` (in `dynamic_slots` only) is the body of a member's hover card. |
| A page of your own | `[[panels]]` with `area` (`settings`, `modcp`, `profile`, `admin`, `content`, `login`) and `hooks = ["panel.render"]`. `content` pages are public and indexed. |
| A block for the page builder | `[[widgets]]` with a `key`, a label and `fields` for its settings; the admin's Widgets page lists it by name and shows the form. `widget.render` gets `{key, settings}`. (`{"plugin": "id", "key": "name"}` on the generic "Plugin block" still works.) |
| A form in the admin panel | `settings.json` (typed fields, see below) opens as a Settings dialog on the plugin's card; `[[admin_pages]]` adds whole pages with their own fields. Values are checked against the declaration and saved with the plugin's settings; `admin.form.save` may refuse them. |
| A button next to a post or in the composer toolbar | `[[post_actions]]` / `[[composer_buttons]]` and the `action.run` hook. `result` is `toast` (a message), `insert` (text at the cursor) or `panel` (sanitized HTML in a dialog). No JavaScript to write. Pressing is limited per member and plugin (`plugin_actions_per_minute`). |
| New Markdown-like syntax | `[[cook_rules]]`: an `open` and `close` marker and an `html` template where `{}` is the (escaped) text between them, or a `callback` name and the `cook.rule` hook decides the HTML (20 a post). A pair may wrap formatting (`||a **b** c||`); `block = true` needs the paragraph to be exactly `open ... close`. No regular expression and no code in a template, so nothing can backtrack; the result still passes the post sanitizer, which keeps only `class` values that start with `plug-` (and no `style`, `id` or `data-*`). Enabling, changing or removing rules re-cooks existing posts in the background. |
| Emoji and stickers | `[[emoji]]` with a `key`, an image in `assets/` and an optional `set`: `:key:` in posts becomes the image, and the composer's emoji picker lists them under their set. Pictures are served from `/plugin-emoji/<id>/...`, which stays the same across upgrades. |
| A link card for a site | `onebox.resolve` with the capability `http:<host>`; it is asked only for links on that host, and answers `{title, description, image, site}` as text. |
| Header links | `[[nav]]` with `url`, `label` or `label_key`, and optionally `groups` (who sees it; administrators see all), `icon`, `menu`/`menu_key` (links sharing a menu become a drop-down in the header bar and a heading in the footer) and `order`. |

Slot, widget and panel HTML goes through the same sanitizer as a post: no scripts, no inline
styles, no `<style>`. Use `class` and your own style sheet. `class` and `dir` are kept; `id`,
`style` and `data-*` are not.

### Forms for settings and admin pages

Declare `settings.json` as typed fields instead of free JSON and the admin panel draws a real form
with its own controls, translated from `locales/`:

```json
{ "properties": {
  "mode": { "type": "string", "title": "When a post has one", "title_key": "setting.mode",
            "enum": ["mask", "reject"], "enum_label_keys": { "mask": "setting.mode_mask" }, "default": "mask" },
  "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 },
  "enabled": { "type": "boolean", "default": true } } }
```

Types are text, number (`integer`), switch (`boolean`), select (`enum`), `format: "textarea"`,
`"uri"`, `"email"`, `"date"`. The same fields, in the manifest's own style, work for
`[[widgets]]` and `[[admin_pages]]` (see [abi.md](abi.md#forms)). Saved values are validated on the
server (ranges, choices), defaults are filled in, and your module reads them from `settings` in
every call.

### Registries

`[[profile_fields]]`, `[[groups]]`, `[[permissions]]`, `[[badges]]`, `[[notification_kinds]]`,
`[[reactions]]`, `[[flag_reasons]]`, `[[settings]]`, `[[api_scopes]]`, `[[tags]]`, `[[nodes]]` and
`[[meta]]` add rows to the forum's own lists (each needs the capability of the same name); the host
calls that act on them (`profile.set`, `group.add`, `badge.grant`, `tags.set`, `nodes.create`,
`meta.set`, `content.edit`, `file.put`, ...) are in [abi.md](abi.md) and typed in
`bf_plugin_api::host`.

### Style sheets and assets

`[client] css = ["style.css"]` and everything under `assets/` are served from
`/plugins/<id>.<hash>/...`, where the hash covers the files, so they are cached for a year and a new
version gets a new address. Relative `url(assets/leaf.svg)` works because the paths are the same as
in the zip. The page's Content Security Policy allows them as `'self'`. Rules:

- Style sheets are placed in a `plugin` cascade layer, so they win over the site's layered CSS
  without `!important` but lose to the site's unlayered rules (admin theme CSS).
- No `@import`, no `url()` to another site, 256 KB per sheet.
- `assets/` holds images and fonts only (`png jpg gif webp avif svg ico woff2 woff ttf otf`), 1 MB a
  file, 3 MB and 100 files in all. SVGs are served with a sandbox policy.
- Use logical CSS properties (`margin-inline-start`, `padding-inline`, `inset-inline-end`) so
  Arabic pages mirror by themselves.

## 6. Translations

Put `locales/en.json`, `locales/pl.json`, `locales/ar.json` (any language code; flat `key: text`).
A manifest label may name a key instead of carrying English text: `label_key = "panel.title"` on a
`[[panels]]`, `[[nav]]`, `[[post_actions]]` or `[[composer_buttons]]` entry. The English text (from
`en.json`) is what is stored; a viewer in another language sees that language's text and falls back
to English.

Text your module draws itself is translated by you: `panel.render`, `slot.render` and `action.run`
receive `locale` (`en`), `dir` (`ltr` or `rtl`) and `strings`, the plugin's catalog for that
language over English. `bf_plugin_api::t(&strings, "key")` reads it. The `i18n.t` host call gives the same lookup for
any language, so a job or an email can pick the recipient's. Escape what you put in HTML
(`bf_plugin_api::esc`); the sanitizer is a second wall, not a replacement.

For Arabic, write HTML as in English and let the page's `dir` and CSS do the mirroring. Put
`dir="ltr"` on spans of code or Latin names inside Arabic text, and `dir="rtl"` on Arabic inside
English text; `dir` survives the sanitizer.

### Taking rights away

A plugin that listens to `user.can` can deny a member's actions (`reply`, `chat`, `write`, a forum's
`read`, ...) with `{"deny": [...], "deny_nodes": {"12": [...]}}`; see the ABI page for the list. It
never grants, and administrators are exempt.

## 7. Develop, sign, ship

```sh
scripts/plugin.sh pack  docs/examples/plugins/community-kit
scripts/plugin.sh watch docs/examples/plugins/community-kit http://127.0.0.1:8080 "$BF_ADMIN_TOKEN"
scripts/plugin.sh sign  docs/examples/plugins/community-kit/dist/plugin.zip <64-hex-seed>
```

`watch` rebuilds and reinstalls on every save, with an administrator's API key. The first install
leaves the plugin disabled for approval; later installs keep what was approved and only ask about new
capabilities. `sign` adds `signature.txt` (Ed25519 over the manifest, module, client code, settings,
slots, translations, style sheets and assets) and prints the public key to put in
`BF_PLUGIN_PUBLIC_KEYS`; with the `require_signed_plugins` setting on, the forum installs only signed
packages.

If a plugin breaks the site, open `/safe-mode`.
