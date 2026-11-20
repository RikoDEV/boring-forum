<script>
  import { tr } from "#lib/i18n.js";
  // A dropdown over a tree (forums, categories), indented by depth. `kind` greys out rows of
  // other kinds (pick only a "forum"); `exclude` leaves one row out (a node cannot be its own
  // parent). `none` is the first option, with the value `noneValue`. `asString` keeps ids as
  // strings, for forms that store them that way. Without `label`, it is a bare select.
  import { orderTree, indented } from "./tree.js";

  let {
    rows = [],
    value = $bindable(),
    label = "",
    none = "",
    noneValue = /** @type {string | number} */ (""),
    kind = undefined,
    exclude = undefined,
    asString = false,
    required = false,
    id = undefined,
    wide = false,
  } = $props();

  const ordered = $derived(orderTree(rows).filter((r) => r.id !== exclude));
  const val = (r) => (asString ? String(r.id) : r.id);
  // A saved id that is no longer in the tree (a deleted forum) still shows, so it can be changed.
  const missing = $derived(
    value != null &&
      value !== "" &&
      value !== noneValue &&
      rows.length > 0 &&
      !rows.some((r) => String(r.id) === String(value)),
  );
</script>

{#snippet options()}
  {#if none}<option value={noneValue}>{none}</option>{/if}
  {#each ordered as r (r.id)}<option value={val(r)} disabled={kind !== undefined && r.kind !== kind}>
      {indented(r)}
    </option>{/each}
  {#if missing}<option {value}>{tr("Deleted #{value}", { value })}</option>{/if}
{/snippet}

{#if label}
  <label class="bf-field" class:acp-wide={wide}>
    <select {id} bind:value {required}>{@render options()}</select><span class="bf-field-label">{label}</span>
  </label>
{:else}
  <select {id} bind:value {required}>{@render options()}</select>
{/if}
