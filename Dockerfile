# =============================================================================
# Seaquel — multi-stage Docker build
#
# Produces a single image that runs both the Node app (SvelteKit + Better Auth
# + storage endpoints) and the Rust DB-execution service as a subprocess.
#
# Build:   docker build -t seaquel .
# Run:     docker run -p 8787:8787 -v seaquel-data:/data seaquel
# =============================================================================

# ---------------------------------------------------------------------------
# Stage 1: Build the Rust binary
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS rust-builder

WORKDIR /build

# Copy only manifests first → Docker layer cache for deps
COPY Cargo.toml Cargo.lock ./
COPY crates/seaquel-db/Cargo.toml crates/seaquel-db/
COPY crates/seaquel-server/Cargo.toml crates/seaquel-server/
COPY crates/seaquel-server/build.rs crates/seaquel-server/

# Stub out source so Cargo can resolve deps without the real code
RUN mkdir -p crates/seaquel-db/src && echo "" > crates/seaquel-db/src/lib.rs \
 && mkdir -p crates/seaquel-server/src && echo "fn main(){}" > crates/seaquel-server/src/main.rs \
 && echo "" > crates/seaquel-server/src/lib.rs \
 && mkdir -p src-tauri/src && echo "" > src-tauri/src/lib.rs && echo "fn main(){}" > src-tauri/src/main.rs

# Pre-fetch + compile deps (cached unless Cargo.toml/lock change)
COPY src-tauri/Cargo.toml src-tauri/
COPY src-tauri/build.rs src-tauri/
RUN cargo build --release -p seaquel-server 2>/dev/null || true

# Now copy real source and build for real. `touch` is load-bearing: the
# stubbed lib.rs/main.rs from the previous step produced cached rlibs in
# /build/target, and `COPY` preserves the source files' original mtimes,
# which can be earlier than those cached rlibs. Cargo then concludes the
# sources haven't changed and links `seaquel-server` against the empty
# stub `seaquel_db` rlib — producing unresolved-import errors for every
# pub item. Bumping mtime forces cargo to recompile the local crates
# (transitive dep rlibs stay cached, which is the point of the split).
COPY crates/ crates/
RUN find crates -name '*.rs' -exec touch {} + \
 && cargo build --release -p seaquel-server \
 && strip target/release/seaquel-server

# ---------------------------------------------------------------------------
# Stage 2: Build the SvelteKit frontend (adapter-node)
# ---------------------------------------------------------------------------
FROM node:22-bookworm-slim AS node-builder

# better-sqlite3 needs python3 + make + g++ for its native build if no
# prebuilt binary is available. bookworm-slim has them via build-essential.
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 make g++ \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy package manifests → npm ci cache layer
COPY package.json package-lock.json ./
RUN npm ci

# Copy source and build the SvelteKit app.
#
# NODE_OPTIONS bumps V8's old-space cap because Vite + adapter-node's Rollup
# pass walks a large dependency graph (monaco-editor, layerchart, apache-arrow,
# mssql, …) and the default ~4 GB heap OOMs partway through. 8 GB is the
# smallest value that completes reliably; raise the Docker engine's container
# memory limit accordingly if a host enforces one.
COPY . .
RUN NODE_OPTIONS="--max-old-space-size=8192" npm run build:web

# Prune to production deps only. better-sqlite3's native .node file is
# already compiled from the `npm ci` above, so the production install
# just removes devDeps without recompiling anything.
RUN npm ci --omit=dev

# ---------------------------------------------------------------------------
# Stage 3: Runtime image
# ---------------------------------------------------------------------------
FROM node:22-bookworm-slim AS runtime

# tini handles PID 1 duties (signal forwarding, zombie reaping).
# libssl3  — tiberius (MSSQL driver) + russh link against OpenSSL at runtime.
# ca-certificates — so outbound TLS (to customer DBs, Postmark, etc.) can
#                   verify certs against the Debian trust store. Not present
#                   in node:22-bookworm-slim by default.
RUN apt-get update && apt-get install -y --no-install-recommends \
    tini \
    libssl3 \
    ca-certificates \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the Rust binary from the Rust build stage
COPY --from=rust-builder /build/target/release/seaquel-server ./seaquel-server

# Copy the SvelteKit build output + server.js entrypoint
COPY --from=node-builder /build/build-web ./build-web
COPY --from=node-builder /build/server.js ./server.js
COPY --from=node-builder /build/package.json ./package.json
# Plain ESM modules shared between server.js and the SvelteKit bundle
# (e.g. tenant-scope helpers). Vite inlines them on the SvelteKit side, but
# server.js loads them at runtime and needs the source files present.
COPY --from=node-builder /build/shared ./shared

# Copy production-only node_modules from the builder (better-sqlite3's native
# .node binary was already compiled there — no build tools needed here).
COPY --from=node-builder /build/node_modules ./node_modules

# ---------------------------------------------------------------------------
# Drop privileges. The `node` user (uid 1000) ships with the base image.
# /data is a named volume in production — chown here so the non-root user
# can create files on first run, and an explicit `chown` on every mount
# isn't required. server.js + seaquel-server only need /app for reads and
# /data for writes; no other FS access is required.
# ---------------------------------------------------------------------------
RUN mkdir -p /data \
 && chown -R node:node /app /data \
 && chmod +x /app/seaquel-server

USER node

# ---------------------------------------------------------------------------
# Environment defaults.
#
# DATA_DIR: persistent storage root. Mount a Docker volume here.
# PORT: the public HTTP port. server.js listens on this.
# SEAQUEL_AUTH_SECRET: REQUIRED in production — a 32+ char random string.
#   Sessions are signed with this; changing it invalidates all sessions.
#   Generate one: openssl rand -hex 32
# SEAQUEL_TRUSTED_ORIGINS: comma-separated origins allowed by Better Auth's
#   CSRF check AND by the `/api/signup` Origin guard. REQUIRED for any
#   non-localhost deployment — without it signup/signin will 403.
# BETTER_AUTH_URL: canonical URL Better Auth uses for absolute links. Set
#   when running behind a reverse proxy that rewrites `Host`.
# SEAQUEL_COOKIE_DOMAIN: set for cross-subdomain cookies, e.g. .seaquel.app
#
# See README.md ("Required environment variables" table) for the full set
# including licensing knobs (SEAQUEL_CONTROL_URL, SEAQUEL_LICENSE_SOFT_TTL,
# SEAQUEL_LICENSE_GRACE_TTL).
# ---------------------------------------------------------------------------
ENV DATA_DIR=/data \
    PORT=8787 \
    NODE_ENV=production

VOLUME /data
EXPOSE 8787

# Liveness probe. Uses Node's built-in fetch so we don't need curl/wget in
# the image. Exits 0 if /health returns 200, nonzero otherwise.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
  CMD node -e "fetch('http://127.0.0.1:' + (process.env.PORT || 8787) + '/health').then(r => process.exit(r.ok ? 0 : 1)).catch(() => process.exit(1))"

# OCI annotations. `revision` is passed in by the publish workflow so the
# image records the exact commit it was built from. The rest stay static.
ARG SEAQUEL_REVISION=unknown
LABEL org.opencontainers.image.title="Seaquel" \
      org.opencontainers.image.description="Self-hostable database client — explore, query, and visualize databases in a browser." \
      org.opencontainers.image.source="https://github.com/webstonehq/seaquel" \
      org.opencontainers.image.url="https://seaquel.app" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.vendor="Webstone Technologies Inc" \
      org.opencontainers.image.revision="${SEAQUEL_REVISION}"

# tini as PID 1 → node server.js (which spawns seaquel-server as a child)
ENTRYPOINT ["tini", "--"]
CMD ["node", "server.js"]
