// A moment in words, like the forum's own "3 hr. ago": relative within 30 days, then the date.
// Pure (language and clock are arguments) so it can be tested; pages use `<When>`.
export function agoText(iso, lang, now = Date.now()) {
  const d = new Date(iso);
  if (!iso || Number.isNaN(+d)) return "";
  const rel = new Intl.RelativeTimeFormat(lang, { numeric: "auto", style: "short" });
  const s = (now - d.getTime()) / 1000;
  if (s < 0) return new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "short" }).format(d);
  if (s < 60) return rel.format(0, "second");
  if (s < 3600) return rel.format(-Math.floor(s / 60), "minute");
  if (s < 86400) return rel.format(-Math.floor(s / 3600), "hour");
  if (s < 2_592_000) return rel.format(-Math.floor(s / 86400), "day");
  return new Intl.DateTimeFormat(lang, { dateStyle: "medium" }).format(d);
}
