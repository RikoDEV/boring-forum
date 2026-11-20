import { defineConfig } from "vite";

// The editor bundle, loaded on demand when someone starts writing (< 127 KB gz).
export default defineConfig({
  // Chunks are served from /assets/ (crates/bf-web/src/assets.rs), whatever page loads them.
  base: "/assets/",
  build: {
    outDir: "dist",
    emptyOutDir: false,
    target: "es2022",
    modulePreload: false,
    rollupOptions: {
      input: "src/editor/main.js",
      preserveEntrySignatures: "exports-only",
      output: { format: "es", entryFileNames: "editor.js", codeSplitting: false },
    },
  },
});
