(window.bfPlugins ??= []).push((api) => {
  document.documentElement.dataset.testPlugin = "loaded";
  api.registerIsland("test-plugin", (el) => (el.textContent = "island mounted"));
});
