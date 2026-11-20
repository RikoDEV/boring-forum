// Types for what the islands read from and put on `window`, and a friendlier default for
// `querySelector`: the islands only ever query for elements they then use as HTMLElements
// (`.dataset`, `.hidden`, `.focus()`), so an untyped query yields HTMLElement, not Element.
// A call with a tag name or a type argument (`querySelector("input")`,
// `querySelector<SVGElement>(…)`) is still typed by the DOM library as usual.

interface TrustedTypePolicy {
  createHTML(input: string): string;
  createScriptURL(input: string): string;
}

interface Window {
  /** Trusted Types, where the browser has them. */
  trustedTypes?: {
    createPolicy(
      name: string,
      rules: { createHTML?: (s: string) => string; createScriptURL?: (s: string) => string },
    ): TrustedTypePolicy;
  };
  /** Set once the dialog click handler is installed (dialog.js), so two bundles do not add it twice. */
  bfDialogs?: boolean;
  /** Plugins register here (see the plugin API). */
  bfPlugins?: any;
  bfComposerButtons?: any[];
  /** Analytics hooks, defined by whichever script the site has enabled. */
  bfTrack?: (name: string, props?: Record<string, unknown>) => void;
  plausible?: any;
  umami?: any;
  gtag?: any;
  dataLayer?: any[];
  _paq?: any[];
}

interface ParentNode {
  querySelector<K extends keyof HTMLElementTagNameMap>(selectors: K): HTMLElementTagNameMap[K] | null;
  querySelectorAll<K extends keyof HTMLElementTagNameMap>(selectors: K): NodeListOf<HTMLElementTagNameMap[K]>;
  querySelector(selectors: `meta[${string}`): HTMLMetaElement | null;
  querySelector(selectors: `input[${string}`): HTMLInputElement | null;
  querySelector(selectors: `form[${string}`): HTMLFormElement | null;
  querySelector(selectors: `a[${string}`): HTMLAnchorElement | null;
  querySelector(selectors: `details[${string}`): HTMLDetailsElement | null;
  querySelector<E extends Element = HTMLElement>(selectors: string): E | null;
  querySelectorAll(selectors: `input[${string}`): NodeListOf<HTMLInputElement>;
  querySelectorAll(selectors: `details${"." | "["}${string}`): NodeListOf<HTMLDetailsElement>;
  querySelectorAll<E extends Element = HTMLElement>(selectors: string): NodeListOf<E>;
}

/** The page's own events (`document.dispatchEvent(new CustomEvent("bf:…", { detail }))`). */
interface DocumentEventMap {
  "bf:post-replaced": CustomEvent;
  "bf:posts-added": CustomEvent;
  "bf:composer-button": CustomEvent;
}

/** Event targets are not always elements (a text node, the document), so the islands call
 *  these with `?.`; declaring them optional lets `e.target.closest?.(…)` check. */
interface EventTarget {
  closest?(selectors: `form${"." | "["}${string}`): HTMLFormElement | null;
  closest?(selectors: `a${"." | "["}${string}`): HTMLAnchorElement | null;
  closest?(selectors: string): HTMLElement | null;
  matches?(selectors: string): boolean;
  blur?(): void;
}
