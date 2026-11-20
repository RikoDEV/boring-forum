<script>
  // A date as relative words with the full date and time on hover.
  import { locale } from "./i18n.js";
  import { when } from "./api.js";
  import { agoText } from "./ago.js";

  /** @type {{ iso?: string | null }} */ let { iso } = $props();
</script>

{#if iso}<time datetime={iso} title={when(iso)}>{agoText(iso, locale())}</time>{:else}—{/if}
