<script>
  import { tr } from "#lib/i18n.js";
  import { isHex, onColour } from "#lib/contrast.js";
  /// A miniature forum page drawn with one palette (light or dark): the header, a card with a link,
  /// text, secondary text and buttons, and the status chips. It sets the design tokens on its own
  /// box, the base colours as they are and the derived ones with the server's formulas
  /// (`base` and `shades` come from GET /color-schemes), so it shows what the stylesheet says,
  /// whatever the panel itself uses. `compact` is the card-sized version for a list of themes.

  /** @type {{ palette: Record<string,string>, mode: "light" | "dark", title?: string, label?: string, base: any[], shades: any[], compact?: boolean }} */
  let { palette, mode, title = "", label = "", base, shades, compact = false } = $props();

  function tokens(node, args) {
    const apply = ({ map, mode, base, shades }) => {
      node.style.cssText = "";
      node.style.colorScheme = mode;
      for (const b of base) if (isHex(map?.[b.key])) node.style.setProperty(`--bf-color-${b.token}`, map[b.key]);
      node.style.setProperty("--bf-color-accent-text", onColour(map?.accent));
      node.style.setProperty("--bf-color-focus", "var(--bf-color-accent)");
      for (const d of shades) node.style.setProperty(`--bf-color-${d.token}`, d.formula);
    };
    apply(args);
    return { update: apply };
  }
</script>

<div class={["acp-scheme-preview", compact && "is-compact"]} use:tokens={{ map: palette, mode, base, shades }}>
  <div class="acp-sp-head"><b>{title}</b><span>{label}</span></div>
  <div class="acp-sp-page">
    <div class="acp-sp-card">
      <a href="#preview" tabindex={compact ? -1 : 0} onclick={(e) => e.preventDefault()}>{tr("A topic title that is a link")}</a>
      <p>{tr("Body text sits on a card. It should be easy to read.")}</p>
      {#if !compact}<small>{tr("Secondary text: who posted and when.")}</small>{/if}
      <div class="acp-sp-actions">
        <span class="acp-sp-btn is-primary">{tr("Reply")}</span>
        <span class="acp-sp-btn">{tr("Cancel")}</span>
      </div>
    </div>
    <div class="acp-sp-chips">
      <span class="is-ok">{tr("Success")}</span><span class="is-warn">{tr("Warning")}</span><span class="is-bad">{tr("Danger")}</span>
      <span class="is-soft">{tr("Selected")}</span>
    </div>
  </div>
</div>
