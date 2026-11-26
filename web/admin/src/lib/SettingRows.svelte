<script>
  /// The rows of a list of settings: a label and its description, and the control that suits the
  /// setting. The Settings page and every feature page with settings draw them with this.
  import { tr } from "#lib/i18n.js";
  import RankEditor from "#lib/RankEditor.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import TreeSelect from "#lib/TreeSelect.svelte";
  import { code } from "#lib/code.js";
  import { isSecret } from "#lib/settings-form.svelte.js";
  import { CODE, CODE_LANG, humanize, meta, pageKinds } from "#lib/settings-meta.js";

  /** @type {{ form: import("#lib/settings-form.svelte.js").SettingsForm, items: any[], found?: string }} */
  let { form, items, found = "" } = $props();

  const { IMAGES } = meta();
  const PAGE_KINDS = pageKinds();
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
  const kindsOf = (v) =>
    String(v ?? "")
      .split(",")
      .map((x) => x.trim())
      .filter((x) => PAGE_KINDS.some(([k]) => k === x));
  const COLOURS = new Set([
    "og_background_colour",
    "og_background_colour_end",
    "og_text_colour",
    "og_accent_colour",
    "mail_accent_colour",
  ]);
  const HEX = /^#[0-9a-f]{6}$/i;
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
    "csp_image_hosts",
    "onebox_blocked_hosts",
    "embed_hosts",
  ]);
</script>

<ul class="acp-rows">
  {#each items as s (s.key)}
    <li
      id="setting-{s.key}"
      class="acp-set"
      class:is-found={found === s.key}
      class:is-image={s.key in IMAGES}
      class:is-wide={s.key === "rank_names" || s.key in CODE || LONG.has(s.key) || String(s.default).includes("\n")}
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
                onchange={(e) => form.uploadImage(s, e)}
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
            options={form.groupList.filter(GROUP_LISTS[s.key][0]).map((g) => ({ value: g.id, label: g.name }))}
            bind:value={() => idsOf(s.draft), (ids) => (s.draft = [...ids].sort((a, b) => a - b).join(","))}
          />
        {:else if s.key in NODE_PICKERS}
          <TreeSelect
            id={s.key}
            rows={form.nodeList}
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
          <div class="acp-code-pane">
            <textarea
              id={s.key}
              class="acp-code"
              rows={CODE[s.key]}
              spellcheck="false"
              autocapitalize="off"
              autocomplete="off"
              use:code={{ lang: CODE_LANG[s.key], value: s.draft }}
              bind:value={s.draft}></textarea>
            {#if s.key === "robots_txt"}
              <div class="acp-code-actions">
                <a class="btn secondary" href="/robots.txt" target="_blank" rel="noopener">
                  {tr("View live file")}
                </a>
                <button type="button" class="secondary" onclick={() => (s.draft = s.default)}>
                  {tr("Reset to default")}
                </button>
              </div>
            {/if}
          </div>
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
