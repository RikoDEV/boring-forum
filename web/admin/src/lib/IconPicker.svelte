<script>
  // The site's icon picker (web/islands/src/icon-picker.js), so the panel and the site offer the
  // same one. `brands` adds the brand logos; `none` offers "no icon".
  import { onMount } from "svelte";
  import { iconPicker } from "../../../islands/src/icon-picker.js";
  import { BRANDS, ICONS, brandNames, iconHref } from "./icons.js";

  let { value = $bindable(""), name = "icon", brands = true, none = true } = $props();
  let host;
  let picker = null;

  onMount(() => {
    picker = iconPicker({
      value: value ?? "",
      name,
      icons: ICONS,
      brands: brands ? BRANDS : [],
      brandNames,
      href: iconHref,
      none,
      onChange: (v) => (value = v),
    });
    // The picker is a plain-DOM widget shared with the site; Svelte only gives it a host.
    // eslint-disable-next-line svelte/no-dom-manipulating
    host.append(picker.element);
    // Opened inside a dialog that shows just after: a frame later it has a layout to measure.
    picker.reveal();
    return () => picker.element.remove();
  });
  $effect(() => {
    picker?.set(value ?? "");
  });
</script>

<div bind:this={host}></div>
