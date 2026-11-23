<script>
  import { followTabHash } from "#lib/tabs.svelte.js";
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import Field from "#lib/Field.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Empty from "#lib/Empty.svelte";
  let tab = $state("hooks");
  followTabHash(["hooks", "log", "apps"], (key) => (tab = key));
  let hooks = $state([]);
  let events = $state([]);
  let log = $state([]);
  const status = new Status();
  const fresh = $state({ url: "", events: ["*"], node_ids: [] });
  let forums = $state([]);
  let secret = $state("");
  let loaded = $state(false);
  let loadedClients = $state(false);
  const load = () =>
    Promise.all([
      get("/tree").then((r) => (forums = r.nodes.filter((n) => n.kind === "forum")), status.fail),
      get("/webhooks").then((r) => {
        hooks = r.webhooks;
        events = r.events;
      }, status.fail),
      get("/webhook-deliveries").then((r) => (log = r), status.fail),
    ]).then(() => (loaded = true));
  let dialog = $state(null);
  function add(e) {
    e.preventDefault();
    status.clear();
    post("/webhooks", fresh).then((r) => {
      secret = r.secret;
      fresh.url = "";
      fresh.node_ids = [];
      dialog = null;
      load();
    }, status.fail);
  }
  const NEW = { hooks: ["hook", tr("New webhook")], apps: ["app", tr("New application")] };
  let clients = $state([]);
  let app = $state({ name: "", uri: "" });
  let creds = $state(null);
  const loadClients = () =>
    get("/oauth-clients")
      .then((r) => (clients = r), status.fail)
      .finally(() => (loadedClients = true));
  function addClient(e) {
    e.preventDefault();
    post("/oauth-clients", { name: app.name, redirect_uris: [app.uri] }).then((r) => {
      creds = r;
      app = { name: "", uri: "" };
      dialog = null;
      loadClients();
    }, status.fail);
  }
  loadClients();
  load();
</script>

<PageHead
  title={tr("Webhooks and apps")}
  intro={tr("Tell other services what happens here, and let other apps sign members in.")}
  status={dialog ? null : status}
  reserve
>
  {#if NEW[tab]}<button onclick={() => (status.clear(), (dialog = NEW[tab][0]))}>{NEW[tab][1]}</button>{/if}
</PageHead>
{#if dialog === "hook"}
  <EditDialog
    title={tr("New webhook")}
    onclose={() => (dialog = null)}
    onsubmit={add}
    {status}
    save={tr("Add webhook")}
  >
    <div class="acp-form">
      <Field
        label={tr("Address")}
        type="url"
        bind:value={fresh.url}
        placeholder={tr("https://example.com/hook")}
        required
        wide
      />
      <div class="acp-wide">
        <MultiSelect
          label={tr("Events")}
          placeholder={tr("Choose events")}
          options={[{ value: "*", label: tr("All events") }, ...events.map((ev) => ({ value: ev, label: ev }))]}
          bind:value={fresh.events}
        />
      </div>
      <div class="acp-wide">
        <MultiSelect
          label={tr("Forums")}
          placeholder={tr("All forums")}
          options={forums.map((f) => ({ value: f.id, label: f.title }))}
          bind:value={fresh.node_ids}
        />
      </div>
    </div>
  </EditDialog>
{:else if dialog === "app"}
  <EditDialog
    title={tr("New application")}
    onclose={() => (dialog = null)}
    onsubmit={addClient}
    {status}
    save={tr("Add application")}
  >
    <div class="acp-form">
      <Field label={tr("App name")} bind:value={app.name} required />
      <Field label={tr("Redirect address")} type="url" bind:value={app.uri} required />
    </div>
  </EditDialog>
{/if}
<div class="acp-tabs" role="tablist">
  {#each [["hooks", tr("Webhooks"), hooks.length], ["log", tr("Deliveries"), log.length], ["apps", tr("Applications"), clients.length]] as [k, label, n]}<button
      role="tab"
      aria-selected={tab === k}
      onclick={() => (tab = String(k))}
    >
      {label}<span class="acp-count">{n}</span>
    </button>{/each}
</div>
{#if tab === "hooks"}
  <p class="muted">
    <small>
      {tr("Each delivery is a signed POST: verify the")} <code>X-BF-Signature</code>
      {tr("header, an HMAC-SHA256 of the body with the secret. Failures retry with growing delays, up to 8 times.")}
    </small>
  </p>
  {#if secret}<p class="ok" role="status">{tr("Secret, shown once:")} <code>{secret}</code></p>{/if}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each hooks as h (h.id)}<li class="acp-rowitem">
          <div class="acp-grow">
            <code>{h.url}</code>
            <div class="acp-chips">
              {#each h.events as ev}<span class="acp-pill is-muted">{ev === "*" ? tr("All events") : ev}</span>{/each}
              {#each h.node_ids as id}<span class="acp-pill">
                  {forums.find((f) => f.id === id)?.title ?? tr("Deleted #{v}", { v: id })}
                </span>{/each}
            </div>
          </div>
          <button class="is-danger" onclick={() => del(`/webhooks/${h.id}`).then(load, status.fail)}>
            {tr("Delete")}
          </button>
        </li>{:else}<Empty
          li
          {loaded}
          title={tr("No webhooks yet.")}
          text={tr("Use New webhook above to add one.")}
        />{/each}
    </ul>
  </div>
{:else if tab === "log"}
  <table>
    <thead>
      <tr>
        <th>{tr("When")}</th><th>{tr("Event")}</th><th>{tr("Status")}</th><th>{tr("Attempts")}</th><th>
          {tr("Last result")}
        </th>
      </tr>
    </thead>
    <tbody>
      {#each log as d (d.id)}<tr>
          <td><small><When iso={d.created_at} /></small></td><td><code>{d.event}</code></td><td>
            <span
              class="acp-pill {d.status === 'delivered' ? 'is-ok' : d.status === 'failed' ? 'is-danger' : 'is-muted'}"
            >
              {d.status}
            </span>
          </td><td>{d.attempts}</td><td><small>{d.last_error ?? d.last_status ?? ""}</small></td>
        </tr>{:else}<Empty
          cols={5}
          {loaded}
          title={tr("None yet.")}
          text={tr("Deliveries are listed here once a webhook fires.")}
        />{/each}
    </tbody>
  </table>
{:else}
  <p class="muted">
    <small>
      {tr("Other apps can sign members in through this forum:")} <code>/oauth/authorize</code>
      {tr("(code flow with S256 PKCE),")} <code>/oauth/token</code>, <code>/oauth/userinfo</code>.
    </small>
  </p>
  {#if creds}<p class="ok" role="status">
      {tr("Client ID")} <code>{creds.client_id}</code>{tr(", secret, shown once:")} <code>{creds.client_secret}</code>
    </p>{/if}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each clients as c (c.client_id)}<li class="acp-rowitem">
          <div class="acp-grow">
            <strong>{c.name}</strong><br /><small class="muted"><code>{c.client_id}</code></small>
          </div>
          <button class="is-danger" onclick={() => del(`/oauth-clients/${c.client_id}`).then(loadClients, status.fail)}>
            {tr("Delete")}
          </button>
        </li>{:else}<Empty
          li
          loaded={loadedClients}
          title={tr("No applications yet.")}
          text={tr("Use New application above to add one.")}
        />{/each}
    </ul>
  </div>
{/if}
