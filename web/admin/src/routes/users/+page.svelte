<script>
  import { untrack } from "svelte";
  import { tn, tr } from "#lib/i18n.js";
  import { followTabHash } from "#lib/tabs.svelte.js";
  import When from "#lib/When.svelte";
  import Box from "#lib/Box.svelte";
  import Field from "#lib/Field.svelte";
  import Empty from "#lib/Empty.svelte";
  import PageHead from "#lib/PageHead.svelte";
  import { Status } from "#lib/status.svelte.js";
  import { resolve } from "$app/paths";
  import { get } from "#lib/api.js";

  const base = resolve("/").replace(/\/$/, "");
  const FILTERS = [
    ["", tr("All")],
    ["new", tr("New")],
    ["staff", tr("Staff")],
    ["pending", tr("Awaiting approval")],
    ["suspended", tr("Suspended")],
    ["silenced", tr("Silenced")],
    ["unverified", tr("Unverified")],
  ];
  let q = $state("");
  let filter = $state("");
  let sort = $state("");
  let page = $state(1);
  let users = $state([]);
  let groups = $state([]);
  let loaded = $state(false);
  const status = new Status();

  const total = $derived(users[0]?.total ?? 0);
  const pages = $derived(Math.max(1, Math.ceil(total / 50)));
  const groupName = (id) => groups.find((g) => g.id === id)?.name ?? id;
  const active = (iso) => iso && new Date(iso) > new Date();

  async function load() {
    status.clear();
    try {
      const params = new URLSearchParams({ q, status: filter, sort, page: String(page) });
      [users, groups] = await Promise.all([get(`/users?${params}`), groups.length ? groups : get("/groups")]);
    } catch (e) {
      status.fail(e);
    }
    loaded = true;
  }
  const pick = (s) => {
    filter = s;
    page = 1;
    load();
  };
  const go = (n) => {
    page = n;
    load();
  };
  load();
  // `#tab=suspended` opens the list on that filter ("All" is the page without one).
  followTabHash(FILTERS.map(([key]) => key).filter(Boolean), (key) => untrack(() => filter !== key && pick(key)));
</script>

<PageHead title={tr("Members")} {status}>
  {#snippet intro()}{tn("admin.count.members_match", total)}
    {tr("Open one to moderate or change their account.")}{/snippet}
</PageHead>

<div class="acp-tabs" role="tablist">
  {#each FILTERS as [key, label]}<button role="tab" aria-selected={filter === key} onclick={() => pick(key)}>
      {label}
    </button>{/each}
</div>
<Box title={tr("Search members")}>
  <form
    class="acp-form"
    onsubmit={(e) => {
      e.preventDefault();
      page = 1;
      load();
    }}
  >
    <Field label={tr("Username or email")} type="search" bind:value={q} />
    <Field
      label={tr("Sort by")}
      value={sort}
      onchange={(e) => {
        sort = e.currentTarget.value;
        page = 1;
        load();
      }}
    >
      <option value="">{tr("Newest")}</option><option value="seen">{tr("Last seen")}</option><option value="posts">
        {tr("Posts")}
      </option><option value="reputation">{tr("Reputation")}</option><option value="name">{tr("Name")}</option>
    </Field>
    <div class="acp-actions"><button>{tr("Search")}</button></div>
  </form>
</Box>

<table>
  <thead>
    <tr>
      <th>{tr("Member")}</th><th>{tr("Group")}</th><th>{tr("Trust")}</th><th>{tr("Topics")}</th><th>
        {tr("Posts")}
      </th><th>{tr("Reputation")}</th><th>{tr("Last seen")}</th><th>{tr("Joined")}</th><th>{tr("Status")}</th>
    </tr>
  </thead>
  <tbody>
    {#each users as u (u.id)}
      <tr>
        <td>
          <div class="acp-rowitem">
            <img
              class="acp-avatar"
              src="/avatar/{encodeURIComponent(u.username)}.svg"
              width="32"
              height="32"
              alt=""
              loading="lazy"
            />
            <div class="acp-grow">
              <a href="{base}/users/detail/?id={u.id}"><strong>{u.username}</strong></a><br /><small class="muted">
                {u.email}
              </small>
            </div>
          </div>
        </td>
        <td>
          {groupName(u.primary_group_id)}{#if u.secondary_groups.length}<br /><small class="muted">
              + {u.secondary_groups.map(groupName).join(", ")}
            </small>{/if}
        </td>
        <td>{u.trust_level}</td>
        <td>{u.topic_count}</td>
        <td>{u.post_count}</td>
        <td>{u.reputation}</td>
        <td>
          <small>
            {#if u.last_seen_at}<When iso={u.last_seen_at} />{:else}{tr("Never")}{/if}
          </small>
        </td>
        <td><small><When iso={u.joined_at} /></small></td>
        <td>
          <div class="acp-chips">
            {#if !u.approved}<span class="acp-pill is-warn">{tr("Awaiting approval")}</span>{/if}
            {#if active(u.suspended_until)}<span class="acp-pill is-danger">{tr("Account suspended")}</span>{/if}
            {#if active(u.silenced_until)}<span class="acp-pill is-danger">{tr("Account silenced")}</span>{/if}
            {#if u.warning_points > 0}<span class="acp-pill is-warn">
                {tr("{warning_points} warning pts", { warning_points: u.warning_points })}
              </span>{/if}
            {#if !u.email_verified}<span class="acp-pill is-muted">{tr("Not verified")}</span>{/if}
            {#if u.approved && !active(u.suspended_until) && !active(u.silenced_until) && u.email_verified && !u.warning_points}<span
                class="acp-pill is-ok"
              >
                {tr("Active account")}
              </span>{/if}
          </div>
        </td>
      </tr>
    {:else}
      <Empty cols={9} {loaded} title={tr("No members match.")} text={tr("Try another search or filter.")} />
    {/each}
  </tbody>
</table>
{#if pages > 1}<div class="acp-pages">
    <button class="secondary" disabled={page <= 1} onclick={() => go(page - 1)}>{tr("← Previous")}</button><span>
      {tr("Page {page} of {pages}", { page, pages })}
    </span><button class="secondary" disabled={page >= pages} onclick={() => go(page + 1)}>{tr("Next →")}</button>
  </div>{/if}
