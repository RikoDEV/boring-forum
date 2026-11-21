// JSON helper for the admin API. The SPA is served from the forum's origin, so the
// session cookie is sent automatically; writes send the CSRF token in a header.
//
// A write that needs the password confirmed again (`sudo-required`) opens the confirmation
// dialog and, once confirmed, is sent again: callers see only the final answer.

import { confirmSudo } from "./sudo.js";
import { csrf, ensureCsrf } from "./csrf.js";
import { locale, tr } from "./i18n.js";

export class ApiError extends Error {
  constructor(status, problem) {
    super(problem?.detail || problem?.title || tr("Request failed ({status})", { status }));
    this.status = status;
    // The member dismissed the password dialog: callers stop rather than ask again.
    this.sudo = problem?.type === "sudo-required";
  }
}

export async function api(method, path, body, retried = false) {
  const headers = { Accept: "application/json" };
  if (method !== "GET") {
    await ensureCsrf();
    headers["X-CSRF-Token"] = csrf();
  }
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const res = await fetch(`/api/v1/admin${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
    credentials: "same-origin",
  });
  if (res.status === 204) return null;
  const data = await res.json().catch(() => null);
  if (data?.type === "sudo-required" && !retried && (await confirmSudo())) return api(method, path, body, true);
  if (!res.ok) throw new ApiError(res.status, data);
  return data;
}

/// A multipart upload (plugin and theme packages) with the same CSRF and sudo handling.
export async function upload(path, body, retried = false) {
  await ensureCsrf();
  const res = await fetch(path, {
    method: "POST",
    headers: { "X-CSRF-Token": csrf() },
    body,
    credentials: "same-origin",
  });
  if (res.ok) return res.json().catch(() => null);
  const data = await res.json().catch(() => null);
  if (data?.type === "sudo-required" && !retried && (await confirmSudo())) return upload(path, body, true);
  throw new ApiError(res.status, data);
}

/// An image or file for the forum's uploads (icons, emoji): the stored file's `url` and size.
export function uploadFile(file) {
  const body = new FormData();
  body.append("file", file);
  return upload("/api/v1/uploads", body);
}

export const get = (p) => api("GET", p);
export const post = (p, b) => api("POST", p, b);
export const put = (p, b) => api("PUT", p, b);
export const patch = (p, b) => api("PATCH", p, b);
export const del = (p) => api("DELETE", p);

export function when(iso) {
  return iso ? new Date(iso).toLocaleString(locale()) : "—";
}
