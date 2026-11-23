<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { orderTree, indented } from "#lib/tree.js";
  import TreeSelect from "#lib/TreeSelect.svelte";
  import { del, get, post, put } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import Field from "#lib/Field.svelte";
  import Section from "#lib/Section.svelte";
  import Switch from "#lib/Switch.svelte";
  import IconButton from "#lib/IconButton.svelte";
  import Empty from "#lib/Empty.svelte";

  const TRIGGERS = {
    topic_created: tr("A topic is started"),
    post_created: tr("A reply is posted"),
    daily: tr("Every day"),
    weekly: tr("Every week"),
    monthly: tr("Every month"),
  };
  const ACTIONS = {
    tag: tr("Add tags"),
    close: tr("Close the topic"),
    move: tr("Move the topic"),
    reply: tr("Reply"),
    create_topic: tr("Start a topic"),
    flag: tr("Send the post to the review queue"),
    ai_reply: tr("Reply, written by AI"),
    ai_report: tr("Start a topic, written by AI from recent activity"),
  };
  const WRITES_TOPIC = ["create_topic", "ai_report"];
  const REASONS = [
    ["inappropriate", tr("Inappropriate")],
    ["spam", tr("Spam")],
  ];
  const scheduled = (t) => ["daily", "weekly", "monthly"].includes(t);

  let rules = $state([]);
  let nodes = $state([]);
  let editing = $state(null);
  let loaded = $state(false);
  const status = new Status();

  const forums = $derived(orderTree(nodes));
  const forumName = (id) => nodes.find((n) => n.id === Number(id))?.title ?? `#${id}`;

  async function load() {
    try {
      const [r, t] = await Promise.all([get("/automations"), get("/tree")]);
      rules = r;
      nodes = t.nodes ?? t;
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }
  load();

  function blank() {
    return {
      id: 0,
      name: "",
      enabled: true,
      trigger: "topic_created",
      forums: [],
      words: "",
      min_tl: "",
      max_tl: "",
      ai_question: "",
      actions: [{ type: "tag", tags: "" }],
    };
  }
  function edit(r) {
    const c = r.conditions ?? {};
    editing = {
      id: r.id,
      name: r.name,
      enabled: r.enabled,
      trigger: r.trigger,
      forums: (c.forums ?? []).map(String),
      words: (c.words ?? []).join(", "),
      min_tl: c.min_trust_level ?? "",
      max_tl: c.max_trust_level ?? "",
      ai_question: c.ai_question ?? "",
      actions: structuredClone($state.snapshot(r.actions ?? [])).map((a) => ({
        ...a,
        forum: a.forum != null ? String(a.forum) : "",
      })),
    };
    status.clear();
  }
  function body(e) {
    const conditions = {};
    if (!scheduled(e.trigger)) {
      if (e.forums.length) conditions.forums = e.forums.map(Number);
      const words = e.words
        .split(",")
        .map((w) => w.trim())
        .filter(Boolean);
      if (words.length) conditions.words = words;
      if (e.min_tl !== "" && e.min_tl !== null) conditions.min_trust_level = Number(e.min_tl);
      if (e.max_tl !== "" && e.max_tl !== null) conditions.max_trust_level = Number(e.max_tl);
      if (e.ai_question.trim()) conditions.ai_question = e.ai_question.trim();
    }
    const actions = e.actions.map((a) => {
      const out = { type: a.type };
      if (a.type === "tag") out.tags = a.tags ?? "";
      if (a.type === "move" || WRITES_TOPIC.includes(a.type)) out.forum = Number(a.forum) || null;
      if (["reply", "create_topic", "ai_reply", "ai_report", "flag"].includes(a.type)) out.text = a.text ?? "";
      if (WRITES_TOPIC.includes(a.type)) out.title = a.title ?? "";
      if (a.type === "ai_report") out.days = Number(a.days) || 7;
      if (a.type === "flag") out.reason = a.reason ?? "inappropriate";
      return out;
    });
    return { name: e.name.trim(), enabled: e.enabled, trigger: e.trigger, conditions, actions };
  }
  async function save(ev) {
    ev.preventDefault();
    status.clear();
    try {
      if (editing.id) await put(`/automations/${editing.id}`, body(editing));
      else await post("/automations", body(editing));
      status.msg = tr('Saved "{n}".', { n: editing.name.trim() });
      editing = null;
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  async function toggle(r) {
    status.clear();
    try {
      await put(`/automations/${r.id}`, { ...r, enabled: !r.enabled });
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  async function remove(r) {
    if (!confirm(tr('Delete the rule "{name}"?', { name: r.name }))) return;
    try {
      await del(`/automations/${r.id}`);
      if (editing?.id === r.id) editing = null;
      await load();
    } catch (e) {
      status.fail(e);
    }
  }
  function summary(r) {
    return (r.actions ?? [])
      .map((a) => {
        if (a.type === "tag") return tr("tag “{tags}”", { tags: a.tags });
        if (a.type === "move") return tr("move to {n}", { n: forumName(a.forum) });
        if (WRITES_TOPIC.includes(a.type))
          return tr("start “{title}” in {forum}", { title: a.title, forum: forumName(a.forum) });
        return ACTIONS[a.type]?.toLowerCase() ?? a.type;
      })
      .join(", ");
  }
  function setTrigger(t) {
    editing.trigger = t;
    // A schedule has no topic to act on: only "start a topic" makes sense.
    if (scheduled(t)) editing.actions = editing.actions.filter((a) => WRITES_TOPIC.includes(a.type));
    if (scheduled(t) && !editing.actions.length)
      editing.actions = [{ type: "create_topic", forum: "", title: "", text: "" }];
  }
  import MarkdownField from "#lib/MarkdownField.svelte";
</script>

<PageHead title={tr("Automation")} status={editing ? null : status}>
  {#snippet intro()}{tr(
      "When something happens and the conditions match, run the actions. Replies and topics are written by the",
    )} <code>plugin-automation</code>
    {tr("account, and what it writes never sets off another rule. In text,")} <code>{"{author}"}</code>,
    <code>{"{title}"}</code>
    {tr("and")} <code>{"{date}"}</code>
    {tr("are filled in.")}{/snippet}
  <button
    onclick={() => {
      status.clear();
      editing = blank();
    }}
  >
    {tr("New rule")}
  </button>
</PageHead>

{#if editing}
  <EditDialog
    title={editing.id ? tr("Edit rule") : tr("New rule")}
    size="is-large"
    onsubmit={save}
    onclose={() => (editing = null)}
    {status}
    ondelete={editing.id ? () => remove(editing) : null}
    save={editing.id ? tr("Save") : tr("Add rule")}
  >
    <Section legend={tr("Rule")} grid>
      <Field label={tr("Name")} bind:value={editing.name} maxlength="100" required />
      <Field
        label={tr("When")}
        value={editing.trigger}
        onchange={(e) => setTrigger(e.currentTarget.value)}
        options={Object.entries(TRIGGERS)}
      />
    </Section>

    {#if !scheduled(editing.trigger)}
      <Section legend={tr("Only if (all optional)")} grid>
        <div class="acp-wide">
          <MultiSelect
            label={tr("In these forums")}
            placeholder={tr("Any forum")}
            options={forums.map((n) => ({ value: String(n.id), label: indented(n), disabled: n.kind !== "forum" }))}
            bind:value={editing.forums}
          />
        </div>
        <Field label={tr("Title or text contains any of (comma separated)")} bind:value={editing.words} wide />
        <Field label={tr("Author trust level at least")} type="number" min="0" max="20" bind:value={editing.min_tl} />
        <Field label={tr("At most")} type="number" min="0" max="20" bind:value={editing.max_tl} />
        <Field
          label={tr("Only if the AI answers yes to this question about the post")}
          bind:value={editing.ai_question}
          maxlength="500"
          wide
        />
      </Section>
    {/if}

    <Section legend={tr("Then")}>
      {#each editing.actions as a, i}
        <div class="acp-action">
          <div class="acp-action-head">
            <Field label={tr("Action {n}", { n: i + 1 })} bind:value={a.type}>
              {#each Object.entries(ACTIONS) as [k, label]}<option
                  value={k}
                  disabled={scheduled(editing.trigger) && !WRITES_TOPIC.includes(k)}
                >
                  {label}
                </option>{/each}
            </Field>
            <IconButton
              icon="trash-2"
              label={tr("Remove action {n}", { n: i + 1 })}
              onclick={() => editing.actions.splice(i, 1)}
              disabled={editing.actions.length === 1}
            />
          </div>
          {#if a.type !== "close"}
            <div class="acp-form acp-form-flat">
              {#if a.type === "tag"}
                <Field label={tr("Tags (comma separated)")} bind:value={a.tags} required wide />
              {:else if a.type === "flag"}
                <Field label={tr("Reason")} bind:value={a.reason} options={REASONS} />
                <Field label={tr("Note for the moderators")} bind:value={a.text} maxlength="200" wide />
              {:else if a.type === "move" || WRITES_TOPIC.includes(a.type)}
                <TreeSelect
                  rows={nodes}
                  bind:value={a.forum}
                  label={a.type === "move" ? tr("To") : tr("In")}
                  none={tr("Choose a forum")}
                  kind="forum"
                  asString
                  required
                  wide
                />
              {/if}
              {#if WRITES_TOPIC.includes(a.type)}
                <Field label={tr("Title")} bind:value={a.title} maxlength="200" required wide />
              {/if}
              {#if a.type === "ai_report"}
                <Field label={tr("Days to report on")} type="number" min="1" max="90" bind:value={a.days} />
              {/if}
              {#if a.type === "reply" || a.type === "create_topic"}
                <MarkdownField label={tr("Text")} rows="5" bind:value={a.text} />
              {:else if a.type === "ai_reply" || a.type === "ai_report"}
                <Field label={tr("Instructions for the AI")} rows="5" bind:value={a.text} required wide />
              {/if}
            </div>
          {/if}
        </div>
      {/each}
      {#if editing.actions.length < 10}
        <button
          type="button"
          class="secondary"
          onclick={() =>
            editing.actions.push(
              scheduled(editing.trigger)
                ? { type: "create_topic", forum: "", title: "", text: "" }
                : { type: "reply", text: "" },
            )}
        >
          {tr("Add an action")}
        </button>
      {/if}
    </Section>

    <Section legend={tr("Status")} switches>
      <Switch label={tr("Enabled")} bind:checked={editing.enabled} />
    </Section>
  </EditDialog>
{/if}

<table>
  <thead>
    <tr>
      <th>{tr("Rule")}</th><th>{tr("When")}</th><th>{tr("Then")}</th><th>{tr("Runs")}</th><th>{tr("Last run")}</th><th
        class="acp-cell-end"
      ></th>
    </tr>
  </thead>
  <tbody>
    {#each rules as r (r.id)}
      <tr class:muted={!r.enabled}>
        <td>
          <button class="link" onclick={() => edit(r)}>{r.name}</button>{#if !r.enabled}
            <small>{tr("(off)")}</small>{/if}
        </td>
        <td>{TRIGGERS[r.trigger] ?? r.trigger}</td>
        <td>{summary(r)}</td>
        <td>{r.run_count}</td>
        <td><When iso={r.last_run_at} /></td>
        <td class="acp-cell-end">
          <button class="secondary" onclick={() => toggle(r)}>{r.enabled ? tr("Turn off") : tr("Turn on")}</button>
          <button class="is-danger" onclick={() => remove(r)}>{tr("Delete")}</button>
        </td>
      </tr>
    {:else}
      <Empty cols={6} {loaded} title={tr("No rules yet.")} text={tr("Use New rule above to add one.")} />
    {/each}
  </tbody>
</table>
