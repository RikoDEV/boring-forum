<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { get, post, put } from "#lib/api.js";

  // What each requirement measures, in the order a level lists them.
  const KINDS = [
    ["posts", tr("Posts written")],
    ["days", tr("Days since joining")],
    ["days_visited", tr("Days visited")],
    ["topics_read", tr("Topics read")],
    ["reputation", tr("Reputation")],
    ["likes_received", tr("Likes received")],
    ["likes_given", tr("Likes given")],
  ];
  const FIELDS = ["name", "auto", ...KINDS.map((k) => k[0]), "recent_posts"];

  const status = new Status();
  let levels = $state([]); // the levels being edited, from 0 up
  let members = $state({}); // level → members, nearly there
  let recentDays = $state(100);
  let saved = $state("");
  let limit = $state(20);
  let editing = $state(null); // { index, level } — a copy being edited in the dialog
  let preview = $state(null);
  let busy = $state(false);

  const strip = (l) => Object.fromEntries(FIELDS.map((f) => [f, l[f]]));
  const payload = () => ({
    levels: levels.map((l) => ({
      ...strip(l),
      ...Object.fromEntries([...KINDS.map((k) => k[0]), "recent_posts"].map((f) => [f, Math.max(0, Number(l[f]) || 0)])),
    })),
    recent_days: Math.min(3650, Math.max(1, Number(recentDays) || 100)),
  });
  const dirty = $derived(JSON.stringify(payload()) !== saved);

  const earnable = (i) => i === 1 || (i > 1 && levels[i - 1]?.auto);
  const label = (l) => l.name || l.label || tr("Level {n}", { n: l.level });

  /** What a level asks for, in a line. */
  function needs(l) {
    if (l.level === 0) return tr("Everyone starts here");
    if (!l.auto) return tr("Given by a moderator");
    const parts = KINDS.filter(([k]) => Number(l[k]) > 0).map(([k, text]) => `${text}: ${l[k]}`);
    if (Number(l.recent_posts) > 0) parts.push(tr("Posts to keep the level: {n}", { n: l.recent_posts }));
    return parts.length ? parts.join(" · ") : tr("Automatically, with no requirements");
  }

  async function load() {
    try {
      const d = await get("/trust");
      levels = d.levels;
      members = Object.fromEntries(d.levels.map((l) => [l.level, { members: l.members, near: l.near_next }]));
      recentDays = d.recent_days;
      limit = d.limit;
      saved = JSON.stringify(payload());
      preview = null;
    } catch (e) {
      status.fail(e);
    }
  }

  function add() {
    status.clear();
    const level = levels.length;
    levels.push({ level, name: "", auto: false, ...Object.fromEntries(KINDS.map(([k]) => [k, 0])), recent_posts: 0 });
    editing = { index: level, level: { ...levels[level] }, fresh: true };
  }

  function edit(i) {
    status.clear();
    editing = { index: i, level: { ...levels[i] } };
  }

  /** Levels above one that is no longer earned cannot be earned either. */
  function settle() {
    for (let i = 1; i < levels.length; i++) {
      if (levels[i].auto && !earnable(i)) levels[i].auto = false;
    }
  }

  function apply(e) {
    e.preventDefault();
    levels[editing.index] = { ...levels[editing.index], ...editing.level };
    settle();
    editing = null;
    preview = null;
  }

  function cancel() {
    // A level added and then cancelled is not kept.
    if (editing?.fresh) levels.pop();
    editing = null;
  }

  function removeTop() {
    levels.pop();
    editing = null;
    preview = null;
  }

  async function runPreview() {
    busy = true;
    const r = await status.run(() => post("/trust/preview", payload()));
    busy = false;
    if (r) preview = r;
  }

  async function save() {
    busy = true;
    status.clear();
    try {
      await put("/trust", payload());
      status.msg = tr("Trust levels saved");
      await load();
    } catch (e) {
      status.fail(e);
    } finally {
      busy = false;
    }
  }

  load();
</script>

<PageHead
  title={tr("Trust levels")}
  intro={tr(
    "The levels members climb. Earned levels rise by themselves when every requirement is met, checked every ten minutes; other levels are given by moderators. Preview shows who would move before you save.",
  )}
  status={editing ? null : status}
>
  <div class="acp-actions">
    <button class="secondary" disabled={busy || !dirty} onclick={runPreview}>{tr("Preview changes")}</button>
    <button class="secondary" disabled={busy || levels.length > limit} onclick={add}>{tr("New level")}</button>
    <button disabled={busy || !dirty} onclick={save}>{tr("Save")}</button>
  </div>
</PageHead>

{#if preview}
  <Box title={tr("What would change")}>
    <div class="acp-box-body">
      <p>{tr("{n} members checked.", { n: preview.evaluated })}</p>
      <ul>
        {#each preview.promoted_to as n, i}
          <li>{tr("{n} would move up to level {level}.", { n, level: i + 1 })}</li>
        {/each}
        <li>{tr("{n} would drop a level for lack of recent activity.", { n: preview.demoted })}</li>
        {#if preview.moved_down}
          <li>{tr("{n} would move down because their level is removed.", { n: preview.moved_down })}</li>
        {/if}
      </ul>
    </div>
  </Box>
{/if}

<Box title={tr("Levels")}>
  <ul class="acp-rows">
    {#each levels as l, i (l.level)}
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{l.level} · {label(l)}</strong>
          <br /><small class="muted">{needs(l)}</small>
          <br /><small class="muted"
            >{tr("Members")}: {members[l.level]?.members ?? 0}{#if l.auto && members[l.level]?.near}
              · {tr("Nearly at the next level")}: {members[l.level].near}{/if}</small
          >
        </div>
        <button class="secondary" onclick={() => edit(i)}>{tr("Edit")}</button>
      </li>
    {/each}
  </ul>
</Box>

<Box title={tr("Recent activity window")}>
  <div class="acp-form">
    <Field label={tr("Days")} type="number" min="1" max="3650" bind:value={recentDays} />
  </div>
</Box>

{#if editing}
  <EditDialog
    title={editing.fresh
      ? tr("New level {n}", { n: editing.index })
      : tr("Level {level}: {name}", { level: editing.index, name: label(editing.level) })}
    onsubmit={apply}
    onclose={cancel}
    save={editing.fresh ? tr("Add") : tr("Save")}
    ondelete={editing.index > 0 && editing.index === levels.length - 1 && !editing.fresh ? removeTop : null}
    deleteLabel={tr("Remove level")}
  >
    <div class="acp-form">
      <Field label={tr("Name (empty uses the default)")} bind:value={editing.level.name} maxlength="40" wide />
      {#if editing.index > 0}
        <Switch
          label={tr("Earned by activity")}
          bind:checked={editing.level.auto}
          disabled={!earnable(editing.index)}
          wide
        />
      {/if}
      {#if editing.index > 0 && editing.level.auto}
        {#each KINDS as [kind, text] (kind)}
          <Field label={text} type="number" min="0" bind:value={editing.level[kind]} />
        {/each}
        <Field label={tr("Posts to keep the level")} type="number" min="0" bind:value={editing.level.recent_posts} />
      {/if}
    </div>
    {#if editing.index > 0 && !earnable(editing.index)}
      <p class="muted">{tr("A level can only be earned when the level below it is earned too.")}</p>
    {/if}
    {#if editing.index === 0}
      <p class="muted">{tr("Everyone starts at this level, so it has no requirements.")}</p>
    {/if}
  </EditDialog>
{/if}
