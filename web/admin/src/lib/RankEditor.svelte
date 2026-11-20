<script>
  import { tr } from "#lib/i18n.js";
  // Edits the `score:name,score:name` setting as a table of ranks.
  let { id, value = $bindable("") } = $props();

  let nextId = 0;
  const parse = (text) =>
    String(text)
      .split(",")
      .map((part) => part.split(/:(.*)/s))
      .filter(([score, name]) => name !== undefined && score.trim() !== "" && Number.isFinite(Number(score)))
      .map(([score, name]) => ({ id: nextId++, score: Number(score), name: name.trim() }))
      .sort((a, b) => a.score - b.score);
  const serialize = (list) =>
    list
      .filter((r) => r.name.trim() && r.score !== null && r.score !== "")
      .map((r) => `${Number(r.score)}:${r.name.trim().replaceAll(",", " ")}`)
      .join(",");

  let rows = $state(parse(value));

  let seen = value; // the setting as last read or written here
  // svelte-ignore state_referenced_locally (the baseline is the rows as first parsed)
  let written = serialize(rows); // what the rows last produced, so opening the page changes nothing

  // Edits to the rows flow out to the setting.
  $effect(() => {
    const out = serialize(rows);
    if (out !== written) {
      written = out;
      seen = out;
      value = out;
    }
  });
  // Discarding changes replaces the value from outside.
  $effect(() => {
    if (value !== seen) {
      seen = value;
      rows = parse(value);
      written = serialize(rows);
    }
  });

  const tidy = () => {
    rows = [...rows].sort((a, b) => Number(a.score) - Number(b.score));
  };
  function add() {
    const top = rows.reduce((max, r) => Math.max(max, Number(r.score) || 0), 0);
    rows.push({ id: nextId++, score: rows.length ? top + 100 : 0, name: "" });
  }
  function remove(row) {
    rows = rows.filter((r) => r !== row);
  }
  const scores = $derived(rows.map((r) => Number(r.score)));
  const hasDuplicate = $derived(new Set(scores).size !== scores.length);
  const noStart = $derived(rows.length > 0 && Math.min(...scores) > 0);
</script>

<div class="acp-ranks" {id}>
  {#if rows.length}
    <div class="acp-rank-head" aria-hidden="true">
      <span>{tr("From reputation")}</span><span>{tr("Rank name")}</span>
    </div>
    {#each rows as row (row.id)}
      <div class="acp-rank">
        <input type="number" aria-label={tr("Reputation needed")} bind:value={row.score} onchange={tidy} />
        <input
          type="text"
          aria-label={tr("Rank name")}
          placeholder={tr("Rank name")}
          maxlength="40"
          bind:value={row.name}
        />
        <button
          type="button"
          class="secondary"
          aria-label={tr("Remove {n}", { n: row.name || "rank" })}
          onclick={() => remove(row)}>{tr("Remove")}</button
        >
      </div>
    {/each}
  {:else}
    <p class="muted">{tr("No ranks yet, so members show no rank.")}</p>
  {/if}
  {#if hasDuplicate}<p class="err">
      {tr("Two ranks start at the same reputation; only one of them will be used.")}
    </p>{/if}
  {#if noStart}<p class="muted">{tr("Members below {n} reputation show no rank.", { n: Math.min(...scores) })}</p>{/if}
  <button type="button" class="secondary" onclick={add}>{tr("Add rank")}</button>
</div>
