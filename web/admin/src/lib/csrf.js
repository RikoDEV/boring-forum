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

/** The token loaded by `ensureCsrf` (empty before that). */
export function csrf() {
  return token;
}
