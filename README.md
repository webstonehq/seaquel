<div align="center">

# Seaquel

**Explore, query, and visualize your databases — all in one app.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Free for personal use](https://img.shields.io/badge/Free-for%20personal%20use-blue.svg)](https://seaquel.app/pricing)
[![Version](https://img.shields.io/github/v/release/webstonehq/seaquel)](https://github.com/webstonehq/seaquel/releases)
[![GitHub Stars](https://img.shields.io/github/stars/webstonehq/seaquel)](https://github.com/webstonehq/seaquel/stargazers)
[![Discord](https://img.shields.io/discord/1452421515164385436?label=Discord&logo=discord&logoColor=white)](https://seaquel.app/discord)
[![Try Demo](https://img.shields.io/badge/Try-Live%20Demo-brightgreen)](https://seaquel.app/demo)

![Seaquel Screenshot](https://seaquel.app/product-screenshot.jpg)

Works with 6 database engines. No account required. Open source, free for personal use.<br>
[Try it in your browser](https://seaquel.app/demo) in seconds.

</div>

## Features

### Query & Edit

- **SQL editor** — Syntax highlighting, formatting, parameter support, and Monaco-based editing
- **Inline result editing** — INSERT, UPDATE, and DELETE rows directly from the results table
- **Visual query builder** — Drag-and-drop canvas for building queries without SQL
- **AI assistant** — Get help writing and understanding SQL queries
- **MCP server** — Let Claude Desktop, Claude Code or another MCP host read your schemas and run read-only queries (desktop)
- **Terminal UI** — Browse, edit and query your saved connections from a terminal with `seaquel-tui`
- **SQL learning sandbox** — Interactive challenges to practice SQL

### Explore & Visualize

- **Schema browser** — Explore tables, columns, indexes, constraints, and more
- **Entity Relationship Diagrams** — Auto-generated ERDs with PNG/SVG export
- **EXPLAIN/ANALYZE visualizer** — Understand query plans with hot path detection
- **Database statistics** — Dashboard with table sizes, row counts, and index usage
- **Data visualization** — Bar, line, pie, and scatter charts from query results

### Collaborate & Share

- **CSV/JSON export** — Export query results to CSV or JSON
- **Query sharing** — Share queries via Git repositories (desktop)
- **Connection import** — Import connections from DBeaver and TablePlus
- **Multi-project** — Organize connections and queries across projects

### Customize

- **Themes** — Light and dark modes with a built-in theme editor
- **Internationalization** — Available in English, Spanish, German, French, Arabic, and Korean
- **Command palette** — Quick access to all actions via keyboard
- **SSH tunneling** — Connect securely through SSH tunnels (desktop)
- **Auto-updates** — Stay current with automatic update notifications

## Comparison with Alternatives

|                       |      Seaquel       |      DBeaver       |     TablePlus      |      DataGrip      |      pgAdmin       |
| --------------------- | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: |
| Open source           | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| Multi-database        | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |        :x:         |
| Browser demo          | :white_check_mark: |        :x:         |        :x:         |        :x:         |        :x:         |
| ERD generation        | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |        :x:         |
| Visual query builder  | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| EXPLAIN visualizer    | :white_check_mark: | :white_check_mark: |        :x:         | :white_check_mark: | :white_check_mark: |
| Free for personal use | :white_check_mark: | :white_check_mark: |        :x:         |        :x:         | :white_check_mark: |
| Lightweight           | :white_check_mark: |        :x:         | :white_check_mark: |        :x:         | :white_check_mark: |

## Installation

### Desktop

Download Seaquel for your platform from [seaquel.app/download](https://seaquel.app/download).

| Platform | Architectures                         |
| -------- | ------------------------------------- |
| macOS    | Intel (x86_64), Apple Silicon (ARM64) |
| Linux    | x86_64, ARM64                         |
| Windows  | x86_64, ARM64                         |

Want to try it first? Check out the [browser demo](https://seaquel.app/demo) (powered by DuckDB WASM).

### Self-host (Docker)

Seaquel ships a single container image at `ghcr.io/webstonehq/seaquel`,
multi-arch (linux/amd64 + linux/arm64). It runs SvelteKit + Better Auth on
the public port and a loopback-only Rust service (database connections,
per-user metadata storage and licensing) as a subprocess in the same
container — one artifact, one `docker run`. A Seaquel subscription license
key is required at first signup, the same as Cloud.

The web app connects to PostgreSQL, MySQL, MariaDB and SQL Server over the
network. SQLite and DuckDB connections, SSH tunnels, shared projects (git),
client-certificate TLS and Unix-socket connections are desktop-only: on a
server they would read files or reach sockets on the host. The SQL tutorial
still works, since it runs DuckDB-WASM in the browser from the image's own
copy.

#### `docker run`

```bash
docker run \
  --name seaquel \
  -p 8787:8787 \
  -v seaquel-data:/data \
  ghcr.io/webstonehq/seaquel:latest
```

That's it. Visit <http://localhost:8787>. The first user to sign up
presents an owner license key from a Seaquel subscription — that key
registers this install with seaquel.app and binds the user as Owner.
Subsequent teammates sign up by pasting their own member license key
from the same subscription's seat pool.

Persistent state lives in `/data`: `auth.db` for identity, one
`users/<userId>/meta.db` per user, and an auto-generated `auth-secret`
file the container uses to sign sessions. Back up that volume and you
back up everything.

#### `docker compose`

```bash
# From a checkout of this repo:
docker compose -f deploy/docker/docker-compose.yml up -d
```

The compose file pulls the pre-built image by default; flip to building
from source by uncommenting the `build:` block. The `.env.example` file
in `deploy/docker/` documents the optional knobs below — copy it to
`.env` only if you need to override something.

#### Configuration

The defaults work out of the box: port `8787`, data at `/data`, talks to
`https://seaquel.app` for licensing, and auto-generates a session-signing
secret at `/data/auth-secret` on first boot. Set the variables below only
when the default doesn't fit your deployment.

| Variable                  | Set this when…                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SEAQUEL_TRUSTED_ORIGINS` | The install is reached by a domain name and `BETTER_AUTH_URL` isn't set, or it has more than one public origin. Comma-separated origins the CSRF checks allow. `BETTER_AUTH_URL`'s origin (else `ORIGIN`'s) is always trusted. With neither set, an install reached at `localhost` or an IP address (`http://localhost:8787`, `http://192.168.1.20:8787`) trusts the address you opened; a domain name is never trusted that way, since a DNS-rebinding page could supply it. |
| `BETTER_AUTH_URL`         | The install is reached by a domain name (set it to the public URL, e.g. `https://seaquel.example.com`), sits behind a reverse proxy, or you want to silence the boot warning below. Sets Better Auth's canonical URL and the origin the CSRF checks trust. Once it is set, only its origin and `SEAQUEL_TRUSTED_ORIGINS` are trusted.                                                                                                                                         |
| `SEAQUEL_AUTH_SECRET`     | Running multiple replicas. All replicas must share a session key; single-container installs don't need to set this.                                                                                                                                                                                                                                                                                                                                                           |
| `SEAQUEL_COOKIE_DOMAIN`   | Sharing sessions across subdomains, e.g. `.example.com`.                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `SEAQUEL_TRUSTED_PROXIES` | Running behind a reverse proxy or load balancer. Comma-separated proxy IPs/CIDRs (e.g. `10.0.0.0/8`). Without it, sign-in rate limits key on the socket address and `X-Forwarded-For` is ignored, so every client behind the proxy shares one limit.                                                                                                                                                                                                                          |
| `SEAQUEL_CONTROL_URL`     | Pointing at a staging control plane, or `http://127.0.0.1:1` to test offline mode. Default `https://seaquel.app`.                                                                                                                                                                                                                                                                                                                                                             |
| `SEAQUEL_AI_EGRESS`       | Changing where the AI assistant's model calls may go. The server calls the provider with the key the browser sends for that call. `public` (default): public addresses over `https` only, redirects never followed. `any`: private networks and `http:` too, for a model server next to Seaquel (Ollama, vLLM). `off`: every model call is refused, for air-gapped installs. Any other value stops the server at startup. See the proxy note below.                           |
| `PORT` / `DATA_DIR`       | Overriding `8787` / `/data`.                                                                                                                                                                                                                                                                                                                                                                                                                                                  |

Internal tuning knobs (`SEAQUEL_LICENSE_SOFT_TTL`, `SEAQUEL_LICENSE_GRACE_TTL`,
`SEAQUEL_BUNDLE_TRUSTED_PUBKEY`) have sensible defaults documented inline
in the source; set them only when you need to.

**Outbound proxies and TLS inspection.** License calls to seaquel.app and
the AI assistant's model calls come from the Rust service. They go through
`HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` (and skip hosts in `NO_PROXY`) when
those are set in the container's environment. If a proxy re-signs TLS,
point `NODE_EXTRA_CA_CERTS` at a PEM file with its CA certificate;
`SSL_CERT_FILE` and `SSL_CERT_DIR` work too.

Behind a proxy, `SEAQUEL_AI_EGRESS=public` still refuses a provider URL
whose host is a private or local IP address, and a name whose answers from
the server's own DNS are all private or local. A name the server can't
resolve itself goes to the proxy, which then decides what it may reach, so
restrict private destinations on the proxy too. The proxy's own host is
trusted as configured, and a provider URL naming the proxy's host is
refused.

**What reaches the Rust service.** It gets only the variables it needs:
the ones above, `DATA_DIR`, the proxy and CA variables, `PATH`, `TZ`,
`LANG` and the temp directories. `PG*` and `MYSQL*` variables (`PGPASSWORD`,
`PGHOST`, …) and a `~/.pgpass` file are never used for a user's connection.

Node and the Rust service must run in the same container (the image's
default). Licensing refuses calls from any other host, so a split
deployment gets a 503 page on every request. The same page shows when the
Rust service is down or can't answer.

With `BETTER_AUTH_URL` unset, every boot logs:

```
WARN [Better Auth]: [better-auth] Base URL could not be determined. Please
set a valid base URL using the baseURL config option or the
BETTER_AUTH_URL environment variable. Without this, callbacks and
redirects may not work correctly.
```

This is expected and harmless for an install reached at `localhost` or an
IP address: signup, sessions and cookies all work. An install reached by a
domain name must set `BETTER_AUTH_URL` to its public URL (or list its origin
in `SEAQUEL_TRUSTED_ORIGINS`), or signup, sign-in and every `/api` call are
refused with 403. Set it also whenever anything rewrites `Host` or you rely
on absolute links (e.g. emailed callbacks).

#### Air-gapped / offline mode

Self-hosted Seaquel can run without any outbound calls to seaquel.app. The
owner downloads a signed license bundle from the seaquel.app dashboard and
uploads it to the instance. From that point, all license checks happen
locally against the bundle; the seaquel.app control plane is never
contacted.

**Workflow:**

1. Subscription owner signs in to seaquel.app and visits
   `https://seaquel.app/dashboard/<slug>/airgap` (the "Offline bundle" tab
   appears for self-hosted tenants).
2. Owner reviews seat assignments and revocations, then clicks "Download
   offline license bundle". A signed `.bundle` file downloads.
3. Owner transfers the file to the self-hosted instance (USB, SCP, etc.)
   and uploads it via `/settings/airgap` (or `/airgap-setup` if no user
   is bound yet).
4. The instance verifies the bundle's signature, walks any
   revocations, and switches to offline mode. The control plane is no
   longer called.

**Renewal:**

- Bundles expire `currentPeriodEnd + 30 days`. Owners download a fresh
  bundle each billing cycle from the same dashboard page.
- Revocations are cumulative — each new bundle carries the full
  revocation list for the subscription.
- A subscription cancellation in seaquel.app automatically adds all of
  that subscription's license keys to the revocation list (so the _next_
  bundle the owner downloads carries the revocations).
- If the bundle isn't refreshed before `not_after`, the install enters
  a read-only revalidate state until a fresh bundle is imported.

**Return to online mode:**

- An owner can remove the offline bundle via `/settings/airgap` →
  "Remove offline bundle". The instance will resume calling seaquel.app
  on the next license check. Make sure connectivity is restored before
  removing the bundle.

**Trust anchors:**

A bundle is verified against the public half of the signing keypair. The
private half — a 32-byte Ed25519 seed — lives only on the control plane,
as the `SEAQUEL_BUNDLE_SIGNING_PRIVATE_KEY` secret on seaquel-app. The
public half is compiled into this repo, as `PROD_TRUSTED_PUBKEYS` in
`crates/seaquel-license/src/server/airgap/bundle_store.rs`.

To derive that public half from a seed without minting anything, pass
`--only-derive`:

```bash
node scripts/mint-airgap-bundle.ts --only-derive --seed-hex=<64-hex>
```

stdout carries just the `SEAQUEL_BUNDLE_TRUSTED_PUBKEY=…` line, so it can
be piped or captured; stderr prints the fingerprint and pubkey separately,
plus a paste-ready `PROD_TRUSTED_PUBKEYS` entry. Omit `--seed-hex` and it
generates a fresh keypair and prints the seed as well — the seed is the
private half, so it belongs in the control-plane secret and nowhere else.

The fingerprint is the first 16 bytes of `SHA-256(pubkey)`, and the
verifier looks keys up by it, so an anchor whose fingerprint does not
match its pubkey rejects every real bundle instead of erroring. The
`ships_a_well_formed_built_in_production_set` test in
`crates/seaquel-license/tests/bundle_store.rs` guards against exactly that.

Because `PROD_TRUSTED_PUBKEYS` is a list, a key rotation can ship a
release carrying both the old and new anchors, switch the control-plane
secret, then drop the old entry in a later release.

**Optional dev override:**

- `SEAQUEL_BUNDLE_TRUSTED_PUBKEY` — comma-separated list of
  `<fingerprint>:<hex-pubkey>` entries to trust additional signing keys
  (for testing with a dev keypair). Production releases ship with the
  seaquel.app production fingerprint baked in.

#### Test Docker image locally

Two smoke-test flows are documented below: **Online mode** (against the
real licensing control plane) and **Air-gapped mode** (with a dev-minted
bundle, no outbound network). Both build the same image.

##### Build the image

```bash
cd path/to/seaquel
docker build -t seaquel:test .
```

The first build takes ~5 min (Rust + Node); subsequent builds reuse layers.

##### Online mode

A dedicated volume (`seaquel-test-data`) keeps the smoke-test isolated from
any production container on the same machine.

```bash
docker run \
  --name seaquel-test \
  -p 8787:8787 \
  -v seaquel-test-data:/data \
  seaquel:test
```

No env vars needed for a localhost smoke test — the CSRF checks trust the
address you open (`http://localhost:8787` or `http://127.0.0.1:8787`) as the
install's own origin, and the session secret auto-generates on first
boot. Wait for the health check to pass (~20 s), then probe it:

```bash
docker ps --filter name=seaquel-test --format '{{.Status}}'
curl http://localhost:8787/health
```

In the browser at <http://localhost:8787>:

1. `/signup` — the first user becomes the tenant Owner. Paste an
   owner-tier subscription license key when prompted.
2. Create a project, add a connection to a Postgres, MySQL, MariaDB or
   SQL Server database you have access to. (SQLite and DuckDB aren't
   offered on web; a `sqlite:` connection string is refused.)
3. Save the password on the connection — the credential-vault setup
   dialog should appear. Pick a passphrase; credentials are encrypted
   in the browser before they ever reach the server (zero-knowledge).
4. Run a query, browse the schema, verify everything works.

Cleanup:

```bash
docker rm -f seaquel-test
docker volume rm seaquel-test-data
```

##### Air-gapped mode

Exercise the offline flow with a locally-minted bundle. Air-gapped mode
itself is purely bundle-driven: once a bundle is imported, the license
service (`crates/seaquel-license/src/server`) routes every call to its
air-gap equivalent and never touches the network. **A real air-gapped deployment just imports the bundle —
there's no env var that turns offline mode "on."**

**1. Mint a bundle with a dev keypair.**

```bash
node scripts/mint-airgap-bundle.ts \
  --owner-key=DEV-OWNER \
  --member-key=DEV-MEMBER \
  --out=/tmp/seaquel-test.bundle
```

The script prints to stderr a line like:

```
SEAQUEL_BUNDLE_TRUSTED_PUBKEY=c6cf6554…:6fecf261…
```

Copy that line — the container needs it as an env var so its verifier
trusts the dev keypair just generated. The baked-in
`PROD_TRUSTED_PUBKEYS` anchor only covers bundles signed by seaquel.app,
so this override is always required for locally minted bundles. To mint
many bundles with the same trust anchor across runs, capture the
`--seed-hex=…` line the script also prints and pass it back on
subsequent invocations — or re-derive the anchor from that seed later
with `--only-derive` (see "Trust anchors" above).

**2. Run the container.**

```bash
TRUSTED_PUBKEY='c6cf6554…:6fecf261…'   # the line from step 1's stderr

docker run \
  --name seaquel-airgap-test \
  -p 8787:8787 \
  -v seaquel-airgap-data:/data \
  -e SEAQUEL_BUNDLE_TRUSTED_PUBKEY="$TRUSTED_PUBKEY" \
  seaquel:test
```

> **Optional test scaffold:** if you want to prove the airgap path
> doesn't leak any network calls, also pass
> `-e SEAQUEL_CONTROL_URL=http://127.0.0.1:1`. That points the would-be
> control-plane URL at a closed port, so any bug in the dispatcher that
> accidentally tried to call out would surface as a loud transport
> error instead of silently reaching real seaquel.app. **It does
> nothing for normal operation** — air-gapped mode skips the network
> regardless of where this URL points. Real air-gapped deployments,
> where the host has no outbound network at all, can leave the default
> in place.

**3. Import the bundle.**

Visit <http://localhost:8787> in the browser. The layout redirects
unauthenticated visitors to `/airgap-setup` whenever (a) no tenant is
yet bound and (b) no bundle is loaded. Upload `/tmp/seaquel-test.bundle`
there. On success you'll see tier/seats/expiry; the page then offers a
"Continue to signup" link.

**4. Sign up with the bundle's owner key.**

At `/signup`, use `DEV-OWNER` as the license key. The license service
routes through `register_install_local` (because a bundle is loaded), the
owner's `member_license` row is written with `is_owner=1`, and a session
cookie comes back. No network call to `seaquel.app` happened.

**5. Try signing up with a key the bundle doesn't know.**

Open a private window, hit `/signup`, paste any random string as the
license key. Expect a `license_not_found` rejection from
`verify_local_membership_license`.

**6. Sign up the member.**

Use `DEV-MEMBER` as the license key. Should succeed.

**7. Confirm the install is in offline mode.**

Settings → Offline license bundle → the badge reads `AIRGAP` and lists
tier, seats, expiry, and the truncated pubkey fingerprint. Or via the
CLI:

```bash
docker exec seaquel-airgap-test node -e \
  'console.log(require("better-sqlite3")("/data/auth.db", { readonly: true })
    .prepare("SELECT mode FROM install_cache WHERE id = 1").get().mode)'
```

`airgap` confirms the dispatcher is in bundle-driven mode.

(The image has no `sqlite3` binary — query the file through the
`better-sqlite3` module the server itself uses.)

**8. Test the revocation walk.**

Mint a fresh bundle that revokes the member's key (note: the new bundle
must have an `issued_at` strictly newer than the current one and the
same `subscription_id`):

```bash
node scripts/mint-airgap-bundle.ts \
  --seed-hex=$LAST_SEED   # the --seed-hex line from step 1's stderr
  --owner-key=DEV-OWNER \
  --member-key=DEV-MEMBER \
  --revoked-keys=DEV-MEMBER \
  --out=/tmp/seaquel-test-v2.bundle
```

Upload the new bundle at `/settings/airgap` as the owner. The license
service walks `member_license` and stamps `revoked_at` on the member's
row in one transaction, then the Node server deletes their Better Auth
sessions. If that last step fails, uploading the same bundle again
finishes it. The member's next request returns 403 and they're bounced
back to login.

**9. Test offline → online transition.**

Still as the owner, click "Remove offline bundle" at `/settings/airgap`.
The instance clears the bundle and flips `install_cache.mode` back to
`online`. `member_license` bindings are kept, so both users stay signed
in.

You will **not** land on `/revalidate` right away — not even with the
`SEAQUEL_CONTROL_URL=http://127.0.0.1:1` scaffold from step 2. The
license state is cached: `last_validated_at` was stamped when the bundle
was imported, so for the next 24 h (`SEAQUEL_LICENSE_SOFT_TTL`) the
install is "soft fresh" and makes **no** control-plane call at all. Once
that lapses it attempts a refresh, and if the call fails it keeps working
until `grace_until` — also stamped at import, default 14 days
(`SEAQUEL_LICENSE_GRACE_TTL`) — before dropping to `revalidate`.

Both windows are stored as absolute timestamps at import time, so
changing those two env vars afterwards does not move them. To reach the
`revalidate` state now, age the stored timestamps directly:

```bash
docker exec seaquel-airgap-test node -e '
  const db = require("better-sqlite3")("/data/auth.db");
  const past = Math.floor(Date.now() / 1000) - 3 * 24 * 3600;
  db.prepare("UPDATE install_cache SET last_validated_at = ?, grace_until = ? WHERE id = 1")
    .run(past, past);
'
```

The next request then logs `license refresh failed` (the closed-port
scaffold proving the call was actually attempted), redirects to
`/revalidate`, and returns 403 from `/api/*` until a fresh bundle is
imported. Without the scaffold the call reaches real seaquel.app and
fails with a 4xx, since this dev container was never registered
upstream.

Cleanup:

```bash
docker rm -f seaquel-airgap-test
docker volume rm seaquel-airgap-data
```

## Database Support

| Feature              |     PostgreSQL     |       MySQL        |      MariaDB       |       SQLite       |       MSSQL        |       DuckDB       |
| -------------------- | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: | :----------------: |
| Connect & query      | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Schema browser       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| EXPLAIN visualizer   | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| ERD generation       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Inline editing       | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Statistics dashboard | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: | :white_check_mark: |
| Self-hosted web app  | :white_check_mark: | :white_check_mark: | :white_check_mark: |        :x:         | :white_check_mark: |        :x:         |

SQLite and DuckDB are desktop-only. The self-hosted web app runs on a
server, where a SQLite or DuckDB "connection" would be a file on that
server, so it doesn't offer them.

## MCP server

The desktop app ships a command line tool, `seaquel-cli`, whose `mcp`
subcommand runs an [MCP](https://modelcontextprotocol.io) server over stdio.
An MCP host such as Claude Desktop or Claude Code can then list the
connections you pick, read their schemas, run read-only queries and saved
queries, and EXPLAIN a query. It uses your saved connections, passwords and
SSH settings from the app, so there is nothing to configure twice. The app
doesn't need to be running.

### Setup

Open **Settings → MCP** in the app. Check the connections, or whole projects,
the server may use, then copy one of the snippets it builds:

- **Claude Desktop:** add the JSON to `claude_desktop_config.json` (Settings →
  Developer → Edit Config in Claude Desktop), merging it into `mcpServers` if
  the file already has one, and restart Claude Desktop.
- **Claude Code:** run the `claude mcp add seaquel -- …` line in a terminal.

The app doesn't come with `seaquel-cli`; it downloads it when you ask. Use
**Install Command Line Tool…** in the app menu, or the button in the panel. The
app fetches the build that matches its own version from the GitHub release,
checks its size and SHA-256, and keeps it in its data folder
(`~/Library/Application Support/app.seaquel.desktop/bin` on macOS,
`~/.local/share/app.seaquel.desktop/bin` on Linux,
`%LOCALAPPDATA%\app.seaquel.desktop\bin` on Windows). The snippets point at
that file. On macOS it also links `/usr/local/bin/seaquel-cli` (asking for
your password if needed), and on Linux `~/.local/bin/seaquel-cli`, so you can
type `seaquel-cli` in a terminal. On Windows it doesn't change `PATH`. After
the app updates, install again to get the matching CLI.

Open the app once after installing or updating it before you start the
server. The server never changes the app's data file, so if a new version has
an update to make, it refuses with "Open the Seaquel app once to update your
data".

### What the server can see

Only the connections named on its command line:

```bash
seaquel-cli mcp --connection <id or name> --connection <id or name>
seaquel-cli mcp --project <id or name>
```

Both flags can be repeated, and names must match exactly. The list is read
when the server starts, so a connection added to an exposed project shows up
after the MCP host restarts the server. With neither flag the server starts
with no connections. The settings panel names connections and
projects by id, since names can change.

The AI sharing settings apply here too, per connection or from Settings → AI:
with schema sharing off the server won't describe that connection, and with
"Allow AI to run read-only queries" off it won't run queries on it. Data
sharing is off by default, so turn it on for each connection you want
queried. Sharing changes apply to the next tool call, without a restart.

### Read-only

The server never writes. Queries go through the same read-only check as the
in-app AI and then run in the database's read-only mode (on SQL Server,
inside a transaction that is always rolled back; a read-only login is the
only full guarantee there). EXPLAIN never runs ANALYZE. Results are capped
at 1,000 rows (100 unless the host asks for more), 64 KB per cell and about
4 MB per result, and each call is cancelled on the database after 60 seconds.

### DuckDB

DuckDB connections open locked down: the server can read the tables and
views in the database file and nothing else. Reading other files (`read_csv`,
`read_parquet`, a path used as a table, `glob`), `COPY`, `ATTACH`, and
installing or loading extensions are refused, so a view over a CSV or Parquet
file fails too. JSON functions work. There is no time zone support, so
functions that need a time zone and arithmetic on `TIMESTAMPTZ` values fail.

`seaquel-cli` runs DuckDB in a separate helper, a download of its own (see
[DuckDB in the terminal](#duckdb-in-the-terminal)). **Install Command Line
Tool…** fetches it along with the CLI. If it's missing, a DuckDB tool call
fails with a message saying to run `seaquel-cli duckdb install`, and the
server prints the same on stderr when it starts.

### Passwords and the macOS keychain

The server reads saved passwords from the same keychain entries as the app.
On macOS the first query on a connection with a saved password (or SSH
password or key passphrase) shows a prompt asking whether `seaquel-cli` may use
it. Choose **Always Allow** and it won't ask again for that item. The call
waits while the prompt is open. If you choose Deny, the call fails with a
message naming the connection. A connection without a saved password can't
be used: save it in the app first.

SSH tunnels work for hosts the app already trusts. The server never adds a
host key, so for a new bastion, connect once in the app and accept its key.

## Terminal UI

`seaquel-tui` is Seaquel in a terminal, laid out like lazygit: numbered panels
for the connection, its tables and views, saved queries and history, and the
changes you've staged, with the table or query editor beside them. It uses the
app's saved connections, passwords, SSH settings and saved queries. Browse a
table, edit cells, stage inserts and deletes, review them as a diff and commit
them in one transaction. Write SQL with completion, run it, page through the
results, look at an `EXPLAIN ANALYZE` tree, and ask the AI assistant for a
query.

It runs beside the app on the same data. A query you save in the TUI shows up
in the app within a second or two, and the other way round. Nothing in it
checks for a license; the [pricing terms](https://seaquel.app/pricing) apply
as they do to the app.

### Install

The app doesn't install `seaquel-tui` for you yet. Download it from the
[GitHub release](https://github.com/webstonehq/seaquel/releases) that matches
your app's version, one file per platform:

| Platform             | Asset                                     |
| -------------------- | ----------------------------------------- |
| macOS, Apple Silicon | `seaquel-tui-aarch64-apple-darwin`        |
| macOS, Intel         | `seaquel-tui-x86_64-apple-darwin`         |
| Linux, x86_64        | `seaquel-tui-x86_64-unknown-linux-gnu`    |
| Linux, ARM64         | `seaquel-tui-aarch64-unknown-linux-gnu`   |
| Windows, x86_64      | `seaquel-tui-x86_64-pc-windows-msvc.exe`  |
| Windows, ARM64       | `seaquel-tui-aarch64-pc-windows-msvc.exe` |

On macOS or Linux, with your app's version in place of `2026.10.0`:

```bash
VERSION=2026.10.0
ASSET=seaquel-tui-aarch64-apple-darwin   # from the table
mkdir -p ~/.local/bin
curl -fL -o ~/.local/bin/seaquel-tui \
  "https://github.com/webstonehq/seaquel/releases/download/v$VERSION/$ASSET"
chmod +x ~/.local/bin/seaquel-tui
seaquel-tui --version
```

`~/.local/bin` has to be on your `PATH`; any folder that is will do.

The binaries are signed but not notarized. A file fetched with `curl` starts
as is, but macOS refuses to run one downloaded with a browser until you clear
the quarantine flag:

```bash
xattr -d com.apple.quarantine ~/.local/bin/seaquel-tui
```

On Windows, download the `.exe` and run it from Windows Terminal. The Windows
builds come from the same release job but haven't been tested yet.

Open the app once after installing or updating it. Like the MCP server,
`seaquel-tui` doesn't upgrade the app's data file; if the file needs an
update, it stops and names the app version to open. A TUI newer than your app
asks for the newer app.

### DuckDB in the terminal

`seaquel-tui` and `seaquel-cli` don't contain DuckDB, which would triple their
size. DuckDB runs in a small helper program, `seaquel-duckdb`, fetched the
first time you need it. Each DuckDB connection gets its own helper process,
which exits when the connection closes.

- **In the TUI**, the first time you connect to a DuckDB database it says
  that DuckDB support is a separate download (about 12 MB) and asks. Press
  Enter to download it; a progress bar follows, then the connection opens.
  Esc cancels and leaves nothing behind.
- **From the command line:**

  ```bash
  seaquel-cli duckdb status    # installed <path>, missing, outdated or unsafe
  seaquel-cli duckdb install
  ```

  The app's **Install Command Line Tool…** also installs it for the CLI.

The helper is downloaded from the GitHub release that matches the program's
version (it refuses a helper of any other version), and its size and SHA-256
are checked against the release before it's put in place. Proxies set with
`HTTPS_PROXY` and extra certificates in `NODE_EXTRA_CA_CERTS` are used.

**Without a network**, download `seaquel-duckdb-<platform>.gz` for your
version from the [releases page](https://github.com/webstonehq/seaquel/releases)
on another machine (the platform names are those in the table above, with
`.exe.gz` on Windows), note the SHA-256 the page shows for it, copy the file
over and run:

```bash
seaquel-cli duckdb install --from seaquel-duckdb-aarch64-apple-darwin.gz \
  --sha256 <the 64-character SHA-256 from the release page>
```

The CLI installs the helper for its own version, which is the one the TUI of
the same version uses.

**Where it lives:** in the app's local data folder, one folder per version:

| Platform | Folder                                                                    |
| -------- | ------------------------------------------------------------------------- |
| macOS    | `~/Library/Application Support/app.seaquel.desktop/bin/duckdb/<version>/` |
| Linux    | `~/.local/share/app.seaquel.desktop/bin/duckdb/<version>/`                |
| Windows  | `%LOCALAPPDATA%\app.seaquel.desktop\bin\duckdb\<version>\`                |

With `SEAQUEL_DATA_DIR` set, it's `$SEAQUEL_DATA_DIR/bin/duckdb/<version>/`.
The folders and the file are readable only by you, and a helper in a folder
others can write to isn't started (`status` says `unsafe`; installing again
fixes the folder). Installing keeps the two newest versions and removes
older ones, so after a newer TUI or CLI has installed its helper, an older
one may ask to download again. To remove DuckDB support, delete the
`bin/duckdb` folder; nothing else refers to it.

### Using it

```bash
seaquel-tui                                # pick a project and a connection
seaquel-tui --connection <id or name>      # straight to a saved connection
seaquel-tui --project <id or name> --theme light --no-mouse --page-size 200
```

Press `?` for every key. The bar at the bottom shows the ones that work where
you are. The main ones:

| Keys                    | What they do                                                    |
| ----------------------- | --------------------------------------------------------------- |
| `1` `2` `3` `4`, `0`    | focus a panel, or the main view; `Tab` cycles                   |
| `j` `k`, `Enter`, `Esc` | move, open, go back                                             |
| `[` `]`                 | switch the focused box's tab                                    |
| `e`, `d`, `a`, `D`      | in a table: edit a cell, stage a delete, an insert, Set default |
| `/`, `F`, `s`           | filter the loaded rows, filter the table, sort                  |
| `u`, `c`                | undo the last staged change, commit the staged changes          |
| `Q`, `+`                | the query editor, a new query tab                               |
| `Ctrl+R`, `Ctrl+E`      | run the whole text, run the statement at the cursor             |
| `Ctrl+X`, `:analyze`    | `EXPLAIN`, `EXPLAIN ANALYZE` of the statement at the cursor     |
| `Ctrl+K`                | ask the AI assistant for SQL                                    |
| `Ctrl+S`, `Ctrl+O`      | save the query, edit it in `$VISUAL` or `$EDITOR`               |
| `Ctrl+C`, `q`           | stop a running statement, quit                                  |

The editor starts in insert mode; `Esc` switches to a small set of vim keys
(`hjkl`, `w b e`, `dd`, `yy`, `p`, `u`, `:w`). Committing on a connection with
the **prod** label asks you to type `prod`. Ask AI only inserts what the model
writes; `Ctrl+R` in its box also runs the answer, but only when it's a single
statement that passes the same read-only check the assistant uses. That check
looks at the SQL text; the statement then runs with the connection's own
rights.

Open query tabs, unsaved SQL included, are kept between runs in
`tui/state.json` in the app's data folder (readable only by you; a tab over
1 MB isn't kept). The log is `logs/tui.log` in the same folder, at `warn`
unless you pass `--log-level`. It never holds SQL, values, passwords or
connection names.

### Passwords and the keychain

The TUI reads passwords the app saved from the system keychain. On macOS the
first read of each one shows a dialog asking whether `seaquel-tui` may use it,
sometimes behind the terminal window; the TUI says it's waiting. Choose
**Always Allow** and it won't ask again for that item. A connection without a
saved password asks for it when you connect. Tick **Save password** and the
TUI stores it once the connection works. The app then asks once, the first
time it reads that password; choose **Always Allow** there too.

On a machine with no keyring to talk to, such as a Linux server reached over
SSH without a Secret Service, the TUI asks for a saved connection's password
each session and can't save it.

An SSH bastion the TUI hasn't seen before shows its host key's fingerprint.
Trusting it adds the key to `~/.ssh/known_hosts`, the same as the app does. A
changed key is refused.

### Terminals

- **tmux:** add `set -sg escape-time 10` to `~/.tmux.conf`. With tmux's
  default (500 ms), `Esc` followed quickly by another key reaches the TUI as
  one Alt-key. The TUI splits most of those back into the two keys, but `Esc`
  then `r` arrives as Alt+R and runs the statement, and `Esc` then `x` as
  Alt+X, which explains it with `ANALYZE` (asking first unless it's a plain
  `SELECT`).
- **Option on macOS:** Terminal.app and iTerm2 type characters such as `®`
  for Option+R unless Option sends Meta (Terminal.app: Settings → Profiles →
  Keyboard → "Use Option as Meta key"; iTerm2: Profiles → Keys → Left Option
  key: Esc+). The keys the bar shows don't need it.
- **Mouse:** clicks select panels, rows, cells and tabs, and the wheel
  scrolls. To select text, hold Shift while dragging (Option in iTerm2, Fn in
  Terminal.app), or start with `--no-mouse`.
- **Colours:** truecolor when `COLORTERM` says so, 256 colours otherwise.
  `--theme light` suits a light background, and `NO_COLOR=1` turns colour off
  (bold, underline and reverse video mark focus and changes instead).
- **Size:** at least 80×24.

## Community

- [Discord](https://seaquel.app/discord) — Chat, ask questions, share feedback
- [GitHub Issues](https://github.com/webstonehq/seaquel/issues) — Bug reports and feature requests

## Contributing

Contributions are welcome! Here's how to get started:

1. Browse the [open issues](https://github.com/webstonehq/seaquel/issues) to find something to work on
2. For larger changes, open an issue first to discuss your approach
3. Fork the repo, create a branch, and submit a pull request

## Development

### Prerequisites

Use [mise](https://mise.jdx.dev/) to install the required toolchain:

```bash
mise install
```

This installs Node.js and Rust as defined in `mise.toml`.

### Setup

```bash
git clone https://github.com/webstonehq/seaquel.git
cd seaquel
npm install
```

### Commands

```bash
npm run tauri:dev       # Start development, note "tauri:dev" vs the default "tauri dev"
npm run tauri build     # Build production app
npm run check           # Type checking
npm run check:watch     # Type checking (watch mode)
```

## Tech Stack

- [Tauri](https://tauri.app/) — Native app shell with Rust backend
- [SvelteKit](https://svelte.dev/docs/kit) — App framework
- [Svelte](https://svelte.dev/) — UI with runes-based reactivity
- [TypeScript](https://www.typescriptlang.org/) — Type safety
- [Tailwind CSS](https://tailwindcss.com/) — Styling
- [Rust](https://www.rust-lang.org/) — Backend plugins and system integration

## License

Seaquel splits its source from the binaries we distribute.

**Source code** — [MIT](LICENSE). View it, modify it, redistribute it, and
build and run your own binaries, including at work, with no license from us.
The MIT grant does not extend to the Seaquel name, logo, or branding, so please
don't ship your builds under them.

**Official desktop binaries** — the builds on
[seaquel.app/download](https://seaquel.app/download), GitHub Releases, and the
in-app updater are free for personal, non-commercial use. Using them for work
requires a commercial license: see [pricing](https://seaquel.app/pricing).

**Seaquel Cloud and the self-host image** — a subscription is required for all
use, personal included.

Full terms: [seaquel.app/terms](https://seaquel.app/terms).
