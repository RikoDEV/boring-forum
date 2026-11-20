<script>
  /// A floating-label field (`label.bf-field`), the panel's one text control: an input of any
  /// `type`, a textarea (`rows`), or a select (`options` as [value, label] pairs or
  /// {value, label}, or <option> children). `wide` spans the whole `.acp-form` row. A hint goes in
  /// the label text (CLAUDE.md: nothing else inside a .bf-field). Other attributes (required, maxlength, min, oninput…) go to the control.

  /** @type {{ label: any, value?: any, type?: string, rows?: number | string, options?: any, wide?: boolean, class?: string, children?: import('svelte').Snippet, [key: string]: any }} */ let {
    label,
    value = $bindable(),
    type = "text",
    rows = 0,
    options = null,
    wide = false,
    class: cls = "",
    children,
    ...rest
  } = $props();
  const pair = (o) => (Array.isArray(o) ? o : [o.value, o.label]);
</script>

<label class={["bf-field", wide && "acp-wide", cls]}>
  {#if options || children}
    <select bind:value {...rest}>
      {#if options}{#each options.map(pair) as [v, text]}<option value={v}>{text}</option>{/each}{/if}
      {@render children?.()}
    </select>
  {:else if rows}
    <textarea bind:value rows={+rows} placeholder=" " {...rest}></textarea>
  {:else}
    <input {type} bind:value placeholder=" " {...rest} />
  {/if}
  <span class="bf-field-label">{label}</span>
</label>
