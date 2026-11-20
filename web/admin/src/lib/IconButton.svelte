<script>
  /// A square button with only an icon; its `label` is the accessible name and the tooltip.
  import { iconHref } from "./icons.js";

  /** @type {{ icon: string, label: string, danger?: boolean, flip?: boolean, [key: string]: any }} */
  let { icon, label, danger = false, flip = false, ...rest } = $props();
</script>

<button type="button" class={["secondary acp-icon-btn", danger && "is-danger"]} aria-label={label} {...rest}>
  <svg class={["bf-icon", flip && "acp-flip"]} aria-hidden="true"><use href={iconHref(icon)}></use></svg>
</button>
