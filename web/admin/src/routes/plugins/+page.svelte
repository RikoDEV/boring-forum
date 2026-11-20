<script>
  import { locale, tn, tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Box from "#lib/Box.svelte";
  import When from "#lib/When.svelte";
  import Section from "#lib/Section.svelte";
  import SchemaForm from "#lib/SchemaForm.svelte";
  import { resolve } from "$app/paths";
  import { fieldsFromSettings, localized, withDefaults } from "#lib/schema.js";
  import { del, get, post, put, upload as sendFile } from "#lib/api.js";
  let plugins = $state([]);
  const status = new Status();
  let installing = $state(false);
  let approving = $state(null);
  let file = $state(null);
  let settings = $state({});
  let groupList = $state([]);
  const base = resolve("/").replace(/\/$/, "");
  // The settings form of a plugin that declares typed settings (settings.json), in a dialog.
  let form = $state(null);
  const settingsFields = (p) => fieldsFromSettings(p.schema ?? {});
  function openSettings(p) {
    status.clear();
    const fields = settingsFields(p);
    form = { p, fields, values: withDefaults(fields, p.settings) };
  }
  function saveForm(e) {
    e.preventDefault();
    status.clear();
    put(`/plugins/${form.p.id}/settings`, { ...form.p.settings, ...form.values }).then(() => {
      form = null;
      load();
    }, status.fail);
  }
  get("/groups").then((r) => (groupList = r), () => {});
  const load = () =>
    get("/plugins").then((r) => {
      plugins = r;
      settings = Object.fromEntries(r.map((p) => [p.id, JSON.stringify(p.settings, null, 2)]));
    }, status.fail);

  /// What each capability lets a plugin do, in words an administrator can weigh.
  function describe(c) {
    if (c.startsWith("http:")) return tr("Its server code may contact {host}", { host: c.slice(5) });
    if (c.startsWith("connect:"))
      return tr("Its browser code may send requests to {origin}", { origin: c.slice(8) });
    const text = {
      kv: tr("Keep its own key-value data"),
      tables: tr("Create its own database tables"),
      api_route: tr("Answer requests under /api/p/"),
      "content.read": tr("Read public topics and posts"),
      "content.write": tr("Post topics and replies from its own account"),
      jobs: tr("Run background and recurring jobs"),
      notify: tr("Send notifications to members"),
      login: tr("Offer a sign-in button"),
      profile_fields: tr("Add questions to member profiles and the sign-up form"),
      groups: tr("Create member groups and put members in them"),
      permissions: tr("Define permissions you choose the groups for"),
      badges: tr("Create badges and give them to members"),
      notification_kinds: tr("Add its own kinds of notification, which members can switch on and off"),
      reactions: tr("Add reactions to posts"),
      flag_reasons: tr("Add reasons for reporting"),
      settings: tr("Add settings to the Settings page"),
      api_scopes: tr("Add scopes members can give their API keys"),
      tags: tr("Create tags and put them on topics"),
      nodes: tr("Create forums"),
      meta: tr("Keep extra data on topics and posts"),
      files: tr("Store files served from this site"),
      "users.read": tr("Read members' public profiles and what they may do"),
      "settings.read": tr("Read the site's public settings"),
      email: tr("Send email to members"),
      realtime: tr("Push live events to members' open pages"),
      client: tr(
        "Run its own JavaScript on every page of this site. That code is outside the sandbox and acts with the visitor's own session, so it can do anything a visitor can. Approve only plugins you trust fully.",
      ),
    };
    return text[c] ?? c;
  }
  async function togglePerm(p, perm, group, on) {
    const groups = on ? [...perm.groups, group] : perm.groups.filter((x) => x !== group);
    try {
      await put(`/plugins/${p.id}/permissions/${perm.key}`, { groups });
      perm.groups = groups;
    } catch (e) {
      status.fail(e);
      load();
    }
  }
  const risky = (c) => c === "client" || c.startsWith("connect:");

  async function upload(e) {
    e.preventDefault();
    status.clear();
    if (!file) return;
    const body = new FormData();
    body.append("file", file);
    try {
      done(await sendFile("/api/v1/admin/plugins/install", body));
    } catch (err) {
      status.error = err.message || tr("Install failed");
    }
  }
  function done(r) {
    installing = false;
    file = null;
    url = "";
    load();
    if (r?.new_capabilities?.length && r.upgraded_from)
      status.msg = tr("Updated from {version}. It needs your approval for {count} new capabilities.", {
        version: r.upgraded_from,
        count: r.new_capabilities.length,
      });
  }
  function saveSettings(p) {
    let value;
    try {
      value = JSON.parse(settings[p.id]);
    } catch {
      return (status.error = tr("Settings must be valid JSON."));
    }
    put(`/plugins/${p.id}/settings`, value).then(load, status.fail);
  }
  let url = $state("");
  const fromUrl = (e) => {
    e.preventDefault();
    status.clear();
    post("/plugins/install-url", { url }).then(done, status.fail);
  };
  const approve = (e) => {
    e.preventDefault();
    const p = approving;
    status.clear();
    post(`/plugins/${p.id}/enable`, { capabilities: p.capabilities }).then(() => {
      approving = null;
      load();
    }, status.fail);
  };
  const rollback = (p) =>
    confirm(tr("Go back to version {version} of {name}?", { version: p.prev_version, name: p.name })) &&
    post(`/plugins/${p.id}/rollback`).then(load, status.fail);
  load();
</script>

<PageHead title={tr("Plugins")} status={installing || approving ? null : status}>
  {#snippet intro()}{tr(
      "Plugins run sandboxed and can do only what you approve. Packages are not signed yet, so install only ones you trust. If one breaks the site, open",
    )} <code>/safe-mode</code>.{/snippet}
  <button
    onclick={() => {
      status.clear();
      installing = true;
    }}>{tr("Install a plugin")}</button
  >
</PageHead>
{#if installing}
  <!-- Two ways in, each with its own button, so the dialog has no single submit. -->
  <EditDialog title={tr("Install a plugin")} onclose={() => (installing = false)} onsubmit={upload} {status} save={tr("Install zip")}>
    <div class="acp-form">
      <label class="acp-wide"
        >{tr("Plugin package")}<input type="file" accept=".zip" onchange={(e) => (file = e.currentTarget.files[0])} /></label
      >
      <Field
        label={tr("Or an archive address: https://host/owner/repo/archive/v1.zip")}
        type="url"
        bind:value={url}
        wide
      />
      <div class="acp-wide">
        <button type="button" class="secondary" disabled={!url} onclick={fromUrl}>{tr("Install from address")}</button>
      </div>
    </div>
  </EditDialog>
{/if}
{#if approving}
  <EditDialog
    title={tr("Approve {name}", { name: approving.name })}
    onclose={() => (approving = null)}
    onsubmit={approve}
    {status}
    save={tr("Approve and enable")}
  >
    {#if approving.new_capabilities.length && approving.prev_version}
      <p class="acp-box-body">{tr("This version asks for more than you approved before. The new ones are marked.")}</p>
    {:else}
      <p class="acp-box-body">{tr("This plugin asks to:")}</p>
    {/if}
    <ul class="acp-rows">
      {#each approving.capabilities as c}
        <li class="acp-rowitem">
          <div class="acp-grow">
            {#if approving.new_capabilities.includes(c) && approving.prev_version}<span class="acp-pill is-warn"
                >{tr("New")}</span
              >{/if}
            {#if risky(c)}<span class="acp-pill is-danger">{tr("Outside the sandbox")}</span>{/if}
            {describe(c)}
          </div>
        </li>
      {:else}
        <li class="acp-rowitem">{tr("Nothing: it can only react to events.")}</li>
      {/each}
    </ul>
  </EditDialog>
{/if}
{#if form}
  <EditDialog
    title={tr("Settings of {name}", { name: form.p.name })}
    onclose={() => (form = null)}
    onsubmit={saveForm}
    {status}
  >
    <SchemaForm fields={form.fields} bind:values={form.values} locales={form.p.schema?.["x-locales"] ?? {}} />
  </EditDialog>
{/if}
{#each plugins as p (p.id)}
  <Box title={p.name}>
    {#snippet actions()}
      {#if p.enabled}<span class="acp-pill is-ok">{tr("On")}</span>
      {:else if p.new_capabilities.length && p.prev_version}<span class="acp-pill is-warn">{tr("Needs approval")}</span>
      {:else}<span class="acp-pill is-muted">{tr("Off")}</span>{/if}
    {/snippet}
    <div class="acp-box-body">
      <small class="muted"
        >{p.version} · {tr("Checksum {checksum}… · hooks: {hooks}", {
          checksum: p.checksum.slice(0, 16),
          hooks: p.hooks.join(", ") || tr("none"),
        })}{#if p.failures}
          · {tn("admin.count.failures", p.failures)}{/if}</small
      >
      {#if p.requires.length}
        <p>
          {tr("Needs: {plugins}", { plugins: p.requires.join(", ") })}
          {#if p.requires_unmet.length}<span class="acp-pill is-danger"
              >{tr("Missing: {plugins}", { plugins: p.requires_unmet.join(", ") })}</span
            >{/if}
        </p>
      {/if}
      {#each (p.schema?.["x-panels"] ?? []).filter((x) => x.area === "admin") as x}<p>
          <a href="/plugin/{p.id}/{x.key}">{x.label}</a>
        </p>{/each}
      {#if p.stats.calls || p.stats.errors}
        <p>
          <small class="muted"
            >{tr("{calls} calls, {errors} errors, about {fuel} fuel per call", {
              calls: p.stats.calls.toLocaleString(locale()),
              errors: p.stats.errors.toLocaleString(locale()),
              fuel: p.stats.avg_fuel.toLocaleString(locale()),
            })}</small
          >
          {#if p.stats.last_error}<br /><small class="err"
              >{tr("Last error")}: <When iso={p.stats.last_error.at} /> {p.stats.last_error.message}</small
            >{/if}
        </p>
      {/if}
    </div>
    <ul class="acp-rows">
      {#each p.capabilities as c}
        <li class="acp-rowitem">
          <div class="acp-grow">
            {#if p.new_capabilities.includes(c) && p.prev_version}<span class="acp-pill is-warn">{tr("New")}</span>{/if}
            {#if risky(c)}<span class="acp-pill is-danger">{tr("Outside the sandbox")}</span>{/if}
            {describe(c)}
          </div>
        </li>
      {:else}
        <li class="acp-rowitem muted">{tr("Asks for nothing.")}</li>
      {/each}
    </ul>
    {#if p.permissions?.length}
      <div class="acp-box-body">
        {#each p.permissions as perm (perm.key)}
          <Section legend={perm.label} hint={tr("Administrators always hold it.")}>
            <div class="acp-switches">
              {#each groupList.filter((g) => g.id > 1 && !g.is_admin) as g (g.id)}
                <label class="acp-group-style"
                  ><input
                    type="checkbox"
                    class="acp-switch"
                    role="switch"
                    checked={perm.groups.includes(g.id)}
                    onchange={(e) => togglePerm(p, perm, g.id, e.currentTarget.checked)}
                  />
                  {g.name}</label
                >
              {/each}
            </div>
          </Section>
        {/each}
      </div>
    {/if}
    <div class="acp-box-body acp-actions">
      {#if p.enabled}<button class="secondary" onclick={() => post(`/plugins/${p.id}/disable`).then(load, status.fail)}
          >{tr("Disable")}</button
        >
      {:else}<button
          onclick={() => {
            status.clear();
            approving = p;
          }}>{tr("Review and enable")}</button
        >{/if}
      {#if p.prev_version}<button class="secondary" onclick={() => rollback(p)}
          >{tr("Roll back to {version}", { version: p.prev_version })}</button
        >{/if}
      <button
        class="secondary is-danger"
        onclick={() =>
          confirm(tr("Uninstall {name} and delete its data?", { name: p.name })) &&
          del(`/plugins/${p.id}`).then(load, status.fail)}>{tr("Uninstall")}</button
      >
    </div>
    {#if settingsFields(p).length || (p.schema?.["x-admin-pages"] ?? []).length}
      <div class="acp-box-body acp-actions">
        {#if settingsFields(p).length}<button class="secondary" onclick={() => openSettings(p)}>{tr("Settings")}</button>{/if}
        {#each p.schema?.["x-admin-pages"] ?? [] as pg (pg.key)}
          <a class="btn secondary" href="{base}/plugins/form?id={p.id}&key={pg.key}"
            >{localized(p.schema?.["x-locales"], pg.label_key, pg.label)}</a
          >
        {/each}
      </div>
    {/if}
    {#if !settingsFields(p).length}
      <details class="acp-box-body bf-disclosure">
        <summary>{tr("Settings (JSON)")}</summary>
        <div class="acp-form">
          <Field label={tr("Settings (JSON)")} rows="4" bind:value={settings[p.id]} wide />
          <div class="acp-actions acp-wide"><button onclick={() => saveSettings(p)}>{tr("Save settings")}</button></div>
        </div>
      </details>
    {/if}
    {#if p.stats.log.length}
      <details class="acp-box-body bf-disclosure">
        <summary>{tr("Log ({count} lines)", { count: p.stats.log.length })}</summary>
        <ul class="acp-rows">
          {#each p.stats.log as line}
            <li class="acp-rowitem"><small class="muted"><When iso={line.at} /></small> <code>{line.message}</code></li>
          {/each}
        </ul>
      </details>
    {/if}
  </Box>
{:else}
  <p class="muted">{tr("None installed.")}</p>
{/each}
