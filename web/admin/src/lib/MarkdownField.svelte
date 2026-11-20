<script>
  /// A long Markdown text, written in the forum's own post editor (the same toolbar, preview and
  /// shortcuts members get), titled by `label` like any group of fields. The textarea underneath
  /// keeps the text, so `bind:value` follows the editor. It is not `required`: the editor hides
  /// the textarea, so a page checks for empty text itself. Remount it with `{#key}` to load
  /// another text.
  import { tr } from "#lib/i18n.js";
  import { markdown } from "#lib/editor.js";
  import Section from "./Section.svelte";

  /** @type {{ label: string, value?: string, rows?: number | string, hint?: string, maxlength?: number | string }} */
  let { label, value = $bindable(""), rows = 8, hint = "", maxlength = undefined } = $props();
</script>

<div class="acp-wide acp-md">
  <Section legend={label} {hint}>
    <textarea use:markdown bind:value {rows} {maxlength} aria-label={label} placeholder={tr("Markdown")}></textarea>
  </Section>
</div>
