<script>
  import Empty from "#lib/Empty.svelte";
  import { SvelteMap } from "svelte/reactivity";
  import { tr, locale } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import PageSettings from "#lib/PageSettings.svelte";
  import Box from "#lib/Box.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import BarChart from "#lib/BarChart.svelte";
  import UserLink from "#lib/UserLink.svelte";
  import When from "#lib/When.svelte";

  const FEATURES = {
    summary: tr("Summaries"),
    gist: tr("Gists"),
    moderation: tr("Spam and mood checks"),
    translate: tr("Translation"),
    search: tr("Search by meaning"),
    embedding: tr("Indexing topics"),
    helper: tr("Composer help"),
    bot: tr("Personas"),
    automation: tr("Automation"),
    report: tr("Reports"),
    test: tr("Connection test"),
  };
  const blank = () => ({
    id: 0,
    slug: "",
    name: "",
    system_prompt: "",
    enabled: true,
    model: "",
    use_search: true,
    max_tokens: 800,
    group_ids: [],
  });

  const status = new Status();
  let overview = $state(null);
  let personas = $state([]);
  let groups = $state([]);
  let draft = $state(null);
  let testing = $state(false);
  let tested = $state(null);

  const num = (n) => Number(n).toLocaleString(locale());
  const feature = (f) => FEATURES[f] ?? f;
  const load = () => get("/ai").then((r) => (overview = r), status.fail);
  const loadPersonas = () => get("/ai/personas").then((r) => (personas = r), status.fail);
  get("/groups").then(
    (g) => (groups = g),
    () => {},
  );
  load();
  loadPersonas();

  // Tokens per day over the days the report covers, oldest first.
  const days = $derived.by(() => {
    if (!overview) return [];
    const out = [];
    for (let i = 13; i >= 0; i--) {
      const d = new Date(Date.now() - i * 86_400_000);
      const key = d.toISOString().slice(0, 10);
      const value = overview.usage
        .filter((u) => u.day === key)
        .reduce((sum, u) => sum + u.prompt_tokens + u.completion_tokens, 0);
      out.push({ label: key, value });
    }
    return out;
  });
  const byFeature = $derived.by(() => {
    const m = new SvelteMap();
    for (const u of overview?.usage ?? []) {
      const row = m.get(u.feature) ?? { feature: u.feature, requests: 0, failures: 0, tokens: 0 };
      row.requests += u.requests;
      row.failures += u.failures;
      row.tokens += u.prompt_tokens + u.completion_tokens;
      m.set(u.feature, row);
    }
    return [...m.values()].sort((a, b) => b.tokens - a.tokens);
  });
  const moodTotals = $derived(
    (overview?.mood ?? []).reduce(
      (t, m) => ({
        positive: t.positive + m.positive,
        neutral: t.neutral + m.neutral,
        negative: t.negative + m.negative,
        toxic: t.toxic + m.toxic,
        spam: t.spam + m.spam,
      }),
      { positive: 0, neutral: 0, negative: 0, toxic: 0, spam: 0 },
    ),
  );

  async function test() {
    testing = true;
    tested = null;
    status.clear();
    try {
      tested = await post("/ai/test");
      load();
    } catch (e) {
      status.fail(e);
    } finally {
      testing = false;
    }
  }
  async function reindex() {
    if (!confirm(tr("Throw away every topic's index? They are made again in the background, which uses the provider.")))
      return;
    status.clear();
    post("/ai/embeddings/reset").then(load, status.fail);
  }

  function save(e) {
    e.preventDefault();
    status.clear();
    const body = { ...draft, max_tokens: Number(draft.max_tokens), slug: draft.slug.trim().toLowerCase() };
    (draft.id ? put(`/ai/personas/${draft.id}`, body) : post("/ai/personas", body)).then(() => {
      draft = null;
      loadPersonas();
    }, status.fail);
  }
  const remove = (p) =>
    confirm(tr("Delete {name}? Its earlier answers stay in their topics.", { name: p.name })) &&
    del(`/ai/personas/${p.id}`).then(() => {
      draft = null;
      loadPersonas();
    }, status.fail);
  const toggle = (p) => put(`/ai/personas/${p.id}`, { ...p, enabled: !p.enabled }).then(loadPersonas, status.fail);
</script>

<PageHead
  title={tr("AI")}
  intro={tr(
    "Summaries, spam checks, translation, search by meaning and a helper in the editor. Everything is off until you turn it on and give it a provider in the settings; nothing here sends private forums to the provider unless you allow it.",
  )}
  {status}
>
  <a class="btn secondary" href="#setting=ai_enabled">{tr("AI settings")}</a>
  <button disabled={testing || !overview?.configured} onclick={test}>{tr("Test the connection")}</button>
</PageHead>

{#if overview}
  <Box title={tr("Connection")}>
    <ul class="acp-rows">
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{overview.configured ? tr("AI is on") : tr("AI is off")}</strong>
          <br /><small class="muted">
            {overview.configured
              ? overview.key_in_environment
                ? tr("The key comes from BF_AI_API_KEY.")
                : tr("The key is in the settings.")
              : tr("Turn on “AI” and fill in the provider's address in the settings.")}
          </small>
        </div>
        <span class="acp-pill" class:is-muted={!overview.configured}>{overview.configured ? tr("On") : tr("Off")}</span>
      </li>
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{tr("Tokens this month")}</strong>
          <br /><small class="muted">
            {overview.token_limit > 0
              ? tr("{used} of {limit}", { used: num(overview.tokens_this_month), limit: num(overview.token_limit) })
              : tr("{used}, with no limit set", { used: num(overview.tokens_this_month) })}
          </small>
        </div>
        {#if overview.token_limit > 0}<meter
            min="0"
            max={overview.token_limit}
            value={overview.tokens_this_month}
            aria-label={tr("Tokens this month")}
          ></meter>{/if}
      </li>
      {#if tested}
        <li class="acp-rowitem" role="status">
          <div class="acp-grow">
            <strong>{tr("It works")}</strong>
            <br /><small class="muted">
              {tr("{model} answered “{answer}” in {ms} ms.", {
                model: tested.model,
                answer: tested.answer,
                ms: tested.ms,
              })}
              {#if tested.embedding_dims}
                {tr("Embeddings: {dims} numbers per topic, {ms} ms.", {
                  dims: tested.embedding_dims,
                  ms: tested.embedding_ms,
                })}
              {/if}
            </small>
          </div>
        </li>
      {/if}
    </ul>
  </Box>

  <Box title={tr("Use in the last 14 days")}>
    <div class="acp-box-body"><BarChart title={tr("Tokens")} data={days} /></div>
    {#if byFeature.length}
      <table>
        <thead>
          <tr><th>{tr("What for")}</th><th>{tr("Requests")}</th><th>{tr("Failed")}</th><th>{tr("Tokens")}</th></tr>
        </thead>
        <tbody>
          {#each byFeature as f (f.feature)}
            <tr>
              <td>{feature(f.feature)}</td><td>{num(f.requests)}</td><td>{num(f.failures)}</td><td>{num(f.tokens)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <Empty title={tr("Nothing has been sent to the provider yet.")} />
    {/if}
    {#if overview.errors.length}
      <details class="acp-box-body bf-disclosure">
        <summary>{tr("Latest failures")}</summary>
        <ul class="acp-rows">
          {#each overview.errors as e}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <strong>{feature(e.feature)}</strong>
                <br /><small class="muted">{e.error}</small>
              </div>
              <When iso={e.at} />
            </li>
          {/each}
        </ul>
      </details>
    {/if}
  </Box>

  <Box title={tr("Mood of new posts")}>
    {#if overview.mood.length}
      <div class="acp-box-body">
        <p>
          {tr(
            "{positive} positive, {neutral} neutral and {negative} negative in 14 days; {toxic} looked abusive and {spam} looked like spam.",
            moodTotals,
          )}
        </p>
        {#if overview.emotions.length}
          <p class="muted">
            {tr("Most common feelings:")}
            {overview.emotions.map((e) => `${e.emotion} (${e.count})`).join(", ")}
          </p>
        {/if}
      </div>
      <table>
        <thead>
          <tr>
            <th>{tr("Day")}</th><th>{tr("Positive")}</th><th>{tr("Neutral")}</th><th>{tr("Negative")}</th><th>
              {tr("Abusive")}
            </th><th>{tr("Spam")}</th>
          </tr>
        </thead>
        <tbody>
          {#each overview.mood as m (m.day)}
            <tr>
              <td>{m.day}</td><td>{m.positive}</td><td>{m.neutral}</td><td>{m.negative}</td><td>{m.toxic}</td><td>
                {m.spam}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <Empty title={tr("No posts have been judged yet.")} text={tr("Turn on the mood record in the settings.")} />
    {/if}
  </Box>

  {#if overview.flagged.length}
    <Box title={tr("Posts the AI doubted")}>
      <ul class="acp-rows">
        {#each overview.flagged as f (f.post)}
          <li class="acp-rowitem">
            <div class="acp-grow">
              <a href="/p/{f.post}">{f.title}</a>
              <br /><small class="muted">
                <UserLink name={f.author} /> · {tr("spam {spam}, abuse {toxicity}", {
                  spam: f.spam,
                  toxicity: f.toxicity,
                })}
              </small>
            </div>
          </li>
        {/each}
      </ul>
    </Box>
  {/if}

  <Box title={tr("Search by meaning")}>
    <ul class="acp-rows">
      <li class="acp-rowitem">
        <div class="acp-grow">
          <strong>{tr("Topics indexed")}</strong>
          <br /><small class="muted">
            {overview.embeddings_configured
              ? tr("{done} of {all}. New topics are added every few minutes.", {
                  done: num(overview.embedded),
                  all: num(overview.topics),
                })
              : tr("Fill in an embedding model in the settings to use related topics and search by meaning.")}
          </small>
        </div>
        {#if overview.topics > 0}<meter
            min="0"
            max={overview.topics}
            value={overview.embedded}
            aria-label={tr("Topics indexed")}
          ></meter>{/if}
        <button class="secondary" disabled={!overview.embedded} onclick={reindex}>{tr("Start again")}</button>
      </li>
    </ul>
  </Box>
{/if}

<PageHead
  title={tr("Personas")}
  intro={tr(
    "A persona answers when a member mentions it as @ai-name in a topic. Write what it is for and how it should answer.",
  )}
>
  <button
    onclick={() => {
      status.clear();
      draft = blank();
    }}
  >
    {tr("New persona")}
  </button>
</PageHead>

{#if draft}
  <EditDialog
    title={draft.id ? tr("Edit {name}", { name: draft.name }) : tr("New persona")}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    ondelete={draft.id ? () => remove(draft) : null}
    save={draft.id ? tr("Save") : tr("Add persona")}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={draft.name} maxlength="60" required />
      <Field label={tr("Mention it as @ai-…")} bind:value={draft.slug} maxlength="20" pattern="[a-z0-9\-]+" required />
      <Field label={tr("Instructions")} bind:value={draft.system_prompt} rows="8" maxlength="8000" wide required />
      <Field label={tr("Model (empty: the site's)")} bind:value={draft.model} />
      <Field label={tr("Longest answer (tokens)")} type="number" min="50" max="4000" bind:value={draft.max_tokens} />
    </div>
    <fieldset class="acp-picker">
      <legend>{tr("Who may ask")}</legend>
      <MultiSelect
        label={tr("Groups")}
        placeholder={tr("Everyone who can post")}
        options={groups.filter((g) => g.id > 1).map((g) => ({ value: g.id, label: g.name }))}
        bind:value={draft.group_ids}
      />
    </fieldset>
    <fieldset class="acp-picker">
      <legend>{tr("Behaviour")}</legend>
      <Switch
        label={tr("Looks up what the forum already says (needs an embedding model)")}
        bind:checked={draft.use_search}
      />
      <Switch label={tr("On")} bind:checked={draft.enabled} />
    </fieldset>
  </EditDialog>
{/if}

<div class="acp-box">
  {#if personas.length}
    <ul class="acp-rows">
      {#each personas as p (p.id)}
        <li class="acp-rowitem" class:is-off={!p.enabled}>
          <div class="acp-grow">
            <strong>{p.name}</strong> <code>@ai-{p.slug}</code>
            {#if !p.enabled}<span class="acp-pill is-muted">{tr("Off")}</span>{/if}
            <br /><small class="muted">{p.system_prompt.slice(0, 140)}{p.system_prompt.length > 140 ? "…" : ""}</small>
          </div>
          <button
            class="secondary"
            onclick={() => {
              status.clear();
              draft = { ...p };
            }}
          >
            {tr("Edit")}
          </button>
          <button class="secondary" onclick={() => toggle(p)}>{p.enabled ? tr("Turn off") : tr("Turn on")}</button>
        </li>
      {/each}
    </ul>
  {:else}
    <Empty
      title={tr("No personas yet.")}
      text={tr("Turn on “Personas answer when mentioned” in the settings, then add one.")}
    />
  {/if}
</div>

<PageSettings path="/ai" onsaved={load} />
