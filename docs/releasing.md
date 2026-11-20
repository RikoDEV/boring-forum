# Releasing


Releases are automatic. Every push to `main` runs `.github/workflows/semantic-release.yml`
([semantic-release](https://semantic-release.gitbook.io/), configured in `.releaserc.json`). It reads
the commit messages since the last tag:

| Commit | Release |
| --- | --- |
| `feat: …` | minor |
| `fix: …`, `perf: …`, `security: …` | patch |
| `feat!: …` or a `BREAKING CHANGE:` footer | major |
| `docs:`, `refactor:`, `test:`, `chore:`, `ci:` | none |

When a commit calls for a release, the workflow sets the version in `Cargo.toml` and `Cargo.lock`
(`scripts/set-version.sh`), writes the notes to `CHANGELOG.md`, commits that as
`chore(release): X.Y.Z`, tags `vX.Y.Z` and creates the GitHub release. It then starts
`.github/workflows/release.yml` for the tag, which builds the server image from the lockfiles,
pushes it to `ghcr.io/rikodev/boring-forum` with CycloneDX SBOMs, a keyless Sigstore signature and
SLSA provenance. That workflow's header shows how to verify an image with `cosign` and
`gh attestation verify`.

## Before the first release

- Tag the version the project is at, so semantic-release continues from it instead of starting at
  1.0.0: `git tag v0.1.0 && git push origin v0.1.0`.
- Allow the `github-actions` bot to push to `main` if it is protected (the release commit).
- Try it without releasing: run the workflow by hand from the Actions tab (it defaults to a dry run).

## Making a release

1. Merge pull requests with conventional commit titles (squash merges use the title).
2. Watch the Semantic release workflow, then the Release workflow it starts. Verify the image as
   its header describes.
3. Migrations run when the new server starts and only go forward, so take a backup first
   (Admin, Backups, or see [Operations](operations.md)) and keep the previous image until the
   new one is healthy (`/readyz`).

## Rollback

Migrations only go forward, so rolling back the image does not roll back the schema. The server
checks that the schema is *at least* what it expects, so the previous image normally runs against
the newer schema as long as the migration was additive (new tables, nullable columns). Decide by
reading the migration files between the two tags:

1. Additive only: redeploy the previous image digest. `/readyz` goes green and traffic returns.
2. A migration dropped or renamed something the old code uses: the old image will fail on those
   queries. Restore the pre-release backup into a fresh database (see [Operations](operations.md#rolling-back-an-upgrade)), point
   `BF_DATABASE_URL` at it and start the old image; writes made since the backup are lost, so
   announce read-only mode first (Admin, Settings, offline mode) if you need to copy them out.
3. Prefer rolling forward with a fix when the migration is destructive; write such migrations in two
   releases (stop using the column, then drop it).

Released images and their SBOMs are immutable by digest (`ghcr.io/rikodev/boring-forum@sha256:...`),
so a rollback target is always one `docker pull` away.

## Reproducible builds

The release image builds from `Cargo.lock` (`--locked`) and the pnpm lockfile (`--frozen-lockfile`)
on base images pinned by digest (see the comment at the top of `deploy/docker/Dockerfile`), with
the Rust toolchain fixed to the version in `rust-toolchain.toml`/the workflow. Two builds of the
same tag use the same inputs; byte-identical output is not guaranteed (embedded paths and
timestamps are not normalised), so compare the SBOM and the provenance attestation, not the image
hash, when checking a rebuild.
