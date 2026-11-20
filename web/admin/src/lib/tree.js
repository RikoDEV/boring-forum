// Trees of forums and categories: rows with `id`, `parent_id` and
// `position`, ordered depth-first with each row's `depth`, as the Forums page lists them.
// For drawing tree lines, each row also has `last` (the last of its siblings) and `guides`: one
// entry per ancestor level below the top, true where that ancestor has siblings still to come,
// so a line runs down past this row.
export function orderTree(rows, compare = (a, b) => a.position - b.position || a.id - b.id) {
  const out = [];
  const walk = (parent, depth, guides) => {
    if (depth > 20) return;
    const kids = rows.filter((r) => (r.parent_id ?? null) === parent).sort(compare);
    kids.forEach((row, i) => {
      const last = i === kids.length - 1;
      out.push({ ...row, depth, last, guides });
      // Top-level rows start their own trees: no line joins them to each other.
      walk(row.id, depth + 1, depth === 0 ? [] : [...guides, !last]);
    });
  };
  walk(null, 0, []);
  return out;
}

// A row's name indented by its depth, for option lists.
export const indented = (row) => `${"— ".repeat(row.depth)}${row.title}`;
