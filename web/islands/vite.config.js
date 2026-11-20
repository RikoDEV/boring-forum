import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import noPreload from "./no-preload.js";

// One ES module, embedded into the Rust binary by crates/bf-web/build.rs.
// Not library mode: that keeps whitespace in ES output, which costs ~1.5 KB gzipped.
export default defineConfig({
  // Chunks are served from /assets/ (crates/bf-web/src/assets.rs), whatever page loads them.
  base: "/assets/",
  plugins: [svelte(), noPreload()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
    target: "es2022",
    modulePreload: false,
    rollupOptions: {
      input: "src/main.js",
      preserveEntrySignatures: "exports-only",
      output: { format: "es", entryFileNames: "islands.js", chunkFileNames: "g-[name]-[hash].js" },
    },
  },
});
