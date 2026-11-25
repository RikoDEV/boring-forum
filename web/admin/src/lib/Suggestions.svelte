<script>
  /// The dashboard's suggestions: settings and set-up the site would be better with. A suggestion
  /// that changes a setting has an Apply button, which is the ordinary settings call; the rest
  /// link to the place to fix them. Dismissed ones wait under a heading and can be restored.
  import { tr } from "#lib/i18n.js";
  import { del, get, post, put } from "#lib/api.js";
  import { Status } from "#lib/status.svelte.js";
  import Notices from "#lib/Notices.svelte";
  import Box from "#lib/Box.svelte";
  import IconButton from "#lib/IconButton.svelte";

  const status = new Status();
  let open = $state([]);
  let dismissed = $state([]);
  let busy = $state("");

  const load = async () => {
    try {
      [open, dismissed] = await Promise.all([get("/suggestions"), get("/suggestions?dismissed=true")]);
    } catch {
      // The dashboard shows its own error for a signed-out panel.
    }
  };
  load();

  const apply = async (s) => {
    busy = s.id;
    const done = await status.run(
      () => put(`/settings/${s.change.setting}`, { value: s.change.value }),
      tr("Applied."),
    );
    busy = "";
    if (done !== undefined) await load();
  };
  const hide = async (s) => {
    await status.run(() => post(`/suggestions/${s.id}/dismiss`));
    await load();
  };
  const restore = async (s) => {
    await status.run(() => del(`/suggestions/${s.id}/dismiss`));
    await load();
  };
</script>

{#if open.length || dismissed.length || status.error || status.msg}
  <Box title={tr("Suggestions")}>
    <Notices {status} />
    {#if open.length}
      <ul class="acp-rows">
        {#each open as s (s.id)}
          <li class="acp-suggest">
            <div class="acp-suggest-text">
              <span class={["acp-pill", s.severity === "warn" && "is-warn"]}>
                {s.severity === "warn" ? tr("Important") : tr("Suggestion")}
              </span>
              <strong>{s.title}</strong>
              <p class="muted">{s.detail}</p>
            </div>
            <div class="acp-suggest-actions">
              {#if s.change}<button type="button" disabled={busy === s.id} onclick={() => apply(s)}>
                  {tr("Apply")}
                </button>{/if}
              {#if s.link}<a class="btn secondary" href={s.link}>{tr("Go to")}</a>{/if}
              <IconButton icon="x" label={tr("Dismiss")} onclick={() => hide(s)} />
            </div>
          </li>
        {/each}
      </ul>
    {:else}<p class="acp-empty">{tr("No suggestions right now.")}</p>{/if}
    {#if dismissed.length}
      <details class="acp-suggest-dismissed">
        <summary>{tr("Dismissed suggestions")} ({dismissed.length})</summary>
        <ul class="acp-rows">
          {#each dismissed as s (s.id)}
            <li class="acp-suggest">
              <div class="acp-suggest-text"><strong>{s.title}</strong></div>
              <div class="acp-suggest-actions">
                <button type="button" class="secondary" onclick={() => restore(s)}>{tr("Restore")}</button>
              </div>
            </li>
          {/each}
        </ul>
      </details>
    {/if}
  </Box>
{/if}
