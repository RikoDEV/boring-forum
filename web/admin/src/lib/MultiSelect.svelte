<script module>
  let next = 0;
</script>

<script>
  import { tr } from "#lib/i18n.js";
  // A multi-select input built on the site's tag input (`.bf-tags`): the field shows what is chosen
  // as chips, typing filters the options, and the list opens below it. `value` is an array of
  // option values; `options` are `{ value, label, disabled? }`.
  //
  // `suggest` makes the options come from a search instead of a fixed list: a function that is
  // given what was typed and returns [{ value, label, hint? }] (see #lib/suggest.js for members
  // and tags). The chips then show the label that was picked. `max` limits how many may be chosen:
  // with 1, picking another replaces the first.
  //
  // Keyboard: ↓/↑ move through the list (opening it), Enter picks or drops the highlighted
  // option, Backspace in an empty search drops the last chip, Escape closes, Home/End jump.
  // ARIA: a combobox controlling a multi-selectable listbox.
  let {
    id = undefined,
    label,
    options = [],
    value = $bindable([]),
    placeholder = "",
    // In a settings row the label is already beside the field: it is kept for screen readers.
    hideLabel = false,
    suggest = null,
    max = 0,
  } = $props();

  // eslint-disable-next-line no-useless-assignment -- `next` is the module-wide counter above
  const uid = `ms-${++next}`;
  // svelte-ignore state_referenced_locally (an id is fixed for the life of the field)
  const inputId = id ?? `${uid}-input`;
  const listId = `${uid}-list`;
  let open = $state(false);
  let query = $state("");
  let active = $state(-1);
  let root = $state();
  let input = $state();
  let listEl = $state();

  // What a search found, and the labels of what it found that was picked.
  let found = $state([]);
  let picked = $state({});
  const byValue = (v) => options.find((o) => o.value === v);
  // Values no option matches any more (a deleted group), still shown so they can be removed.
  // A search's values are names, shown as they are.
  const chips = $derived(
    value.map((v) => ({ value: v, label: byValue(v)?.label ?? picked[v] ?? (suggest ? String(v) : tr("Deleted #{v}", { v })) })),
  );
  const shown = $derived(
    suggest
      ? found.filter((o) => !value.includes(o.value))
      : options.filter((o) => !query.trim() || o.label.toLowerCase().includes(query.trim().toLowerCase())),
  );
  const pickable = $derived(shown.filter((o) => !o.disabled));

  function toggle(v) {
    if (max && !value.includes(v) && value.length >= max) {
      if (max > 1) return;
      value = [];
    }
    const o = shown.find((x) => x.value === v);
    if (o) picked[v] = o.label;
    value = value.includes(v) ? value.filter((x) => x !== v) : [...value, v];
    if (max === 1) {
      query = "";
      open = false;
    }
  }
  function remove(v) {
    value = value.filter((x) => x !== v);
    input?.focus();
  }
  function openList() {
    if (!open) {
      open = true;
      active = shown.findIndex((o) => !o.disabled);
    }
  }
  function move(by) {
    openList();
    if (!pickable.length) return;
    let i = active;
    for (let step = 0; step < shown.length; step++) {
      i = (i + by + shown.length) % shown.length;
      if (!shown[i].disabled) break;
    }
    active = i;
    document.getElementById(`${uid}-opt-${i}`)?.scrollIntoView({ block: "nearest" });
  }
  function keydown(e) {
    if (e.isComposing || e.keyCode === 229) return; // an IME is still choosing its text
    switch (e.key) {
      case "ArrowDown":
        e.preventDefault();
        move(1);
        break;
      case "ArrowUp":
        e.preventDefault();
        move(-1);
        break;
      case "Home":
      case "End":
        if (!open) return;
        e.preventDefault();
        active = e.key === "Home" ? shown.findIndex((o) => !o.disabled) : shown.findLastIndex((o) => !o.disabled);
        break;
      case "Enter":
        if (!open) return;
        e.preventDefault();
        if (shown[active] && !shown[active].disabled) toggle(shown[active].value);
        break;
      case "Escape":
        if (open) {
          e.preventDefault();
          open = false;
        }
        break;
      case "Backspace":
        if (!query && value.length) value = value.slice(0, -1);
        break;
      case "Tab":
        open = false;
        break;
    }
  }

  $effect(() => {
    if (!open) return;
    const away = (e) => {
      if (!root?.contains(e.target)) open = false;
    };
    document.addEventListener("pointerdown", away);
    return () => document.removeEventListener("pointerdown", away);
  });
  // The list is a popover (the browser's top layer), so a dialog or a scrolling box around the
  // field cannot cut it off. It is placed under the field, or above it when there is no room.
  function place() {
    if (!root || !listEl) return;
    const r = root.getBoundingClientRect();
    const room = window.innerHeight - r.bottom;
    const h = Math.min(listEl.scrollHeight, 224);
    listEl.style.width = `${r.width}px`;
    listEl.style.left = `${r.left}px`;
    listEl.style.top = `${room < h + 8 && r.top > room ? r.top - h - 2 : r.bottom + 2}px`;
  }
  $effect(() => {
    if (!open || !listEl) return;
    if (!listEl.matches(":popover-open")) listEl.showPopover();
    place();
    const again = () => place();
    window.addEventListener("resize", again);
    window.addEventListener("scroll", again, true);
    return () => {
      window.removeEventListener("resize", again);
      window.removeEventListener("scroll", again, true);
      if (listEl?.matches(":popover-open")) listEl.hidePopover();
    };
  });
  // What was found changes the list's height.
  $effect(() => {
    shown.length;
    place();
  });
  // A search asks its source for what was typed, shortly after the last key.
  $effect(() => {
    if (!suggest) return;
    const typed = query.trim();
    let current = true;
    const timer = setTimeout(async () => {
      const r = await suggest(typed).catch(() => []);
      if (current) found = r;
    }, typed ? 150 : 0);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  });
  // A new search starts at the first match.
  $effect(() => {
    query;
    active = shown.findIndex((o) => !o.disabled);
  });
</script>

<div
  class="bf-field bf-multi"
  class:is-open={open}
  class:has-value={value.length > 0}
  class:has-label={!hideLabel}
  bind:this={root}
>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div
    class="bf-tags"
    onclick={() => {
      input?.focus();
      openList();
    }}
  >
    {#each chips as c (c.value)}
      <span class="bf-tag-chip"
        >{c.label}<button
          type="button"
          aria-label={tr("Remove {label}", { label: c.label })}
          onclick={(e) => {
            e.stopPropagation();
            remove(c.value);
          }}>×</button
        ></span
      >
    {/each}
    <input
      bind:this={input}
      id={inputId}
      bind:value={query}
      role="combobox"
      aria-expanded={open}
      aria-controls={listId}
      aria-autocomplete="list"
      aria-activedescendant={open && active >= 0 ? `${uid}-opt-${active}` : undefined}
      aria-label={hideLabel ? label : undefined}
      autocomplete="off"
      placeholder={value.length ? "" : placeholder}
      onfocus={openList}
      onkeydown={keydown}
    />
    {#if value.length}
      <button
        type="button"
        class="bf-multi-clear acp-icon-btn"
        aria-label={tr("Clear {label}", { label })}
        onclick={(e) => {
          e.stopPropagation();
          value = [];
          input?.focus();
        }}>×</button
      >
    {/if}
    <span class="bf-multi-arrow" aria-hidden="true"></span>
  </div>
  {#if !hideLabel}<label class="bf-field-label" for={inputId}>{label}</label>{/if}
  {#if open}
    <ul class="bf-tags-list" id={listId} popover="manual" bind:this={listEl} role="listbox" aria-multiselectable={max !== 1} aria-label={label}>
      {#each shown as o, i (o.value)}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <li
          id="{uid}-opt-{i}"
          role="option"
          aria-selected={value.includes(o.value)}
          aria-disabled={o.disabled || undefined}
          class:is-active={i === active}
          onpointerdown={(e) => e.preventDefault()}
          onpointermove={() => !o.disabled && (active = i)}
          onclick={() => !o.disabled && toggle(o.value)}
        >
          {o.label}{#if o.hint}
            <small class="muted">{o.hint}</small>{/if}
        </li>
      {:else}
        <li class="bf-multi-empty" role="presentation">
          {suggest ? (query.trim() ? tr("No matches") : tr("Type to search")) : options.length ? tr("No matches") : tr("Nothing to choose")}
        </li>
      {/each}
    </ul>
  {/if}
</div>
