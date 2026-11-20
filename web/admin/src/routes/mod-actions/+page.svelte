<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import IconButton from "#lib/IconButton.svelte";
  import { del, get, post, put } from "#lib/api.js";

  const KINDS = {
    lock: tr("Lock the topic"),
    unlock: tr("Unlock the topic"),
    pin: tr("Pin the topic"),
    unpin: tr("Unpin the topic"),
    reply: tr("Post a reply"),
    move: tr("Move to a forum"),
  };
  const MAX_STEPS = 10;
  let actions = $state([]);
  let forums = $state([]);
  let draft = $state(null); // the action being added or changed
  const status = new Status();

  const load = () => get("/mod-actions").then((r) => (actions = r), status.fail);
  get("/tree").then(
    (t) => (forums = t.nodes.filter((n) => n.kind === "forum")),
    () => {},
  );
  const forumName = (id) => forums.find((f) => String(f.id) === String(id))?.title ?? `#${id}`;
  const stepText = (s) =>
    s.kind === "move"
      ? tr("Move to {forum}", { forum: forumName(s.value) })
      : s.kind === "reply"
        ? tr("Post a reply")
        : KINDS[s.kind];

  const open = (a) => {
    status.clear();
    draft = a
      ? { id: a.id, name: a.name, steps: a.steps.map((s) => ({ kind: s.kind, value: s.value ?? "" })) }
      : { id: 0, name: "", steps: [{ kind: "lock", value: "" }] };
  };
  const addStep = () => draft.steps.length < MAX_STEPS && draft.steps.push({ kind: "lock", value: "" });

  async function save(e) {
    e.preventDefault();
    status.clear();
    const body = {
      id: draft.id,
      name: draft.name.trim(),
      steps: draft.steps.map((s) => ({
        kind: s.kind,
        value: s.kind === "reply" || s.kind === "move" ? String(s.value).trim() : null,
      })),
    };
    try {
      await (draft.id ? put(`/mod-actions/${draft.id}`, body) : post("/mod-actions", body));
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
  const remove = (a) =>
    confirm(tr("Delete {name}?", { name: a.name })) &&
    del(`/mod-actions/${a.id}`).then(() => {
      draft = null;
      load();
    }, status.fail);

  load();
</script>

<PageHead
  title={tr("Saved moderator actions")}
  intro={tr(
    "A named series of steps a moderator runs on a topic with one click: lock, pin, move or post a canned reply. Bans and IP lookups stay in the moderator panel.",
  )}
  status={draft ? null : status}
>
  <button onclick={() => open(null)}>{tr("New action")}</button>
</PageHead>

{#if draft}
  <EditDialog
    title={draft.id ? tr("Edit {name}", { name: draft.name }) : tr("New action")}
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    ondelete={draft.id ? () => remove(draft) : null}
    save={draft.id ? tr("Save") : tr("Add")}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={draft.name} maxlength="60" required wide />
    </div>
    <Section legend={tr("Steps, run in order")}>
      {#each draft.steps as s, i}
        <div class="acp-form acp-form-flat">
          <Field label={tr("Step {n}", { n: i + 1 })} bind:value={s.kind} options={Object.entries(KINDS)} />
          {#if s.kind === "move"}
            <Field
              label={tr("Forum")}
              bind:value={s.value}
              required
              options={[["", ""], ...forums.map((f) => [String(f.id), f.title])]}
            />
          {:else if s.kind === "reply"}
            <Field label={tr("Reply text")} bind:value={s.value} rows="3" maxlength="10000" required wide />
          {/if}
          <IconButton
            icon="trash-2"
            label={tr("Remove step {n}", { n: i + 1 })}
            disabled={draft.steps.length === 1}
            onclick={() => draft.steps.splice(i, 1)}
          />
        </div>
      {/each}
      <div class="acp-actions">
        <button type="button" class="secondary" disabled={draft.steps.length >= MAX_STEPS} onclick={addStep}>
          {tr("Add a step")}
        </button>
      </div>
    </Section>
  </EditDialog>
{/if}

<table>
  <thead><tr><th>{tr("Name")}</th><th>{tr("Steps")}</th><th class="acp-cell-end"></th></tr></thead>
  <tbody>
    {#each actions as a (a.id)}
      <tr>
        <td>{a.name}</td><td class="muted">{a.steps.map(stepText).join(" → ")}</td>
        <td class="acp-cell-end"><button class="secondary" onclick={() => open(a)}>{tr("Edit")}</button></td>
      </tr>
    {:else}
      <tr><td colspan="3" class="muted">{tr("No saved actions yet.")}</td></tr>
    {/each}
  </tbody>
</table>
