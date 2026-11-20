<script>
  /// An on/off setting: the panel's switch (`input.acp-switch`) with its label beside it. Several
  /// go in a <Section>, which lines them up. `check` draws a plain checkbox instead, for a cell
  /// of an `.acp-form` grid.

  /** @type {{ label?: any, checked?: boolean, check?: boolean, wide?: boolean, children?: import('svelte').Snippet, [key: string]: any }} */ let {
    label,
    checked = $bindable(false),
    check = false,
    wide = false,
    children,
    ...rest
  } = $props();
</script>

{#if check}
  <label class={["acp-check", wide && "acp-wide"]}>
    <input type="checkbox" bind:checked {...rest} />
    {label}{@render children?.()}
  </label>
{:else}
  <label class={["acp-group-style", wide && "acp-wide"]}>
    <input type="checkbox" class="acp-switch" role="switch" bind:checked {...rest} />
    {label}{@render children?.()}
  </label>
{/if}
