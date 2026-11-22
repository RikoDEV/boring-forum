<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import Switch from "#lib/Switch.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import Box from "#lib/Box.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { get, put, when } from "#lib/api.js";
  import { Crud } from "#lib/crud.svelte.js";

  const PLACES = {
    header: tr("Under the header"),
    between_posts: tr("Between posts"),
    topic_list: tr("Above topic lists"),
    sidebar: tr("In sidebars"),
    footer: tr("Above the footer"),
  };
  let ads = $state([]);
  let groups = $state([]);
  const crud = new Crud({ path: "/ads", blank: () => blank(), load: () => load() });
  const status = crud.status;
  const groupOptions = $derived(groups.map((g) => ({ value: String(g.id), label: g.name })));

  async function load() {
    try {
      [ads, groups] = await Promise.all([get("/ads"), get("/groups")]);
    } catch (e) {
      status.fail(e);
    }
  }
  load();

  const day = (iso) => (iso ? iso.slice(0, 10) : "");
  const blank = () => ({
    id: 0,
    name: "",
    placement: "header",
    html: "",
    url: "",
    show_groups: [],
    hide_groups: [],
    every: 5,
    weight: 1,
    starts: "",
    ends: "",
    active: true,
  });
  function edit(a) {
    crud.open(
      a && {
        ...a,
        url: a.url ?? "",
        show_groups: a.show_groups.map(String),
        hide_groups: a.hide_groups.map(String),
        starts: day(a.starts_at),
        ends: day(a.ends_at),
      },
    );
  }
  function save(e) {
    e.preventDefault();
    const a = crud.draft;
    crud.save(
      {
        name: a.name,
        placement: a.placement,
        html: a.html,
        url: a.url || null,
        show_groups: a.show_groups.map(Number),
        hide_groups: a.hide_groups.map(Number),
        every: Number(a.every) || 5,
        weight: Number(a.weight) || 1,
        active: a.active,
        starts_at: a.starts ? new Date(`${a.starts}T00:00:00Z`).toISOString() : null,
        ends_at: a.ends ? new Date(`${a.ends}T23:59:59Z`).toISOString() : null,
      },
      tr('Saved "{name}".', { name: a.name }),
    );
  }
  // ads.txt is one site setting rather than an ad, so it has its own small form.
  let adsTxt = $state("");
  const adsTxtStatus = new Status();
  get("/settings").then(
    (all) => (adsTxt = String(all.find((x) => x.key === "ads_txt")?.value ?? "")),
    (e) => adsTxtStatus.fail(e),
  );
  const saveAdsTxt = (e) => {
    e.preventDefault();
    return adsTxtStatus.run(() => put("/settings/ads_txt", { value: adsTxt }), tr("Saved."));
  };
  const remove = (a) => confirm(tr('Delete the ad "{name}"?', { name: a.name })) && crud.remove(a.id);
  const ctr = (a) => (a.impressions ? `${((a.clicks / a.impressions) * 100).toFixed(2)}%` : "—");
</script>

<PageHead
  title={tr("Advertisements")}
  intro={tr(
    "One ad per place on a page, picked by weight among those the visitor's groups may see. HTML is cleaned when saved: no scripts or embeds. Impressions are saved every few minutes.",
  )}
  status={crud.draft ? null : status}
>
  <button onclick={() => edit(null)}>{tr("New ad")}</button>
</PageHead>

{#if crud.draft}
  <EditDialog
    title={crud.draft.id ? tr("Edit ad") : tr("New ad")}
    size="is-large"
    onsubmit={save}
    onclose={crud.close}
    {status}
    ondelete={crud.draft.id ? () => remove(crud.draft) : null}
    save={crud.draft.id ? tr("Save") : tr("Add ad")}
  >
    <Section grid>
      <Field label={tr("Name (staff only)")} bind:value={crud.draft.name} maxlength="100" required />
      <Field label={tr("Where")} bind:value={crud.draft.placement} options={Object.entries(PLACES)} />
      {#if crud.draft.placement === "between_posts"}<Field
          label={tr("After every n-th post")}
          type="number"
          min="1"
          max="100"
          bind:value={crud.draft.every}
        />{/if}
      <Field
        label={tr("HTML (an image, text, a banner)")}
        rows="6"
        class="acp-code"
        bind:value={crud.draft.html}
        required
        wide
        spellcheck="false"
      />
      <Field label={tr("Link (counted; optional)")} type="url" bind:value={crud.draft.url} wide />
    </Section>
    <Section legend={tr("Who sees it")} grid>
      <MultiSelect
        label={tr("Show to")}
        placeholder={tr("Everyone")}
        options={groupOptions}
        bind:value={crud.draft.show_groups}
      />
      <MultiSelect
        label={tr("Never show to")}
        placeholder={tr("No one")}
        options={groupOptions}
        bind:value={crud.draft.hide_groups}
      />
    </Section>
    <Section legend={tr("When")} grid>
      <Field label={tr("From")} type="date" bind:value={crud.draft.starts} />
      <Field label={tr("Until")} type="date" bind:value={crud.draft.ends} />
      <Field label={tr("Weight")} type="number" min="1" max="100" bind:value={crud.draft.weight} />
    </Section>
    <Section legend={tr("Status")} switches>
      <Switch label={tr("Active")} bind:checked={crud.draft.active} />
    </Section>
  </EditDialog>
{/if}

<table>
  <thead>
    <tr>
      <th>{tr("Ad")}</th><th>{tr("Where")}</th><th>{tr("Impressions")}</th><th>{tr("Clicks")}</th><th>
        {tr("Click rate")}
      </th><th>{tr("Runs")}</th>
    </tr>
  </thead>
  <tbody>
    {#each ads as a (a.id)}
      <tr class:muted={!a.active}>
        <td>
          <button class="link" onclick={() => edit(a)}>{a.name}</button>{#if !a.active}
            <small>{tr("(off)")}</small>{/if}
        </td>
        <td>{PLACES[a.placement]}</td><td>{a.impressions}</td><td>{a.url ? a.clicks : "—"}</td><td>
          {a.url ? ctr(a) : "—"}
        </td>
        <td class="small">{a.starts_at || a.ends_at ? `${when(a.starts_at)} – ${when(a.ends_at)}` : tr("Always")}</td>
      </tr>
    {:else}
      <tr><td colspan="6" class="muted">{tr("No ads.")}</td></tr>
    {/each}
  </tbody>
</table>

<Box title={tr("The ads.txt file")}>
  <form class="acp-form" onsubmit={saveAdsTxt}>
    <Field
      label={tr("Published at /ads.txt (empty: no file)")}
      rows="6"
      class="acp-code"
      bind:value={adsTxt}
      wide
      spellcheck="false"
    />
    <div class="acp-actions acp-wide">
      <button>{tr("Save")}</button>
      {#if adsTxtStatus.msg}<small class="muted">{adsTxtStatus.msg}</small>{/if}
      {#if adsTxtStatus.error}<small class="error">{adsTxtStatus.error}</small>{/if}
    </div>
  </form>
</Box>
