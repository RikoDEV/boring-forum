<script>
  /// The error and confirmation of a page's Status.
  let { status } = $props();
</script>

{#if status.error}<p class="err" role="alert">{status.error}</p>{/if}
{#if status.msg}<p class="ok" role="status">{status.msg}</p>{/if}
