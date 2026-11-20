<script>
  /// A group, titled by `legend` when it needs one, inside a box or dialog (`fieldset.acp-picker`): pickers, switches, previews,
  /// anything that is not a row of fields. `grid` lays its children out as an `.acp-form` grid
  /// (fields of one kind per row); `switches` stacks <Switch>es; `hint` is a line under it all.

  /** @type {{ legend?: string, grid?: boolean, switches?: boolean, hint?: string, children?: import('svelte').Snippet }} */ let {
    legend = "",
    grid = false,
    switches = false,
    hint = "",
    children,
  } = $props();
</script>

<fieldset class={["acp-picker", !legend && "is-plain"]}>
  {#if legend}<legend>{legend}</legend>{/if}
  {#if grid}<div class="acp-form acp-form-flat">{@render children()}</div>
  {:else if switches}<div class="acp-switches">{@render children()}</div>
  {:else}{@render children()}{/if}
  {#if hint}<small class="muted acp-picker-hint">{hint}</small>{/if}
</fieldset>
