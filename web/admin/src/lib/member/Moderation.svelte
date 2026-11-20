<script>
  /// Warnings and private staff notes about a member, with the dialogs that add them.
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import { del, post, when } from "#lib/api.js";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import UserLink from "#lib/UserLink.svelte";

  /** @type {{ id: number, ov: any, refresh: (msg: string) => any }} */
  let { id, ov, refresh } = $props();
  const dlg = new Status();
  let dialog = $state(null); // "warn" | "note"
  let points = $state(1);
  let reason = $state("");
  let note = $state("");

  const open = (which) => {
    dlg.clear();
    reason = note = "";
    points = 1;
    dialog = which;
  };

  async function submit(e, run, done) {
    e.preventDefault();
    dlg.clear();
    try {
      await run();
      dialog = null;
      await refresh(done);
    } catch (err) {
      dlg.fail(err);
    }
  }
</script>

<div class="acp-cols">
  <section class="acp-box">
    <div class="acp-box-head">
      <h2>{tr("Warnings")}</h2>
      <button class="secondary" onclick={() => open("warn")}>{tr("Warn")}</button>
    </div>
    {#if ov?.warnings.length}
      <ul class="acp-rows">
        {#each ov.warnings as w (w.id)}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <strong>{tr("Points")}: {w.points}</strong>
              <span class="acp-pill" class:is-muted={!w.active}>{w.active ? tr("Active") : tr("Expired")}</span>
              <br /><small class="muted acp-wrap">{w.reason}</small>
            </div>
            <small class="muted"><UserLink name={w.moderator} fallback={tr("Staff")} /> · {when(w.created_at)}</small>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="acp-empty">{tr("No warnings.")}</p>
    {/if}
  </section>

  <section class="acp-box">
    <div class="acp-box-head">
      <h2>{tr("Staff notes")}</h2>
      <button class="secondary" onclick={() => open("note")}>{tr("Add note")}</button>
    </div>
    {#if ov?.notes.length}
      <ul class="acp-rows">
        {#each ov.notes as n (n.id)}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <span class="acp-wrap">{n.body}</span>
              <br /><small class="muted"
                ><UserLink name={n.author} fallback={tr("Staff")} /> · {when(n.created_at)}</small
              >
            </div>
            <button
              class="secondary"
              onclick={() =>
                del(`/users/${id}/notes/${n.id}`).then(
                  () => refresh(tr("Note removed")),
                  (e) => dlg.fail(e),
                )}>{tr("Remove")}</button
            >
          </li>
        {/each}
      </ul>
    {:else}
      <p class="acp-empty">{tr("Only staff see these notes.")}</p>
    {/if}
  </section>
</div>

{#if dialog === "warn"}
  <EditDialog
    title={tr("Warn this member")}
    status={dlg}
    save={tr("Warn")}
    onclose={() => (dialog = null)}
    onsubmit={(e) =>
      submit(e, () => post(`/users/${id}/warn`, { points: Number(points), reason }), tr("Warning given"))}
  >
    <div class="acp-form">
      <Field label={tr("Points")} type="number" min="1" max="100" bind:value={points} required />
      <Field label={tr("Reason the member sees")} bind:value={reason} rows="4" maxlength="500" required wide />
    </div>
  </EditDialog>
{:else if dialog === "note"}
  <EditDialog
    title={tr("Add a staff note")}
    status={dlg}
    save={tr("Add note")}
    onclose={() => (dialog = null)}
    onsubmit={(e) => submit(e, () => post(`/users/${id}/notes`, { body: note }), tr("Note added"))}
  >
    <div class="acp-form">
      <Field label={tr("Note, only staff see it")} bind:value={note} rows="5" maxlength="2000" required wide />
    </div>
  </EditDialog>
{/if}
