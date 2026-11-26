// Settings being edited on a page: loading them, what has changed, saving and discarding. The
// Settings page and every feature page that has settings of its own use this one, with the same
// controls (SettingRows) and the same bar for unsaved changes (SaveBar).

import { onMount } from "svelte";
import { SvelteMap } from "svelte/reactivity";
import { api, get, ApiError, uploadFile } from "./api.js";
import { tn, tr } from "./i18n.js";
import { humanize } from "./settings-meta.js";

// A key or secret arrives without its value, only with `set`: whether one is stored. Typing
// replaces it, leaving the field empty keeps it, and `clear` removes it.
export const isSecret = (s) => "set" in s;
const value = (s) => (s.type === "int" ? Number(s.draft) : s.draft);

export class SettingsForm {
  /** @type {any[]} */
  settings = $state([]);
  loaded = $state(false);
  saving = $state(false);
  /** For the group and forum pickers. */
  groupList = $state([]);
  nodeList = $state([]);
  /** Changes with every save: what shows the saved look draws itself again. */
  stamp = $state(Date.now());
  byKey = $derived(new SvelteMap(this.settings.map((s) => [s.key, s])));
  dirty = $derived(this.settings.filter((s) => this.isDirty(s)));

  /**
   * Call while a component is being set up: it loads the settings `wanted` accepts, and warns
   * before the browser leaves with changes unsaved.
   * @param {import("./status.svelte.js").Status} status
   * @param {(key: string) => boolean} wanted
   */
  constructor(status, wanted) {
    this.status = status;
    this.wanted = wanted;
    onMount(() => {
      this.load();
      const warn = (e) => {
        if (this.dirty.length) e.preventDefault();
      };
      addEventListener("beforeunload", warn);
      return () => removeEventListener("beforeunload", warn);
    });
  }

  isDirty = (s) => s.clear || value(s) !== s.value;

  async load() {
    Promise.all([get("/groups"), get("/tree")]).then(
      ([groups, tree]) => {
        this.groupList = groups;
        this.nodeList = tree.nodes ?? [];
      },
      () => {},
    );
    try {
      const data = await get("/settings");
      this.settings = data.filter((s) => this.wanted(s.key)).map((s) => ({ ...s, draft: s.value, clear: false }));
    } catch (e) {
      this.status.fail(e);
    }
    this.loaded = true;
  }

  save = async () => {
    this.status.clear();
    this.saving = true;
    const failures = [];
    let done = 0;
    for (const s of [...this.dirty]) {
      try {
        if (isSecret(s)) {
          // The value is not kept here either: the field goes back to "saved" or empty.
          const typed = s.draft;
          await api("PUT", `/settings/${s.key}`, typed ? { value: typed } : { value: "", clear: true });
          s.set = Boolean(typed);
          s.draft = "";
          s.clear = false;
        } else {
          await api("PUT", `/settings/${s.key}`, { value: value(s) });
          s.value = value(s);
        }
        done += 1;
        if (s.key === "site_name") window.dispatchEvent(new CustomEvent("bf-site-name", { detail: s.value }));
      } catch (e) {
        // The password dialog was dismissed: stop, rather than ask again for every setting.
        if (e instanceof ApiError && e.sudo) {
          failures.push(tr("Not saved: confirm your password to change settings."));
          break;
        }
        failures.push(`${humanize(s.key)}: ${e.message}`);
      }
    }
    this.saving = false;
    if (done) this.stamp = Date.now();
    if (failures.length) this.status.error = failures.join(" · ");
    if (done) this.status.msg = tn("admin.count.settings_saved", done);
  };

  discard = () => {
    for (const s of [...this.dirty]) {
      s.draft = s.value;
      s.clear = false;
    }
    this.status.clear();
  };

  // A picture goes to the uploads first; the setting then holds its path, saved with the rest.
  uploadImage = async (s, e) => {
    const file = e.currentTarget.files[0];
    e.currentTarget.value = "";
    if (!file) return;
    this.status.clear();
    try {
      s.draft = (await uploadFile(file)).url.split("#")[0];
    } catch (err) {
      this.status.fail(err);
    }
  };
}
