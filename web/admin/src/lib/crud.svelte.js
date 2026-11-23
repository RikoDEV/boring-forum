// The add/edit/delete flow every list page repeats: a draft record held in a dialog, written with
// POST (new, id 0) or PUT (existing), then the list is loaded again. A failure shows on `status`.
// Pages keep their own list, their own body shaping and their own confirmations.

import { del, post, put } from "./api.js";
import { Status } from "./status.svelte.js";

export class Crud {
  /// The record in the open dialog, or null.
  draft = $state(null);
  status = new Status();

  /// `path` is the collection ("/ads"), `blank` makes a new record, `load` refreshes the list.
  constructor({ path, blank, load }) {
    this.path = path;
    this.blank = blank;
    this.reload = load;
  }

  /// Opens the dialog on `item` (already shaped for the form), or on a blank record.
  open = (item) => {
    this.status.clear();
    this.draft = item ?? this.blank();
  };

  close = () => {
    this.draft = null;
  };

  /// Writes `body` for the open record; `ok` is the confirmation text to show afterwards.
  save = async (body, ok) => {
    const id = this.draft.id;
    try {
      if (id) await put(`${this.path}/${id}`, body);
      else await post(this.path, body);
      this.draft = null;
      if (ok) this.status.msg = ok;
      await this.reload();
    } catch (e) {
      this.status.fail(e);
    }
  };

  /// Deletes record `id` (the caller has asked the admin to confirm); `ok` is the confirmation.
  remove = async (id, ok) => {
    try {
      await del(`${this.path}/${id}`);
      this.draft = null;
      if (ok) this.status.msg = ok;
      await this.reload();
    } catch (e) {
      this.status.fail(e);
    }
  };
}
