// Picture grids in the editor: `[grid]` … `[/grid]` around lines of pictures shows them side by side
// (the cooker draws the same). Outside the grid's lines the editor shows the tiles; with the cursor
// inside, the lines are plain text again so pictures can be added, moved or taken out.
import { WidgetType } from "@codemirror/view";
import { mk, tr } from "../dom.js";
import { IMAGE, finders, isPicture, openImageEditor, readImage } from "./image.js";

const OPEN = /^\s*\[grid(?:=([2-6]))?\]\s*$/;
const CLOSE = /^\s*\[\/grid\]\s*$/;

/** The column counts a grid can be given; 0 is automatic: as many as fit. */
export const COLUMNS = [0, 2, 3, 4, 5, 6];

/** The grids of a document: the line that opens each, the line that closes it and the columns it asks for (0: automatic). */
export function gridBlocks(state) {
  const doc = state.doc;
  const out = [];
  let open = null;
  let columns = 0;
  for (let n = 1; n <= doc.lines; n++) {
    const line = doc.line(n);
    const m = OPEN.exec(line.text);
    if (m) {
      open = line;
      columns = m[1] ? Number(m[1]) : 0;
    } else if (open && CLOSE.test(line.text)) {
      out.push({ open, close: line, columns });
      open = null;
    }
  }
  return out;
}

/** Writes how many columns a grid has into the line that opens it. */
export function setColumns(view, block, columns) {
  view.dispatch({
    changes: { from: block.open.from, to: block.open.to, insert: columns ? `[grid=${columns}]` : "[grid]" },
  });
  view.focus();
}

/** The pictures between a grid's two lines, with where each is written. */
export function gridImages(state, block) {
  const out = [];
  for (let n = block.open.number + 1; n < block.close.number; n++) {
    const line = state.doc.line(n);
    for (const m of line.text.matchAll(IMAGE)) {
      if (/^(https?:\/\/|\/)/.test(m[2]) && isPicture(m[2])) {
        out.push({ from: line.from + m.index, to: line.from + m.index + m[0].length, ...readImage(m) });
      }
    }
  }
  return out;
}

/** A line made only of pictures. */
const isPictureLine = (text) => {
  IMAGE.lastIndex = 0;
  const has = IMAGE.test(text);
  IMAGE.lastIndex = 0;
  return has && text.replace(IMAGE, "").trim() === "";
};

/** The grid the position is in, or null. */
export const gridAt = (state, pos) => gridBlocks(state).find((b) => pos >= b.open.from && pos <= b.close.to) ?? null;

/**
 * Puts pictures in a grid: the selected lines when they are all pictures, otherwise the run of picture
 * lines around the cursor. With no picture there it opens an empty grid to put them in.
 * Returns how many pictures went in.
 */
export function groupImages(view, at = view.state.selection.main.head) {
  const doc = view.state.doc;
  const sel = view.state.selection.main;
  let first = doc.lineAt(sel.from);
  let last = doc.lineAt(sel.to);
  const selected = !sel.empty && at >= sel.from && at <= sel.to;
  if (!selected || !isPictureLine(first.text) || !isPictureLine(last.text)) {
    first = last = doc.lineAt(at);
    if (isPictureLine(first.text)) {
      while (first.number > 1 && isPictureLine(doc.line(first.number - 1).text)) first = doc.line(first.number - 1);
      while (last.number < doc.lines && isPictureLine(doc.line(last.number + 1).text)) last = doc.line(last.number + 1);
    }
  }
  const lines = [];
  for (let n = first.number; n <= last.number; n++) lines.push(doc.line(n));
  if (!lines.every((l) => isPictureLine(l.text))) {
    // Nothing to group: an empty grid with the cursor inside it.
    const row = doc.lineAt(at);
    const lead = row.text.trim() ? "\n" : "";
    const pos = row.text.trim() ? row.to : row.from;
    const text = `${lead}[grid]\n\n[/grid]`;
    view.dispatch({
      changes: { from: pos, to: row.text.trim() ? pos : row.to, insert: text },
      selection: { anchor: pos + lead.length + "[grid]\n".length },
    });
    view.focus();
    return 0;
  }
  const count = lines.reduce((n, l) => n + [...l.text.matchAll(IMAGE)].length, 0);
  // The cursor ends up after the grid, so it shows as its tiles at once. At the end of the text that
  // takes a new line to stand on.
  const atEnd = last.to >= doc.length;
  view.dispatch({
    changes: [
      { from: first.from, insert: "[grid]\n" },
      { from: last.to, insert: atEnd ? "\n[/grid]\n" : "\n[/grid]" },
    ],
    selection: { anchor: last.to + "[grid]\n\n[/grid]".length + 1 },
  });
  view.focus();
  return count;
}

/** Takes the two lines of a grid away and leaves the pictures. */
export function ungroup(view, block) {
  const doc = view.state.doc;
  const gone = (line) => ({ from: line.from, to: Math.min(doc.length, line.to + 1) });
  view.dispatch({
    changes: [
      gone(block.open),
      doc.lines > block.close.number
        ? gone(block.close)
        : { from: Math.max(0, block.close.from - 1), to: block.close.to },
    ],
  });
  view.focus();
}

/** The tiles of a grid, shown while the cursor is outside it. */
export class GridWidget extends WidgetType {
  constructor(images, columns = 0) {
    super();
    this.images = images;
    this.columns = columns;
  }
  eq(other) {
    return (
      other.columns === this.columns &&
      other.images.length === this.images.length &&
      other.images.every(
        (x, i) => x.url === this.images[i].url && x.alt === this.images[i].alt && x.caption === this.images[i].caption,
      )
    );
  }
  toDOM(view) {
    const wrap = mk("div", { className: "cm-grid" });
    if (this.columns) wrap.style.gridTemplateColumns = `repeat(${this.columns}, minmax(0, 1fr))`;
    const where = () => {
      const pos = view.posAtDOM(wrap);
      return gridAt(view.state, pos);
    };
    this.images.forEach((im, i) => {
      const tile = mk("div", { className: "cm-grid-tile" });
      const [src] = im.url.split("#");
      const img = mk("img", { src, alt: im.alt, draggable: false });
      img.addEventListener("load", () => view.requestMeasure());
      img.addEventListener("error", () => {
        tile.classList.add("is-broken");
        tile.textContent = im.alt || src;
        view.requestMeasure();
      });
      tile.append(img);
      const find = () => {
        const block = where();
        return block ? (gridImages(view.state, block)[i] ?? null) : null;
      };
      finders.set(tile, find);
      tile.addEventListener("click", () => openImageEditor(view, tile, find, { size: false }));
      wrap.append(tile);
    });
    const bar = mk("div", { className: "cm-grid-bar" });
    const edit = mk("button", { type: "button", textContent: tr("Edit lines") });
    edit.addEventListener("click", (e) => {
      e.stopPropagation();
      const block = where();
      if (block) view.dispatch({ selection: { anchor: block.open.to + 1 } });
      view.focus();
    });
    const split = mk("button", { type: "button", textContent: tr("Ungroup") });
    split.addEventListener("click", (e) => {
      e.stopPropagation();
      const block = where();
      if (block) ungroup(view, block);
    });
    // How many tiles to a row.
    const cols = mk("select", { className: "cm-grid-columns" });
    cols.setAttribute("aria-label", tr("Columns"));
    for (const n of COLUMNS)
      cols.append(mk("option", { value: String(n), textContent: n ? String(n) : tr("Automatic") }));
    cols.value = String(this.columns);
    cols.addEventListener("click", (e) => e.stopPropagation());
    cols.addEventListener("change", () => {
      const block = where();
      if (block) setColumns(view, block, Number(cols.value));
    });
    bar.append(cols, edit, split);
    wrap.append(bar);
    return wrap;
  }
  // The tiles and the bar handle their own clicks.
  ignoreEvent() {
    return true;
  }
}
