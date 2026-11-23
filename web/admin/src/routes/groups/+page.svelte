<script>
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put, uploadFile } from "#lib/api.js";
  import { backgroundOf, readable } from "../../../../islands/src/ink.js";
  import { iconHref } from "#lib/icons.js";
  import IconPicker from "#lib/IconPicker.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Box from "#lib/Box.svelte";
  import Empty from "#lib/Empty.svelte";

  const LIMITS = [
    ["max_upload_kb", tr("Upload size, KB"), 0],
    ["posts_per_day", tr("Posts a day"), 0],
    ["edit_minutes", tr("Edit window, minutes"), 0],
    ["max_images_per_post", tr("Images per post"), 0],
    ["pm_per_hour", tr("Conversations an hour"), 0],
    ["pm_max_participants", tr("People per conversation"), 2],
  ];
  const BUILT_IN = {
    1: tr("Visitors who are not signed in"),
    2: tr("Everyone who signs up"),
    3: tr("Moderate every forum"),
    4: tr("Run the whole site"),
  };

  let groups = $state([]);
  let find = $state("");
  let loaded = $state(false);
  let draft = $state(null); // the group being added or changed
  const status = new Status();
  // The preview's text: the group's colour, made readable on what is behind it, as on the site.
  function ink(node, colour) {
    const paint = (c) => {
      node.style.removeProperty("--acp-group-ink");
      if (c) node.style.setProperty("--acp-group-ink", readable(c, backgroundOf(node)));
    };
    paint(colour);
    return { update: paint };
  }
  async function uploadImage(e) {
    const file = e.currentTarget.files[0];
    e.currentTarget.value = "";
    if (!file) return;
    status.clear();
    try {
      draft.image = (await uploadFile(file)).url.split("#")[0];
    } catch (err) {
      status.fail(err);
    }
  }

  const load = () =>
    get("/groups")
      .then((r) => (groups = r), status.fail)
      .finally(() => (loaded = true));
  const shown = $derived(groups.filter((g) => `${g.name} ${g.slug}`.toLowerCase().includes(find.trim().toLowerCase())));
  const kind = (g) =>
    g.is_admin
      ? [tr("Administrators"), "is-danger"]
      : g.is_moderator
        ? [tr("Moderators"), "is-warn"]
        : g.club
          ? [tr("Club"), "is-muted"]
          : g.id <= 2
            ? [tr("Built in"), "is-muted"]
            : [tr("Custom"), "is-muted"];
  const deletable = (g) => g.id > 4 && !g.club;
  const renamable = (g) => g.id !== 1 && !g.club;

  function edit(g) {
    status.clear();
    const limits = Object.fromEntries(LIMITS.map(([k]) => [k, g.limits?.[k] ?? ""]));
    draft = { ...g, colour: g.colour ?? "", icon: g.icon ?? "", image: g.image ?? "", limits, was: g };
  }
  function fresh() {
    status.clear();
    draft = {
      id: 0,
      name: "",
      slug: "",
      colour: "",
      icon: "",
      image: "",
      require_2fa: false,
      limits: Object.fromEntries(LIMITS.map(([k]) => [k, ""])),
      members: 0,
    };
  }

  async function save(e) {
    e.preventDefault();
    status.clear();
    try {
      let id = draft.id;
      if (!id) id = (await post("/groups", { name: draft.name })).id;
      await put(`/groups/${id}`, {
        name: draft.name,
        slug: draft.id ? draft.slug : "",
        colour: draft.colour,
        icon: draft.icon,
        image: draft.image,
      });
      if (id !== 1) {
        const limits = {};
        for (const [k] of LIMITS)
          if (draft.limits[k] !== "" && draft.limits[k] != null) limits[k] = Number(draft.limits[k]);
        await put(`/groups/${id}/limits`, limits);
        if (draft.require_2fa !== (draft.was?.require_2fa ?? false))
          await put(`/groups/${id}/require-2fa`, { on: draft.require_2fa });
      }
      status.msg = draft.id ? tr("Saved {name}.", { name: draft.name }) : tr("Created {name}.", { name: draft.name });
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  async function remove(g) {
    const who = g.members ? ` ${tn("admin.count.group_members_go", g.members)}` : "";
    if (!confirm(tr('Delete the group "{name}"?{who}', { name: g.name, who }))) return;
    status.clear();
    try {
      await del(`/groups/${g.id}`);
      status.msg = tr("Deleted {name}.", { name: g.name });
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  load();
</script>

{#snippet label(g)}
  <span
    class="acp-group-label"
    class:is-styled={!!g.colour}
    style:--acp-group-colour={g.colour || null}
    use:ink={g.colour}
  >
    {#if g.image}<img src={g.image} alt="" />{:else if g.icon}<svg class="bf-icon" aria-hidden="true">
        <use href={iconHref(g.icon)}></use>
      </svg>{/if}{g.name || tr("New group")}
  </span>
{/snippet}

<PageHead
  title={tr("Groups")}
  intro={tr(
    "A group's colour and icon show beside its members' names on posts and profiles. Forum permissions are set per forum on the Forums page.",
  )}
>
  <button onclick={fresh}>{tr("New group")}</button>
</PageHead>
{#if !draft}
  {#if status.error}<p class="err" role="alert">{status.error}</p>{/if}
  {#if status.msg}<p class="ok" role="status">{status.msg}</p>{/if}
{/if}

{#if draft}
  <EditDialog
    title={draft.id ? tr("Edit {name}", { name: draft.was.name }) : tr("New group")}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    ondelete={draft.id && deletable(draft.was) ? () => remove(draft.was) : null}
    deleteLabel={tr("Delete group")}
    save={draft.id ? tr("Save") : tr("Create group")}
  >
    <div class="acp-form">
      <Field
        label={tr("Name")}
        bind:value={draft.name}
        maxlength="50"
        required
        disabled={draft.id && !renamable(draft.was)}
      />
      {#if draft.id}
        <Field
          label={tr("Address: /g/… and @mention")}
          bind:value={draft.slug}
          maxlength="50"
          pattern="[a-z0-9\-]+"
          required
          disabled={!renamable(draft.was)}
        />
      {/if}
      {#if draft.was?.club}<p class="muted acp-wide">
          {tr("Belongs to the club {club}, so it takes the club's name.", { club: draft.was.club })}
        </p>{/if}
    </div>
    <fieldset class="acp-picker">
      <legend>{tr("Colour")}</legend>
      <div class="acp-group-style">
        <label class="acp-colour-input" class:is-empty={!draft.colour} title={tr("Choose a colour")}>
          <input
            type="color"
            value={draft.colour || "#3b82f6"}
            oninput={(e) => (draft.colour = e.currentTarget.value)}
            aria-label={tr("Group colour")}
          />
        </label>
        {#if draft.colour}<button type="button" class="secondary" onclick={() => (draft.colour = "")}>
            {tr("No colour")}
          </button>{:else}<span class="muted">{tr("None: click the circle to choose one.")}</span>{/if}
        <span class="muted">{tr("Looks like")}</span>{@render label(draft)}
      </div>
    </fieldset>
    <fieldset class="acp-picker">
      <legend>{tr("Badge picture")}</legend>
      <div class="acp-group-style">
        {#if draft.image}<span class="acp-image-preview is-wide"><img src={draft.image} alt="" /></span>{/if}
        <label class="acp-upload">
          {tr("Upload a picture")}<input
            type="file"
            accept="image/png,image/jpeg,image/webp,image/gif"
            onchange={uploadImage}
          />
        </label>
        {#if draft.image}<button type="button" class="secondary" onclick={() => (draft.image = "")}>
            {tr("Remove")}
          </button>{/if}
        <span class="muted">
          {draft.image
            ? tr("Shown in place of the icon.")
            : tr("Optional: a rank banner or crest, shown in place of the icon.")}
        </span>
      </div>
    </fieldset>
    <fieldset class="acp-picker">
      <legend>{tr("Icon")}</legend>
      <IconPicker bind:value={draft.icon} />
    </fieldset>
    {#if draft.id !== 1}
      <fieldset class="acp-picker">
        <legend>{tr("Limits (blank means no limit)")}</legend>
        <div class="acp-form acp-form-flat">
          {#each LIMITS as [k, text, min]}<Field label={text} type="number" {min} bind:value={draft.limits[k]} />{/each}
        </div>
      </fieldset>
      <fieldset class="acp-picker">
        <legend>{tr("Sign-in")}</legend>
        <Switch label={tr("Members must use two-factor sign-in")} bind:checked={draft.require_2fa} />
      </fieldset>
    {/if}
  </EditDialog>
{/if}

<Box title={tr("Find a group")}>
  <div class="acp-form"><Field label={tr("Find a group")} type="search" bind:value={find} /></div>
</Box>
<div class="acp-box">
  <ul class="acp-rows">
    {#each shown as g (g.id)}
      {@const [k, tone] = kind(g)}
      <li class="acp-rowitem">
        <div class="acp-grow">
          {@render label(g)}<br /><small class="muted">
            {BUILT_IN[g.id] ?? (g.club ? tr("Club: {club}", { club: g.club }) : `/g/${g.slug}`)}{g.require_2fa
              ? tr(" · two-factor required")
              : ""}
          </small>
        </div>
        <span class="acp-pill {tone}">{k}</span>
        {#if g.plugin}<span class="acp-pill is-muted">{tr("From plugin {plugin}", { plugin: g.plugin })}</span>{/if}
        <small class="muted acp-group-count">{g.id === 1 ? "" : tn("count.members", g.members)}</small>
        {#if !g.plugin}<button class="secondary" onclick={() => edit(g)}>{tr("Edit")}</button>{/if}
      </li>
    {:else}
      <Empty li {loaded} title={tr("No groups match.")} />
    {/each}
  </ul>
</div>
