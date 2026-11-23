import { page } from "$app/state";

/// Opens a page on the tab its address names (`#tab=news`), now and whenever the address changes
/// while the page stays, as when the search picks another tab of the page it is on. Call it while
/// the page is being set up. `keys` are the tabs the page has; anything else is ignored.
export function followTabHash(keys, set) {
  $effect(() => {
    const key = /^#tab=(.+)$/.exec(page.url.hash)?.[1];
    if (key && keys.includes(key)) set(key);
  });
}
