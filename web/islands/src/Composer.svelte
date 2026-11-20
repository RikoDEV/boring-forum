<script>
  import { tr } from "./dom.js";
  // Upgrades the textarea before this island to the CodeMirror composer. The editor bundle
  // loads when the form nears the viewport, so the toolbar is there before the first click
  // and readers who stay at the top never download it. "Quote" links insert a quote here.
  let { editorUrl, draft } = $props();
  let anchor;
  let picked = $state(null);
  let load = null;
  let form = null;

  // Appends a quote of a post (or just `selected` text from it) to the editor.
  async function insertQuote(id, selected) {
    const res = await fetch(`/api/v1/posts/${id}/raw`).catch(() => null);
    if (!res?.ok) return;
    const p = await res.json();
    // Forms submit CRLF; CodeMirror counts it as one character, which would misplace the cursor.
    const quoted = (selected ?? p.raw)
      .replace(/\r\n?/g, "\n")
      .split("\n")
      .map((l) => `> ${l}`)
      .join("\n");
    const text = `${quoted}\n>\n> — ${p.author ? `@${p.author}` : tr("a former member")}, [${tr("post #{n}", { n: p.post_number })}](${p.url})\n\n`;
    const view = await load();
    const end = view.state.doc.length;
    const sep = end && !view.state.doc.toString().endsWith("\n\n") ? "\n\n" : "";
    view.dispatch({
      changes: { from: end, insert: sep + text },
      selection: { anchor: end + sep.length + text.length },
    });
    view.focus();
    form?.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  async function insertPicked() {
    const { id, text } = picked;
    picked = null;
    document.getSelection()?.removeAllRanges();
    await insertQuote(id, text);
  }

  $effect(() => {
    const textarea = anchor?.closest("form")?.querySelector("textarea");
    if (!textarea) return;
    let editor = null;
    load = () => (editor ??= import(/* @vite-ignore */ editorUrl).then((m) => m.mount(textarea, { draft })));
    textarea.addEventListener("focus", () => load().then((v) => v.focus()), { once: true });
    const near = new IntersectionObserver(
      (seen) => {
        if (seen.some((e) => e.isIntersecting)) {
          near.disconnect();
          load();
        }
      },
      { rootMargin: "300px" },
    );
    near.observe(textarea);

    form = textarea.closest("form");
    const replyField = form?.querySelector('input[name="reply_to"]');
    const replyNote = form?.querySelector(".replying");

    const onQuote = async (e) => {
      const reply = e.target.closest?.("[data-reply]");
      if (reply && replyField) {
        e.preventDefault();
        replyField.value = reply.dataset.reply;
        if (replyNote) {
          replyNote.hidden = false;
          replyNote.textContent = `${tr("Replying to #{n}", { n: reply.dataset.reply })} `;
          const cancel = document.createElement("button");
          cancel.type = "button";
          cancel.className = "link";
          cancel.textContent = `(${tr("cancel")})`;
          cancel.onclick = () => {
            replyField.value = "";
            replyNote.hidden = true;
          };
          replyNote.append(cancel);
        }
        (await load()).focus();
        form.scrollIntoView({ behavior: "smooth", block: "center" });
        return;
      }
      const link = e.target.closest?.("[data-quote]");
      if (!link) return;
      e.preventDefault();
      await insertQuote(link.dataset.quote);
    };
    // Floating "Quote" button over text selected inside a post.
    const onSelect = () => {
      const sel = document.getSelection();
      const text = sel?.toString().trim();
      const post = sel?.anchorNode?.parentElement?.closest(".post");
      const id = post?.querySelector("[data-quote]")?.dataset.quote;
      if (!text || !id || !post.querySelector(".cooked")?.contains(sel.anchorNode)) {
        picked = null;
        return;
      }
      const r = sel.getRangeAt(0).getBoundingClientRect();
      picked = { id, text, top: window.scrollY + r.top - 40, left: window.scrollX + r.left };
    };
    document.addEventListener("mouseup", onSelect);
    document.addEventListener("keyup", onSelect);
    document.addEventListener("click", onQuote);
    return () => {
      document.removeEventListener("click", onQuote);
      document.removeEventListener("mouseup", onSelect);
      document.removeEventListener("keyup", onSelect);
    };
  });
</script>

<span bind:this={anchor} hidden></span>
{#if picked}
  <button
    type="button"
    class="btn primary quote-float"
    style:top="{picked.top}px"
    style:left="{picked.left}px"
    onmousedown={(e) => e.preventDefault()}
    onclick={insertPicked}>{tr("Quote")}</button
  >
{/if}
