<script>
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import Field from "#lib/Field.svelte";
  let info = $state(null);
  let sql = $state("SELECT username, post_count FROM users ORDER BY post_count DESC LIMIT 20");
  let name = $state("");
  let description = $state("");
  let current = $state(null);
  let result = $state(null);
  let running = $state(false);
  const status = new Status();
  const load = () => get("/explorer").then((d) => (info = d), status.fail);
  load();

  const columns = $derived(result?.rows?.length ? Object.keys(result.rows[0]) : []);
  const show = (v) => (v === null ? "" : typeof v === "object" ? JSON.stringify(v) : String(v));

  async function run() {
    status.clear();
    running = true;
    try {
      result = await post("/explorer/run", { sql });
    } catch (e) {
      status.fail(e);
      result = null;
    } finally {
      running = false;
    }
  }
  function open(q) {
    current = q;
    sql = q.sql;
    name = q.name;
    description = q.description;
    result = null;
  }
  async function save() {
    status.clear();
    const body = { name: name.trim(), description, sql };
    try {
      if (current) await put(`/explorer/queries/${current.id}`, body);
      else {
        const { id } = await post("/explorer/queries", body);
        current = { id, ...body };
      }
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  async function remove() {
    if (!current || !confirm(tr('Delete "{name}"?', { name: current.name }))) return;
    try {
      await del(`/explorer/queries/${current.id}`);
      current = null;
      name = "";
      description = "";
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  function csv() {
    const cell = (v) => {
      const s = show(v);
      return /[",\n\r]/.test(s) ? `"${s.replaceAll('"', '""')}"` : s;
    };
    const lines = [columns.map(cell).join(","), ...result.rows.map((r) => columns.map((c) => cell(r[c])).join(","))];
    const url = URL.createObjectURL(new Blob([lines.join("\r\n")], { type: "text/csv" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = `${(name || "query").replace(/[^\w-]+/g, "-")}.csv`;
    a.click();
    URL.revokeObjectURL(url);
  }
</script>

<PageHead
  title={tr("Data Explorer")}
  intro={tr(
    "Read-only SQL over forum data. Queries see the views listed below, never passwords, 2FA secrets, tokens, IP addresses or private messages. Each run is written to the audit log.",
  )}
  {status}
/>
{#if info && !info.available}<p class="err" role="alert">
    {tr("The")} <code>bf_explorer</code>
    {tr(
      "database role is missing, so queries cannot run. A database superuser can create it with the statements at the end of 0001_baseline.sql.",
    )}
  </p>{/if}

<div class="acp-split">
  <aside class="acp-box">
    <div class="acp-box-head"><h2>{tr("Saved queries")}</h2></div>
    <ul class="acp-rows">
      {#each info?.queries ?? [] as q (q.id)}
        <li>
          <button class="link" aria-current={current?.id === q.id} onclick={() => open(q)}>{q.name}</button
          >{#if q.description}<p class="muted"><small>{q.description}</small></p>{/if}
        </li>
      {/each}
      <li>
        <button
          class="link"
          onclick={() => {
            current = null;
            name = "";
            description = "";
          }}>{tr("+ New query")}</button
        >
      </li>
    </ul>
    <div class="acp-box-head"><h2>{tr("Views")}</h2></div>
    <ul class="acp-rows">
      {#each info?.schema ?? [] as t (t.name)}
        <li>
          <details class="bf-disclosure is-small"><summary><code>{t.name}</code></summary><small class="muted">{t.columns.join(", ")}</small></details>
        </li>
      {/each}
    </ul>
  </aside>

  <section>
    <div class="acp-form">
      <Field label={tr("SELECT …")} rows="10" spellcheck="false" bind:value={sql} class="acp-code" />
      <div class="acp-actions">
        <button onclick={run} disabled={running || !info?.available}>{running ? tr("Running…") : tr("Run")}</button>
      </div>
      <Field label={tr("Name")} bind:value={name} />
      <Field label={tr("Description")} bind:value={description} />
      <div class="acp-actions">
        <button class="secondary" onclick={save} disabled={!name.trim()}
          >{current ? tr("Save changes") : tr("Save query")}</button
        >{#if current}<button class="secondary" onclick={remove}>{tr("Delete")}</button>{/if}
      </div>
    </div>

    {#if result}
      <p class="muted">
        <small
          >{tn("admin.count.rows", result.rows.length)}{result.truncated
            ? ` ${tr("(the first {max_rows})", { max_rows: info.max_rows })}`
            : ""} · {tr("{ms} ms", { ms: result.ms })}</small
        >{#if result.rows.length}
          <button class="link" onclick={csv}>{tr("Download CSV")}</button>{/if}
      </p>
      {#if result.rows.length}
        <div style:overflow-x="auto">
          <table>
            <thead
              ><tr
                >{#each columns as c}<th>{c}</th>{/each}</tr
              ></thead
            >
            <tbody
              >{#each result.rows as r}<tr
                  >{#each columns as c}<td>{show(r[c])}</td>{/each}</tr
                >{/each}</tbody
            >
          </table>
        </div>
      {/if}
    {/if}
  </section>
</div>
