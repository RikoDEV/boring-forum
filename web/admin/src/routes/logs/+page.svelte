<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import Empty from "#lib/Empty.svelte";
  import { get } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  let errors = $state([]);
  let tasks = $state([]);
  let failed = $state([]);
  const status = new Status();
  let loadedErrors = $state(false);
  let loadedTasks = $state(false);
  let loadedFailed = $state(false);
  get("/logs/errors")
    .then((r) => (errors = r), status.fail)
    .finally(() => (loadedErrors = true));
  get("/logs/tasks")
    .then((r) => (tasks = r), status.fail)
    .finally(() => (loadedTasks = true));
  get("/failed-logins")
    .then((r) => (failed = r), status.fail)
    .finally(() => (loadedFailed = true));
</script>

<PageHead title={tr("Logs")} {status} />
<h2>{tr("Server errors")}</h2>
<table>
  <thead>
    <tr><th>{tr("When")}</th><th>{tr("Request")}</th><th>{tr("Status")}</th><th>{tr("Request ID")}</th></tr>
  </thead>
  <tbody>
    {#each errors as e}<tr>
        <td><small><When iso={e.created_at} /></small></td><td>{e.method} {e.path}</td><td>{e.status}</td><td>
          <small>{e.request_id}</small>
        </td>
      </tr>{:else}<Empty cols={4} loaded={loadedErrors} title={tr("None.")} />{/each}
  </tbody>
</table>
<h2>{tr("Scheduled tasks")}</h2>
<table>
  <thead><tr><th>{tr("When")}</th><th>{tr("Task")}</th><th>{tr("Result")}</th></tr></thead>
  <tbody>
    {#each tasks as t}<tr>
        <td><small><When iso={t.created_at} /></small></td><td>{t.task}</td><td>
          <small>{JSON.stringify(t.detail)}</small>
        </td>
      </tr>{:else}<Empty cols={3} loaded={loadedTasks} title={tr("None yet.")} />{/each}
  </tbody>
</table>
<h2>{tr("Failed logins")}</h2>
<table>
  <thead><tr><th>{tr("When")}</th><th>{tr("Login")}</th><th>{tr("IP")}</th></tr></thead>
  <tbody>
    {#each failed as f}<tr>
        <td><small><When iso={f.created_at} /></small></td><td>
          {#if f.username}<UserLink name={f.username} />{:else}{f.login ?? ""}{/if}
        </td><td>{f.ip ?? ""}</td>
      </tr>{:else}<Empty cols={3} loaded={loadedFailed} title={tr("None.")} />{/each}
  </tbody>
</table>
