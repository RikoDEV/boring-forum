<script>
  /// The bar that appears while settings have unsaved changes.
  import { tn, tr } from "#lib/i18n.js";

  /** @type {{ form: import("#lib/settings-form.svelte.js").SettingsForm }} */
  let { form } = $props();
</script>

{#if form.dirty.length}
  <div class="acp-savebar" role="region" aria-label={tr("Unsaved changes")}>
    <span>{tn("admin.count.unsaved", form.dirty.length)}</span>
    <button type="button" class="secondary" disabled={form.saving} onclick={form.discard}>{tr("Discard")}</button>
    <button type="button" disabled={form.saving} onclick={form.save}>
      {form.saving ? tr("Saving…") : tr("Save changes")}
    </button>
  </div>
{/if}
