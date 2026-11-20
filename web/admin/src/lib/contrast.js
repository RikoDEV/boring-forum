// Colour helpers for the colour scheme editor: WCAG contrast of #rrggbb colours, and the text
// colour that reads on a button of a given colour (the same rule as the server's, in
// crates/bf-web/src/schemes.rs, so the preview matches the stylesheet).

export const isHex = (c) => /^#[0-9a-f]{6}$/i.test(c ?? "");

const luminance = (c) => {
  const [r, g, b] = [1, 3, 5]
    .map((i) => parseInt(c.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
};

/** Contrast ratio of two colours; null when either is not a #rrggbb colour. */
export function contrast(a, b) {
  if (!isHex(a) || !isHex(b)) return null;
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** White or near-black, whichever reads better on `accent`. */
export const onColour = (accent) =>
  !isHex(accent) || (contrast(accent, "#ffffff") ?? 0) >= (contrast(accent, "#0b0f14") ?? 0) ? "#ffffff" : "#0b0f14";
