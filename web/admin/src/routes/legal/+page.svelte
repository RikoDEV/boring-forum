<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import Box from "#lib/Box.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import MarkdownField from "#lib/MarkdownField.svelte";
  import { get, put } from "#lib/api.js";

  const KINDS = [
    ["terms", tr("Terms of service")],
    ["privacy", tr("Privacy policy")],
    ["guidelines", tr("Community guidelines")],
  ];
  const versions = $state({}); // kind -> latest version (0: never published)
  let draft = $state(null); // { kind, label, body } while a document is open for a new version
  const status = new Status();
  const dialog = new Status();

  // Opening a document starts from its latest version, so an edit is a change, not a rewrite.
  async function open(kind, label) {
    status.clear();
    dialog.clear();
    try {
      const r = await get(`/legal/${kind}`);
      draft = { kind, label, body: r.body };
    } catch (err) {
      status.fail(err);
    }
  }

  async function publish(e) {
    e.preventDefault();
    dialog.clear();
    if (!draft.body.trim()) return void (dialog.error = tr("The document needs text."));
    try {
      const r = await put(`/legal/${draft.kind}`, { body: draft.body });
      versions[draft.kind] = r.version;
      status.msg =
        draft.kind === "guidelines"
          ? tr("Published version {version}. Members will be asked to accept it.", { version: r.version })
          : tr("Published version {version}. Members will be asked to accept it before posting again.", {
              version: r.version,
            });
      draft = null;
    } catch (err) {
      dialog.fail(err);
    }
  }

  for (const [k] of KINDS) {
    get(`/legal/${k}`).then((r) => (versions[k] = r.version), status.fail);
  }
</script>

<PageHead
  title={tr("Legal documents")}
  intro={tr(
    "Each save publishes a new version. Members accept the latest terms and privacy policy when they register, and again after a change.",
  )}
  {status}
/>

<Box>
  <ul class="acp-rows">
    {#each KINDS as [k, label]}
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{label}</strong><br />
          {#if versions[k]}<small class="muted">{tr("Version {version}", { version: versions[k] })}</small>
          {:else if k in versions}<small class="muted">
              {tr("Not published yet: it has no link on the site.")}
            </small>{/if}
        </div>
        {#if versions[k]}<a href="/legal/{k}" target="_blank">{tr("View current")}</a>{/if}
        <button class="secondary" onclick={() => open(k, label)}>{tr("Edit")}</button>
      </li>
    {/each}
  </ul>
</Box>

{#if draft}
  <EditDialog
    title={draft.label}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={publish}
    status={dialog}
    save={tr("Publish new version")}
  >
    <MarkdownField label={tr("Document text")} bind:value={draft.body} rows={16} />
  </EditDialog>
{/if}
