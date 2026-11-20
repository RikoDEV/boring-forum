<script>
  import { tr, locale } from "#lib/i18n.js";
  // Site traffic in the manner of Discourse's: the headline and its change, four rates, page views
  // by day stacked by who looked, and the three lists of where visitors come from and land.
  // Crawlers are drawn but left out of every number except the chart's own.
  /** @type {{ data: any }} */ let { data } = $props();

  const SERIES = [
    { key: "member", label: () => tr("Logged in") },
    { key: "anonymous", label: () => tr("Anonymous") },
    { key: "crawler", label: () => tr("Crawlers") },
  ];
  const shown = $state({ member: true, anonymous: true, crawler: false });
  let active = $state(null);

  const compact = (n) => new Intl.NumberFormat(locale(), { notation: "compact", maximumFractionDigits: 1 }).format(n);
  const whole = (n) => n.toLocaleString(locale());
  const day = (iso) => new Date(`${iso}T00:00:00`).toLocaleDateString(locale(), { day: "numeric", month: "short" });
  const total = (d) => SERIES.reduce((n, s) => n + (shown[s.key] ? d[s.key] : 0), 0);

  const change = $derived(
    data.views_before ? Math.round(((data.views - data.views_before) / data.views_before) * 100) : null,
  );
  const top = $derived.by(() => {
    const max = Math.max(1, ...data.daily.map(total));
    // The next round number up (1, 2, 5, 10 ...), so the grid lines fall on whole values.
    const step = 10 ** Math.floor(Math.log10(max / 4 || 1));
    const nice = [1, 2, 5, 10].map((m) => m * step).find((s) => s * 4 >= max) ?? step * 10;
    return nice * 4;
  });
  const ticks = $derived([0, 1, 2, 3, 4].map((i) => (top / 4) * i));
  // Every day's label when they fit; every few days on a long range.
  let narrow = $state(false);
  $effect(() => {
    const q = window.matchMedia("(max-width: 40rem)");
    const set = () => (narrow = q.matches);
    set();
    q.addEventListener("change", set);
    return () => q.removeEventListener("change", set);
  });
  const every = $derived(Math.ceil(data.daily.length / (narrow ? 8 : 30)));
  const duration = (s) => (s < 60 ? tr("{n} s", { n: s }) : tr("{m} min {s} s", { m: Math.floor(s / 60), s: s % 60 }));
  const pct = (n) => (data.visits ? Math.round((n / data.visits) * 100) : 0);
  const country = (code) => {
    try {
      return new Intl.DisplayNames([locale()], { type: "region" }).of(code) ?? code;
    } catch {
      return code;
    }
  };
  const empty = $derived(!data.daily.some((d) => d.member + d.anonymous + d.crawler > 0));
</script>

<section class="acp-box acp-traffic">
  <div class="acp-box-head"><h2 class="acp-eyebrow">{tr("Site traffic")}</h2></div>
  <div class="acp-traffic-body">
    <div class="acp-traffic-top">
      <p class="acp-traffic-headline">
        {tr("{views} pageviews in the last {days} days", {
          views: compact(data.views),
          days: data.days,
        })}{#if change !== null}&nbsp;— <span class:is-up={change >= 0} class:is-down={change < 0}
            >{change >= 0 ? tr("up {pct}%", { pct: change }) : tr("down {pct}%", { pct: -change })}</span
          >{/if}
      </p>
      <dl class="acp-traffic-rates">
        <div>
          <dd>{data.member_share === null ? "—" : `${data.member_share}%`}</dd>
          <dt>{tr("Logged-in share")}</dt>
        </div>
        <div>
          <dd>{data.direct_share === null ? "—" : `${data.direct_share}%`}</dd>
          <dt>{tr("Direct traffic")}</dt>
        </div>
        <div>
          <dd>{data.bounce_rate === null ? "—" : `${data.bounce_rate}%`}</dd>
          <dt>{tr("Bounce rate")}</dt>
        </div>
        <div>
          <dd>{data.avg_session_seconds === null ? "—" : duration(data.avg_session_seconds)}</dd>
          <dt>{tr("Avg. visit length")}</dt>
        </div>
      </dl>
    </div>

    <div class="acp-traffic-plot">
      <div class="acp-traffic-y" aria-hidden="true">
        {#each [...ticks].reverse() as t}<span>{whole(t)}</span>{/each}
      </div>
      <div class="acp-traffic-grid">
        <div class="acp-traffic-lines" aria-hidden="true">
          {#each ticks as _}<i></i>{/each}
        </div>
        <div
          class="acp-traffic-bars"
          role="img"
          aria-label={tr("{title}: {total} in the last {length} days", {
            title: tr("Page views"),
            total: data.views,
            length: data.days,
          })}
        >
          {#each data.daily as d, i}
            <div
              class="acp-traffic-col"
              class:is-active={active === i}
              role="presentation"
              onmouseenter={() => (active = i)}
              onmouseleave={() => (active = null)}
            >
              {#each [...SERIES].reverse() as s}
                {#if shown[s.key] && d[s.key] > 0}
                  <b class="is-{s.key}" style="height: {(d[s.key] / top) * 100}%"></b>
                {/if}
              {/each}
              {#if active === i}
                <span class="acp-traffic-tip" class:is-end={i > data.daily.length / 2}>
                  <strong>{day(d.day)}</strong>
                  {#each SERIES as s}{#if shown[s.key]}<span class="is-{s.key}">{s.label()}: {whole(d[s.key])}</span
                      >{/if}{/each}
                </span>
              {/if}
            </div>
          {/each}
        </div>
        <div class="acp-traffic-x" aria-hidden="true">
          {#each data.daily as d, i}<span>{i % every === 0 ? day(d.day) : ""}</span>{/each}
        </div>
      </div>
    </div>
    {#if empty}<p class="acp-empty">
        {tr("No page views recorded yet: they are counted as people browse the site.")}
      </p>{/if}

    <div class="acp-traffic-legend" role="group" aria-label={tr("Show in the chart")}>
      {#each SERIES as s}
        <button class="secondary is-{s.key}" aria-pressed={shown[s.key]} onclick={() => (shown[s.key] = !shown[s.key])}
          ><i></i>{s.label()}</button
        >
      {/each}
    </div>

    <div class="acp-traffic-lists">
      <section>
        <h3>{tr("Top referrers")}</h3>
        {#if data.referrers.length}<ul>
            {#each data.referrers as r}<li>
                <span>{r.name}</span><b>{pct(r.views)}% <small>({whole(r.views)})</small></b>
              </li>{/each}
          </ul>{:else}<p class="acp-empty">{tr("None yet")}</p>{/if}
      </section>
      <section>
        <h3>{tr("Top entry URLs")}</h3>
        {#if data.entries.length}<ul>
            {#each data.entries as r}<li>
                <a href={r.name} title={r.name}>{r.name}</a><b>{pct(r.views)}% <small>({whole(r.views)})</small></b>
              </li>{/each}
          </ul>{:else}<p class="acp-empty">{tr("None yet")}</p>{/if}
      </section>
      <section>
        <h3>{tr("Top countries")}</h3>
        {#if data.countries.length}<ul>
            {#each data.countries as r}<li>
                <span><abbr>{r.name}</abbr> {country(r.name)}</span><b
                  >{Math.round((r.views / Math.max(1, data.views)) * 100)}% <small>({whole(r.views)})</small></b
                >
              </li>{/each}
          </ul>{:else}<p class="acp-empty">{tr("None yet")}</p>{/if}
      </section>
    </div>
  </div>
</section>
