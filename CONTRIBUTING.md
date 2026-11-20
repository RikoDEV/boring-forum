# Contributing

Thanks for helping. This file covers setting up, the checks a change must pass, and the rules
that keep the codebase consistent. Security issues go to [SECURITY.md](SECURITY.md),
not to an issue.

## Repository layout

| Path | What |
|---|---|
| `crates/` | The Rust workspace: `bf-server` (routes, middleware, background jobs), `bf-web` (templates and assets), `bf-db` (queries and the schema), `bf-core` (settings, permissions, translations), `bf-cook` (Markdown and BBCode to safe HTML), `bf-seo`, `bf-realtime`, `bf-plugin-api` (the SDK plugins are built with) and `bf-bench` |
| `web/` | Svelte islands, the SvelteKit admin panel and the generated TypeScript API client |
| `themes/default` | The default theme: layered CSS modules, icons and flags |
| `tests/` | The end-to-end suite (Vitest), curl suites and fixtures |
| `tools/` | The Docker-free dev database, dev seeding and importers for IPS, phpBB and Discourse |
| `scripts/` | Test runners, backups, checks, translations and screenshots |
| `deploy/` | Container images, `compose.yml` and database roles |
| `fuzz/`, `supply-chain/`, `deny.toml` | Fuzz targets, cargo-vet audits and the cargo-deny policy |

## Setting up

- Rust (the exact version is pinned in `rust-toolchain.toml`; rustup installs it), Node 26 and pnpm 11,
  Docker with Compose.
- `cp .env.example .env` and set at least `BF_DATABASE_URL`.
- Database: `docker compose -f deploy/compose.yml up -d --wait db`, or without Docker the
  in-memory PGlite server in `tools/devdb`.
- Front end: `cd web && pnpm install && pnpm build`.
- Run: `cargo run -p bf-server`, then open http://127.0.0.1:8080. The first account registered
  becomes the administrator. `tools/seed-dev.mjs` fills a fresh forum with sample content (its
  header lists the two steps).

## Before you open a pull request

```sh
scripts/check.sh                    # fmt, clippy, tests, translations, unused dependencies, web checks
scripts/check.sh --fast             # the same without the test run
scripts/test.sh                     # end-to-end suite in Docker
```

A change that adds or edits text a person reads also needs `python scripts/i18n.py sync`, the
Polish and Arabic translations (see "No English text in code without a translation" in
[CLAUDE.md](CLAUDE.md)), and `python scripts/i18n.py check` plus `cargo test -p bf-core i18n`.
A change to how a page looks needs a screenshot of it at 1280px and 390px, in dark mode and, for
colours, light mode (`scripts/shot.mjs`); the UI rules in CLAUDE.md (no inline styles, names are
links, dates have their full date on hover) apply to every page.

`scripts/check.sh` runs `cargo nextest` when it is installed (`cargo install cargo-nextest
--locked`), like CI's `quality` workflow, and falls back to `cargo test`. The workflow also
measures line coverage of `bf-core` (minimum set in `quality.yml`), builds the docs with
`-D warnings`, runs `cargo machete`, applies the migrations to an empty database, and runs
mutation testing on the permission code every week.

Individual checks: `cargo run --release -p bf-bench` (server performance budgets), `scripts/query-count.sh` (at most 3 SQL statements per page), `(cd web && node scripts/size-check.mjs)` (JavaScript and CSS size budgets), `cargo test -p bf-cook --test xss_corpus`, `cargo test -p bf-core --test perms_properties`, `scripts/test.sh idor`, and `fuzz/README.md` for fuzzing.

CI also runs `cargo deny`, `cargo audit`, `cargo vet`, semgrep, CodeQL, the XSS corpus, the
permission property tests, the IDOR suite and short fuzzing runs; see `.github/workflows/`.

## Rules

- **SQL**: every value is bound. `AssertSqlSafe` needs a `//` comment directly above it saying
  why the string is constant.
- **Placement**: code goes in the module for its concern, in the matching domain of `bf-db` and
  `bf-server`. A module that needs a vague name (`misc`, `more`, `extras`) is two modules.
- **Migrations**: add a new file; never edit an applied one.
- **Settings**: declare them in `bf_core::settings` with a type, default and description.
- **Errors** leave the server as `application/problem+json`; use `AppError`.
- **Front end**: islands stay small and lazy; styles go in the matching CSS module, using the
  design tokens and logical properties. Check the budgets.
- **Accessibility**: keyboard reachable, labelled, and usable in forced-colour mode.
- **Writing**: comments say why, not what. Plain English in docs and interface text.
- **Commits**: one change per commit, with a message that says what and why, starting with a
  conventional prefix (`feat:`, `fix:`, `security:`, `perf:`, `docs:`, `refactor:`, `test:`, `chore:`).
  semantic-release reads them to choose the next version and write the changelog: `feat` is a
  minor release, `fix`, `perf` and `security` a patch, and `!` after the type (or a
  `BREAKING CHANGE:` footer) a major one; the other types do not release.

## Formatting

`rustfmt.toml` and `.editorconfig` define the style; Prettier formats the theme CSS
(`.prettierrc.json`). Line endings are LF everywhere (`.gitattributes`).

## Licence

By contributing you agree that your contribution is licensed under the
[GNU AGPL v3](LICENSE), like the rest of the project.
