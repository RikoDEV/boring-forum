import { sveltekit } from "@sveltejs/kit/vite";
import { defineConfig } from "vite";
import adapter from "@sveltejs/adapter-static";

export default defineConfig({
  // Static SPA served by the Rust server under /admin.
  plugins: [sveltekit({ adapter: adapter({ fallback: "index.html" }), paths: { base: "/admin" } })],
  server: { proxy: { "/api": "http://127.0.0.1:8080" } },
});
