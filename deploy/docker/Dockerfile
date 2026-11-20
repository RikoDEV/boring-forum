# syntax=docker/dockerfile:1
# Build context: repository root. `DOCKER_BUILDKIT=1 docker build -f deploy/docker/Dockerfile .`
#
# Base images are pinned by digest so a rebuild is the same toolchain; the tag after each
# `FROM` is only for humans. To update: `docker buildx imagetools inspect <tag>`, copy the
# top-level Digest, and review the release notes of the image. Digests recorded 2026-11-16.

# node:26-alpine
FROM node:26-alpine@sha256:6bd9ab43c0ec0190ca7b2d193906cb8b7edf6e7f4afa43ddeccd6452710bffb5 AS web
WORKDIR /src/web
RUN npm install -g pnpm@11
# Dependencies first, so the pnpm store layer is reused until a lockfile changes.
COPY web/ ./
COPY themes/ ../themes/
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile && pnpm --filter @bf/islands build && pnpm --filter @bf/admin build && node scripts/size-check.mjs

# rust:1-alpine
FROM rust:1-alpine@sha256:2cccd8b212b37dad15a555589c1b6787da4ab913fe24d8a9b2d99d3498869409 AS server
RUN apk add --no-cache musl-dev ca-certificates
WORKDIR /src
COPY . .
COPY --from=web /src/web/islands/dist web/islands/dist
# BuildKit cache mounts keep the registry, git checkouts and incremental build state between
# builds, so only changed crates recompile. The binary is copied out because the target
# directory is not part of the image layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p bf-server --bin boring-forum \
    && cp target/release/boring-forum /boring-forum
# A named volume takes the owner of the directory it is mounted over.
RUN mkdir -p /data/uploads /data/backups && chown -R 65534:65534 /data

# The importer: `docker compose --profile import run --rm importer <source> <backup>` (see
# deploy/importer/import.sh). Not the default target: it carries throwaway Postgres and MariaDB
# servers to restore a forum's backup into, Node for the exporters, and the forum binary.
FROM node:26-alpine@sha256:6bd9ab43c0ec0190ca7b2d193906cb8b7edf6e7f4afa43ddeccd6452710bffb5 AS importer
RUN apk add --no-cache bash tar gzip su-exec postgresql mariadb mariadb-client ca-certificates     && mkdir -p /opt/importers && cd /opt/importers && npm install --no-audit --no-fund pg mysql2
COPY tools/importers/*.mjs /opt/importers/
COPY deploy/importer/import.sh /usr/local/bin/import-forum
COPY --from=server /boring-forum /boring-forum
RUN chmod +x /usr/local/bin/import-forum
WORKDIR /opt/importers
ENTRYPOINT ["/usr/local/bin/import-forum"]

# Nothing but the binary, CA certificates and the admin bundle: no shell, no package manager.
FROM scratch
COPY --from=server /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=server /boring-forum /boring-forum
COPY --from=web /src/web/admin/build /admin
COPY --from=server --chown=65534:65534 /data /data
ENV BF_BIND=0.0.0.0:8080 BF_ADMIN_DIR=/admin BF_LOG_JSON=true
EXPOSE 8080
# Non-root (nobody). Compose also makes the root filesystem read-only.
USER 65534:65534
# The image has no curl; the binary probes its own /readyz.
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 CMD ["/boring-forum", "healthcheck"]
# SIGTERM starts a drain (readiness goes 503, then in-flight requests finish): exec form, so
# the server is PID 1 and receives the signal.
STOPSIGNAL SIGTERM
ENTRYPOINT ["/boring-forum"]
