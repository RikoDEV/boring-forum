<script>
  import { resolve } from "$app/paths";
  import { page } from "$app/state";
  import { mount, tick } from "svelte";
  import AdminSearch from "#lib/AdminSearch.svelte";
  import { setEditorUrl } from "#lib/editor.js";
  import { loadShell, tr } from "#lib/i18n.js";
  import "#lib/site.css";
  import "#lib/admin.css";
  import { tooltips } from "../../../islands/src/tooltip.js";
  import sprite from "../../../../themes/default/icons.svg?url";

  const base = resolve("/").replace(/\/$/, "");
  let { children } = $props();
  // The hover bubble the site uses, for icon buttons here too.
  $effect(() => tooltips(".acp-icon-btn"));
  let open = $state(false);

  /** @type {[string, [string, string, string][]][]} */
  const groups = [
    [tr("Overview"), [["", tr("Dashboard"), "gauge"]]],
    [
      tr("People"),
      [
        ["/users", tr("Members"), "user"],
        ["/groups", tr("Groups"), "users"],
        ["/message", tr("Mass message"), "megaphone"],
        ["/trust", tr("Trust levels"), "shield-check"],
        ["/fields", tr("Profile fields"), "pencil"],
      ],
    ],
    [
      tr("Content"),
      [
        ["/forums", tr("Forums"), "folder"],
        ["/tags", tr("Tags, emoji, invites"), "flag"],
        ["/community", tr("Community"), "handshake"],
        ["/chat", tr("Chat"), "message-circle"],
        ["/badges", tr("Badges"), "award"],
        ["/words", tr("Watched words"), "eye-off"],
        ["/mod-actions", tr("Saved mod actions"), "hammer"],
        ["/pages", tr("Pages"), "file-text"],
        ["/text", tr("Site text"), "quote"],
        ["/permalinks", tr("Permalinks"), "link"],
        ["/legal", tr("Legal"), "landmark"],
      ],
    ],
    [
      tr("Appearance"),
      [
        ["/themes", tr("Themes"), "brush"],
        ["/colours", tr("Colour schemes"), "palette"],
        ["/widgets", tr("Widgets"), "panel-bottom"],
        ["/ads", tr("Advertisements"), "newspaper"],
        ["/navigation", tr("Navigation"), "menu"],
      ],
    ],
    [
      tr("System"),
      [
        ["/settings", tr("Settings"), "settings"],
        ["/backups", tr("Backups"), "archive"],
        ["/plugins", tr("Plugins"), "wrench"],
        ["/webhooks", tr("Webhooks"), "zap"],
        ["/federation", tr("Federation"), "globe"],
        ["/mail", tr("Mail log"), "mail"],
        ["/audit", tr("Staff log"), "history"],
        ["/ai", tr("AI"), "sparkles"],
        ["/automations", tr("Automation"), "rocket"],
        ["/explorer", tr("Data Explorer"), "database"],
        ["/logs", tr("Logs"), "list"],
        ["/maintenance", tr("Maintenance"), "server"],
      ],
    ],
  ];
  const current = (p) => page.url.pathname.replace(/\/$/, "") === `${base}${p}`;
  let siteName = $state("");
  let shell = $state("");
  // The site's own header, with its styles and islands. The styles go first in <head>, so the
  // panel's stylesheet still decides how its own pages look.
  loadShell()
    .then(async (s) => {
      if (!s) return;
      setEditorUrl(s.editor);
      for (const href of [...s.css].reverse()) {
        const link = document.createElement("link");
        link.rel = "stylesheet";
        link.href = href;
        document.head.prepend(link);
      }
      const style = document.createElement("style");
      style.textContent = s.critical;
      document.head.prepend(style);
      shell = s.html;
      await tick();
      // A reply being written belongs to the forum pages, not here.
      document.querySelector('[data-island="composer-resume"]')?.remove();
      // The panel's search takes the place of the forum's in the header.
      const host = document.createElement("div");
      host.className = "acp-search-host";
      document.querySelector(".acp-global .header-search")?.replaceWith(host);
      mount(AdminSearch, { target: host, props: { pages: groups } });
      // The header's islands read their messages from here, as on the site's pages.
      const messages = document.createElement("script");
      messages.type = "application/json";
      messages.id = "bf-i18n";
      messages.textContent = s.messages ?? "{}";
      document.body.append(messages);
      const script = document.createElement("script");
      script.type = "module";
      script.textContent = s.loader;
      document.body.append(script);
    })
    .catch(() => {});
  fetch("/api/v1/admin/settings")
    .then((r) => (r.ok ? r.json() : []))
    .then((all) => (siteName = all.find((s) => s.key === "site_name")?.value ?? ""))
    .catch(() => {});
  // The settings page announces a new name, so the header follows without a reload.
  if (typeof window !== "undefined")
    window.addEventListener("bf-site-name", (/** @type {CustomEvent} */ e) => {
      siteName = e.detail;
      const brand = document.querySelector(".site-header .brand");
      // A logo stays; only its text alternative takes the new name.
      if (brand?.classList.contains("has-logo")) for (const img of brand.querySelectorAll("img")) img.alt = e.detail;
      else if (brand) brand.textContent = e.detail;
    });
  // The same choice the site's theme button stores.
  try {
    const theme = localStorage.getItem("bf-theme");
    if (theme) {
      document.documentElement.dataset.theme = theme;
      document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", theme);
    }
  } catch {
    // Storage blocked: follow the system.
  }
</script>

<svelte:head><title>{tr("Admin - {site}", { site: siteName || tr("Forum") })}</title></svelte:head>

<!-- eslint-disable-next-line svelte/no-at-html-tags -- the shell is server-rendered, trusted markup -->
<div class="acp-global">{@html shell}</div>
<div class="acp-top" role="region" aria-label={tr("Admin")}>
  <button
    class="acp-ghost acp-menu-btn acp-icon-btn"
    aria-label={tr("Menu")}
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#menu"></use></svg>
  </button>
  <a class="acp-brand" href="{base}/">{tr("Admin")}</a>
</div>
<div class="acp-shell">
  <nav class="acp-nav" class:is-open={open} aria-label={tr("Admin")}>
    {#each groups as [title, items]}
      <div class="acp-group">{title}</div>
      {#each items as [p, label, icon]}
        <a href="{base}{p}/" aria-current={current(p) ? "page" : undefined} onclick={() => (open = false)}>
          <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{icon}"></use></svg>
          {label}
        </a>
      {/each}
    {/each}
  </nav>
  <main class="acp-main">{@render children()}</main>
</div>
