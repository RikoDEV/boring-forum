<script>
  /// A panel box (`section.acp-box`) with its heading and optional actions on the right. The body
  /// is the children: a table, `.acp-rows`, or a `<div class="acp-form">` grid.

  /** @type {{ title?: string, actions?: import('svelte').Snippet, children?: import('svelte').Snippet }} */ let {
    title = "",
    actions,
    children,
  } = $props();
</script>

<section class="acp-box">
  {#if title || actions}<div class="acp-box-head">
      {#if title}<h2>{title}</h2>{/if}{@render actions?.()}
    </div>{/if}
  {@render children()}
</section>
