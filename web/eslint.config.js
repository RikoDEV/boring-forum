// Flat config for the front end: islands (browser scripts + Svelte), the admin SPA (Svelte 5)
// and the Node scripts and tests. Formatting is Prettier's job (`pnpm format:check`), so no
// stylistic rules here.
import js from "@eslint/js";
import globals from "globals";
import svelte from "eslint-plugin-svelte";

const rules = {
  "no-unused-vars": [
    "error",
    { args: "after-used", argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrors: "none" },
  ],
  eqeqeq: ["error", "always", { null: "ignore" }],
  "prefer-const": "error",
  "no-var": "error",
  "no-shadow-restricted-names": "error",
  "no-return-assign": ["error", "except-parens"],
  "no-throw-literal": "error",
  "no-implicit-globals": "error",
  "no-eval": "error",
  "no-new-func": "error",
  "no-script-url": "error",
  "no-await-in-loop": "off",
  "require-atomic-updates": "off",
  "no-useless-assignment": "error",
  "no-empty": ["error", { allowEmptyCatch: true }],
};

export default [
  { ignores: ["**/node_modules/**", "**/dist/**", "**/build/**", "**/.svelte-kit/**", "api/src/schema.d.ts"] },
  js.configs.recommended,
  ...svelte.configs["flat/recommended"],
  {
    languageOptions: { ecmaVersion: "latest", sourceType: "module", globals: { ...globals.browser } },
    rules,
  },
  {
    files: ["**/*.svelte", "**/*.svelte.js"],
    rules: {
      ...rules,
      // In Svelte 5, `let { a } = $props()` is the only way to declare a prop: core prefer-const
      // would flag it, the Svelte-aware rule does not.
      "prefer-const": "off",
      "svelte/prefer-const": "error",
      // Lists here are static admin tables that are replaced whole, never reordered in place.
      "svelte/require-each-key": "off",
      // The admin SPA is a static single-page app mounted under /admin with a base path.
      "svelte/no-navigation-without-resolve": "off",
    },
  },
  {
    files: ["scripts/**", "**/test/**", "**/vite*.config.js", "eslint.config.js", "islands/no-preload.js"],
    languageOptions: { globals: { ...globals.node } },
  },
];
