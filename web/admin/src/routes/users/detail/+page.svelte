<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import Notices from "#lib/Notices.svelte";
  import { trailOf } from "#lib/crumbs.svelte.js";
  import { page } from "$app/state";
  import { del, get, patch, post } from "#lib/api.js";
  import Switch from "#lib/Switch.svelte";
  import { iconHref } from "#lib/icons.js";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Activity from "#lib/member/Activity.svelte";
  import Moderation from "#lib/member/Moderation.svelte";
  import Security from "#lib/member/Security.svelte";
  import When from "#lib/When.svelte";

  const id = $derived(Number(page.url.searchParams.get("id")));
  let u = $state(null);
  let groups = $state([]);
  let ov = $state(null); // activity, warnings, notes, addresses, connections and mail
  let tab = $state("overview");
  let dialog = $state(null); // "identity" | "silence" | "suspend" | "clear"
  const dlg = new Status();
  let ident = $state({ username: "", email: "", display_name: "" });
  let restrict = $state({ days: 7, reason: "" });
  let clearing = $state({});
  let trust = $state(null);
  let trustLevels = $state([{ level: 0, label: "" }]);
  let history = $state([]);
  const status = new Status();
  let title = $state("");
  let mergeInto = $state("");
  let danger = $state("");

  const active = (iso) => iso && new Date(iso) > new Date();
  const days = (n) => new Date(Date.now() + n * 86400e3).toISOString();
  const rows = (v) => (Array.isArray(v) ? v : []);
  const show = (v) => (typeof v === "object" && v !== null ? JSON.stringify(v) : String(v ?? ""));

  async function load() {
    try {
      const [list, g] = await Promise.all([get(`/users?id=${id}`), get("/groups")]);
      u = list[0] ?? null;
      groups = g;
      if (!u) return (status.error = tr("That member does not exist."));
      title = u.title ?? "";
      get("/trust").then(
        (r) => (trustLevels = r.levels),
        () => {},
      );
      get(`/users/${id}/trust`).then(
        (r) => (trust = r),
        () => {},
      );
      get(`/users/${id}/overview`).then(
        (r) => (ov = r),
        () => {},
      );
      get(`/users/${id}/history`).then(
        (r) => (history = rows(r)),
        () => {},
      );
    } catch (e) {
      status.fail(e);
    }
  }
  async function act(fn, done) {
    status.clear();
    danger = "";
    try {
      await fn();
      status.msg = done;
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  const change = (body, done) => act(() => patch(`/users/${id}`, body), done);
  /** For the tabs' components: say what was done, and reload. */
  const refresh = async (msg) => {
    status.msg = msg;
    await load();
  };
  /** Runs a dialog's request; the dialog closes when it worked, and shows the error when not. */
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
  function openDialog(which) {
    dlg.clear();
    if (which === "identity") ident = { username: u.username, email: u.email, display_name: u.display_name ?? "" };
    if (which === "silence") restrict = { days: 7, reason: "" };
    if (which === "suspend") restrict = { days: 30, reason: "" };
    if (which === "clear") clearing = {};
    dialog = which;
  }
  const CLEARABLE = [
    ["avatar", tr("Profile picture")],
    ["cover", tr("Cover picture")],
    ["about", tr("About text")],
    ["signature", tr("Signature")],
    ["location", tr("Location")],
    ["website", tr("Website")],
    ["title", tr("Title")],
  ];
  // Dangerous actions ask with a second click rather than a browser dialog.
  const arm = (key, fn) => {
    if (danger === key) fn();
    else danger = key;
  };
  // The member's other groups, as the picker shows them; a change is saved at once.
  let picked = $state([]);
  const sameIds = (a, b) => a.length === b.length && [...a].sort().join() === [...b].sort().join();
  $effect(() => {
    if (u) picked = [...u.secondary_groups];
  });
  $effect(() => {
    const next = [...picked];
    // Whatever happens (saved, refused, password prompt cancelled), the picker shows what is stored.
    if (u && !sameIds(next, u.secondary_groups))
      change({ secondary_groups: next }, tr("Groups changed")).finally(() => (picked = [...u.secondary_groups]));
  });
  $effect(() => trailOf(u ? [{ label: u.username }] : []));
  $effect(() => {
    if (id) load();
  });
</script>

<Notices {status} />
{#if u}
  <section class="acp-box acp-profile">
    <img class="acp-avatar" src="/avatar/{encodeURIComponent(u.username)}.svg" width="64" height="64" alt="" />
    <div>
      <h1>{u.username}</h1>
      <div class="muted"><small>{u.email}{u.title ? ` · ${u.title}` : ""}</small></div>
      <div class="acp-chips">
        {#if !u.approved}<span class="acp-pill is-warn">{tr("Awaiting approval")}</span>{/if}
        {#if active(u.suspended_until)}
          {@const [before, after] = tr("Suspended until {n}", { n: "" }).split("")}
          <span class="acp-pill is-danger">{before}<When iso={u.suspended_until} />{after}</span>
        {/if}
        {#if active(u.silenced_until)}
          {@const [before, after] = tr("Silenced until {n}", { n: "" }).split("")}
          <span class="acp-pill is-danger">{before}<When iso={u.silenced_until} />{after}</span>
        {/if}
        {#if !u.email_verified}<span class="acp-pill is-muted">{tr("Email not verified")}</span>{/if}
        {#if u.totp_enabled}<span class="acp-pill is-ok">{tr("Two-factor on")}</span>{/if}
      </div>
    </div>
    <div class="acp-actions acp-profile-actions">
      <button class="secondary" aria-label={tr("Edit name and email")} onclick={() => openDialog("identity")}>
        <svg class="bf-icon" aria-hidden="true"><use href={iconHref("pencil")}></use></svg><span class="acp-lbl">
          {tr("Edit")}
        </span>
      </button>
      <a class="acp-linkbtn" href="/u/{u.username}" aria-label={tr("View profile")}>
        <svg class="bf-icon" aria-hidden="true"><use href={iconHref("user")}></use></svg><span class="acp-lbl">
          {tr("View profile")}
        </span>
      </a>
      <a
        class="acp-linkbtn"
        href="/messages?to={encodeURIComponent(u.username)}#new-message"
        aria-label={tr("Message")}
      >
        <svg class="bf-icon" aria-hidden="true"><use href={iconHref("mail")}></use></svg><span class="acp-lbl">
          {tr("Message")}
        </span>
      </a>
      <a class="acp-linkbtn" href="/u/{u.id}/warnings" aria-label={tr("Warnings")}>
        <svg class="bf-icon" aria-hidden="true"><use href={iconHref("flag")}></use></svg><span class="acp-lbl">
          {tr("Warnings")}
        </span>
      </a>
      <button
        class="secondary"
        aria-label={tr("View as")}
        onclick={() => post(`/users/${id}/impersonate`).then(() => (location.href = "/"), status.fail)}
      >
        <svg class="bf-icon" aria-hidden="true"><use href={iconHref("eye")}></use></svg><span class="acp-lbl">
          {tr("View as")}
        </span>
      </button>
    </div>
  </section>

  <div class="acp-tiles">
    {#each [[tr("Topics"), u.topic_count], [tr("Posts"), u.post_count], [tr("Reputation"), u.reputation], [tr("Trust level"), u.trust_level], [tr("Warning points"), u.warning_points]] as [label, n]}
      <div class="acp-tile" style:grid-template-columns="1fr"><b>{n}</b><span>{label}</span></div>
    {/each}
  </div>

  <div class="acp-tabs" role="tablist">
    {#each [["overview", tr("Overview")], ["activity", tr("Activity")], ["moderation", tr("Moderation")], ["security", tr("Security")]] as [key, label]}
      <button role="tab" aria-selected={tab === key} onclick={() => (tab = key)}>{label}</button>
    {/each}
  </div>

  {#if tab === "activity"}
    <Activity {ov} />
  {:else if tab === "moderation"}
    <Moderation {id} {ov} {refresh} />
  {:else if tab === "security"}
    <Security {id} {ov} {refresh} />
  {:else}
    {#snippet need(r)}
      <li class:is-met={r.current >= r.needed}>
        <span class="acp-trust-label">{r.label}</span>
        <b>{Math.min(r.current, r.needed)} / {r.needed}</b>
        <progress value={r.current} max={r.needed}></progress>
      </li>
    {/snippet}
    {#if trust}
      <section class="acp-box">
        <div class="acp-box-head">
          <h2>{tr("Trust level")}</h2>
          <span class="muted">
            {trust.level} · {trust.name}{#if trust.next}&#32;→ {trust.next.level} · {trust.next.name}{/if}
          </span>
        </div>
        <div class="acp-box-body acp-trust">
          {#if trust.locked}
            <p>{tr("Set by hand, so automatic promotion and demotion skip this member")}</p>
          {:else if trust.paused}
            <p>{tr("Not promoted while suspended or silenced.")}</p>
          {/if}
          {#if trust.next}
            {#if trust.next.requirements.length}
              <ul class="acp-trust-list">
                {#each trust.next.requirements as r}{@render need(r)}{/each}
              </ul>
            {:else}
              <p>{tr("No requirements; moves up at the next check.")}</p>
            {/if}
          {/if}
          {#if trust.keep}
            <p class="muted">{tr("Keeping level {n}", { n: trust.level })}</p>
            <ul class="acp-trust-list">
              {@render need(trust.keep)}
            </ul>
          {/if}
          {#if !trust.next && !trust.keep}
            <p class="muted">{tr("Nothing more to earn or keep at this level.")}</p>
          {/if}
        </div>
      </section>
    {/if}

    <div class="acp-cols">
      <div>
        <section class="acp-box">
          <div class="acp-box-head"><h2>{tr("Moderation")}</h2></div>
          <ul class="acp-rows">
            {#if !u.approved}<li class="acp-action-row">
                <div class="acp-grow">
                  <strong>{tr("Approval")}</strong>
                  <p>{tr("This account is waiting to be let in.")}</p>
                </div>
                <button onclick={() => change({ approved: true }, tr("Approved"))}>{tr("Approve")}</button>
              </li>{/if}
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Silence")}</strong>
                <p>{tr("Can read, but cannot post or message.")}</p>
              </div>
              {#if active(u.silenced_until)}<button
                  class="secondary"
                  onclick={() => change({ silenced_until: null }, tr("Silence lifted"))}
                >
                  {tr("Lift silence")}
                </button>
              {:else}<button class="secondary" onclick={() => openDialog("silence")}>{tr("Silence")}</button>{/if}
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Suspend")}</strong>
                <p>{tr("Signed out and cannot log in.")}</p>
              </div>
              {#if active(u.suspended_until)}<button
                  class="secondary"
                  onclick={() => change({ suspended_until: null }, tr("Suspension lifted"))}
                >
                  {tr("Lift suspension")}
                </button>
              {:else}<button class="secondary" onclick={() => openDialog("suspend")}>{tr("Suspend")}</button>{/if}
            </li>
          </ul>
        </section>

        <section class="acp-box">
          <div class="acp-box-head"><h2>{tr("Permissions")}</h2></div>
          <ul class="acp-rows">
            <li class="acp-action-row">
              <div class="acp-grow"><strong>{tr("Primary group")}</strong></div>
              <select
                aria-label={tr("Primary group")}
                value={u.primary_group_id}
                onchange={(e) => change({ primary_group_id: Number(e.currentTarget.value) }, tr("Group changed"))}
              >
                {#each groups.filter((g) => g.id !== 1) as g}<option value={g.id}>{g.name}</option>{/each}
              </select>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow"><strong>{tr("Trust level")}</strong></div>
              <select
                aria-label={tr("Trust level")}
                value={u.trust_level}
                onchange={(e) => change({ trust_level: Number(e.currentTarget.value) }, tr("Trust level changed"))}
              >
                {#each trustLevels as l}<option value={l.level}>
                    {l.level}{l.label ? ` · ${l.label}` : ""}
                  </option>{/each}
              </select>
              <Switch
                check
                label={tr("Keep this level")}
                checked={u.trust_level_locked}
                onchange={(e) =>
                  change(
                    { trust_level_locked: e.currentTarget.checked },
                    e.currentTarget.checked ? tr("Trust level locked") : tr("Trust level unlocked"),
                  )}
              />
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Other groups")}</strong>
                <div class="acp-pick">
                  <MultiSelect
                    label={tr("Other groups")}
                    hideLabel
                    placeholder={tr("Add to a group")}
                    options={groups
                      .filter((g) => g.id > 2 && g.id !== u.primary_group_id)
                      .map((g) => ({ value: g.id, label: g.name }))}
                    bind:value={picked}
                  />
                </div>
              </div>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Title")}</strong>
                <p>{tr("Shown under their name.")}</p>
              </div>
              <input bind:value={title} aria-label={tr("Title")} /><button
                class="secondary"
                onclick={() => change({ title: title.trim() || null }, tr("Title saved"))}
              >
                {tr("Save")}
              </button>
            </li>
          </ul>
        </section>
      </div>

      <div>
        <section class="acp-box">
          <div class="acp-box-head"><h2>{tr("Account")}</h2></div>
          <dl class="acp-dl">
            <dt>{tr("Joined")}</dt>
            <dd><When iso={u.joined_at} /></dd>
            <dt>{tr("Last seen")}</dt>
            <dd><When iso={u.last_seen_at} /></dd>
            <dt>{tr("Member id")}</dt>
            <dd>{u.id}</dd>
          </dl>
          <ul class="acp-rows">
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Email")}</strong>
                <p>{u.email_verified ? tr("Verified") : tr("Not verified")}</p>
              </div>
              <button
                class="secondary"
                onclick={() => change({ email_verified: !u.email_verified }, tr("Email status changed"))}
              >
                {u.email_verified ? tr("Mark unverified") : tr("Mark verified")}
              </button>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Password")}</strong>
                <p>{tr("Mails them a link to choose a new one.")}</p>
              </div>
              <button
                class="secondary"
                onclick={() => act(() => post(`/users/${id}/send-password-reset`), tr("Reset link sent"))}
              >
                {tr("Send reset link")}
              </button>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Profile")}</strong>
                <p>{tr("Remove a picture or text they should not have.")}</p>
              </div>
              <button class="secondary" onclick={() => openDialog("clear")}>{tr("Clear fields")}</button>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Sessions")}</strong>
                <p>{tr("Sign them out on every device.")}</p>
              </div>
              <button
                class="secondary"
                onclick={() => act(() => post(`/users/${id}/logout`), tr("Signed out everywhere"))}
              >
                {tr("Log out")}
              </button>
            </li>
            {#if u.totp_enabled}<li class="acp-action-row">
                <div class="acp-grow">
                  <strong>{tr("Two-factor")}</strong>
                  <p>{tr("For someone who lost their device.")}</p>
                </div>
                <button
                  class="secondary"
                  onclick={() => act(() => post(`/users/${id}/reset-2fa`), tr("Two-factor reset"))}
                >
                  {tr("Reset")}
                </button>
              </li>{/if}
          </ul>
        </section>

        <section class="acp-box">
          <div class="acp-box-head"><h2>{tr("Danger zone")}</h2></div>
          <ul class="acp-rows">
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Merge into another account")}</strong>
                <p>{tr("Moves their content there and removes this account.")}</p>
              </div>
              <input
                type="number"
                min="1"
                bind:value={mergeInto}
                placeholder={tr("Member id")}
                aria-label={tr("Merge into member id")}
                style:width="7rem"
              /><button
                class="is-danger"
                disabled={!mergeInto}
                onclick={() =>
                  arm("merge", () => act(() => post(`/users/${id}/merge`, { into: Number(mergeInto) }), tr("Merged")))}
              >
                {danger === "merge" ? tr("Click again") : tr("Merge")}
              </button>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Delete as spammer")}</strong>
                <p>{tr("Removes the account and all its posts.")}</p>
              </div>
              <button
                class="is-danger"
                onclick={() =>
                  arm("spam", () => act(() => post(`/users/${id}/delete-spammer`), tr("Deleted as a spammer")))}
              >
                {danger === "spam" ? tr("Click again") : tr("Delete spammer")}
              </button>
            </li>
            <li class="acp-action-row">
              <div class="acp-grow">
                <strong>{tr("Delete account")}</strong>
                <p>{tr("Posts stay, without their name.")}</p>
              </div>
              <button class="is-danger" onclick={() => arm("del", () => act(() => del(`/users/${id}`), tr("Deleted")))}>
                {danger === "del" ? tr("Click again") : tr("Delete")}
              </button>
            </li>
          </ul>
        </section>
      </div>
    </div>

    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("History")}</h2></div>
      {#if history.length}<ul class="acp-rows">
          {#each history as row}<li>
              <small class="acp-wrap">{Object.values(row).map(show).join(" · ")}</small>
            </li>{/each}
        </ul>{:else}<p class="acp-empty">{tr("Nothing yet.")}</p>{/if}
    </section>
  {/if}

  {#if dialog === "identity"}
    <EditDialog
      title={tr("Edit name and email")}
      status={dlg}
      onclose={() => (dialog = null)}
      onsubmit={(e) =>
        submit(
          e,
          () =>
            patch(`/users/${id}/identity`, {
              username: ident.username,
              email: ident.email,
              display_name: ident.display_name.trim() || null,
            }),
          tr("Saved"),
        )}
    >
      <div class="acp-form">
        <Field label={tr("Username")} bind:value={ident.username} required wide />
        <Field label={tr("Display name")} bind:value={ident.display_name} maxlength="60" wide />
        <Field label={tr("Email")} type="email" bind:value={ident.email} required wide />
      </div>
    </EditDialog>
  {:else if dialog === "silence" || dialog === "suspend"}
    <EditDialog
      title={dialog === "silence" ? tr("Silence this member") : tr("Suspend this member")}
      status={dlg}
      save={dialog === "silence" ? tr("Silence") : tr("Suspend")}
      onclose={() => (dialog = null)}
      onsubmit={(e) =>
        submit(
          e,
          () =>
            patch(`/users/${id}`, {
              [dialog === "silence" ? "silenced_until" : "suspended_until"]: days(Number(restrict.days)),
              reason: restrict.reason,
            }),
          dialog === "silence" ? tr("Silenced") : tr("Suspended"),
        )}
    >
      <div class="acp-form">
        <Field label={tr("Days")} type="number" min="1" max="365" bind:value={restrict.days} required />
        <Field
          label={tr("Reason the member sees")}
          bind:value={restrict.reason}
          rows="4"
          maxlength="500"
          required
          wide
        />
      </div>
    </EditDialog>
  {:else if dialog === "clear"}
    <EditDialog
      title={tr("Clear profile fields")}
      status={dlg}
      save={tr("Clear")}
      onclose={() => (dialog = null)}
      onsubmit={(e) =>
        submit(
          e,
          () => post(`/users/${id}/clear-profile`, { what: CLEARABLE.map((c) => c[0]).filter((k) => clearing[k]) }),
          tr("Profile fields cleared"),
        )}
    >
      <div class="acp-switches">
        {#each CLEARABLE as [key, label] (key)}<Switch {label} bind:checked={clearing[key]} />{/each}
      </div>
    </EditDialog>
  {/if}
{/if}
