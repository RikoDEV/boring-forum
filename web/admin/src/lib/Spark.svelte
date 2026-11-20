<script>
  // A tiny trend of one measure for a stat card: decoration only, the card's text says the numbers.
  /** @type {{ data: number[] }} */ let { data } = $props();
  const max = $derived(Math.max(1, ...data));
</script>

<span class="acp-spark" aria-hidden="true">
  {#each data as v}<i style="height: {v > 0 ? Math.max(8, (v / max) * 100) : 0}%"></i>{/each}
</span>
