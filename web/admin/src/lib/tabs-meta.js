// The tabs of the panel's pages, for the search: [page address, tab key, label]. A page that lists
// a tab here opens on it from `#tab=key` (see tabs.svelte.js). The labels are the ones the pages
// show, translated when this module runs, so the search loads it once the language is known.
import { tr } from "#lib/i18n.js";

/** @type {[string, string, string][]} */
export const TABS = [
  ["/chat", "channels", tr("Channels")],
  ["/chat", "rules", tr("Integrations")],
  ["/chat", "flags", tr("Flags")],
  ["/community", "reactions", tr("Reactions")],
  ["/community", "tags", tr("Tag groups")],
  ["/community", "rules", tr("Group rules")],
  ["/community", "news", tr("Announcements")],
  ["/community", "reasons", tr("Report reasons")],
  ["/mail", "log", tr("Log")],
  ["/mail", "templates", tr("Templates")],
  ["/mail", "bad", tr("Bad addresses")],
  ["/mail", "test", tr("Test delivery")],
  ["/tags", "tags", tr("Tags")],
  ["/tags", "emoji", tr("Custom emoji")],
  ["/tags", "invites", tr("Invites")],
  ["/tags", "searches", tr("Top searches")],
  ["/webhooks", "hooks", tr("Webhooks")],
  ["/webhooks", "log", tr("Deliveries")],
  ["/webhooks", "apps", tr("Applications")],
  // The parts of a forum open once one is picked (a theme's open by their own choice of theme).
  ["/forums", "general", tr("General")],
  ["/forums", "icon", tr("Icon")],
  ["/forums", "behaviour", tr("Posting")],
  ["/forums", "permissions", tr("Permissions")],
  ["/forums", "people", tr("Moderators")],
  ["/forums", "seo", tr("SEO")],
  ["/users", "new", tr("New")],
  ["/users", "staff", tr("Staff")],
  ["/users", "pending", tr("Awaiting approval")],
  ["/users", "suspended", tr("Suspended")],
  ["/users", "silenced", tr("Silenced")],
  ["/users", "unverified", tr("Unverified")],
];
