import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import noPreload from "./no-preload.js";

// Member-only islands (the composer), loaded only on pages that have one, so they
// never count against the guest JS budget.
export default defineConfig({
  // Chunks are served from /assets/ (crates/bf-web/src/assets.rs), whatever page loads them.
  base: "/assets/",
  plugins: [svelte(), noPreload()],
  build: {
    outDir: "dist",
    emptyOutDir: false,
    target: "es2022",
    modulePreload: false,
    rollupOptions: {
      input: "src/members.js",
      preserveEntrySignatures: "exports-only",
      output: { format: "es", entryFileNames: "members.js", chunkFileNames: "m-[name]-[hash].js" },
    },
  },
});
