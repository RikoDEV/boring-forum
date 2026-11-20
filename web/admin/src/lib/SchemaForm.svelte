<script>
  /// The fields a plugin declares (see schema.js), drawn with the panel's own controls: floating
  /// fields in one `.acp-form` grid, switches together in a <Section>. A hint goes in the label
  /// text. `values` is bound; `locales` is the plugin's `x-locales`, for translated labels.
  import Field from "./Field.svelte";
  import Section from "./Section.svelte";
  import Switch from "./Switch.svelte";
  import { localized } from "./schema.js";

  /** @type {{ fields: any[], values?: Record<string, any>, locales?: Record<string, any> }} */
  let { fields, values = $bindable({}), locales = {} } = $props();
  const text = (f) => {
    const label = localized(locales, f.label_key, f.label);
    const help = localized(locales, f.help_key, f.help);
    return help ? `${label}: ${help}` : label;
  };
  const choices = (f) => f.options.map((o) => [o.value, localized(locales, o.label_key, o.label)]);
  const fieldCells = $derived(fields.filter((f) => f.type !== "switch"));
  const switches = $derived(fields.filter((f) => f.type === "switch"));
</script>

{#if fieldCells.length}
  <div class="acp-form">
    {#each fieldCells as f (f.key)}
      {#if f.type === "select"}
        <Field label={text(f)} bind:value={values[f.key]} options={choices(f)} />
      {:else if f.type === "textarea"}
        <Field label={text(f)} bind:value={values[f.key]} rows={f.rows || 4} wide />
      {:else if f.type === "number"}
        <Field label={text(f)} type="number" min={f.min ?? undefined} max={f.max ?? undefined} bind:value={values[f.key]} />
      {:else}
        <Field label={text(f)} type={f.type} bind:value={values[f.key]} />
      {/if}
    {/each}
  </div>
{/if}
{#if switches.length}
  <Section switches>
    {#each switches as f (f.key)}
      <Switch label={text(f)} bind:checked={values[f.key]} />
    {/each}
  </Section>
{/if}
