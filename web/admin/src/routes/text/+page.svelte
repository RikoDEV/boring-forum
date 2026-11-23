<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, put } from "#lib/api.js";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Box from "#lib/Box.svelte";
  import Empty from "#lib/Empty.svelte";

  const LIMIT = 100;
  let catalog = $state([]);
  let overrides = $state([]);
  let locales = $state([]);
  let locale = $state("en");
  // /admin/text?q=… opens with that search, as the forum page's translate button does.
  let q = $state(new URLSearchParams(globalThis.location?.search).get("q") ?? "");
  let onlyMine = $state(false);
  let draft = $state(null); // the message being changed: { key, text, default, value, had }
  let loaded = $state(false);
  const status = new Status();

  const load = () =>
    get(`/text?locale=${locale}`)
      .then((r) => {
        catalog = r.catalog;
        overrides = r.overrides;
        locales = r.locales;
      }, status.fail)
      .finally(() => (loaded = true));
  const mine = (key) => overrides.find((o) => o.key === key && o.locale === locale)?.value ?? "";
  const count = $derived(overrides.filter((o) => o.locale === locale).length);
  const matches = $derived.by(() => {
    const needle = q.trim().toLowerCase();
    return catalog.filter(
      (c) =>
        (!onlyMine || mine(c.key)) &&
        (!needle || `${c.key} ${c.text} ${c.default} ${mine(c.key)}`.toLowerCase().includes(needle)),
    );
  });
  const shown = $derived(matches.slice(0, LIMIT));

  const lines = (t) => Math.min(12, Math.max(2, t.split("\n").length));
  const edit = (c) => {
    status.clear();
    draft = { ...c, value: mine(c.key) || c.default, had: !!mine(c.key) };
  };
  function save(e) {
    e.preventDefault();
    status.clear();
    put(`/text/${locale}/${draft.key}`, { value: draft.value }).then(() => {
      draft = null;
      return load();
    }, status.fail);
  }
  const remove = () =>
    del(`/text/${locale}/${draft.key}`).then(() => {
      draft = null;
      return load();
    }, status.fail);
  const pick = () => {
    draft = null;
    load();
  };
  load();
</script>

<PageHead
  title={tr("Site text")}
  intro={tr("Replace any interface text, one language at a time. A replacement takes the place of the whole message.")}
  status={draft ? null : status}
/>

{#if draft}
  <EditDialog
    title={draft.key}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    ondelete={draft.had ? remove : null}
    deleteLabel={tr("Remove override")}
    save={tr("Save text")}
  >
    <div class="acp-form">
      <Field
        label={tr("English")}
        value={draft.text}
        rows={lines(draft.text)}
        readonly
        wide
        class="acp-code"
        dir="ltr"
      />
      {#if locale !== "en" && draft.default !== draft.text}
        <Field
          label={tr("Built in for this language")}
          value={draft.default}
          rows={lines(draft.default)}
          readonly
          wide
          class="acp-code"
          dir="auto"
        />
      {/if}
      <Field
        label={tr("Your text")}
        bind:value={draft.value}
        rows={Math.max(6, lines(draft.value))}
        required
        wide
        class="acp-code"
        dir="auto"
        spellcheck="false"
        autofocus
      />
      <p class="acp-wide muted">
        <small>
          {tr(
            "Write a variable as { $name }, as in the English text above. For plurals use { $n -> [one] … *[other] … }.",
          )}
        </small>
      </p>
    </div>
  </EditDialog>
{/if}

<Box title={tr("Search text")}>
  <div class="acp-form">
    <Field
      label={tr("Language")}
      value={locale}
      onchange={(e) => {
        locale = e.currentTarget.value;
        pick();
      }}
      options={locales.map((l) => [l.code, l.name])}
    />
    <Field label={tr("Search text")} type="search" bind:value={q} />
    <Switch check wide label="{tr('Only my changes')} ({count})" bind:checked={onlyMine} />
  </div>
</Box>

{#if shown.length}
  <div class="acp-box">
    <ul class="acp-rows">
      {#each shown as c (c.key)}
        <li class="acp-rowitem">
          <div class="acp-grow">
            <small class="muted acp-text-key">{c.key}</small>{#if mine(c.key)}
              <span class="acp-pill">{tr("Changed")}</span>{/if}<br /><span dir="auto" class="acp-text-preview">
              {mine(c.key) || c.default}
            </span>
          </div>
          <button class="secondary" onclick={() => edit(c)}>{tr("Edit")}</button>
        </li>
      {/each}
    </ul>
  </div>
  {#if matches.length > LIMIT}<p class="muted">
      {tr("Showing the first {n} of {total}. Search to narrow the list.", { n: LIMIT, total: matches.length })}
    </p>{/if}
{:else}
  <Empty
    {loaded}
    title={onlyMine && !q ? tr("You have not changed any text in this language.") : tr("No text matches.")}
  />
{/if}
