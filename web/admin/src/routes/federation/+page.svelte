<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import TreeSelect from "#lib/TreeSelect.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import Empty from "#lib/Empty.svelte";
  import { api, get, post } from "#lib/api.js";

  let mirrors = $state([]);
  let nodes = $state([]);
  let fresh = $state(null); // the follow being added
  let loaded = $state(false);
  const status = new Status();

  const forumName = (id) => nodes.find((n) => n.id === id)?.title ?? `#${id}`;

  async function load() {
    try {
      const [m, t] = await Promise.all([get("/federation/mirrors"), get("/tree")]);
      mirrors = m;
      nodes = t.nodes;
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }
  load();

  async function add(e) {
    e.preventDefault();
    status.clear();
    try {
      await post("/federation/mirrors", { node_id: Number(fresh.node_id), handle: fresh.handle.trim() });
      status.msg = tr("Follow sent to {n}. It shows as accepted once the other server answers.", {
        n: fresh.handle.trim(),
      });
      fresh = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
  async function remove(m) {
    if (
      !confirm(
        tr("Stop following {handle} into {n}? Topics already copied stay.", {
          handle: m.handle,
          n: forumName(m.node_id),
        }),
      )
    )
      return;
    try {
      await api("DELETE", "/federation/mirrors", { node_id: m.node_id, actor: m.actor });
      await load();
    } catch (err) {
      status.fail(err);
    }
  }
</script>

<PageHead
  title={tr("Federation")}
  intro={tr(
    "A forum here can follow a group on another server: a Lemmy community, a board on another forum, any ActivityPub group. What the group shares becomes topics and replies in that forum, by accounts that cannot sign in here. Only forums guests can read may follow, since what arrives is public. Turn federation on, and direct messages with other servers, under Settings.",
  )}
  status={fresh ? null : status}
>
  <button
    onclick={() => {
      status.clear();
      fresh = { node_id: "", handle: "" };
    }}
  >
    {tr("Follow a remote group")}
  </button>
</PageHead>

{#if fresh}
  <EditDialog
    title={tr("Follow a remote group")}
    onsubmit={add}
    onclose={() => (fresh = null)}
    {status}
    save={tr("Follow")}
  >
    <Section legend={tr("Group")} grid>
      <Field
        label={tr("Group (name@server or its address)")}
        bind:value={fresh.handle}
        required
        wide
        spellcheck="false"
      />
      <TreeSelect
        rows={nodes}
        bind:value={fresh.node_id}
        label={tr("Into")}
        none={tr("Choose a forum")}
        kind="forum"
        required
        wide
      />
    </Section>
  </EditDialog>
{/if}

<table>
  <thead>
    <tr><th>{tr("Group")}</th><th>{tr("Into")}</th><th>{tr("Status")}</th><th class="acp-cell-end"></th></tr>
  </thead>
  <tbody>
    {#each mirrors as m (m.node_id + m.actor)}
      <tr>
        <td><a href={m.actor} rel="noopener">{m.handle}</a></td><td>{forumName(m.node_id)}</td><td>
          {m.accepted ? tr("Following") : tr("Waiting for the other server")}
        </td>
        <td class="acp-cell-end"><button class="is-danger" onclick={() => remove(m)}>{tr("Stop")}</button></td>
      </tr>
    {:else}
      <Empty
        cols={4}
        {loaded}
        title={tr("No forum follows a remote group.")}
        text={tr("Use Follow a remote group above to add one.")}
      />
    {/each}
  </tbody>
</table>
