// Text emoticons that become emoji as people type (the `emoticons_to_emoji` setting; the page carries
// `data-emoticons="on"` when it is switched on). One module for every place text is written: the
// post editor (CodeMirror) and the plain text boxes (chat, statuses and their replies).

/** The emoticons that change, and what they become. Each counts only as a word of its own. */
export const EMOTICONS = {
  ":)": "\u{1F642}",
  ":-)": "\u{1F642}",
  ":(": "\u{1F641}",
  ":-(": "\u{1F641}",
  ":D": "\u{1F604}",
  ":-D": "\u{1F604}",
  ";)": "\u{1F609}",
  ";-)": "\u{1F609}",
  ":P": "\u{1F61B}",
  ":-P": "\u{1F61B}",
  ":p": "\u{1F61B}",
  ":-p": "\u{1F61B}",
  ":O": "\u{1F62E}",
  ":-O": "\u{1F62E}",
  ":o": "\u{1F62E}",
  ":/": "\u{1F615}",
  ":-/": "\u{1F615}",
  ":'(": "\u{1F622}",
  ":*": "\u{1F618}",
  ":|": "\u{1F610}",
  ">:(": "\u{1F620}",
  xD: "\u{1F606}",
  XD: "\u{1F606}",
  "<3": "\u2764\uFE0F",
};

/** Typing one of these after an emoticon ends it. */
export const ENDS = /^[\s.,!?]$/;

export const emoticonsOn = () => document.documentElement.dataset.emoticons === "on";

/**
 * The emoticon that ends at `end` in `text`: [where it starts, its emoji], or null. It must start
 * the text or follow a space, so `a:)b`, `10:30` and `http://x` are never touched.
 * @param {string} text @param {number} end
 * @returns {[number, string] | null}
 */
export function emoticonBefore(text, end) {
  let start = end;
  while (start > 0 && !/\s/.test(text[start - 1])) start--;
  const emoji = Object.hasOwn(EMOTICONS, text.slice(start, end)) ? EMOTICONS[text.slice(start, end)] : undefined;
  return emoji ? [start, emoji] : null;
}

/** Plain text boxes: the emoticon before the caret changes when a space, a full stop or Enter ends it. */
export function emoticonsInTextBoxes() {
  const swap = (/** @type {HTMLTextAreaElement} */ box, /** @type {number} */ end) => {
    const hit = emoticonBefore(box.value, end);
    if (!hit) return;
    const caret = box.selectionStart;
    box.setRangeText(hit[1], hit[0], end);
    const moved = caret + hit[1].length - (end - hit[0]);
    box.setSelectionRange(moved, moved);
  };
  document.addEventListener("input", (e) => {
    const box = e.target;
    // CodeMirror mirrors its text into a hidden box with a plain `input` event; only typing counts.
    if (!emoticonsOn() || !(box instanceof HTMLTextAreaElement) || !(e instanceof InputEvent)) return;
    if (e.inputType === "insertText" && e.data && ENDS.test(e.data)) swap(box, box.selectionStart - 1);
  });
  document.addEventListener(
    "keydown",
    (e) => {
      const box = e.target;
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing && emoticonsOn() && box instanceof HTMLTextAreaElement)
        swap(box, box.selectionStart);
    },
    true,
  );
  // One typed last, with nothing after it, as Send is pressed.
  document.addEventListener(
    "submit",
    (e) => {
      if (!emoticonsOn() || !(e.target instanceof HTMLFormElement)) return;
      for (const box of e.target.querySelectorAll("textarea")) {
        const hit = emoticonBefore(box.value, box.value.length);
        if (hit) box.value = box.value.slice(0, hit[0]) + hit[1];
      }
    },
    true,
  );
}
