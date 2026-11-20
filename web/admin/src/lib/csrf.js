// The CSRF token the forum sets in a cookie; every admin write sends it in `X-CSRF-Token`.
export function csrf() {
  return document.cookie.match(/(?:^|;\s*)__Host-bf_csrf=([0-9a-f]{64})/)?.[1] ?? "";
}
