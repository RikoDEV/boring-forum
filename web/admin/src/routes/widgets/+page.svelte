<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, patch, post, put } from "#lib/api.js";
  import sprite from "../../../../../themes/default/icons.svg?url";
  import Field from "#lib/Field.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import SchemaForm from "#lib/SchemaForm.svelte";
  import { localized, withDefaults } from "#lib/schema.js";

  // The pages with a layout come from the server (`PAGES` in admin/themes.rs): every one has a
  // top, side and bottom area, except the home page with its main column.
  const areasOf = (p) =>
    p === "home"
      ? [
          ["home_top", tr("Top of the page"), "top"],
          ["home_main", tr("Main column"), "main"],
          ["home_bottom", tr("Sidebar"), "side"],
        ]
      : [
          [`${p}_top`, tr("Top of the page"), "top"],
          [`${p}_side`, tr("Sidebar"), "side"],
          [`${p}_bottom`, tr("Bottom of the page"), "bottom"],
        ];
  const KINDS = {
    forum_list: [tr("Forum list"), tr("All forums and categories"), "folder"],
    home_latest: [tr("Latest topics feed"), tr("The main list of new topics"), "message-square"],
    home_stats: [tr("Forum statistics"), tr("Totals for the whole forum"), "table"],
    online_users: [tr("Who's online"), tr("Members active right now"), "users"],
    latest_topics: [tr("Latest topics"), tr("A short list of recent topics"), "history"],
    latest_statuses: [tr("Status updates"), tr("What members posted lately"), "quote"],
    top_posters: [tr("Top posters"), tr("The most active members"), "star"],
    stats: [tr("Small statistics"), tr("A compact count of members and posts"), "gauge"],
    custom: [tr("Custom text"), tr("Your own Markdown"), "pencil"],
    tag_cloud: [tr("Tag cloud"), tr("Popular tags"), "flag"],
    upcoming_events: [tr("Upcoming events"), tr("The calendar's next events, or dates you list by hand"), "clock"],
    calendar: [tr("Calendar"), tr("A month grid with the forum's events"), "calendar"],
    plugin: [tr("Plugin block"), tr("Content from an installed plugin"), "wrench"],
    html: [tr("HTML"), tr("Your own markup, cleaned like a post"), "code"],
    browsing: [tr("Recently browsing"), tr("Members and visitors on this page now"), "eye"],
    suggested_topics: [tr("Suggested topics"), tr("Topics to read next, under a topic"), "history"],
  };
  // Drawn for each viewer on the home page: home only, one of each.
  const HOME_ONLY = ["forum_list", "home_latest", "home_stats"];
  const CACHE = [
    [0, tr("Not cached")],
    [60, "1 minute"],
    [300, "5 minutes"],
  ];

  // Blocks that enabled plugins declare in their manifests (`[[widgets]]`), with their settings forms.
  let plugins = $state([]);
  get("/plugins").then(
    (r) => (plugins = r),
    () => {},
  );
  const declared = $derived(
    plugins
      .filter((p) => p.enabled)
      .flatMap((p) =>
        (p.schema?.["x-widgets"] ?? []).map((w) => ({
          ...w,
          plugin: p.id,
          locales: p.schema?.["x-locales"] ?? {},
          label: localized(p.schema?.["x-locales"], w.label_key, w.label),
          description: localized(p.schema?.["x-locales"], w.description_key, w.description),
        })),
      ),
  );
  const declaredFor = (w) =>
    w.kind === "plugin" ? declared.find((d) => d.plugin === w.config?.plugin && d.key === w.config?.key) : null;
  const wname = (w) => declaredFor(w)?.label ?? name(w.kind);
  function addDeclared(d) {
    status.clear();
    const area = adding;
    post("/widgets", {
      area,
      kind: "plugin",
      title: "",
      cache_seconds: 60,
      config: { plugin: d.plugin, key: d.key, settings: withDefaults(d.fields, {}) },
    }).then(async ({ id }) => {
      adding = null;
      await load();
      const made = widgets.find((w) => w.id === id);
      if (made && d.fields.length) edit(made);
    }, status.fail);
  }

  let widgets = $state([]);
  let kinds = $state([]);
  let pages = $state([]);
  const status = new Status();
  let tab = $state("home");
  let adding = $state(null); // the area a widget is being added to
  let editing = $state(null); // a copy of the widget being changed
  let dragged = $state(null);
  let over = $state(null);

  const load = () =>
    get("/widgets").then((r) => {
      widgets = r.widgets;
      kinds = r.kinds;
      pages = r.pages ?? [];
    }, status.fail);
  const current = $derived(pages.find((p) => p.name === tab));
  const pageGroups = $derived([...new Set(pages.map((p) => p.group))]);
  const inArea = (a) => widgets.filter((w) => w.area === a).sort((x, y) => x.position - y.position);
  const countOn = (page) => widgets.filter((w) => w.area.startsWith(`${page}_`)).length;
  const name = (k) => KINDS[k]?.[0] ?? k;
  const icon = (k) => KINDS[k]?.[2] ?? "table";
  const cacheLabel = (s) => (s <= 0 ? tr("Not cached") : s < 300 ? tr("Cached {s}s", { s }) : tr("Cached 5 min"));
  // The page draws these itself: recently browsing on the home, forum and topic pages (one each),
  // suggested topics on the topic page.
  const PAGE_DRAWN = ["browsing", "suggested_topics"];
  const pageOf = (area) => area.split("_")[0];
  const canAdd = (k, area) => {
    if (PAGE_DRAWN.includes(k)) {
      const page = pageOf(area);
      const pages = k === "browsing" ? ["home", "forum", "topic"] : ["topic"];
      return pages.includes(page) && !widgets.some((w) => w.kind === k && pageOf(w.area) === page);
    }
    return !HOME_ONLY.includes(k) || (area.startsWith("home_") && !widgets.some((w) => w.kind === k));
  };
  // Only the fields these kinds use; everything else in config is kept as it is.
  const usesLimit = (k) =>
    ["latest_topics", "latest_statuses", "top_posters", "tag_cloud", "upcoming_events", "calendar"].includes(k);
  const usesJson = (k) => ["plugin", "upcoming_events"].includes(k);

  function add(kind) {
    status.clear();
    const area = adding;
    post("/widgets", { area, kind, title: "", cache_seconds: 60, config: {} }).then(async ({ id }) => {
      adding = null;
      await load();
      const made = widgets.find((w) => w.id === id);
      if (made && !HOME_ONLY.includes(kind) && !PAGE_DRAWN.includes(kind)) edit(made);
    }, status.fail);
  }
  function edit(w) {
    editing = {
      ...w,
      markdown: w.config?.markdown ?? "",
      limit: w.config?.limit ?? "",
      json: JSON.stringify(w.config ?? {}, null, 2),
      settings: declaredFor(w) ? withDefaults(declaredFor(w).fields, w.config?.settings) : {},
    };
  }
  function save(e) {
    e.preventDefault();
    status.clear();
    let config = { ...(editing.config ?? {}) };
    if (declaredFor(editing)) {
      config.settings = editing.settings;
    } else if (usesJson(editing.kind)) {
      try {
        config = JSON.parse(editing.json || "{}");
      } catch {
        status.error = tr("The settings are not valid JSON.");
        return;
      }
    }
    if (editing.kind === "custom") config.markdown = editing.markdown;
    if (usesLimit(editing.kind)) {
      if (editing.limit === "" || editing.limit === null) delete config.limit;
      else config.limit = Number(editing.limit);
    }
    patch(`/widgets/${editing.id}`, {
      title: editing.title,
      config,
      cache_seconds: Number(editing.cache_seconds),
    }).then(() => {
      editing = null;
      load();
    }, status.fail);
  }
  const remove = (w) =>
    confirm(tr('Remove "{n}"?', { n: w.title || wname(w) })) &&
    del(`/widgets/${w.id}`).then(() => {
      if (editing?.id === w.id) editing = null;
      load();
    }, status.fail);

  function place(w, area, index) {
    const ids = inArea(area)
      .map((x) => x.id)
      .filter((id) => id !== w.id);
    ids.splice(index ?? ids.length, 0, w.id);
    put("/widgets/arrange", { area, ids }).then(load, status.fail);
  }
  function drop(area, before) {
    over = null;
    if (!dragged) return;
    const w = dragged;
    dragged = null;
    const ids = inArea(area)
      .map((x) => x.id)
      .filter((id) => id !== w.id);
    place(w, area, before ? ids.indexOf(before.id) : ids.length);
  }
  function nudge(w, by) {
    const ids = inArea(w.area).map((x) => x.id);
    const i = ids.indexOf(w.id) + by;
    if (i >= 0 && i < ids.length) place(w, w.area, i);
  }
  load();
</script>

<PageHead
  title={tr("Widgets")}
  {status}
  intro={tr("Blocks shown around your pages. Drag them between areas, or use the arrows.")}
>
  {#if current?.url}<a class="acp-btnlink" href="{current.url}?edit_layout=1">{tr("Edit on the page")}</a>{/if}
</PageHead>

<div class="acp-form acp-page-pick">
  <Field
    label={tr("Page")}
    bind:value={tab}
    onchange={() => {
      adding = null;
      editing = null;
    }}
  >
    {#each pageGroups as g (g)}
      <optgroup label={g}>
        {#each pages.filter((p) => p.group === g) as p (p.name)}<option value={p.name}
            >{p.label}{countOn(p.name) ? ` (${countOn(p.name)})` : ""}</option
          >{/each}
      </optgroup>
    {/each}
  </Field>
</div>

<div class="acp-board">
  {#each areasOf(tab) as [area, label, shape] (area)}
    <section
      class="acp-area is-{shape}"
      class:is-over={over === area}
      aria-label={label}
      ondragover={(e) => {
        e.preventDefault();
        over = area;
      }}
      ondragleave={(e) => {
        if (!e.currentTarget.contains(/** @type {Node} */ (e.relatedTarget))) over = null;
      }}
      ondrop={() => drop(area)}
    >
      <header>
        <h2>{label}</h2>
        <button
          class="secondary"
          onclick={() => {
            adding = adding === area ? null : area;
            editing = null;
          }}
          aria-expanded={adding === area}>{tr("Add")}</button
        >
      </header>
      {#if adding === area}
        <ul class="acp-kindlist" aria-label={tr("Choose a widget")}>
          {#each kinds.filter((k) => canAdd(k, area)) as k}
            <li>
              <button class="acp-kind" onclick={() => add(k)}
                ><svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{icon(k)}"></use></svg><span
                  ><strong>{name(k)}</strong><small>{KINDS[k]?.[1] ?? ""}</small></span
                ></button
              >
            </li>
          {/each}
          {#each declared as d (d.plugin + d.key)}
            <li>
              <button class="acp-kind" onclick={() => addDeclared(d)}
                ><svg class="bf-icon" aria-hidden="true"><use href="{sprite}#wrench"></use></svg><span
                  ><strong>{d.label}</strong><small>{d.description}</small></span
                ></button
              >
            </li>
          {/each}
        </ul>
      {/if}
      <ul class="acp-widgets">
        {#each inArea(area) as w, i (w.id)}
          <li
            class="acp-widget"
            class:is-dragging={dragged?.id === w.id}
            draggable={editing?.id !== w.id}
            ondragstart={() => (dragged = w)}
            ondragend={() => {
              dragged = null;
              over = null;
            }}
            ondragover={(e) => e.preventDefault()}
            ondrop={(e) => {
              e.stopPropagation();
              drop(area, w);
            }}
          >
            <div class="acp-widget-row">
              <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{icon(w.kind)}"></use></svg>
              <span class="acp-grow"
                ><strong>{w.title || wname(w)}</strong><small class="muted"
                  >{w.title ? `${wname(w)} · ` : ""}{cacheLabel(w.cache_seconds)}</small
                ></span
              >
              <span class="acp-widget-tools">
                <button
                  class="acp-ghost acp-icon-btn"
                  disabled={i === 0}
                  onclick={() => nudge(w, -1)}
                  aria-label={tr("Move {n} up", { n: w.title || wname(w) })}>↑</button
                >
                <button
                  class="acp-ghost acp-icon-btn"
                  disabled={i === inArea(area).length - 1}
                  onclick={() => nudge(w, 1)}
                  aria-label={tr("Move {n} down", { n: w.title || wname(w) })}>↓</button
                >
                <button
                  class="acp-ghost"
                  onclick={() => {
                    editing?.id === w.id ? (editing = null) : edit(w);
                    adding = null;
                  }}
                  aria-expanded={editing?.id === w.id}>{tr("Edit")}</button
                >
              </span>
            </div>
          </li>
        {:else}
          <li class="acp-area-empty">{tr("Nothing here yet")}</li>
        {/each}
      </ul>
    </section>
  {/each}
</div>

{#if editing}
  <EditDialog
    title={tr("Edit {name}", { name: editing.title || wname(editing) })}
    onclose={() => (editing = null)}
    onsubmit={save}
    {status}
    ondelete={() => remove(editing)}
    deleteLabel={tr("Remove")}
  >
    <div class="acp-form">
      <Field label={tr("Title")} bind:value={editing.title} />
      {#if !HOME_ONLY.includes(editing.kind)}
        <Field label={tr("Refresh")} bind:value={editing.cache_seconds}>
          {#each CACHE as [s, text]}<option value={s}>{text}</option>{/each}
          {#if !CACHE.some(([s]) => s === editing.cache_seconds)}<option value={editing.cache_seconds}
              >{tr("5 minutes")}</option
            >{/if}
        </Field>
      {/if}
      {#if usesLimit(editing.kind)}<Field
          label={tr("How many to show")}
          type="number"
          min="1"
          max="50"
          bind:value={editing.limit}
        />{/if}
      {#if editing.kind === "custom"}<Field label={tr("Markdown")} rows="5" wide bind:value={editing.markdown} />{/if}
      {#if usesJson(editing.kind) && !declaredFor(editing)}<Field
          class="acp-code"
          wide
          label={editing.kind === "plugin"
            ? tr('Settings (JSON), for example {"plugin": "id", "key": "block"}')
            : tr('Settings (JSON), for example {"events": [{"date": "2027-01-31", "title": "Meetup", "url": "/t/1"}]}')}
          rows="5"
          bind:value={editing.json}
        />{/if}
    </div>
    {#if declaredFor(editing)?.fields.length}<SchemaForm
        fields={declaredFor(editing).fields}
        bind:values={editing.settings}
        locales={declaredFor(editing).locales}
      />{/if}
  </EditDialog>
{/if}
