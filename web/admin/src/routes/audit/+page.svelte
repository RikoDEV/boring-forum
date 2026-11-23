<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import Field from "#lib/Field.svelte";
  import Empty from "#lib/Empty.svelte";
  import { get } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  let rows = $state([]);
  let action = $state("");
  let actor = $state("");
  const status = new Status();
  let more = $state(false);
  let loaded = $state(false);

  async function load(before) {
    status.clear();
    try {
      const qs = new URLSearchParams({ action, actor, ...(before ? { before } : {}) });
      const page = await get(`/audit?${qs}`);
      rows = before ? [...rows, ...page] : page;
      more = page.length === 100;
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }

  // `kind:id` targets link where the thing can be seen; a member by their name, the rest by number.
  const LINKS = { post: (id) => `/p/${id}`, topic: (id) => `/t/${id}`, ban: () => "/modcp" };
  load();
</script>

{#snippet target(r)}
  {@const [kind, id] = (r.target ?? "").split(":")}
  {#if kind === "user" && r.target_name}<UserLink name={r.target_name} />
  {:else if LINKS[kind] && /^[0-9]+$/.test(id ?? "")}<a href={LINKS[kind](id)}>{r.target}</a>
  {:else}{r.target ?? ""}{/if}
{/snippet}

<PageHead title={tr("Staff action log")} {status} />
<Box title={tr("Filter")}>
  <form
    class="acp-form"
    onsubmit={(e) => {
      e.preventDefault();
      load();
    }}
  >
    <Field label={tr("Action prefix, e.g. topic.")} bind:value={action} />
    <Field label={tr("Staff username")} bind:value={actor} />
    <div class="acp-actions"><button>{tr("Filter")}</button></div>
  </form>
</Box>
<table>
  <thead>
    <tr>
      <th>{tr("When")}</th><th>{tr("Who")}</th><th>{tr("Action")}</th><th>{tr("Target")}</th><th>{tr("Details")}</th>
    </tr>
  </thead>
  <tbody>
    {#each rows as r (r.id)}
      <tr>
        <td><small><When iso={r.created_at} /></small></td><td><UserLink name={r.actor} fallback="system" /></td><td>
          <code>{r.action}</code>
        </td>
        <td>{@render target(r)}</td><td>
          <small class="muted acp-wrap">{JSON.stringify(r.details) === "{}" ? "" : JSON.stringify(r.details)}</small>
        </td>
      </tr>
    {:else}
      <Empty cols={5} {loaded} title={tr("No entries.")} text={tr("Staff actions are recorded here as they happen.")} />
    {/each}
  </tbody>
</table>
{#if more}<div class="row">
    <button class="secondary" onclick={() => load(rows.at(-1).id)}>{tr("Older")}</button>
  </div>{/if}
