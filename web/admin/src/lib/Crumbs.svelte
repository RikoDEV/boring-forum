<script>
  /// The tiny breadcrumb above every page: "Administration › Content › Forums". `items` run from the
  /// panel's root to the page; the last is the page itself. An item is a link (`href`), a view the
  /// page switches itself (`onclick`), or plain text (a section of the menu).
  import { tr } from "#lib/i18n.js";
  import sprite from "../../../../themes/default/icons.svg?url";

  /** @type {{ items: { label: string, href?: string, onclick?: () => void }[] }} */
  let { items } = $props();
</script>

{#if items.length}
  <nav class="acp-crumbs" aria-label={tr("Breadcrumb")}>
    {#each items as c, i}
      {#if i === items.length - 1}<span aria-current="page">{c.label}</span>
      {:else}
        {#if c.onclick}<button type="button" class="acp-link" onclick={c.onclick}>{c.label}</button>
        {:else if c.href}<a href={c.href}>{c.label}</a>
        {:else}<span class="acp-crumb-group">{c.label}</span>{/if}
        <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#chevron-right"></use></svg>
      {/if}
    {/each}
  </nav>
{/if}
