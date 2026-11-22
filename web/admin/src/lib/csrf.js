// The CSRF token; every admin write sends it in `X-CSRF-Token`. Its cookie is HttpOnly, so the panel
// asks the forum for the token once and keeps it for the life of the page.
let token = "";
let loading = null;

/** Loads the token (once); call before the first write. */
export function ensureCsrf() {
  if (token) return Promise.resolve(token);
  loading ??= fetch("/api/v1/csrf", { credentials: "same-origin", headers: { Accept: "application/json" } })
    .then((res) => (res.ok ? res.json() : null))
    .then((data) => {
      token = data?.token ?? "";
      return token;
    })
    .finally(() => {
      loading = null;
    });
  return loading;
}

/** Puts the token where the site's own scripts look for it: a `csrf-token` meta tag, as on the site's
 * pages. The header's menus and the Markdown editor read it from there; without it their requests
 * go out with no token and the server answers "Your session expired". */
export function publishCsrf(value) {
  if (!value || typeof document === "undefined") return;
  let meta = document.querySelector('meta[name="csrf-token"]');
  if (!meta) {
    meta = document.createElement("meta");
    meta.name = "csrf-token";
    document.head.append(meta);
  }
  meta.content = value;
}

/** The token loaded by `ensureCsrf` (empty before that). */
export function csrf() {
  return token;
}
