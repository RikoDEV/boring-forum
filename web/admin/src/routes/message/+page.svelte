<script>
  import { tr, locale } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import { get, post } from "#lib/api.js";
  import PageHead from "#lib/PageHead.svelte";
  import Box from "#lib/Box.svelte";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";
  import Section from "#lib/Section.svelte";
  import MultiSelect from "#lib/MultiSelect.svelte";
  import EditDialog from "#lib/EditDialog.svelte";
  import UserLink from "#lib/UserLink.svelte";
  import When from "#lib/When.svelte";
  import Empty from "#lib/Empty.svelte";

  const status = new Status();
  const dlg = new Status();
  let groups = $state([]);
  let levels = $state([]);
  let history = $state([]);
  let loaded = $state(false);
  let composing = $state(false);
  let confirming = $state(false);
  let subject = $state("");
  let body = $state("");
  let notice = $state(true);
  let conversation = $state(false);
  const audience = $state({
    groups: [],
    trust_min: "",
    trust_max: "",
    active_days: "",
    min_posts: 0,
    verified_only: true,
  });
  let reach = $state(null); // { recipients, emails } for the filters as they stand
  const num = (n) => Number(n ?? 0).toLocaleString(locale());

  const payload = () => ({
    groups: audience.groups,
    trust_min: audience.trust_min === "" ? null : Number(audience.trust_min),
    trust_max: audience.trust_max === "" ? null : Number(audience.trust_max),
    active_days: audience.active_days === "" || audience.active_days === null ? null : Number(audience.active_days),
    min_posts: Math.max(0, Number(audience.min_posts) || 0),
    verified_only: audience.verified_only,
  });

  async function loadAll() {
    try {
      const [g, t, h] = await Promise.all([get("/groups"), get("/trust"), get("/mass-message")]);
      groups = g.filter((x) => x.id > 1);
      levels = t.levels;
      history = h;
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }

  // How many members the filters reach, a moment after they stop changing.
  let timer;
  $effect(() => {
    const filters = payload();
    clearTimeout(timer);
    timer = setTimeout(async () => {
      try {
        reach = await post("/mass-message/preview", { audience: filters });
      } catch {
        reach = null;
      }
    }, 350);
    return () => clearTimeout(timer);
  });

  // A message still being delivered refreshes the list until it is done.
  $effect(() => {
    if (!history.some((m) => m.status === "running")) return;
    const t = setInterval(async () => {
      try {
        history = await get("/mass-message");
      } catch {
        /* the next tick tries again */
      }
    }, 3000);
    return () => clearInterval(t);
  });

  function open() {
    status.clear();
    dlg.clear();
    composing = true;
  }

  /** The first step: check the text is there, then show what will be sent. */
  function review(e) {
    e.preventDefault();
    dlg.clear();
    if (!body.trim()) return void (dlg.error = tr("Write the text of the message."));
    confirming = true;
  }

  async function send(e) {
    e.preventDefault();
    dlg.clear();
    try {
      await post("/mass-message", { subject, body, audience: payload(), notice, conversation });
      composing = confirming = false;
      subject = body = "";
      status.msg = tr("On its way: the message is being delivered.");
      history = await get("/mass-message");
    } catch (err) {
      dlg.fail(err);
    }
  }

  const levelOptions = $derived([["", tr("Any")], ...levels.map((l) => [String(l.level), `${l.level} · ${l.label}`])]);
  const STATE = { running: tr("Sending"), done: tr("Delivered"), failed: tr("Failed") };
  loadAll();
  import MarkdownField from "#lib/MarkdownField.svelte";
</script>

<PageHead
  title={tr("Mass message")}
  intro={tr(
    "Write to many members at once: a notice with an email, a private message, or both. Members can switch the email off in their notification settings, and every email has an unsubscribe link.",
  )}
  status={composing ? null : status}
>
  <button onclick={open}>{tr("New message")}</button>
</PageHead>

{#if composing}
  <EditDialog
    title={confirming ? tr("Send this message?") : tr("New mass message")}
    size="is-large"
    status={dlg}
    save={confirming ? tr("Send") : tr("Review")}
    onclose={() => {
      composing = false;
      confirming = false;
    }}
    onsubmit={(e) => (confirming ? send(e) : review(e))}
  >
    {#if confirming}
      <div class="acp-box-body">
        <p><strong>{subject}</strong></p>
        <p class="muted acp-wrap" style:white-space="pre-wrap">{body}</p>
        <p>
          {tr("Recipients")}: <strong>{num(reach?.recipients)}</strong>
          {#if notice}· {tr("With email")}: <strong>{num(reach?.emails)}</strong>{/if}
          {#if conversation}· {tr("Each also gets a private message from you.")}{/if}
        </p>
        <p class="muted">{tr("This cannot be undone once sending starts.")}</p>
      </div>
    {:else}
      <div class="acp-form">
        <Field label={tr("Subject")} bind:value={subject} maxlength="150" required wide />
        <MarkdownField label={tr("Text")} bind:value={body} rows="8" maxlength="20000" />
      </div>
      <Section legend={tr("How it is sent")} switches>
        <Switch label={tr("Notice and email")} bind:checked={notice} />
        <Switch label={tr("Private message to each member")} bind:checked={conversation} />
      </Section>
      <Section legend={tr("Who gets it")} grid>
        <div class="acp-wide">
          <MultiSelect
            label={tr("Groups")}
            placeholder={tr("Any group")}
            options={groups.map((g) => ({ value: g.id, label: g.name }))}
            bind:value={audience.groups}
          />
        </div>
        <Field label={tr("Trust level from")} bind:value={audience.trust_min} options={levelOptions} />
        <Field label={tr("Trust level up to")} bind:value={audience.trust_max} options={levelOptions} />
        <Field label={tr("Seen within days")} type="number" min="1" bind:value={audience.active_days} />
        <Field label={tr("At least this many posts")} type="number" min="0" bind:value={audience.min_posts} />
      </Section>
      <Section switches>
        <Switch label={tr("Only confirmed email addresses")} bind:checked={audience.verified_only} />
      </Section>
      <p class="muted acp-reach">
        {#if reach}
          {tr("Recipients")}: <strong>{num(reach.recipients)}</strong>
          {#if notice}· {tr("With email")}: <strong>{num(reach.emails)}</strong>{/if}
        {:else}…{/if}
      </p>
    {/if}
  </EditDialog>
{/if}

<Box title={tr("Sent messages")}>
  {#if history.length}
    <ul class="acp-rows">
      {#each history as m (m.id)}
        <li class="acp-rowitem">
          <div class="acp-grow">
            <strong class="acp-wrap">{m.subject}</strong>
            <br /><small class="muted">
              <UserLink name={m.sender} fallback={tr("Staff")} /> · <When iso={m.created_at} /> ·
              {num(m.delivered)} / {num(m.recipients)}
            </small>
          </div>
          <span class="acp-pill" class:is-ok={m.status === "done"} class:is-danger={m.status === "failed"}>
            {STATE[m.status] ?? m.status}
          </span>
        </li>
      {/each}
    </ul>
  {:else}
    <Empty {loaded} title={tr("Nothing sent yet.")} text={tr("Messages you send to members are listed here.")} />
  {/if}
</Box>
