<script>
  /// A member's addresses, other accounts on them, sign-in connections and the mail sent to them,
  /// with the bans, revocations and export an administrator can do from here.
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import { del, post, when } from "#lib/api.js";
  import UserLink from "#lib/UserLink.svelte";

  /** @type {{ id: number, ov: any, refresh: (msg: string) => any }} */
  let { id, ov, refresh } = $props();
  const status = new Status();
  let armed = $state("");

  /** Runs an action, then reloads the page's data; a dangerous one asks for a second click. */
  async function run(fn, done, key = "") {
    if (key && armed !== key) {
      armed = key;
      return;
    }
    armed = "";
    status.clear();
    try {
      await fn();
      await refresh(done);
    } catch (e) {
      status.fail(e);
    }
  }

  async function download() {
    status.clear();
    try {
      const data = await post(`/users/${id}/export`);
      const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }));
      const a = Object.assign(document.createElement("a"), { href: url, download: `member-${id}.json` });
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      status.fail(e);
    }
  }
  const agent = (ua) => (ua ?? "").slice(0, 60);
</script>

{#if status.error}<p class="err" role="alert">{status.error}</p>{/if}

{#if ov}
  <div class="acp-cols">
    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("IP addresses")}</h2></div>
      {#if ov.ips.length}
        <ul class="acp-rows">
          {#each ov.ips as row (row.ip)}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <code>{row.ip}</code>
                {#if row.banned}<span class="acp-pill is-danger">{tr("Banned")}</span>{/if}
                {#if row.others > 0}<span class="acp-pill is-warn">{tr("Other accounts")}: {row.others}</span>{/if}
                <br /><small class="muted">{tr("Last used")}: {when(row.last_used)}</small>
              </div>
              {#if !row.banned}
                <button
                  class="is-danger"
                  onclick={() =>
                    run(
                      () => post(`/users/${id}/ban`, { kind: "ip", value: row.ip }),
                      tr("Address banned"),
                      `ip-${row.ip}`,
                    )}>{armed === `ip-${row.ip}` ? tr("Click again") : tr("Ban")}</button
                >
              {/if}
            </li>
          {/each}
        </ul>
      {:else}
        <p class="acp-empty">{tr("None recorded.")}</p>
      {/if}
    </section>

    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Other accounts on the same addresses")}</h2></div>
      {#if ov.same_ip.length}
        <ul class="acp-rows">
          {#each ov.same_ip as o (`${o.id}-${o.ip}`)}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <UserLink name={o.username} /> · <a href="?id={o.id}">{tr("Details")}</a>
                <br /><small class="muted"><code>{o.ip}</code> · {when(o.last_used)}</small>
              </div>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="acp-empty">{tr("No other account used these addresses.")}</p>
      {/if}
    </section>
  </div>

  <div class="acp-cols">
    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Sign-in connections")}</h2></div>
      <ul class="acp-rows">
        {#each ov.connections.identities as c (c.provider)}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <strong>{c.provider}</strong>
              <br /><small class="muted">{c.email ?? ""} · {when(c.created_at)}</small>
            </div>
            <button
              class="secondary"
              onclick={() =>
                run(() => del(`/users/${id}/identities/${encodeURIComponent(c.provider)}`), tr("Connection removed"))}
              >{tr("Unlink")}</button
            >
          </li>
        {/each}
        {#each ov.connections.passkeys as k}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <strong>{tr("Passkey")}: {k.name}</strong>
              <br /><small class="muted"
                >{k.last_used_at ? `${tr("Last used")}: ${when(k.last_used_at)}` : tr("Never used")}</small
              >
            </div>
          </li>
        {/each}
        {#each ov.connections.api_keys as k (k.id)}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <strong>{k.oauth_client ? `${tr("App")}: ${k.oauth_client}` : `${tr("API key")}: ${k.name}`}</strong>
              <br /><small class="muted"
                >{k.scope} · {k.last_used_at ? `${tr("Last used")}: ${when(k.last_used_at)}` : tr("Never used")}</small
              >
            </div>
            <button class="secondary" onclick={() => run(() => del(`/users/${id}/api-keys/${k.id}`), tr("Key revoked"))}
              >{tr("Revoke")}</button
            >
          </li>
        {/each}
        {#if !ov.connections.identities.length && !ov.connections.passkeys.length && !ov.connections.api_keys.length}
          <li class="muted">{tr("No linked sign-ins, passkeys, keys or apps.")}</li>
        {/if}
      </ul>
    </section>

    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Signed-in devices")}</h2></div>
      {#if ov.connections.sessions.length}
        <ul class="acp-rows">
          {#each ov.connections.sessions as s}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <small class="acp-wrap">{agent(s.user_agent)}</small>
                <br /><small class="muted"
                  >{s.ip ?? "—"}{s.country ? ` · ${s.country}` : ""} · {when(s.last_used_at)}</small
                >
              </div>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="acp-empty">{tr("Not signed in anywhere.")}</p>
      {/if}
    </section>
  </div>

  <div class="acp-cols">
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Mail sent to them")}</h2>
        {#if ov.mail.bad}<span class="acp-pill is-danger">{tr("Address marked bad")}</span>
        {:else if ov.mail.bounces > 0}<span class="acp-pill is-warn">{tr("Bounces")}: {ov.mail.bounces}</span>{/if}
      </div>
      {#if ov.mail.recent.length}
        <ul class="acp-rows">
          {#each ov.mail.recent as m}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <span class="acp-wrap">{m.subject}</span>
                {#if m.last_error}<br /><small class="err acp-wrap">{m.last_error}</small>{/if}
              </div>
              <small class="muted">{m.sent_at ? when(m.sent_at) : tr("Waiting to be sent")}</small>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="acp-empty">{tr("No mail sent yet.")}</p>
      {/if}
    </section>

    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Data and bans")}</h2></div>
      <ul class="acp-rows">
        <li class="acp-action-row">
          <div class="acp-grow">
            <strong>{tr("Export their data")}</strong>
            <p>{tr("Everything the site holds about them, as a file. Logged.")}</p>
          </div>
          <button class="secondary" onclick={download}>{tr("Download")}</button>
        </li>
        <li class="acp-action-row">
          <div class="acp-grow">
            <strong>{tr("Ban their email address")}</strong>
            <p>{tr("Nobody can register with it again.")}</p>
          </div>
          <button
            class="is-danger"
            onclick={() => run(() => post(`/users/${id}/ban`, { kind: "email" }), tr("Email banned"), "email")}
            >{armed === "email" ? tr("Click again") : tr("Ban")}</button
          >
        </li>
      </ul>
    </section>
  </div>
{/if}
