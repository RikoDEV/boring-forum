<script>
  // The one search of the admin panel: pages of the sidebar and every setting, found by what it
  // is called, what it does and where it lives. A setting opens the settings page on its group,
  // scrolled to the setting.
  import { goto } from "$app/navigation";
  import { resolve } from "$app/paths";
  import { get } from "#lib/api.js";
  import { tr } from "#lib/i18n.js";
  import sprite from "../../../../themes/default/icons.svg?url";

  /** @type {{ pages: [string, [string, string, string][]][] }} */
  let { pages } = $props();

  let q = $state("");
  let open = $state(false);
  let at = $state(0);
  /** @type {any[] | null} */
  let settings = $state(null);
  /** @type {HTMLInputElement | undefined} */
  let input = $state();
  const listId = "acp-find-list";
  // The labels are translated when the module runs, so it is loaded once the language is known,
  // not with the panel's shell.
  /** @type {typeof import("#lib/settings-meta.js") | undefined} */
  let meta = $state();
  /** @type {[string, string, string][]} */
  let tabs = $state([]);

  async function load() {
    if (settings) return;
    settings = [];
    try {
      const [m, s, t] = await Promise.all([
        import("#lib/settings-meta.js"),
        get("/settings"),
        import("#lib/tabs-meta.js"),
      ]);
      meta = m;
      settings = s;
      tabs = t.TABS;
    } catch {
      settings = null;
    }
  }

  const words = $derived(q.trim().toLowerCase().split(/\s+/).filter(Boolean));
  const matches = (hay) => words.every((w) => hay.includes(w));

  const results = $derived.by(() => {
    if (!words.length) return /** @type {any[]} */ ([]);
    /** @type {any[]} */
    const out = [];
    for (const [section, items] of pages)
      for (const [path, label, icon] of items)
        if (matches(`${label} ${section}`.toLowerCase()))
          out.push({ kind: "page", id: `p${path}`, label, hint: section, icon, path });
    // A tab of a page: found by its name and the page's, opening the page on that tab.
    for (const [path, key, label] of tabs) {
      const page = pages.flatMap(([, items]) => items).find(([p]) => p === path);
      if (!page) continue;
      if (matches(`${label} ${page[1]}`.toLowerCase()))
        out.push({
          kind: "tab",
          id: `t${path}${key}`,
          label,
          hint: page[1],
          icon: page[2],
          path: `${path}#tab=${key}`,
        });
    }
    /** @type {any[]} */
    const found = [];
    if (!meta) return out;
    const { placeOf, groupOf, groupTitle, humanize } = meta;
    for (const s of settings ?? []) {
      const place = placeOf(s.key).split("#")[0];
      const where = place
        ? (pages.flatMap(([, items]) => items).find(([p]) => p === place)?.[1] ?? "")
        : groupTitle(groupOf(s.key));
      const label = humanize(s.key);
      if (matches(`${label} ${s.key} ${s.description} ${where}`.toLowerCase()))
        found.push({
          kind: "setting",
          id: `s${s.key}`,
          label,
          hint: s.description,
          where,
          // A name that starts with the words ranks above one that only mentions them.
          rank: label.toLowerCase().startsWith(words[0]) ? 0 : label.toLowerCase().includes(words[0]) ? 1 : 2,
          key: s.key,
        });
    }
    found.sort((a, b) => a.rank - b.rank);
    return [...out, ...found.slice(0, 12)];
  });

  function pick(r) {
    q = "";
    open = false;
    input?.blur();
    if (r.kind === "tab") goto(`${resolve(r.path.split("#")[0])}#${r.path.split("#")[1]}`);
    else if (r.kind === "page") goto(resolve(r.path === "" ? "/" : r.path));
    else {
      const [path, hash] = (meta?.placeOf(r.key) || `/settings#setting=${encodeURIComponent(r.key)}`).split("#");
      goto(`${resolve(path)}${hash ? `#${hash}` : ""}`);
    }
  }

  function onkey(e) {
    if (e.key === "Escape") {
      if (q) q = "";
      else input?.blur();
      open = false;
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      if (!results.length) return;
      e.preventDefault();
      open = true;
      at = (at + (e.key === "ArrowDown" ? 1 : results.length - 1)) % results.length;
    } else if (e.key === "Enter" && open && results[at]) {
      e.preventDefault();
      pick(results[at]);
    }
  }

  $effect(() => {
    words;
    at = 0;
  });
  $effect(() => {
    const el = document.getElementById(`acp-find-${at}`);
    el?.scrollIntoView({ block: "nearest" });
  });
</script>

<!-- Takes the place of the site's search in the header, so it wears the same classes. "/" reaches
     it through the site's own shortcut. -->
<div class="header-search acp-find" class:is-open={open}>
  <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#search"></use></svg>
  <input
    name="q"
    bind:this={input}
    type="search"
    role="combobox"
    aria-expanded={open && words.length > 0}
    aria-controls={listId}
    aria-autocomplete="list"
    aria-activedescendant={open && results[at] ? `acp-find-${at}` : undefined}
    autocomplete="off"
    spellcheck="false"
    placeholder={tr("Search pages and settings")}
    aria-label={tr("Search pages and settings")}
    bind:value={q}
    onfocus={() => {
      open = true;
      load();
    }}
    onblur={() => setTimeout(() => (open = false), 120)}
    onkeydown={onkey}
  />
  <kbd aria-hidden="true">/</kbd>
  {#if open && words.length}
    <ul class="acp-find-list" id={listId} role="listbox" aria-label={tr("Search results")}>
      {#each results as r, i (r.id)}
        <li
          id="acp-find-{i}"
          role="option"
          aria-selected={i === at}
          class:is-active={i === at}
          onmousedown={(e) => {
            e.preventDefault();
            pick(r);
          }}
          onmousemove={() => (at = i)}
        >
          {#if r.kind === "page" || r.kind === "tab"}
            <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{r.icon}"></use></svg>
          {:else}
            <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#settings"></use></svg>
          {/if}
          <span class="acp-find-text">
            <span class="acp-find-label">{r.label}</span>
            <span class="acp-find-hint">
              {r.kind === "page"
                ? tr("Page · {section}", { section: r.hint })
                : r.kind === "tab"
                  ? tr("Tab · {page}", { page: r.hint })
                  : r.hint}
            </span>
          </span>
          {#if r.kind === "setting" && r.where}<span class="acp-pill">{r.where}</span>{/if}
        </li>
      {:else}
        <li class="acp-find-none" role="presentation">
          {settings === null ? tr("Settings could not be loaded.") : tr("Nothing matches “{q}”.", { q })}
        </li>
      {/each}
    </ul>
  {/if}
</div>
