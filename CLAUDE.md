# Working on boring-forum

## UI work is not done until you have looked at it

Every change that alters what a page looks like (templates, theme CSS, the admin SPA, island
scripts) is checked by screenshot before it is reported as done. Compiling proves nothing about
layout. Reporting "not checked visually" is not a substitute when the check is possible.

```sh
node scripts/shot.mjs <url> <out.png> [--cookie "…"] [--click <selector>]… [--hover <selector>] [--eval <js>] [--width 390] [--light]
```

- It drives the Playwright Chromium already installed under `%LOCALAPPDATA%\ms-playwright`; no
  browser extension is needed.
- Shoot every changed screen at **1280px and 390px**, in dark mode (the default) and, for theme
  colour changes, `--light`. Open dialogs and menus with `--click`; reach state below the fold with
  `--eval 'document.querySelector(…).scrollIntoView({block:"center"})'`. Show a tooltip with `--hover`.
- Read the PNG. Then fix every `layout:` warning it prints (sideways scrolling, overlapping
  controls) that your change caused, or say which ones were there before.
- Check the awkward states too: empty or unset values, long names, many items, a disabled field.

### Signing in for screenshots

The dev accounts `siteadmin` and `user` hold the owner's real data: never change them. Use a
throwaway account. Register over HTTP, promote it with SQL when an admin is needed, and demote it
afterwards:

```sh
MSYS_NO_PATHCONV=1 wsl.exe -d Ubuntu --cd /mnt/c/Users/jacek/Desktop/boring-forum -- \
  docker compose -f deploy/compose.yml exec -T db psql -U forum -d forum -c \
  "UPDATE users SET primary_group_id = 4 WHERE username_lower = 'qa_tmp_admin'"   # 2 to demote
```

The session cookies are `__Host-` cookies: curl does not send them back over plain http, so pass
them yourself with `-H "Cookie: …"` and to the screenshot tool with `--cookie`.

### What needs rebuilding before a screenshot shows your change

| Changed | Rebuild |
| --- | --- |
| `themes/**/*.css`, `crates/bf-web/templates/*` | restart the server (CSS and templates are compiled into the binary) |
| `web/admin/**` | `pnpm --dir web/admin build` (in Git Bash, `export PATH=/c/Users/jacek/AppData/Local/pnpm:$PATH` first) |
| `web/islands/**` | `pnpm build` in `web/` |
| `crates/bf-db/migrations/*` | restart the server; the migrations run at startup |

## Staff areas are refused on the server, before anything renders

- `/admin/*` (the admin SPA, its HTML and every bundle) is served only to an administrator, by
  `admin_gate` in `crates/bf-server/src/lib.rs`. Everyone else, guests included, gets the site's
  404 page: no redirect to `/login`, nothing that confirms the panel exists.
- `/api/v1/admin/*` is refused by `admin_api_gate` (`http/middleware.rs`) before any handler or
  extractor runs, so a malformed request cannot learn field names. Handlers still call
  `admin(&state, &v, &headers, write)`: the gate is a second wall, not a replacement, and only
  the handler checks CSRF and sudo for writes.
- A staff page rendered by the server (`/modcp`, `/review`, …) checks access in its handler and
  returns `AppError::NotFound`/`Forbidden` before building any layout. Never ship a page whose
  shell renders and only its data request fails.
- After adding a staff page or endpoint, request it as a guest and as a plain member (see the
  throwaway account above) and confirm the error page, not a 200.

## Build forms and panels from the patterns that already exist

Most broken layouts came from mixing layout systems or inventing a one-off component. Copy a
working neighbour (Badges is the reference for an ACP edit dialog) and use these pieces as they are:

**Admin panel (`web/admin`)**: use the components in `web/admin/src/lib`, which render the
markup below: `Field` (text, number, date, textarea with `rows`, select with `options`),
`Switch` (`check` for a plain checkbox), `Section` (a titled `fieldset.acp-picker`; `grid` or
`switches`), `EditDialog` (dialog + form + Delete/Cancel/Save, error from `status`), `Box`,
`IconButton`, `MultiSelect`, `TreeSelect`, `PageHead`. Every add/edit form is an `EditDialog`
opened from a `PageHead` button; never an inline form above the list.
- A box with a form: `section.acp-box` → `div.acp-box-head > h2` → `form.acp-form`. The heading
  goes **outside** the form; inside `.acp-form`, every child is a grid cell.
- `.acp-form` is a grid with `align-items: end`. Cells in one row must be the same kind (all
  floating fields, all checkboxes), or their controls land at different heights. Put anything
  else (pickers, previews, switches) in its own `fieldset.acp-picker` with a `legend`.
- Text input: `label.bf-field` → `input placeholder=" "` + `span.bf-field-label`, nothing else
  inside. Put hints in the label text (`Address: /blogs/…`), not in an extra element.
- Checkbox: `label.acp-check`; switch: `input.acp-switch role="switch"`; full-width cell:
  `.acp-wide`; buttons: `div.acp-actions`.
- Dialogs: `dialog.acp-modal` opened with `showModal()`, as on the Badges page.

**Forum pages (`crates/bf-web/templates`, `themes/default/css`)**
- Modals: `details.bf-modal > summary` + `div.bf-modal-body` with `.bf-modal-actions`.
- Tooltips: `data-tip="…"` on any element (optional `data-tip-side="below|end"`); icon buttons
  show their `aria-label` without markup. One component: `web/islands/src/tooltip.js` draws a
  single bubble in the browser's top layer (a popover), placed from the trigger's position, so
  no `overflow: hidden` box, dialog or scroller can clip it and none needs a workaround. No
  `title=`, no `::after` bubbles, no per-page tooltip CSS; if one is cut off, fix the component.
  The admin panel shares it (`.acp-icon-btn[aria-label]`).
- Badges: `<span class="bf-badge is-ok">` (icon first when it helps); `is-ok`, `is-warn`, `is-bad`,
  `is-accent` give the meaning, none is neutral. States, tags, prefixes, groups, counts and staff
  notes all use it (`components/badges.css`); the admin panel's `.acp-pill` is the same pill. Do not
  make a new pill style for a new state.
- Widget areas (the page builder): add the page to `PAGES` in `admin/themes.rs` and to the
  `widgets_area_check` constraint (a migration), then opt the handler in with
  `l.area = page_area(…)` (pages on `layout.html`) or `Shell::with_area` (the shared shell), taking
  `EditLayout` for `?edit_layout=1`. The "Edit layout" button is `_layout_edit.html`, nowhere else.
- Inline `style=""` is blocked by the CSP. Put the value in a `data-` attribute and set it from the
  islands script through CSSOM (see `data-group-colour` in `web/islands/src/main.js`).

## Every username is a link, every date has its full date on hover: in the whole app

This is a rule for all pages, cards, menus, chat, emails rendered as HTML and the admin panel.
Only the viewer's own greeting and a profile's own heading are exempt; a name inside a row that
is already one link (a notification) stays text, since links cannot nest.

- Server templates: names through `layout.user_link(name)`; dates as `bf_web::ago::Ago`
  (`ago(t)`, or `Ago::labelled("Oct 2026", t)` for a coarser text). Never a formatted `String`.
- Islands: `userLink(name)` and `when(text, iso)` from `web/islands/src/dom.js`. A JSON field
  with a date's text (`"when": ago(t)`) also sends its moment (`"when_at": t.to_rfc3339()`).
- Admin panel: `#lib/UserLink.svelte`; relative dates get `title={when(iso)}`.
- `localTimes()` in `main.js` gives every `<time datetime>` its title, in the reader's zone.

## No English text in code without a translation (English, Polish, Arabic)

Every word a person reads goes through the catalogs in `crates/bf-core/locales/{en,pl,ar}.ftl` (Fluent). A message id is the catalog key with
its dots written as hyphens (`nav.forums` is `nav-forums`), and `{name}` in a catalog is `{ $name }`.
Write it in English where it is used; `scripts/i18n.py` finds it by the call:

- Templates: `layout.t.t("key")`, `layout.t.n("count.key", n)`, `layout.t.msg("English")`.
- Rust in a request: `v.t().msg("…")` / `msgf("… {name}", &[("name", …)])`; deep in helpers
  `http::i18n::{tr, trf, trh}`; text stored now and shown later `bf_core::i18n::en("…")`
  (translate it where it is shown). Errors: `AppError::Validation("…")` or
  `AppError::invalid("… {name}", …)`; the error middleware translates them. Mail goes out in
  the recipient's language (`http::i18n::user_t`), not the sender's.
- Islands: `tr("English {name}", { name })` and `tn("count.key", n)` from `dom.js`.
- Admin panel: `tr(…)` / `tn(…)` / `locale()` from `#lib/i18n.js`, never `toLocaleString()`
  without `locale()`.
- The first argument is a literal (the catalog key is its words, `msg.first_like`; `sync` keeps
  `locales/keys.json` for the few texts whose words collide). A plural is one Fluent message,
  still called `tn("count.key", n)` / `layout.t.n("count.key", n)`: `count-topics = { $n -> [one] …
  [few] … *[other] … }` (CLDR categories: en one/other, pl one/few/many/other, Arabic all six; `*` marks
  the default). Never `n === 1 ? "x" : "xs"`. Translators may use Fluent selects for gender or case,
  `-term` and `{ message }` references and `NUMBER()`; hand-written messages go above the generated
  block of a catalog, which `sync` rewrites. The browser gets the catalogs compiled to JSON, and the
  overrides in the admin panel are Fluent patterns.
- Then `python scripts/i18n.py sync`, translate `target/i18n-missing.json` into Polish and
  Arabic (keep the `{name}` placeholders), `python scripts/i18n.py apply file.json`;
  `python scripts/i18n.py check` (also lints variables, plural categories and ids),
  `python scripts/test_i18n.py` and `cargo test -p bf-core i18n` must pass.
- Protocol text (ActivityPub, HTTP headers), logs and CLI output stay English.

## Before writing CSS, read the rules that already match

- Grep for the element and the classes around it (`.acp-form input`, `input[type="checkbox"]`,
  `.bf-field >`) and check which rule wins. A new rule loses to a more specific existing one:
  `input[type="color"]` lost to `.acp-form input:not(…)`.
- The site's CSS is in cascade layers (`critical.css`: `@layer tokens, base, app, a11y`); the admin
  panel's `admin.css` is not, so **any** admin rule beats any site rule whatever the specificity.
  An admin rule for an element the site already styles (focus rings, inputs, checkboxes) replaces
  the site's look in the panel: check the element on a site page and in the panel. Example: the
  admin `:focus-visible` outline drew a second ring around the site's focused-field border.
- `themes/default/css/components/fields.css` restyles every checkbox and radio, in the admin panel
  too: a control built on a checkbox (a switch) must undo the border, transform and tick it adds.
- A closed `<details>` contains its content's layout, so its absolutely placed panel can still
  stick out past the page edge. Hidden things (`visibility: hidden`, tooltips) still count toward
  the page width.
