<script>
  import { tr } from "#lib/i18n.js";
  /// The panel's editor: a <Dialog> holding a form, its fields, the error from `status`, and the
  /// buttons (Delete on the start side when `ondelete` is given, Cancel, Save). Show it with
  /// `{#if draft}<EditDialog …>`; Cancel, × and Escape call `onclose`. Children are the body:
  /// <Section>s, or a `<div class="acp-form">` grid of <Field>s.
  import Dialog from "./Dialog.svelte";

  /** @type {{ title: string, onsubmit: (e: SubmitEvent) => any, onclose: () => any, status?: any, save?: string, busy?: boolean, ondelete?: (() => any) | null, deleteLabel?: string, size?: string, extra?: import('svelte').Snippet, children?: import('svelte').Snippet }} */
  let {
    title,
    onsubmit,
    onclose,
    status = null,
    save = tr("Save"),
    busy = false,
    ondelete = null,
    deleteLabel = tr("Delete"),
    size = "",
    extra,
    children,
  } = $props();
</script>

<Dialog {title} {size} {onclose} {extra}>
  <form {onsubmit}>
    <div class="bf-dialog-body acp-dialog-body">
      {@render children()}
      {#if status?.error}<p class="err" role="alert">{status.error}</p>{/if}
    </div>
    <div class="bf-dialog-actions">
      {#if ondelete}<button type="button" class="is-danger bf-dialog-aside" onclick={ondelete}>{deleteLabel}</button
        >{/if}
      <button type="button" class="secondary" onclick={() => onclose?.()}>{tr("Cancel")}</button>
      <button disabled={busy}>{save}</button>
    </div>
  </form>
</Dialog>
