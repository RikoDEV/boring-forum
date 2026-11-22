// The forum's full Markdown editor, for the panel's long texts. Its address comes with the
// site shell (see +layout.svelte); until then, or if the shell fails, the plain textarea stays.

import { ensureCsrf, publishCsrf } from "./csrf.js";

let found;
const url = new Promise((resolve) => (found = resolve));

export function setEditorUrl(u) {
  if (u) found(u);
}

/// Replaces the textarea with the editor and returns its CodeMirror view. The textarea keeps
/// the text and fires `input` on every change, so `bind:value` follows it.
export async function mountEditor(textarea) {
  // The editor reads its token from the meta tag (see publishCsrf); the shell normally set it already.
  publishCsrf(await ensureCsrf());
  const m = await import(/* @vite-ignore */ await url);
  return m.mount(textarea);
}

/// Swaps the whole text, as when another document is opened.
export function setText(view, text) {
  view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: text } });
}

/// Svelte action: gives a textarea the full editor. The textarea stays (hidden by the editor)
/// and keeps the text, so `bind:value` works; remount it with `{#key}` to load another text.
export function markdown(textarea) {
  let view = null;
  let gone = false;
  mountEditor(textarea)
    .then((v) => {
      if (gone) v.destroy?.();
      else view = v;
    })
    .catch(() => {}); // the plain textarea still works
  return {
    destroy() {
      gone = true;
      view?.destroy?.();
    },
  };
}
