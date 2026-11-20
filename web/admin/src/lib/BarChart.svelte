<script>
  import { tn, tr, locale } from "#lib/i18n.js";
  // One measure as thin bars from a shared baseline. Hover or focus a column for its value; the
  // table below holds the same numbers for anyone who cannot read the chart.
  let { title, data, unit = "", span = "" } = $props();
  const max = $derived(Math.max(1, ...data.map((d) => d.value)));
  const total = $derived(data.reduce((sum, d) => sum + d.value, 0));
  let active = $state(null);
  const shown = $derived(active === null ? null : data[active]);
</script>

<figure class="acp-chart">
  <figcaption>
    <span>{title}</span>
    <b>{shown ? shown.value.toLocaleString(locale()) : total.toLocaleString(locale())}</b>
    <small class="muted">{shown ? shown.label : span || tn("admin.count.last_days", data.length)}{unit}</small>
  </figcaption>
  <div class="acp-chart-plot">
    <span class="acp-chart-max" aria-hidden="true">{max.toLocaleString(locale())}</span>
    <div
      class="acp-chart-bars"
      role="img"
      aria-label={span
        ? tr("{title}: {total}, at most {max} in a column", { title, total, max })
        : tr("{title}: {total} in the last {length} days, at most {max} in a day", {
            title,
            total,
            length: data.length,
            max,
          })}
    >
      {#each data as d, i}
        <div
          class="acp-chart-col"
          class:is-active={active === i}
          title="{d.label}: {d.value}"
          role="presentation"
          onmouseenter={() => (active = i)}
          onmouseleave={() => (active = null)}
        >
          {#if d.value > 0}<i style="height: {Math.max(2, (d.value / max) * 100)}%"></i>{/if}
        </div>
      {/each}
    </div>
  </div>
  <div class="acp-chart-axis" aria-hidden="true">
    <span>{data[0]?.label}</span><span>{data[data.length - 1]?.label}</span>
  </div>
  <details class="acp-chart-table bf-disclosure is-small">
    <summary>{tr("View as table")}</summary>
    <table>
      <thead><tr><th>{span ? tr("Slot") : tr("Day")}</th><th>{title}</th></tr></thead>
      <tbody>
        {#each data as d}<tr><td>{d.label}</td><td>{d.value}</td></tr>{/each}
      </tbody>
    </table>
  </details>
</figure>
