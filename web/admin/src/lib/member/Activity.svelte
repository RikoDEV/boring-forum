<script>
  /// A member's activity on the details page: what they have written, how it was received, the
  /// badges they hold and the reports made about their posts.
  import { tr, locale } from "#lib/i18n.js";
  import { when } from "#lib/api.js";

  /** @type {{ ov: any }} */
  let { ov } = $props();
  const a = $derived(ov?.activity ?? {});
  const num = (n) => Number(n ?? 0).toLocaleString(locale());
</script>

{#if ov}
  <div class="acp-tiles">
    {#each [[tr("Likes received"), num(a.likes_received)], [tr("Likes given"), num(a.likes_given)], [tr("Solved answers"), num(a.solved)], [tr("Reports against them"), num(a.reports_against)], [tr("Reports they filed"), num(a.reports_filed)]] as [label, n]}
      <div class="acp-tile" style:grid-template-columns="1fr"><b>{n}</b><span>{label}</span></div>
    {/each}
  </div>

  <div class="acp-cols">
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Recent posts")}</h2>
        {#if a.last_post_at}<span class="muted">{tr("Last post")}: {when(a.last_post_at)}</span>{/if}
      </div>
      {#if ov.recent_posts.length}
        <ul class="acp-rows">
          {#each ov.recent_posts as p (p.id)}
            <li class="acp-rowitem">
              <div class="acp-grow">
                <a href={p.url}>{p.title}</a>
                {#if p.is_topic}<span class="acp-pill is-muted">{tr("Topic")}</span>{/if}
                {#if p.state !== "visible"}<span class="acp-pill is-warn">{p.state}</span>{/if}
                <br /><small class="muted acp-wrap">{p.excerpt}</small>
              </div>
              <small class="muted">{when(p.created_at)}</small>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="acp-empty">{tr("No posts yet.")}</p>
      {/if}
    </section>

    <div>
      <section class="acp-box">
        <div class="acp-box-head"><h2>{tr("Badges")}</h2></div>
        {#if ov.badges.length}
          <div class="acp-box-body acp-chips">
            {#each ov.badges as b}<span class="acp-pill" title={when(b.earned_at)}>{b.name}</span>{/each}
          </div>
        {:else}
          <p class="acp-empty">{tr("No badges yet.")}</p>
        {/if}
      </section>

      <section class="acp-box">
        <div class="acp-box-head"><h2>{tr("Reports about their posts")}</h2></div>
        {#if ov.reports_against.length}
          <ul class="acp-rows">
            {#each ov.reports_against as r (r.id)}
              <li class="acp-rowitem">
                <div class="acp-grow">
                  <a href={r.url}>{r.title}</a>
                  <br /><small class="muted">{r.reason}</small>
                </div>
                <span class="acp-pill" class:is-muted={r.status !== "open" && r.status !== "pending"}>{r.status}</span>
                <small class="muted">{when(r.created_at)}</small>
              </li>
            {/each}
          </ul>
        {:else}
          <p class="acp-empty">{tr("No reports.")}</p>
        {/if}
      </section>
    </div>
  </div>
{/if}
