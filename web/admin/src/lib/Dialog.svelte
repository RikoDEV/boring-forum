<script>
  import { tr } from "#lib/i18n.js";
  // The site's modal dialog (`.bf-dialog`, themes/default/css/components/modal.css; the site
  // builds the same thing in web/islands/src/dialog.js) for the panel's editors. It opens when
  // mounted, so a page shows it with `{#if draft}<Dialog …>`; closing (×, Escape, a click
  // beside it) calls `onclose`, and the page drops its draft.
  //
  // Children are the form: put the fields in `.bf-dialog-body` and the buttons in
  // `.bf-dialog-actions`. `extra` renders in the header beside the title.
  import { iconHref } from "./icons.js";
  import { firstField } from "../../../islands/src/dialog.js";

  /** @type {{ title: string, size?: string, onclose?: () => any, extra?: import('svelte').Snippet, children?: import('svelte').Snippet }} */
  let { title, size = "", onclose, extra, children } = $props();
  let d;
  let opener = null;

  $effect(() => {
    opener = document.activeElement;
    d.showModal();
    firstField(d)?.focus();
    return () => opener?.isConnected && opener.focus();
  });
</script>

<dialog
  bind:this={d}
  class="bf-dialog {size}"
  aria-label={title}
  onclose={() => onclose?.()}
  onclick={(e) => e.target === d && d.close()}
>
  <header class="bf-dialog-head">
    <h2>{title}</h2>
    {@render extra?.()}
    <button type="button" class="acp-ghost acp-dialog-x" aria-label={tr("Close")} onclick={() => d.close()}>
      <svg class="bf-icon" aria-hidden="true"><use href={iconHref("x")}></use></svg>
    </button>
  </header>
  {@render children()}
</dialog>
