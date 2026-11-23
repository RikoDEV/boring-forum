// Markdown composer: CodeMirror 6 over a plain <textarea>. The textarea stays in
// the form and is kept in sync, so posting works exactly as without JavaScript.
import { acceptCompletion, autocompletion } from "@codemirror/autocomplete";
import { Decoration, EditorView, ViewPlugin, WidgetType, keymap, placeholder } from "@codemirror/view";
import { ENDS, emoticonBefore, emoticonsOn } from "../emoticons.js";
import { StateField, EditorState, EditorSelection, Compartment, Prec } from "@codemirror/state";
import { aiHelper } from "./ai.js";
import { startChat } from "./chat.js";
import { dialog } from "../dialog.js";
import { siteSudo } from "../sudo.js";
import { composing, csrf, el, icon as svgIcon, jsonHeaders, mk, mkc, tr } from "../dom.js";
import { extractUrls } from "./urls.js";
import { IMAGE, finders, imageMarkdown, isPicture, openImageEditor, readImage, removeImage } from "./image.js";
import { openMenu } from "./menu.js";
import { COLUMNS, GridWidget, gridAt, gridBlocks, gridImages, groupImages, setColumns, ungroup } from "./grid.js";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
// No syntax highlighting: @codemirror/lang-markdown pulls in the HTML, CSS and JS
// grammars (170 KB gz, over the 120 KB budget), and the preview covers the need.

const wrap =
  (before, after = before) =>
  (view) => {
    const changes = view.state.changeByRange((r) => {
      const text = view.state.sliceDoc(r.from, r.to);
      return {
        changes: { from: r.from, to: r.to, insert: before + text + after },
        range: EditorSelection.range(r.from + before.length, r.from + before.length + text.length),
      };
    });
    view.dispatch(view.state.update(changes, { scrollIntoView: true, userEvent: "input" }));
    view.focus();
    return true;
  };

// Surrounds the selection with marks, or takes them off when it is already surrounded (so the
// same shortcut turns bold on and off). A single `*` next to another `*` is part of bold.
const format =
  (m, a = m) =>
  (view) => {
    const doc = view.state.doc;
    const at = (from, len) => doc.sliceString(Math.max(0, from), Math.max(0, from) + len);
    const edge = (from, mark) => mark.length > 1 || at(from, 1) !== mark;
    const changes = view.state.changeByRange((r) => {
      const text = doc.sliceString(r.from, r.to);
      if (
        r.from >= m.length &&
        at(r.from - m.length, m.length) === m &&
        at(r.to, a.length) === a &&
        edge(r.from - m.length - 1, m) &&
        edge(r.to + a.length, a)
      ) {
        return {
          changes: [
            { from: r.from - m.length, to: r.from },
            { from: r.to, to: r.to + a.length },
          ],
          range: EditorSelection.range(r.from - m.length, r.to - m.length),
        };
      }
      if (
        text.length >= m.length + a.length &&
        text.startsWith(m) &&
        text.endsWith(a) &&
        (m.length > 1 || (text[1] !== m && text[text.length - 2] !== a))
      ) {
        const inner = text.slice(m.length, text.length - a.length);
        return {
          changes: { from: r.from, to: r.to, insert: inner },
          range: EditorSelection.range(r.from, r.from + inner.length),
        };
      }
      return {
        changes: { from: r.from, to: r.to, insert: m + text + a },
        range: EditorSelection.range(r.from + m.length, r.from + m.length + text.length),
      };
    });
    view.dispatch(view.state.update(changes, { scrollIntoView: true, userEvent: "input" }));
    view.focus();
    return true;
  };

const LIST = /^(\s*)(?:[-*+]|\d+[.)])\s(?:\[[ xX]\]\s)?/;
// Gives every selected line a prefix, or takes it off when they all have it. `strip` matches
// whatever prefix a line has now (its first group is kept), so a bulleted line can become numbered.
const lineEdit = (test, strip, make) => (view) => {
  const doc = view.state.doc;
  const lines = [];
  for (const r of view.state.selection.ranges) {
    for (let pos = r.from; pos <= r.to;) {
      const line = doc.lineAt(pos);
      if (!lines.some((l) => l.from === line.from)) lines.push(line);
      pos = line.to + 1;
    }
  }
  const off = lines.every((l) => test.test(l.text));
  const changes = lines.map((l, i) => {
    const m = strip.exec(l.text);
    return { from: l.from, to: l.from + (m ? m[0].length : 0), insert: (m?.[1] ?? "") + (off ? "" : make(i)) };
  });
  view.dispatch({ changes, userEvent: "input" });
  view.focus();
  return true;
};
const bullets = lineEdit(/^\s*[-*+] (?!\[[ xX]\] )/, LIST, () => "- ");
const numbers = lineEdit(/^\s*\d+[.)] /, LIST, (i) => `${i + 1}. `);
const tasks = lineEdit(/^\s*[-*+] \[[ xX]\] /, LIST, () => "- [ ] ");
const quote = lineEdit(/^> ?/, /^()> ?/, () => "> ");
const heading = (level) => lineEdit(new RegExp(`^#{${level}} `), /^()#{1,6} /, () => `${"#".repeat(level)} `);
// The heading button: none, then ## ### ####, then none again.
const nextHeading = (view) => {
  const level = /^(#{1,6}) /.exec(view.state.doc.lineAt(view.state.selection.main.head).text)?.[1].length ?? 0;
  return heading(level < 2 ? 2 : level < 4 ? level + 1 : 4)(view);
};

// Its own lines, fenced: code, a hidden block, display math.
const fenced = (open, close) => (view) => {
  const doc = view.state.doc;
  const changes = view.state.changeByRange((r) => {
    const text = doc.sliceString(r.from, r.to);
    const lead = r.from > doc.lineAt(r.from).from ? "\n" : "";
    const tail = r.to < doc.lineAt(r.to).to ? "\n" : "";
    const start = r.from + lead.length + open.length + 1;
    return {
      changes: { from: r.from, to: r.to, insert: `${lead}${open}\n${text}\n${close}${tail}` },
      range: EditorSelection.range(start, start + text.length),
    };
  });
  view.dispatch(view.state.update(changes, { scrollIntoView: true, userEvent: "input" }));
  view.focus();
  return true;
};
const rule = (view) => {
  const line = view.state.doc.lineAt(view.state.selection.main.to);
  view.dispatch({
    changes: { from: line.to, insert: "\n\n---\n" },
    selection: { anchor: line.to + 6 },
    userEvent: "input",
  });
  view.focus();
  return true;
};
// `[^n]` where the cursor is, and its definition at the end, where the cursor goes.
const footnote = (view) => {
  const doc = view.state.doc;
  const n = (doc.toString().match(/^\[\^\d+\]:/gm) ?? []).length + 1;
  const at = view.state.selection.main.to;
  const mark = `[^${n}]`;
  const def = `\n\n${mark}: `;
  view.dispatch({
    changes: [
      { from: at, insert: mark },
      { from: doc.length, insert: def },
    ],
    selection: { anchor: doc.length + mark.length + def.length },
    userEvent: "input",
  });
  view.focus();
  return true;
};
// A web address selected becomes the link's target; other text becomes its label.
const link = (view) => {
  const r = view.state.selection.main;
  const text = view.state.sliceDoc(r.from, r.to);
  const url = /^https?:\/\/\S+$/.test(text.trim());
  const insert = url ? `[](${text.trim()})` : text ? `[${text}](https://)` : "[](https://)";
  const head = url || !text ? r.from + 1 : r.from + text.length + 3;
  view.dispatch({
    changes: { from: r.from, to: r.to, insert },
    selection: url || !text ? { anchor: head } : { anchor: head, head: head + 8 },
    userEvent: "input",
  });
  view.focus();
  return true;
};
// Enter at the end of a list item or quote line starts the next one; on an empty one it ends the list.
const NEXT = /^(\s*)(?:(?:([-*+])|(\d+)([.)]))\s(?:(\[[ xX]\])\s)?|(>\s?))/;
const continueList = (view) => {
  const { state } = view;
  const r = state.selection.main;
  const line = state.doc.lineAt(r.from);
  const m = NEXT.exec(line.text);
  if (!r.empty || r.from !== line.to || !m) return false;
  // Inside a code fence a dash is just text.
  if (((state.sliceDoc(0, line.from).match(/^\s*(```|~~~)/gm) ?? []).length & 1) === 1) return false;
  if (line.text.length === m[0].length) {
    view.dispatch({ changes: { from: line.from, to: line.to }, selection: { anchor: line.from }, userEvent: "delete" });
    return true;
  }
  const mark = m[6] ? "> " : `${m[2] ?? `${+m[3] + 1}${m[4]}`} ${m[5] ? "[ ] " : ""}`;
  view.dispatch({
    changes: { from: r.from, insert: `\n${m[1]}${mark}` },
    selection: { anchor: r.from + 1 + m[1].length + mark.length },
    userEvent: "input",
  });
  return true;
};

// Formatted text pasted from a web page or a document becomes Markdown. Only the common
// formatting survives; anything else is its text.
function htmlToMarkdown(html) {
  const doc = new DOMParser().parseFromString(previewPolicy.createHTML(html), "text/html");
  const kids = (n, ctx) => [...n.childNodes].map((c) => node(c, ctx)).join("");
  const around = (n, mark, ctx) => {
    const m = /^(\s*)([\s\S]*?)(\s*)$/.exec(kids(n, ctx));
    return m[2] ? `${m[1]}${mark}${m[2]}${mark}${m[3]}` : m[0];
  };
  const node = (n, ctx = {}) => {
    if (n.nodeType === 3) return ctx.pre ? n.textContent : n.textContent.replace(/\s+/g, " ");
    if (n.nodeType !== 1) return "";
    const t = n.localName;
    if (/^(script|style|meta|head|title|svg|img)$/.test(t)) return "";
    if (t === "b" || t === "strong") return around(n, "**", ctx);
    if (t === "i" || t === "em") return around(n, "*", ctx);
    if (t === "s" || t === "del" || t === "strike") return around(n, "~~", ctx);
    if (t === "code") return around(n, "`", ctx);
    if (t === "br") return "\n";
    if (t === "a") {
      const href = n.getAttribute("href") ?? "";
      const label = kids(n, ctx).trim();
      return /^(https?:|mailto:)/i.test(href) && label ? `[${label}](${href})` : label;
    }
    if (/^h[1-6]$/.test(t)) return `\n\n${"#".repeat(Math.max(2, +t[1]))} ${kids(n, ctx).trim()}\n\n`;
    if (t === "pre") return `\n\n\`\`\`\n${n.textContent.replace(/\n$/, "")}\n\`\`\`\n\n`;
    if (t === "blockquote") {
      const quoted = kids(n, ctx)
        .trim()
        .split("\n")
        .map((l) => `> ${l}`);
      return `\n\n${quoted.join("\n")}\n\n`;
    }
    if (t === "ul" || t === "ol") {
      const items = [...n.children].filter((c) => c.localName === "li");
      const body = items.map(
        (li, i) => `${t === "ol" ? `${i + 1}.` : "-"} ${kids(li, ctx).trim().replace(/\n+/g, " ")}`,
      );
      return `\n\n${body.join("\n")}\n\n`;
    }
    if (t === "p" || t === "div" || t === "tr") return `\n\n${kids(n, ctx)}\n\n`;
    return kids(n, ctx);
  };
  return kids(doc.body, {})
    .replace(/[ \t]+\n/g, "\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}
const FORMATTED = /<(b|strong|i|em|a|h[1-6]|ul|ol|li|blockquote|pre|code|s|del)[\s>]/i;

// The shortcut label: ⌘ on a Mac, Ctrl elsewhere.
const MOD = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl+";
const hint = (keys) =>
  keys ? ` (${keys.replace("Mod-", MOD).replace("Shift-", "Shift+").replace("Alt-", "Alt+")})` : "";

// The bar keeps what is used most: [icon, label, command, shortcut].
/** @type {[string, string, (view: any) => boolean, string?][]} */
const TOOLS = [
  ["bold", tr("Bold"), format("**"), "Mod-b"],
  ["italic", tr("Italic"), format("*"), "Mod-i"],
  ["link", tr("Link"), link, "Mod-k"],
  ["heading", tr("Heading"), nextHeading],
  ["quote", tr("Quote"), quote, "Mod-Shift-9"],
  ["list", tr("Bulleted list"), bullets, "Mod-Shift-8"],
];
// Puts the pictures around the cursor in a grid, or opens an empty one.
const imageGrid = (view) => {
  groupImages(view);
  return true;
};

// The Insert menu (and the "/" menu): [icon, label, command, shortcut, kind]. `narrow` items are
// on the bar on wide screens and only here on a phone; `math` is offered only where the site
// renders it; `table` and `palette` open their own pickers.
/** @type {[string, string, any, string?, string?][]} */
const INSERT = [
  ["heading", tr("Heading"), nextHeading, "", "narrow"],
  ["quote", tr("Quote"), quote, "", "narrow"],
  ["list", tr("Bulleted list"), bullets, "", "narrow"],
  ["list-ordered", tr("Numbered list"), numbers, "Mod-Shift-7"],
  ["list-checks", tr("Task list"), tasks],
  ["table", tr("Table"), "table"],
  ["image", tr("Picture grid"), imageGrid],
  ["code-xml", tr("Code block"), fenced("```", "```")],
  ["underline", tr("Underline"), format("++"), "Mod-u"],
  ["strikethrough", tr("Strikethrough"), format("~~"), "Mod-Shift-x"],
  ["palette", tr("Colour"), "palette"],
  ["minus", tr("Horizontal rule"), rule],
  ["asterisk", tr("Footnote"), footnote],
  ["eye-off", tr("Collapsed spoiler"), fenced("```spoiler", "```")],
  ["lock", tr("Hidden until someone replies"), fenced("```hide", "```")],
  ["sigma", tr("Math"), format("$"), "", "math"],
  ["sigma", tr("Math block"), fenced("$$", "$$"), "", "math"],
];

// A button with an icon from the page's sprite.
function iconButton(name, title) {
  const b = mk("button", { type: "button", className: "bf-icon-btn" }, svgIcon(name));
  b.setAttribute("aria-label", title);
  return b;
}

const EMOJI = Object.fromEntries(
  (
    "smile:😄 grinning:😀 joy:😂 rofl:🤣 sweat_smile:😅 wink:😉 blush:😊 innocent:😇 heart_eyes:😍 kissing_heart:😘 yum:😋 " +
    "stuck_out_tongue:😛 sunglasses:😎 thinking:🤔 neutral_face:😐 expressionless:😑 unamused:😒 roll_eyes:🙄 grimacing:😬 " +
    "relieved:😌 pensive:😔 sleepy:😪 sleeping:😴 mask:😷 nerd_face:🤓 confused:😕 worried:😟 open_mouth:😮 astonished:😲 " +
    "flushed:😳 cry:😢 sob:😭 scream:😱 angry:😠 rage:😡 skull:💀 poop:💩 clown_face:🤡 ghost:👻 alien:👽 robot:🤖 " +
    "thumbsup:👍 thumbsdown:👎 ok_hand:👌 clap:👏 wave:👋 pray:🙏 muscle:💪 raised_hands:🙌 handshake:🤝 point_up:☝️ v:✌️ " +
    "eyes:👀 brain:🧠 heart:❤️ broken_heart:💔 orange_heart:🧡 yellow_heart:💛 green_heart:💚 blue_heart:💙 purple_heart:💜 " +
    "sparkles:✨ star:⭐ fire:🔥 boom:💥 zap:⚡ tada:🎉 trophy:🏆 rocket:🚀 bulb:💡 warning:⚠️ x:❌ white_check_mark:✅ check:✅ " +
    "question:❓ exclamation:❗ 100:💯 lock:🔒 key:🔑 bug:🐛 wrench:🔧 hammer:🔨 gear:⚙️ link:🔗 book:📖 memo:📝 pushpin:📌 " +
    "calendar:📅 email:📧 phone:📱 computer:💻 coffee:☕ beer:🍺 pizza:🍕 cat:🐱 dog:🐶 sun:☀️ moon:🌙 rainbow:🌈 snowflake:❄️"
  )
    .split(" ")
    .map((pair) => pair.split(":")),
);

let customEmoji = null;
const loadEmoji = () =>
  (customEmoji ??= fetch("/api/v1/emoji")
    .then((r) => (r.ok ? r.json() : []))
    .catch(() => []));
// The member's skin tone, for emoji that take one.
const TONES = ["", "\u{1F3FB}", "\u{1F3FC}", "\u{1F3FD}", "\u{1F3FE}", "\u{1F3FF}"];
const HANDS = new Set(["👍", "👎", "👌", "👏", "👋", "🙏", "💪", "🙌", "🤝"]);
const withTone = (e) => (HANDS.has(e) ? e + (TONES[+document.documentElement.dataset.skin || 0] ?? "") : e);

// Suggestions for `@member`, `#forum` (inserted as a link) and `:emoji:`. In a reply,
// `@` alone lists the topic's participants, and they come first as the name is typed.
const completeFor = (topic) => async (ctx) => {
  const m = ctx.matchBefore(/(?:^|(?<=[\s(]))[@#:][\p{L}\p{N}_.-]*/u);
  if (!m || (m.from === m.to && !ctx.explicit)) return null;
  const sigil = m.text[0];
  const q = m.text.slice(1);
  if (!q && !(sigil === "@" && topic)) return null;
  if (sigil === ":") {
    const custom = (await loadEmoji())
      .filter((c) => c.name.startsWith(q))
      .map((c) => ({ label: `:${c.name}:`, detail: "custom" }));
    const options = Object.entries(EMOJI)
      .filter(([name]) => name.startsWith(q))
      .map(([name, e]) => ({ label: `:${name}:`, detail: withTone(e), apply: withTone(e) }));
    return { from: m.from, options: [...custom, ...options], filter: false };
  }
  const kind = sigil === "@" ? "user" : "node";
  const at = kind === "user" && topic ? `&topic=${topic}` : "";
  const res = await fetch(`/api/v1/autocomplete?kind=${kind}&q=${encodeURIComponent(q)}${at}`).catch(() => null);
  if (!res?.ok) return null;
  const rows = await res.json();
  if (!rows.length) return null;
  // A space follows the inserted name, so typing goes on without one.
  const options = rows.map((r) =>
    kind === "user"
      ? {
          label: `@${r.label}`,
          apply: `@${r.label} `,
          detail: r.group
            ? tr("Group")
            : r.name && r.name !== r.label
              ? r.name
              : r.in_topic
                ? tr("In this topic")
                : undefined,
          type: r.group ? "group" : "member",
          who: r.label,
        }
      : {
          label: `#${r.label}`,
          detail: r.tag ? tr("Tag") : tr("Forum"),
          type: r.tag ? "tag" : "forum",
          apply: `[${r.label}](${r.url}) `,
        },
  );
  return { from: m.from, options, filter: false };
};

// Each suggestion's picture: the member's avatar, or a mark for a group, forum or tag.
const completionPicture = {
  position: 20,
  render(c) {
    if (c.type === "member")
      return mk("img", {
        className: "cm-mention-avatar",
        src: `/avatar/${encodeURIComponent(c.who)}.svg`,
        alt: "",
        width: 24,
        height: 24,
      });
    if (!["group", "forum", "tag"].includes(c.type)) return null;
    return mk("span", { className: "cm-mention-mark", textContent: { group: "@", forum: "#", tag: "#" }[c.type] });
  },
};

// Live Markdown: formatting shows as you type, and the markers are hidden except on the line
// being edited. The text itself stays plain Markdown. Regular expressions, not a parser, for
// the bundle budget (see the note on lang-markdown above).
const hide = Decoration.replace({});
/** @type {[RegExp, string, number][]} */
const INLINE = [
  [/`([^`\n]+)`/g, "cm-md-code", 1],
  [/\*\*([^*\n]+)\*\*/g, "cm-md-strong", 2],
  [/__([^_\n]+)__/g, "cm-md-strong", 2],
  [/(?<![*\w])\*([^*\s][^*\n]*)\*(?!\*)/g, "cm-md-em", 1],
  [/(?<![_\w])_([^_\s][^_\n]*)_(?![_\w])/g, "cm-md-em", 1],
  [/~~([^~\n]+)~~/g, "cm-md-strike", 2],
  [/\+\+([^+\n]+)\+\+/g, "cm-md-underline", 2],
];

// Text colours that `[text]{colour}` can use, with a swatch for the toolbar menu. Kept in step
// with the cooker's allow-list and the theme's `--bf-clr-*` tokens.
const SWATCH = {
  red: "#e5484d",
  orange: "#f76808",
  yellow: "#ffb224",
  green: "#30a46c",
  blue: "#0b64c0",
  purple: "#8e4ec6",
  gray: "#889096",
};
const COLOR_RE = /\[([^\]\n]+)\]\{(red|orange|yellow|green|blue|purple|gray)\}/g;

// List markers render as real bullets or checkboxes while writing; the raw marker shows only on
// the line being edited. GFM task boxes toggle on click.
class BulletWidget extends WidgetType {
  constructor(level) {
    super();
    this.level = level;
  }
  eq(o) {
    return o.level === this.level;
  }
  toDOM() {
    return mk("span", { className: "cm-li-bullet", textContent: `${["•", "◦", "▪"][this.level % 3]}\u00a0` });
  }
}

class TaskWidget extends WidgetType {
  constructor(checked) {
    super();
    this.checked = checked;
  }
  eq(o) {
    return o.checked === this.checked;
  }
  toDOM(view) {
    const box = mk("input", { type: "checkbox", checked: this.checked, className: "cm-li-check" });
    box.addEventListener("mousedown", (e) => {
      e.preventDefault();
      const line = view.state.doc.lineAt(view.posAtDOM(box));
      const m = /\[([ xX])\]/.exec(line.text);
      if (!m) return;
      const at = line.from + m.index + 1;
      view.dispatch({ changes: { from: at, to: at + 1, insert: this.checked ? " " : "x" } });
    });
    return box;
  }
  ignoreEvent() {
    return true;
  }
}

function markdownDecorations(view) {
  const out = [];
  const { doc, selection } = view.state;
  const active = new Set();
  if (view.hasFocus)
    for (const r of selection.ranges)
      for (let n = doc.lineAt(r.from).number; n <= doc.lineAt(r.to).number; n++) active.add(n);
  let fenced = false;
  let raw = false;
  // Tints the text between `open` and `close` marker characters and hides the markers.
  const tint = (cls, from, to, open, close) => {
    out.push(Decoration.mark({ class: cls }).range(from + open, to - close));
    if (!raw) out.push(hide.range(from, from + open), hide.range(to - close, to));
  };
  const firstVisible = view.visibleRanges[0]?.from ?? 0;
  const lastVisible = view.visibleRanges.at(-1)?.to ?? doc.length;
  for (let n = 1; n <= doc.lines; n++) {
    const line = doc.line(n);
    if (line.from > lastVisible) break;
    const fence = /^\s*```/.test(line.text);
    const shown = line.to >= firstVisible;
    if (fence || fenced) {
      if (shown) out.push(Decoration.line({ class: "cm-md-fence" }).range(line.from));
      if (fence) fenced = !fenced;
      continue;
    }
    if (!shown) continue;
    raw = active.has(n);
    const block = /^(#{1,6} |> ?|\s*[-*+] |\s*\d+[.)] )/.exec(line.text);
    let body = line.from;
    if (block) {
      const mark = block[1];
      body = line.from + mark.length;
      if (mark[0] === "#" || mark[0] === ">") {
        const cls = mark[0] === "#" ? `cm-md-h${Math.min(mark.length - 1, 3)}` : "cm-md-quote";
        out.push(Decoration.line({ class: cls }).range(line.from));
        if (!raw) out.push(hide.range(line.from, body));
      } else {
        const li = /^(\s*)(?:([-*+])|(\d+)[.)])\s/.exec(line.text);
        const indent = line.from + (li ? li[1].length : 0);
        if (!li) {
          out.push(Decoration.mark({ class: "cm-md-bullet" }).range(line.from, body));
        } else if (li[2]) {
          // Bullet item, or a GFM task box when it starts with `[ ]` / `[x]`.
          const task = /^\[([ xX])\]\s/.exec(line.text.slice(li[1].length + 2));
          const end = indent + 2 + (task ? task[0].length : 0);
          if (raw) {
            out.push(Decoration.mark({ class: "cm-md-bullet" }).range(indent, end));
          } else if (task) {
            out.push(Decoration.replace({ widget: new TaskWidget(task[1].toLowerCase() === "x") }).range(indent, end));
          } else {
            out.push(
              Decoration.replace({ widget: new BulletWidget(Math.floor(li[1].length / 2)) }).range(indent, indent + 2),
            );
          }
          body = end;
        } else {
          // Ordered item: keep the number, just tint it.
          out.push(Decoration.mark({ class: "cm-md-bullet" }).range(indent, body));
        }
      }
    }
    const text = line.text;
    for (const m of text.matchAll(/(!?)\[([^\]\n]+)\]\(([^)\n]+)\)/g)) {
      const from = line.from + m.index;
      tint("cm-md-link", from, from + m[0].length, m[1].length + 1, m[0].length - m[1].length - m[2].length - 1);
    }
    for (const [re, cls, size] of INLINE) {
      for (const m of text.matchAll(re)) {
        const from = line.from + m.index;
        const to = from + m[0].length;
        if (from < body || to - from <= size * 2) continue;
        tint(cls, from, to, size, size);
      }
    }
    for (const m of text.matchAll(COLOR_RE)) {
      const from = line.from + m.index;
      if (from < body) continue;
      tint(`cm-clr-${m[2]}`, from, from + m[0].length, 1, m[0].length - m[1].length - 1);
    }
  }
  // Overlapping replacements (say `**` inside a code span) would make the set invalid.
  out.sort((a, b) => a.from - b.from || a.value.startSide - b.value.startSide);
  const kept = [];
  let hiddenTo = -1;
  for (const r of out) {
    if (r.value === hide) {
      if (r.from < hiddenTo) continue;
      hiddenTo = r.to;
    }
    kept.push(r);
  }
  return Decoration.set(kept, true);
}

const liveMarkdown = ViewPlugin.fromClass(
  class {
    constructor(view) {
      this.decorations = markdownDecorations(view);
    }
    update(u) {
      if (u.docChanged || u.selectionSet || u.viewportChanged || u.focusChanged)
        this.decorations = markdownDecorations(u.view);
    }
  },
  { decorations: (v) => v.decorations },
);

// Images show as pictures while writing. A picture alone on its line replaces its Markdown until
// the cursor moves onto that line. Drag to move; the corner handle resizes, which rewrites the
// size in the link (`#640x480`), so the text stays plain Markdown.
let movingImage = null;

class ImageWidget extends WidgetType {
  constructor(url, alt, caption = "") {
    super();
    this.url = url;
    this.alt = alt;
    this.caption = caption;
  }
  eq(other) {
    return other.url === this.url && other.alt === this.alt && other.caption === this.caption;
  }
  toDOM(view) {
    const wrap = mk("div", { className: "cm-image", draggable: true, title: tr("Drag to move") });
    const [src, size] = this.url.split("#");
    const img = mk("img", { src, alt: this.alt, draggable: false });
    const given = /^(\d+)x(\d+)$/.exec(size ?? "");
    if (given) img.style.width = `${given[1]}px`;
    img.addEventListener("load", () => view.requestMeasure());
    // A picture that will not load shows its name instead of a broken icon.
    img.addEventListener("error", () => {
      wrap.replaceChildren(document.createTextNode(this.alt || src));
      wrap.classList.add("is-broken");
      view.requestMeasure();
    });
    const lineOf = () => view.state.doc.lineAt(view.posAtDOM(wrap));
    // Where this picture is written, read again each time: the text may have changed since.
    const find = () => {
      const line = lineOf();
      const m = [...line.text.matchAll(IMAGE)].find((x) => x[2] === this.url && x[1] === this.alt);
      return m ? { from: line.from + m.index, to: line.from + m.index + m[0].length, ...readImage(m) } : null;
    };
    finders.set(wrap, find);
    wrap.addEventListener("dragstart", (e) => {
      const found = find();
      if (!found) return e.preventDefault();
      const line = lineOf();
      const text = view.state.sliceDoc(found.from, found.to);
      const whole = text.length === line.text.trim().length;
      movingImage = {
        text,
        from: whole ? line.from : found.from,
        to: whole ? line.to : found.to,
        whole,
      };
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", text);
    });
    wrap.addEventListener("dragend", () => (movingImage = null));
    // A click opens the small panel for the picture's description, caption and size.
    wrap.addEventListener("click", () => openImageEditor(view, wrap, find));
    const handle = mk("span", { className: "cm-image-handle", title: tr("Drag to resize") });
    handle.addEventListener("click", (e) => e.stopPropagation());
    handle.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      e.stopPropagation();
      handle.setPointerCapture(e.pointerId);
      wrap.draggable = false;
      const startX = e.clientX;
      const startW = img.getBoundingClientRect().width;
      const most = Math.min(img.naturalWidth || 4000, view.contentDOM.clientWidth);
      const move = (ev) =>
        (img.style.width = `${Math.round(Math.max(48, Math.min(most, startW + ev.clientX - startX)))}px`);
      handle.addEventListener("pointermove", move);
      handle.addEventListener(
        "pointerup",
        () => {
          handle.removeEventListener("pointermove", move);
          wrap.draggable = true;
          const w = Math.round(img.getBoundingClientRect().width);
          const h = Math.round(w * (img.naturalHeight / img.naturalWidth));
          if (!w || !h) return;
          const at = find();
          if (!at) return;
          view.dispatch({
            changes: {
              from: at.from,
              to: at.to,
              insert: imageMarkdown({ alt: at.alt, url: `${src}#${w}x${h}`, caption: at.caption }),
            },
          });
        },
        { once: true },
      );
    });
    wrap.append(img, handle);
    return wrap;
  }
  // The picture handles its own clicks and drags, but can be dropped on.
  ignoreEvent(e) {
    return e.type !== "drop" && e.type !== "dragover";
  }
}

function imageDecorations(state) {
  const out = [];
  const sel = state.selection.main;
  // A grid shows as its tiles while the cursor is outside it, and its lines are not drawn one by one.
  // With the cursor inside, the lines are text again, each with its own picture.
  const tiled = new Set();
  for (const block of gridBlocks(state)) {
    if (sel.from <= block.close.to && sel.to >= block.open.from) continue;
    const images = gridImages(state, block);
    if (!images.length) continue;
    out.push(
      Decoration.replace({ widget: new GridWidget(images, block.columns), block: true }).range(
        block.open.from,
        block.close.to,
      ),
    );
    for (let n = block.open.number; n <= block.close.number; n++) tiled.add(n);
  }
  for (let n = 1; n <= state.doc.lines; n++) {
    if (tiled.has(n)) continue;
    const line = state.doc.line(n);
    if (!line.text.includes("![")) continue;
    const found = [...line.text.matchAll(IMAGE)].filter((m) => /^(https?:\/\/|\/)/.test(m[2]) && isPicture(m[2]));
    const editing = sel.from <= line.to && sel.to >= line.from;
    if (found.length === 1 && found[0][0].length === line.text.trim().length && !editing) {
      out.push(
        Decoration.replace({
          widget: new ImageWidget(found[0][2], found[0][1], readImage(found[0]).caption),
          block: true,
        }).range(line.from, line.to),
      );
      continue;
    }
    for (const m of found)
      out.push(
        Decoration.widget({ widget: new ImageWidget(m[2], m[1], readImage(m).caption), block: true, side: 1 }).range(
          line.to,
        ),
      );
  }
  return Decoration.set(out, true);
}

const liveImages = StateField.define({
  create: imageDecorations,
  update: (value, tr) => (tr.docChanged || tr.selection ? imageDecorations(tr.state) : value),
  provide: (f) => EditorView.decorations.from(f),
});

// Tables render as real tables while writing and fall back to their Markdown when the cursor is
// inside them, the same idea as images. The text stays plain GFM.
const splitRow = (t) =>
  t
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|")
    .map((c) => c.trim());

// A separator line (`|---|:--:|`) returns each column's alignment; anything else returns null, so
// a run of `|`-lines is only treated as a table when a separator sits under the header.
function sepAligns(t) {
  const s = t.trim();
  if (!s.includes("|") || !s.includes("-") || !/^[\s|:-]+$/.test(s)) return null;
  const cells = splitRow(s);
  if (!cells.length || !cells.every((c) => /^:?-{1,}:?$/.test(c))) return null;
  return cells.map((c) =>
    c.startsWith(":") && c.endsWith(":") ? "center" : c.endsWith(":") ? "right" : c.startsWith(":") ? "left" : "",
  );
}

class TableWidget extends WidgetType {
  constructor(src, aligns) {
    super();
    this.src = src;
    this.aligns = aligns;
  }
  eq(other) {
    return other.src === this.src;
  }
  toDOM(view) {
    // The table renders read-only inside CodeMirror; clicking a cell opens a small textarea that
    // lives in document.body (outside the editor's contentEditable), so CodeMirror never fights for
    // focus or selection. Edits are written back to the Markdown when editing ends.
    const src = this.src;
    const aligns = [...this.aligns];
    const wrap = mkc("div", "cm-table-wrap");
    const thead = mk("thead");
    const tbody = mk("tbody");
    const table = mk("table", { className: "cm-table" }, thead, tbody);

    let ed = null; // the floating textarea
    let edCell = null; // the cell it edits
    const reposition = () => {
      if (!ed || !edCell) return;
      const r = edCell.getBoundingClientRect();
      Object.assign(ed.style, {
        top: `${r.top}px`,
        left: `${r.left}px`,
        width: `${r.width}px`,
        height: `${r.height}px`,
      });
    };
    const commitToCell = () => {
      if (ed && edCell) edCell.textContent = ed.value.replace(/\s+/g, " ").trim();
    };
    const removeEd = () => {
      if (!ed) return;
      ed.remove();
      window.removeEventListener("scroll", reposition, true);
      window.removeEventListener("resize", reposition);
      ed = null;
      edCell = null;
    };
    const commitDoc = () => {
      const md = toMarkdown();
      if (md === src) return;
      const from = view.posAtDOM(wrap);
      view.dispatch({ changes: { from, to: from + src.length, insert: md } });
    };
    const finish = () => {
      commitToCell();
      removeEd();
      commitDoc();
    };

    const allCells = () => [...table.querySelectorAll("th, td")];
    const addRow = () => {
      const cols = thead.rows[0].cells.length;
      const tr = mk("tr", {}, ...Array.from({ length: cols }, (_, i) => makeCell("td", "", i)));
      tbody.append(tr);
      return tr;
    };

    const openCellEditor = (cell) => {
      commitToCell(); // save the cell we're leaving (DOM only; no document write yet)
      edCell = cell;
      if (!ed) {
        ed = mk("textarea", { className: "cm-cell-editor", rows: 1 });
        ed.addEventListener("keydown", (e) => {
          if (composing(e)) return; // Enter, Tab and Escape belong to the IME while it composes
          if (e.key === "Escape") {
            e.preventDefault();
            finish();
            view.focus();
          } else if (e.key === "Tab") {
            e.preventDefault();
            const cells = allCells();
            const next = cells[cells.indexOf(edCell) + (e.shiftKey ? -1 : 1)];
            if (next) openCellEditor(next);
            else if (!e.shiftKey) openCellEditor(addRow().cells[0]);
          } else if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            const col = [...edCell.parentElement.cells].indexOf(edCell);
            const tr = edCell.parentElement;
            let nextTr = tr.parentElement === thead ? tbody.rows[0] : tbody.rows[[...tbody.rows].indexOf(tr) + 1];
            if (!nextTr) nextTr = addRow();
            openCellEditor(nextTr.cells[col] ?? nextTr.cells[0]);
          }
        });
        // A real blur (focus left the textarea for good) ends editing; navigation refocuses it, so
        // the deferred check sees it still focused and does nothing.
        ed.addEventListener("blur", () => setTimeout(() => ed && document.activeElement !== ed && finish(), 0));
        document.body.append(ed);
        window.addEventListener("scroll", reposition, true);
        window.addEventListener("resize", reposition);
      }
      ed.value = cell.textContent;
      reposition();
      ed.focus();
      ed.select();
    };

    const makeCell = (tag, text, i) => {
      const cell = mk(tag, { textContent: text });
      if (aligns[i]) cell.style.textAlign = aligns[i];
      // Stop CodeMirror from moving its cursor here, then open the floating editor.
      cell.addEventListener("mousedown", (e) => {
        e.preventDefault();
        e.stopPropagation();
        openCellEditor(cell);
      });
      return cell;
    };

    const rows = src.split("\n").filter((r) => r.trim() !== "");
    const header = splitRow(rows[0] ?? "");
    thead.append(mk("tr", {}, ...header.map((c, i) => makeCell("th", c, i))));
    for (const row of rows.slice(2)) {
      const cells = splitRow(row);
      tbody.append(mk("tr", {}, ...header.map((_, i) => makeCell("td", cells[i] ?? "", i))));
    }

    // Rebuild the GFM source from the live cells.
    const toMarkdown = () => {
      const cellText = (c) => c.textContent.replace(/\s+/g, " ").trim().replace(/\|/g, "\\|") || " ";
      const sepFor = (a) => (a === "center" ? ":---:" : a === "right" ? "---:" : a === "left" ? ":---" : "---");
      const cols = thead.rows[0].cells.length;
      const line = (cells) => `| ${[...cells].map(cellText).join(" | ")} |`;
      const out = [line(thead.rows[0].cells), `| ${aligns.slice(0, cols).map(sepFor).join(" | ")} |`];
      for (const tr of tbody.rows) out.push(line(tr.cells));
      return out.join("\n");
    };

    // Structural controls, shown on hover or focus. They act on the cell being edited.
    const tools = mkc("div", "cm-table-tools");
    const toolBtn = (label, title, fn) => {
      const b = mk("button", { type: "button", className: "cm-table-tool", textContent: label, title });
      b.addEventListener("mousedown", (e) => {
        e.preventDefault();
        e.stopPropagation();
      });
      b.addEventListener("click", () => {
        const target = edCell ?? thead.rows[0].cells[0];
        commitToCell();
        removeEd();
        fn(target);
        commitDoc();
      });
      return b;
    };
    tools.append(
      toolBtn("+ Col", tr("Add column"), () => {
        aligns.push("");
        const i = thead.rows[0].cells.length;
        thead.rows[0].append(makeCell("th", tr("Column {n}", { n: i + 1 }), i));
        for (const tr of tbody.rows) tr.append(makeCell("td", "", i));
      }),
      toolBtn("+ Row", tr("Add row"), () => addRow()),
      toolBtn("− Col", tr("Delete column"), (cell) => {
        if (thead.rows[0].cells.length <= 1) return;
        const i = [...cell.parentElement.cells].indexOf(cell);
        aligns.splice(i, 1);
        thead.rows[0].deleteCell(i);
        for (const tr of tbody.rows) tr.deleteCell(i);
      }),
      toolBtn("− Row", tr("Delete row"), (cell) => {
        const tr = cell.closest("tr");
        if (tr && tr.parentElement === tbody) tr.remove();
      }),
    );

    wrap.append(tools, table);
    /** @type {any} */ (wrap)._cleanup = removeEd; // CodeMirror calls destroy() when it drops the widget
    return wrap;
  }
  destroy(dom) {
    dom._cleanup?.();
  }
  ignoreEvent() {
    return true;
  }
}

function tableDecorations(state) {
  const out = [];
  const { doc } = state;
  const sel = state.selection.main;
  let n = 1;
  while (n <= doc.lines) {
    const header = doc.line(n);
    const aligns = n < doc.lines ? sepAligns(doc.line(n + 1).text) : null;
    if (aligns && header.text.includes("|") && header.text.trim() !== "") {
      let last = doc.line(n + 1);
      let m = n + 2;
      while (m <= doc.lines && doc.line(m).text.includes("|") && doc.line(m).text.trim() !== "") {
        last = doc.line(m);
        m++;
      }
      const from = header.from;
      const to = last.to;
      if (sel.from > to || sel.to < from) {
        out.push(
          Decoration.replace({ widget: new TableWidget(doc.sliceString(from, to), aligns), block: true }).range(
            from,
            to,
          ),
        );
      }
      n = m;
      continue;
    }
    n++;
  }
  return Decoration.set(out, true);
}

const liveTables = StateField.define({
  create: tableDecorations,
  update: (value, tr) => (tr.docChanged || tr.selection ? tableDecorations(tr.state) : value),
  provide: (f) => EditorView.decorations.from(f),
});

// The visual rendering (live Markdown, images, tables) lives in a compartment so the "source"
// toolbar button can switch it off and show plain Markdown.
const VISUAL = [liveMarkdown, liveImages, liveTables];
const visualCompartment = new Compartment();

// Inserts a GFM table skeleton of the chosen size and selects the first header cell.
function insertTable(view, rows, cols) {
  const header = `| ${Array.from({ length: cols }, (_, i) => tr("Column {n}", { n: i + 1 })).join(" | ")} |`;
  const sep = `| ${Array.from({ length: cols }, () => "---").join(" | ")} |`;
  const bodyRow = `|${" |".repeat(cols)}`;
  const body = Array.from({ length: Math.max(1, rows - 1) }, () => bodyRow).join("\n");
  const pos = view.state.selection.main.head;
  const line = view.state.doc.lineAt(pos);
  const prefix = line.text.trim() === "" ? "" : "\n";
  const at = line.to;
  const table = `${header}\n${sep}\n${body}\n`;
  view.dispatch({ changes: { from: at, insert: `${prefix}\n${table}` } });
  const cellStart = at + prefix.length + 1 + 2; // past the leading newline and "| "
  view.dispatch({ selection: EditorSelection.range(cellStart, cellStart + tr("Column {n}", { n: 1 }).length) });
  view.focus();
}

// A small panel under `anchor`, one at a time: a second click on the button, a click elsewhere
// or Escape closes it. `fill(panel, close)` puts its contents in.
function popup(cls, anchor, fill) {
  const open = document.querySelector(`.${cls}`);
  if (open) return open.remove();
  const pop = mkc("div", cls);
  const close = () => {
    pop.remove();
    document.removeEventListener("mousedown", onAway);
    document.removeEventListener("keydown", onKey);
  };
  const onAway = (e) => !pop.contains(e.target) && e.target !== anchor && close();
  const onKey = (e) => e.key === "Escape" && close();
  fill(pop, close);
  document.body.append(pop);
  const rect = anchor.getBoundingClientRect();
  pop.style.insetBlockStart = `${rect.bottom + 6}px`;
  pop.style.insetInlineStart = `${Math.max(8, Math.min(rect.left, window.innerWidth - pop.offsetWidth - 8))}px`;
  setTimeout(() => document.addEventListener("mousedown", onAway), 0);
  document.addEventListener("keydown", onKey);
}

// A hover grid for picking table size, like a word processor's table button.
const openTableDialog = (view, anchor) =>
  popup("composer-table-picker", anchor, (pop, close) => {
    const MAX = 8;
    const grid = mkc("div", "composer-table-grid");
    grid.style.setProperty("--cols", String(MAX));
    const label = mk("div", { className: "composer-table-size", textContent: tr("Pick a size") });
    const cells = [];
    for (let r = 1; r <= MAX; r++) {
      for (let c = 1; c <= MAX; c++) {
        const cell = mkc("span", "composer-table-cell");
        cell.dataset.r = String(r);
        cell.dataset.c = String(c);
        cell.addEventListener("mouseenter", () => {
          label.textContent = `${c} × ${r}`;
          for (const x of cells) x.classList.toggle("is-on", +x.dataset.r <= r && +x.dataset.c <= c);
        });
        cell.addEventListener("click", () => {
          close();
          insertTable(view, r, c);
        });
        cells.push(cell);
        grid.append(cell);
      }
    }
    pop.append(grid, label);
  });

// A swatch menu; picking a colour wraps the selection in `[text]{colour}`.
const openColorMenu = (view, anchor) =>
  popup("composer-color-menu", anchor, (pop, close) => {
    for (const [name, hex] of Object.entries(SWATCH)) {
      const b = mk("button", { type: "button", className: "composer-swatch", title: name });
      b.style.background = hex;
      b.addEventListener("click", () => {
        close();
        wrap("[", `]{${name}}`)(view);
      });
      pop.append(b);
    }
  });

// Moves a dragged picture to its own line, above or below the block under the pointer.
function dropImage(view, e) {
  const m = movingImage;
  movingImage = null;
  const doc = view.state.doc;
  const block = view.lineBlockAtHeight(e.clientY - view.documentTop);
  const below = e.clientY - view.documentTop > block.top + block.height / 2;
  const endOfSource = m.whole && m.to < doc.length ? m.to + 1 : m.to;
  const at = below ? block.to : block.from;
  if (at >= m.from && at <= endOfSource) return true;
  const insert = below ? `\n${m.text}` : `${m.text}\n`;
  // The last line has no newline after it, so it gives up the one before.
  const from = m.whole && m.to === doc.length && m.from > 0 ? m.from - 1 : m.from;
  view.dispatch({
    changes: [
      { from, to: endOfSource, insert: "" },
      { from: at, insert },
    ],
  });
  return true;
}

const OPEN_KEY = "bf-composer-open";

// Docked panel: docked, minimized or fullscreen, remembered per browser.
function dockControls(form, view, draftContext, sheet) {
  const KEY = "bf-composer-dock";
  const remember = (mode) => {
    try {
      if (mode && /^(reply|new-topic):/.test(draftContext ?? "")) {
        const old = JSON.parse(localStorage.getItem(OPEN_KEY) || "null");
        const onTopicPage = document.querySelector("h1")?.closest("main");
        localStorage.setItem(
          OPEN_KEY,
          JSON.stringify({
            action: new URL(form.action).pathname,
            draft: draftContext,
            replyTo: form.querySelector('input[name="reply_to"]')?.value ?? "",
            title:
              old?.draft === draftContext && old.title
                ? old.title
                : onTopicPage
                  ? document.querySelector("h1").textContent.trim()
                  : "topic",
          }),
        );
      } else {
        localStorage.removeItem(OPEN_KEY);
      }
    } catch {
      // Storage blocked: the panel just won't follow to the next page.
    }
  };
  let current = "";
  const handle = mk("div", { className: "composer-handle", hidden: true });
  const buttons = {};
  // Minimized, the whole composer gives way to a button in the corner that brings it back (a
  // draft in it stays).
  let before = sheet ? "docked" : "";
  const fab = iconButton("reply", "");
  fab.className = "btn primary bf-composer-fab";
  fab.hidden = true;
  const fabText = mk("span");
  fab.append(fabText);
  fab.addEventListener("click", () => apply(before));
  document.body.append(fab);
  const apply = (wanted) => {
    // A sheet is always a panel: leaving minimized or fullscreen returns it to docked.
    const mode = sheet && !wanted ? "docked" : wanted;
    if (mode === "minimized" && current !== "minimized") before = current || (sheet ? "docked" : "");
    fab.hidden = mode !== "minimized";
    if (mode === "minimized") {
      fabText.textContent = sheet ? tr("New topic") : tr("Reply");
      fab.setAttribute("aria-label", fabText.textContent);
    }
    current = mode;
    form.classList.toggle("docked", mode !== "");
    form.classList.toggle("minimized", mode === "minimized");
    form.classList.toggle("fullscreen", mode === "fullscreen");
    handle.hidden = mode !== "docked";
    for (const [k, b] of Object.entries(buttons)) b.setAttribute("aria-pressed", String(mode === k));
    try {
      if (!sheet) localStorage.setItem(KEY, mode);
    } catch {
      // Storage blocked: the choice still applies to this page view.
    }
    remember(mode);
    view.requestMeasure();
  };
  const group = document.createDocumentFragment();
  const modes = [
    ["docked", "panel-bottom", tr("Dock")],
    ["minimized", "minus", tr("Minimize")],
    ["fullscreen", "maximize-2", tr("Fullscreen")],
  ];
  for (const [mode, icon, label] of modes) {
    if (sheet && mode === "docked") continue;
    const b = iconButton(icon, label);
    b.addEventListener("click", () => apply(current === mode ? "" : mode));
    buttons[mode] = b;
    group.append(b);
  }
  // Drag the top edge to resize while docked.
  handle.addEventListener("pointerdown", (e) => {
    handle.setPointerCapture(e.pointerId);
    const move = (ev) => form.style.setProperty("--dock-height", `${Math.max(160, window.innerHeight - ev.clientY)}px`);
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", () => handle.removeEventListener("pointermove", move), { once: true });
  });
  form.prepend(handle);
  let saved = "";
  try {
    saved = localStorage.getItem(KEY) || "";
  } catch {
    // Storage blocked: start undocked.
  }
  if (sheet) apply("docked");
  else if (saved) apply(saved);
  return group;
}

// Only for the preview, whose HTML comes from the server's cooker.
const previewPolicy = window.trustedTypes?.createPolicy("bf-preview", { createHTML: (s) => s }) ?? {
  createHTML: (s) => s,
};

// Large photos shrink in the browser first: faster uploads, same result. GIFs keep
// their animation, and anything that cannot be decoded goes up untouched.
const MAX_SIDE = 2560;
async function resizeImage(file) {
  if (!/^image\/(jpeg|png|webp)$/.test(file.type) || typeof createImageBitmap === "undefined") return file;
  try {
    const bmp = await createImageBitmap(file);
    const scale = MAX_SIDE / Math.max(bmp.width, bmp.height);
    if (scale >= 1) return file;
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(bmp.width * scale);
    canvas.height = Math.round(bmp.height * scale);
    canvas.getContext("2d").drawImage(bmp, 0, 0, canvas.width, canvas.height);
    const blob = await new Promise((ok) => canvas.toBlob(ok, file.type, 0.85));
    return blob ? new File([blob], file.name, { type: file.type }) : file;
  } catch {
    return file;
  }
}

// XMLHttpRequest, because fetch cannot report upload progress.
function uploadFile(file, onProgress, signal, target = "") {
  return new Promise((resolve, reject) => {
    const body = new FormData();
    body.append("file", file);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", target ? `/api/v1/uploads?for=${encodeURIComponent(target)}` : "/api/v1/uploads");
    xhr.setRequestHeader("X-CSRF-Token", csrf());
    xhr.upload.onprogress = (e) => e.lengthComputable && onProgress(e.loaded / e.total);
    xhr.onerror = () => reject(new Error(tr("Upload failed")));
    xhr.onabort = () => reject(new Error(tr("Cancelled")));
    xhr.onload = () => {
      let data = {};
      try {
        data = JSON.parse(xhr.responseText);
      } catch {
        // Not JSON: the status decides below.
      }
      if (xhr.status >= 200 && xhr.status < 300) resolve(data.markdown);
      else reject(new Error(data.detail || data.title || tr("Upload failed")));
    };
    signal.addEventListener("abort", () => xhr.abort());
    xhr.send(body);
  });
}

// Drafts: saved to the server 2 s after the last change, restored on load.
function drafts(context, getData, setData, status) {
  if (!context) return { schedule() {}, flush() {}, saveNow() {}, discard() {} };
  const url = `/api/v1/drafts?context=${encodeURIComponent(context)}`;
  let timer = null;
  let last = "";
  // Until the saved draft has been read, a save would overwrite it.
  let loaded = false;
  const isEmpty = (data) => !data.body.trim() && !(data.title ?? "").trim();
  const save = async () => {
    if (!loaded) {
      timer = setTimeout(save, 500);
      return;
    }
    const data = getData();
    const json = JSON.stringify(data);
    if (json === last) return;
    const empty = isEmpty(data);
    const res = await fetch(url, {
      method: empty ? "DELETE" : "PUT",
      headers: jsonHeaders(),
      body: empty ? undefined : json,
    }).catch(() => null);
    if (res?.ok) {
      last = json;
      status(empty ? "" : tr("Draft saved"));
    } else {
      status(tr("Draft not saved. Keep this page open and copy your text if this continues."));
    }
  };
  fetch(url)
    .then(async (res) => {
      if (res.status !== 200) return;
      const { data } = await res.json();
      if (data && setData(data)) {
        last = JSON.stringify(getData());
        status(tr("Draft restored"));
      }
    })
    .catch(() => {})
    .finally(() => (loaded = true));
  return {
    schedule() {
      clearTimeout(timer);
      timer = setTimeout(save, 2000);
    },
    flush() {
      clearTimeout(timer);
    },
    // Leaving the page: a keepalive request outlives it.
    saveNow() {
      clearTimeout(timer);
      if (!loaded) return;
      const data = getData();
      const json = JSON.stringify(data);
      if (json === last || isEmpty(data)) return;
      last = json;
      fetch(url, {
        method: "PUT",
        // Browsers refuse keepalive bodies above about 64 KB.
        keepalive: json.length < 60000,
        headers: jsonHeaders(),
        body: json,
      }).catch(() => {});
    },
    // The text went out under another draft context, so this one is finished with.
    discard() {
      clearTimeout(timer);
      last = "";
      fetch(url, { method: "DELETE", keepalive: true, headers: { "X-CSRF-Token": csrf() } }).catch(() => {});
    },
  };
}

// A chip on the form opens a dialog of extra fields (an event, a publish time). Once they are
// filled the chip shows what was set; "Remove" in the dialog clears them.
function wireChip(chip) {
  const box = /** @type {HTMLDialogElement | null} */ (document.getElementById(chip.dataset.dialog ?? ""));
  const label = chip.querySelector(".bf-chip-label");
  if (!box || !label) return;
  const fields = /** @type {(HTMLInputElement | HTMLSelectElement)[]} */ ([...box.querySelectorAll("input, select")]);
  const val = (name) => /** @type {HTMLInputElement | null} */ (box.querySelector(`[name="${name}"]`))?.value ?? "";
  const when = (date, time) => {
    const d = new Date(`${date}T12:00`);
    const day = Number.isNaN(+d)
      ? date
      : d.toLocaleDateString(document.documentElement.lang || undefined, { month: "short", day: "numeric" });
    return time ? `${day}, ${time}` : day;
  };
  const summary = () =>
    chip.dataset.chip === "event"
      ? val("event_date") && when(val("event_date"), val("event_start"))
      : Number(val("publish_minutes")) > 0 && tr("in {n} min", { n: Number(val("publish_minutes")) });
  const refresh = () => {
    const text = summary();
    label.textContent = text || chip.dataset.empty || "";
    chip.classList.toggle("is-set", !!text);
  };
  box.addEventListener("close", refresh);
  box.querySelector("[data-chip-clear]")?.addEventListener("click", () => {
    for (const f of fields) {
      if (f.name === "event_timezone") f.value = "UTC";
      else if (f instanceof HTMLSelectElement) f.selectedIndex = 0;
      else f.value = "";
    }
    refresh();
    box.close();
  });
  refresh();
}

/** The emoticon ending at `pos` as [from, emoji], unless the switch is off or it is inside code. */
function swappable(state, pos) {
  if (!emoticonsOn()) return null;
  const line = state.doc.lineAt(pos);
  const hit = emoticonBefore(line.text, pos - line.from);
  if (!hit) return null;
  const from = line.from + hit[0];
  // In inline code (an odd number of backticks before it) or a fenced block, `:)` is code.
  if (((line.text.slice(0, hit[0]).match(/`/g) ?? []).length & 1) === 1) return null;
  if (((state.sliceDoc(0, from).match(/^\s*```/gm) ?? []).length & 1) === 1) return null;
  return [from, hit[1]];
}

export function mount(textarea, opts = {}) {
  const form = textarea.form;
  const titleInput = form?.querySelector('input[name="title"]');
  // In a reply, the topic: its participants are suggested first for @mentions.
  const topic = /\/t\/(\d+)\/reply$/.exec(form?.getAttribute("action") ?? "")?.[1] ?? null;
  let dirty = false;
  let rawMode = false;
  // Restoring a saved draft changes the document but leaves nothing unsaved.
  let restoring = false;

  const root = mkc("div", "composer-editor");
  const bar = el("div", { class: "composer-toolbar", role: "toolbar", "aria-label": tr("Formatting") });
  // Formatting and inserting on the left; the mode, helpers and options on the right.
  const left = mkc("div", "composer-tools");
  const right = mkc("div", "composer-side");
  // What the "/" menu offers; filled once the Insert menu is built.
  /** @type {{ label: string, detail: string, run: (v: any) => void }[]} */
  const slashItems = [];
  // Shown over selected text: the formatting people reach for first.
  const selbar = el("div", {
    class: "composer-selbar",
    hidden: "",
    role: "toolbar",
    "aria-label": tr("Format selection"),
  });
  selbar.addEventListener("mousedown", (e) => e.preventDefault());
  const pane = mk("div");
  const out = mk("div", { className: "cooked composer-preview", hidden: true });

  // In a shadow root CodeMirror uses constructable stylesheets; on the document it writes
  // a <style> tag, which the strict CSP refuses.
  const shadow = pane.attachShadow({ mode: "open" });
  const view = new EditorView({
    parent: shadow,
    root: shadow,
    state: EditorState.create({
      doc: textarea.value,
      extensions: [
        // Page CSS does not reach the shadow root.
        EditorView.theme({
          "&": opts.sheet
            ? { minHeight: "0" }
            : { minHeight: "var(--bf-editor-min, 10rem)", maxHeight: "var(--bf-editor-max, 60vh)" },
          // The focus ring is drawn on the rounded .composer-editor box (page CSS), not here.
          "&.cm-focused": { outline: "none" },
          ".cm-scroller": { fontFamily: "var(--bf-font-body)", lineHeight: "1.6", padding: ".5rem" },
          ".cm-content": { caretColor: "var(--bf-color-text)" },
          ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--bf-color-text)" },
          ".cm-placeholder": { color: "var(--bf-color-muted)" },
          // Suggestions (@member, #forum, :emoji:): a menu in the site's style.
          ".cm-tooltip": {
            border: "1px solid var(--bf-color-border)",
            borderRadius: "var(--bf-radius)",
            background: "var(--bf-color-surface)",
            color: "var(--bf-color-text)",
            boxShadow: "var(--bf-shadow-menu)",
          },
          ".cm-tooltip.cm-tooltip-autocomplete > ul": {
            fontFamily: "var(--bf-font-body)",
            fontSize: ".9rem",
            minWidth: "15rem",
            maxWidth: "min(24rem, 90vw)",
            maxHeight: "18rem",
            padding: "4px",
          },
          ".cm-tooltip.cm-tooltip-autocomplete > ul > li": {
            display: "flex",
            alignItems: "center",
            gap: ".6rem",
            padding: ".4rem .55rem",
            borderRadius: "6px",
            lineHeight: "1.3",
            cursor: "pointer",
          },
          ".cm-tooltip.cm-tooltip-autocomplete > ul > li:hover": { background: "var(--bf-color-surface-2)" },
          ".cm-tooltip-autocomplete ul li[aria-selected]": {
            background: "var(--bf-color-accent-soft)",
            color: "var(--bf-color-text)",
          },
          ".cm-completionLabel": { fontWeight: "600", whiteSpace: "nowrap" },
          ".cm-completionMatchedText": { textDecoration: "none", color: "var(--bf-color-accent)" },
          ".cm-completionDetail": {
            marginInlineStart: "auto",
            paddingInlineStart: ".75rem",
            fontStyle: "normal",
            fontSize: ".82rem",
            color: "var(--bf-color-muted)",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          },
          ".cm-mention-avatar": { width: "24px", height: "24px", borderRadius: "50%", flex: "none" },
          ".cm-mention-mark": {
            display: "grid",
            placeItems: "center",
            width: "24px",
            height: "24px",
            borderRadius: "50%",
            flex: "none",
            background: "var(--bf-color-surface-2)",
            color: "var(--bf-color-muted)",
            fontWeight: "700",
            fontSize: ".8rem",
          },
          ".cm-image": {
            position: "relative",
            width: "fit-content",
            maxWidth: "100%",
            margin: ".25rem 0",
            cursor: "grab",
            lineHeight: "0",
          },
          ".cm-image img": { display: "block", maxWidth: "100%", height: "auto", borderRadius: "4px" },
          ".cm-image:hover img": { outline: "2px solid var(--bf-color-accent)" },
          ".cm-image-handle": {
            position: "absolute",
            insetInlineEnd: "-4px",
            insetBlockEnd: "-4px",
            width: "14px",
            height: "14px",
            borderRadius: "50%",
            background: "var(--bf-color-accent)",
            border: "2px solid var(--bf-color-surface)",
            cursor: "nwse-resize",
            touchAction: "none",
            opacity: "0",
          },
          ".cm-image:hover .cm-image-handle, .cm-image-handle:active": { opacity: "1" },
          ".cm-grid": {
            position: "relative",
            display: "grid",
            gridTemplateColumns: "repeat(auto-fill, minmax(min(100%, 9rem), 1fr))",
            gap: ".35rem",
            margin: ".25rem 0",
            lineHeight: "0",
          },
          ".cm-grid-tile": {
            cursor: "pointer",
            overflow: "hidden",
            borderRadius: "4px",
            aspectRatio: "4 / 3",
            lineHeight: "1.3",
            fontSize: ".8rem",
            color: "var(--bf-color-muted)",
            background: "var(--bf-color-surface-2)",
          },
          ".cm-grid-tile img": { display: "block", width: "100%", height: "100%", objectFit: "cover" },
          ".cm-grid-tile:hover": { outline: "2px solid var(--bf-color-accent)" },
          ".cm-grid-bar": {
            position: "absolute",
            insetBlockStart: "4px",
            insetInlineEnd: "4px",
            display: "flex",
            gap: "4px",
            opacity: "0",
          },
          ".cm-grid:hover .cm-grid-bar, .cm-grid-bar:focus-within": { opacity: "1" },
          ".cm-grid-bar button, .cm-grid-bar select": {
            font: "inherit",
            fontSize: ".78rem",
            lineHeight: "1.2",
            padding: ".2rem .55rem",
            border: "1px solid var(--bf-color-border)",
            borderRadius: "999px",
            background: "var(--bf-color-surface)",
            color: "var(--bf-color-text)",
            cursor: "pointer",
          },
          ".cm-md-strong": { fontWeight: "700" },
          ".cm-md-em": { fontStyle: "italic" },
          ".cm-md-strike": { textDecoration: "line-through" },
          ".cm-md-underline": { textDecoration: "underline" },
          ...Object.fromEntries(Object.keys(SWATCH).map((c) => [`.cm-clr-${c}`, { color: `var(--bf-clr-${c})` }])),
          ".cm-md-code": {
            fontFamily: "var(--bf-font-mono)",
            fontSize: ".9em",
            background: "var(--bf-color-surface-2)",
            borderRadius: "3px",
            padding: "0 .2em",
          },
          ".cm-md-link": { color: "var(--bf-color-accent)" },
          ".cm-md-bullet": { color: "var(--bf-color-muted)" },
          ".cm-li-bullet": { color: "var(--bf-color-muted)" },
          ".cm-li-check": { margin: "0 .4em 0 0", verticalAlign: "-0.08em", cursor: "pointer" },
          ".cm-md-h1": { fontSize: "1.4em", fontWeight: "650" },
          ".cm-md-h2": { fontSize: "1.2em", fontWeight: "650" },
          ".cm-md-h3": { fontSize: "1.05em", fontWeight: "650" },
          ".cm-md-quote": {
            borderLeft: "3px solid var(--bf-color-border-strong)",
            paddingLeft: ".75rem",
            color: "var(--bf-color-muted)",
          },
          ".cm-md-fence": {
            fontFamily: "var(--bf-font-mono)",
            fontSize: ".9em",
            background: "var(--bf-color-surface-2)",
          },
          ".cm-table-wrap": { margin: ".4rem 0", overflowX: "auto" },
          ".cm-table": { borderCollapse: "collapse", fontSize: ".95em", background: "var(--bf-color-surface)" },
          ".cm-table th, .cm-table td": {
            border: "1px solid var(--bf-color-border-strong)",
            padding: ".3rem .55rem",
            textAlign: "left",
            minWidth: "3rem",
            height: "1.8rem",
            cursor: "text",
          },
          ".cm-table th": { background: "var(--bf-color-surface-2)", fontWeight: "650" },
          ".cm-table th:focus, .cm-table td:focus": {
            outline: "2px solid var(--bf-color-accent)",
            outlineOffset: "-2px",
          },
          ".cm-table-tools": {
            display: "flex",
            gap: "3px",
            marginBottom: "3px",
            opacity: "0",
            transition: "opacity .12s",
          },
          ".cm-table-wrap:hover .cm-table-tools, .cm-table-wrap:focus-within .cm-table-tools": { opacity: "1" },
          ".cm-table-tool": {
            font: "inherit",
            fontSize: ".72rem",
            lineHeight: "1.4",
            padding: "1px 7px",
            border: "1px solid var(--bf-color-border)",
            borderRadius: "4px",
            background: "var(--bf-color-surface-2)",
            color: "var(--bf-color-text)",
            cursor: "pointer",
          },
          ".cm-table-tool:hover": { borderColor: "var(--bf-color-accent)", color: "var(--bf-color-accent)" },
        }),
        history(),
        visualCompartment.of(VISUAL),
        autocompletion({
          override: [
            completeFor(topic),
            // "/" at the start of a line lists what can be inserted, as the Insert menu does.
            (ctx) => {
              const line = ctx.state.doc.lineAt(ctx.pos);
              const m = /^\/([\w ]*)$/.exec(line.text.slice(0, ctx.pos - line.from));
              if (!m || !slashItems.length) return null;
              return {
                from: ctx.pos - m[1].length,
                filter: true,
                options: slashItems.map((i) => ({
                  label: i.label,
                  detail: i.detail,
                  apply: (v, _c, from, to) => {
                    v.dispatch({ changes: { from: from - 1, to } });
                    i.run(v);
                  },
                })),
              };
            },
          ],
          icons: false,
          addToOptions: [completionPicture],
          maxRenderedOptions: 12,
        }),
        // Tab takes the highlighted suggestion, as Enter does; with none open it leaves the editor.
        Prec.highest(keymap.of([{ key: "Tab", run: acceptCompletion }])),
        EditorView.lineWrapping,
        placeholder(textarea.getAttribute("placeholder") || tr("Write in Markdown…")),
        keymap.of([
          { key: "Mod-b", run: format("**") },
          { key: "Mod-i", run: format("*") },
          { key: "Mod-u", run: format("++") },
          { key: "Mod-e", run: format("`") },
          { key: "Mod-k", run: link },
          { key: "Mod-Shift-x", run: format("~~") },
          { key: "Mod-Shift-7", run: numbers },
          { key: "Mod-Shift-8", run: bullets },
          { key: "Mod-Shift-9", run: quote },
          { key: "Mod-Alt-1", run: heading(2) },
          { key: "Mod-Alt-2", run: heading(3) },
          { key: "Mod-Alt-3", run: heading(4) },
          // An emoticon just typed at the end of the line changes before Enter ends it.
          {
            key: "Enter",
            run: (v) => {
              const at = v.state.selection.main;
              const hit = at.empty && swappable(v.state, at.head);
              if (hit) v.dispatch({ changes: { from: hit[0], to: at.head, insert: hit[1] }, userEvent: "input.type" });
              return false;
            },
          },
          { key: "Enter", run: continueList },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        // Typing a space, a full stop or the like after an emoticon turns it into its emoji.
        EditorView.inputHandler.of((v, from, to, text) => {
          if (from !== to || !ENDS.test(text)) return false;
          const hit = swappable(v.state, from);
          if (!hit) return false;
          v.dispatch({
            changes: { from: hit[0], to: from, insert: hit[1] + text },
            selection: { anchor: hit[0] + hit[1].length + text.length },
            userEvent: "input.type",
          });
          return true;
        }),
        EditorView.contentAttributes.of({
          "aria-label": textarea.labels?.[0]?.textContent || textarea.getAttribute("aria-label") || tr("Message"),
        }),
        EditorView.updateListener.of((u) => {
          if (u.selectionSet || u.docChanged || u.focusChanged || u.geometryChanged) showSelection(u.view);
          if (u.docChanged) {
            textarea.value = u.state.doc.toString();
            count();
            // For listeners on the hidden textarea, such as the PM typing indicator.
            textarea.dispatchEvent(new Event("input", { bubbles: true }));
            if (restoring) return;
            warnDuplicates(textarea.value);
            dirty = true;
            draft.schedule();
          }
        }),
        EditorView.domEventHandlers({
          paste: (e, v) => {
            const data = e.clipboardData;
            if (!data || insertFiles(data.files)) return !!data;
            const r = v.state.selection.main;
            const plain = data.getData("text/plain").trim();
            // A web address pasted over selected text makes it a link.
            if (r.from !== r.to && /^https?:\/\/\S+$/.test(plain)) {
              e.preventDefault();
              const label = v.state.sliceDoc(r.from, r.to);
              v.dispatch({
                changes: { from: r.from, to: r.to, insert: `[${label}](${plain})` },
                userEvent: "input.paste",
              });
              return true;
            }
            // Formatted text becomes Markdown, except in a code fence or the raw view.
            const html = data.getData("text/html");
            if (!rawMode && html && FORMATTED.test(html)) {
              const fenced = ((v.state.sliceDoc(0, r.from).match(/^\s*```/gm) ?? []).length & 1) === 1;
              const md = fenced ? "" : htmlToMarkdown(html);
              if (md) {
                e.preventDefault();
                v.dispatch({ ...v.state.replaceSelection(md), userEvent: "input.paste", scrollIntoView: true });
                return true;
              }
            }
            return false;
          },
          dragover: (e) => {
            if (!movingImage) return false;
            e.preventDefault();
            return true;
          },
          drop: (e, v) => {
            if (!movingImage) return insertFiles(e.dataTransfer?.files);
            e.preventDefault();
            return dropImage(v, e);
          },
        }),
      ],
    }),
  });

  const statusEl = el("div", { class: "composer-status", role: "status" });
  const status = (text) => (statusEl.textContent = text);
  // How much of the post's length is used: shown from 80% of the limit, red over it.
  const limit = Number(document.querySelector('meta[name="bf-post"]')?.content) || 0;
  const countEl = mk("div", { className: "composer-count", hidden: true });
  const count = () => {
    const n = view.state.doc.length;
    countEl.hidden = !limit || n < limit * 0.8;
    countEl.classList.toggle("is-over", n > limit);
    countEl.textContent = tr("{n} of {max} characters", { n: n.toLocaleString(), max: limit.toLocaleString() });
  };

  // Text the server put in the form after an error wins over the saved draft.
  const serverText = !!textarea.value.trim();
  const draft = drafts(
    opts.draft,
    () => ({ body: view.state.doc.toString(), ...(titleInput ? { title: titleInput.value } : {}) }),
    (data) => {
      if (serverText || !data.body) return false;
      // A quote or typing may have arrived while the draft loaded: keep both.
      const current = view.state.doc.toString();
      if (current.includes(data.body)) return false;
      restoring = !current;
      view.dispatch({ changes: { from: 0, insert: current ? `${data.body}\n\n` : data.body } });
      restoring = false;
      if (titleInput && !titleInput.value && data.title) titleInput.value = data.title;
      return true;
    },
    status,
  );
  titleInput?.addEventListener("input", () => draft.schedule());
  // Similar topics while typing a title.
  if (titleInput) {
    const similar = mkc("ul", "similar small");
    titleInput.after(similar);
    let timer;
    const look = () => {
      clearTimeout(timer);
      timer = setTimeout(async () => {
        const q = titleInput.value
          .trim()
          .split(/\s+/)
          .filter((w) => w.length > 3)
          .join(" or ");
        similar.replaceChildren();
        if (!q) return;
        const res = await fetch(`/api/v1/search?in=topics&q=${encodeURIComponent(q)}`).catch(() => null);
        if (!res?.ok) return;
        for (const r of (await res.json()).slice(0, 5)) {
          const li = mk("li", {}, mk("a", { href: r.url, target: "_blank", textContent: r.title }));
          if (r.forum) li.append(" ", mk("span", { className: "muted bf-similar-forum", textContent: r.forum }));
          similar.append(li);
        }
      }, 500);
    };
    titleInput.addEventListener("input", look);
    look();
  }

  // Tells the writer when a link in the text was already posted. Debounced; a newer check
  // cancels the one in flight, and a failed request leaves the hint as it was.
  const seenEl = el("div", { class: "composer-status", role: "status" });
  let seenTimer;
  let seenAbort;
  let seenKey = "";
  function warnDuplicates(text) {
    clearTimeout(seenTimer);
    seenTimer = setTimeout(async () => {
      const urls = extractUrls(text);
      const key = urls.join("\n");
      if (key === seenKey) return;
      seenKey = key;
      seenAbort?.abort();
      if (!urls.length) {
        seenEl.textContent = "";
        return;
      }
      const abort = (seenAbort = new AbortController());
      const res = await fetch("/api/v1/links/seen", {
        method: "POST",
        headers: jsonHeaders(),
        body: JSON.stringify({ urls }),
        signal: abort.signal,
      }).catch(() => null);
      if (!res?.ok || abort.signal.aborted) return;
      const seen = await res.json().catch(() => []);
      if (abort.signal.aborted) return;
      seenEl.textContent = seen.length
        ? tr("Already posted: {list}", {
            list: seen.map((s) => tr("{url} in “{title}”", { url: s.url, title: s.title })).join("; "),
          })
        : "";
    }, 800);
  }

  // Puts Markdown on its own line at the cursor: right after other text, a link could read as part
  // of it (or as an image, after a "!").
  function insertOnOwnLine(md) {
    const pos = view.state.selection.main.head;
    const lead = view.state.doc.lineAt(pos).from < pos ? "\n" : "";
    view.dispatch({
      changes: { from: pos, insert: `${lead}${md}\n` },
      selection: { anchor: pos + lead.length + md.length + 1 },
    });
  }

  // Returns true when it handled the event, so CodeMirror skips its default.
  function insertFiles(files) {
    if (!files || !files.length) return false;
    for (const file of files) {
      const row = mkc("div", "upload-row");
      const bar = mk("progress", { max: 1, value: 0 });
      bar.setAttribute("aria-label", tr("Uploading {name}", { name: file.name }));
      const stop = mk("button", { type: "button", className: "link", textContent: tr("Cancel") });
      const abort = new AbortController();
      stop.addEventListener("click", () => abort.abort());
      row.append(`${file.name} `, bar, " ", stop);
      footline.before(row);
      resizeImage(file)
        .then((small) => uploadFile(small, (p) => (bar.value = p), abort.signal))
        .then(
          (md) => {
            insertOnOwnLine(md);
            status(tr("Uploaded {name}", { name: file.name }));
          },
          (err) => status(`${file.name}: ${err.message}`),
        )
        .finally(() => row.remove());
    }
    return true;
  }

  for (const [icon, title, run, keys] of TOOLS) {
    const b = iconButton(icon, title + hint(keys));
    b.addEventListener("click", () => run(view));
    if (icon !== "bold" && icon !== "italic" && icon !== "link") b.classList.add("bf-narrow-hide");
    left.append(b);
  }
  for (const [icon, title, run] of /** @type {[string, string, (view: any) => boolean][]} */ ([
    ...TOOLS.filter((t) => ["bold", "italic", "link", "code"].includes(t[0])),
    ["code", tr("Code"), format("`")],
    ["strikethrough", tr("Strikethrough"), format("~~")],
  ])) {
    const b = iconButton(icon, title);
    b.addEventListener("click", () => run(view));
    selbar.append(b);
  }
  // Floats over the selection (or beneath it, at the top of the box).
  function showSelection(v) {
    const r = v.state.selection.main;
    const at = r.empty || !v.hasFocus || pane.hidden || rawMode ? null : v.coordsAtPos(r.from);
    if (!at) {
      selbar.hidden = true;
      return;
    }
    selbar.hidden = false;
    const box = root.getBoundingClientRect();
    const above = at.top - box.top - selbar.offsetHeight - 6;
    selbar.style.insetBlockStart = `${above >= 0 ? above : at.bottom - box.top + 6}px`;
    selbar.style.insetInlineStart = `${Math.max(0, Math.min(at.left - box.left, box.width - selbar.offsetWidth))}px`;
  }

  // A button that opens a list of actions under it.
  const dropdown = (icon, label, text) => {
    const wrap = mkc("div", "composer-more");
    const button = iconButton(icon, label);
    if (text) button.append(mk("span", { className: "composer-btn-text", textContent: text }));
    button.setAttribute("aria-haspopup", "true");
    button.setAttribute("aria-expanded", "false");
    const panel = mk("div", { className: "composer-more-panel", hidden: true });
    const close = () => {
      panel.hidden = true;
      button.setAttribute("aria-expanded", "false");
    };
    button.addEventListener("click", () => {
      panel.hidden = !panel.hidden;
      button.setAttribute("aria-expanded", String(!panel.hidden));
      // Near the bottom of the screen (a docked composer) the list opens upwards.
      panel.classList.remove("is-up");
      if (!panel.hidden && panel.getBoundingClientRect().bottom > innerHeight - 8) panel.classList.add("is-up");
    });
    document.addEventListener("click", (e) => {
      if (!wrap.contains(/** @type {Node} */ (e.target))) close();
    });
    wrap.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && !panel.hidden) {
        close();
        button.focus();
      }
    });
    wrap.append(button, panel);
    const add = (icon, title, run, keys = "", cls = "") => {
      const item = iconButton(icon, title + hint(keys));
      item.className = `composer-more-item ${cls}`.trim();
      item.append(mk("span", { textContent: title }));
      if (keys) item.append(mk("kbd", { textContent: hint(keys).slice(2, -1) }));
      item.addEventListener("click", () => {
        close();
        run();
      });
      panel.append(item);
      return item;
    };
    return { wrap, button, panel, add, close };
  };
  const postMeta = document.querySelector('meta[name="bf-post"]');
  const insert = dropdown("plus", tr("Insert"), tr("Insert"));
  insert.button.classList.add("composer-insert");
  const anchor = () => insert.button;
  for (const [icon, title, run, keys, kind] of INSERT) {
    if (kind === "math" && !postMeta?.hasAttribute("data-math")) continue;
    const go = () =>
      run === "table" ? openTableDialog(view, anchor()) : run === "palette" ? openColorMenu(view, anchor()) : run(view);
    insert.add(icon, title, go, keys, kind === "narrow" ? "bf-narrow-only" : "");
    if (kind !== "narrow") slashItems.push({ label: title, detail: hint(keys).slice(2, -1), run: go });
  }
  left.append(insert.wrap);
  // Buttons from plugins: those registered already, and any that arrive later.
  const addPluginButton = ({ label, icon, onClick }) => {
    const b = icon ? iconButton(icon, label) : mk("button", { type: "button", className: "btn", textContent: label });
    const sel = () => view.state.selection.main;
    b.addEventListener("click", () =>
      onClick({
        insert: (text) => {
          view.dispatch(view.state.replaceSelection(String(text)));
          view.focus();
        },
        // The selected text ("" when nothing is selected), and the whole document.
        selection: () => view.state.sliceDoc(sel().from, sel().to),
        text: () => view.state.doc.toString(),
        // Surrounds the selection: wrap("**", "**"), wrap("[x]", "[/x]").
        wrap: (before, after = before) => {
          const { from, to } = sel();
          const inner = view.state.sliceDoc(from, to);
          view.dispatch({
            changes: { from, to, insert: `${before}${inner}${after}` },
            selection: { anchor: from + String(before).length, head: from + String(before).length + inner.length },
          });
          view.focus();
        },
        // Replaces the selection and selects what was put there.
        replace: (text) => {
          const { from, to } = sel();
          view.dispatch({
            changes: { from, to, insert: String(text) },
            selection: { anchor: from, head: from + String(text).length },
          });
          view.focus();
        },
      }),
    );
    left.append(b);
  };
  (window.bfComposerButtons ?? []).forEach(addPluginButton);
  document.addEventListener("bf:composer-button", (e) => addPluginButton(e.detail));
  // Write and Preview: the second is the post as the server will render it, so link previews,
  // hidden blocks, math and the rest show up there.
  const modes = el("div", { class: "composer-modes", role: "group", "aria-label": tr("Mode") });
  const modeButton = (icon, label) => {
    const b = iconButton(icon, label);
    b.append(mk("span", { className: "composer-btn-text", textContent: label }));
    return b;
  };
  const writeMode = modeButton("pencil", tr("Write"));
  const peek = modeButton("eye", tr("Preview"));
  const showMode = async (preview) => {
    writeMode.setAttribute("aria-pressed", String(!preview));
    peek.setAttribute("aria-pressed", String(preview));
    out.hidden = !preview;
    pane.hidden = preview;
    selbar.hidden = true;
    for (const b of bar.querySelectorAll("button")) if (!modes.contains(b)) b.disabled = preview;
    if (!preview) return view.focus();
    out.textContent = tr("Loading…");
    try {
      const res = await fetch("/api/v1/preview", {
        method: "POST",
        headers: jsonHeaders({ Accept: "application/json" }),
        body: JSON.stringify({ raw: view.state.doc.toString() }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) throw new Error(data.detail || data.title);
      out.innerHTML = previewPolicy.createHTML(data.html || `<p class="muted">${tr("Nothing to preview yet.")}</p>`);
    } catch (e) {
      out.textContent = e.message || tr("That did not work. Try again.");
    }
  };
  writeMode.addEventListener("click", () => peek.getAttribute("aria-pressed") === "true" && showMode(false));
  peek.addEventListener("click", () => peek.getAttribute("aria-pressed") !== "true" && showMode(true));
  writeMode.setAttribute("aria-pressed", "true");
  peek.setAttribute("aria-pressed", "false");
  modes.append(writeMode, peek);

  // Less common options: the Markdown source, for those who prefer to see it all.
  const options = dropdown("ellipsis", tr("Editor options"));
  const source = options.add("code", tr("Markdown source"), () => {
    rawMode = !rawMode;
    source.setAttribute("aria-pressed", String(rawMode));
    source.lastElementChild.textContent = rawMode ? tr("Visual editor") : tr("Markdown source");
    selbar.hidden = true;
    view.dispatch({ effects: visualCompartment.reconfigure(rawMode ? [] : VISUAL) });
    view.focus();
  });
  source.setAttribute("aria-pressed", "false");

  const picker = mk("input", { type: "file", multiple: true, hidden: true });
  picker.addEventListener("change", () => {
    insertFiles(picker.files);
    picker.value = "";
  });
  const attach = iconButton("paperclip", tr("Upload images or files (or paste / drop them)"));
  attach.addEventListener("click", () => picker.click());
  // A panel of our own: a native list ignores the dark theme in some browsers.
  const emoji = mkc("div", "composer-emoji");
  const emojiButton = iconButton("smile", tr("Insert emoji"));
  const emojiPanel = mk("div", { className: "composer-emoji-panel", hidden: true });
  const pick = (text) => () => {
    wrap(text, "")(view);
    emojiPanel.hidden = true;
  };
  for (const e of "😀 😂 😉 😍 😎 🤔 😢 😡 👍 👎 👏 🙏 🎉 ❤️ 🔥 ✅ ❌ ⭐ 💡 🚀".split(" ")) {
    const b = mk("button", { type: "button", textContent: e });
    b.addEventListener("click", pick(withTone(e)));
    emojiPanel.append(b);
  }
  // The site's own emoji and the sets plugins ship, with a heading for each set, on first open.
  let customLoaded = false;
  const loadCustom = async () => {
    customLoaded = true;
    let heading = null;
    for (const c of [...(await loadEmoji())].sort((a, b) => (a.set ?? "").localeCompare(b.set ?? ""))) {
      if (c.set && c.set !== heading) {
        heading = c.set;
        emojiPanel.append(mk("span", { className: "bf-emoji-set", textContent: c.set }));
      }
      const b = mk("button", { type: "button" }, mk("img", { src: c.url, alt: "" }));
      b.setAttribute("aria-label", c.label || `:${c.name}:`);
      b.addEventListener("click", pick(`:${c.name}:`));
      emojiPanel.append(b);
    }
  };
  emojiButton.addEventListener("click", () => {
    emojiPanel.hidden = !emojiPanel.hidden;
    if (!emojiPanel.hidden && !customLoaded) loadCustom();
  });
  document.addEventListener("click", (e) => {
    if (!emoji.contains(/** @type {Node} */ (e.target))) emojiPanel.hidden = true;
  });
  emoji.append(emojiButton, emojiPanel);
  attach.classList.add("bf-narrow-hide");
  emoji.classList.add("bf-narrow-hide");
  left.append(attach, picker, emoji);
  // GIFs, when the site has a provider and a key: the picker loads as the button is first used.
  const gifMeta = /** @type {HTMLMetaElement | null} */ (document.querySelector('meta[name="bf-gif"]'));
  if (gifMeta?.content) {
    const cfg = {
      key: gifMeta.content,
      provider: gifMeta.dataset.provider ?? "klipy",
      format: gifMeta.dataset.format ?? "webp",
      filter: gifMeta.dataset.filter ?? "high",
      country: gifMeta.dataset.country ?? "US",
      limit: Number(gifMeta.dataset.limit) || 0,
    };
    const openGifs = async () => {
      const { openGifPicker } = await import("./gif.js");
      openGifPicker(cfg, (markdown) => {
        insertOnOwnLine(markdown);
        view.focus();
      });
    };
    const gifButton = mk("button", {
      type: "button",
      className: "bf-icon-btn composer-gif-btn bf-narrow-hide",
      textContent: "GIF",
    });
    gifButton.setAttribute("aria-label", tr("Insert a GIF"));
    gifButton.addEventListener("click", openGifs);
    left.append(gifButton);
    insert.add("film", tr("GIF"), openGifs, "", "bf-narrow-only");
    slashItems.push({ label: tr("GIF"), detail: "", run: openGifs });
  }
  insert.add("paperclip", tr("Image or file"), () => picker.click(), "", "bf-narrow-only");
  insert.add("smile", tr("Emoji"), () => emojiButton.click(), "", "bf-narrow-only");
  slashItems.push({ label: tr("Image or file"), detail: "", run: () => picker.click() });
  slashItems.push({ label: tr("Emoji"), detail: "", run: () => emojiButton.click() });
  const eventChip = form?.querySelector('[data-chip="event"]');
  if (eventChip instanceof HTMLElement) {
    insert.add("calendar", tr("Event"), () => eventChip.click());
    slashItems.push({ label: tr("Event"), detail: "", run: () => eventChip.click() });
  }
  const helper = aiHelper({ view, iconButton, topic, form });
  if (helper) right.append(helper);
  right.append(modes, options.wrap);
  // Private messages have no drafts or docked panel.
  if (form && /^(reply|new-topic):/.test(opts.draft ?? "")) {
    const controls = dockControls(form, view, opts.draft, opts.sheet);
    const slot = form.querySelector("[data-composer-controls]");
    // Docking is for wide screens; on a phone the bar keeps its room for writing.
    if (slot) {
      slot.append(controls);
    } else {
      right.append(mk("span", { className: "composer-dock bf-narrow-hide" }, controls));
    }
  }
  const closeSheet = () => {
    if (dirty) draft.saveNow(); // reopening restores it
    dirty = false;
    try {
      localStorage.removeItem(OPEN_KEY);
    } catch {
      // Storage blocked: nothing to forget.
    }
    form.classList.remove("is-open");
    if (location.hash === "#new-topic") window.history.replaceState(null, "", location.pathname + location.search);
  };
  if (opts.draft) {
    // Two steps instead of a browser dialog: the first click arms it for a few seconds.
    const discard = iconButton("trash-2", tr("Discard draft"));
    discard.className = "btn composer-discard";
    const word = mk("span");
    discard.append(word);
    let armed = null;
    const disarm = () => {
      clearTimeout(armed);
      armed = null;
      word.textContent = tr("Discard");
      discard.classList.remove("is-armed");
    };
    disarm();
    discard.addEventListener("blur", disarm);
    discard.addEventListener("click", async () => {
      if (!armed) {
        word.textContent = tr("Click again to discard");
        discard.classList.add("is-armed");
        armed = setTimeout(disarm, 4000);
        return;
      }
      disarm();
      draft.flush();
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: "" } });
      if (titleInput) titleInput.value = "";
      draft.flush();
      await fetch(`/api/v1/drafts?context=${encodeURIComponent(opts.draft)}&purge=1`, {
        method: "DELETE",
        headers: { "X-CSRF-Token": csrf() },
      });
      dirty = false;
      localStorage.removeItem(OPEN_KEY);
      if (opts.sheet) closeSheet();
      else status(tr("Draft discarded"));
    });
    right.append(discard);
  }

  // The editable line is only as tall as its text: a click anywhere in the box must focus it.
  pane.addEventListener("click", () => view.hasFocus || view.focus());
  if (opts.sheet) {
    const close = iconButton("x", tr("Close"));
    close.addEventListener("click", closeSheet);
    form.querySelector("[data-composer-controls]")?.append(close);
  }
  const foot = form?.querySelector("[data-composer-foot]");
  if (foot) {
    // Discard sits beside the send button, so the tools can take a row of their own on phones.
    const discard = right.querySelector(".composer-discard");
    if (discard) foot.prepend(discard);
  }
  bar.append(left, right);
  const footline = mkc("div", "composer-footline", statusEl);
  footline.append(countEl);
  root.append(pane, out, selbar, footline, seenEl);
  count();
  textarea.hidden = true;
  textarea.removeAttribute("required"); // hidden fields cannot show validation bubbles
  textarea.after(root);
  root.before(bar);
  for (const chip of form?.querySelectorAll("[data-chip]") ?? []) wireChip(/** @type {HTMLElement} */ (chip));

  // The stored panel may belong to another composer; only this one's text follows the member.
  const docked = () => {
    try {
      const open = JSON.parse(localStorage.getItem(OPEN_KEY) || "null");
      return open && open.draft === opts.draft ? open : null;
    } catch {
      return null;
    }
  };
  window.addEventListener("beforeunload", (e) => {
    // A docked reply is saved and follows the member, so leaving is safe.
    if (dirty && textarea.value.trim() && !docked()) e.preventDefault();
  });
  window.addEventListener("pagehide", () => {
    if (dirty) draft.saveNow();
    // The next page may ask for the draft before that save lands, so a docked panel also
    // carries its text across; it is removed on submit or discard.
    try {
      const open = docked();
      if (open)
        localStorage.setItem(
          OPEN_KEY,
          JSON.stringify({ ...open, body: textarea.value, titleText: titleInput?.value ?? "" }),
        );
    } catch {
      // Storage blocked: the saved draft still restores it.
    }
  });
  const sent = () => {
    dirty = false;
    localStorage.removeItem(OPEN_KEY);
    draft.flush(); // the server deletes the draft once the post is saved
  };
  // A sheet posts by fetch and stays open if refused, so only success may clear this.
  if (opts.sheet) {
    form?.addEventListener("bf:sent", () => {
      sent();
      draft.discard();
    });
  } else {
    form?.addEventListener("submit", sent);
  }
  form?.addEventListener("bf:focus", () => view.focus());

  // A right click opens the actions people look for in a menu, for what is under the pointer: a
  // picture or a grid, the selected text, or the place in the text. Shift keeps the browser's own menu.
  const kb = (keys) =>
    hint(keys)
      .slice(2, -1)
      .replace(/(^|\+)([a-z])$/, (_, plus, c) => plus + c.toUpperCase());
  const pasteFromClipboard = () =>
    navigator.clipboard
      ?.readText()
      .then((text) =>
        view.dispatch({ ...view.state.replaceSelection(text), userEvent: "input.paste", scrollIntoView: true }),
      )
      .catch(() => status(tr("Press {keys} to paste", { keys: kb("Mod-v") })));
  // A submenu to choose how many tiles a grid has in a row; the present choice is ticked.
  const columnsItem = (block) => ({
    label: tr("Columns"),
    children: COLUMNS.map((n) => ({
      label: n ? String(n) : tr("Automatic"),
      icon: n === block.columns ? "check" : undefined,
      run: () => setColumns(view, block, n),
    })),
  });
  const contextItems = (tile, gridEl, x, y) => {
    const sel = view.state.selection.main;
    const at = { getBoundingClientRect: () => new DOMRect(x, y, 0, 0) };
    const go = (run) => () =>
      run === "table" ? openTableDialog(view, at) : run === "palette" ? openColorMenu(view, at) : run(view);
    /** @type {any[]} */
    const items = [];
    const find = tile ? finders.get(tile) : null;
    const found = find?.();
    if (found && tile) {
      const [src] = found.url.split("#");
      const inGrid = tile.matches(".cm-grid-tile");
      const block = gridAt(view.state, found.from);
      items.push(
        { label: tr("Edit picture…"), icon: "image", run: () => openImageEditor(view, tile, find, { size: !inGrid }) },
        { label: tr("Open picture"), run: () => window.open(src, "_blank", "noopener") },
        {
          label: tr("Copy picture address"),
          run: () => navigator.clipboard?.writeText(new URL(src, location.href).href),
        },
        { label: tr("Remove picture"), icon: "trash-2", run: () => removeImage(view, found.from, found.to) },
        ...(block
          ? [columnsItem(block), { label: tr("Ungroup the grid"), run: () => ungroup(view, block) }]
          : [{ label: tr("Show these pictures side by side"), run: () => groupImages(view, found.from) }]),
        "-",
      );
    } else if (gridEl) {
      const block = gridAt(view.state, view.posAtDOM(gridEl));
      if (block) {
        items.push(
          {
            label: tr("Edit the grid's lines"),
            run: () => view.dispatch({ selection: { anchor: block.open.to + 1 } }),
          },
          columnsItem(block),
          { label: tr("Ungroup the grid"), run: () => ungroup(view, block) },
          "-",
        );
      }
    }
    items.push(
      {
        label: tr("Cut"),
        icon: "scissors",
        keys: kb("Mod-x"),
        disabled: sel.empty,
        run: () => document.execCommand("cut"),
      },
      { label: tr("Copy"), keys: kb("Mod-c"), disabled: sel.empty, run: () => document.execCommand("copy") },
      { label: tr("Paste"), keys: kb("Mod-v"), run: pasteFromClipboard },
      {
        label: tr("Select all"),
        keys: kb("Mod-a"),
        run: () => view.dispatch({ selection: { anchor: 0, head: view.state.doc.length } }),
      },
      "-",
      { label: tr("Bold"), icon: "bold", keys: kb("Mod-b"), run: () => format("**")(view) },
      { label: tr("Italic"), icon: "italic", keys: kb("Mod-i"), run: () => format("*")(view) },
      { label: tr("Underline"), icon: "underline", keys: kb("Mod-u"), run: () => format("++")(view) },
      { label: tr("Strikethrough"), icon: "strikethrough", keys: kb("Mod-Shift-x"), run: () => format("~~")(view) },
      { label: tr("Code"), icon: "code", keys: kb("Mod-e"), run: () => format("`")(view) },
      { label: tr("Link"), icon: "link", keys: kb("Mod-k"), run: () => link(view) },
      "-",
      {
        label: tr("Paragraph"),
        icon: "heading",
        children: [
          { label: tr("Heading"), keys: kb("Mod-Alt-1"), run: () => heading(2)(view) },
          { label: tr("Subheading"), keys: kb("Mod-Alt-2"), run: () => heading(3)(view) },
          { label: tr("Small heading"), keys: kb("Mod-Alt-3"), run: () => heading(4)(view) },
          "-",
          { label: tr("Quote"), icon: "quote", keys: kb("Mod-Shift-9"), run: () => quote(view) },
          { label: tr("Bulleted list"), icon: "list", keys: kb("Mod-Shift-8"), run: () => bullets(view) },
          { label: tr("Numbered list"), icon: "list-ordered", keys: kb("Mod-Shift-7"), run: () => numbers(view) },
          { label: tr("Task list"), icon: "list-checks", run: () => tasks(view) },
          { label: tr("Code block"), icon: "code-xml", run: () => fenced("```", "```")(view) },
        ],
      },
      {
        label: tr("Insert"),
        icon: "plus",
        children: [
          { label: tr("Image or file"), icon: "paperclip", run: () => picker.click() },
          { label: tr("Emoji"), icon: "smile", run: () => emojiButton.click() },
          ...INSERT.filter(
            ([, , , , kind]) => kind !== "narrow" && (kind !== "math" || postMeta?.hasAttribute("data-math")),
          ).map(([icon, label, run, keys]) => ({ label, icon, keys: keys ? kb(keys) : undefined, run: go(run) })),
        ],
      },
    );
    return items;
  };
  view.dom.addEventListener("contextmenu", (e) => {
    if (e.shiftKey) return;
    e.preventDefault();
    const target = /** @type {Element} */ (e.target);
    const tile = target.closest?.(".cm-image, .cm-grid-tile") ?? null;
    const gridEl = target.closest?.(".cm-grid") ?? null;
    const sel = view.state.selection.main;
    const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
    // As in a text field: the caret goes where the click is, unless that is inside the selection.
    if (!tile && !gridEl && pos != null && (sel.empty || pos < sel.from || pos > sel.to)) {
      view.dispatch({ selection: { anchor: pos } });
    }
    openMenu(contextItems(tile, gridEl, e.clientX, e.clientY), e.clientX, e.clientY, () => view.focus());
  });
  return view;
}

// Page editor for the home page: blocks in a side panel go into areas by dragging (or by
// choosing a block and then an area); blocks on the page move, change settings or go away.
const BLOCKS = {
  forum_list: [tr("Forum list"), "folder"],
  home_latest: [tr("Latest topics"), "message-square"],
  home_stats: [tr("Statistics"), "table"],
  latest_statuses: [tr("Latest status updates"), "quote"],
  upcoming_events: [tr("Upcoming events"), "clock"],
  calendar: [tr("Calendar"), "calendar"],
  stats: [tr("Totals, one line"), "table"],
  latest_topics: [tr("Latest topics, simple list"), "list"],
  top_posters: [tr("Top posters"), "trophy"],
  online_users: [tr("Who's online"), "users"],
  tag_cloud: [tr("Tag cloud"), "tag"],
  custom: [tr("Custom content"), "file-text"],
  html: [tr("HTML"), "code"],
  browsing: [tr("Recently browsing"), "eye"],
  suggested_topics: [tr("Suggested topics"), "history"],
};
// Drawn by the page itself, one per page, only on the pages that have the data.
const PAGE_DRAWN = { browsing: ["home", "forum", "topic"], suggested_topics: ["topic"] };
// One of each, and they look after their own title and content.
const SINGLE = new Set(["forum_list", "home_latest", "home_stats"]);
// Nothing to set: these go straight in.
const NO_SETTINGS = new Set([...SINGLE, "browsing", "suggested_topics"]);
const LISTS = new Set(
  "latest_statuses latest_topics top_posters online_users tag_cloud upcoming_events calendar".split(" "),
);

export const chatApp = (target, props) => startChat(target, props, { policy: previewPolicy, iconButton, uploadFile });

export async function layoutEditor() {
  const { confirmSudo } = siteSudo(csrf);
  const api = async (method, path, body, retried = false) => {
    const res = await fetch(`/api/v1/admin/widgets${path}`, {
      method,
      headers: jsonHeaders(),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) {
      const problem = await res.json().catch(() => null);
      // Changing the layout needs the password confirmed again: ask, then send it once more.
      if (problem?.type === "sudo-required" && !retried && (await confirmSudo())) return api(method, path, body, true);
      throw new Error(problem?.detail ?? tr("That did not work."));
    }
    return res.status === 204 ? null : res.json();
  };
  const make = mkc;
  const say = (text) => (statusLine.textContent = text);
  const { widgets, kinds } = await api("GET", "");
  const areas = [...document.querySelectorAll("[data-area]")];
  // The forum list, latest topics and statistics are drawn by the home page itself.
  const onHome = areas.some((a) => a.dataset.area.startsWith("home_"));
  let dragged = null;
  let newKind = null;
  let armed = null;

  const panel = make("aside", "bf-layout-panel");
  panel.setAttribute("aria-label", tr("Edit layout"));
  const done = make("a", "btn primary", tr("Done"));
  // Back to the same page, with its own query, without the editor.
  const back = new URL(location.href);
  back.searchParams.delete("edit_layout");
  done.href = back.pathname + back.search;
  const head = make("div", "bf-layout-head");
  head.append(make("strong", "", tr("Edit layout")), done);
  const statusLine = make(
    "p",
    "small muted",
    tr("Drag a block into a highlighted area, or click it and then choose an area."),
  );
  statusLine.setAttribute("role", "status");
  const list = make("div", "bf-layout-blocks");
  const owners = [...new Set(areas.map((a) => a.dataset.area.split("_")[0]))];
  const drawable = (k) =>
    !PAGE_DRAWN[k] ||
    owners.some((o) => PAGE_DRAWN[k].includes(o) && !widgets.some((w) => w.kind === k && w.area.split("_")[0] === o));
  for (const kind of kinds.filter(
    (k) => BLOCKS[k] && drawable(k) && !(SINGLE.has(k) && (!onHome || widgets.some((w) => w.kind === k))),
  )) {
    const b = iconButton(BLOCKS[kind][1], BLOCKS[kind][0]);
    b.className = "bf-layout-block";
    b.append(make("span", "", BLOCKS[kind][0]));
    b.draggable = true;
    b.addEventListener("dragstart", () => {
      newKind = kind;
      dragged = null;
      slot.textContent = BLOCKS[kind][0];
      slot.style.height = "4rem";
    });
    b.addEventListener("click", () => {
      armed = armed === kind ? null : kind;
      for (const x of list.children) x.classList.toggle("is-armed", x === b && armed === kind);
      document.body.classList.toggle("bf-layout-armed", !!armed);
      say(armed ? tr("Choose where {block} goes.", { block: BLOCKS[kind][0] }) : "");
    });
    list.append(b);
  }
  panel.append(head, statusLine, list);
  document.body.append(panel);
  document.body.classList.add("bf-layout-editing");

  const order = (area) => [...area.querySelectorAll(":scope > [data-widget]")].map((w) => +w.dataset.widget);
  const saveArea = (area) => api("PUT", "/arrange", { area: area.dataset.area, ids: order(area) });
  const fail = (e) => say(e.message);

  // Title, list size, cache time, and the text of a custom block.
  const settings = (kind, w) =>
    new Promise((resolve) => {
      const form = make("form", "stack");
      const field = (label, input) => {
        const l = make("label", "bf-field");
        if (input.tagName !== "SELECT") input.placeholder = " ";
        l.append(input, make("span", "bf-field-label", label));
        form.append(l);
        return input;
      };
      const input = (type, value) => mk("input", { type, value });
      const big = (rows, label, value) => field(label, mk("textarea", { rows, value }));
      const single = NO_SETTINGS.has(kind);
      const title = single ? null : field(tr("Title"), input("text", w?.title ?? BLOCKS[kind]?.[0] ?? ""));
      const limit = LISTS.has(kind) ? field(tr("How many to show"), input("number", w?.config?.limit ?? 5)) : null;
      const text = kind === "custom" ? big(6, tr("Content (Markdown)"), w?.config?.markdown ?? "") : null;
      const html = kind === "html" ? big(8, tr("HTML"), w?.config?.html ?? "") : null;
      const cache = single ? null : field(tr("Refresh every, in seconds"), input("number", w?.cache_seconds ?? 60));
      // Long pages make dragging awkward, and a keyboard cannot drag at all.
      let place = null;
      if (w) {
        place = make("select");
        for (const area of areas) place.append(new Option(area.dataset.areaLabel, area.dataset.area));
        place.value = w.area;
        field(tr("Place"), place);
      }
      const cancel = mk("button", { type: "button", className: "btn", textContent: tr("Cancel") });
      form.append(mk("div", { className: "bf-dialog-actions" }, cancel, make("button", "btn primary", tr("Save"))));
      // Settles once: Save gives the settings; Cancel, Escape and × give null.
      let settled = false;
      const finish = (v) => {
        if (settled) return;
        settled = true;
        box.close();
        resolve(v);
      };
      const box = dialog({ title: BLOCKS[kind]?.[0] ?? kind, body: [form], onClose: () => finish(null) });
      cancel.addEventListener("click", () => finish(null));
      form.addEventListener("submit", (e) => {
        e.preventDefault();
        finish({
          title: title ? title.value : "",
          place: place?.value,
          cache_seconds: cache ? +cache.value || 0 : 0,
          config: {
            ...(w?.config ?? {}),
            ...(limit ? { limit: +limit.value || 5 } : {}),
            ...(text ? { markdown: text.value } : {}),
            ...(html ? { html: html.value } : {}),
          },
        });
      });
      box.show();
    });

  const addBlock = async (kind, area, before) => {
    // A block with nothing to set goes straight in.
    const body = NO_SETTINGS.has(kind) ? { title: "", cache_seconds: 0, config: {} } : await settings(kind);
    if (!body) return;
    try {
      const { id } = await api("POST", "", { kind, area: area.dataset.area, ...body });
      const ids = order(area);
      const at = before ? ids.indexOf(+before.dataset.widget) : ids.length;
      ids.splice(at, 0, id);
      await api("PUT", "/arrange", { area: area.dataset.area, ids });
      location.reload();
    } catch (e) {
      fail(e);
    }
  };

  // Where a dragged block would land, shown live; the other blocks slide out of its way.
  const still = matchMedia("(prefers-reduced-motion: reduce)");
  const slide = (change) => {
    const blocks = [...document.querySelectorAll("[data-widget]")];
    const before = new Map(blocks.map((b) => [b, b.getBoundingClientRect()]));
    change();
    if (still.matches) return;
    for (const b of blocks) {
      const was = before.get(b);
      const now = b.getBoundingClientRect();
      const [x, y] = [was.left - now.left, was.top - now.top];
      if (!x && !y) continue;
      b.animate([{ transform: `translate(${x}px, ${y}px)` }, { transform: "none" }], {
        duration: 160,
        easing: "ease-out",
      });
    }
  };
  const slot = make("div", "bf-layout-slot");
  const showSlot = (area, y) => {
    const others = [...area.querySelectorAll(":scope > [data-widget]")].filter((b) => b !== dragged);
    const next =
      others.find((b) => {
        const r = b.getBoundingClientRect();
        return y < r.top + r.height / 2;
      }) ?? null;
    if (slot.parentElement === area && slot.nextElementSibling === next) return;
    if (!slot.parentElement && dragged) dragged.classList.add("is-lifted");
    slide(() => area.insertBefore(slot, next));
  };
  const clearDrag = () => {
    slot.remove();
    dragged?.classList.remove("is-lifted");
    dragged = null;
    newKind = null;
  };
  document.addEventListener("dragend", clearDrag);

  for (const area of areas) {
    const label = mk("button", { type: "button", className: "bf-layout-area", textContent: area.dataset.areaLabel });
    label.addEventListener("click", () => {
      if (!armed) return;
      addBlock(armed, area);
    });
    area.prepend(label);
    area.addEventListener("dragover", (e) => {
      if (!dragged && !newKind) return;
      e.preventDefault();
      showSlot(area, e.clientY);
    });
    area.addEventListener("drop", async (e) => {
      e.preventDefault();
      const next = slot.nextElementSibling;
      const before = slot.parentElement === area && next?.matches("[data-widget]") ? next : null;
      if (newKind) {
        const kind = newKind;
        clearDrag();
        return addBlock(kind, area, before);
      }
      if (!dragged) return;
      const moving = dragged;
      const from = moving.closest("[data-area]");
      slide(() => {
        area.insertBefore(moving, before);
        clearDrag();
      });
      try {
        await saveArea(area);
        if (from !== area) await saveArea(from);
        say(tr("Saved."));
      } catch (err) {
        fail(err);
      }
    });
  }

  for (const el of document.querySelectorAll("[data-widget]")) {
    const w = widgets.find((x) => x.id === +el.dataset.widget);
    if (!w) continue;
    el.draggable = true;
    el.addEventListener("dragstart", () => {
      dragged = el;
      newKind = null;
      slot.textContent = "";
      slot.style.height = `${Math.min(el.offsetHeight, 240)}px`;
    });
    const bar = make("div", "bf-layout-tools");
    const move = (by) => async () => {
      const sibling = by < 0 ? el.previousElementSibling : el.nextElementSibling;
      if (!sibling?.matches("[data-widget]")) return;
      slide(() => (by < 0 ? sibling.before(el) : sibling.after(el)));
      await saveArea(el.closest("[data-area]")).catch(fail);
    };
    const up = iconButton("arrow-up", tr("Move up"));
    up.addEventListener("click", move(-1));
    const edit = iconButton("settings", tr("Block settings"));
    edit.addEventListener("click", async () => {
      const body = await settings(w.kind, w);
      if (!body) return;
      const { place, ...rest } = body;
      api("PATCH", `/${w.id}`, { ...rest, area: w.area, kind: w.kind })
        .then(async () => {
          if (place && place !== w.area) {
            const target = areas.find((x) => x.dataset.area === place);
            await api("PUT", "/arrange", { area: place, ids: [...order(target), w.id] });
          }
          location.reload();
        })
        .catch(fail);
    });
    const remove = iconButton("trash-2", tr("Remove block"));
    remove.addEventListener("click", () => {
      // Two steps, like the post editor's discard button.
      if (!remove.classList.contains("is-armed")) {
        remove.classList.add("is-armed");
        say(tr("Click the bin again to remove this block."));
        return;
      }
      api("DELETE", `/${w.id}`).then(() => {
        // It goes back to the panel, ready to be added again.
        if (SINGLE.has(w.kind)) return location.reload();
        el.remove();
        say(tr("Removed."));
      }, fail);
    });
    bar.append(iconButton("grip-vertical", tr("Drag to move")), up, edit, remove);
    el.prepend(bar);
  }
}
