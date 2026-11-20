import { loadShell } from "#lib/i18n.js";

export const ssr = false;
export const prerender = false;

// The messages (and the site's header) arrive before the first page renders.
export const load = () => loadShell();
