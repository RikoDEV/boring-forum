// Sudo mode in the admin panel: when a write needs the password confirmed again, a dialog asks
// for it over the page, and the write is retried. Nothing on the page is lost. The dialog itself
// is shared with the forum's layout editor (islands/src/sudo.js); the text comes from here.

import { csrf } from "./csrf.js";
import { tr } from "./i18n.js";
import { createSudo } from "../../../islands/src/sudo.js";

/** Resolves true once the password is confirmed, false if the dialog was dismissed. */
export const { confirmSudo } = createSudo({
  csrf,
  text: () => ({
    code: tr("Authenticator code"),
    password: tr("Password"),
    confirm: tr("Confirm"),
    cancel: tr("Cancel"),
    none: tr("This account signs in without a password. Sign out and sign in again, then try once more."),
    lead: tr("For your security, confirm it's you before changing this. You won't be asked again for a while."),
    title: tr("Confirm it's you"),
    checking: tr("Checking…"),
    failed: tr("That did not work. Try again."),
    offline: tr("Could not reach the server. Try again."),
  }),
});
