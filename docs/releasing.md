# Releasing


A release is made by pushing a `vX.Y.Z` tag, and only then. Nothing releases on a push to `main`.
The tag starts `.github/workflows/release.yml`, which checks that the tagged commit is on `main` and
that `Cargo.toml` has the same version, then builds the server image from the lockfiles, pushes it to
`ghcr.io/rikodev/boring-forum` with CycloneDX SBOMs, a keyless Sigstore signature and SLSA
provenance, and creates the GitHub release with generated notes. That workflow's header shows how to
verify an image with `cosign` and `gh attestation verify`.

## Making a release

1. Pick the version. Run the **Next version preview** workflow from the Actions tab to see what the
   conventional commits since the last tag suggest (`feat` minor, `fix`/`perf`/`security` patch,
   `feat!:` or a `BREAKING CHANGE:` footer major; `docs`, `refactor`, `test`, `chore`, `ci` none).
   It only prints; the number is still your decision.
2. Set the version and merge it to `main`:

   ```sh
   scripts/set-version.sh 0.2.0     # Cargo.toml and Cargo.lock
   git commit -am "chore(release): 0.2.0"
   ```

3. Tag that commit once it is on `main`, and push the tag:

   ```sh
   git tag v0.2.0 && git push origin v0.2.0
   ```

4. Watch the Release workflow and verify the image as its header describes. It refuses a tag that is
   not on `main` or does not match `Cargo.toml`; fix that and push a new tag.
5. Migrations run when the new server starts and only go forward, so take a backup first
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
