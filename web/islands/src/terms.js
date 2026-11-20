// Updated terms. Until a member accepts them, writing is refused on the server
// (`moderation::gate`); everything else stays open. This dialog asks once per visit, steps aside
// for "Not now" (the layout's banner stays), never covers the pages a member must always reach
// (settings, data export, closing the account, the legal texts, appeals), and comes back the
// moment the member tries to write.
import { dialog } from "./dialog.js";
import { el, tr } from "./dom.js";

// Parsing only, into an inert document: the site's own accept page.
const policy = window.trustedTypes?.createPolicy("bf-terms", { createHTML: (s) => s }) ?? { createHTML: (s) => s };

// Pages that never open the dialog by themselves.
const OPEN_PAGES = /^\/(settings|legal|statements|contact|transparency)(\/|$)/;
// Writing: the composers, replying and quoting, reactions and votes, messages.
const WRITE_FORMS = /^\/(t\/\d+\/(reply|vote-topic)|c\/\d+\/new|post\/\d+\/(like|react|vote)|messages(\/|$))/;
const WRITE_CLICKS = '[data-quote], [data-reply], a[href="#new-topic"], a[href="#reply"]';

const LATER = "bf-terms-later";
const later = () => {
  try {
    return sessionStorage.getItem(LATER) === "1";
  } catch {
    return false;
  }
};
const setLater = () => {
  try {
    sessionStorage.setItem(LATER, "1");
  } catch {
    /* private mode: the dialog just asks again on the next page */
  }
};

// Where accepting returns to: this page, or for a refused form, the page the form was on.
const returnTo = () => {
  if (document.querySelector("[data-error]") && document.referrer) {
    try {
      const r = new URL(document.referrer);
      if (r.origin === location.origin) return r.pathname + r.search + r.hash;
    } catch {
      /* fall through */
    }
  }
  return location.pathname + location.search + location.hash;
};

async function build() {
  const res = await fetch("/legal/accept", { redirect: "manual" }).catch(() => null);
  if (!res?.ok) return null; // nothing pending any more
  const doc = new DOMParser().parseFromString(policy.createHTML(await res.text()), "text/html");
  const page = doc.querySelector("main article");
  const form = page?.querySelector("form.bf-terms-accept");
  if (!page || !form) return null;

  const intro = el(
    "p",
    { class: "muted" },
    tr("Until you accept them you can still read, change your settings and download your data, but not post."),
  );
  const text = el("div", { class: "bf-terms-text cooked", tabindex: "0", role: "region", "aria-label": tr("Terms") });
  text.append(...[...(page.querySelector(".cooked")?.childNodes ?? [])].map((n) => document.adoptNode(n)));

  const accept = document.adoptNode(form);
  const decline = el("a", { class: "btn", href: "/settings#s-privacy" }, tr("I don't accept"));
  const notNow = el("button", { type: "button", class: "btn" }, tr("Not now"));
  let accepted = false;
  // Escape is "Not now"; a click beside it does nothing, so it is not lost by accident.
  const box = dialog({
    title: page.querySelector("h1")?.textContent.trim() || tr("Updated terms"),
    className: "is-large",
    body: [intro, text],
    actions: [decline, notNow, accept],
    closeButton: false,
    backdrop: false,
    keep: true,
    onClose: () => !accepted && setLater(),
  });
  notNow.addEventListener("click", () => box.close("later"));
  // Declining leads to the account's data export and closing, where the dialog does not follow.
  decline.addEventListener("click", () => setLater());
  accept.addEventListener("submit", () => {
    accepted = true;
    accept.querySelector('input[name="next"]').value = returnTo();
  });
  return {
    open() {
      if (box.element.open) return;
      box.show();
      accept.querySelector("button")?.focus();
    },
  };
}

export async function termsGate() {
  if (typeof HTMLDialogElement === "undefined") return;
  const gate = await build();
  if (!gate) return;

  // Asked once per visit, and not on the pages that must stay open.
  if (!later() && !OPEN_PAGES.test(location.pathname)) gate.open();

  // The banner's button.
  document.addEventListener("click", (e) => {
    if (e.target.closest?.("[data-terms-open]")) {
      e.preventDefault();
      gate.open();
    }
  });
  // Starting to write: the dialog instead of a refusal later.
  const stop = (e) => {
    e.preventDefault();
    e.stopImmediatePropagation();
    gate.open();
  };
  document.addEventListener(
    "click",
    (e) => {
      if (e.target.closest?.(WRITE_CLICKS)) stop(e);
    },
    true,
  );
  document.addEventListener(
    "submit",
    (e) => {
      const form = /** @type {HTMLFormElement} */ (e.target);
      const action = new URL(form.action, location.href);
      if (
        form.method === "post" &&
        action.origin === location.origin &&
        (form.matches(".composer") || WRITE_FORMS.test(action.pathname))
      )
        stop(e);
    },
    true,
  );
  document.addEventListener(
    "focusin",
    (e) => {
      const composer = e.target.closest?.("form.composer");
      if (!composer) return;
      e.target.blur?.();
      gate.open();
    },
    true,
  );
  // Any other write that the server refuses for the terms opens the dialog too.
  const fetch0 = window.fetch;
  window.fetch = async (...args) => {
    const res = await fetch0(...args);
    if (res.status === 422) {
      res
        .clone()
        .text()
        .then((t) => t.includes("/legal/accept") && gate.open())
        .catch(() => {});
    }
    return res;
  };
}
