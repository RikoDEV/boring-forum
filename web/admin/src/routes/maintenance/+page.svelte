<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import Box from "#lib/Box.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import { get, post, put } from "#lib/api.js";

  const status = new Status();
  const dialog = new Status();
  let exported = $state("");
  let importText = $state("");
  let dialogKind = $state(""); // "export", "import" or "check" while that dialog is open
  const check = $state({ user: "", forum: "" });
  let checkResult = $state(null);
  let forums = $state([]);

  get("/tree").then(
    (t) => (forums = t.nodes.filter((n) => n.kind !== "category")),
    () => {},
  );

  async function run(label, fn, into = status) {
    into.clear();
    try {
      const r = await fn();
      status.msg = r
        ? tr("{task}: done ({result})", { task: label, result: JSON.stringify(r) })
        : tr("{task}: done", { task: label });
      return true;
    } catch (e) {
      into.fail(e);
      return false;
    }
  }

  const TASKS = [
    ["recount", tr("Recount statistics")],
    ["recook", tr("Re-cook all posts")],
    ["rebuild-search", tr("Rebuild search indexes")],
    ["housekeeping", tr("Run housekeeping")],
    ["clear-caches", tr("Clear caches")],
  ];
  const task = (t, label) => run(label, () => post(`/maintenance/${t}`));
  const offline = (on) =>
    run(on ? tr("Offline mode on") : tr("Offline mode off"), () => put("/settings/offline", { value: on }));
  const close = () => (dialogKind = "");
  const openDialog = (kind) => {
    dialog.clear();
    dialogKind = kind;
  };
  async function doExport() {
    status.clear();
    if (await run(tr("Export"), async () => void (exported = JSON.stringify(await get("/settings-export"), null, 2)))) {
      openDialog("export");
    }
  }
  async function doImport(e) {
    e.preventDefault();
    let body;
    try {
      body = JSON.parse(importText);
    } catch {
      dialog.error = tr("That is not valid JSON.");
      return;
    }
    if (await run(tr("Import"), () => post("/settings-import", body), dialog)) {
      importText = "";
      close();
    }
  }
  async function doCheck(e) {
    e.preventDefault();
    await run(
      tr("Permission check"),
      async () =>
        void (checkResult = await get(`/permission-check?user=${encodeURIComponent(check.user)}&forum=${check.forum}`)),
      dialog,
    );
    if (!dialog.error) close();
  }
</script>

<PageHead title={tr("Maintenance")} {status} />

<Box title={tr("Tasks")}>
  <div class="acp-actions">
    {#each TASKS as [t, label]}<button onclick={() => task(t, label)}>{label}</button>{/each}
  </div>
</Box>

<Box title={tr("Offline mode")}>
  <p class="muted">
    {tr("Only administrators can use the site while it is offline. The message is the offline_message setting.")}
  </p>
  <div class="acp-actions">
    <button onclick={() => offline(true)}>{tr("Take the site offline")}</button><button
      class="secondary"
      onclick={() => offline(false)}>{tr("Bring it back")}</button
    >
  </div>
</Box>

<Box title={tr("Settings export and import")}>
  <div class="acp-actions">
    <button class="secondary" onclick={doExport}>{tr("Export changed settings")}</button><button
      class="secondary"
      onclick={() => openDialog("import")}>{tr("Import settings")}</button
    >
  </div>
</Box>

<Box title={tr("Permission check")}>
  <div class="acp-actions">
    <button class="secondary" onclick={() => openDialog("check")}>{tr("Check a member's permissions")}</button>
  </div>
  {#if checkResult}
    <p>
      {tr("{user} in {forum} can: {perms}", {
        user: checkResult.user,
        forum: checkResult.forum,
        perms: checkResult.result.join(", ") || tr("nothing"),
      })}
    </p>
    <ul>
      {#each checkResult.groups as g}<li>
          {tr("Group {group}: {grants}", { group: g.group, grants: g.grants.join(", ") || tr("no grants") })}
        </li>{/each}
      {#each checkResult.reasons as r}<li>{r}</li>{/each}
    </ul>
  {/if}
</Box>

{#if dialogKind === "export"}
  <EditDialog
    title={tr("Export changed settings")}
    onclose={close}
    onsubmit={(e) => (e.preventDefault(), close())}
    save={tr("Close")}
  >
    <div class="acp-form">
      <Field label={tr("Changed settings")} rows="10" wide readonly value={exported} />
    </div>
  </EditDialog>
{:else if dialogKind === "import"}
  <EditDialog title={tr("Import settings")} onclose={close} onsubmit={doImport} status={dialog} save={tr("Import")}>
    <div class="acp-form">
      <Field label={tr("Paste an export here")} rows="10" wide required bind:value={importText} />
    </div>
  </EditDialog>
{:else if dialogKind === "check"}
  <EditDialog title={tr("Permission check")} onclose={close} onsubmit={doCheck} status={dialog} save={tr("Check")}>
    <div class="acp-form">
      <Field label={tr("Username")} bind:value={check.user} required />
      <Field
        label={tr("Forum")}
        bind:value={check.forum}
        required
        options={[["", ""], ...forums.map((f) => [f.id, f.title])]}
      />
    </div>
  </EditDialog>
{/if}
