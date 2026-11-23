<script>
  import { tn, tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import Empty from "#lib/Empty.svelte";
  import { onMount, tick } from "svelte";
  import { page } from "$app/state";
  import RankEditor from "#lib/RankEditor.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import { api, get, ApiError, uploadFile } from "#lib/api.js";
  import TreeSelect from "#lib/TreeSelect.svelte";
  import { code } from "#lib/code.js";
  import { CODE, CODE_LANG, groupOf, humanize, MANAGED, meta } from "#lib/settings-meta.js";
  import sprite from "../../../../../themes/default/icons.svg?url";

  // A key or secret arrives without its value, only with `set`: whether one is stored. Typing
  // replaces it, leaving the field empty keeps it, and `clear` removes it.
  const isSecret = (s) => "set" in s;
  const CHOICES = {
    ai_api_style: [
      ["openai", tr("OpenAI-compatible (OpenAI, Ollama, vLLM, OpenRouter, Gemini…)")],
      ["anthropic", "Anthropic"],
    ],
    ai_spam_action: [
      ["flag", tr("Send to the review queue")],
      ["hold", tr("Hold until a moderator approves")],
      ["hide", tr("Hide")],
    ],
    default_locale: [
      ["browser", tr("Browser-based")],
      ["en", "English"],
      ["pl", "Polski"],
      ["ar", "العربية"],
    ],
    post_style: [
      ["classic", tr("Classic (author column)")],
      ["discourse", tr("Discourse (avatar beside the text)")],
    ],
    forum_layout: [
      ["list", tr("List (rows with the latest post)")],
      ["boxes", tr("Boxes (a card for each forum)")],
      ["compact", tr("Compact (one line per forum)")],
      ["latest", tr("Forums and latest topics side by side")],
    ],
    hotlink_protection: [
      ["off", tr("Off: links never expire")],
      ["files", tr("Files: documents and archives, not pictures")],
      ["all", tr("Everything, pictures too")],
    ],
    edit_history_visible_to: [
      ["everyone", tr("Everyone")],
      ["members", tr("Members")],
      ["author", tr("The author")],
      ["staff", tr("Staff")],
    ],
    nofollow_links: [
      ["all", tr("All links")],
      ["external", tr("Links to other sites")],
      ["none", tr("No links")],
    ],
    imgproxy_format: [
      ["", tr("Chosen by the browser")],
      ["webp", "WebP"],
      ["avif", "AVIF"],
      ["jpeg", "JPEG"],
      ["png", "PNG"],
    ],
    og_template: [
      ["classic", tr("Classic: text on the left")],
      ["centered", tr("Centered")],
      ["poster", tr("Poster: a picture behind the text")],
    ],
    gif_provider: [
      ["klipy", "KLIPY"],
      ["giphy", "GIPHY"],
    ],
    gif_format: [
      ["webp", "WebP"],
      ["gif", "GIF"],
    ],
    gif_content_filter: [
      ["off", tr("Off")],
      ["low", tr("Low")],
      ["medium", tr("Medium")],
      ["high", tr("High")],
    ],
    analytics_provider: [
      ["", tr("None")],
      ["plausible", tr("Plausible (no cookies)")],
      ["umami", tr("Umami (no cookies)")],
      ["ga4", tr("Google Analytics 4 (cookies, after consent)")],
      ["matomo", tr("Matomo (cookies, after consent)")],
    ],
    captcha_provider: [
      ["", tr("None")],
      ["altcha", tr("ALTCHA (self-hosted, no account)")],
      ["turnstile", tr("Cloudflare Turnstile")],
      ["hcaptcha", "hCaptcha"],
      ["recaptcha", tr("Google reCAPTCHA v2")],
    ],
  };
  // Settings that hold several groups, as ids separated by commas.
  // For each: which groups may be ticked, and what none ticked means.
  const GROUP_LISTS = {
    translator_groups: [(g) => g.id > 1 && !g.is_staff, tr("Staff only")],
    referral_reward_groups: [(g) => g.id > 4, tr("No reward")],
  };
  // Settings that list file types, as names separated by commas. The server knows how to check
  // exactly these by their contents; nothing else can be added.
  const FILE_TYPES = [
    ["png", tr("PNG image")],
    ["jpg", tr("JPEG image")],
    ["gif", tr("GIF image")],
    ["webp", tr("WebP image")],
    ["pdf", tr("PDF document")],
    ["zip", tr("ZIP archive")],
    ["txt", tr("Text file")],
  ];
  const FILE_TYPE_LISTS = new Set(["upload_allowed_types", "chat_allowed_types"]);
  // The kinds of page a share picture can be drawn for, and the settings that are colours.
  const PAGE_KINDS = [
    ["home", tr("Home page")],
    ["topic", tr("Topics")],
    ["forum", tr("Forums")],
    ["tag", tr("Tags")],
    ["profile", tr("Profiles")],
    ["page", tr("Custom pages")],
    ["other", tr("Other public pages")],
  ];
  const kindsOf = (v) =>
    String(v ?? "")
      .split(",")
      .map((x) => x.trim())
      .filter((x) => PAGE_KINDS.some(([k]) => k === x));
  const COLOURS = new Set(["og_background_colour", "og_background_colour_end", "og_text_colour", "og_accent_colour"]);
  const HEX = /^#[0-9a-f]{6}$/i;
  // The preview is the saved look: it is drawn again after each save.
  let previewKind = $state("topic");
  let previewStamp = $state(Date.now());
  const typesOf = (v) =>
    String(v ?? "")
      .split(",")
      .map((x) => x.trim())
      .filter((x) => FILE_TYPES.some(([k]) => k === x));
  const idsOf = (v) =>
    String(v ?? "")
      .split(",")
      .map((x) => Number(x.trim()))
      .filter((x) => x > 0);
  // Settings that hold a forum or category id: (what 0 means, which kind may be chosen).
  const NODE_PICKERS = {
    clubs_parent_node: [tr("Top level"), "category"],
    embed_forum: [tr("Off (no embedding)"), "forum"],
  };
  const LONG = new Set([
    "blocked_link_domains",
    "review_link_domains",
    "blocked_user_agents",
    "login_apple_key",
    "cookie_consent_text",
    "offline_message",
    "blocked_email_domains",
    "allowed_email_domains",
    "reserved_usernames",
    "sidebar_links",
    "csp_image_hosts",
    "onebox_blocked_hosts",
    "embed_hosts",
  ]);
  const { GROUPS, ICONS, IMAGES } = meta();
  const known = new Set(GROUPS.flatMap((g) => g[3]));

  let settings = $state([]);
  let loaded = $state(false);
  const status = new Status();
  let saving = $state(false);
  let active = $state("general");
  // The setting the header search sent us to: its group opens and it is scrolled into view.
  let found = $state("");

  // ALTCHA runs on this site, and "None" checks nothing: neither uses a site key.
  const hidden = (s) => s.key === "captcha_site_key" && ["", "altcha"].includes(byKey.get("captcha_provider")?.draft);
  const byKey = $derived(new Map(settings.map((s) => [s.key, s])));
  const others = $derived(settings.filter((s) => !known.has(s.key) && !s.key.startsWith("plugin:")).map((s) => s.key));
  const fromPlugins = $derived(settings.filter((s) => s.key.startsWith("plugin:")).map((s) => s.key));
  const groups = $derived([
    ...GROUPS.map(([id, title, hint, keys]) => ({
      id,
      title,
      hint,
      items: /** @type {string[]} */ (keys).map((k) => byKey.get(k)).filter((s) => s && !hidden(s)),
    })),
    .../** @type {[string, string, string, string[]][]} */ ([
      ["other", tr("Other"), tr("Settings without a group yet"), others],
      ["plugins", tr("From plugins"), tr("Settings that installed plugins ask for"), fromPlugins],
    ])
      .filter((g) => g[3].length)
      .map(([id, title, hint, keys]) => ({ id, title, hint, items: keys.map((k) => byKey.get(k)) })),
  ]);
  const value = (s) => (s.type === "int" ? Number(s.draft) : s.draft);
  const isDirty = (s) => s.clear || value(s) !== s.value;
  const dirty = $derived(settings.filter(isDirty));
  const shown = $derived(groups.filter((g) => g.id === active));

  let groupList = $state([]);
  let nodeList = $state([]);

  async function loadPickers() {
    const [g, t] = await Promise.all([get("/groups"), get("/tree")]);
    groupList = g;
    nodeList = t.nodes ?? [];
  }

  async function load() {
    loadPickers().catch(() => {});
    try {
      const data = await get("/settings");
      settings = data.filter((s) => !MANAGED.has(s.key)).map((s) => ({ ...s, draft: s.value, clear: false }));
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }

  async function save() {
    status.clear();
    saving = true;
    const failures = [];
    let done = 0;
    for (const s of [...dirty]) {
      try {
        if (isSecret(s)) {
          // The value is not kept here either: the field goes back to "saved" or empty.
          const typed = s.draft;
          await api("PUT", `/settings/${s.key}`, typed ? { value: typed } : { value: "", clear: true });
          s.set = Boolean(typed);
          s.draft = "";
          s.clear = false;
        } else {
          await api("PUT", `/settings/${s.key}`, { value: value(s) });
          s.value = value(s);
        }
        done += 1;
        if (s.key === "site_name") window.dispatchEvent(new CustomEvent("bf-site-name", { detail: s.value }));
      } catch (e) {
        // The password dialog was dismissed: stop, rather than ask again for every setting.
        if (e instanceof ApiError && e.sudo) {
          failures.push(tr("Not saved: confirm your password to change settings."));
          break;
        }
        failures.push(`${humanize(s.key)}: ${e.message}`);
      }
    }
    saving = false;
    if (done) previewStamp = Date.now();
    if (failures.length) status.error = failures.join(" · ");
    if (done) status.msg = tn("admin.count.settings_saved", done);
  }

  // A picture goes to the uploads first; the setting then holds its path, saved with the rest.
  async function uploadImage(s, e) {
    const file = e.currentTarget.files[0];
    e.currentTarget.value = "";
    if (!file) return;
    status.clear();
    try {
      s.draft = (await uploadFile(file)).url.split("#")[0];
    } catch (err) {
      status.fail(err);
    }
  }

  function discard() {
    for (const s of [...dirty]) {
      s.draft = s.value;
      s.clear = false;
    }
    status.clear();
  }

  function choose(id) {
    active = id;
    history.replaceState(history.state, "", `#${id}`);
  }

  async function reveal(hash) {
    const m = /^#setting=(.+)$/.exec(hash);
    if (!m) {
      const wanted = hash.slice(1);
      if (GROUPS.some((g) => g[0] === wanted)) active = wanted;
      return;
    }
    const key = decodeURIComponent(m[1]);
    if (!byKey.has(key)) return;
    active = groupOf(key);
    found = key;
    await tick();
    const row = document.getElementById(`setting-${key}`);
    row?.scrollIntoView({ block: "center", behavior: "smooth" });
    row?.querySelector("input, select, textarea, button")?.focus({ preventScroll: true });
    setTimeout(() => (found = found === key ? "" : found), 2500);
  }

  // The settings arrive after the page does; a search result waits for them.
  $effect(() => {
    const hash = page.url.hash;
    if (loaded) reveal(hash);
  });

  onMount(() => {
    load();
    const warn = (e) => {
      if (dirty.length) e.preventDefault();
    };
    addEventListener("beforeunload", warn);
    return () => removeEventListener("beforeunload", warn);
  });
</script>

<PageHead title={tr("Settings")} {status} />

<div class="acp-settings">
  <nav class="acp-settings-nav" aria-label={tr("Setting groups")}>
    {#each groups as g (g.id)}
      {@const changed = g.items.filter(isDirty).length}
      <button
        type="button"
        class="acp-ghost"
        aria-current={active === g.id ? "true" : undefined}
        onclick={() => choose(g.id)}
      >
        <span class="acp-settings-tab">
          <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{ICONS[g.id]}"></use></svg>{g.title}
        </span>{#if changed}<span class="acp-pill" title={tr("Unsaved changes")}>{changed}</span>{/if}
      </button>
    {/each}
  </nav>
  <div class="acp-settings-body">
    {#each shown as g (g.id)}
      <section class="acp-box">
        <div class="acp-settings-title">
          <h2>{g.title}</h2>
          <p class="muted">{g.hint}</p>
        </div>
        <ul class="acp-rows">
          {#each g.items as s (s.key)}
            <li
              id="setting-{s.key}"
              class="acp-set"
              class:is-found={found === s.key}
              class:is-image={s.key in IMAGES}
              class:is-wide={s.key === "rank_names" ||
                s.key in CODE ||
                LONG.has(s.key) ||
                String(s.default).includes("\n")}
            >
              <div class="acp-set-text">
                <label for={s.key === "rank_names" ? undefined : s.key} title={s.key}>{humanize(s.key)}</label>
                <p>{s.description}</p>
              </div>
              <div class="acp-set-control">
                {#if s.key in IMAGES}
                  <div class="acp-image-set">
                    <span class="acp-image-preview is-{IMAGES[s.key]}">
                      {#if s.draft}<img src={s.draft} alt="" />{:else}<span class="muted">{tr("None")}</span>{/if}
                    </span>
                    <label class="acp-upload secondary">
                      {tr("Upload")}<input
                        id={s.key}
                        type="file"
                        accept="image/png,image/jpeg,image/webp,image/gif"
                        onchange={(e) => uploadImage(s, e)}
                      />
                    </label>
                    {#if s.draft}<button type="button" class="secondary" onclick={() => (s.draft = "")}>
                        {tr("Remove")}
                      </button>{/if}
                  </div>
                {:else if s.key === "rank_names"}
                  <RankEditor id={s.key} bind:value={s.draft} />
                {:else if s.type === "bool"}
                  <input id={s.key} class="acp-switch" type="checkbox" role="switch" bind:checked={s.draft} />
                {:else if FILE_TYPE_LISTS.has(s.key)}
                  <MultiSelect
                    id={s.key}
                    label={humanize(s.key)}
                    hideLabel
                    placeholder={tr("None")}
                    options={FILE_TYPES.map(([value, label]) => ({ value, label }))}
                    bind:value={
                      () => typesOf(s.draft),
                      (types) =>
                        (s.draft = FILE_TYPES.map(([k]) => k)
                          .filter((k) => types.includes(k))
                          .join(","))
                    }
                  />
                {:else if s.key === "og_kinds"}
                  <MultiSelect
                    id={s.key}
                    label={humanize(s.key)}
                    hideLabel
                    placeholder={tr("None")}
                    options={PAGE_KINDS.map(([value, label]) => ({ value, label }))}
                    bind:value={
                      () => kindsOf(s.draft),
                      (kinds) =>
                        (s.draft = PAGE_KINDS.map(([k]) => k)
                          .filter((k) => kinds.includes(k))
                          .join(","))
                    }
                  />
                {:else if COLOURS.has(s.key)}
                  <div class="acp-group-style">
                    <label class="acp-colour-input" class:is-empty={!HEX.test(s.draft)} title={tr("Choose a colour")}>
                      <input
                        type="color"
                        value={HEX.test(s.draft) ? s.draft : "#3b82f6"}
                        oninput={(e) => (s.draft = e.currentTarget.value)}
                        aria-label={humanize(s.key)}
                      />
                    </label>
                    <input
                      id={s.key}
                      type="text"
                      class="acp-colour-text"
                      maxlength="7"
                      spellcheck="false"
                      bind:value={s.draft}
                    />
                    {#if s.key === "og_background_colour_end" && s.draft}<button
                        type="button"
                        class="secondary"
                        onclick={() => (s.draft = "")}
                      >
                        {tr("No gradient")}
                      </button>{/if}
                  </div>
                {:else if s.key in GROUP_LISTS}
                  <MultiSelect
                    id={s.key}
                    label={humanize(s.key)}
                    hideLabel
                    placeholder={GROUP_LISTS[s.key][1]}
                    options={groupList.filter(GROUP_LISTS[s.key][0]).map((g) => ({ value: g.id, label: g.name }))}
                    bind:value={() => idsOf(s.draft), (ids) => (s.draft = [...ids].sort((a, b) => a - b).join(","))}
                  />
                {:else if s.key in NODE_PICKERS}
                  <TreeSelect
                    id={s.key}
                    rows={nodeList}
                    bind:value={s.draft}
                    none={NODE_PICKERS[s.key][0]}
                    noneValue={0}
                    kind={NODE_PICKERS[s.key][1]}
                  />
                {:else if s.type === "int"}
                  <input id={s.key} type="number" min={s.min} max={s.max} bind:value={s.draft} />
                {:else if CHOICES[s.key]}
                  <select id={s.key} bind:value={s.draft}>
                    {#each CHOICES[s.key] as [v, label]}<option value={v}>{label}</option>{/each}
                  </select>
                {:else if s.key in CODE}
                  <textarea
                    id={s.key}
                    class="acp-code"
                    rows={CODE[s.key]}
                    spellcheck="false"
                    autocapitalize="off"
                    autocomplete="off"
                    use:code={{ lang: CODE_LANG[s.key], value: s.draft }}
                    bind:value={s.draft}></textarea>
                {:else if isSecret(s)}
                  <div class="acp-secret-set">
                    <input
                      id={s.key}
                      type="password"
                      autocomplete="off"
                      spellcheck="false"
                      placeholder={s.clear ? tr("Removed when you save") : s.set ? tr("Saved; type to replace") : ""}
                      bind:value={s.draft}
                    />
                    {#if s.set && !s.clear}<button
                        type="button"
                        class="secondary"
                        onclick={() => {
                          s.draft = "";
                          s.clear = true;
                        }}
                      >
                        {tr("Remove")}
                      </button>{/if}
                  </div>
                {:else if LONG.has(s.key) || String(s.default).includes("\n")}
                  <textarea id={s.key} rows={String(s.default).includes("\n") ? 8 : 3} bind:value={s.draft}></textarea>
                {:else}
                  <input id={s.key} type="text" required={s.key === "site_name"} bind:value={s.draft} />
                {/if}
              </div>
            </li>
          {/each}
        </ul>
        {#if g.id === "share"}
          <div class="acp-og-preview">
            <div class="acp-og-bar">
              <label class="acp-og-kind">
                {tr("Preview as")}
                <select bind:value={previewKind}>
                  {#each PAGE_KINDS as [v, label]}<option value={v}>{label}</option>{/each}
                </select>
              </label>
              <span class="muted">{tr("Shows the saved settings; save your changes to see them here.")}</span>
            </div>
            <img
              src="/api/v1/admin/og-preview?kind={previewKind}&t={previewStamp}"
              alt={tr("Share picture preview")}
              width="1200"
              height="630"
            />
          </div>
        {/if}
      </section>
    {:else}
      {#if loaded}<section class="acp-box"><Empty title={tr("No settings in this group.")} /></section>{/if}
    {/each}
  </div>
</div>

{#if dirty.length}
  <div class="acp-savebar" role="region" aria-label={tr("Unsaved changes")}>
    <span>{tn("admin.count.unsaved", dirty.length)}</span>
    <button type="button" class="secondary" disabled={saving} onclick={discard}>{tr("Discard")}</button>
    <button type="button" disabled={saving} onclick={save}>{saving ? tr("Saving…") : tr("Save changes")}</button>
  </div>
{/if}
