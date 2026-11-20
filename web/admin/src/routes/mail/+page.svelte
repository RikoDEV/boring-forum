<script>
  import { resolve } from "$app/paths";
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, when } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  import Field from "#lib/Field.svelte";
  let tab = $state("log");
  let rows = $state([]);
  const status = new Status();
  let bad = $state([]);
  const loadBad = () => get("/mail/bad").then((r) => (bad = r), status.fail);
  get("/mail").then((r) => (rows = r), status.fail);
  loadBad();
  let testTo = $state("");
  let note = $state("");
  const sendTest = (e) => {
    e.preventDefault();
    post("/mail/test", { to: testTo }).then(() => (note = tr("Queued for {testTo}", { testTo })), status.fail);
  };
  const clearBad = (u) => del(`/mail/bad/${u.id}`).then(loadBad, status.fail);
</script>

<PageHead
  title={tr("Mail")}
  intro={tr("Outgoing email, newest first. Failed messages are retried up to 5 times.")}
  {status}
  reserve
/>
<div class="acp-tabs" role="tablist">
  {#each [["log", tr("Log"), rows.length], ["templates", tr("Templates"), null], ["bad", tr("Bad addresses"), bad.length], ["test", tr("Test delivery"), null]] as [k, label, n]}<button
      role="tab"
      aria-selected={tab === k}
      onclick={() => (tab = String(k))}
    >
      {label}{#if n !== null}<span class="acp-count">{n}</span>{/if}
    </button>{/each}
</div>
{#if tab === "log"}
  <table>
    <thead><tr><th>{tr("Queued")}</th><th>{tr("To")}</th><th>{tr("Subject")}</th><th>{tr("Status")}</th></tr></thead>
    <tbody>
      {#each rows as m (m.id)}
        <tr>
          <td><small><When iso={m.created_at} /></small></td><td>{m.to_addr}</td><td>{m.subject}</td>
          <td>
            {#if m.sent_at}<span class="acp-pill is-ok" title={when(m.sent_at)}>
                {tr("Sent")}
              </span>{:else if m.attempts >= 5}<span class="acp-pill is-danger" title={m.last_error}>
                {tr("Failed")}
              </span>{:else if m.last_error}<span class="acp-pill is-warn" title={m.last_error}>
                {tr("Retrying ({attempts})", { attempts: m.attempts })}
              </span>{:else}<span class="acp-pill is-muted">{tr("Queued")}</span>{/if}
          </td>
        </tr>
      {:else}<tr><td colspan="4" class="muted">{tr("No mail yet.")}</td></tr>{/each}
    </tbody>
  </table>
{:else if tab === "templates"}
  <section class="acp-box">
    <div class="acp-box-head"><h2>{tr("Email texts")}</h2></div>
    <div class="acp-box-body">
      <p>
        {tr(
          "Each email's subject and body is a text in Site text, translated into every language there, like the rest of the site.",
        )}
      </p>
      <div class="acp-actions">
        <a class="btn" href="{resolve('/text')}?q=mail.">{tr("Edit email texts")}</a>
      </div>
    </div>
  </section>
{:else if tab === "bad"}
  <p class="muted">
    <small>{tr("No mail is sent to these after repeated bounces. Changing the address also clears this.")}</small>
  </p>
  <table>
    <thead><tr><th>{tr("Member")}</th><th>{tr("Address")}</th><th>{tr("Bounces")}</th><th></th></tr></thead>
    <tbody>
      {#each bad as u (u.id)}<tr>
          <td><UserLink name={u.username} /></td><td>{u.email}</td><td>{u.bounces}</td><td>
            <button onclick={() => clearBad(u)}>{tr("Clear")}</button>
          </td>
        </tr>{:else}<tr><td colspan="4" class="muted">{tr("No bad addresses.")}</td></tr>{/each}
    </tbody>
  </table>
{:else}
  <section class="acp-box">
    <form class="acp-form" onsubmit={sendTest}>
      <Field
        label={tr("Send a test email to")}
        type="email"
        bind:value={testTo}
        placeholder={tr("you@example.com")}
        required
      />
      <div class="acp-actions"><button>{tr("Send test")}</button></div>
    </form>
  </section>
  {#if note}<p class="ok" role="status">{note}</p>{/if}
{/if}
