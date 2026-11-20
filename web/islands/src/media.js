// Profile pictures: a crop dialog for the avatar, drag-to-position for the cover. Both work on
// canvases, because the CSP allows neither blob: images nor inline styles. Without JS the plain
// forms still post the file as it is.
import { dialog } from "./dialog.js";
import { csrf, el, formParams, icon, toast, tr } from "./dom.js";

const AVATAR_SIZE = 512;
const COVER_WIDTH = 2400;
const REPOSITION = "bf-cover-reposition";

async function send(form, file, name) {
  const body = new FormData(form);
  body.set("file", file, name);
  form.classList.add("is-busy");
  const res = await fetch(form.action, { method: "POST", body }).catch(() => null);
  form.classList.remove("is-busy");
  if (res?.ok) return true;
  const data = await res?.json().catch(() => null);
  toast(data?.detail || data?.title || tr("The upload failed. Please try again."), true);
  return false;
}

// The avatar address is cached for a few minutes; without this the old picture stays in this
// browser until then.
const refreshAvatar = (name) => fetch(`/avatar/${encodeURIComponent(name)}.svg`, { cache: "reload" }).catch(() => {});

const blob = (canvas, type, quality) => new Promise((ok) => canvas.toBlob(ok, type, quality));

function cropDialog(bitmap) {
  return new Promise((done) => {
    const VIEW = 320;
    const canvas = el("canvas", { width: VIEW, height: VIEW, class: "bf-crop-canvas" });
    const ctx = canvas.getContext("2d");
    const min = VIEW / Math.min(bitmap.width, bitmap.height);
    let zoom = 1;
    let x = (VIEW - bitmap.width * min) / 2;
    let y = (VIEW - bitmap.height * min) / 2;
    const clamp = () => {
      const s = min * zoom;
      x = Math.min(0, Math.max(VIEW - bitmap.width * s, x));
      y = Math.min(0, Math.max(VIEW - bitmap.height * s, y));
    };
    const paint = () => {
      clamp();
      const s = min * zoom;
      ctx.clearRect(0, 0, VIEW, VIEW);
      ctx.drawImage(bitmap, x, y, bitmap.width * s, bitmap.height * s);
    };
    const setZoom = (next) => {
      const z = Math.min(4, Math.max(1, next));
      // Keep the middle of the view where it is.
      x = VIEW / 2 - ((VIEW / 2 - x) * z) / zoom;
      y = VIEW / 2 - ((VIEW / 2 - y) * z) / zoom;
      zoom = z;
      paint();
    };
    let drag = null;
    canvas.addEventListener("pointerdown", (e) => {
      canvas.setPointerCapture(e.pointerId);
      drag = { px: e.clientX, py: e.clientY, x, y };
    });
    canvas.addEventListener("pointermove", (e) => {
      if (!drag) return;
      x = drag.x + e.clientX - drag.px;
      y = drag.y + e.clientY - drag.py;
      paint();
    });
    canvas.addEventListener("pointerup", () => (drag = null));
    const slider = el("input", { type: "range", min: 1, max: 4, step: 0.01, value: 1, "aria-label": tr("Zoom") });
    slider.addEventListener("input", () => setZoom(+slider.value));
    canvas.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        setZoom(zoom - e.deltaY / 500);
        slider.value = String(zoom);
      },
      { passive: false },
    );
    // Settles once: Save gives the picture; Cancel, Escape and the close button give null.
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      box.close();
      done(value);
    };
    const cancel = el("button", { type: "button", class: "btn" }, tr("Cancel"));
    cancel.addEventListener("click", () => finish(null));
    const save = el("button", { type: "button", class: "btn primary" }, tr("Save picture"));
    save.addEventListener("click", async () => {
      const out = el("canvas", { width: AVATAR_SIZE, height: AVATAR_SIZE });
      const f = AVATAR_SIZE / VIEW;
      const s = min * zoom * f;
      out.getContext("2d").drawImage(bitmap, x * f, y * f, bitmap.width * s, bitmap.height * s);
      finish(await blob(out, "image/webp", 0.9));
    });
    const stage = el("div", { class: "bf-crop-stage" });
    stage.append(canvas);
    const zoomRow = el("div", { class: "bf-crop-zoom" });
    zoomRow.append(icon("search"), slider);
    // A click beside it would lose the framing just set up: only the buttons and Escape close it.
    const box = dialog({
      title: tr("Crop your picture"),
      className: "bf-crop",
      body: [el("p", { class: "muted small" }, tr("Drag to move, scroll or use the slider to zoom.")), stage, zoomRow],
      actions: [cancel, save],
      backdrop: false,
      onClose: () => finish(null),
    });
    paint();
    box.show();
  });
}

function avatar(form) {
  const input = form.querySelector('input[type="file"]');
  const name = form.dataset.username;
  input.addEventListener("change", async () => {
    const file = input.files[0];
    input.value = "";
    if (!file) return;
    // GIFs keep their animation, so they go up as they are.
    if (file.type === "image/gif") {
      if (await send(form, file, file.name)) {
        await refreshAvatar(name);
        location.reload();
      }
      return;
    }
    const bitmap = await createImageBitmap(file).catch(() => null);
    if (!bitmap) return toast(tr("That file could not be read as a picture."), true);
    const cropped = await cropDialog(bitmap);
    if (cropped && (await send(form, cropped, "avatar.webp"))) {
      await refreshAvatar(name);
      location.reload();
    }
  });
}

function cover(form) {
  const input = form.querySelector('input[type="file"]');
  input.addEventListener("change", async () => {
    const file = input.files[0];
    input.value = "";
    if (!file) return;
    const bitmap = await createImageBitmap(file).catch(() => null);
    if (!bitmap) return toast(tr("That file could not be read as a picture."), true);
    const scale = Math.min(1, COVER_WIDTH / bitmap.width);
    const canvas = el("canvas", { width: Math.round(bitmap.width * scale), height: Math.round(bitmap.height * scale) });
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    if (await send(form, await blob(canvas, "image/jpeg", 0.85), "cover.jpg")) {
      // A new picture starts in position mode, once the page shows it.
      try {
        sessionStorage.setItem(REPOSITION, "1");
      } catch {
        // Storage blocked: the Reposition button still works.
      }
      location.reload();
    }
  });
}

function reposition(wrap) {
  const img = wrap.querySelector("img.cover");
  const token = csrf();
  const start = async () => {
    await img.decode().catch(() => {});
    const box = img.getBoundingClientRect();
    const spare = (box.width * img.naturalHeight) / img.naturalWidth - box.height;
    if (spare <= 1) return toast(tr("This picture already fits: there is nothing to move."));
    const before = +img.dataset.y;
    let now = before;
    wrap.classList.add("is-positioning");
    const bar = el("div", { class: "bf-cover-bar" });
    const hint = el("span", {}, tr("Drag the picture to position it"));
    const cancel = el("button", { type: "button", class: "btn" }, tr("Cancel"));
    const save = el("button", { type: "button", class: "btn primary" }, tr("Save position"));
    bar.append(hint, cancel, save);
    wrap.append(bar);
    let drag = null;
    const down = (e) => {
      img.setPointerCapture(e.pointerId);
      drag = { py: e.clientY, y: now };
    };
    const move = (e) => {
      if (!drag) return;
      now = Math.min(100, Math.max(0, drag.y - ((e.clientY - drag.py) / spare) * 100));
      img.style.objectPosition = `50% ${now}%`;
    };
    const up = () => (drag = null);
    img.addEventListener("pointerdown", down);
    img.addEventListener("pointermove", move);
    img.addEventListener("pointerup", up);
    const stop = () => {
      img.removeEventListener("pointerdown", down);
      img.removeEventListener("pointermove", move);
      img.removeEventListener("pointerup", up);
      wrap.classList.remove("is-positioning");
      bar.remove();
    };
    cancel.addEventListener("click", () => {
      img.style.objectPosition = `50% ${before}%`;
      stop();
    });
    save.addEventListener("click", async () => {
      const y = Math.round(now);
      const res = await fetch("/settings/cover/position", {
        method: "POST",
        body: new URLSearchParams({ csrf: token, y: String(y) }),
      }).catch(() => null);
      if (!res?.ok) return toast(tr("The position could not be saved."), true);
      img.dataset.y = y;
      stop();
    });
  };
  wrap.querySelector("[data-cover-reposition]")?.addEventListener("click", start);
  let pending = false;
  try {
    pending = sessionStorage.getItem(REPOSITION) === "1";
    sessionStorage.removeItem(REPOSITION);
  } catch {
    // Storage blocked: nothing was left for us.
  }
  if (pending) start();
}

export function profileMedia() {
  // Everyone sees the saved position; the stylesheet alone cannot read it from the page.
  for (const img of document.querySelectorAll("img.cover[data-y]")) img.style.objectPosition = `50% ${img.dataset.y}%`;
  const a = document.querySelector("form.bf-avatar-edit");
  if (a) avatar(a);
  const c = document.querySelector("form.bf-cover-upload");
  if (c) cover(c);
  const wrap = document.querySelector(".bf-cover-wrap");
  if (wrap?.querySelector("[data-cover-reposition]")) reposition(wrap);
  // The cached avatar address has to change with the picture here too.
  const remove = /** @type {HTMLFormElement | null} */ (document.querySelector("form.bf-avatar-remove"));
  remove?.addEventListener("submit", async (e) => {
    e.preventDefault();
    const res = await fetch(remove.action, { method: "POST", body: formParams(remove) }).catch(() => null);
    if (!res?.ok) return toast(tr("The picture could not be removed."), true);
    await refreshAvatar(a.dataset.username);
    location.reload();
  });
}
