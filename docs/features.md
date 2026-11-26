# Features and settings

## AI features

All optional and off until `ai_enabled` is set and a provider is given (Admin → Settings → AI;
Admin → AI for the connection test, usage and personas). The provider is anything that speaks the
OpenAI chat and embeddings API (OpenAI, Ollama, vLLM, OpenRouter, Gemini's compatibility endpoint)
or Anthropic's messages API. Summaries, gists, spam / abuse / mood checks, machine translation,
related topics, search by meaning, the editor helper, bot personas (`@ai-name`) and the automation
actions (`ai_question` condition, `ai_reply`, `ai_report`, `flag`) all go through one client that
logs every request in `ai_usage` and enforces `ai_monthly_token_limit` and `ai_user_daily_limit`.

- **Privacy.** Hidden (`[hide]`) blocks are stripped from everything sent. With `ai_public_only`
  (the default) only forums that guests can read are sent; private forums and messages never are.
  The key can live in `BF_AI_API_KEY` instead of the database. The provider address is set by an
  administrator and is called directly, so a local model on the same host works.
- **Embeddings** are one vector per topic in `ai_embeddings`, searched in memory (cosine, no
  extension needed). `ai_embedding_max_topics` caps the memory (50,000 topics at 1,536 numbers is
  about 300 MB; ask the provider for fewer with `ai_embedding_dims`). Changing the model rebuilds
  them in the background.
- **Moderation** runs after a post is public and never delays it. A post over the spam or abuse
  threshold is reported from the `plugin-ai` account into the review queue, held, or hidden
  (`ai_spam_action`).
- `ai_usage` rows are kept 400 days.

## Link click counts

A link in a post shows how many times it was clicked, as a small badge beside it (`161`, `1.2K`), like
Discourse. It covers web links, including full addresses of this forum, and leaves out pictures, mentions
and files. A link made into a preview card carries its count in the card's first line.

- A visitor counts once a day for a link: a member by account, a guest by network. Only a digest of the
  two is kept, in memory for that day; nothing is written down. The post's own author does not count.
- Admin, Settings, Link previews has the switch (`link_click_counts`, on by default). Off, nothing is
  counted and no badge is shown; the counts already kept stay.

## Who can open an uploaded file

A file may be fetched by those who may see a place it is used. `upload_uses` lists those places
(a post, a private message, a chat message) and is kept by database triggers, so every way a text
is saved is covered, imports and mail replies included. A file used nowhere (an avatar, an emoji,
a picture not posted yet) stays open.

- A file in a post: anyone who can read the post, guests included when a guest can read the
  forum. In a forum guests cannot read, signed-in members who can read it. Hidden and deleted
  posts show their files only to moderators of the forum.
- Opening an attached file (anything but a picture shown in the post) needs the forum's
  **Download** permission, a column in the forum's permission matrix (`download`, bit 64). Members who
  can read but not download see the post and its pictures, and get a 404 for the files. Migration
  `0246` gave the right to everyone who could already read a forum, and the default grants
  (new forums, clubs, imports) include it.
- A file in a private message: its participants. In chat: those who can see the channel.
- The uploader and administrators can always open it. A file that is also in a public post is
  public. Anyone else gets a 404, the same answer as for a file that does not exist.
- `uploads_members_only` turns guests away from every file used in content, public posts included.
- Files only some may have are sent `Cache-Control: private` with `Vary: Cookie`; the rest keep
  `public, immutable`. Uploads are never kept in the page microcache, since each request is checked.

**Hotlinking and expiring links.** `hotlink_protection` is `files` (default), `all` or `off`; it
covers documents and archives, and pictures too with `all`. For a covered file:

- A guest's page, feed or JSON answer carries every such link with a signature:
  `/uploads/<key>?x=<expires>.<signature>`. The stored text never holds one: links are signed on
  the way out, outside the page cache. A signed-in member is not rewritten and needs none.
- Someone who is not signed in is sent the file only with a valid signature, or when following a
  link in the browser (a top-level navigation, as from an email). A copied address is dead once
  it expires, and a plain request from a script or another site's page is a 403.
- A page on another site that loads the file in a browser is refused even with a good signature
  (`Sec-Fetch-Site: cross-site`, or a foreign `Referer` where the browser sends no Fetch Metadata).
  Someone who copies a signed link to their own server can use it until it expires.
- A link stays the same all window long, so browsers can cache the file. `upload_link_hours` (6)
  is the window; a link works for between one and two of them. A signed file may be cached by
  others for the time that is left; everything else is `private`.
- The key comes from `BF_SECRET_KEY`. **Set it when several nodes serve
  one site** (`BF_CLUSTER=1`): without it each process signs with a key of its own and the other
  nodes cannot check the links.
- Feed readers, link previews, other servers and API consumers that do not sign in must use the
  signed links in what they are sent; a bare `/uploads/` address of a covered file is refused to
  them. Pictures stay plain under `files`, so previews and federated posts keep working.

**Files nobody posted.** A file whose draft is thrown away with the Discard button is deleted at
once when nothing else uses it and the member uploaded it. A draft cleared by posting never
deletes files. Everything else unused is removed by the daily sweep after seven days.

## Upload scanning

**Known illegal images.** Organisations that fight child sexual abuse (NCMEC in the US, IWF in the
UK, the Tech Coalition's Lantern programme) share hash lists with platforms that sign up. Put the
list in a file, one `pdq:<64 hex>` or `sha256:<64 hex>` per line, and point `BF_IMAGE_HASHLIST`
at it; restart to reload. A match stores nothing, suspends the uploader, ends their sessions,
writes `upload.known_image` to the audit log and raises an admin notice. Reporting to the
authorities is the operator's legal duty and is not automated.

## Federation

Turn on `federation_enabled`. Members are then `@name@your.host` and public forums
`@forum.<slug>@your.host` on Mastodon and other ActivityPub servers. Only forums that guests can
read federate. New topics and replies go to followers, and replies from the fediverse become posts
by accounts named `name@their.host`. Those accounts cannot sign in and are left out of the member
list; moderate them like any member (suspending one refuses its further replies).

- `federation_blocked_domains`: instances to refuse, one per line; subdomains are included.
- `federation_posts_per_hour`: the most replies accepted from one instance in an hour.
- The base URL must be https and must not change once federating: actor ids are built from it.
- Outgoing deliveries are queued (`bf_queue_depth{queue="federation"}`) and retried for about
  four hours before they are dropped.

## EU Digital Services Act

- **Notices** (Art. 16): `/legal/notice`, linked from the footer, lets anyone report illegal
  content. Staff decide them at `/review/dsa`; the notifier gets a confirmation and the decision
  by email. Set `dsa_contact_email` for the single point of contact (Art. 11–12).
- **Statements of reasons** (Art. 17): recorded automatically when staff hide or delete someone
  else's post, when reports hide a post, and when an admin suspends or silences an account (send
  `reason` with the user update for the facts). The member is notified with a link.
- **Appeals** (Art. 20): from the statement page, for six months. Someone other than the original
  moderator decides at `/review/dsa`; reversing restores the post or lifts the restriction.
- **Transparency** (Art. 15, 24): `/transparency` reports each half-year.

The software records what the DSA asks for; whether your site is in scope, and the legal text of
your terms, are for you and your lawyer.

## Embedding comments

Set `embed_hosts` to the sites allowed, and `embed_forum` to a public forum for the topics. Then on a page of one of those sites: `<iframe src="https://forum.example/embed/comments?url=https%3A%2F%2Fblog.example%2Fpost" width="100%" height="400"></iframe>`. Pages are matched without their query string, and a page must exist to get a thread.

## Reply by email

Point your mail provider's inbound webhook at `/api/v1/webhooks/inbound-mail` with `Authorization: Bearer $BF_INBOUND_TOKEN` and a JSON body of `{"to": "...", "text": "..."}`. Pass the mail's headers as `"headers": {"Auto-Submitted": "..."}` where your provider offers them, so that automatic replies (out-of-office notes, delivery reports) are recognised and never posted. Replies follow the same rules as the site: permissions, limits, word screening and plugins. Providers with another format need a small adapter in front.

## Plugins and themes

`boring-forum plugin new <id>` creates a starting point, and `boring-forum theme watch <dir> <site-url> <admin-key>` pushes a theme on every save. Plugins are described in [plugins/getting-started.md](plugins/getting-started.md), and `tests/fixtures/plugin-ext/` is a working example of the host calls.

## Themes

**ACP → Appearance → Themes** lists the built-in look and every installed theme as a light and a
dark miniature, drawn from the colour schemes it shows. A theme's **Settings** tab holds:

- **Default** (`default_theme_id`): the theme everyone gets who has not chosen one.
- **On**, and **Theme can be selected by members** (`selectable`): off hides it from Settings → Interface.
- **Colour scheme, light mode / dark mode** (`themes.light_scheme`, `dark_scheme`): the schemes the
  theme shows, or the site's defaults when empty. They sit under the theme's own tokens in its
  stylesheet, so a member's own colour choice (`data-scheme`) still wins, and editing a scheme
  rebuilds every theme that uses it.
- **Only for these groups** (`themes.group_ids`): the theme is offered to, and applied for, only
  members of those groups, whatever the site's default or their own pick; others fall back to the
  site's default, then to the built-in look. Empty means everyone.

The built-in look can be turned off (`builtin_theme_enabled`) once an enabled theme is the site's
default. While it is off the default theme cannot be turned off, deleted or replaced by the
built-in look, and someone who may not use the default gets the first theme they may. The CSS of
the built-in look itself is always loaded; "off" means it is no longer a choice.

## Colour schemes

**ACP → Appearance → Colour schemes** edits the palettes members and guests see. A scheme is a
name and two palettes, one for light mode and one for dark, of eleven base colours: page
background, cards and panels, borders, text, secondary text, links and buttons, header background
and text, and success, warning and danger. Hover shades, soft backgrounds, strong borders and the
text on a button are worked out from them (`crates/bf-web/src/schemes.rs`), so changing the
accent changes every shade built on it. The editor warns when a pair falls under 4.5:1 contrast.

- Three schemes ship: Standard, Forest and Plum. Any can be edited, copied or,
  when it is not a default, deleted. **Make default** applies a scheme to everyone who has not
  picked one, guests included. Light mode and dark mode have their own default
  (`default_light_scheme`, `default_dark_scheme`): the light palette of one scheme and the dark
  palette of another can be the site's look. A default cannot be deleted, nor a scheme a theme uses.
- A scheme can be hidden (**Members can choose this scheme** off): it stays for the default and
  for those who already have it, and is no longer offered in Settings → Interface.
- All schemes are built into one stylesheet at `/scheme/<hash>.css`, linked after the built-in CSS
  and before a theme's, so a theme's own tokens still win. It is rebuilt on every change; the
  address is hash-named and cached forever. A member's pick selects a block with `data-scheme` on
  `<html>`; deleting a scheme returns its members to the default.
- Themes can still bring schemes of their own (`tokens.schemes`); they are listed after these.
- The admin panel itself keeps the built-in look.

## GIFs

The editor has a GIF button (also in the Insert menu and as `/gif`) when **Settings → GIFs** has
the picker switched on and a key. Providers: **KLIPY** and **GIPHY**; each is a small adapter in
`web/islands/src/editor/gif.js` and a line in `gif_hosts` (`crates/bf-web/src/security.rs`).

- **The browser talks to the provider, not this server.** KLIPY's integration terms require that
  API calls and media loads come from the member's browser and that its media is not stored or
  re-hosted, so the picker calls the provider's API directly and inserts the address it returns as
  an ordinary Markdown picture (`![title](https://…)`). For that reason the API key is not secret:
  it is on every page a signed-in member can write from (`<meta name="bf-gif">`). Use a key meant
  for browsers, and restrict it in the provider's panel when it can be.
- **Privacy:** members choosing a GIF, and everyone reading a post with one, contact the provider
  (their address and browser reach it). The picker sends an anonymous per-browser id, kept in
  local storage, for the provider's ranking. Say so in your privacy policy.
- **Content security policy:** with a provider on, `connect-src` gains its API origin; and when
  pictures are restricted to listed hosts (`csp_image_hosts`), its media hosts are added too. Only
  the chosen provider's hosts are ever opened.
- **Settings:** `gifs_enabled`, `gif_provider` (`klipy`, `giphy`), `gif_api_key`, `gif_format`
  (`webp`, `gif`), `gif_content_filter` (`off`, `low`, `medium`, `high`; GIPHY ratings g/pg/pg-13/r),
  `gif_country` (KLIPY), `gif_limit_scroll` and `gif_max_results`. KLIPY's terms ask that its
  search box reads "Search KLIPY" and that results appear as returned: the picker does both and
  does not reorder or filter them.
- GIFs are not in chat messages yet.

## Site traffic

The dashboard's **Site traffic** panel (`/api/v1/admin/traffic?days=7..90`) counts page views the
server itself serves: an HTML answer with status 200 to a GET, anywhere but `/admin` and `/api`
(prefetches are skipped). It sets no cookie and stores no address. Each row holds the path, the
referring site, a country when `BF_GEO_HEADER` names a header such as `CF-IPCountry`, who looked
(a signed-in member, an anonymous visitor or a crawler, told by `bot_name`), and a **visitor** that
is a hash of address, browser and the UTC day under `BF_SECRET_KEY`. The hash separates one day's
visitors, which is all that bounce rate (a visit of one page) and visit length (first to last view
of a day) need; it cannot be followed to another day or turned back into an address.

- Rows are written in batches every few seconds, off the request path; a full queue drops views
  rather than slowing a page. Rows older than 180 days are removed by the daily purge.
- Percentages, rates and the lists leave crawlers out; the chart draws them when switched on.
- Switch counting off with the `record_pageviews` setting (Settings, Security).

## imgproxy

**Settings → Uploads and media → imgproxy** sends pictures from other sites, and the picture on a
link-preview card, through your own [imgproxy](https://imgproxy.net): a reader's address and
browser never reach the other site, the picture is resized and recompressed, and an old post
keeps its picture when the other site takes it away.

- Set `imgproxy_url` (its origin), `imgproxy_key` and `imgproxy_salt` (imgproxy's `IMGPROXY_KEY`
  and `IMGPROXY_SALT`, in hex), then switch it on. With anything missing or not hex it stays off.
  The key and salt are signing secrets: the staff log records that they changed, not to what.
- Addresses are imgproxy's signed form, `{url}/{signature}/rs:fit:{width}:0:0/{base64url(source)}[.format]`
  (`imgproxy_width`, `imgproxy_format`). Posts keep the original address in their stored text and it
  is swapped on the way out, outside the page cache, so changing the settings applies to every
  post at once. A page's validator carries a fingerprint of the settings, so nothing stale is kept.
- Left alone: this site's own pictures, pictures on imgproxy's own host, media of GIF providers
  (KLIPY's and GIPHY's terms ask that it loads straight from them) and GIFs in general, since
  imgproxy keeps only the first frame of an animation unless its own animation settings are on
  (`imgproxy_gifs` sends them through anyway).
- When pictures are limited to listed hosts (`csp_image_hosts`), imgproxy's origin is added to
  the page's `img-src`; otherwise every https host is already allowed.

## Staff notes on posts

A moderator of a forum, and an administrator, can add a note to any post from its **⋯ menu → Add a
staff note**. Notes are shown under the post to staff only: they are never sent to anyone else,
and the post's author cannot see, edit or remove them. They live in their own table
(`post_notes`), not in the post's text, so editing or deleting the post leaves them. Staff who
moderate the forum delete a note from its bin button; adding and deleting are in the staff log
(`post.note.add`, `post.note.delete`).

## Badges

One pill, `.bf-badge` (`themes/default/css/components/badges.css`), is used for a state (Solved,
Locked, Pinned), a tag, a topic prefix, a group, an unread count and a staff note, in the topic
list, the hover card, the topic page, the profile and the review queue. `is-ok`, `is-warn`,
`is-bad` and `is-accent` give it a meaning; none is a quiet one. The admin panel's `.acp-pill` is
the same pill. `.tag`, `.prefix` and `.group-badge` still work for a theme's own markup.

## nofollow on links in posts

Every link in a post is cooked with `rel="nofollow ugc noopener"`. **Settings → Posts and topics**
relaxes that when the page is built, so a change applies to every post at once (no recook):

- **Links with nofollow** (`nofollow_links`): `all` (the default), `external` (links to other
  sites only; links to this site, `@mentions` and in-page links are followed) or `none`.
- **Trust level whose links are followed** (`nofollow_exempt_trust_level`): links in posts by
  members at or above it are followed; 5, the default, is above every level, so no one.
- **Sites whose links are followed** (`nofollow_exempt_domains`): domains, or addresses, separated
  by spaces or commas. A domain covers its subdomains (`friend.example` covers
  `blog.friend.example`, not `notfriend.example`).

A followed link keeps `rel="noopener"`; `ugc` goes with `nofollow`, since search engines read
`ugc` as a hint of the same kind. This applies to the topic page. Feeds, the API and federation
send the stock `nofollow ugc noopener`.

## Default language

**Settings → General → Default language** (`default_locale`) is **Browser-based** unless set: each
visitor who has not picked a language gets their browser's (`Accept-Language`), or English when it
asks for none of ours. Choosing a language instead gives that one to everyone who has not picked,
whatever their browser says. A member's own language and the language cookie (`bf_lang`) always
win. Mail to someone with no language of their own, digests, push and AI text use the chosen
language, or English while the setting is browser-based. A site that had saved English before
this setting existed becomes browser-based, which is what it already did.

## Permalinks

**ACP → Content → Permalinks** keeps redirects for addresses the forum does not know, so the links
of a forum moved here from elsewhere keep working. Each is an old address, like `c/old-forum/12`
or `viewtopic.php?t=7`, and what it points at: a forum or category, a topic, a post, a tag, a
member, or any address (on this site, or a web address).

- It applies when an address ends in a 404, whichever part of the forum answered; the visitor
  gets a **301**. The addresses of the forum itself, `/api` and `/admin` are never touched, and
  a permalink cannot point back at its own address.
- Matching ignores case, leading and trailing slashes and percent-escapes. An address with a
  query string is tried first, then without it (`viewtopic.php?t=7`, then `viewtopic.php`). Other
  parameters are dropped, not passed on.
- Targets are checked when saved. A permalink whose forum or topic is later deleted stays in the
  list marked "Missing" and does nothing, so the address keeps its ordinary 404.
- Adding, changing and deleting one is in the staff log (`permalink.add`, `.update`, `.delete`).
