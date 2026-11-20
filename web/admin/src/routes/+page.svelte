<script>
  import { tn, tr, locale } from "#lib/i18n.js";
  import { Status } from "#lib/status.svelte.js";
  import { resolve } from "$app/paths";
  import { del, get } from "#lib/api.js";
  import When from "#lib/When.svelte";
  import UserLink from "#lib/UserLink.svelte";
  import BarChart from "#lib/BarChart.svelte";
  import Spark from "#lib/Spark.svelte";
  import Traffic from "#lib/Traffic.svelte";
  const base = resolve("/").replace(/\/$/, "");
  let stats = $state(null);
  let analytics = $state(null);
  let board = $state(null);
  let notices = $state([]);
  const status = new Status();
  const failed = (e) =>
    (status.error = e.status === 403 ? tr("Log in to the forum as an administrator to use this panel.") : e.message);
  const loadNotices = () =>
    get("/notices").then(
      (n) => (notices = n),
      () => {},
    );
  get("/stats").then((s) => (stats = s), failed);
  let days = $state(30);
  let traffic = $state(null);
  const loadAnalytics = () => {
    get(`/traffic?days=${days}`).then(
      (t) => (traffic = t),
      () => {},
    );
    return get(`/analytics?days=${days}`).then(
      (a) => (analytics = a),
      () => {},
    );
  };
  const pick = (n) => {
    days = n;
    loadAnalytics();
  };
  loadAnalytics();
  get("/dashboard").then(
    (b) => (board = b),
    () => {},
  );
  loadNotices();

  const day = (iso) => new Date(`${iso}T00:00:00`).toLocaleDateString(locale(), { month: "short", day: "numeric" });
  const series = (key) => analytics.daily.map((d) => ({ label: day(d.day), value: d[key] }));
  const bytes = (n) => {
    const units = ["B", "KB", "MB", "GB", "TB"];
    const i = n > 0 ? Math.min(Math.floor(Math.log(n) / Math.log(1024)), units.length - 1) : 0;
    return `${(n / 1024 ** i).toFixed(i ? 1 : 0)} ${units[i]}`;
  };
  // Week against the week before; nothing to compare with shows no change line.
  const change = (now, before) => {
    if (!before) return now ? tr("new this week") : "";
    const pct = Math.round(((now - before) / before) * 100);
    return tr("{pct}% on last week", { pct: `${pct > 0 ? "+" : ""}${pct}` });
  };
  const maxForum = $derived(Math.max(1, ...(analytics?.top_forums ?? []).map((f) => f.posts)));
  const maxMember = $derived(Math.max(1, ...(analytics?.top_members ?? []).map((m) => m.posts)));
  const maxLevel = $derived(Math.max(1, ...(analytics?.trust_levels ?? []).map((l) => l.members)));
  const sum = (key) => analytics.daily.reduce((n, d) => n + d[key], 0);
  const hours = $derived(
    (analytics?.by_hour ?? []).map((value, h) => ({ label: `${String(h).padStart(2, "0")}:00`, value })),
  );
  // 2024-01-01 was a Monday: the labels come from the reader's language.
  const weekdays = $derived(
    (analytics?.by_weekday ?? []).map((value, i) => ({
      label: new Date(2024, 0, 1 + i).toLocaleDateString(locale(), { weekday: "short" }),
      value,
    })),
  );
  const age = (hours) =>
    hours == null
      ? tr("None waiting")
      : hours < 48
        ? tr("{n} h", { n: hours })
        : tr("{n} d", { n: Math.round(hours / 24) });
  const answered = $derived(
    analytics?.topics_in_range
      ? Math.round(((analytics.topics_in_range - analytics.unanswered) / analytics.topics_in_range) * 100)
      : null,
  );
</script>

<div class="acp-head">
  <h1>{tr("Dashboard")}</h1>
  <div class="acp-range" role="group" aria-label={tr("Period")}>
    {#each [7, 30, 90] as n}
      <button class="secondary" aria-pressed={days === n} onclick={() => pick(n)}>{tn("admin.count.days", n)}</button>
    {/each}
  </div>
</div>
{#if status.error}<p class="err" role="alert">{status.error} <a href="/login">{tr("Log in")}</a></p>{/if}
{#each notices as n (n.id)}<p class="err row">
    {n.message} <small><When iso={n.created_at} /></small>
    <button class="secondary" onclick={() => del(`/notices/${n.id}`).then(loadNotices)}>{tr("Dismiss")}</button>
  </p>{/each}

<div class="acp-kpis">
  {#if stats}
    <a class="acp-kpi" href="{base}/users/"
      ><span>{tr("Members")}</span><b>{stats.users.toLocaleString(locale())}</b>{#if analytics}<small
          >{tr("{signups_7d} joined this week", { signups_7d: analytics.signups_7d })}</small
        ><Spark data={analytics.daily.map((d) => d.signups)} />{/if}</a
    >
    <a class="acp-kpi" href="{base}/forums/"
      ><span>{tr("Topics")}</span><b>{stats.topics.toLocaleString(locale())}</b>{#if analytics}<small
          >{change(analytics.topics_7d, analytics.topics_prev_7d)}</small
        ><Spark data={analytics.daily.map((d) => d.topics)} />{/if}</a
    >
    <div class="acp-kpi">
      <span>{tr("Posts")}</span><b>{stats.posts.toLocaleString(locale())}</b>{#if analytics}<small
          >{change(analytics.posts_7d, analytics.posts_prev_7d)}</small
        ><Spark data={analytics.daily.map((d) => d.posts)} />{/if}
    </div>
  {/if}
  {#if analytics}
    <div class="acp-kpi">
      <span>{tr("Likes")}</span><b>{analytics.likes_7d.toLocaleString(locale())}</b><small
        >{change(analytics.likes_7d, analytics.likes_prev_7d)}</small
      ><Spark data={analytics.daily.map((d) => d.likes)} />
    </div>
    <div class="acp-kpi">
      <span>{tr("Signed in now")}</span><b>{analytics.sessions.toLocaleString(locale())}</b><small
        >{tr("{n} active in 24 hours", { n: analytics.active_1d })}</small
      >
    </div>
  {/if}
</div>

<div class="acp-kpis acp-kpis-watch">
  {#if stats}
    <a class="acp-kpi" class:is-warn={stats.pending_reviews > 0} href="/review"
      ><span>{tr("Awaiting review")}</span><b>{stats.pending_reviews.toLocaleString(locale())}</b><small
        >{stats.pending_reviews
          ? analytics?.oldest_review_hours != null
            ? tr("Oldest waiting {age}", { age: age(analytics.oldest_review_hours) })
            : tr("Open the queue")
          : tr("All clear")}</small
      ></a
    >
  {/if}
  {#if analytics}
    <div class="acp-kpi">
      <span>{tr("Unanswered topics")}</span><b>{analytics.unanswered.toLocaleString(locale())}</b><small
        >{answered === null
          ? tr("No topics in this period")
          : tr("{pct}% of new topics have a reply", { pct: answered })}</small
      >
    </div>
    <a class="acp-kpi" class:is-warn={analytics.mail_failing > 0} href="{base}/mail/"
      ><span>{tr("Mail waiting to send")}</span><b>{analytics.mail_queued.toLocaleString(locale())}</b><small
        >{analytics.mail_failing ? tr("{n} failing", { n: analytics.mail_failing }) : tr("None failing")}</small
      ></a
    >
    <a class="acp-kpi" href="{base}/logs/"
      ><span>{tr("Failed logins, 24 hours")}</span><b>{analytics.failed_logins_1d.toLocaleString(locale())}</b><small
        >{tr("Over {days} days: {n}", { days: analytics.days, n: sum("failed_logins") })}</small
      ></a
    >
    <a class="acp-kpi" href="{base}/users/"
      ><span>{tr("Restricted members")}</span><b
        >{(analytics.suspended + analytics.silenced).toLocaleString(locale())}</b
      ><small
        >{tr("{n} suspended, {m} silenced", { n: analytics.suspended, m: analytics.silenced })}{analytics.unapproved
          ? ` · ${tr("{n} awaiting approval", { n: analytics.unapproved })}`
          : ""}</small
      ></a
    >
  {/if}
</div>

{#if traffic}<Traffic data={traffic} />{/if}

{#if analytics}
  <div class="acp-charts">
    <section class="acp-box"><BarChart title={tr("Posts")} data={series("posts")} /></section>
    <section class="acp-box"><BarChart title={tr("New topics")} data={series("topics")} /></section>
    <section class="acp-box"><BarChart title={tr("New members")} data={series("signups")} /></section>
    <section class="acp-box"><BarChart title={tr("Likes")} data={series("likes")} /></section>
    <section class="acp-box"><BarChart title={tr("Reports and reviews")} data={series("reports")} /></section>
    {#if sum("chats")}<section class="acp-box">
        <BarChart title={tr("Chat messages")} data={series("chats")} />
      </section>{/if}
    {#if sum("searches")}<section class="acp-box">
        <BarChart title={tr("Searches")} data={series("searches")} />
      </section>{/if}
    {#if sum("failed_logins")}<section class="acp-box">
        <BarChart title={tr("Failed logins")} data={series("failed_logins")} />
      </section>{/if}
    <section class="acp-box">
      <BarChart
        title={tr("Posts by day of the week")}
        data={weekdays}
        span={tn("admin.count.last_days", analytics.days)}
      />
    </section>
    <section class="acp-box">
      <BarChart title={tr("Posts by hour of the day")} data={hours} span={tr("UTC")} />
    </section>
  </div>
{/if}
<div class="acp-masonry">
  {#if analytics}
    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Active members")}</h2></div>
      <dl class="acp-facts">
        <div>
          <dt>{tr("Last 24 hours")}</dt>
          <dd>{analytics.active_1d.toLocaleString(locale())}</dd>
        </div>
        <div>
          <dt>{tr("Last 7 days")}</dt>
          <dd>{analytics.active_7d.toLocaleString(locale())}</dd>
        </div>
        <div>
          <dt>{tr("Last 30 days")}</dt>
          <dd>{analytics.active_30d.toLocaleString(locale())}</dd>
        </div>
        <div>
          <dt>{tr("Uploads")}</dt>
          <dd>
            {analytics.uploads.toLocaleString(locale())} <small class="muted">{bytes(analytics.upload_bytes)}</small>
          </dd>
        </div>
      </dl>
    </section>
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Busiest forums")}</h2>
        <small class="muted">{tr("Posts, {days} days", { days: analytics.days })}</small>
      </div>
      {#if analytics.top_forums.length}
        <ul class="acp-busiest">
          {#each analytics.top_forums as f}
            <li>
              <span>{f.title}</span><b>{f.posts.toLocaleString(locale())}</b><i
                style="width: {(f.posts / maxForum) * 100}%"
              ></i>
            </li>
          {/each}
        </ul>
      {:else}<p class="acp-empty">{tr("No posts in this period.")}</p>{/if}
    </section>
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Most active members")}</h2>
        <small class="muted">{tr("Posts, {days} days", { days: analytics.days })}</small>
      </div>
      {#if analytics.top_members.length}
        <ul class="acp-busiest">
          {#each analytics.top_members as m}
            <li>
              <span><UserLink name={m.username} /></span><b>{m.posts.toLocaleString(locale())}</b><i
                style="width: {(m.posts / maxMember) * 100}%"
              ></i>
            </li>
          {/each}
        </ul>
      {:else}<p class="acp-empty">{tr("No posts in this period.")}</p>{/if}
    </section>
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Members by trust level")}</h2>
      </div>
      <ul class="acp-busiest">
        {#each analytics.trust_levels as l}
          <li>
            <span>{tr("Level {level}", { level: l.level })}</span><b>{l.members.toLocaleString(locale())}</b><i
              style="width: {(l.members / maxLevel) * 100}%"
            ></i>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if board}
    <section class="acp-box">
      <div class="acp-box-head"><h2>{tr("Needs attention")}</h2></div>
      {#if board.problems.length}<ul class="acp-rows">
          {#each board.problems as p}<li>
              {#if p.link}<a href={p.link}>{p.text}</a>{:else}{p.text}{/if}
            </li>{/each}
        </ul>{:else}<p class="acp-empty">{tr("Nothing needs attention.")}</p>{/if}
    </section>
    <section class="acp-box">
      <div class="acp-box-head">
        <h2>{tr("Latest staff actions")}</h2>
        <a class="muted" href="{base}/audit/"><small>{tr("View log")}</small></a>
      </div>
      {#if board.actions.length}<ul class="acp-rows">
          {#each board.actions as a}<li>
              <UserLink name={a.actor} fallback="system" />: {a.action}
              <small class="muted">{a.target ?? ""} · <When iso={a.created_at} /></small>
            </li>{/each}
        </ul>{:else}<p class="acp-empty">{tr("No actions yet.")}</p>{/if}
    </section>
  {/if}
</div>
{#if board}
  <p class="muted">
    <small
      >{tr("Version {version}", { version: board.version })}{#if board.latest && board.latest !== board.version}{tr(
          "; {latest} is available",
          { latest: board.latest },
        )}{/if}</small
    >
  </p>
{/if}
