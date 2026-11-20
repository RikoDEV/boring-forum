// What a page tells the admin after an action: an error, or a confirmation. Every page shows it
// the same way, through <PageHead> or <Notices>.

export class Status {
  error = $state("");
  msg = $state("");

  clear = () => {
    this.error = this.msg = "";
  };

  /// Shows why a request failed. A dismissed password dialog is not an error worth repeating.
  fail = (e) => {
    this.msg = "";
    if (!e?.sudo) this.error = e?.message || String(e);
  };

  /// Runs an action: clears the last message, and shows `ok` (text, or a function of the result)
  /// when it succeeds or the error when it fails. Resolves to the result, or undefined on failure.
  run = async (action, ok) => {
    this.clear();
    try {
      const result = await action();
      if (ok) this.msg = typeof ok === "function" ? ok(result) : ok;
      return result;
    } catch (e) {
      this.fail(e);
    }
  };
}
