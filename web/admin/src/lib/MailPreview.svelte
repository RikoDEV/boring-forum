<script>
  /// Sample mails as members get them: the HTML in a frame of its own (the server gives that page
  /// its own policy, and it runs no script), the plain text under it, and a test to oneself.
  import { tr } from "#lib/i18n.js";
  import { get, post } from "#lib/api.js";
  import { resolve } from "$app/paths";
  import Field from "#lib/Field.svelte";
  import Switch from "#lib/Switch.svelte";

  /** @type {{ status: import("#lib/status.svelte.js").Status }} */
  let { status } = $props();

  const KINDS = [
    ["reply", tr("Reply in a topic")],
    ["digest", tr("Digest")],
    ["login", tr("New sign-in")],
    ["security", tr("Security notice")],
    ["verify", tr("Email confirmation")],
    ["reset", tr("Password reset")],
  ];
  const LANGUAGES = [
    ["en", "English"],
    ["pl", "Polski"],
    ["ar", "العربية"],
  ];
  const WIDTHS = [
    ["desktop", tr("Desktop")],
    ["phone", tr("Phone")],
  ];

  let kind = $state("reply");
  let lang = $state("en");
  let width = $state("desktop");
  let dark = $state(false);
  let sample = $state({ subject: "", text: "" });
  // The settings behind the logo and colour are changed elsewhere; a reload shows them.
  const frame = $derived(
    `${resolve("/").replace(/\/admin\/?$/, "")}/api/v1/admin/mail/preview/page?${new URLSearchParams({ kind, lang, dark: String(dark) })}`,
  );
  $effect(() => {
    get(`/mail/preview?${new URLSearchParams({ kind, lang })}`).then((s) => (sample = s), status.fail);
  });
  const send = (e) => {
    e.preventDefault();
    status.run(
      () => post("/mail/preview", { kind, lang }),
      (sent) => tr("Queued for {testTo}", { testTo: sent.to }),
    );
  };
</script>

<section class="acp-box">
  <div class="acp-box-head">
    <h2>{tr("Preview")}</h2>
    <a class="muted" href="#tab=settings"><small>{tr("Logo and colour")}</small></a>
  </div>
  <form class="acp-form" onsubmit={send}>
    <Field label={tr("Email")} bind:value={kind} options={KINDS} />
    <Field label={tr("Language")} bind:value={lang} options={LANGUAGES} />
    <Field label={tr("Width")} bind:value={width} options={WIDTHS} />
    <Switch label={tr("Dark colours")} bind:checked={dark} wide />
    <div class="acp-actions"><button>{tr("Send this to me")}</button></div>
  </form>
</section>
<section class="acp-box">
  <div class="acp-box-head"><h2 dir="auto">{sample.subject}</h2></div>
  <div class="acp-mail-preview">
    <iframe class={["acp-mail-frame", width === "phone" && "is-phone"]} title={tr("Preview")} sandbox="" src={frame}
    ></iframe>
  </div>
  <details class="acp-mail-text">
    <summary>{tr("Plain text version")}</summary>
    <pre dir="auto">{sample.text}</pre>
  </details>
</section>
