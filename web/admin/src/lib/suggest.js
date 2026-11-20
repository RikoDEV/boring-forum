// Sources of suggestions for <MultiSelect suggest={…}>: the same search the site's own inputs use
// (message recipients, @mentions, tags), through the signed-in admin's session.

/** @returns {Promise<{ label: string, name?: string, group?: boolean }[]>} */
async function search(kind, q) {
  if (!q) return [];
  const res = await fetch(`/api/v1/autocomplete?kind=${kind}&q=${encodeURIComponent(q)}`, { headers: { Accept: "application/json" } });
  return res.ok ? await res.json() : [];
}

/** Members by name. The value is the username; a member's display name is the hint. */
export const members = async (/** @type {string} */ q) =>
  (await search("user", q))
    .filter((r) => !r.group)
    .map((r) => ({ value: r.label, label: r.label, hint: r.name && r.name !== r.label ? r.name : undefined }));

/** Tags by name. The value is the tag's name. */
export const tags = async (/** @type {string} */ q) => (await search("tag", q)).map((r) => ({ value: r.label, label: r.label }));
