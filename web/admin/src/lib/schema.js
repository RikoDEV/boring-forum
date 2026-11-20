// Typed forms a plugin declares (its settings.json, [[admin_pages]], and the settings of a
// [[widgets]] kind), as the server describes them in crates/bf-server/src/platform/plugins/forms.rs.
// A field is {key, type, label, label_key, help, help_key, default, options, min, max, rows}.
import { locale } from "./i18n.js";

/** The text for `key` in the panel's language from a plugin's `locales` ({code: {key: text}}), else English, else `fallback`. */
export function localized(locales, key, fallback = "") {
  if (!key) return fallback;
  const lang = locale();
  return locales?.[lang]?.[key] ?? locales?.[lang.split("-")[0]]?.[key] ?? locales?.en?.[key] ?? fallback;
}

const KINDS = { boolean: "switch", switch: "switch", integer: "number", number: "number", textarea: "textarea" };

/** A JSON Schema property (or an already normalised field) as a field, or null. */
function field(key, v) {
  const options = (v.options ?? v.enum ?? []).map((o) =>
    typeof o === "string"
      ? { value: o, label: v.enum_labels?.[o] ?? o, label_key: v.enum_label_keys?.[o] ?? null }
      : { value: o.value, label: o.label ?? o.value, label_key: o.label_key ?? null },
  );
  const format = { uri: "url", url: "url", email: "email", date: "date", color: "text", textarea: "textarea" }[v.format];
  const declared = v.type ?? "text";
  const type = options.length ? "select" : (KINDS[declared] ?? (declared === "string" || declared === "text" ? (format ?? "text") : (["url", "email", "date"].includes(declared) ? declared : declared === "color" ? "text" : null)));
  if (!type) return null;
  return {
    key,
    type,
    label: v.label ?? v.title ?? key,
    label_key: v.label_key ?? v.title_key ?? null,
    help: v.help ?? v.description ?? "",
    help_key: v.help_key ?? v.description_key ?? null,
    default: v.default ?? null,
    options,
    min: v.min ?? v.minimum ?? null,
    max: v.max ?? v.maximum ?? null,
    rows: v.rows ?? (type === "textarea" ? 4 : 0),
  };
}

/** The fields of a plugin's settings.json, in either style. */
export function fieldsFromSettings(schema) {
  if (Array.isArray(schema?.fields)) return schema.fields.map((f) => field(f.key, f)).filter(Boolean);
  return Object.entries(schema?.properties ?? {})
    .map(([k, v]) => field(k, v))
    .filter(Boolean);
}

/** Saved values with each field's default under them. */
export function withDefaults(fields, saved) {
  const out = { ...(saved ?? {}) };
  for (const f of fields) if (out[f.key] === undefined) out[f.key] = f.default ?? (f.type === "switch" ? false : f.type === "number" ? "" : "");
  return out;
}
