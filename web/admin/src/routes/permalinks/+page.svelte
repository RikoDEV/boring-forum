<script>
  import { tr } from "#lib/i18n.js";
  import { Crud } from "#lib/crud.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import { members, tags } from "#lib/suggest.js";
  import { get } from "#lib/api.js";

  let rows = $state([]);
  let total = $state(0);
  let nodes = $state([]);
  let search = $state("");
  // The member or tag a permalink points at, as the picker holds it.
  let pick = $state([]);
  const crud = new Crud({ path: "/permalinks", blank: () => blank(), load: () => load() });
  const status = crud.status;

  const KINDS = [
    ["forum", tr("Forum or category")],
    ["topic", tr("Topic")],
    ["post", tr("Post")],
    ["tag", tr("Tag")],
    ["member", tr("Member")],
    ["url", tr("Address")],
  ];
  const NAME = Object.fromEntries(KINDS);
  // What the second field asks for, by type.
  const ASKS = {
    topic: tr("Topic ID"),
    post: tr("Post ID"),
    url: tr("Send visitors to (an address on this site, or a web address)"),
  };
  const forums = $derived(nodes.map((n) => [n.id, `${n.kind === "category" ? "▾ " : "   "}${n.title}`]));

  async function load() {
    try {
      const r = await get(`/permalinks?q=${encodeURIComponent(search.trim())}`);
      rows = r.permalinks;
      total = r.total;
    } catch (e) {
      status.fail(e);
    }
  }
  get("/tree").then((t) => (nodes = t.nodes), () => {});
  load();

  let timer = 0;
  const typed = () => {
    clearTimeout(timer);
    timer = window.setTimeout(load, 250);
  };

  const blank = () => ({ id: 0, url: "", kind: "forum", target_id: nodes[0]?.id ?? "", target: "" });
  function edit(p) {
    crud.open(p && { id: p.id, url: p.url, kind: p.kind, target_id: p.target_id ?? "", target: p.target ?? "" });
    pick = p && ["member", "tag"].includes(p.kind) && p.target ? [p.target] : [];
  }
  function save(e) {
    e.preventDefault();
    status.clear();
    const d = crud.draft;
    crud.save({
      url: d.url,
      kind: d.kind,
      target_id: ["forum", "topic", "post"].includes(d.kind) ? Number(d.target_id) || null : null,
      target: ["forum", "topic", "post"].includes(d.kind)
        ? null
        : ["member", "tag"].includes(d.kind)
          ? (pick[0] ?? "")
          : d.target,
    });
  }
  const remove = () => confirm(tr("Delete the permalink {url}?", { url: crud.draft.url })) && crud.remove(crud.draft.id);
</script>

<PageHead
  title={tr("Permalinks")}
  intro={tr(
    "Redirects for addresses the forum does not know, such as the links of a forum you moved here. Visitors and search engines are sent on with a permanent redirect.",
  )}
  status={crud.draft ? null : status}
>
  <button onclick={() => edit(null)}>{tr("Add permalink")}</button>
</PageHead>

{#if crud.draft}
  <EditDialog
    title={crud.draft.id ? tr("Edit permalink") : tr("Add permalink")}
    onclose={crud.close}
    onsubmit={save}
    {status}
    save={crud.draft.id ? tr("Save") : tr("Add")}
    ondelete={crud.draft.id ? remove : null}
  >
    <div class="acp-form">
      <Field label={tr("Old address, like c/old-forum/12 or viewtopic.php?t=7")} bind:value={crud.draft.url} required wide />
      <Field label={tr("Points at")} bind:value={crud.draft.kind} options={KINDS} onchange={() => (pick = [])} />
      {#if crud.draft.kind === "forum"}
        <Field label={tr("Forum or category")} bind:value={crud.draft.target_id} options={forums} />
      {:else if crud.draft.kind === "topic" || crud.draft.kind === "post"}
        <Field label={ASKS[crud.draft.kind]} type="number" min="1" bind:value={crud.draft.target_id} required />
      {:else if crud.draft.kind === "member"}
        <MultiSelect label={tr("Member")} bind:value={pick} suggest={members} max={1} placeholder={tr("Type a name")} />
      {:else if crud.draft.kind === "tag"}
        <MultiSelect label={tr("Tag")} bind:value={pick} suggest={tags} max={1} placeholder={tr("Type a tag")} />
      {:else}
        <Field label={ASKS[crud.draft.kind]} bind:value={crud.draft.target} required wide />
      {/if}
    </div>
  </EditDialog>
{/if}

<div class="acp-filter">
  <Field label={tr("Search address or destination")} type="search" bind:value={search} oninput={typed} />
</div>

<table>
  <thead>
    <tr><th>{tr("Address")}</th><th>{tr("Destination")}</th><th class="acp-cell-end"></th></tr>
  </thead>
  <tbody>
    {#each rows as p (p.id)}
      <tr>
        <td class="acp-wrap"><code>{p.url}</code></td>
        <td class="acp-wrap">
          {#if p.missing}<span class="acp-pill is-danger">{tr("Missing")}</span>
            <span class="muted">{NAME[p.kind]} {p.target_id ?? p.target}</span>
          {:else}
            <span class="muted">{NAME[p.kind]}:</span> {p.label}
            {#if p.kind !== "url"}<small class="muted"><code>{p.destination}</code></small>{/if}
          {/if}
        </td>
        <td class="acp-cell-end"><button class="secondary" onclick={() => edit(p)}>{tr("Edit")}</button></td>
      </tr>
    {:else}
      <tr><td colspan="3" class="muted">{search.trim() ? tr("Nothing matches.") : tr("No permalinks yet.")}</td></tr>
    {/each}
  </tbody>
</table>
{#if total > rows.length && !search.trim()}<p class="muted"><small>{tr("The first {n} of {total} are shown: search for the rest.", { n: rows.length, total })}</small></p>{/if}
