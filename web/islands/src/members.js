// Member-only islands. Loaded by the page loader only when one of these is present; each
// island is its own chunk, so a page pays only for the islands it has.
import { mount as svelteMount } from "svelte";

const islands = {
  composer: () => import("./Composer.svelte"),
  "passkey-register": () => import("./PasskeyRegister.svelte"),
  "passkey-login": () => import("./PasskeyLogin.svelte"),
};

/**
 * Starts a member-only island (the composer, passkey forms) on a `[data-island]` element.
 * @param {string} name
 * @param {HTMLElement} target
 */
export function mount(name, target) {
  const load = islands[name];
  if (!load) return false;
  let props = {};
  try {
    props = JSON.parse(target.dataset.props || "{}");
  } catch {
    // Bad props render the island with defaults instead of breaking the page.
  }
  load().then((m) => svelteMount(m.default, { target, props }));
  return true;
}
