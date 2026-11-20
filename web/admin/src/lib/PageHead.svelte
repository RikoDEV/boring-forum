<script>
  /// A page's title, its one-paragraph introduction (text, or a snippet when it holds links or
  /// code), actions on the right, and its Status below. A page whose tabs have different actions
  /// (or none) sets `reserve`, so the room for the buttons stays and the tabs below do not move.
  import Notices from "./Notices.svelte";

  /** @type {{ title: string, intro?: string | import('svelte').Snippet, status?: any, reserve?: boolean, children?: import('svelte').Snippet }} */
  let { title, intro = "", status = null, reserve = false, children } = $props();
</script>

<div class="acp-head">
  <div>
    <h1>{title}</h1>
    {#if typeof intro === "function"}<p>{@render intro()}</p>{:else if intro}<p>{intro}</p>{/if}
  </div>
  {#if reserve}<div class="acp-head-actions">{@render children?.()}</div>{:else}{@render children?.()}{/if}
</div>
{#if status}<Notices {status} />{/if}
