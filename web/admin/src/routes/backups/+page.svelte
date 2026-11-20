<script>
  import { onDestroy } from "svelte";
  import { tr, locale } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import Section from "#lib/Section.svelte";
  import Switch from "#lib/Switch.svelte";
  import Field from "#lib/Field.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import IconButton from "#lib/IconButton.svelte";
  import When from "#lib/When.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { get, post, put, del } from "#lib/api.js";

  const status = new Status();
  const dialog = new Status();

  let backups = $state([]);
  let activity = $state({ running: null, phase: "", last_error: null });
  let directory = $state("");
  let loaded = $state(false);
  /** The settings this page edits, as the server holds them. */
  const cfg = $state({ backup_enabled: false, backup_interval_hours: 24, backup_keep: 7, backup_uploads: true });
  /** The backup the restore dialog is open for, and what was typed to confirm it. */
  let restoring = $state(null);
  let confirmText = $state("");
  let timer = 0;
  let wasRunning = "";

  const running = $derived(Boolean(activity.running));
  const KIND = { backup: tr("Backing up"), restore: tr("Restoring") };
  const PHASE = {
    database: tr("reading the database"),
    packing: tr("packing the archive"),
    unpacking: tr("unpacking the archive"),
    loading: tr("loading the database"),
    uploads: tr("restoring uploaded files"),
  };

  function size(n) {
    const f = (v) => new Intl.NumberFormat(locale(), { maximumFractionDigits: 1 }).format(v);
    if (n >= 1024 ** 3) return tr("{n} GB", { n: f(n / 1024 ** 3) });
    if (n >= 1024 ** 2) return tr("{n} MB", { n: f(n / 1024 ** 2) });
    return tr("{n} KB", { n: f(Math.max(n, 1) / 1024) });
  }

  async function load() {
    try {
      const r = await get("/backups");
      backups = r.backups;
      directory = r.directory;
      const was = wasRunning;
      activity = r.activity;
      wasRunning = r.activity.running ?? "";
      if (was === "restore" && !wasRunning) finishedRestore(r.activity);
    } catch (e) {
      // A restore replaces the sessions too, so the request that follows it can be refused.
      if (wasRunning === "restore") finishedRestore(null);
      else status.fail(e);
    }
    loaded = true;
    schedule();
  }

  function finishedRestore(a) {
    wasRunning = "";
    if (a?.last_error) {
      status.error = a.last_error;
    } else {
      status.msg = tr("The restore finished. Sign in again.");
      window.setTimeout(() => window.location.assign("/login"), 2500);
    }
  }

  function schedule() {
    clearTimeout(timer);
    if (running || wasRunning) timer = window.setTimeout(load, 2000);
  }
  onDestroy(() => clearTimeout(timer));

  get("/settings").then(
    (all) => {
      for (const s of all) if (s.key in cfg) cfg[s.key] = s.value;
    },
    () => {},
  );
  load();

  async function setting(key, value) {
    status.clear();
    try {
      await put(`/settings/${key}`, { value });
      cfg[key] = value;
      status.msg = tr("Saved.");
    } catch (e) {
      status.fail(e);
      get("/settings").then(
        (all) => all.forEach((s) => s.key in cfg && (cfg[s.key] = s.value)),
        () => {},
      );
    }
  }

  async function backUp() {
    status.clear();
    try {
      await post("/backups", { skip_uploads: false });
      await load();
    } catch (e) {
      status.fail(e);
    }
  }

  async function download(b) {
    status.clear();
    try {
      // The file itself comes from a link sent to the administrator's email address.
      const r = await post(`/backups/${b.name}/download-link`);
      status.msg = tr("A download link was sent to {email}. It works once, for 15 minutes.", { email: r.sent_to });
    } catch (e) {
      status.fail(e);
    }
  }

  async function remove(b) {
    if (!confirm(tr("Delete the backup {name}?", { name: b.name }))) return;
    status.clear();
    try {
      await del(`/backups/${b.name}`);
      status.msg = tr("Deleted.");
      await load();
    } catch (e) {
      status.fail(e);
    }
  }

  function askRestore(b) {
    dialog.clear();
    confirmText = "";
    restoring = b;
  }

  async function restore(e) {
    e.preventDefault();
    dialog.clear();
    try {
      await post(`/backups/${restoring.name}/restore`, { confirm: confirmText.trim().toLowerCase() });
      restoring = null;
      wasRunning = "restore";
      await load();
    } catch (err) {
      dialog.fail(err);
    }
  }
</script>

<PageHead
  title={tr("Backups")}
  intro={tr(
    "A backup is one file with the whole database and, if you like, the uploaded files. Restoring one replaces everything on the site with what it holds.",
  )}
  {status}
>
  <button onclick={backUp} disabled={running}>{tr("Back up now")}</button>
</PageHead>

{#if running}
  <p role="status" class="muted">
    {KIND[activity.running]}{PHASE[activity.phase] ? `: ${PHASE[activity.phase]}` : "…"}
  </p>
{:else if activity.last_error}
  <p class="err" role="alert">{activity.last_error}</p>
{/if}

<Box title={tr("Automatic backups")}>
  <Section switches>
    <Switch
      label={tr("Back up the site automatically")}
      checked={cfg.backup_enabled}
      onchange={(e) => setting("backup_enabled", e.currentTarget.checked)}
    />
    <Switch
      label={tr("Include uploaded files in automatic backups")}
      checked={cfg.backup_uploads}
      onchange={(e) => setting("backup_uploads", e.currentTarget.checked)}
    />
  </Section>
  <Section grid>
    <Field
      label={tr("Hours between backups")}
      type="number"
      min="1"
      max="720"
      value={cfg.backup_interval_hours}
      onchange={(e) => setting("backup_interval_hours", Number(e.currentTarget.value) || 24)}
    />
    <Field
      label={tr("Backups to keep")}
      type="number"
      min="1"
      max="365"
      value={cfg.backup_keep}
      onchange={(e) => setting("backup_keep", Number(e.currentTarget.value) || 7)}
    />
  </Section>
</Box>

<Box title={tr("Stored backups")}>
  <div class="acp-box-body">
    <p class="muted"><small>{tr("Kept in {directory} on the server.", { directory })}</small></p>
  </div>
  {#if backups.length}
    <ul class="acp-rows">
      {#each backups as b (b.name)}
        <li class="acp-rowitem">
          <div class="acp-grow">
            <code class="acp-wrap">{b.name}</code>
            <br />
            <small class="muted"><When iso={b.created_at} /> · {size(b.size)}</small>
            <span class="acp-pill">{b.uploads ? tr("Database and uploads") : tr("Database only")}</span>
            {#if b.trigger === "automatic"}<span class="acp-pill is-muted">{tr("Automatic")}</span>{/if}
            {#if !b.restorable}<span class="acp-pill is-warn">{tr("From a newer version")}</span>{/if}
          </div>
          <div class="acp-actions">
            <button class="secondary" onclick={() => download(b)}>{tr("Email me a download link")}</button>
            <button class="secondary" onclick={() => askRestore(b)} disabled={running || !b.restorable}
              >{tr("Restore")}</button
            >
            <IconButton
              icon="trash-2"
              label={tr("Delete {name}", { name: b.name })}
              danger
              onclick={() => remove(b)}
              disabled={running}
            />
          </div>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="acp-empty">{loaded ? tr("No backups yet.") : tr("Loading…")}</p>
  {/if}
</Box>

{#if restoring}
  <EditDialog
    title={tr("Restore this backup?")}
    onclose={() => (restoring = null)}
    onsubmit={restore}
    status={dialog}
    save={tr("Restore")}
    busy={confirmText.trim().toLowerCase() !== "restore"}
  >
    <div class="acp-form">
      <p class="acp-wide">
        {tr(
          "Everything on the site is replaced by what {name} holds: members, posts, settings and uploaded files. Whatever happened after it was made is lost. The site goes offline while it runs, and you will have to sign in again.",
          { name: restoring.name },
        )}
      </p>
      <Field label={tr("Type restore to confirm")} bind:value={confirmText} required wide autocomplete="off" />
    </div>
  </EditDialog>
{/if}
