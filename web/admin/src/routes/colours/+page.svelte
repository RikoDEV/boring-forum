<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import { contrast, isHex } from "#lib/contrast.js";
  import SchemePreview from "#lib/SchemePreview.svelte";
  import sprite from "../../../../../themes/default/icons.svg?url";

  let schemes = $state([]);
  let base = $state([]);
  let shades = $state([]);
  let defaultLight = $state("");
  let defaultDark = $state("");
  let selected = $state(null);
  let savedCopy = $state("");
  let adding = $state(false);
  const fresh = $state({ name: "", copy_from: 0 });
  const status = new Status();

  // The labels are written here (not taken from the server) so they are translated.
  const LABEL = {
    background: tr("Page background"),
    surface: tr("Cards and panels"),
    text: tr("Text"),
    muted: tr("Secondary text"),
    border: tr("Borders"),
    accent: tr("Links and buttons"),
    header_background: tr("Header background"),
    header_text: tr("Header text"),
    success: tr("Success"),
    warning: tr("Warning"),
    danger: tr("Danger"),
  };
  const GROUPS = [
    [tr("Page"), ["background", "surface", "border"]],
    [tr("Text"), ["text", "muted"]],
    [tr("Accent"), ["accent"]],
    [tr("Header"), ["header_background", "header_text"]],
    [tr("Status"), ["success", "warning", "danger"]],
  ];
  // Pairs that have to be readable together, as [foreground, background].
  const PAIRS = [
    ["text", "background"],
    ["text", "surface"],
    ["muted", "surface"],
    ["accent", "surface"],
    ["header_text", "header_background"],
    ["success", "surface"],
    ["warning", "surface"],
    ["danger", "surface"],
  ];

  async function load() {
    const r = await get("/color-schemes").catch(status.fail);
    if (!r) return;
    schemes = r.schemes;
    base = r.base;
    shades = r.derived;
    defaultLight = r.default_light;
    defaultDark = r.default_dark;
    fresh.copy_from ||= schemes[0]?.id ?? 0;
    if (selected) {
      const again = schemes.find((s) => s.id === selected.id);
      if (!again) selected = null;
    }
  }
  load();

  const edit = (s) => ({
    id: s.id,
    slug: s.slug,
    name: s.name,
    selectable: s.selectable,
    light: { ...s.light },
    dark: { ...s.dark },
  });
  const snapshot = (s) => JSON.stringify([s.name, s.selectable, s.light, s.dark]);
  const dirty = $derived(!!selected && snapshot(selected) !== savedCopy);
  function open(s) {
    status.clear();
    selected = edit(s);
    savedCopy = snapshot(selected);
  }
  const close = () => {
    selected = null;
    status.clear();
  };

  async function add(e) {
    e.preventDefault();
    status.clear();
    try {
      const made = await post("/color-schemes", { name: fresh.name, copy_from: fresh.copy_from });
      adding = false;
      fresh.name = "";
      await load();
      const s = schemes.find((x) => x.id === made.id);
      if (s) open(s);
    } catch (err) {
      status.fail(err);
    }
  }
  async function save() {
    status.clear();
    try {
      await put(`/color-schemes/${selected.id}`, {
        name: selected.name,
        light: selected.light,
        dark: selected.dark,
        selectable: selected.selectable,
      });
      savedCopy = snapshot(selected);
      status.msg = tr("Saved.");
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
  // The site's default for light mode and for dark mode can be different schemes.
  const isDefault = (s) => s.slug === defaultLight || s.slug === defaultDark;
  const setDefault = (s, light, dark) =>
    put("/color-schemes/default", { id: s.id, light, dark }).then(load, status.fail);
  async function remove(s) {
    if (!confirm(tr("Delete the colour scheme {name}? Members who chose it get the default.", { name: s.name })))
      return;
    try {
      await del(`/color-schemes/${s.id}`);
      if (selected?.id === s.id) selected = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  const strip = (m) =>
    ["header_background", "background", "surface", "text", "accent", "danger"].map((k) => m?.[k]).filter(Boolean);

  // Text on a colour: the mode's palette gives the pairs that fall short of WCAG AA (4.5:1).
  const low = (map, mode) =>
    PAIRS.flatMap(([fg, bg]) => {
      const r = contrast(map?.[fg], map?.[bg]);
      return r !== null && r < 4.5
        ? [
            tr("{mode}: {a} on {b} is {n}:1; aim for at least 4.5:1.", {
              mode,
              a: LABEL[fg],
              b: LABEL[bg],
              n: r.toFixed(1),
            }),
          ]
        : [];
    });
  const warnings = $derived(selected ? [...low(selected.light, tr("Light")), ...low(selected.dark, tr("Dark"))] : []);
</script>

{#if !selected}
  <PageHead
    title={tr("Colour schemes")}
    intro={tr(
      "Palettes of base colours, one for light mode and one for dark. Hover shades, soft backgrounds and borders are worked out from them. Members choose among the schemes you allow; the default is for everyone else.",
    )}
  >
    <button onclick={() => (adding = true)}>{tr("New colour scheme")}</button>
  </PageHead>
  {#if status.error && !adding}<p class="err" role="alert">{status.error}</p>{/if}
  {#if adding}
    <EditDialog
      title={tr("New colour scheme")}
      onclose={() => (adding = false)}
      onsubmit={add}
      {status}
      save={tr("Create")}
    >
      <div class="acp-form">
        <Field label={tr("Name")} bind:value={fresh.name} maxlength="40" required wide />
        <Field
          label={tr("Start from")}
          bind:value={fresh.copy_from}
          options={schemes.map((s) => [s.id, s.name])}
          wide
        />
      </div>
    </EditDialog>
  {/if}
  <ul class="acp-themes" aria-label={tr("Colour schemes")}>
    {#each schemes as s (s.id)}
      <li class="acp-theme">
        <button
          class="acp-theme-look acp-scheme-look"
          onclick={() => open(s)}
          aria-label={tr("Edit {name}", { name: s.name })}
        >
          <span>
            {#each strip(s.light) as c}<i style:background={c}></i>{/each}
          </span>
          <span>
            {#each strip(s.dark) as c}<i style:background={c}></i>{/each}
          </span>
        </button>
        <div class="acp-theme-body">
          <div class="acp-grow"><strong>{s.name}</strong></div>
          {#if s.slug === defaultLight && s.slug === defaultDark}<span class="acp-pill is-ok">
              {tr("Default")}
            </span>{:else}
            {#if s.slug === defaultLight}<span class="acp-pill is-ok">{tr("Default, light")}</span>{/if}
            {#if s.slug === defaultDark}<span class="acp-pill is-ok">{tr("Default, dark")}</span>{/if}
          {/if}
          {#if !s.selectable && !isDefault(s)}<span class="acp-pill is-muted">{tr("Hidden")}</span>{/if}
        </div>
        <div class="acp-actions">
          <button class="secondary" onclick={() => open(s)}>{tr("Edit")}</button>
          {#if s.slug !== defaultLight || s.slug !== defaultDark}<button
              class="secondary"
              onclick={() => setDefault(s, true, true)}
            >
              {tr("Make default")}
            </button>{/if}
          {#if !isDefault(s)}
            <details class="acp-more">
              <summary class="acp-icon-btn" aria-label={tr("More for {name}", { name: s.name })}>
                <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#chevron-down"></use></svg>
              </summary>
              <div><button class="is-danger" onclick={() => remove(s)}>{tr("Delete")}</button></div>
            </details>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
{:else}
  <div class="acp-head">
    <div>
      <button class="acp-link" onclick={close}>{tr("← All colour schemes")}</button>
      <h1>{selected.name}</h1>
    </div>
    <div class="acp-actions">
      {#if dirty}<span class="muted"><small>{tr("Unsaved changes")}</small></span>{:else if status.msg}<span
          class="muted"
          role="status"
        >
          <small>{status.msg}</small>
        </span>{/if}
      <button onclick={save} disabled={!dirty}>{tr("Save")}</button>
    </div>
  </div>
  {#if status.error}<p class="err" role="alert">{status.error}</p>{/if}
  {#each warnings as w}<p class="err" role="status">{w}</p>{/each}

  <section class="acp-box">
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={selected.name} maxlength="40" required />
      <Switch wide bind:checked={selected.selectable} disabled={isDefault(selected)}>
        {tr("Members can choose this scheme")}
      </Switch>
    </div>
  </section>

  <section class="acp-box">
    <h2 class="acp-token-group">{tr("Default")}</h2>
    <div class="acp-form">
      <Switch
        wide
        checked={selected.slug === defaultLight}
        disabled={selected.slug === defaultLight}
        onchange={() => setDefault(selected, true, false)}
      >
        {tr("Its light palette is the site's default in light mode")}
      </Switch>
      <Switch
        wide
        checked={selected.slug === defaultDark}
        disabled={selected.slug === defaultDark}
        onchange={() => setDefault(selected, false, true)}
      >
        {tr("Its dark palette is the site's default in dark mode")}
      </Switch>
    </div>
  </section>

  <div class="acp-scheme-edit">
    <section class="acp-box">
      <div class="acp-tokens acp-tokens-head">
        <span></span><strong>{tr("Light")}</strong><strong>{tr("Dark")}</strong>
      </div>
      {#each GROUPS as [group, keys]}
        <h2 class="acp-token-group">{group}</h2>
        {#each keys as k}
          <div class="acp-tokens">
            <span>{LABEL[k]}</span>
            {#each [selected.light, selected.dark] as map, i}
              <span class="acp-swatch">
                <small class="acp-swatch-mode">{i ? tr("Dark") : tr("Light")}</small>
                <input
                  type="color"
                  value={isHex(map[k]) ? map[k] : "#808080"}
                  oninput={(e) => (map[k] = e.currentTarget.value)}
                  aria-label={tr("{label}, {mode}, picker", { label: LABEL[k], mode: i ? tr("dark") : tr("light") })}
                />
                <input
                  bind:value={map[k]}
                  maxlength="7"
                  spellcheck="false"
                  class:is-bad={!isHex(map[k])}
                  aria-label="{LABEL[k]}, {i ? tr('dark') : tr('light')}"
                />
              </span>
            {/each}
          </div>
        {/each}
      {/each}
    </section>

    <section class="acp-box acp-scheme-previews" aria-label={tr("Preview")}>
      <SchemePreview palette={selected.light} mode="light" title={selected.name} label={tr("Light")} {base} {shades} />
      <SchemePreview palette={selected.dark} mode="dark" title={selected.name} label={tr("Dark")} {base} {shades} />
    </section>
  </div>
{/if}
