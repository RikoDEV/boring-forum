import { composing, tr } from "./dom.js";

// Tag input: chips with suggestions over a plain comma-separated field, which stays in the
// form (hidden), so the server sees what it does without JavaScript.
const make = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
};

function upgrade(field, people) {
  if (field.dataset.tagReady) return;
  field.dataset.tagReady = "1";
  // `data-max="1"` makes it a single pick: the chip replaces what was there, and the box closes.
  const max = Number(field.dataset.max) || 0;
  let tags = field.value
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);
  let options = [];
  let active = -1;
  let seq = 0;

  const box = make("div", "bf-tags");
  const input = make("input");
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-label", field.getAttribute("aria-label") || (people ? tr("Names") : tr("Tags")));
  input.autocomplete = "off";
  input.placeholder = field.placeholder || tr("Add tags…");
  if (field.id) {
    input.id = field.id;
    field.removeAttribute("id"); // the page's <label for> now points at the visible input
  }
  const list = make("ul", "bf-tags-list");
  list.setAttribute("role", "listbox");
  list.hidden = true;

  const close = () => {
    list.hidden = true;
    active = -1;
    input.setAttribute("aria-expanded", "false");
  };
  const paintList = () => {
    list.replaceChildren(
      ...options.map((name, i) => {
        const li = make("li", i === active ? "is-active" : "", people ? undefined : name);
        if (people) {
          const img = make("img", "avatar");
          img.src = `/avatar/${encodeURIComponent(name)}.svg`;
          img.width = img.height = 20;
          img.alt = "";
          li.append(img, ` ${name}`);
        }
        li.setAttribute("role", "option");
        li.setAttribute("aria-selected", String(i === active));
        // Before blur, or the list would close first.
        li.addEventListener("mousedown", (e) => {
          e.preventDefault();
          add(name);
        });
        return li;
      }),
    );
    list.hidden = !options.length;
    input.setAttribute("aria-expanded", String(!list.hidden));
  };
  const paint = () => {
    field.value = tags.join(", ");
    box.replaceChildren(
      ...tags.map((name) => {
        const chip = make("span", "bf-tag-chip", name);
        const x = make("button", "", "×");
        x.type = "button";
        x.setAttribute("aria-label", tr("Remove {name}", { name }));
        x.addEventListener("click", () => {
          tags = tags.filter((t) => t !== name);
          paint();
          input.focus();
        });
        chip.append(x);
        return chip;
      }),
      input,
      list,
    );
    input.placeholder = tags.length ? "" : field.placeholder || tr("Add tags…");
    // A full single pick shows only its chip; removing it brings the input back.
    input.hidden = Boolean(max) && tags.length >= max;
  };
  const suggest = async () => {
    const mine = ++seq;
    const q = input.value.trim();
    if (people && !q) {
      options = [];
      return paintList();
    }
    const res = await fetch(`/api/v1/autocomplete?kind=${people ? "user" : "tag"}&q=${encodeURIComponent(q)}`).catch(
      () => null,
    );
    if (mine !== seq || !res?.ok || document.activeElement !== input) return;
    const lower = tags.map((t) => t.toLowerCase());
    options = (await res.json()).map((r) => r.label).filter((n) => !lower.includes(n.toLowerCase()));
    active = -1;
    paintList();
  };
  function add(name) {
    const clean = name.trim();
    if (max === 1 && clean) tags = [clean];
    else if (clean && !tags.some((t) => t.toLowerCase() === clean.toLowerCase()) && !(max && tags.length >= max))
      tags.push(clean);
    input.value = "";
    paint();
    input.focus();
    suggest();
  }
  let timer;
  input.addEventListener("focus", suggest);
  input.addEventListener("blur", () => {
    if (input.value.trim()) add(input.value);
    close();
  });
  input.addEventListener("input", () => {
    if (input.value.includes(",")) {
      for (const part of input.value.split(",")) add(part);
      return;
    }
    clearTimeout(timer);
    timer = setTimeout(suggest, 150);
  });
  input.addEventListener("keydown", (e) => {
    if (composing(e)) return; // an IME is still choosing its text
    if ((e.key === "ArrowDown" || e.key === "ArrowUp") && options.length) {
      e.preventDefault();
      active = (active + (e.key === "ArrowDown" ? 1 : -1) + options.length) % options.length;
      paintList();
    } else if (e.key === "Enter" && (input.value.trim() || active >= 0)) {
      e.preventDefault(); // adds a tag instead of submitting the form
      add(active >= 0 ? options[active] : input.value);
    } else if (e.key === "Backspace" && !input.value && tags.length) {
      tags.pop();
      paint();
      input.focus();
      suggest();
    } else if (e.key === "Escape" && !list.hidden) {
      e.stopPropagation();
      close();
    }
  });
  box.addEventListener("click", (e) => {
    if (e.target === box) input.focus();
  });
  field.type = "hidden";
  field.removeAttribute("required");
  field.after(box);
  paint();
}

export const tagInputs = (root = document) => {
  for (const f of root.querySelectorAll("input[data-tag-input]")) upgrade(f, false);
  for (const f of root.querySelectorAll("input[data-people-input]")) upgrade(f, true);
};
