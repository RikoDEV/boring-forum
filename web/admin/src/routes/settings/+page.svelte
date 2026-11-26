<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import Empty from "#lib/Empty.svelte";
  import { tick } from "svelte";
  import { page } from "$app/state";
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import SettingRows from "#lib/SettingRows.svelte";
  import SaveBar from "#lib/SaveBar.svelte";
  import { SettingsForm } from "#lib/settings-form.svelte.js";
  import { CODE, groupOf, humanize, meta, movedGroup, pageKinds, placeOf } from "#lib/settings-meta.js";
  import sprite from "../../../../../themes/default/icons.svg?url";

  const PAGE_KINDS = pageKinds();
  const { GROUPS, ICONS } = meta();
  const known = new Set(GROUPS.flatMap((g) => g[3]));

  const status = new Status();
  // What belongs to a feature is edited on that feature's page, not here.
  const form = new SettingsForm(status, (key) => !placeOf(key));
  const settings = $derived(form.settings);
  const byKey = $derived(form.byKey);
  let active = $state("general");
  /** The Custom code setting being edited: each has a tab of its own. */
  let codeTab = $state("custom_css");
  // The setting the header search sent us to: its group opens and it is scrolled into view.
  let found = $state("");
  let previewKind = $state("topic");

  // ALTCHA runs on this site, and "None" checks nothing: neither uses a site key.
  const hidden = (s) => s.key === "captcha_site_key" && ["", "altcha"].includes(byKey.get("captcha_provider")?.draft);
  const others = $derived(settings.filter((s) => !known.has(s.key)).map((s) => s.key));
  const groups = $derived([
    ...GROUPS.map(([id, title, hint, keys]) => ({
      id,
      title,
      hint,
      items: /** @type {string[]} */ (keys).map((k) => byKey.get(k)).filter((s) => s && !hidden(s)),
    })),
    .../** @type {[string, string, string, string[]][]} */ ([
      ["other", tr("Other"), tr("Settings without a group yet"), others],
    ])
      .filter((g) => g[3].length)
      .map(([id, title, hint, keys]) => ({ id, title, hint, items: keys.map((k) => byKey.get(k)) })),
  ]);
  const isDirty = (s) => form.isDirty(s);
  const shown = $derived(groups.filter((g) => g.id === active));
  /** The rows of a group: for Custom code, the one whose tab is open. */
  const rowsOf = (g) => (g.id === "custom" ? g.items.filter((s) => s.key === codeTab) : g.items);

  function choose(id) {
    active = id;
    history.replaceState(history.state, "", `#${id}`);
  }

  // An old link or a saved bookmark may name a setting, or a group, that now has a page of its
  // own: it goes there.
  const move = (to) => {
    const [path, hash] = to.split("#");
    goto(`${resolve(path)}${hash ? `#${hash}` : ""}`, { replaceState: true });
  };

  async function reveal(hash) {
    const m = /^#setting=(.+)$/.exec(hash);
    if (!m) {
      const wanted = hash.slice(1);
      if (movedGroup(wanted)) move(movedGroup(wanted));
      else if (GROUPS.some((g) => g[0] === wanted)) active = wanted;
      return;
    }
    const key = decodeURIComponent(m[1]);
    if (placeOf(key)) return move(placeOf(key));
    if (!byKey.has(key)) return;
    active = groupOf(key);
    if (key in CODE) codeTab = key;
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
    if (form.loaded) reveal(hash);
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
        {#if g.id === "custom"}
          <div class="acp-tabs acp-code-tabs" role="tablist" aria-label={g.title}>
            {#each g.items as s (s.key)}
              <button role="tab" aria-selected={codeTab === s.key} onclick={() => (codeTab = s.key)}>
                {humanize(s.key)}{#if isDirty(s)}<span class="acp-pill" title={tr("Unsaved changes")}>•</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
        <SettingRows {form} items={rowsOf(g)} {found} />
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
              src="/api/v1/admin/og-preview?kind={previewKind}&t={form.stamp}"
              alt={tr("Share picture preview")}
              width="1200"
              height="630"
            />
          </div>
        {/if}
      </section>
    {:else}
      {#if form.loaded}<section class="acp-box"><Empty title={tr("No settings in this group.")} /></section>{/if}
    {/each}
  </div>
</div>

<SaveBar {form} />
