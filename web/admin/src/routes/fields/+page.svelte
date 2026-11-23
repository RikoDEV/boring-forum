<script>
  import Empty from "#lib/Empty.svelte";
  import { tn, tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import { iconHref, sprite } from "#lib/icons.js";
  import IconPicker from "#lib/IconPicker.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";

  // What each type is for, with the icon a field of that type shows when it has none of its own.
  const KINDS = {
    text: [tr("Short text"), tr("One line: a name, a place, a handle."), "pencil"],
    textarea: [tr("Long text"), tr("A few sentences, up to 2000 characters."), "file-text"],
    url: [tr("Link"), tr("A web address, optionally limited to certain sites."), "link"],
    select: [tr("Choice"), tr("One answer from a list you set."), "list"],
  };
  // Ready-made rules for common profile links; a starting point the admin can still change.
  const PRESETS = [
    ["GitHub", "https://github.com/", "brand-github"],
    ["GitLab", "https://gitlab.com/", "brand-gitlab"],
    ["Mastodon", "https://mastodon.social/@", "brand-mastodon"],
    ["Bluesky", "https://bsky.app/profile/", "brand-bluesky"],
    ["X", "https://x.com/", "brand-x"],
    ["YouTube", "https://www.youtube.com/@", "brand-youtube"],
    ["Twitch", "https://www.twitch.tv/", "brand-twitch"],
    // LinkedIn asked Simple Icons to remove its logo, so this one has a general icon.
    ["LinkedIn", "https://www.linkedin.com/in/", "briefcase"],
  ];

  let fields = $state([]);
  let draft = $state(null); // the field being added or changed
  let tryValue = $state("");
  const status = new Status();

  let loaded = $state(false);
  const load = () =>
    get("/profile-fields")
      .then((r) => (fields = r), status.fail)
      .finally(() => (loaded = true));
  const lines = (text) =>
    text
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean);

  function fresh() {
    status.clear();
    tryValue = "";
    draft = {
      id: 0,
      label: "",
      kind: "text",
      icon: "",
      optionsText: "",
      prefixesText: "",
      url_shorthand: false,
      https_only: true,
      required: false,
      show_on_register: false,
      public: true,
    };
  }
  function edit(f) {
    status.clear();
    tryValue = "";
    draft = {
      ...f,
      icon: f.icon ?? "",
      optionsText: f.options.join("\n"),
      prefixesText: f.url_prefixes.join("\n"),
      was: f,
    };
  }
  function preset(/** @type {string[]} */ [name, prefix, icon]) {
    if (!draft.label.trim()) draft.label = name;
    draft.prefixesText = prefix;
    draft.url_shorthand = true;
    draft.https_only = true;
    if (!draft.icon) draft.icon = icon;
  }

  const body = (d) => ({
    label: d.label,
    kind: d.kind,
    icon: d.icon || null,
    options: d.kind === "select" ? lines(d.optionsText) : [],
    url_prefixes: d.kind === "url" ? lines(d.prefixesText) : [],
    url_shorthand: d.kind === "url" && d.url_shorthand,
    https_only: d.kind === "url" && d.https_only,
    required: d.required,
    show_on_register: d.show_on_register,
    public: d.public,
  });

  async function save(e) {
    e.preventDefault();
    status.clear();
    try {
      if (draft.id) await put(`/profile-fields/${draft.id}`, body(draft));
      else await post("/profile-fields", body(draft));
      status.msg = tr("Saved {n}.", { n: draft.label.trim() });
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  async function remove(f) {
    const answers = f.answers ? ` ${tn("admin.count.field_answers_go", f.answers)}` : "";
    if (!confirm(tr('Delete the field "{label}"?{answers}', { label: f.label, answers }))) return;
    status.clear();
    try {
      await del(`/profile-fields/${f.id}`);
      status.msg = tr("Deleted {label}.", { label: f.label });
      draft = null;
      await load();
    } catch (err) {
      status.fail(err);
    }
  }

  async function move(i, by) {
    const order = fields.map((f) => f.id);
    const [id] = order.splice(i, 1);
    order.splice(i + by, 0, id);
    fields = order.map((x) => fields.find((f) => f.id === x));
    try {
      await put("/profile-fields/order", order);
    } catch (err) {
      status.fail(err);
      await load();
    }
  }

  // The same rules the server applies (bf_db::accounts::fields::Field::check_link), so the admin
  // can try values before saving. The server stays the judge.
  function tryLink(d, raw) {
    const v = raw.trim();
    if (!v) return null;
    const prefixes = lines(d.prefixesText);
    let full = v;
    if (d.url_shorthand && prefixes.length && !v.includes("://")) {
      const name = v.replace(/^[@/]+/, "");
      if (!name) return [false, tr("Enter a name or a link.")];
      full = prefixes[0] + name;
    }
    if (/[\s"'<>`\\]/.test(full)) return [false, tr("No spaces or quotes.")];
    const lower = full.toLowerCase();
    if (!(lower.startsWith("https://") || (!d.https_only && lower.startsWith("http://"))))
      return [false, d.https_only ? tr("Must start with https://") : tr("Must start with https:// or http://")];
    const match = prefixes.find((p) => lower.startsWith(p.toLowerCase()));
    if (prefixes.length && !match) return [false, tr("Must start with {n}", { n: prefixes.join(" or ") })];
    if (match && full.length === match.length) return [false, tr("Add a name after {match}", { match })];
    return [true, full];
  }
  const tried = $derived(draft?.kind === "url" ? tryLink(draft, tryValue) : null);
  const hintFor = (d) => {
    if (d.kind !== "url") return "";
    const p = lines(d.prefixesText);
    const scheme = d.https_only ? "https://" : "http(s)://";
    if (!p.length) return tr("A link starting with {scheme}", { scheme });
    if (d.url_shorthand)
      return p.length === 1
        ? tr("Your name on {n}… or the full link", { n: p[0] })
        : tr("Your name on {n}… or a link starting with {n2}", { n: p[0], n2: p.slice(1).join(", ") });
    return tr("A link starting with {n}", { n: p.join(", ") });
  };
  const sample = (d) =>
    d.kind === "url"
      ? tried?.[0]
        ? tried[1]
        : lines(d.prefixesText)[0]
          ? `${lines(d.prefixesText)[0]}yourname`
          : "https://example.com"
      : d.kind === "select"
        ? (lines(d.optionsText)[0] ?? tr("An option"))
        : d.kind === "textarea"
          ? tr("A few sentences about it.")
          : tr("Their answer");
  const iconOf = (f) => f.icon || KINDS[f.kind]?.[2] || "pencil";

  load();
</script>

<PageHead
  title={tr("Profile fields")}
  intro={tr(
    "Extra questions on members' profiles. Each can be asked at sign-up, required, and shown or kept for staff.",
  )}
>
  <button onclick={fresh}>{tr("New field")}</button>
</PageHead>
{#if !draft}
  {#if status.error}<p class="err" role="alert">{status.error}</p>{/if}
  {#if status.msg}<p class="ok" role="status">{status.msg}</p>{/if}
{/if}

{#if draft}
  <EditDialog
    title={draft.id ? tr("Edit {label}", { label: draft.was.label }) : tr("New field")}
    size="is-large"
    onclose={() => (draft = null)}
    onsubmit={save}
    {status}
    ondelete={draft.id ? () => remove(draft.was) : null}
    deleteLabel={tr("Delete field")}
    save={draft.id ? tr("Save") : tr("Add field")}
  >
    {#if !draft.id}
      <fieldset class="acp-picker">
        <legend>{tr("Type")}</legend>
        <div class="acp-kinds">
          {#each Object.entries(KINDS) as [k, [name, about, icon]]}
            <label class="acp-kind" class:is-on={draft.kind === k}>
              <input type="radio" name="kind" value={k} bind:group={draft.kind} />
              <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#{icon}"></use></svg>
              <span><strong>{name}</strong><small>{about}</small></span>
            </label>
          {/each}
        </div>
      </fieldset>
    {/if}

    <div class="acp-form">
      <label class="bf-field acp-wide">
        <input name="label" bind:value={draft.label} placeholder=" " maxlength="80" required /><span
          class="bf-field-label"
        >
          {tr("Label")}{draft.id ? ` · ${KINDS[draft.kind][0]}` : ""}
        </span>
      </label>
      {#if draft.kind === "select"}
        <Field label={tr("Options, one per line")} bind:value={draft.optionsText} rows="4" required wide />
      {/if}
    </div>

    {#if draft.kind === "url"}
      <fieldset class="acp-picker">
        <legend>{tr("Link rules")}</legend>
        <div class="acp-presets">
          <span class="muted">{tr("Start from")}</span>{#each PRESETS as p}<button
              type="button"
              class="secondary"
              onclick={() => preset(p)}
            >
              {p[0]}
            </button>{/each}
        </div>
        <div class="acp-form acp-form-flat">
          <Field
            label={tr("Allowed link starts, one per line")}
            bind:value={draft.prefixesText}
            rows="2"
            spellcheck="false"
            wide
          />
        </div>
        <small class="muted acp-picker-hint">
          {tr("Empty allows any link. With several, the first is the one short names are added to.")}
        </small>
        <div class="acp-switches">
          <Switch label={tr("Only https:// links")} bind:checked={draft.https_only} />
          <Switch
            label={lines(draft.prefixesText)[0]
              ? tr("Members can type just their name, added to {prefix}", { prefix: lines(draft.prefixesText)[0] })
              : tr("Members can type just their name")}
            bind:checked={draft.url_shorthand}
            disabled={!lines(draft.prefixesText).length}
          />
        </div>
        <div class="acp-form acp-form-flat">
          <Field label={tr("Try a value")} name="try" bind:value={tryValue} spellcheck="false" wide />
        </div>
        {#if tried}<p class={tried[0] ? "ok" : "err"} role="status">
            {tried[0] ? tr("Saved as {n}", { n: tried[1] }) : tried[1]}
          </p>{/if}
      </fieldset>
    {/if}

    <fieldset class="acp-picker">
      <legend>{tr("Icon")}</legend>
      <IconPicker bind:value={draft.icon} />
    </fieldset>

    <fieldset class="acp-picker">
      <legend>{tr("Where it appears")}</legend>
      <div class="acp-switches">
        <Switch label={tr("Ask at sign-up")} bind:checked={draft.show_on_register} />
        <Switch label={tr("Required")} bind:checked={draft.required} />
        <Switch label={tr("Show on the profile (off: only the member and staff see it)")} bind:checked={draft.public} />
      </div>
    </fieldset>

    <fieldset class="acp-picker">
      <legend>{tr("Preview")}</legend>
      <div class="acp-field-preview">
        <div>
          <small class="muted">{tr("In the form")}</small>
          <div class="acp-form acp-form-flat">
            <label class="bf-field acp-wide">
              {#if draft.kind === "select"}<select disabled>
                  <option>{lines(draft.optionsText)[0] ?? ""}</option>
                </select>{:else if draft.kind === "textarea"}<textarea rows="2" placeholder=" " disabled
                ></textarea>{:else}<input placeholder=" " value="" disabled />{/if}<span class="bf-field-label">
                {draft.label || tr("Label")}{draft.required ? "" : " (optional)"}
              </span>
            </label>
          </div>
          {#if hintFor(draft)}<small class="muted">{hintFor(draft)}</small>{/if}
        </div>
        <div>
          <small class="muted">{draft.public ? tr("On the profile") : tr("Not on the profile")}</small>
          <dl class="acp-field-show" class:is-off={!draft.public}>
            <dt>
              <svg class="bf-icon" aria-hidden="true"><use href={iconHref(iconOf(draft))}></use></svg>{draft.label ||
                tr("Label")}
            </dt>
            <dd>
              {#if draft.kind === "url"}<span class="acp-link-like">
                  {sample(draft).replace(/^https?:\/\//, "")}
                </span>{:else}{sample(draft)}{/if}
            </dd>
          </dl>
        </div>
      </div>
    </fieldset>
  </EditDialog>
{/if}

<div class="acp-box">
  <ul class="acp-rows">
    {#each fields as f, i (f.id)}
      <li class="acp-rowitem acp-field-row">
        <span class="acp-field-icon" aria-hidden="true">
          <svg class="bf-icon"><use href={iconHref(iconOf(f))}></use></svg>
        </span>
        <div class="acp-grow">
          <strong>{f.label}</strong>
          <small class="muted acp-field-about">
            {KINDS[f.kind]?.[0] ?? f.kind}{f.kind === "select" ? ` · ${f.options.join(", ")}` : ""}{f.kind === "url" &&
            f.hint
              ? ` · ${f.hint}`
              : ""} · {tn("admin.count.answers", f.answers)}
          </small>
        </div>
        <span class="acp-field-pills">
          {#if f.required}<span class="acp-pill is-warn">{tr("Required")}</span>{/if}
          {#if f.show_on_register}<span class="acp-pill">{tr("At sign-up")}</span>{/if}
          {#if !f.public}<span class="acp-pill is-muted">{tr("Staff only")}</span>{/if}
          {#if f.plugin}<span class="acp-pill is-muted">{tr("From plugin {plugin}", { plugin: f.plugin })}</span>{/if}
        </span>
        <span class="acp-field-move">
          <button
            type="button"
            class="secondary acp-icon-btn"
            aria-label={tr("Move {label} up", { label: f.label })}
            disabled={i === 0}
            onclick={() => move(i, -1)}
          >
            <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#arrow-up"></use></svg>
          </button>
          <button
            type="button"
            class="secondary acp-icon-btn"
            aria-label={tr("Move {label} down", { label: f.label })}
            disabled={i === fields.length - 1}
            onclick={() => move(i, 1)}
          >
            <svg class="bf-icon acp-flip" aria-hidden="true"><use href="{sprite}#arrow-up"></use></svg>
          </button>
        </span>
        {#if !f.plugin}<button class="secondary" onclick={() => edit(f)}>{tr("Edit")}</button>{/if}
      </li>
    {:else}
      <Empty
        li
        {loaded}
        title={tr("No profile fields yet")}
        text={tr("Use New field above to ask for a location, a website or a GitHub name.")}
      />
    {/each}
  </ul>
</div>
