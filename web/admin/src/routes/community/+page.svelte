<script>
  import { followTabHash } from "#lib/tabs.svelte.js";
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Section from "#lib/Section.svelte";
  import sprite from "../../../../../themes/default/icons.svg?url";
  import EditDialog from "#lib/EditDialog.svelte";
  import Empty from "#lib/Empty.svelte";
  let tab = $state("reactions");
  followTabHash(["reactions", "tags", "rules", "news", "reasons"], (key) => (tab = key));
  let reactions = $state([]);
  let groups = $state([]);
  let rules = $state([]);
  let notices = $state([]);
  let reasons = $state([]);
  const status = new Status();
  let loaded = $state(false);
  const load = () =>
    Promise.all([
      get("/reactions"),
      get("/tag-groups"),
      get("/promotion-rules"),
      get("/announcements"),
      get("/report-reasons"),
    ])
      .then(([a, b, c, d, e]) => {
        reasons = e;
        reactions = a;
        groups = b.map((g) => ({ ...g, tagsText: g.tags.join(", "), nodesText: g.nodes.join(", ") }));
        rules = c;
        notices = d;
      }, status.fail)
      .finally(() => (loaded = true));
  const blankReaction = () => ({ name: "", icon: "", score: 1, enabled: true, position: 10 });
  const blankGroup = () => ({ name: "", one_per_topic: false, tagsText: "", nodesText: "" });
  const blankRule = () => ({
    name: "",
    enabled: true,
    min_posts: 0,
    min_days: 0,
    min_reputation: 0,
    min_topics_read: 0,
    add_group: null,
  });
  const blankNotice = () => ({ title: "", body: "", enabled: true, starts_at: null, ends_at: null });
  const blankReason = () => ({
    key: "",
    label: "",
    hint: "",
    weight: 1,
    needs_note: false,
    illegal: false,
    message_user: false,
    enabled: true,
    position: 60,
  });
  let newReason = $state(blankReason());
  let newReaction = $state(blankReaction());
  let newGroup = $state(blankGroup());
  let newRule = $state(blankRule());
  let newNotice = $state(blankNotice());
  const num = (v) => (v === "" || v == null ? null : Number(v));
  const groupBody = (g) => ({
    name: g.name,
    one_per_topic: g.one_per_topic,
    tags: g.tagsText
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean),
    nodes: g.nodesText
      .split(",")
      .map((s) => Number(s.trim()))
      .filter(Boolean),
  });
  const ruleBody = (r) => ({ ...r, set_trust_level: null, add_group: num(r.add_group) });
  // Reactions are ordered by grabbing a row's handle and dropping it above or below another row.
  let grabbed = $state(null);
  let hover = $state(null);
  async function putOrder(ids) {
    status.clear();
    const at = new Map(ids.map((id, i) => [id, i]));
    reactions = [...reactions].sort((a, b) => at.get(a.id) - at.get(b.id));
    try {
      await put("/reactions/order", { ids });
    } catch (e) {
      status.fail(e);
      await load();
    }
  }
  function overRow(e, r) {
    if (grabbed === null || grabbed === r.id) return;
    e.preventDefault();
    const box = e.currentTarget.getBoundingClientRect();
    hover = { id: r.id, after: e.clientY > box.top + box.height / 2 };
  }
  function dropRow(e) {
    e.preventDefault();
    if (grabbed === null || !hover) return;
    const ids = reactions.map((r) => r.id).filter((id) => id !== grabbed);
    ids.splice(ids.indexOf(hover.id) + (hover.after ? 1 : 0), 0, grabbed);
    putOrder(ids);
    grabbed = hover = null;
  }
  function nudgeRow(e, r) {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    e.preventDefault();
    const ids = reactions.map((x) => x.id);
    const i = ids.indexOf(r.id);
    const j = i + (e.key === "ArrowUp" ? -1 : 1);
    if (j < 0 || j >= ids.length) return;
    [ids[i], ids[j]] = [ids[j], ids[i]];
    const handle = e.currentTarget;
    putOrder(ids);
    queueMicrotask(() => requestAnimationFrame(() => handle.isConnected && handle.focus()));
  }
  const removeReaction = (r) =>
    confirm(
      tr('Delete the reaction "{name}"? What members reacted with it disappears from posts.', { name: r.name }),
    ) && removeRow(del(`/reactions/${r.id}`), () => (reactions = reactions.filter((x) => x.id !== r.id)));
  // Changing or removing a row leaves the lists as they are: reloading them would put back what was
  // typed in the other rows and not saved yet. (Adding goes through a dialog, which reloads.)
  const saveRow = (promise) => status.run(() => promise, tr("Saved."));
  const removeRow = (promise, drop) =>
    status.run(async () => {
      await promise;
      drop();
    });
  // What a header button is making: a dialog holds the form, and closes once it was saved.
  let dialog = $state(null);
  const open = (what) => () => {
    status.clear();
    dialog = what;
  };
  const NEW = {
    reasons: ["reason", tr("New reason")],
    reactions: ["reaction", tr("New reaction")],
    tags: ["group", tr("New tag group")],
    rules: ["rule", tr("New group rule")],
    news: ["notice", tr("New announcement")],
  };
  async function create(e, send, reset) {
    e.preventDefault();
    status.clear();
    try {
      await send();
    } catch (err) {
      return status.fail(err);
    }
    reset();
    dialog = null;
    load();
  }
  load();
  import MarkdownField from "#lib/MarkdownField.svelte";
</script>

{#snippet ruleFields(r)}
  <Field label={tr("At least posts")} type="number" min="0" bind:value={r.min_posts} />
  <Field label={tr("At least days")} type="number" min="0" bind:value={r.min_days} />
  <Field label={tr("At least reputation")} type="number" min="0" bind:value={r.min_reputation} />
  <Field label={tr("At least topics read")} type="number" min="0" bind:value={r.min_topics_read} />
  <Field label={tr("Add to group id")} type="number" min="1" bind:value={r.add_group} />
{/snippet}
<PageHead title={tr("Community")} status={dialog ? null : status} reserve>
  {#if NEW[tab]}<button onclick={open(NEW[tab][0])}>{NEW[tab][1]}</button>{/if}
</PageHead>
{#if dialog === "reason"}
  <EditDialog
    title={tr("New reason")}
    onclose={() => (dialog = null)}
    {status}
    save={tr("Add reason")}
    onsubmit={(e) =>
      create(
        e,
        () =>
          post("/report-reasons", {
            ...newReason,
            weight: Number(newReason.weight),
            position: Number(newReason.position),
          }),
        () => (newReason = blankReason()),
      )}
  >
    <div class="acp-form">
      <Field label={tr("Key (a-z, 0-9, _)")} bind:value={newReason.key} required pattern="[a-z0-9_]+" />
      <Field label={tr("Label")} bind:value={newReason.label} required />
      <Field label={tr("Weight")} type="number" step="0.25" min="0" max="10" bind:value={newReason.weight} />
    </div>
  </EditDialog>
{:else if dialog === "reaction"}
  <EditDialog
    title={tr("New reaction")}
    onclose={() => (dialog = null)}
    {status}
    save={tr("Add reaction")}
    onsubmit={(e) =>
      create(
        e,
        () => post("/reactions", newReaction),
        () => (newReaction = blankReaction()),
      )}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={newReaction.name} required />
      <Field label={tr("Emoji")} bind:value={newReaction.icon} required />
    </div>
  </EditDialog>
{:else if dialog === "group"}
  <EditDialog
    title={tr("New tag group")}
    onclose={() => (dialog = null)}
    {status}
    save={tr("Add group")}
    onsubmit={(e) =>
      create(
        e,
        () => post("/tag-groups", groupBody(newGroup)),
        () => (newGroup = blankGroup()),
      )}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={newGroup.name} required />
      <Field label={tr("Tags")} bind:value={newGroup.tagsText} />
      <Field label={tr("Forum ids")} bind:value={newGroup.nodesText} />
      <div class="acp-wide">
        <Section switches><Switch label={tr("One per topic")} bind:checked={newGroup.one_per_topic} /></Section>
      </div>
    </div>
  </EditDialog>
{:else if dialog === "rule"}
  <EditDialog
    title={tr("New group rule")}
    onclose={() => (dialog = null)}
    {status}
    save={tr("Add rule")}
    onsubmit={(e) =>
      create(
        e,
        () => post("/promotion-rules", ruleBody(newRule)),
        () => (newRule = blankRule()),
      )}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={newRule.name} required wide />
      {@render ruleFields(newRule)}
      <div class="acp-wide"><Section switches><Switch label={tr("On")} bind:checked={newRule.enabled} /></Section></div>
    </div>
  </EditDialog>
{:else if dialog === "notice"}
  <EditDialog
    title={tr("New announcement")}
    onclose={() => (dialog = null)}
    {status}
    save={tr("Add announcement")}
    onsubmit={(e) =>
      create(
        e,
        () => post("/announcements", newNotice),
        () => (newNotice = blankNotice()),
      )}
  >
    <div class="acp-form">
      <Field label={tr("Title")} bind:value={newNotice.title} required wide />
      <MarkdownField label={tr("Text")} rows="4" bind:value={newNotice.body} />
      <div class="acp-wide">
        <Section switches><Switch label={tr("On")} bind:checked={newNotice.enabled} /></Section>
      </div>
    </div>
  </EditDialog>
{/if}
<div class="acp-tabs" role="tablist">
  {#each [["reactions", tr("Reactions"), reactions.length], ["tags", tr("Tag groups"), groups.length], ["rules", tr("Group rules"), rules.length], ["news", tr("Announcements"), notices.length], ["reasons", tr("Report reasons"), reasons.length]] as [key, label, n]}
    <button role="tab" aria-selected={tab === key} onclick={() => (tab = String(key))}>
      {label}<span class="acp-count">{n}</span>
    </button>
  {/each}
</div>

{#if tab === "reasons"}
  <p class="muted">
    <small>
      {tr(
        'A report adds the reporter\'s track record times the reason\'s weight to the post\'s score; at the "hide after reports" setting the post is hidden until a moderator looks. "Illegal" reasons are recorded as illegal content in DSA statements.',
      )}
    </small>
  </p>
  <div class="acp-matrix-wrap">
    <table>
      <thead>
        <tr>
          <th>{tr("Key")}</th><th>{tr("Label")}</th><th>{tr("Weight")}</th><th>{tr("Needs note")}</th><th>
            {tr("Illegal")}
          </th><th>{tr("Message only")}</th><th>{tr("Order")}</th><th>{tr("On")}</th><th class="acp-cell-end"></th>
        </tr>
      </thead>
      <tbody>
        {#each reasons as r (r.key)}
          <tr>
            <td><code>{r.key}</code></td>
            <td>
              <input bind:value={r.label} aria-label={tr("Label")} disabled={!!r.plugin} />{#if r.plugin}<div>
                  <span class="acp-pill is-muted">{tr("From plugin {plugin}", { plugin: r.plugin })}</span>
                </div>{/if}
            </td>
            <td>
              <input
                type="number"
                step="0.25"
                min="0"
                max="10"
                bind:value={r.weight}
                aria-label={tr("Weight")}
                style:width="5rem"
              />
            </td>
            <td><Switch bind:checked={r.needs_note} aria-label={tr("Needs a note")} /></td>
            <td><Switch bind:checked={r.illegal} aria-label={tr("Illegal content")} /></td>
            <td>
              <Switch
                bind:checked={r.message_user}
                aria-label={tr("Send the note to the author instead of filing a report")}
              />
            </td>
            <td><input type="number" bind:value={r.position} aria-label={tr("Order")} style:width="5rem" /></td>
            <td><Switch bind:checked={r.enabled} aria-label={tr("Enabled")} disabled={r.key === "other"} /></td>
            <td class="acp-cell-end">
              {#if !r.plugin}<button
                  onclick={() =>
                    saveRow(
                      put(`/report-reasons/${r.key}`, { ...r, weight: Number(r.weight), position: Number(r.position) }),
                    )}
                >
                  {tr("Save")}
                </button>
                {#if r.key !== "other"}<button
                    class="is-danger"
                    onclick={() =>
                      removeRow(
                        del(`/report-reasons/${r.key}`),
                        () => (reasons = reasons.filter((x) => x.key !== r.key)),
                      )}
                  >
                    {tr("Delete")}
                  </button>{/if}{/if}
            </td>
          </tr>
        {:else}
          <Empty cols={9} {loaded} title={tr("No report reasons yet.")} text={tr("Use New reason above to add one.")} />
        {/each}
      </tbody>
    </table>
  </div>
{:else if tab === "reactions"}
  <div class="acp-matrix-wrap">
    <table>
      <thead>
        <tr>
          <th class="acp-grip-col"><span class="visually-hidden">{tr("Order")}</span></th><th>{tr("Emoji")}</th><th>
            {tr("Name")}
          </th><th>{tr("Reputation")}</th><th>{tr("On")}</th><th class="acp-cell-end"></th>
        </tr>
      </thead>
      <tbody>
        {#each reactions as r (r.id)}
          <tr
            class:is-grabbed={grabbed === r.id}
            class:is-drop-before={hover?.id === r.id && !hover.after}
            class:is-drop-after={hover?.id === r.id && hover.after}
            ondragover={(e) => overRow(e, r)}
            ondragleave={() => hover?.id === r.id && (hover = null)}
            ondrop={dropRow}
          >
            <td class="acp-grip-col">
              <span
                class="acp-grip"
                role="button"
                tabindex="0"
                draggable="true"
                aria-label={tr("Move {name}. Alt and the arrow keys also move it.", { name: r.name })}
                ondragstart={(e) => {
                  grabbed = r.id;
                  e.dataTransfer.effectAllowed = "move";
                  e.dataTransfer.setData("text/plain", String(r.id));
                  e.dataTransfer.setDragImage(e.currentTarget.closest("tr"), 0, 0);
                }}
                ondragend={() => {
                  grabbed = null;
                  hover = null;
                }}
                onkeydown={(e) => nudgeRow(e, r)}
              >
                <svg class="bf-icon" aria-hidden="true"><use href="{sprite}#grip-vertical"></use></svg>
              </span>
            </td>
            <td><input bind:value={r.icon} size="3" aria-label={tr("Emoji")} disabled={!!r.plugin} /></td>
            <td>
              <input bind:value={r.name} aria-label={tr("Name")} disabled={!!r.plugin} />{#if r.plugin}<div>
                  <span class="acp-pill is-muted">{tr("From plugin {plugin}", { plugin: r.plugin })}</span>
                </div>{/if}
            </td>
            <td>
              <select bind:value={r.score} aria-label={tr("Reputation")}>
                <option value={1}>+1</option><option value={0}>0</option><option value={-1}>-1</option>
              </select>
            </td>
            <td><Switch bind:checked={r.enabled} aria-label={tr("Enabled")} /></td>
            <td class="acp-cell-end">
              {#if !r.plugin}<button onclick={() => saveRow(put(`/reactions/${r.id}`, r))}>{tr("Save")}</button>
                {#if !r.is_like}<button class="is-danger" onclick={() => removeReaction(r)}>
                    {tr("Delete")}
                  </button>{/if}{/if}
            </td>
          </tr>
        {:else}
          <Empty cols={6} {loaded} title={tr("No reactions yet.")} text={tr("Use New reaction above to add one.")} />
        {/each}
      </tbody>
    </table>
  </div>
{:else if tab === "tags"}
  <p class="muted"><small>{tr("Tags are slugs. Forums are ids; leave empty to allow everywhere.")}</small></p>
  <div class="acp-card-list">
    {#each groups as g (g.id)}
      <section class="acp-box">
        <div class="acp-box-head"><h2>{g.name}</h2></div>
        <div class="acp-form">
          <Field label={tr("Name")} bind:value={g.name} />
          <Field label={tr("Tags")} bind:value={g.tagsText} />
          <Field label={tr("Forum ids")} bind:value={g.nodesText} />
          <div class="acp-wide">
            <Section switches><Switch label={tr("One per topic")} bind:checked={g.one_per_topic} /></Section>
          </div>
          <div class="acp-actions">
            <button onclick={() => saveRow(put(`/tag-groups/${g.id}`, groupBody(g)))}>{tr("Save")}</button><button
              class="is-danger acp-end"
              onclick={() =>
                removeRow(del(`/tag-groups/${g.id}`), () => (groups = groups.filter((x) => x.id !== g.id)))}
            >
              {tr("Delete")}
            </button>
          </div>
        </div>
      </section>
    {:else}
      <Empty {loaded} title={tr("No tag groups yet.")} text={tr("Use New tag group above to add one.")} />
    {/each}
  </div>
{:else if tab === "rules"}
  <p class="muted">
    <small>
      {tr(
        "Checked every ten minutes. A group rule adds members to a group when they meet its conditions, and cannot grant staff groups. Trust levels have their own requirements under Settings, Trust and warnings.",
      )}
    </small>
  </p>
  <div class="acp-card-list">
    {#each rules as r (r.id)}
      <section class="acp-box">
        <div class="acp-box-head">
          <h2>{r.name}</h2>
          <Switch label={tr("On")} bind:checked={r.enabled} />
        </div>
        <div class="acp-form">
          <Field label={tr("Name")} bind:value={r.name} wide />
          {@render ruleFields(r)}
          <div class="acp-actions">
            <button onclick={() => saveRow(put(`/promotion-rules/${r.id}`, ruleBody(r)))}>{tr("Save")}</button><button
              class="is-danger acp-end"
              onclick={() =>
                removeRow(del(`/promotion-rules/${r.id}`), () => (rules = rules.filter((x) => x.id !== r.id)))}
            >
              {tr("Delete")}
            </button>
          </div>
        </div>
      </section>
    {:else}
      <Empty {loaded} title={tr("No group rules yet.")} text={tr("Use New group rule above to add one.")} />
    {/each}
  </div>
{:else}
  <div class="acp-card-list">
    {#each notices as a (a.id)}
      <section class="acp-box">
        <div class="acp-box-head">
          <h2>{a.title}</h2>
          <Switch label={tr("On")} bind:checked={a.enabled} />
        </div>
        <div class="acp-form">
          <Field label={tr("Title")} bind:value={a.title} wide />
          {#key a.id}<MarkdownField label={tr("Text")} rows="3" bind:value={a.body} />{/key}
          <div class="acp-actions">
            <button onclick={() => saveRow(put(`/announcements/${a.id}`, a))}>{tr("Save")}</button><button
              class="is-danger acp-end"
              onclick={() =>
                removeRow(del(`/announcements/${a.id}`), () => (notices = notices.filter((x) => x.id !== a.id)))}
            >
              {tr("Delete")}
            </button>
          </div>
        </div>
      </section>
    {:else}
      <Empty {loaded} title={tr("No announcements yet.")} text={tr("Use New announcement above to add one.")} />
    {/each}
  </div>
{/if}
