<script>
  import { resolve } from "$app/paths";
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { get, put } from "#lib/api.js";
  import { Crud } from "#lib/crud.svelte.js";
  import { sprite } from "#lib/icons.js";
  import IconPicker from "#lib/IconPicker.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";

  const COLORS = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
  const TIERS = [
    ["bronze", tr("Bronze")],
    ["silver", tr("Silver")],
    ["gold", tr("Gold")],
  ];
  const blank = () => ({
    id: 0,
    name: "",
    description: "",
    category: "Custom",
    tier: "bronze",
    icon: "trophy",
    color: 1,
    level: 0,
    min_posts: 0,
    min_reputation: 0,
    min_solved: 0,
    points: 1,
    enabled: true,
    slug: null,
  });

  let badges = $state([]);
  const crud = new Crud({ path: "/badges", blank: () => blank(), load: () => load() }); // draft: the badge being added or changed
  const status = crud.status;
  let find = $state("");

  const load = () => get("/badges").then((r) => (badges = r), status.fail);
  const levelText = (n) => (n >= 1000 && n % 1000 === 0 ? `${n / 1000}k` : String(n));
  const shown = $derived(
    badges.filter((b) => `${b.name} ${b.category}`.toLowerCase().includes(find.trim().toLowerCase())),
  );
  const categories = $derived([...new Set(shown.map((b) => b.category))]);

  function save(e) {
    e.preventDefault();
    status.clear();
    const body = {
      ...crud.draft,
      points: Number(crud.draft.points),
      level: Number(crud.draft.level),
      min_posts: Number(crud.draft.min_posts),
      min_reputation: Number(crud.draft.min_reputation),
      min_solved: Number(crud.draft.min_solved),
    };
    crud.save(body);
  }
  const toggle = (b) => put(`/badges/${b.id}`, { ...b, enabled: !b.enabled }).then(load, status.fail);
  const ofPlugin = (b) => b.slug?.startsWith("plugin:");
  const remove = (b) =>
    confirm(
      b.slug
        ? tr("Delete {name}? Members who earned it lose it. This is one of the forum's own badges and cannot be brought back.", { name: b.name })
        : tr("Delete {name}? Members who earned it lose it.", { name: b.name }),
    ) &&
    crud.remove(b.id);
  load();
</script>

{#snippet medal(b)}
  <span class="bf-medal bf-medal-{b.tier}" title={b.tier}
    ><span class="bf-medal-shape bf-medal-c{b.color}"
      ><svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{b.icon}"></use></svg></span
    >{#if Number(b.level) > 0}<span class="bf-medal-level">{levelText(Number(b.level))}</span>{/if}</span
  >
{/snippet}

<PageHead
  title={tr("Badges")}
  intro={tr(
    "Members earn these automatically. You can change how any of them looks, including the ones that come with the forum.",
  )}
>
  <button onclick={() => crud.open()}>{tr("New badge")}</button>
</PageHead>
{#if status.error && !crud.draft}<p class="err" role="alert">{status.error}</p>{/if}

{#if crud.draft}
  <EditDialog
    title={crud.draft.id ? tr("Edit {name}", { name: crud.draft.name }) : tr("New badge")}
    size="is-large"
    onclose={crud.close}
    onsubmit={save}
    {status}
    ondelete={crud.draft.id && !ofPlugin(crud.draft) ? () => remove(crud.draft) : null}
    deleteLabel={tr("Delete")}
    save={crud.draft.id ? tr("Save") : tr("Add badge")}
  >
    {#snippet extra()}{@render medal(crud.draft)}{/snippet}
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={crud.draft.name} maxlength="60" required />
      <Field label={tr("Description")} bind:value={crud.draft.description} wide />
      <Field label={tr("Group")} bind:value={crud.draft.category} maxlength="40" />
      <Field label={tr("Medal shape")} bind:value={crud.draft.tier}
        >{#each TIERS as [v, label]}<option value={v}>{label}</option>{/each}</Field
      >
      <Field label={tr("Points")} type="number" min="0" bind:value={crud.draft.points} />
      <Field label={tr("Number on the medal (0: none)")} type="number" min="0" bind:value={crud.draft.level} />
      <p class="acp-wide muted">
        <small
          >{tr("The name, description and group show as written here, in every language, until translated.")}
          <a href={resolve("/text")}>{tr("Translate them in Site text")}</a></small
        >
      </p>
    </div>
    <fieldset class="acp-picker">
      <legend>{tr("Colour")}</legend>
      <div class="acp-colors">
        {#each COLORS as c}<label class="acp-color bf-medal-c{c}" class:is-on={crud.draft.color === c}
            ><input
              type="radio"
              name="color"
              value={c}
              bind:group={crud.draft.color}
              aria-label={tr("Colour {n}", { n: c + 1 })}
            /></label
          >{/each}
      </div>
    </fieldset>
    <fieldset class="acp-picker">
      <legend>{tr("Icon")}</legend>
      <!-- Medals draw from the general sprite only, and always have an icon. -->
      <IconPicker bind:value={crud.draft.icon} brands={false} none={false} />
    </fieldset>
    {#if crud.draft.slug}
      <p class="acp-empty">{tr("When this badge is earned is built into the forum and can't be changed.")}</p>
    {:else}
      <fieldset class="acp-picker">
        <legend>{tr("Earned at (any one is enough to start; all you set must be met)")}</legend>
        <div class="acp-form acp-form-flat">
          <Field label={tr("Posts")} type="number" min="0" bind:value={crud.draft.min_posts} />
          <Field label={tr("Reputation")} type="number" min="0" bind:value={crud.draft.min_reputation} />
          <Field label={tr("Accepted answers")} type="number" min="0" bind:value={crud.draft.min_solved} />
        </div>
      </fieldset>
    {/if}
  </EditDialog>
{/if}

<div class="acp-settings-head">
  <input type="search" bind:value={find} placeholder={tr("Find a badge")} aria-label={tr("Find a badge")} />
</div>
{#each categories as category}
  <h2>{category}</h2>
  <div class="acp-box">
    <ul class="acp-rows">
      {#each shown.filter((b) => b.category === category) as b (b.id)}
        <li class="acp-rowitem" class:is-off={!b.enabled}>
          {@render medal(b)}
          <div class="acp-grow">
            <strong>{b.name}</strong>
            {#if !b.enabled}<span class="acp-pill is-muted">{tr("Off")}</span>{/if}
            {#if b.slug?.startsWith("plugin:")}<span class="acp-pill is-muted"
                >{tr("From plugin {plugin}", { plugin: b.slug.split(":")[1] })}</span
              >{/if}<br /><small class="muted"
              >{b.description}{b.description ? " · " : ""}{tn("count.points", b.points)}</small
            >
          </div>
          {#if !b.slug?.startsWith("plugin:")}
            <button class="secondary" onclick={() => crud.open({ ...b })}>{tr("Edit")}</button>
            <button class="secondary" onclick={() => toggle(b)}>{b.enabled ? tr("Turn off") : tr("Turn on")}</button>
          {/if}
        </li>
      {/each}
    </ul>
  </div>
{/each}
