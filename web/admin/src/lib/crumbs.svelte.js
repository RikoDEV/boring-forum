/// What a page adds to the breadcrumb after its own name, for a detail view it opens itself (a
/// theme, a member). `back` is what the page's own crumb does when clicked, so it can close that
/// view; leave it null when the detail view is a route of its own (the crumb is then a link).
export const crumbs = $state({ extra: [], back: null });

/// Call from an $effect: `$effect(() => trailOf(selected ? [{ label: selected.name }] : [], close))`.
/// Returns what to run when the page goes away.
export function trailOf(extra, back = null) {
  crumbs.extra = extra;
  crumbs.back = back;
  return () => {
    crumbs.extra = [];
    crumbs.back = null;
  };
}
