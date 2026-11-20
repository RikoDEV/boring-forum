<script>
  /// A form page a plugin declares with `[[admin_pages]]`: its fields are saved with the plugin's
  /// settings, checked by the server against the declaration (and by the plugin's
  /// `admin.form.save`, if it listens).
  import { page } from "$app/state";
  import { tr } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import SchemaForm from "#lib/SchemaForm.svelte";
  import { localized, withDefaults } from "#lib/schema.js";
  import { get, put } from "#lib/api.js";

  const status = new Status();
  const id = page.url.searchParams.get("id");
  const key = page.url.searchParams.get("key");
  let plugin = $state(null);
  let values = $state({});
  let missing = $state(false);
  const locales = $derived(plugin?.schema?.["x-locales"] ?? {});
  const spec = $derived((plugin?.schema?.["x-admin-pages"] ?? []).find((p) => p.key === key));

  get("/plugins").then((all) => {
    plugin = all.find((p) => p.id === id) ?? null;
    const s = (plugin?.schema?.["x-admin-pages"] ?? []).find((p) => p.key === key);
    if (!s) return (missing = true);
    values = withDefaults(s.fields, plugin.settings);
  }, status.fail);

  function save(e) {
    e.preventDefault();
    status.clear();
    put(`/plugins/${id}/settings`, { ...plugin.settings, ...values }).then(
      () => (status.msg = tr("Saved.")),
      status.fail,
    );
  }
</script>

<PageHead
  title={spec ? localized(locales, spec.label_key, spec.label) : (plugin?.name ?? tr("Plugins"))}
  {status}
  intro={spec?.intro ? localized(locales, spec.intro_key, spec.intro) : ""}
/>
{#if missing}
  <p class="muted">{tr("This plugin has no such page.")}</p>
{:else if spec}
  <Box title={plugin.name}>
    <form onsubmit={save}>
      <SchemaForm fields={spec.fields} bind:values {locales} />
      <div class="acp-box-body acp-actions"><button>{tr("Save")}</button></div>
    </form>
  </Box>
{/if}
