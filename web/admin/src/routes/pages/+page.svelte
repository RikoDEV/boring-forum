<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put, when } from "#lib/api.js";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Section from "#lib/Section.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  let pages = $state([]);
  let draft = $state(null);
  const status = new Status();
  const blank = () => ({
    id: 0,
    title: "",
    slug: "",
    template: "single",
    visibility: "public",
    published: false,
    path: "",
  });
  const VIS = { public: tr("Everyone"), members: tr("Members"), staff: tr("Staff") };
  // A page starts empty: its layout says which areas it has, and blocks go into them.
  const LAYOUTS = [
    ["single", tr("Single column"), tr("One wide column for your blocks, with a top and a bottom.")],
    ["sidebar", tr("Content and sidebar"), tr("A main column with a narrower sidebar beside it.")],
    ["columns2", tr("Two columns"), tr("Two equal columns under a full-width top.")],
    ["columns3", tr("Three columns"), tr("Three equal columns, for features, teasers or links.")],
  ];
  const layoutName = (k) => LAYOUTS.find((l) => l[0] === k)?.[1] ?? k;
  const address = (p) => p.path || `/page/${p.slug}`;
  const load = () => get("/pages").then((r) => (pages = r), status.fail);
  async function save(e) {
    e.preventDefault();
    status.clear();
    const body = {
      title: draft.title,
      slug: draft.slug,
      template: draft.template,
      visibility: draft.visibility,
      published: draft.published,
      path: draft.path ?? "",
    };
    try {
      if (draft.id) await put(`/pages/${draft.id}`, body);
      else await post("/pages", body);
      status.msg = draft.id ? tr("Saved") : tr('Page created. Open it and use "Edit layout" to add blocks.');
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
  let arming = $state(0);
  const remove = (p) => {
    if (arming !== p.id) return (arming = p.id);
    del(`/pages/${p.id}`).then(() => {
      arming = 0;
      load();
    }, status.fail);
  };
  load();
</script>

<PageHead title={tr("Pages")} status={draft ? null : status}>
  {#snippet intro()}{tr(
      'Pages you build from blocks. Pick a layout, then arrange blocks with "Edit layout" on the page. Add one to the header from',
    )}
    <a href="../navigation/">{tr("Navigation")}</a>
    {tr("using its address.")}{/snippet}
  <button onclick={() => (draft = blank())}>{tr("New page")}</button>
</PageHead>
{#if draft}
  <EditDialog
    title={draft.id ? tr("Edit {title}", { title: draft.title }) : tr("New page")}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    save={draft.id ? tr("Save") : tr("Add page")}
  >
    <div class="acp-form">
      <Field label={tr("Title")} bind:value={draft.title} required maxlength="150" wide />
      <Field
        label={`${tr("Address (made from the title if empty)")}: ${tr("about-us")}`}
        bind:value={draft.slug}
        wide
      />
      <Field
        label={`${tr("Own address, instead of /page/… (optional)")}: ${tr("/about")}`}
        bind:value={draft.path}
        wide
      />
      <Field label={tr("Visible to")} bind:value={draft.visibility}>
        {#each Object.entries(VIS) as [k, v]}<option value={k}>{v}</option>{/each}
      </Field>
      <div class="acp-wide">
        <Section switches><Switch label={tr("Published")} bind:checked={draft.published} /></Section>
      </div>
      <div class="acp-wide">
        <Section
          legend={tr("Layout")}
          hint={draft.id ? tr("Blocks in an area the new layout lacks move to its first column.") : ""}
        >
          {#each LAYOUTS as [key, name, about] (key)}
            <label class="acp-check">
              <input type="radio" name="page-layout" value={key} bind:group={draft.template} />
              <span>{name} <small class="muted">· {about}</small></span>
            </label>
          {/each}
        </Section>
      </div>
    </div>
  </EditDialog>
{/if}
<div class="acp-box">
  <ul class="acp-rows">
    {#each pages as p (p.id)}
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{p.title}</strong><br /><a href={address(p)}><small>{address(p)}</small></a>
          <small class="muted">· {layoutName(p.template)} {tr("· updated {n}", { n: when(p.updated_at) })}</small>
        </div>
        <span class="acp-pill {p.published ? 'is-ok' : 'is-muted'}">{p.published ? tr("Published") : tr("Draft")}</span>
        <span class="acp-pill is-muted">{VIS[p.visibility]}</span>
        <a class="acp-btnlink" href={`${address(p)}?edit_layout=1`}>{tr("Design")}</a>
        <button class="secondary" onclick={() => (draft = { ...p })}>{tr("Edit")}</button>
        <button class="is-danger" onclick={() => remove(p)}>
          {arming === p.id ? tr("Click again") : tr("Delete")}
        </button>
      </li>
    {:else}<li class="muted">{tr("No pages yet.")}</li>{/each}
  </ul>
</div>
