<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, patch, post, put, upload as sendFile } from "#lib/api.js";
  import EditDialog from "#lib/EditDialog.svelte";
  import sprite from "../../../../../themes/default/icons.svg?url";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import Switch from "#lib/Switch.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import SchemePreview from "#lib/SchemePreview.svelte";
  import { resolve } from "$app/paths";

  const base = resolve("/").replace(/\/$/, "");

  let themes = $state([]);
  let builtinOn = $state(true);
  let schemes = $state([]);
  let baseColours = $state([]);
  let shades = $state([]);
  let defaultLight = $state("");
  let defaultDark = $state("");
  let groups = $state([]);
  let slots = $state([]);
  let defaultId = $state(0);
  let selected = $state(null);
  let savedCopy = $state("");
  const status = new Status();
  let fresh = $state({ slug: "", name: "", kind: "theme" });
  let url = $state("");
  let adding = $state(false);
  let source = $state("new");
  let frame = $state(null);
  let part = $state("settings");
  let previewing = $state(false);

  const GROUPS = [
    [
      tr("Page"),
      [
        ["bg", tr("Background")],
        ["surface", tr("Boxes and cards")],
        ["border", tr("Borders")],
      ],
    ],
    [
      tr("Text"),
      [
        ["text", tr("Text")],
        ["muted", tr("Quiet text")],
      ],
    ],
    [
      tr("Accent"),
      [
        ["accent", tr("Links and buttons")],
        ["accent-text", tr("Text on buttons")],
      ],
    ],
    [
      tr("Header"),
      [
        ["header-bg", tr("Header background")],
        ["header-text", tr("Header text")],
      ],
    ],
  ];
  const PAIRS = [
    ["text", "surface"],
    ["text", "bg"],
    ["muted", "surface"],
    ["accent", "surface"],
    ["accent-text", "accent"],
    ["header-text", "header-bg"],
  ];
  const LABEL = Object.fromEntries(GROUPS.flatMap(([, rows]) => /** @type {string[][]} */ (rows)));
  const isHex = (c) => /^#[0-9a-f]{6}$/i.test(c ?? "");

  // WCAG contrast of two #rrggbb colours; null when either is something else.
  function contrast(a, b) {
    const lum = (c) => {
      if (!isHex(c)) return null;
      const [r, g, bl] = [1, 3, 5]
        .map((i) => parseInt(c.slice(i, i + 2), 16) / 255)
        .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
      return 0.2126 * r + 0.7152 * g + 0.0722 * bl;
    };
    const [x, y] = [lum(a), lum(b)];
    return x === null || y === null ? null : (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
  }
  const lowContrast = (map, mode) =>
    PAIRS.flatMap(([fg, bg]) => {
      const r = contrast(map?.[fg], map?.[bg]);
      return r !== null && r < 4.5
        ? [
            tr("{mode}: {n} on {n2} is {n3}:1; aim for at least 4.5:1.", {
              mode,
              n: LABEL[fg],
              n2: LABEL[bg],
              n3: r.toFixed(1),
            }),
          ]
        : [];
    });
  const warnings = $derived(
    selected ? [...lowContrast(selected.light, tr("Light")), ...lowContrast(selected.dark, tr("Dark"))] : [],
  );

  const snapshot = (s) =>
    JSON.stringify([s.light, s.dark, s.css, s.slotText, s.enabled, s.selectable, s.light_scheme, s.dark_scheme, s.group_ids]);
  const dirty = $derived(!!selected && snapshot(selected) !== savedCopy);
  const swatches = (t) => {
    const l = t.tokens?.light ?? {};
    return [l["header-bg"], l.bg, l.surface, l.accent].filter(Boolean);
  };
  // The palettes a theme shows: the schemes it chose, or the site's defaults. Null is the built-in look.
  const scheme = (slug) => schemes.find((x) => x.slug === slug) ?? schemes[0];
  const look = (t) => ({
    light: scheme(t?.light_scheme || defaultLight)?.light ?? {},
    dark: scheme(t?.dark_scheme || defaultDark)?.dark ?? {},
  });
  const schemeOptions = $derived([
    ["", tr("Site default")],
    ...schemes.map((x) => [x.slug, x.name]),
  ]);
  const groupOptions = $derived(groups.map((g) => ({ value: String(g.id), label: g.name })));
  const groupNames = (t) => t.group_ids.map((id) => groups.find((g) => g.id === id)?.name).filter(Boolean).join(", ");
  const realThemes = $derived(themes.filter((t) => t.kind === "theme"));
  const components = $derived(themes.filter((t) => t.kind !== "theme"));

  async function load() {
    try {
      const r = await get("/themes");
      themes = r.themes;
      slots = r.slots;
      defaultId = r.default;
      builtinOn = r.builtin_enabled;
      const c = await get("/color-schemes");
      schemes = c.schemes;
      baseColours = c.base;
      shades = c.derived;
      defaultLight = c.default_light;
      defaultDark = c.default_dark;
      groups = await get("/groups");
    } catch (e) {
      status.fail(e);
    }
  }
  function open(t) {
    selected = {
      ...t,
      css: t.css ?? "",
      light: { ...(t.tokens?.light ?? {}) },
      dark: { ...(t.tokens?.dark ?? {}) },
      slotText: { ...(t.slots ?? {}) },
      light_scheme: t.light_scheme ?? "",
      dark_scheme: t.dark_scheme ?? "",
      group_ids: (t.group_ids ?? []).map(String),
    };
    savedCopy = snapshot(selected);
    part = t.kind === "theme" ? "settings" : "colours";
    status.clear();
  }
  function close() {
    if (dirty && !confirm(tr("Leave without saving your changes?"))) return;
    if (previewing) stopPreview();
    selected = null;
    status.msg = "";
  }
  async function create() {
    status.clear();
    try {
      await post("/themes", fresh);
      fresh = { slug: "", name: "", kind: "theme" };
      adding = false;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
  async function save() {
    status.clear();
    const clean = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v));
    try {
      await patch(`/themes/${selected.id}`, {
        tokens: { ...selected.tokens, light: clean(selected.light), dark: clean(selected.dark) },
        css: selected.css,
        slots: clean(selected.slotText),
        enabled: selected.enabled,
        selectable: selected.selectable,
        light_scheme: selected.light_scheme,
        dark_scheme: selected.dark_scheme,
        group_ids: selected.group_ids.map(Number),
      });
      savedCopy = snapshot(selected);
      status.msg = tr("Saved.");
      await load();
      frame?.contentWindow?.location.reload();
    } catch (e) {
      status.fail(e);
    }
  }
  const startPreview = () =>
    post(`/themes/${selected.id}/preview`).then(() => {
      previewing = true;
      frame?.contentWindow?.location.reload();
    }, status.fail);
  const stopPreview = () =>
    post("/themes/preview/stop").then(() => {
      previewing = false;
      frame?.contentWindow?.location.reload();
    }, status.fail);
  const makeDefault = (id) => put("/settings/default_theme_id", { value: id }).then(load, status.fail);
  const setBuiltin = (on) => put("/settings/builtin_theme_enabled", { value: on }).then(load, status.fail);
  const toggle = (t) => patch(`/themes/${t.id}`, { enabled: !t.enabled }).then(load, status.fail);
  const remove = (t) =>
    confirm(tr("Delete {name}? This cannot be undone.", { name: t.name })) &&
    del(`/themes/${t.id}`).then(load, status.fail);
  async function upload(form) {
    status.clear();
    const body = new FormData();
    body.append("file", form.file.files[0]);
    try {
      await sendFile("/api/v1/admin/themes/install", body);
      adding = false;
      load();
    } catch (err) {
      status.error = err.message || tr("Install failed");
    }
  }
  const fromUrl = () => {
    status.clear();
    post("/themes/install-url", { url }).then(() => {
      url = "";
      adding = false;
      load();
    }, status.fail);
  };
  function addTheme(e) {
    e.preventDefault();
    if (source === "new") return create();
    if (source === "zip") return upload(e.currentTarget);
    return fromUrl();
  }
  load();
</script>

{#snippet card(t)}
  <li class="acp-theme" class:is-off={!t.enabled}>
    <button class="acp-theme-look has-preview" onclick={() => open(t)} aria-label={tr("Edit {name}", { name: t.name })}>
      {#if t.kind === "theme"}
        <SchemePreview palette={look(t).light} mode="light" base={baseColours} {shades} compact />
        <SchemePreview palette={look(t).dark} mode="dark" base={baseColours} {shades} compact />
      {:else}<span class="muted"><small>{tr("Adds to every theme")}</small></span>{/if}
    </button>
    <div class="acp-theme-body">
      <div class="acp-grow">
        <strong>{t.name}</strong><small class="muted">{tr("Version {version}", { version: t.version })}</small>
      </div>
      {#if t.id === defaultId}<span class="acp-pill is-ok">{tr("Default")}</span>{/if}
      {#if !t.enabled}<span class="acp-pill is-muted">{tr("Off")}</span>{/if}
      {#if t.kind === "theme" && t.enabled && !t.selectable}<span class="acp-pill is-muted">{tr("Hidden")}</span>{/if}
      {#if t.group_ids.length}<span class="acp-pill is-muted" title={groupNames(t)}>{tr("Limited to groups")}</span>{/if}
    </div>
    <div class="acp-actions">
      <button class="secondary" onclick={() => open(t)}>{tr("Edit")}</button>
      {#if t.kind === "theme" && t.id !== defaultId && t.enabled}<button
          class="secondary"
          onclick={() => makeDefault(t.id)}>{tr("Make default")}</button
        >{/if}
      <details class="acp-more">
        <summary class="acp-icon-btn" aria-label={tr("More for {name}", { name: t.name })}
          ><svg class="bf-icon" aria-hidden="true"><use href="{sprite}#chevron-down"></use></svg></summary
        >
        <div>
          <button class="secondary" onclick={() => toggle(t)}>{t.enabled ? tr("Turn off") : tr("Turn on")}</button>
          {#if t.id === defaultId && builtinOn}<button class="secondary" onclick={() => makeDefault(0)}
              >{tr("Use the built-in look")}</button
            >{/if}
          <button class="is-danger" onclick={() => remove(t)}>{tr("Delete")}</button>
        </div>
      </details>
    </div>
  </li>
{/snippet}

{#if !selected}
  <PageHead title={tr("Themes")}>
    {#snippet intro()}{tr("Choose how the forum looks. If a theme breaks the site, open")}
      <a href="/safe-mode"><code>/safe-mode</code></a>
      {tr("to see it without themes.")}{/snippet}
    <button onclick={() => (adding = true)}>{tr("Add a theme")}</button>
  </PageHead>
  {#if status.error && !adding}<p class="err" role="alert">{status.error}</p>{/if}
  {#if adding}
    <EditDialog
      title={tr("Add a theme")}
      onclose={() => (adding = false)}
      onsubmit={addTheme}
      {status}
      save={source === "new" ? tr("Create") : tr("Install")}
    >
      <div class="acp-form">
        <Field
          label={tr("Source")}
          bind:value={source}
          wide
          options={[
            ["new", tr("Start a new one")],
            ["zip", tr("Upload a zip")],
            ["url", tr("Install from an address")],
          ]}
        />
        {#if source === "new"}
          <Field label={tr("Name")} bind:value={fresh.name} required />
          <Field label={tr("Id: id-with-dashes")} bind:value={fresh.slug} required />
          <Field label={tr("Type")} bind:value={fresh.kind} wide
            ><option value="theme">{tr("Theme")}</option><option value="component"
              >{tr("Component (adds to any theme)")}</option
            ></Field
          >
        {:else if source === "zip"}
          <div class="acp-wide">
            <input type="file" name="file" accept=".zip" required aria-label={tr("Theme zip file")} />
          </div>
        {:else}
          <Field label={tr("Zip address: https://host/owner/repo/archive/v1.zip")} type="url" bind:value={url} wide required />
        {/if}
      </div>
    </EditDialog>
  {/if}
  <ul class="acp-themes" aria-label={tr("Themes")}>
    <li class="acp-theme" class:is-off={!builtinOn}>
      <div class="acp-theme-look has-preview is-builtin">
        <SchemePreview palette={look(null).light} mode="light" base={baseColours} {shades} compact />
        <SchemePreview palette={look(null).dark} mode="dark" base={baseColours} {shades} compact />
      </div>
      <div class="acp-theme-body">
        <div class="acp-grow">
          <strong>{tr("Boring Forum")}</strong><small class="muted">{tr("Comes with the forum")}</small>
        </div>
        {#if !defaultId}<span class="acp-pill is-ok">{tr("Default")}</span>{/if}
        {#if !builtinOn}<span class="acp-pill is-muted">{tr("Off")}</span>{/if}
      </div>
      <div class="acp-actions">
        {#if defaultId && builtinOn}<button class="secondary" onclick={() => makeDefault(0)}>{tr("Make default")}</button>{/if}
        {#if builtinOn && defaultId}
          <button class="secondary" onclick={() => setBuiltin(false)}>{tr("Turn off")}</button>
        {:else if !builtinOn}
          <button class="secondary" onclick={() => setBuiltin(true)}>{tr("Turn on")}</button>
        {/if}
      </div>
    </li>
    {#each realThemes as t (t.id)}{@render card(t)}{/each}
  </ul>
  {#if components.length}
    <h2>{tr("Components")}</h2>
    <p class="muted">{tr("Small additions that apply on top of whichever theme is in use.")}</p>
    <ul class="acp-themes">
      {#each components as t (t.id)}{@render card(t)}{/each}
    </ul>
  {/if}
{:else}
  <div class="acp-head">
    <div>
      <button class="acp-link" onclick={close}>{tr("← All themes")}</button>
      <h1>{selected.name}</h1>
    </div>
    <div class="acp-actions">
      {#if dirty}<span class="muted"><small>{tr("Unsaved changes")}</small></span>{:else if status.msg}<span
          class="muted"
          role="status"><small>{status.msg}</small></span
        >{/if}
      <button onclick={save} disabled={!dirty}>{tr("Save")}</button>
    </div>
  </div>
  {#if status.error}<p class="err" role="alert">{status.error}</p>{/if}
  <div class="acp-tabs" role="tablist">
    {#each [...(selected.kind === "theme" ? [["settings", tr("Settings")]] : []), ["colours", tr("Colour tweaks")], ["css", "CSS"], ["slots", tr("Template slots")], ["preview", tr("Preview")]] as [k, label]}<button
        role="tab"
        aria-selected={part === k}
        onclick={() => (part = k)}>{label}</button
      >{/each}
  </div>

  {#if part === "settings"}
    <section class="acp-box">
      <Section switches>
        <Switch check checked={selected.id === defaultId} disabled={selected.id === defaultId && !builtinOn}
          onchange={(e) => makeDefault(e.currentTarget.checked ? selected.id : 0)}
          >{tr("Theme is the site's default, for everyone who has not chosen one")}</Switch
        >
        <Switch check bind:checked={selected.enabled}>{tr("Theme is on")}</Switch>
        <Switch check bind:checked={selected.selectable}>{tr("Theme can be selected by members")}</Switch>
      </Section>
      <Section legend={tr("Colour schemes")} grid hint={tr("The schemes this theme shows. A member's own colour choice still wins.")}>
        <Field label={tr("Colour scheme, light mode")} bind:value={selected.light_scheme} options={schemeOptions} />
        <Field label={tr("Colour scheme, dark mode")} bind:value={selected.dark_scheme} options={schemeOptions} />
      </Section>
      <p class="muted acp-pad"><a href="{base}/colours/">{tr("Edit colour schemes")}</a></p>
      <Section legend={tr("Who can use it")} grid hint={tr("Limited to these groups, the theme is not offered to anyone else, whatever the site's default or their choice. Empty means everyone.")}>
        <MultiSelect label={tr("Only for these groups")} placeholder={tr("Everyone")} options={groupOptions} bind:value={selected.group_ids} />
      </Section>
    </section>
    <section class="acp-box acp-scheme-previews is-row" aria-label={tr("Preview")}>
      <SchemePreview palette={look(selected).light} mode="light" title={selected.name} label={tr("Light")} base={baseColours} {shades} />
      <SchemePreview palette={look(selected).dark} mode="dark" title={selected.name} label={tr("Dark")} base={baseColours} {shades} />
    </section>
  {:else if part === "colours"}
    {#each warnings as w}<p class="err" role="status">{w}</p>{/each}
    <section class="acp-box">
      <div class="acp-tokens acp-tokens-head">
        <span></span><strong>{tr("Light")}</strong><strong>{tr("Dark")}</strong>
      </div>
      {#each GROUPS as [group, rows]}
        <h2 class="acp-token-group">{group}</h2>
        {#each rows as [k, label]}
          <div class="acp-tokens">
            <span>{label}</span>
            {#each [selected.light, selected.dark] as map, i}
              <span class="acp-swatch">
                <input
                  type="color"
                  value={isHex(map[k]) ? map[k] : "#808080"}
                  class:is-unset={!map[k]}
                  oninput={(e) => (map[k] = e.currentTarget.value)}
                  aria-label={tr("{label}, {n}, picker", { label, n: i ? "dark" : "light" })}
                />
                <input bind:value={map[k]} placeholder={tr("Default")} aria-label="{label}, {i ? 'dark' : 'light'}" />
              </span>
            {/each}
          </div>
        {/each}
      {/each}
      <h2 class="acp-token-group">{tr("Shape")}</h2>
      <div class="acp-tokens">
        <span>{tr("Corner radius")}</span>
        <input bind:value={selected.light.radius} placeholder={tr("Default")} aria-label={tr("Corner radius, light")} />
        <input bind:value={selected.dark.radius} placeholder={tr("Default")} aria-label={tr("Corner radius, dark")} />
      </div>
    </section>
  {:else if part === "css"}
    <section class="acp-box">
      <div class="acp-form">
        <Field label={tr("Extra CSS")} bind:value={selected.css} rows="20" class="acp-code" wide />
      </div>
    </section>
  {:else if part === "slots"}
    <section class="acp-box">
      <p class="acp-empty">
        {tr("HTML templates placed at fixed points on every page. They are sanitized when saved.")}
      </p>
      <div class="acp-form">
        {#each slots as s}<Field label={s} bind:value={selected.slotText[s]} rows="2" wide />{/each}
      </div>
    </section>
  {:else}
    <section class="acp-box">
      <div class="acp-box-head">
        <small class="muted"
          >{previewing
            ? tr("Only you see this theme, across the whole site, until you stop.")
            : tr("See this theme on the site without changing it for anyone else.")}{dirty
            ? tr(" Save first to include your changes.")
            : ""}</small
        >
        {#if previewing}<button class="secondary" onclick={stopPreview}>{tr("Stop preview")}</button>{:else}<button
            onclick={startPreview}>{tr("Preview just for me")}</button
          >{/if}
      </div>
      <iframe class="acp-preview" bind:this={frame} src="/" title={tr("Preview of {name}", { name: selected.name })}
      ></iframe>
    </section>
  {/if}
{/if}
