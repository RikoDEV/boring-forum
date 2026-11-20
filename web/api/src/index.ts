// Typed API client generated from the server's OpenAPI document (`pnpm openapi`).
import createClient from "openapi-fetch";
import type { paths } from "./schema";

export type { paths, components } from "./schema";

export function createApi(options: { baseUrl?: string; adminToken?: string } = {}) {
  return createClient<paths>({
    baseUrl: options.baseUrl ?? "",
    headers: options.adminToken ? { Authorization: `Bearer ${options.adminToken}` } : undefined,
  });
}
