<script>
  // A member's name, linked to their public profile, as everywhere on the forum pages.
  // `fallback` stands in when there is no name (a system action, a deleted account).

  /** @type {{ name?: string | null, fallback?: string }} */ let { name, fallback = "—" } = $props();
</script>

{#if name && name !== "deleted"}<a class="bf-user-link" href="/u/{encodeURIComponent(name.toLowerCase())}">
    {name}
  </a>{:else}{name || fallback}{/if}
