<script>
  /// The settings of one feature, on that feature's own page. Which they are is listed once, in
  /// settings-meta.js, which also hides them from Settings and sends searches and old links here.
  /// A page with tabs keeps this mounted while another tab is open (hide it, do not remove it),
  /// so that unsaved changes are not lost; `onshow` opens its tab when a link names a setting.
  import { tick } from "svelte";
  import { page } from "$app/state";
  import { tr } from "#lib/i18n.js";
  import Notices from "#lib/Notices.svelte";
  import SettingRows from "#lib/SettingRows.svelte";
  import SaveBar from "#lib/SaveBar.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { SettingsForm } from "#lib/settings-form.svelte.js";
  import { keysOn } from "#lib/settings-meta.js";

  /** @type {{ path: string, title?: string, hint?: string, hidden?: boolean, onshow?: () => void, onsaved?: () => void }} */
  let { path, title = tr("Settings"), hint = "", hidden = false, onshow, onsaved } = $props();

  const status = new Status();
  // svelte-ignore state_referenced_locally (a page's path does not change)
  const wanted = keysOn(path);
  const form = new SettingsForm(status, wanted.has);
  // In the order the list gives, not the server's.
  const items = $derived(wanted.order(form.settings));
  let found = $state("");

  let first = true;
  $effect(() => {
    form.stamp;
    if (!first) onsaved?.();
    first = false;
  });

  async function reveal(hash) {
    const key = decodeURIComponent(/^#setting=(.+)$/.exec(hash)?.[1] ?? "");
    if (!key || !form.byKey.has(key)) return;
    onshow?.();
    found = key;
    await tick();
    const row = document.getElementById(`setting-${key}`);
    row?.scrollIntoView({ block: "center", behavior: "smooth" });
    row?.querySelector("input, select, textarea, button")?.focus({ preventScroll: true });
    setTimeout(() => (found = found === key ? "" : found), 2500);
  }

  $effect(() => {
    const hash = page.url.hash;
    if (form.loaded) reveal(hash);
  });
</script>

<div {hidden}>
  <Notices {status} />
  {#if items.length}
    <section class="acp-box">
      <div class="acp-settings-title">
        <h2>{title}</h2>
        {#if hint}<p class="muted">{hint}</p>{/if}
      </div>
      <SettingRows {form} {items} {found} />
    </section>
  {/if}
</div>
<SaveBar {form} />
