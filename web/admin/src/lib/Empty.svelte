<script>
  /// The empty state of a list: a bold line and what to do about it, the same everywhere. `cols`
  /// makes it a table row spanning that many columns, `li` a list item; `loaded` false draws
  /// nothing, so a list does not announce "nothing yet" while its data is still on the way.

  /** @type {{ title: string, text?: string, cols?: number, li?: boolean, loaded?: boolean }} */
  let { title, text = "", cols = 0, li = false, loaded = true } = $props();
</script>

{#snippet body()}
  <strong>{title}</strong>{#if text}<span class="muted">{text}</span>{/if}
{/snippet}
{#if loaded}
  {#if cols}
    <tr class="acp-empty-row"><td colspan={cols}><div class="acp-empty-state">{@render body()}</div></td></tr>
  {:else if li}
    <li class="acp-empty-state">{@render body()}</li>
  {:else}
    <div class="acp-empty-state">{@render body()}</div>
  {/if}
{/if}
