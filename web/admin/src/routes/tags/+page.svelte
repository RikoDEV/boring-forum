<script>
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, patch, post, uploadFile, when } from "#lib/api.js";
  import UserLink from "#lib/UserLink.svelte";
  import Field from "#lib/Field.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Dialog from "#lib/Dialog.svelte";
  let tab = $state("tags");
  let tags = $state([]);
  let report = $state([]);
  const status = new Status();
  const load = () =>
    Promise.all([get("/tags"), get("/search-report")]).then(([t, r]) => {
      tags = t;
      report = r;
    }, status.fail);
  const rename = (t) => patch(`/tags/${t.id}`, { name: t.name }).then(load, status.fail);
  const remove = (t) =>
    confirm(tr('Delete tag "{name}" from {topics}?', { name: t.name, topics: tn("count.topics", t.topics) })) &&
    del(`/tags/${t.id}`).then(load, status.fail);
  let emoji = $state([]);
  let eName = $state("");
  let invites = $state([]);
  let newInvite = $state("");
  // The dialog that is open: a new tag, a new emoji, or the invite link just made.
  let dialog = $state(null);
  let tagName = $state("");
  let eFile = $state(null);
  const loadExtras = () => {
    fetch("/api/v1/emoji")
      .then((r) => r.json())
      .then(
        (r) => (emoji = r),
        () => {},
      );
    get("/invites").then((r) => (invites = r), status.fail);
  };
  async function addEmoji(e) {
    e.preventDefault();
    status.clear();
    if (!eFile) return;
    let data;
    try {
      data = await uploadFile(eFile);
    } catch (err) {
      return (status.error = err.message || tr("Upload failed"));
    }
    try {
      await post("/emoji", { name: eName, url: data.url });
    } catch (err) {
      return status.fail(err);
    }
    eName = "";
    eFile = null;
    dialog = null;
    loadExtras();
  }
  async function addTag(e) {
    e.preventDefault();
    status.clear();
    try {
      await post("/tags", { name: tagName });
    } catch (err) {
      return status.fail(err);
    }
    tagName = "";
    dialog = null;
    load();
  }
  const makeInvite = () => {
    status.clear();
    return post("/invites", {}).then((r) => {
      newInvite = r.url;
      dialog = "invite";
      loadExtras();
    }, status.fail);
  };
  const show = (d) => () => {
    status.clear();
    dialog = d;
  };
  const ADD = {
    tags: [tr("New tag"), show("tag")],
    emoji: [tr("New emoji"), show("emoji")],
    invites: [tr("Create invite link"), makeInvite],
  };
  load();
  loadExtras();
</script>

<PageHead title={tr("Tags, emoji, invites")} status={dialog ? null : status} reserve>
  {#if ADD[tab]}<button onclick={ADD[tab][1]}>{ADD[tab][0]}</button>{/if}
</PageHead>
{#if dialog === "tag"}
  <EditDialog title={tr("New tag")} onclose={() => (dialog = null)} onsubmit={addTag} {status} save={tr("Add tag")}>
    <div class="acp-form"><Field label={tr("Tag name")} bind:value={tagName} maxlength="40" required wide /></div>
  </EditDialog>
{:else if dialog === "emoji"}
  <EditDialog
    title={tr("New emoji")}
    onclose={() => (dialog = null)}
    onsubmit={addEmoji}
    {status}
    save={tr("Add emoji")}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={eName} required wide />
      <label class="acp-wide">
        {tr("Picture")}<input
          type="file"
          accept="image/*"
          required
          onchange={(e) => (eFile = e.currentTarget.files[0])}
        />
      </label>
    </div>
  </EditDialog>
{:else if dialog === "invite"}
  <Dialog title={tr("Invite link")} onclose={() => (dialog = null)}>
    <div class="bf-dialog-body">
      <p class="acp-wrap"><code>{newInvite}</code></p>
      <p class="muted">{tr("Copy it now; it is not shown again.")}</p>
    </div>
    <div class="bf-dialog-actions">
      <button type="button" onclick={() => navigator.clipboard?.writeText(newInvite)}>{tr("Copy")}</button>
    </div>
  </Dialog>
{/if}
<div class="acp-tabs" role="tablist">
  {#each [["tags", tr("Tags"), tags.length], ["emoji", tr("Custom emoji"), emoji.length], ["invites", tr("Invites"), invites.length], ["searches", tr("Top searches"), report.length]] as [key, label, n]}
    <button role="tab" aria-selected={tab === key} onclick={() => (tab = String(key))}>
      {label}<span class="acp-count">{n}</span>
    </button>
  {/each}
</div>
{#if tab === "tags"}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each tags as t (t.id)}
        <li class="acp-rowitem">
          <input
            class="acp-grow"
            bind:value={t.name}
            aria-label={tr("Tag name")}
            disabled={!!t.plugin}
          />{#if t.plugin}<span class="acp-pill is-muted">
              {tr("From plugin {plugin}", { plugin: t.plugin })}
            </span>{/if}<small class="muted">{tn("count.topics", t.topics)}</small>{#if !t.plugin}<button
              class="secondary"
              onclick={() => rename(t)}
            >
              {tr("Rename")}
            </button><button class="is-danger" onclick={() => remove(t)}>{tr("Delete")}</button>{/if}
        </li>
      {:else}<li class="muted">{tr("No tags yet. Add one, or they appear here once a topic uses them.")}</li>{/each}
    </ul>
  </div>
{:else if tab === "emoji"}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each emoji as e (e.name)}
        <li class="acp-rowitem">
          <img src={e.url} alt="" width="24" height="24" /><code class="acp-grow">:{e.name}:</code><button
            class="is-danger"
            onclick={() => del(`/emoji/${e.name}`).then(loadExtras)}
          >
            {tr("Delete")}
          </button>
        </li>
      {:else}<li class="muted">{tr("No custom emoji yet.")}</li>{/each}
    </ul>
  </div>
{:else if tab === "invites"}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each invites as i (i.id)}
        <li class="acp-rowitem">
          <span class="acp-grow">{tr("Invite #{id}", { id: i.id })}</span>{#if i.used_by}<span class="acp-pill is-ok">
              {tr("Used by")}
              <UserLink name={i.used_by} />
            </span>{:else}<small class="muted" title={when(i.expires_at)}>
              {tr("Expires {n}", { n: i.expires_at.slice(0, 10) })}
            </small><button class="is-danger" onclick={() => del(`/invites/${i.id}`).then(loadExtras)}>
              {tr("Revoke")}
            </button>{/if}
        </li>
      {:else}<li class="muted">{tr("No invites yet.")}</li>{/each}
    </ul>
  </div>
{:else}
  <p class="muted"><small>{tr("Last 30 days.")}</small></p>
  <table>
    <thead><tr><th>{tr("Term")}</th><th>{tr("Searches")}</th><th>{tr("Average results")}</th></tr></thead>
    <tbody>
      {#each report as r (r.term)}<tr>
          <td>{r.term}</td><td>{r.searches}</td><td>{r.avg_results.toFixed(1)}</td>
        </tr>{:else}<tr><td colspan="3" class="muted">{tr("No searches yet.")}</td></tr>{/each}
    </tbody>
  </table>
{/if}
