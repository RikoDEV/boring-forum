// Click counts beside the links in posts, as a small badge: "in-category 161".
//
// The server keeps a count for each link of a post (`/api/v1/topics/{id}/links`) and counts a click
// when the page sends a beacon. This file decides which links carry a badge and draws it.
import { el, tn } from "./dom.js";

const WEB_LINK = /^(?:https?:)?\/\//i;

/** The key of one link of one post in the table of counts. */
export const clickKey = (/** @type {string | number} */ post, /** @type {string} */ url) => `${post}\n${url}`;

/** 1234 becomes "1.2K", in the page's language. */
export function compact(/** @type {number} */ n, /** @type {string | undefined} */ locale) {
  return new Intl.NumberFormat(locale, { notation: "compact", maximumFractionDigits: 1 }).format(n);
}

/** The address a link was written with: a card made from a link keeps it, as the card's own is resolved. */
export const written = (/** @type {Element} */ a) =>
  /** @type {HTMLElement} */ (a).dataset?.url ?? a.getAttribute("href") ?? "";

/** The id of the post a link sits in. */
export function postOf(/** @type {Element} */ a) {
  const post = /** @type {HTMLElement | null} */ (a.closest(".post"));
  return (
    /** @type {HTMLElement | null | undefined} */ (post?.querySelector("[data-quote]"))?.dataset.quote ??
    post?.dataset.post
  );
}

/**
 * Whether a link is counted: one to a web address, in the text of a saved post. Not a picture's link
 * (its click opens the picture), a mention, an attachment, or anything in the composer's preview.
 */
export function countable(/** @type {Element} */ a) {
  if (!WEB_LINK.test(a.getAttribute("href") ?? "")) return false;
  if (a.matches(".mention, .hashtag, .attachment, [download]") || a.querySelector("img, picture, svg")) return false;
  return !a.closest(".composer-preview");
}

/** The badge: a short number to see, the full count in words for a screen reader and the tooltip. */
export function badge(/** @type {number} */ clicks, /** @type {string | undefined} */ locale) {
  const words = tn("count.clicks", clicks);
  const b = el("span", { class: "bf-badge bf-link-clicks", "data-tip": words });
  b.append(
    el("span", { "aria-hidden": "true" }, compact(clicks, locale)),
    el("span", { class: "visually-hidden" }, words),
  );
  return b;
}

/**
 * Puts a badge beside each counted link under `root` that has clicks. `counts` maps `clickKey` to the
 * number. A link made into a card carries its badge in the card's first line.
 */
export function decorate(
  /** @type {ParentNode} */ root,
  /** @type {Map<string, number>} */ counts,
  /** @type {string | undefined} */ locale = document.documentElement.lang || undefined,
) {
  for (const a of root.querySelectorAll(".cooked a[href]")) {
    const post = postOf(a);
    if (post === undefined || a.matches("[data-clicks]") || (!countable(a) && !a.matches(".bf-onebox"))) continue;
    const clicks = counts.get(clickKey(post, written(a)));
    if (!clicks) continue;
    a.setAttribute("data-clicks", String(clicks));
    const b = badge(clicks, locale);
    const line = a.matches(".bf-onebox") ? a.querySelector(".bf-onebox-text small") : null;
    if (line) line.append(" ", b);
    else a.after(b);
  }
}
