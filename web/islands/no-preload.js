// Vite wraps every dynamic import in a preload helper (about 0.6 KB gzipped) that would fetch the
// chunk's dependencies ahead of time. These bundles have no CSS and every dependency list it gets
// is empty, so plain import() does the same: this unwraps the calls before minification, and
// the then-unused helper is dropped.
const WRAPPED =
  /__vitePreload\(\s*\(\)\s*=>\s*(import\(\s*["'`][^"'`]+["'`]\s*\))\s*,\s*(?:\[\s*\]|void 0|__VITE_PRELOAD__)(?:\s*,\s*import\.meta\.url)?\s*\)/g;

export default function noPreload() {
  return {
    name: "bf:no-preload",
    enforce: "post",
    renderChunk(code) {
      if (!code.includes("__vitePreload")) return null;
      return { code: code.replace(WRAPPED, "$1"), map: null };
    },
  };
}
