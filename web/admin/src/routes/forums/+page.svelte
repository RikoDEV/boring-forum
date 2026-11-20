<script>
  import { tick } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { orderTree } from "#lib/tree.js";
  import TreeSelect from "#lib/TreeSelect.svelte";
  import { get, post, put, patch, del, uploadFile } from "#lib/api.js";
  import IconPicker from "#lib/IconPicker.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Section from "#lib/Section.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import MarkdownField from "#lib/MarkdownField.svelte";

  let nodes = $state([]);
  let bits = $state({});
  let groups = $state([]);
  let selected = $state(null);
  let tab = $state("general");
  // `#node=<id>` in the address picks a forum or category when the page opens.
  let wanted = typeof location === "undefined" ? null : (/^#node=(\d+)$/.exec(location.hash)?.[1] ?? null);
  let grants = $state({});
  let mods = $state("");
  const status = new Status();
  const fresh = $state({ kind: "forum", title: "", parent_id: "" });

  const PERMS = ["view", "read", "create", "reply", "attach", "download", "moderate"];

  // Depth-first order with indentation for the tree.
  const ordered = $derived(orderTree(nodes));

  async function load() {
    try {
      const [t, g] = await Promise.all([get("/tree"), get("/groups")]);
      nodes = t.nodes;
      bits = t.bits;
      groups = g;
      if (selected) select(nodes.find((n) => n.id === selected.id));
      else if (wanted) {
        // Arrived from a forum page's quick edit ("All settings"): that forum is already chosen.
        const n = nodes.find((x) => x.id === Number(wanted));
        wanted = null;
        if (n) {
          tab = "general";
          await select(n);
          await tick();
          document.querySelector('.acp-tree [aria-current="true"]')?.scrollIntoView({ block: "center" });
        }
      }
    } catch (e) {
      status.fail(e);
    }
  }

  async function select(n) {
    selected = n ? { ...n } : null;
    if (!n) return;
    grants = Object.fromEntries(
      groups.map((g) => [g.id, Object.fromEntries(PERMS.map((p) => [p, ((n.grants[g.id] ?? 0) & bits[p]) !== 0]))]),
    );
    mods = (await get(`/nodes/${n.id}/moderators`)).map((m) => m.username).join(", ");
  }

  async function run(fn, done) {
    status.clear();
    try {
      await fn();
      status.msg = done;
      await load();
    } catch (e) {
      status.fail(e);
    }
  }

  // The tree ends at the bottom of the window. Its height is fixed from where its column starts on
  // the page (below the heading), so it never changes while the page scrolls.
  function fitHeight(node) {
    const wide = matchMedia("(min-width: 1101px)");
    const fit = () => {
      const top = node.parentElement.getBoundingClientRect().top + scrollY;
      node.style.maxBlockSize = wide.matches ? `${Math.max(160, innerHeight - top - 24)}px` : "";
    };
    fit();
    // Anything above the tree changing size (the heading, a status message, late fonts) moves it.
    const watch = new ResizeObserver(fit);
    watch.observe(document.body);
    watch.observe(node.parentElement);
    addEventListener("resize", fit);
    return {
      destroy() {
        watch.disconnect();
        removeEventListener("resize", fit);
      },
    };
  }

  // Reordering: grab a row and drop it above or below another row (it joins that row's parent), or
  // onto the middle of a row to put it inside. Alt+Up/Down moves the focused row among its siblings,
  // Alt+Left takes it out of its parent and Alt+Right puts it inside the sibling above. A parent
  // takes its children along.
  let drag = $state(null);
  let over = $state(null);
  const parentOf = (n) => n.parent_id ?? null;
  const siblingsOf = (n) =>
    nodes.filter((x) => parentOf(x) === parentOf(n)).sort((a, b) => a.position - b.position || a.id - b.id);

  const childrenOf = (id) =>
    nodes.filter((x) => x.parent_id === id).sort((a, b) => a.position - b.position || a.id - b.id);
  // Everything below a row: it cannot be dropped into its own subtree.
  function below(id, out = new SvelteSet()) {
    for (const k of childrenOf(id)) if (!out.has(k.id)) below(k.id, out.add(k.id));
    return out;
  }

  async function reorder(parent, ids) {
    status.clear();
    const at = new Map(ids.map((id, i) => [id, i]));
    nodes = nodes.map((x) => (at.has(x.id) ? { ...x, parent_id: parent, position: at.get(x.id) } : x));
    try {
      await put("/nodes/reorder", { parent_id: parent, ids });
    } catch (e) {
      status.fail(e);
      await load();
    }
  }

  function dragOver(e, n) {
    if (!drag || drag.id === n.id || below(drag.id).has(n.id)) return;
    e.preventDefault();
    e.dataTransfer.dropEffect = "move";
    const box = e.currentTarget.getBoundingClientRect();
    const y = (e.clientY - box.top) / box.height;
    over = { id: n.id, at: y < 0.28 ? "before" : y > 0.72 ? "after" : "inside" };
  }

  function drop(e) {
    e.preventDefault();
    if (!drag || !over) return;
    const target = nodes.find((x) => x.id === over.id);
    if (target) {
      if (over.at === "inside") {
        const ids = childrenOf(target.id)
          .map((x) => x.id)
          .filter((id) => id !== drag.id);
        reorder(target.id, [...ids, drag.id]);
      } else {
        const ids = siblingsOf(target)
          .map((x) => x.id)
          .filter((id) => id !== drag.id);
        ids.splice(ids.indexOf(target.id) + (over.at === "after" ? 1 : 0), 0, drag.id);
        reorder(parentOf(target), ids);
      }
    }
    drag = over = null;
  }

  function nudge(e, n) {
    if (!e.altKey || !["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"].includes(e.key)) return;
    e.preventDefault();
    const ids = siblingsOf(n).map((x) => x.id);
    const i = ids.indexOf(n.id);
    if (e.key === "ArrowLeft") {
      const parent = nodes.find((x) => x.id === n.parent_id);
      if (!parent) return;
      const out = siblingsOf(parent)
        .map((x) => x.id)
        .filter((id) => id !== n.id);
      out.splice(out.indexOf(parent.id) + 1, 0, n.id);
      reorder(parentOf(parent), out);
    } else if (e.key === "ArrowRight") {
      if (i < 1) return;
      const into = ids[i - 1];
      reorder(into, [...childrenOf(into).map((x) => x.id), n.id]);
    } else {
      const j = i + (e.key === "ArrowUp" ? -1 : 1);
      if (j < 0 || j >= ids.length) return;
      [ids[i], ids[j]] = [ids[j], ids[i]];
      reorder(parentOf(n), ids);
    }
    const row = e.currentTarget;
    // The list redraws in the new order: keep the keyboard on the row that moved.
    queueMicrotask(() => requestAnimationFrame(() => row.isConnected && row.focus()));
  }

  async function removeNode() {
    const n = selected;
    if (!confirm(tr('Delete "{title}"?', { title: n.title }))) return;
    status.clear();
    try {
      await del(`/nodes/${n.id}`);
      selected = null;
      status.msg = tr('Deleted "{title}"', { title: n.title });
      await load();
    } catch (e) {
      status.fail(e);
    }
  }

  let creating = $state(false);
  async function create(e) {
    e.preventDefault();
    status.clear();
    const body = { kind: fresh.kind, title: fresh.title, parent_id: fresh.parent_id ? Number(fresh.parent_id) : null };
    // Node creation lives on the forum API next to the other admin writes.
    try {
      await post("/nodes", body);
    } catch (err) {
      return status.fail(err);
    }
    fresh.title = "";
    creating = false;
    status.msg = tr('Created {kind} "{title}"', { kind: body.kind, title: body.title });
    await load();
  }

  // An icon from a sprite (not an emoji): it shows in the picker, not in the emoji field.
  const isSpriteIcon = (icon) => /^[a-z0-9-]+$/.test(icon ?? "");
  const saveIcon = () =>
    run(
      () => put(`/nodes/${selected.id}/icon`, { icon: selected.icon ?? "", image: selected.icon_image ?? "" }),
      tr("Icon saved"),
    );
  async function uploadIcon(e) {
    const file = e.currentTarget.files[0];
    if (!file) return;
    try {
      const data = await uploadFile(file);
      selected.icon_image = data.url.split("#")[0];
    } catch (err) {
      status.error = err.message || tr("Upload failed");
    }
  }

  const saveNode = () =>
    run(
      () =>
        patch(`/nodes/${selected.id}`, {
          title: selected.title,
          default_sort: selected.default_sort ?? "",
          seo_title: selected.seo_title ?? "",
          seo_description: selected.seo_description ?? "",
          list_style: selected.list_style,
          template_md: selected.template_md ?? "",
          approve_mode: selected.approve_mode,
          mode: selected.mode,
          auto_close_days: selected.auto_close_days ? Number(selected.auto_close_days) : null,
          min_trust_level: Number(selected.min_trust_level),
          require_tag: selected.require_tag,
          counts_posts: selected.counts_posts,
          parent_id: selected.parent_id ? Number(selected.parent_id) : null,
        }),
      tr("Saved"),
    );

  const parentTitle = $derived(nodes.find((n) => n.id === selected?.parent_id)?.title ?? "");

  const savePerms = () =>
    run(
      () =>
        selected.parent_id && selected.inherit_perms
          ? patch(`/nodes/${selected.id}`, { inherit_perms: true })
          : put(
              `/nodes/${selected.id}/permissions`,
              Object.fromEntries(
                Object.entries(grants).map(([g, ps]) => [g, PERMS.reduce((acc, p) => acc | (ps[p] ? bits[p] : 0), 0)]),
              ),
            ),
      tr("Permissions saved"),
    );

  let prefixes = $state("");
  const savePrefixes = () =>
    run(
      () =>
        put(
          `/nodes/${selected.id}/prefixes`,
          prefixes
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean),
        ),
      tr("Prefixes saved"),
    );

  const saveMods = () =>
    run(
      () =>
        put(`/nodes/${selected.id}/moderators`, {
          usernames: mods
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean),
        }),
      tr("Moderators saved"),
    );

  load();
</script>

<PageHead
  title={tr("Forums")}
  intro={tr("Pick a forum to edit it, its permissions and its moderators.")}
  status={creating ? null : status}
>
  <button
    onclick={() => {
      status.clear();
      creating = true;
    }}
  >
    {tr("New category or forum")}
  </button>
</PageHead>
{#if creating}
  <EditDialog
    title={tr("New category or forum")}
    onclose={() => (creating = false)}
    onsubmit={create}
    {status}
    save={tr("Add")}
  >
    <div class="acp-form">
      <Field label={tr("Kind")} bind:value={fresh.kind}>
        <option value="category">{tr("Category")}</option><option value="forum">{tr("Forum")}</option>
      </Field>
      <Field label={tr("Title")} bind:value={fresh.title} required />
      <TreeSelect rows={nodes} bind:value={fresh.parent_id} label={tr("Inside")} none={tr("Top level")} wide />
    </div>
  </EditDialog>
{/if}

<div class="acp-split">
  <div>
    <div class="acp-box acp-tree-box" use:fitHeight>
      <ul class="acp-tree">
        {#each ordered as n (n.id)}
          <li
            draggable="true"
            class:is-dragging={drag?.id === n.id}
            class:is-drop-before={over?.id === n.id && over.at === "before"}
            class:is-drop-after={over?.id === n.id && over.at === "after"}
            class:is-drop-inside={over?.id === n.id && over.at === "inside"}
            ondragstart={(e) => {
              drag = n;
              e.dataTransfer.effectAllowed = "move";
              e.dataTransfer.setData("text/plain", String(n.id));
            }}
            ondragover={(e) => dragOver(e, n)}
            ondragleave={() => over?.id === n.id && (over = null)}
            ondrop={drop}
            ondragend={() => {
              drag = null;
              over = null;
            }}
          >
            <button
              aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight"
              onkeydown={(e) => nudge(e, n)}
              class:is-category={n.kind === "category"}
              aria-current={selected?.id === n.id ? "true" : undefined}
              onclick={() => select(n)}
            >
              {#each n.guides as line, i (i)}<span class="acp-tree-guide" class:is-line={line} aria-hidden="true"
                ></span>{/each}
              {#if n.depth > 0}<span class="acp-tree-branch" class:is-last={n.last} aria-hidden="true"></span>{/if}
              <span class="acp-grow">{n.title}</span>
              {#if n.kind === "forum"}<small class="muted">{n.topic_count}</small>{:else if n.kind !== "category"}<small
                  class="muted"
                >
                  {n.kind}
                </small>{/if}
            </button>
          </li>
        {/each}
      </ul>
    </div>
  </div>

  {#if selected}
    <div>
      <div class="acp-tabs" role="tablist">
        {#each [["general", tr("General")], ["icon", tr("Icon")], ["behaviour", tr("Posting")], ["permissions", tr("Permissions")], ["people", selected.kind === "forum" ? tr("Prefixes and moderators") : tr("Moderators")], ["seo", "SEO"]] as [key, label]}
          <button role="tab" aria-selected={tab === key} onclick={() => (tab = key)}>{label}</button>
        {/each}
      </div>
      <section class="acp-box">
        <div class="acp-box-head">
          <h2>{selected.title}</h2>
          <small class="muted">
            {selected.kind}{#if selected.plugin}
              · {tr("From plugin {plugin}", { plugin: selected.plugin })}{/if}
          </small>
          <button class="is-danger" onclick={removeNode}>{tr("Delete")}</button>
        </div>
        {#if tab === "general"}
          <div class="acp-form">
            <Field label={tr("Title")} bind:value={selected.title} disabled={!!selected.plugin} />
            <TreeSelect
              rows={nodes}
              bind:value={selected.parent_id}
              label={tr("Inside")}
              none={tr("Top level")}
              noneValue={null}
              exclude={selected.id}
            />
            <Field label={tr("List style")} bind:value={selected.list_style}>
              <option value="table">{tr("Table")}</option><option value="list">{tr("List")}</option>
            </Field>
            <Field label={tr("Default sort")} bind:value={selected.default_sort}>
              <option value={null}>{tr("Latest activity")}</option><option value="newest">{tr("Newest")}</option><option
                value="replies"
              >
                {tr("Most replies")}
              </option><option value="views">{tr("Most views")}</option><option value="likes">
                {tr("Most liked")}
              </option><option value="title">{tr("Title")}</option>
            </Field>
            <div class="acp-actions"><button onclick={saveNode}>{tr("Save")}</button></div>
          </div>
        {:else if tab === "icon"}
          <div class="acp-form">
            <fieldset class="acp-picker acp-wide">
              <legend>{tr("Icon")}</legend>
              {#key selected.id}<IconPicker bind:value={selected.icon} />{/key}
            </fieldset>
            <Field
              label={tr("Or one emoji")}
              value={isSpriteIcon(selected.icon) ? "" : (selected.icon ?? "")}
              oninput={(e) => (selected.icon = e.currentTarget.value)}
              placeholder="🎮"
            />
            <label>
              {tr("Or a picture")}<small>{tr("Takes priority; square works best")}</small><input
                type="file"
                accept="image/png,image/jpeg,image/webp,image/gif"
                onchange={uploadIcon}
              />
            </label>
            {#if selected.icon_image}<div class="acp-rowitem acp-wide">
                <img src={selected.icon_image} alt="" width="48" height="48" style:border-radius="50%" /><button
                  type="button"
                  class="secondary"
                  onclick={() => (selected.icon_image = "")}
                >
                  {tr("Remove picture")}
                </button>
              </div>{/if}
            <div class="acp-actions"><button onclick={saveIcon}>{tr("Save icon")}</button></div>
          </div>
        {:else if tab === "behaviour"}
          <div class="acp-form">
            <Field label={tr("Approval")} bind:value={selected.approve_mode}>
              <option value="none">{tr("Normal rules")}</option><option value="all">
                {tr("Hold all posts")}
              </option><option value="links">{tr("Hold posts with links")}</option>
            </Field>
            <Field label={tr("Mode")} bind:value={selected.mode}>
              <option value="discussion">{tr("Discussion")}</option><option value="qa">
                {tr("Questions and answers (answers voted best first)")}
              </option><option value="ideas">{tr("Ideas (members vote for topics)")}</option>
            </Field>
            <Field
              label={tr("Auto-close after (days)")}
              type="number"
              min="1"
              max="3650"
              bind:value={selected.auto_close_days}
            />
            <Field
              label={tr("Minimum trust level")}
              type="number"
              min="0"
              max="20"
              bind:value={selected.min_trust_level}
            />
            <div class="acp-wide">
              <Section switches>
                <Switch label={tr("Topics need a tag")} bind:checked={selected.require_tag} />
                <Switch label={tr("Posts count toward members' totals")} bind:checked={selected.counts_posts} />
              </Section>
            </div>
            {#key selected.id}
              <MarkdownField
                label={tr("New topic template")}
                hint={tr("Starts every new topic in this forum.")}
                bind:value={selected.template_md}
                rows={8}
              />
            {/key}
            <div class="acp-actions"><button onclick={saveNode}>{tr("Save")}</button></div>
          </div>
        {:else if tab === "permissions"}
          {#if selected.parent_id}
            <Section switches>
              <Switch
                label={tr("Use the permissions of “{parent}”", { parent: parentTitle })}
                bind:checked={selected.inherit_perms}
              />
            </Section>
          {/if}
          <div class="acp-matrix-wrap">
            <table class="acp-matrix" style:margin="0" style:border="0">
              <thead>
                <tr>
                  <th>{tr("Group")}</th>{#each PERMS as p}<th>{p}</th>{/each}
                </tr>
              </thead>
              <tbody>
                {#each groups as g (g.id)}<tr>
                    <td>{g.name}</td>{#each PERMS as p}<td>
                        <input
                          type="checkbox"
                          bind:checked={grants[g.id][p]}
                          disabled={selected.inherit_perms}
                          aria-label="{g.name} {p}"
                        />
                      </td>{/each}
                  </tr>{/each}
              </tbody>
            </table>
          </div>
          <div class="acp-form">
            <div class="acp-actions">
              <button onclick={savePerms}>{tr("Save permissions")}</button><small class="muted">
                {selected.inherit_perms
                  ? tr("Changes to the parent's permissions apply here too. Untick the switch to set your own.")
                  : tr("Guests can never post or moderate, whatever is ticked here.")}
              </small>
            </div>
          </div>
        {:else if tab === "people"}
          <div class="acp-form">
            {#if selected.kind === "forum"}<label class="bf-field acp-wide">
                <input bind:value={prefixes} placeholder={tr("Question, Solved, Bug")} /><span class="bf-field-label">
                  {tr("Topic prefixes")}
                </span><small>{tr("Separated by commas")}</small>
              </label>
              <div class="acp-actions">
                <button class="secondary" onclick={savePrefixes}>{tr("Save prefixes")}</button>
              </div>{/if}
            <label class="bf-field acp-wide">
              <input placeholder=" " bind:value={mods} /><span class="bf-field-label">{tr("Moderators")}</span><small>
                {tr("Usernames, separated by commas")}
              </small>
            </label>
            <div class="acp-actions"><button class="secondary" onclick={saveMods}>{tr("Save moderators")}</button></div>
          </div>
        {:else}
          <div class="acp-form">
            <Field label={tr("SEO title")} bind:value={selected.seo_title} wide />
            <Field label={tr("SEO description")} bind:value={selected.seo_description} rows="2" wide />
            <div class="acp-actions"><button onclick={saveNode}>{tr("Save")}</button></div>
          </div>
        {/if}
      </section>
    </div>
  {:else}
    <section class="acp-box"><p class="acp-empty">{tr("Pick a forum on the left.")}</p></section>
  {/if}
</div>
