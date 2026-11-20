<script>
  import { tr } from "#lib/i18n.js";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { del, get, post, put } from "#lib/api.js";
  import UserLink from "#lib/UserLink.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import Field from "#lib/Field.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  let tab = $state("channels");
  const status = new Status();
  let channels = $state([]);
  let rules = $state([]);
  let providers = $state([]);
  let flags = $state([]);
  const blank = () => ({ name: "", description: "", access: "open", group_ids: [] });
  let channel = $state(blank());
  let groups = $state([]);
  const ACCESS = { open: tr("All members"), groups: tr("Chosen groups"), invite: tr("Invite only") };
  const groupNames = (c) => c.group_ids.map((id) => groups.find((g) => g.id === id)?.name ?? id).join(", ");
  let rule = $state({ provider: "slack", url: "", token: "", channel: "", filter: "watch", node_id: "", tag: "" });
  let dialog = $state(null);
  const num = (v) => (v === "" || v === null ? null : Number(v));
  const load = () => {
    get("/chat/channels").then((r) => (channels = r), status.fail);
    get("/groups").then((r) => (groups = r), status.fail);
    get("/chat/rules").then((r) => {
      rules = r.rules;
      providers = r.providers;
    }, status.fail);
    get("/chat/flags").then((r) => (flags = r), status.fail);
  };
  function addChannel(e) {
    e.preventDefault();
    status.clear();
    post("/chat/channels", channel).then(() => {
      channel = blank();
      dialog = null;
      load();
    }, status.fail);
  }
  const archive = (c) =>
    put(`/chat/channels/${c.id}`, {
      name: c.name,
      description: c.description,
      access: c.access,
      group_ids: c.group_ids,
      archived: !c.archived,
    }).then(load, status.fail);
  const removeChannel = (c) =>
    confirm(tr("Delete {name} and its messages?", { name: c.name })) &&
    del(`/chat/channels/${c.id}`).then(load, status.fail);
  function addRule(e) {
    e.preventDefault();
    status.clear();
    post("/chat/rules", { ...rule, node_id: num(rule.node_id) }).then(() => {
      rule = { ...rule, url: "", token: "", channel: "", tag: "" };
      dialog = null;
      load();
    }, status.fail);
  }
  const removeRule = (r) => del(`/chat/rules/${r.id}`).then(load, status.fail);
  const testRule = (r) => {
    status.clear();
    post(`/chat/rules/${r.id}/test`).then(() => (status.msg = tr("Sent. Check the other service.")), status.fail);
  };
  const dismiss = (f) => del(`/chat/flags/${f.message_id}`).then(load, status.fail);
  const removeFlagged = (f) => post(`/chat/flags/${f.message_id}/remove`).then(load, status.fail);
  load();
</script>

<PageHead
  title={tr("Chat")}
  intro={tr("Channels, flagged messages, and forum activity sent to other chat services.")}
  status={dialog ? null : status}
  reserve
>
  {#if tab === "channels"}<button
      onclick={() => {
        status.clear();
        dialog = "channel";
      }}
    >
      {tr("New channel")}
    </button>{:else if tab === "rules"}<button
      onclick={() => {
        status.clear();
        dialog = "rule";
      }}
    >
      {tr("New integration")}
    </button>{/if}
</PageHead>
{#if dialog === "channel"}
  <EditDialog
    title={tr("New channel")}
    onclose={() => (dialog = null)}
    onsubmit={addChannel}
    {status}
    save={tr("Add channel")}
  >
    <div class="acp-form">
      <Field label={tr("Name")} bind:value={channel.name} required />
      <Field label={tr("Description")} bind:value={channel.description} />
      <Field label={tr("Who can see it")} bind:value={channel.access}>
        <option value="open">{tr("All members")}</option><option value="groups">{tr("Chosen groups")}</option><option
          value="invite"
        >
          {tr("Invite only: you add people from the channel")}
        </option>
      </Field>
      {#if channel.access === "groups"}<div class="acp-wide">
          <MultiSelect
            label={tr("Groups")}
            placeholder={tr("No groups")}
            options={groups.map((g) => ({ value: g.id, label: g.name }))}
            bind:value={channel.group_ids}
          />
        </div>{/if}
    </div>
  </EditDialog>
{:else if dialog === "rule"}
  <EditDialog
    title={tr("New integration")}
    onclose={() => (dialog = null)}
    onsubmit={addRule}
    {status}
    save={tr("Add rule")}
  >
    <div class="acp-form">
      <Field label={tr("Service")} bind:value={rule.provider}>
        {#each providers as p}<option>{p}</option>{/each}
      </Field>
      <Field label={tr("Filter")} bind:value={rule.filter}>
        <option>{tr("watch")}</option><option>{tr("follow")}</option><option>{tr("mute")}</option>
      </Field>
      <Field label={tr("Webhook or server address")} type="url" bind:value={rule.url} />
      <Field label={tr("Token")} type="password" bind:value={rule.token} autocomplete="off" />
      <Field label={tr("Channel, chat or room")} bind:value={rule.channel} />
      <Field label={tr("Forum id (empty for all)")} type="number" bind:value={rule.node_id} />
      <Field label={tr("Tag (empty for all)")} bind:value={rule.tag} />
    </div>
  </EditDialog>
{/if}
<div class="acp-tabs" role="tablist">
  {#each [["channels", tr("Channels"), channels.length], ["rules", tr("Integrations"), rules.length], ["flags", tr("Flags"), flags.length]] as [k, label, n]}<button
      role="tab"
      aria-selected={tab === k}
      onclick={() => (tab = String(k))}
    >
      {label} ({n})
    </button>{/each}
</div>

{#if tab === "channels"}
  <table class="acp-table">
    <thead>
      <tr><th>{tr("Name")}</th><th>{tr("Who can see it")}</th><th>{tr("Messages")}</th><th></th></tr>
    </thead><tbody>
      {#each channels as c}<tr>
          <td>
            {c.name}{#if c.archived}
              {tr("(archived)")}{/if}
          </td><td>
            {ACCESS[c.access]}{#if c.access === "groups"}: {groupNames(c)}{/if}
          </td><td>{c.messages}</td><td>
            <button class="btn" onclick={() => archive(c)}>{c.archived ? tr("Restore") : tr("Archive")}</button>
            <button class="btn" onclick={() => removeChannel(c)}>{tr("Delete")}</button>
          </td>
        </tr>{/each}
    </tbody>
  </table>
{:else if tab === "rules"}
  <p class="muted">
    <small>
      {tr("Only topics guests can read are sent.")} <strong>{tr("Follow")}</strong>
      {tr("sends new topics,")} <strong>{tr("watch")}</strong>
      {tr("every post, and")} <strong>{tr("mute")}</strong>
      {tr(
        "stops that service for what it matches. Telegram needs a bot token and chat id; Matrix a homeserver address, access token and room id; Zulip a server address,",
      )} <code>bot-email:api-key</code>
      {tr("as token, and")} <code>stream/topic</code>{tr(". The others need an incoming webhook address.")}
    </small>
  </p>
  <table class="acp-table">
    <thead>
      <tr><th>{tr("Service")}</th><th>{tr("Sends to")}</th><th>{tr("Filter")}</th><th>{tr("Scope")}</th><th></th></tr>
    </thead><tbody>
      {#each rules as r}<tr>
          <td>{r.provider}</td><td>{r.host} {r.channel}</td><td>{r.filter}</td><td>
            {r.node_id ?? tr("All forums")}{#if r.tag}
              · {r.tag}{/if}
          </td><td>
            <button class="btn" onclick={() => testRule(r)}>{tr("Test")}</button>
            <button class="btn" onclick={() => removeRule(r)}>{tr("Delete")}</button>
          </td>
        </tr>{/each}
    </tbody>
  </table>
{:else}
  <table class="acp-table">
    <thead>
      <tr><th>{tr("Message")}</th><th>{tr("By")}</th><th>{tr("Flagged by")}</th><th>{tr("Reason")}</th><th></th></tr>
    </thead><tbody>
      {#each flags as f}<tr>
          <td><a href="/chat/{f.channel_id}">{f.raw}</a></td><td><UserLink name={f.author} /></td><td>
            <UserLink name={f.flagged_by} />
          </td><td>{f.reason}</td><td>
            <button class="btn" onclick={() => dismiss(f)}>{tr("Keep")}</button>
            <button class="btn" onclick={() => removeFlagged(f)}>{tr("Remove")}</button>
          </td>
        </tr>{:else}<tr><td colspan="5" class="muted">{tr("Nothing flagged.")}</td></tr>{/each}
    </tbody>
  </table>
{/if}
