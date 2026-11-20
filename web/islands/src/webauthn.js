import { jsonHeaders, tr } from "./dom.js";

// base64url <-> bytes for WebAuthn JSON, and the CSRF token from the page.
export const toBytes = (s) => Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
export const toB64 = (buf) =>
  btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
export const supported = () => typeof window !== "undefined" && "PublicKeyCredential" in window;

/**
 * POSTs JSON with the CSRF token and returns the parsed answer; throws with the server's message.
 * @param {string} url
 * @param {unknown} [body]
 */
export async function postJson(url, body) {
  const res = await fetch(url, {
    method: "POST",
    headers: jsonHeaders(),
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok && !res.redirected)
    throw Object.assign(new Error(data.detail || data.title || tr("Request failed")), { data });
  return { res, data };
}
