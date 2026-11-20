// Distinct http(s) links in a piece of text, in order of first appearance.
export function extractUrls(text) {
  return [...new Set((text ?? "").match(/https?:\/\/[^\s)\]>]+/g) ?? [])];
}
