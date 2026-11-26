<script>
  import { followTabHash } from "#lib/tabs.svelte.js";
  import { resolve } from "$app/paths";
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import PageSettings from "#lib/PageSettings.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, when } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  import Field from "#lib/Field.svelte";
  import Empty from "#lib/Empty.svelte";
  import MailPreview from "#lib/MailPreview.svelte";
  let tab = $state("log");
  followTabHash(["log", "templates", "preview", "bad", "test", "settings"], (key) => (tab = key));
  let rows = $state([]);
  const status = new Status();
  let bad = $state([]);
  let loadedLog = $state(false);
  let loadedBad = $state(false);
  const loadBad = () =>
    get("/mail/bad")
      .then((r) => (bad = r), status.fail)
      .finally(() => (loadedBad = true));
  get("/mail")
    .then((r) => (rows = r), status.fail)
    .finally(() => (loadedLog = true));
  // Whether the server can send at all. A misconfigured sender leaves every message "Queued", and
  // one with no transport marks them "Sent" without delivering them: say so above the log.
  let sender = $state({ kind: "", detail: "" });
  get("/mail/status").then((r) => (sender = r), status.fail);
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
  intro={tr("Outgoing email, newest first. Failed messages are tried again for about two days.")}
  {status}
  reserve
/>
<div class="acp-tabs" role="tablist">
  {#each [["log", tr("Log"), rows.length], ["templates", tr("Templates"), null], ["preview", tr("Preview"), null], ["bad", tr("Bad addresses"), bad.length], ["test", tr("Test delivery"), null], ["settings", tr("Settings"), null]] as [k, label, n]}<button
      role="tab"
      aria-selected={tab === k}
      onclick={() => (tab = String(k))}
    >
      {label}{#if n !== null}<span class="acp-count">{n}</span>{/if}
    </button>{/each}
</div>
{#if sender.kind === "error"}
  <p class="err" role="alert">
    {tr("Mail is not being sent: {reason}. Messages stay queued until this is fixed.", { reason: sender.detail })}
  </p>
{:else if sender.kind === "log"}
  <p class="err" role="alert">
    {tr("No mail sender is configured (BF_SMTP_URL), so messages show as sent but are never delivered.")}
  </p>
{:else if sender.kind === "dir"}
  <p class="muted">{tr("Mail is written to files in {dir}, not delivered.", { dir: sender.detail })}</p>
{/if}
{#if tab === "log"}
  <table class="acp-mail-log">
    <thead><tr><th>{tr("Queued")}</th><th>{tr("To")}</th><th>{tr("Subject")}</th><th>{tr("Status")}</th></tr></thead>
    <tbody>
      {#each rows as m (m.id)}
        <tr>
          <td><small><When iso={m.created_at} /></small></td><td>{m.to_addr}</td><td>{m.subject}</td>
          <td>
            {#if m.sent_at}<span class="acp-pill is-ok" title={when(m.sent_at)}>
                {tr("Sent")}
              </span>{:else if m.attempts >= 9}<span class="acp-pill is-danger">{tr("Failed")}</span>
            {:else if m.last_error}<span class="acp-pill is-warn">
                {tr("Retrying ({attempts})", { attempts: m.attempts })}
              </span>{:else}<span class="acp-pill is-muted">{tr("Queued")}</span>{/if}
            {#if !m.sent_at && m.last_error}<small class="muted acp-mail-err">{m.last_error}</small>{/if}
          </td>
        </tr>
      {:else}<Empty
          cols={4}
          loaded={loadedLog}
          title={tr("No mail yet.")}
          text={tr("Messages the forum sends are listed here.")}
        />{/each}
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
{:else if tab === "preview"}
  <MailPreview {status} />
{:else if tab === "bad"}
  <p class="muted">
    <small>{tr("No mail is sent to these after repeated bounces. Changing the address also clears this.")}</small>
  </p>
  <table>
    <thead>
      <tr><th>{tr("Member")}</th><th>{tr("Address")}</th><th>{tr("Bounces")}</th><th class="acp-cell-end"></th></tr>
    </thead>
    <tbody>
      {#each bad as u (u.id)}<tr>
          <td><UserLink name={u.username} /></td><td>{u.email}</td><td>{u.bounces}</td><td class="acp-cell-end">
            <button class="secondary" onclick={() => clearBad(u)}>{tr("Clear")}</button>
          </td>
        </tr>{:else}<Empty
          cols={4}
          loaded={loadedBad}
          title={tr("No bad addresses.")}
          text={tr("Addresses that keep bouncing are listed here.")}
        />{/each}
    </tbody>
  </table>
{:else if tab === "settings"}
  <!-- drawn below, where it stays mounted -->
{:else}
  <section class="acp-box">
    <div class="acp-box-head"><h2>{tr("Test delivery")}</h2></div>
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

<PageSettings path="/mail" hidden={tab !== "settings"} onshow={() => (tab = "settings")} />
