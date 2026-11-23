<script>
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import Empty from "#lib/Empty.svelte";
  import { get, post, del } from "#lib/api.js";
  let words = $state([]);
  let loaded = $state(false);
  const fresh = $state({ action: "block", pattern: "", replacement: "" });
  const status = new Status();
  let adding = $state(false);
  // Import from a text file: the words are read in the browser, sent as text, one action for all.
  let importing = $state(false);
  let file = $state({ name: "", text: "", words: 0 });
  const bulk = $state({ action: "block", replacement: "" });
  // One word per line, or several separated by commas; lines starting with # are notes.
  const countWords = (text) =>
    new Set(
      text
        .replace(/^\uFEFF/, "")
        .split(/\r?\n/)
        .filter((l) => l.trim() && !l.trim().startsWith("#"))
        .flatMap((l) => l.split(/[,;]/).map((w) => w.trim().toLowerCase()))
        .filter((w) => w && !/\s/.test(w)),
    ).size;
  async function pick(e) {
    const f = e.currentTarget.files[0];
    if (!f) return;
    const text = await f.text();
    file = { name: f.name, text, words: countWords(text) };
  }
  async function importWords(e) {
    e.preventDefault();
    status.clear();
    try {
      const r = await post("/watched-words/import", {
        action: bulk.action,
        replacement: bulk.action === "replace" ? bulk.replacement : null,
        text: file.text,
      });
      importing = false;
      file = { name: "", text: "", words: 0 };
      status.msg = tr("Words added: {added}, already there: {existing}, skipped: {skipped}.", r);
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  const LABELS = {
    block: tr("Block the post"),
    censor: tr("Censor (■■■)"),
    require_approval: tr("Hold for approval"),
    replace: tr("Replace with"),
    flag: tr("Publish, and report as spam"),
    silence: tr("Hold the post and silence its author"),
  };

  async function load() {
    try {
      words = await get("/watched-words");
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }

  async function add(e) {
    e.preventDefault();
    status.clear();
    try {
      await post("/watched-words", { ...fresh, replacement: fresh.action === "replace" ? fresh.replacement : null });
      fresh.pattern = fresh.replacement = "";
      adding = false;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  const remove = (w) => del(`/watched-words/${w.id}`).catch(status.fail).then(load);

  load();
</script>

<PageHead title={tr("Watched words")} status={adding || importing ? null : status}>
  {#snippet intro()}{tr("Whole words, any case.")} <code>*</code>
    {tr("matches any ending:")} <code>spam*</code>
    {tr("also catches “spammer”. Forum moderators are not affected.")}{/snippet}
  <button class="secondary" onclick={() => (status.clear(), (importing = true))}>{tr("Import from file")}</button>
  <button onclick={() => (status.clear(), (adding = true))}>{tr("New watched word")}</button>
</PageHead>
{#if importing}
  <EditDialog
    title={tr("Import watched words")}
    onclose={() => (importing = false)}
    onsubmit={importWords}
    {status}
    busy={!file.words}
    save={tr("Import")}
  >
    <div class="acp-form">
      <Field label={tr("Action for all of them")} bind:value={bulk.action} options={Object.entries(LABELS)} />
      {#if bulk.action === "replace"}<Field
          label={tr("Replacement")}
          bind:value={bulk.replacement}
          required
          wide
        />{/if}
      <div class="acp-wide">
        <Section
          legend={tr("Word list")}
          hint={tr("A text file: one word per line, or words separated by commas. Lines starting with # are ignored.")}
        >
          <div class="acp-group-style">
            <label class="acp-upload">
              {tr("Choose a file")}<input type="file" accept=".txt,.csv,.list,text/plain,text/csv" onchange={pick} />
            </label>
            {#if file.name}<span class="muted">
                {tr("{name}: {n} words found", { name: file.name, n: file.words })}
              </span>{:else}<span class="muted">{tr("No file chosen.")}</span>{/if}
          </div>
        </Section>
      </div>
    </div>
  </EditDialog>
{/if}
{#if adding}
  <EditDialog title={tr("New watched word")} onclose={() => (adding = false)} onsubmit={add} {status} save={tr("Add")}>
    <div class="acp-form">
      <Field label={tr("Action")} bind:value={fresh.action} options={Object.entries(LABELS)} />
      <Field label={tr("Word")} bind:value={fresh.pattern} required />
      {#if fresh.action === "replace"}<Field
          label={tr("Replacement")}
          bind:value={fresh.replacement}
          required
          wide
        />{/if}
    </div>
  </EditDialog>
{/if}
<table>
  <thead><tr><th>{tr("Word")}</th><th>{tr("Action")}</th><th class="acp-cell-end"></th></tr></thead>
  <tbody>
    {#each words as w (w.id)}
      <tr>
        <td><code>{w.pattern}</code></td><td>{LABELS[w.action]}{w.replacement ? ` “${w.replacement}”` : ""}</td>
        <td class="acp-cell-end"><button class="is-danger" onclick={() => remove(w)}>{tr("Remove")}</button></td>
      </tr>
    {:else}
      <Empty cols={3} {loaded} title={tr("No watched words.")} text={tr("Use New watched word above to add one.")} />
    {/each}
  </tbody>
</table>
