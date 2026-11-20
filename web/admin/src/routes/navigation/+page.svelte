<script>
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import IconButton from "#lib/IconButton.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { iconHref } from "#lib/icons.js";
  import { get, put } from "#lib/api.js";

  // The site's own pages a header link can point at, with the icon each shows here.
  const BUILTIN = {
    forums: [tr("Forums"), "folder"],
    latest: [tr("Latest"), "history"],
    top: [tr("Top"), "trophy"],
    search: [tr("Search"), "search"],
    members: [tr("Members"), "users"],
    groups: [tr("Groups"), "shield"],
    clubs: [tr("Clubs"), "tent"],
    live: [tr("Live Q&A"), "mic"],
  };
  // Module links show only while their module is on (Settings → Community features).
  const MODULES = {
    clubs: "clubs_enabled",
    live: "live_topics_enabled",
  };
  const DEFAULTS = ["forums", "latest", "top", "search"].map((key) => ({ key, groups: [] }));

  let items = $state([]);
  let saved = $state("[]"); // what the server has, to count changes against
  let groups = $state([]);
  let enabled = $state({});
  let editing = $state(null); // { index, draft }: index -1 adds a link
  let busy = $state(false);
  const status = new Status();

  const off = (item) => item.key in MODULES && !enabled[MODULES[item.key]];
  const name = (item) => (item.key ? (BUILTIN[item.key]?.[0] ?? item.key) : item.label);
  const audience = (item) =>
    item.groups?.length
      ? groups
          .filter((g) => item.groups.includes(g.id))
          .map((g) => g.name)
          .join(", ")
      : tr("Everyone");
  const groupOptions = $derived(groups.map((g) => ({ value: String(g.id), label: g.name })));
  const unused = $derived(Object.keys(BUILTIN).filter((k) => !items.some((i) => i.key === k)));
  const changes = $derived(JSON.stringify(items) === saved ? 0 : 1);

  async function load() {
    try {
      const [settings, g] = await Promise.all([get("/settings"), get("/groups")]);
      groups = g;
      enabled = Object.fromEntries(settings.filter((s) => typeof s.value === "boolean").map((s) => [s.key, s.value]));
      const raw = settings.find((s) => s.key === "nav_links")?.value;
      // Links saved with one group become a list.
      items = (raw ? JSON.parse(raw) : DEFAULTS).map(({ group, ...i }) => ({
        ...i,
        groups: i.groups ?? (group ? [group] : []),
      }));
      saved = JSON.stringify(items);
    } catch (e) {
      status.fail(e);
    }
  }
  load();

  function move(i, by) {
    const next = [...items];
    const [item] = next.splice(i, 1);
    next.splice(i + by, 0, item);
    items = next;
  }

  // The dialog edits a copy; groups are strings there, for the multi-select.
  function open(index) {
    status.clear();
    const item = index >= 0 ? items[index] : { key: unused[0] ?? "", label: "", url: "", groups: [] };
    editing = {
      index,
      kind: item.key ? "page" : "custom",
      draft: { ...item, groups: (item.groups ?? []).map(String) },
    };
  }
  function apply(e) {
    e.preventDefault();
    const { index, kind, draft } = editing;
    const g = draft.groups.map(Number);
    const item =
      kind === "page" ? { key: draft.key, groups: g } : { label: draft.label.trim(), url: draft.url.trim(), groups: g };
    if (kind === "custom" && !/^(\/|https:\/\/)/.test(item.url)) {
      status.error = tr("A link must start with / or https://.");
      return;
    }
    items = index >= 0 ? items.map((it, n) => (n === index ? item : it)) : [...items, item];
    editing = null;
  }
  const remove = (i) => (items = items.filter((_, n) => n !== i));

  async function save() {
    status.clear();
    busy = true;
    try {
      await put("/settings/nav_links", { value: JSON.stringify(items) });
      saved = JSON.stringify(items);
      status.msg = tr("Saved. Guests may see the old menu for a few seconds.");
    } catch (err) {
      status.fail(err);
    }
    busy = false;
  }
  const discard = () => (items = JSON.parse(saved));
</script>

<PageHead title={tr("Navigation")} status={editing ? null : status}>
  {#snippet intro()}{tr("Links in the site header, in order. Custom links must start with")} <code>/</code>
    {tr("or")} <code>https://</code>{tr("; others are not shown.")}{/snippet}
  <button onclick={() => open(-1)}>{tr("Add link")}</button>
</PageHead>

<Box title={tr("Preview")}>
  <nav class="acp-nav-preview" aria-label={tr("Preview")}>
    {#each items as item}
      <span class:is-off={off(item)} data-tip={off(item) ? tr("hidden while the module is off") : undefined}
        >{name(item) || "…"}</span
      >
    {:else}
      <span class="muted">{tr("No links: the header shows only the site name.")}</span>
    {/each}
  </nav>
</Box>

<Box title={tr("Header links")}>
  <ul class="acp-rows">
    {#each items as item, i (i)}
      <li class="acp-rowitem acp-nav-row" class:is-off={off(item)}>
        <span class="acp-field-move">
          <IconButton
            icon="arrow-up"
            label={tr("Move {n} up", { n: name(item) })}
            disabled={i === 0}
            onclick={() => move(i, -1)}
          />
          <IconButton
            icon="arrow-up"
            flip
            label={tr("Move {n} down", { n: name(item) })}
            disabled={i === items.length - 1}
            onclick={() => move(i, 1)}
          />
        </span>
        <span class="acp-field-icon"
          ><svg class="bf-icon" aria-hidden="true"
            ><use href={iconHref(item.key ? (BUILTIN[item.key]?.[1] ?? "link") : "link")}></use></svg
          ></span
        >
        <div class="acp-grow">
          <strong>{name(item)}</strong>
          <small class="muted acp-field-about">{item.key ? tr("Built-in page") : item.url} · {audience(item)}</small>
        </div>
        {#if off(item)}<span
            class="acp-pill is-warn"
            data-tip={tr("Switch the module on under Settings to show this link")}>{tr("Module off")}</span
          >{/if}
        {#if item.groups?.length}<span class="acp-pill is-muted">{tr("Some groups")}</span>{/if}
        <IconButton icon="pencil" label={tr("Edit {n}", { n: name(item) })} onclick={() => open(i)} />
        <IconButton icon="trash-2" label={tr("Remove {n}", { n: name(item) })} onclick={() => remove(i)} />
      </li>
    {:else}
      <li class="acp-empty-state">
        <strong>{tr("No header links")}</strong><span class="muted"
          >{tr("Use Add link to put pages in the header.")}</span
        >
      </li>
    {/each}
  </ul>
</Box>

{#if changes}
  <div class="acp-savebar" role="region" aria-label={tr("Unsaved changes")}>
    <span>{tn("admin.count.unsaved", changes)}</span>
    <button type="button" class="secondary" disabled={busy} onclick={discard}>{tr("Discard")}</button>
    <button type="button" disabled={busy} onclick={save}>{busy ? tr("Saving…") : tr("Save changes")}</button>
  </div>
{/if}

{#if editing}
  <EditDialog
    title={editing.index >= 0 ? tr("Edit link") : tr("Add link")}
    onsubmit={apply}
    onclose={() => (editing = null)}
    {status}
    save={editing.index >= 0 ? tr("Done") : tr("Add link")}
  >
    {#if editing.index < 0}
      <Section legend={tr("Link to")}>
        <div class="acp-kinds">
          <label class="acp-kind" class:is-on={editing.kind === "page"}>
            <input type="radio" name="kind" value="page" bind:group={editing.kind} disabled={!unused.length} />
            <svg class="bf-icon" aria-hidden="true"><use href={iconHref("folder")}></use></svg>
            <span
              ><strong>{tr("A page of this site")}</strong><small
                >{unused.length ? tr("Forums, members, clubs…") : tr("All of them are in the menu already.")}</small
              ></span
            >
          </label>
          <label class="acp-kind" class:is-on={editing.kind === "custom"}>
            <input type="radio" name="kind" value="custom" bind:group={editing.kind} />
            <svg class="bf-icon" aria-hidden="true"><use href={iconHref("link")}></use></svg>
            <span
              ><strong>{tr("Your own link")}</strong><small>{tr("Any address, with a label you choose.")}</small></span
            >
          </label>
        </div>
      </Section>
    {/if}
    <Section legend={tr("Link")} grid>
      {#if editing.kind === "page"}
        {#if editing.index < 0}
          <Field
            label={tr("Page")}
            bind:value={editing.draft.key}
            options={unused.map((k) => [
              k,
              BUILTIN[k][0] + (k in MODULES && !enabled[MODULES[k]] ? ` (${tr("module off")})` : ""),
            ])}
            required
            wide
          />
        {:else}
          <Field label={tr("Page")} value={BUILTIN[editing.draft.key]?.[0] ?? editing.draft.key} disabled wide />
        {/if}
      {:else}
        <Field label={tr("Label")} bind:value={editing.draft.label} maxlength="60" required />
        <Field label={tr("Address: / or https://…")} bind:value={editing.draft.url} required spellcheck="false" />
      {/if}
    </Section>
    <Section legend={tr("Who sees it")} grid>
      <div class="acp-wide">
        <MultiSelect
          label={tr("Visible to")}
          placeholder={tr("Everyone")}
          options={groupOptions}
          bind:value={editing.draft.groups}
        />
      </div>
    </Section>
  </EditDialog>
{/if}
