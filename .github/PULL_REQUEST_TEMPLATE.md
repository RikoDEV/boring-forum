<!-- Every pull request carries the security checklist. -->

## What this changes

<!-- One or two sentences. Link the issue it comes from. -->

## Security checklist

Tick each line or say why it does not apply. "Not applicable" is a fine answer; skipping the
list is not, because the point is that someone looked.

- [ ] **Authorization** — new data access goes through a repository function that takes an
      `Actor` and filters inside the query. No "fetch by id without actor". Every new route
      declares its required permission.
- [ ] **Input limits** — request bodies are typed structs with length, size and range limits,
      unknown fields rejected. New list endpoints have a maximum page size.
- [ ] **Output escaping** — no new raw HTML path. Anything rendered unescaped comes from the
      sanitizer or the cook pipeline through `SafeHtml`.
- [ ] **SQL** — parameterized queries only. Any dynamic SQL is built from enum values, not
      from strings, and `AssertSqlSafe` is used only where the input provably cannot be
      attacker-controlled (say where, in a comment).
- [ ] **Rate limits** — new write or expensive endpoints are rate-limited per IP and per user.
- [ ] **Logging** — no password, token, cookie, full email address or post content reaches a log
      line or an error message. Query strings go through `bf_core::log::redact_uri`.
- [ ] **Secrets** — nothing new is stored in plain text that should be encrypted at rest;
      nothing secret is serialized into an API response or a template.
- [ ] **Audit log** — staff and security-relevant actions write an `audit_log` row.
- [ ] **Migrations** — reversible or clearly one-way-and-why; no destructive statement without a
      note in the PR body.
- [ ] **Tests** — a test that fails without this change. Permission changes come with a case in
      the IDOR suite or the permission property tests.

## Two reviewers required?

`.github/CODEOWNERS` requires **two** approvals for changes to authentication, permissions, the
sanitizer and cook pipeline, uploads, the plugin host, and anything cryptographic.

- [ ] This PR touches one of those areas, and I have requested a second reviewer.
- [ ] It does not.

## How this was tested

<!-- Commands run, and what you checked by hand. Say if you did not run the suite. -->
