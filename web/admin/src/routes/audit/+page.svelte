<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import Notices from "#lib/Notices.svelte";
  import { get } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  let rows = $state([]);
  let action = $state("");
  let actor = $state("");
  const status = new Status();
  let more = $state(false);

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

<h1>{tr("Staff action log")}</h1>
<form
  class="row"
  onsubmit={(e) => {
    e.preventDefault();
    load();
  }}
>
  <input bind:value={action} placeholder={tr("Action prefix, e.g. topic.")} aria-label={tr("Action")} />
  <input bind:value={actor} placeholder={tr("Staff username")} aria-label={tr("Staff member")} />
  <button>{tr("Filter")}</button>
</form>
<Notices {status} />
<table>
  <thead
    ><tr
      ><th>{tr("When")}</th><th>{tr("Who")}</th><th>{tr("Action")}</th><th>{tr("Target")}</th><th>{tr("Details")}</th
      ></tr
    ></thead
  >
  <tbody>
    {#each rows as r (r.id)}
      <tr
        ><td><small><When iso={r.created_at} /></small></td><td><UserLink name={r.actor} fallback="system" /></td><td
          ><code>{r.action}</code></td
        >
        <td>{@render target(r)}</td><td
          ><small class="muted acp-wrap">{JSON.stringify(r.details) === "{}" ? "" : JSON.stringify(r.details)}</small
          ></td
        ></tr
      >
    {:else}
      <tr><td colspan="5" class="muted">{tr("No entries.")}</td></tr>
    {/each}
  </tbody>
</table>
{#if more}<div class="row">
    <button class="secondary" onclick={() => load(rows.at(-1).id)}>{tr("Older")}</button>
  </div>{/if}
