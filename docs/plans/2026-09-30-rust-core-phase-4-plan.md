# Phase 4 Implementation Plan: `seaquel mcp`

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** Ship a command-line binary bundled with the desktop app whose one subcommand, `mcp`, runs an MCP server over stdio. The server lets an MCP host such as Claude Desktop or Claude Code list the user's saved connections, read their schemas and run read-only queries and saved queries on them. It uses the same Core, storage, keychain and SSH code as the app.

**Architecture:**
- **New crates.**
  - `seaquel-workspace`, a domain crate. Its first module turns a stored connection plus its keychain secrets into a `ConnectConfig`. That logic is a port of the TypeScript in `connection-manager.svelte.ts`, `connection-string.ts` and `wire.ts`.
  - `seaquel-mcp`, an interface crate. It holds the rmcp server and its tool set.
  - `seaquel-cli`, which builds the binary. It uses clap, and `mcp` is its only subcommand.
- **Core additions.**
  - `Workspace::connect_saved(id)`, which loads the row, reads secrets and opens the SSH tunnel.
  - A `max_rows` option on read-only queries, which truncates instead of failing.
- **How the MCP server reads the desktop app's data.** It opens `seaquel.db` read-only, so it never migrates or writes the file. It reads the app's passwords through the same keychain entries.
- **Packaging.** The binary ships as a Tauri sidecar. On macOS a menu item links it onto `PATH`, and a settings panel shows the config snippet an MCP host needs.

**Tech Stack:** Rust (rmcp, clap 4, schemars 1, the existing Core, storage, secrets, SSH and engines), Tauri 2 `bundle.externalBin`, TypeScript/Svelte 5 for one settings panel, and the e2e Docker databases plus the SSH container.

**Inputs:**
- the design doc: decisions 10, 14 and 15, "Terminal binaries", "MCP tool set (first cut)", the Risks section on keychain and concurrent writers, and the "Phase 3 cost" estimating notes;
- phase 3's hand-off: the `data_version`/`StorageChanged` event, serialising migrations, and the macOS keychain check;
- the AI-safety phase's follow-up "Fetch only a sample for the AI";
- two code maps taken on 2026-09-30, which supply the numbers below.

---

## What the code map found

These facts come from the code map and the Tauri and keyring sources, and the plan is built on them.

1. **The sidecar can't be called `seaquel`.**
   - tauri-build 2.6.3's `copy_binaries` refuses a sidecar with the same name as the Cargo package, and the desktop package is `seaquel`.
   - Renaming the package doesn't help:
     - **macOS:** the app's executable is `Contents/MacOS/seaquel`, and APFS is case-insensitive.
     - **Linux:** deb and rpm put both the app and its sidecars in `/usr/bin`, so `/usr/bin/seaquel` is already the GUI.
     - **Windows:** `Seaquel.exe` sits in the install folder, so putting that folder on `PATH` would make `seaquel` start the GUI.
   - The file therefore ships as `seaquel-cli-<target-triple>`. Open question 1 is about the name users type.
2. **On macOS, a second binary can't read the app's keychain items without asking.**
   - keyring 3.6.3 uses the legacy file-based keychain (`SecKeychainFindGenericPassword`, never the data protection keychain).
   - Each item's ACL trusts only the app that created it, matched by its designated requirement, which includes the bundle identifier.
   - So a CLI signed by the same team gets an "Allow / Always Allow / Deny" prompt per item. "Always Allow" sticks for a signed build, and unsigned dev builds ask again after each rebuild.
   - Keychain access groups apply only to the data protection keychain. Moving to it would orphan every saved password, and it would need a provisioning profile that a bare CLI can't carry.
   - Open question 2 is how to handle the prompt.
3. **Nothing in Rust builds a `ConnectConfig` from a stored connection. The TypeScript does it, spread across several places:**
   - `toRustConfig` in `wire.ts`, per engine: MSSQL uses its fields and ignores the string; DuckDB takes a path; SQLite and Postgres/MySQL take the connection string, with MariaDB going through the mysql driver;
   - putting the password back into a stripped URL (`url.password = …`);
   - rebuilding a missing string with `buildConnectionString`, which covers ssl-mode mapping and default ports;
   - replacing the host and port when an SSH tunnel is used;
   - picking the password from `db:<id>`, `ssh:<id>` and `ssh-key:<id>` according to the `save*` flags;
   - falling back to the username from the URL.
   
   Shared connections are imported into `connections` as ordinary rows. `SharedConnectionManager.buildRuntimeConnection` has no callers.
4. **The read-only path is ready in Core.** `query_stream` with `with_read_only(true)` runs `check_read_only` and each engine's `query_read_only`. The row cap is all-or-nothing, though: more than `max_query_rows()` rows (100,000) fails with `RESULT_TOO_LARGE`. An MCP host needs a sample instead.
5. **Scoping.** Every connection belongs to a project (`project_id` NOT NULL). The GUI shows only the active project, whose id is in `app_state.lastActiveProjectId`. Each row has `ai_share_schema` and `ai_share_data`, where NULL means use the global default in the AI settings.
6. **Distribution pieces.**
   - `release.yml` builds six targets through tauri-action, with macOS notarized and Windows signed by Azure Trusted Signing.
   - `tauri.conf.json` has no `externalBin` yet.
   - The app menu is built in `src-tauri/src/lib.rs` `create_menu`.
   - The shell plugin isn't needed, because we only ship the sidecar and never spawn it.
   - rmcp and clap aren't in the lockfile yet.

## Open questions

Each question has a recommendation, and the plan is written as if it is taken. If one isn't, only the named tasks change.

**Answered (2026-09-30):** the owner took all three recommendations: the command is `seaquel-cli` everywhere, the macOS keychain prompt is accepted and explained, and only connections named on the command line are exposed. Execution is subagent-driven.

1. **What should users type?** The file is `seaquel-cli` whatever the answer (finding 1).
   - **Recommendation: `seaquel-cli` everywhere,** in `PATH` links, docs and the MCP snippet. It's unambiguous on all three platforms, and MCP hosts are configured with a full path anyway.
   - **The alternative:** `seaquel` on macOS, through the menu's symlink, and `seaquel-cli` on Linux and Windows, where the GUI owns `seaquel`. That matches the design doc but gives different docs per platform.
   - Tasks 8 and 10 change.
2. **The macOS keychain prompt** (finding 2).
   - **Recommendation: accept it and explain it.** The first query on a connection with a saved password shows one macOS prompt per item. The settings panel and docs say to choose "Always Allow". Until the user answers, a tool call that needs the secret waits. If the user denies it, the tool call fails with a message naming the connection and the fix.
   - **The alternative:** the CLI asks a running app for secrets over a local socket. That means no prompts, but only while the app is open. It also opens a new local IPC surface, with its own authentication, to design and review, which is a phase of its own.
   - Tasks 3 and 9 change.
3. **Which connections does the MCP server expose?**
   - **Recommendation: only the ones named on the command line.**
     - `seaquel-cli mcp --connection <name or id>` can be repeated. `--project <name>` exposes a project's connections.
     - With neither flag, the server starts with no connections, and `list_connections` explains how to add some.
     - The settings panel builds the command line with checkboxes.
     - Rows whose `ai_share_schema` or `ai_share_data` is off (after the global default) keep those limits in the MCP tools too.
   - **The alternative:** expose every saved connection by default. That's convenient, but it hands an MCP host, and the model behind it, every database the user has ever saved, across all projects.
   - Tasks 6 and 9 change.

## Decisions (2026-09-30)

### 1. Crates and the binary

| Crate | Kind (`check-crate-deps`) | Holds |
|---|---|---|
| `seaquel-workspace` | domain | `connections::build_config(row, secrets) -> ConnectConfig`, the ported connection-string helpers, and later phase 5's services |
| `seaquel-mcp` | interface | the rmcp `ServerHandler`, tool arguments (serde + schemars) and the result formatting |
| `seaquel-cli` | interface | `main.rs`: clap with the `mcp` subcommand, `--version` and `--help` (with a line pointing to `seaquel.app/terms`, decision 14), and stderr logging. The binary is named `seaquel-cli` |

- Core's features gain `workspace`. The CLI enables `storage`, `secrets`, `ssh`, `workspace` and the five engines. It doesn't enable `git` or either `license-*` feature, because the CLI doesn't check licenses (decision 14).
- `seaquel-mcp` and `seaquel-cli` depend on Core, rpc and types only, following rule 3. `seaquel-workspace` is reached through Core's re-export.

### 2. `Workspace::connect_saved`

```rust
impl Workspace {
    /// Load the connection, read its secrets, open an SSH tunnel if it has one,
    /// and open it on Core. Returns Core's connection id; the tunnel lives as
    /// long as the connection (closed on disconnect and on Core drop).
    pub async fn connect_saved(&self, core: &Core, id: &str, policy: HostKeyPolicy) -> Result<String, CoreError>;
}
pub enum HostKeyPolicy { KnownOnly /* MCP */ , Trust(String) /* GUI, later */ }
```

- **`build_config` is a pure function.** It takes the row plus `Secrets { db, ssh, ssh_key }` and returns a `ConnectConfig`. It is tested against frozen fixtures recorded from the TypeScript (Task 2), the same way phases 1, 2b and 3 did.
- **Secrets follow the TS rules.** `db:<id>` is read only when `savePassword` is set. `ssh:<id>` and `ssh-key:<id>` are read only when their own flags are set. SQLite and DuckDB never need a password. A missing required secret gives `CREDENTIALS_REQUIRED`, and the message names the connection and says to save the password in the app.
- **SSH uses `HostKeyPolicy::KnownOnly`.** An unknown host gives `UNKNOWN_HOST_KEY`, and the message says to connect once in the app. The MCP server never writes `known_hosts`.
- **The GUI keeps its TypeScript connect path in phase 4.** Phase 5 moves `ConnectionService` into Core and switches the GUI to `connect_saved`. Until then the fixtures keep the two paths in step.

### 3. Storage from a second process

- **The MCP server opens `seaquel.db` read-only.** A new `StorageOptions { read_only: true }` opens with `mode=ro` and runs no baseline, migrations or data steps.
- **If the file isn't up to date, it refuses.** This covers a pending baseline, an unapplied migration and a pending data step. The error is `STORAGE_NEEDS_UPGRADE`: "Open the Seaquel app once to update your data." The bundled CLI and the app are the same version, so in practice the app has always opened the file first.
- **What this gives up, on purpose.** Phase 4 doesn't write the metadata file: no query history from MCP and no saved-query edits. There is no second writer yet, so the design's `data_version` polling and `StorageChanged` event aren't needed. They move to the phase that first adds a second writer (phase 7's `seaquel conn add`, or phase 5). Removing the "second writer" risk is simpler than building change events nothing would consume.
- **The migration race still gets fixed in this phase, because it exists today on web** (phase 3 follow-up). Two pools can open one file, for example an evicted workspace still finishing a request next to a fresh one. Both can then run a pending SQL migration. `Storage::open` now holds a `BEGIN IMMEDIATE` lock while it checks and runs migrations, and it re-checks after taking the lock. That serialises the migrator the way data steps already are.
- **Data dir.** `data_dir("app.seaquel.desktop")`, or `.dev` in a debug build of the CLI; `SEAQUEL_DATA_DIR` overrides both. The keychain service is `app.seaquel.desktop` in both, as in the app.

### 4. Row limits (the AI-safety follow-up)

- **`QueryOptions` gains `max_rows: Option<usize>`,** which is valid only with `read_only`. When it's set, Core passes it to `Driver::query_read_only`. The engine stops after `max_rows + 1` rows, returns the first `max_rows`, and sets `truncated: true` on the final batch. Without it, behaviour doesn't change: `RESULT_TOO_LARGE` at 100,000 rows.
- **What each engine changes:**
  - **Postgres, MySQL and SQLite:** `fetch_capped` takes the cap and a mode (error or truncate).
  - **DuckDB:** the wrapper's `LIMIT` becomes `max_rows + 1`.
  - **MSSQL:** `run_query_drained` stops and drains at the cap, as it already does for the row cap. Escape detection still runs.
- **The in-app AI's `run_query` uses it too,** with 1,000 rows, because the model only sees the first five. This closes the AI-safety follow-up.

### 5. The MCP tools (first cut)

Each tool resolves `connection` against the exposed set only (open question 3). A connection is opened on first use and cached for the server's lifetime.

| Tool | Arguments | Result | Refused when |
|---|---|---|---|
| `list_connections` | — | name, id, engine, project, `schema` and `data` sharing flags | — |
| `list_schemas` | `connection` | schema names | schema sharing is off |
| `list_tables` | `connection`, `schema?` | tables and views with type | schema sharing is off |
| `describe_table` | `connection`, `schema?`, `table` | columns (name, type, nullable, default, PK), indexes, foreign keys | schema sharing is off |
| `run_query` | `connection`, `sql`, `max_rows?` (default 100, max 1,000) | columns, rows as JSON (tagged values rendered with `cellText` rules), `truncated` | data sharing is off; any statement the read-only check or the database refuses |
| `explain_query` | `connection`, `sql` | the engine's plan text | data sharing is off; the SQL fails the read-only check |
| `list_saved_queries` | `connection?`, `project?` | name, id, description, parameters | — |
| `run_saved_query` | `connection`, `saved_query`, `params?` (object) | as `run_query` | as `run_query`; missing or extra parameters |

- **Reads only.** `run_query` goes through `query_stream` with `read_only` and `max_rows`, which gives the token check and database enforcement. There is no write opt-in in phase 4. The design's per-connection write opt-in comes with a later phase and its own review.
- **Saved queries.** `run_saved_query` substitutes `{{params}}` with `seaquel_sql::params` exactly as the editor does. The result is still read-only.
- **Tool errors** come back as MCP tool errors (`isError: true`) with Core's code and message, never as protocol errors. Messages never contain secrets.
- **Dashboard tools come later.** They wait until dashboards are in Core (phase 5).

### 6. Distribution

- **Sidecar build.**
  - `tauri.conf.json` gets `"externalBin": ["binaries/seaquel-cli"]`.
  - A script, `scripts/build-cli.mjs`, runs `cargo build [--release] -p seaquel-cli --target <triple>` and copies the result to `src-tauri/binaries/seaquel-cli-<triple>[.exe]`. The binaries are gitignored.
  - `tauri:dev` and `tauri build` run it first through `beforeDevCommand`/`beforeBuildCommand`, or through the npm scripts. tauri-build fails when the file is missing.
- **`release.yml`.** One step per target, before tauri-action, runs the script with `--release --target ${{ matrix.target }}`. The bundler signs sidecars along with the app: codesign with hardened runtime on macOS, and `signCommand` on Windows. Task 8 confirms this on a draft release.
- **Putting the command on `PATH`, per platform:**
  - **macOS:** the app menu gets "Install Command Line Tool…". It symlinks `/usr/local/bin/seaquel-cli` to `Contents/MacOS/seaquel-cli`, and asks for admin rights through `osascript … with administrator privileges` only when needed. A second click reports "already installed".
  - **Linux:** deb and rpm put it in `/usr/bin/seaquel-cli` as part of how Tauri installs sidecars, so nothing more is needed. The AppImage gets the same menu item, linking into `~/.local/bin`.
  - **Windows:** phase 4 doesn't change `PATH`. The settings panel shows the full path, which is all an MCP host needs. Adding it to `PATH` through NSIS and WiX hooks is a follow-up.
- **Settings.** A new "MCP" settings panel shows:
  - the binary's path, and whether it's on `PATH`;
  - checkboxes for connections or a project;
  - the resulting command line;
  - ready-to-paste JSON for Claude Desktop (`mcpServers`) and a `claude mcp add` line for Claude Code, each with a copy button;
  - the keychain note on macOS.

### 7. Trust boundary

The MCP host, and the model behind it, controls every tool argument. What stops it:

| Threat | Guard |
|---|---|
| A write through `run_query` or `run_saved_query` | the read-only check plus the database's read-only mode (the AI-safety phase). The per-engine gaps listed in that plan apply here too |
| Reaching a connection the user didn't expose | resolution only against the exposed set. Names and ids are matched exactly. No tool takes a connection string, host, path or driver |
| A huge result or a slow query | `max_rows` ≤ 1,000; the driver stops fetching once the rows it kept hold 8 MB (`QueryOptions::max_bytes`; one cell can still be as large as the database allows); a cell's text cut at 64 KB and a result's rows stopped before its JSON passes ~4 MB; and a per-call timeout (60 s, not counting a pending keychain prompt, then the query is cancelled) |
| Secrets in results or errors | Core's error messages already exclude them. The MCP layer adds a redaction test over every error path |
| Files through SQLite or DuckDB | these engines open only the saved path. The MCP server opens DuckDB restricted (`ConnectSavedOptions::restricted`): external access, extension autoinstall and autoload are off and the configuration is locked, so `read_*`, a path as a table, `glob` and `COPY` can't reach files beyond the database and SQL can't turn that back on. The in-app AI keeps the documented gap: its DuckDB instance is the editor's, whose file functions need external access |

Task 7 is a probe of this boundary, written into the plan rather than left to review, as the phase 3 cost notes asked.

---

## Ground rules

- **Git and branches.** No git writes: the owner forbids `git add`, `commit`, `mv`, `stash`, branches and worktrees. Read-only git is fine. Each task ends with a checkpoint summary.
- **Repo conventions.**
  - Never edit `src/lib/components/ui/*`.
  - Error toasts use `errorToast`.
  - Run svelte-autofixer on changed `.svelte` files, and oxfmt on TS.
  - New `messages/en.json` keys get translated with the `i18n-translator` agent.
- **Crates.**
  - Core may not use `tokio::spawn`, `Instant` or `SystemTime`.
  - Infra and interface crates are native-only.
  - Keep `npm run crates:check` green.
- **Parallel agents.** Each agent owns the files its task names and uses its own `CARGO_TARGET_DIR`. Shared files are:
  - Core `lib.rs`/`Cargo.toml`
  - `seaquel-rpc`
  - `src-tauri/src/lib.rs`
  - the root `Cargo.toml`
  
  Edit them with small exact changes, re-reading each one first. Run rustfmt only on your own files.
- **User data.**
  - Tests never open the real data dir, the real keychain service or `~/.ssh`.
  - Use temp dirs, `MemoryStore` and temp `known_hosts`.
  - The keychain tests stay gated by `SEAQUEL_TEST_KEYCHAIN=1`.
- **Secrets.** No secret in `Debug`, errors, logs or MCP output. Test the redaction.
- **stdout belongs to the MCP protocol.** Logs go to stderr only. A test runs the binary and checks that stdout carries only JSON-RPC.
- **Full check list** (the phase 3 list, plus the new crates):
  - `npm run crates:check`
  - `cargo fmt --all --check`
  - workspace clippy with `-D warnings`
  - both wasm32 clippy lines
  - `cargo test --workspace --exclude seaquel` with the live env (all `SEAQUEL_TEST_*` including `SSH`, and `REQUIRE_ENGINES=1`)
  - `cargo check -p seaquel` and seaquel clippy
  - `npm run wasm:build`
  - `npm run check`
  - `CI=1 npx vitest run`
  - oxlint
  - the three builds
  - `npm run tauri build` on the owner's Mac once Task 8 lands, to check the sidecar in the bundle
- **Effort log:** append to `docs/plans/2026-09-30-phase-4-effort.md`.

---

## Order and estimates

| # | Task | Estimate | Needs | Alongside |
|---|---|---|---|---|
| 1 | Crates, rules, clap/rmcp pins | 0.5–0.75 h | — | 2 |
| 2 | Freeze the TS connect-config baseline | 1–1.5 h | — | 1 |
| 3 | `seaquel-workspace::connections` + `connect_saved` | 2–3 h | 1, 2 | 4, 5 |
| 4 | Read-only storage open, migration lock | 1–1.5 h | 1 | 3, 5 |
| 5 | `max_rows` on read-only queries (Core + 5 engines + in-app AI) | 2–3 h | — | 3, 4 |
| 6 | `seaquel-mcp` tools + `seaquel-cli mcp` | 3–4 h | 3, 4, 5 | — |
| 7 | Trust-boundary probe | 1–1.5 h | 6 | 8 |
| 8 | Sidecar build, release.yml, menu item, PATH | 2–3 h | 6 | 7, 9 |
| 9 | MCP settings panel | 1–1.5 h | 6 | 7, 8 |
| 10 | Docs, measure, checkpoint | 0.75–1 h | all | — |
| | Review fixes (40%: another process and an LLM reach this) | 6–8 h | | |
| | **Total** | **~21–29 h** | | |

By phase 3's numbers, a first pass runs at about half its estimate where the planned mechanism holds and at about the estimate where it doesn't. So expect roughly 13–17 h logged. The riskiest tasks are:
- **Task 8:** the signing and notarization of a sidecar can only be confirmed on a real release run;
- **Task 5:** it touches every engine's read-only path.

---

## Part A — Groundwork

### Task 1: Crates, rules, pins

**Files:**
- Create: `crates/seaquel-{workspace,mcp,cli}/{Cargo.toml,src/lib.rs}`, and `crates/seaquel-cli/src/main.rs` with the `[[bin]] name = "seaquel-cli"`.
- Modify:
  - the root `Cargo.toml`: members, plus workspace deps `rmcp` (pin the current version from crates.io, with the `server`, `macros` and `transport-io` features; `client` and `transport-child-process` go in dev), `clap = { version = "4", features = ["derive"] }` and `schemars = "1"`;
  - `scripts/check-crate-deps.mjs`: `seaquel-workspace` in `DOMAIN_AND_INFRA`, and `seaquel-mcp` and `seaquel-cli` in `INTERFACES`;
  - Core's `workspace` feature.
- Create: `docs/plans/2026-09-30-phase-4-effort.md`.

**Steps:**
1. Write failing cases in `scripts/check-crate-deps.test.mjs`:
   - `seaquel-cli` depending on `seaquel-workspace` directly is rejected;
   - `seaquel-workspace` naming an engine crate is rejected.
2. Add the crates. `seaquel-cli --version` prints the version and the terms line. `seaquel-cli mcp` exits with "not implemented yet" on stderr.
3. Record the rmcp version and its API surface (the tool macro, `stdio()`, the client) in the effort log for Task 6.

### Task 2: Freeze the TS connect-config baseline

**Files:** `docs/plans/artifacts/2026-09-30-freeze-connect-config.mjs.txt` and `crates/seaquel-workspace/tests/fixtures/connect-config/*.json` with a README (frozen, same rules as phase 3's).

**Steps:**
1. Bundle today's `wire.ts` (`toRustConfig`), `connection-string.ts` (`buildConnectionString`, `getConnectionData`'s rebuild rule) and the reconnect steps in `connection-manager.svelte.ts`: password reinjection, the SSH host/port rewrite and the URL-username fallback. Use esbuild, as phase 3's recorder did. Where the logic is inline in a Svelte class, extract it verbatim into a scratch module, and say so in the README.
2. Record at least 60 cases: row, secrets and tunnel port in, `ConnectConfig` out. Cover:
   - each engine, and each ssl mode;
   - default and custom ports;
   - a string present, missing, or stripped of its password;
   - key=value MSSQL-style strings;
   - a TablePlus URL;
   - unicode and URL-special characters in the user and password;
   - SSH on and off;
   - imported shared rows with no string;
   - SQLite and DuckDB paths, including `:memory:`;
   - MariaDB;
   - an empty username.
3. Record the secret-selection rules as cases too: which keys are read under which flags, and when the TS gives up.

## Part B — Core pieces

### Task 3: `seaquel-workspace::connections` and `connect_saved`

**Files:** `crates/seaquel-workspace/src/{lib.rs,connections.rs,connection_string.rs}` and `tests/connect_config.rs`; Core `workspace.rs` (`connect_saved`, `HostKeyPolicy`); `crates/seaquel-core/tests/connect_saved.rs`.

**Steps:**
1. `connect_config.rs` replays every Task 2 case. Write it first.
2. Port `build_config` and the helpers it needs. Keep the TS's quirks: the fixtures define correct behaviour. List any quirk that looks like a bug in the effort log and leave it.
3. `connect_saved`:
   - read the row through storage;
   - read secrets through the workspace's `SecretStore` according to the flags;
   - open the tunnel with `KnownOnly`;
   - call `Core::connect`;
   - tie the tunnel to the connection: it's closed on disconnect, on a failed connect and on Core drop.
4. Live tests, one per server engine, against the Docker databases:
   - saved rows in a temp storage file, secrets in `MemoryStore`, and a query;
   - one through the SSH container with a temp `known_hosts` already holding the key;
   - an unknown host key gives `UNKNOWN_HOST_KEY`, and nothing is written;
   - missing secrets give `CREDENTIALS_REQUIRED`.

### Task 4: Read-only storage open; migration lock

**Files:** `crates/seaquel-storage/src/open.rs`, `tests/open.rs`, `migrations/README.md`.

**Steps:**
1. Write the failing tests first:
   - A read-only open of an up-to-date file works and can't write. An insert through the pool fails.
   - A read-only open of a file that needs the baseline, a migration or a data step gives `STORAGE_NEEDS_UPGRADE` and leaves the file byte-identical.
   - A read-only open while another pool holds a write transaction still reads (WAL).
   - Two pools race to open a file with a pending test migration: exactly one applies it and both opens succeed. Use a migration added only in the test.
2. Implement `StorageOptions::read_only` (sqlx `read_only(true)` and no journal-mode change) and the `BEGIN IMMEDIATE` migrator lock.

### Task 5: `max_rows` on read-only queries

**Files:**
- Rust: `crates/seaquel-core/src/lib.rs` (`QueryOptions`), `crates/seaquel-engine/src/{lib.rs,sqlx_driver.rs}` (`fetch_capped` mode), each engine's `query_read_only` and its `tests/read_only.rs`, and the `StreamEvent` final batch (`truncated`).
- Types: `seaquel-types` and `npm run types:gen`.
- TS: the in-app AI's `run_query` (`src/lib/services/ai`, `QueryCrud.executeReadOnly`, the providers' `selectReadOnly`).

**Steps:**
1. For each engine, add a live test: a query over 5,000 rows with `max_rows = 10` returns 10 rows and `truncated`; with 3 rows it returns `truncated: false`; without `max_rows` it keeps `RESULT_TOO_LARGE` at the cap. On MSSQL, escape detection still fires with `max_rows` set.
2. Implement it, in one place per engine.
3. The in-app AI passes 1,000. The tool result tells the model when rows were truncated. Update the AI tests.

## Part C — The MCP server

### Task 6: `seaquel-mcp` tools and `seaquel-cli mcp`

**Files:** `crates/seaquel-mcp/src/{lib.rs,server.rs,tools/*.rs,format.rs}`, `crates/seaquel-mcp/tests/{tools.rs,stdio.rs}`, `crates/seaquel-cli/src/main.rs`.

**Steps:**
1. **Tests first.** They run in-process over an rmcp client on a `tokio::io::duplex` pair, against a temp workspace holding Postgres, SQLite and DuckDB saved connections. They cover:
   - each tool's happy path;
   - every refusal in Decision 5's table;
   - saved-query parameters;
   - truncation;
   - an unknown connection;
   - the exposed-set filter.
   
   `stdio.rs` spawns the built binary and checks the handshake, a tool call, and that stdout carries only JSON-RPC while logs go to stderr.
2. **Implement the tools.** Build the exposed set from `--connection`/`--project` against `connections::load_all` and `projects::load_all`. Honour the AI sharing flags after the global default, which is read from `app_state`'s AI settings the way the GUI reads it.
3. **Results.** Cell values render the same way as `$lib/values` `cellText`: bigint and decimal as strings, and bytes as base64 with a marker. Port the rule; don't guess it. Keep results compact: columns once, then rows as arrays.
4. **`seaquel-cli mcp`:**
   - opens the workspace read-only (Decision 3) with `KeychainStore`;
   - builds Core with the SSH known_hosts default;
   - serves stdio, and exits cleanly when stdin closes, closing its connections and tunnels;
   - `--log-level` sends logs to stderr.

### Task 7: Trust-boundary probe

A separate agent attacks the server through the MCP client only, with Task 6's temp workspace and the live databases. The probe must try to:
- write through `run_query` and `run_saved_query` on each engine, reusing the AI-safety attack lists;
- reach an unexposed connection by name tricks: case, whitespace, id vs name, unicode lookalikes;
- get a secret into any result or error;
- make the server write to `seaquel.db` or `known_hosts`;
- exceed `max_rows` or the timeout;
- break stdout framing, for example with a result containing a newline or huge output.

Every hole found gets fixed in its task and a regression test. The probe's findings go into this plan as a findings block.

## Part D — Shipping

### Task 8: Sidecar build, release, menu, PATH

**Files:**
- `scripts/build-cli.mjs`
- `package.json` (`tauri:dev`, `tauri build` and the scripts that run first)
- `src-tauri/tauri.conf.json` (`externalBin`)
- `.gitignore` (`src-tauri/binaries/`)
- `.github/workflows/release.yml`
- `src-tauri/src/lib.rs` (the menu item and the symlink install, with a Rust unit test on the path logic)
- `src-tauri/capabilities` if needed

**Steps:**
1. **`build-cli.mjs`.** It detects the host triple (`rustc -vV`), or takes `--target`. It builds in debug or release and copies the binary with the `-<triple>` suffix. When `CARGO_TARGET_DIR` is set, it must honour it.
2. **Wire it into the dev and build scripts.** `npm run tauri dev` and `npm run tauri build` must work from a clean checkout.
3. **`release.yml`.**
   - Add the step, using `shell: bash` so it works on Windows.
   - Check how the bundler signs sidecars: read the tauri-bundler source for the pinned version.
   - Record what the owner must check on the first draft release: `codesign -dv --verbose=4` on the sidecar inside the notarized app, and `signtool verify` on Windows.
4. **Menu item.**
   - The macOS menu gets "Install Command Line Tool…", with the Linux AppImage variant as in Decision 6.
   - The item reports success, "already installed", or the error, through a dialog.
   - No shell-out except `osascript` for the admin prompt. Build the symlink command from fixed paths only, never from user input.

### Task 9: MCP settings panel

**Files:**
- `src/lib/components/settings/mcp/*.svelte`
- the settings navigation
- a small Tauri command or a `core_call` group method that returns the sidecar's path and whether it's on `PATH`. Prefer a `core_call` group, so the command list stays small.
- `messages/en.json`, then translate.

**Steps:**
1. Show what Decision 6 lists:
   - the binary path;
   - the connection and project checkboxes;
   - the generated command;
   - the Claude Desktop JSON and the `claude mcp add` line, each with a copy button;
   - the macOS keychain note;
   - on Linux AppImage and macOS, the install button, which uses the same code as the menu.
2. Add vitest cases for the snippet generation, including quoting of names with spaces and quotes. On web and in the demo the panel is hidden.

### Task 10: Docs, measure, checkpoint

- **CLAUDE.md:** the new crates, `connect_saved`, read-only storage opens, `max_rows`, the MCP server's rules (stdout, the exposed set, no writes), and the build and PATH scripts.
- **README:** an "MCP server" section covering setup with Claude Desktop and Claude Code, the keychain prompt, and the exposed-set flags.
- **Design doc:**
  - a status line;
  - a "Phase 4 cost" section;
  - update "Storage ownership": `StorageChanged` is deferred, and phase 4 doesn't write storage;
  - update "Terminal binaries": the `seaquel-cli` name, Windows `PATH` deferred, and the keychain prompt instead of access groups.
- **This plan:** execution notes and release notes.
- **Effort log:** the totals.
- **Checkpoint.**

**Status (Task 10):** done. CLAUDE.md, README.md (an "MCP server" section), the design doc's status line, "Storage ownership" and "Terminal binaries" notes and "Phase 4 cost", the execution notes, release notes and follow-ups below, and the effort log's totals are written. The full check list passed (effort log, Task 10). The manual checks below are not run yet.

---

## Manual checks

For the owner. Checks marked "signed build" need a build from `release.yml` (a draft release); the others work with `npm run tauri build` on your Mac. The test databases are `npm run e2e:db:up` plus `npm run e2e:db:seed`; the SSH container is user `seaquel`, password `seaquel-test-password`, on `127.0.0.1:2222`, and reaches Postgres as `postgres:5432`. The CLI below is `/Applications/Seaquel.app/Contents/MacOS/seaquel-cli` (or `seaquel-cli` once installed on `PATH`). To see saved connection ids: `sqlite3 -readonly "$HOME/Library/Application Support/app.seaquel.desktop/seaquel.db" 'select id, name, project_id from connections'`.

- **Claude Desktop.** In the app, turn on "Allow AI to run read-only queries" for one Postgres connection (its AI settings, or Settings → AI). Open Settings → MCP, check that connection, copy the Claude Desktop JSON into Claude Desktop's config (Settings → Developer → Edit Config), and restart Claude Desktop. Then:
  - Ask it to list the tables and count the rows of one. It works, and the tool calls show `list_tables` and `run_query`.
  - Ask it to delete a row. The tool result is `READ_ONLY: …`, and `select count(*)` in the app shows the row is still there.
  - Ask about a connection you didn't check. `list_connections` doesn't show it, and naming it gives `CONNECTION_NOT_FOUND`.
  - Turn data sharing off for the connection in the app and ask for a count again, without restarting: `DATA_SHARING_OFF`.
- **Claude Code.** Copy the `claude mcp add seaquel -- …` line from Settings → MCP and run it in a terminal. `claude mcp list` shows `seaquel` as connected. In `claude`, `/mcp` lists the eight tools, and asking for a row count works. Remove it with `claude mcp remove seaquel`.
- **Keychain prompt (macOS, signed build).** Expose a connection with a saved password (for SQL Server: `sa`/`Seaquel_Test_123!` on `127.0.0.1:1433`, SSL mode `prefer`). The first query shows one macOS prompt asking whether `seaquel-cli` may use the item; the call waits while it's open. Choose "Always Allow": the query runs, and after restarting the MCP host the next query doesn't ask. Then in Keychain Access, find the `app.seaquel.desktop` item for `db:<id>`, remove `seaquel-cli` from its Access Control, query again and choose Deny: the tool result is `SECRET_UNREADABLE` and names the connection. An unsigned local build asks again after every rebuild, as expected.
- **SSH, known host.** In the app, add a Postgres connection with host `postgres`, port 5432, database `seaquel_test`, user `postgres`, through SSH `127.0.0.1:2222` as `seaquel` with the password saved. Connect and approve the host key. Expose it in Settings → MCP and query it from Claude Desktop: it works. `shasum ~/.ssh/known_hosts` is the same before and after.
- **SSH, unknown host.** `cp ~/.ssh/known_hosts /tmp/kh.bak && ssh-keygen -R '[127.0.0.1]:2222'`, restart the MCP host and query the same connection: the tool result is `UNKNOWN_HOST_KEY`, saying to connect once in the Seaquel app and trust the key, and `grep -c '127.0.0.1\]:2222' ~/.ssh/known_hosts` stays `0`. Restore with `cp /tmp/kh.bak ~/.ssh/known_hosts`.
- **Install Command Line Tool (macOS).** With nothing at `/usr/local/bin/seaquel-cli`, choose Seaquel → Install Command Line Tool…: an admin prompt, then a success dialog. `ls -l /usr/local/bin/seaquel-cli` points into `Seaquel.app/Contents/MacOS/`, and `seaquel-cli --version` prints the app's version and the terms line. Choose it again: "already installed", with no prompt. Settings → MCP now says it's installed. Remove it with `sudo rm /usr/local/bin/seaquel-cli`. On a Linux AppImage the same item links `~/.local/bin/seaquel-cli` with no prompt; on a deb, `dpkg -L seaquel | grep seaquel-cli` shows `/usr/bin/seaquel-cli` and there's no item.
- **Upgrade order.** Make a data dir that still needs an upgrade and point both programs at it:
  ```bash
  mkdir -p /tmp/sq-up && sqlite3 /tmp/sq-up/seaquel.db < crates/seaquel-storage/tests/fixtures/schemas/v2026.9.2.sql
  SEAQUEL_DATA_DIR=/tmp/sq-up /Applications/Seaquel.app/Contents/MacOS/seaquel-cli mcp < /dev/null; echo "exit $?"
  ```
  It prints `STORAGE_NEEDS_UPGRADE: … Open the Seaquel app once to update your data` on stderr, exits non-zero, and `shasum /tmp/sq-up/seaquel.db` is unchanged. Then start the app on it once (`SEAQUEL_DATA_DIR=/tmp/sq-up /Applications/Seaquel.app/Contents/MacOS/seaquel`), quit, and run the CLI line again: it starts and waits on stdin with no error (Ctrl+C ends it). `rm -rf /tmp/sq-up` afterwards.
- **AI in the app.** On a table with well over 1,000 rows (`generate_series` in Postgres: `create table big as select g from generate_series(1, 50000) g`), ask the assistant for all its rows, then ask how many rows it received. The query runs without an error, and the assistant says the result was cut at 1,000 rather than calling 1,000 the table's size (the tool result starts with "The result was truncated").
- **SQL Server `verify-full`.** Against the test container, whose certificate is self-signed: a SQL Server connection to `127.0.0.1:1433` (`sa`/`Seaquel_Test_123!`) with SSL mode `verify-full` now fails with a certificate error, where it used to connect; `prefer` connects. If you have a SQL Server with a certificate your Mac trusts, `verify-full` to its real host name connects.
- **A password with `%`.** `docker exec seaquel-mysql mysql -uroot -e "CREATE USER 'pct'@'%' IDENTIFIED BY '50%off+a&b'; GRANT SELECT ON seaquel_test.* TO 'pct'@'%';"`. Add a MySQL connection to `127.0.0.1:3306`, database `seaquel_test`, user `pct`, that password saved. Quit and reopen the app: it reconnects by itself and a query works (before this release the reconnect failed). Expose it over MCP and query it: it works too. Clean up with `docker exec seaquel-mysql mysql -uroot -e "DROP USER 'pct'@'%'"`.
- **Signing on the first draft release** (Task 8). Download the draft's macOS build, then:
  - `codesign -dv --verbose=4 /Applications/Seaquel.app/Contents/MacOS/seaquel-cli` shows the Developer ID authority and `flags=0x10000(runtime)`;
  - `codesign --verify --deep --strict --verbose=2 /Applications/Seaquel.app` and `spctl -a -vv /Applications/Seaquel.app` pass, and `xcrun stapler validate /Applications/Seaquel.app` works;
  - `/Applications/Seaquel.app/Contents/MacOS/seaquel-cli --version` runs with no Gatekeeper prompt.

  On Windows, `signtool verify /pa /v "C:\Program Files\Seaquel\seaquel-cli.exe"` (or wherever the installer put `Seaquel.exe`) verifies, and `seaquel-cli.exe --version` runs. On Linux, `seaquel-cli --version` runs from `/usr/bin` (deb, rpm) and from the AppImage after the menu install.

---

## Execution notes (2026-09-26)

The plan was executed task by task with subagents, Tasks 1–2, 3–5 and 7–9 partly in parallel, with a review after most tasks and a round of fixes. Task 7's probe ran twice, the second time after the fixes the first run led to. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-09-30-phase-4-effort.md`; the measured cost is in the design doc ("Phase 4 cost").

**What went differently from the plan**

- **Core re-exports the domain crate as `seaquel_core::domain`** (Task 1), since Core already has a private `workspace` module. It also re-exports `seaquel_sql` as `seaquel_core::sql`, for saved-query parameters. `seaquel-mcp` is an interface library that only `seaquel-cli` may depend on (`INTERFACE_LIBS` in `check-crate-deps.mjs`), and neither depends on `seaquel-rpc`.
- **`--version` prints the desktop app's version** (Task 1): `build.rs` reads it from `src-tauri/Cargo.toml`, since the two ship together.
- **The GUI has two connect paths, and the fixtures record which one each row takes** (Task 2). autoReconnect never connects a Postgres, MySQL, MariaDB or SQLite row without a stored string; only the reconnect tab's rebuild does, which covers every shared import. The 107 cases are autoReconnect's config (71), the tab's rebuild (24) and `CREDENTIALS_REQUIRED` (12).
- **`build_config` isn't one call** (Task 3): the tunnel's port is known only after it opens, so it's `read_secrets` → `tunnel_config` → open → `build_config`. `connect_saved` takes `impl Into<ConnectSavedOptions>`, and unlike the TS it refuses when the keychain refuses a read (`SECRET_UNREADABLE`) instead of connecting without the password.
- **"Up to date" is three read-only checks** (Task 4): each baseline step's own condition (`schema::is_current`), the migrator's table, and pending data steps. A WAL file with no `-wal`/`-shm` is checked with `immutable`, so the check leaves no sidecar files behind. The migrator lock uses sqlx's `run_direct` on the locked connection, since sqlx's migrate lock is a no-op on SQLite.
- **Row limits are a `CappedResult` and a `RowCap`** (Task 5), not a field on `QueryResult`. `truncated` is on `StreamBatch`, left out when false. Postgres skips its `ROLLBACK` after a truncation, which took 4.5 s on 20M rows.
- **Cells render bytes as `\x` hex**, as `cellText` does, not base64 as Decision 5 guessed (Task 6). `run_saved_query` also takes `max_rows`. `list_saved_queries` never returns SQL and lists a project's queries only when one of its exposed connections shares its schema.
- **seaquel-mcp has its own stdio transport** (Task 6 review): rmcp 3.4.1 drops a line that isn't JSON without a reply. `stdio.rs` lives in seaquel-cli, the crate that owns the binary.
- **Decision 7 grew after the probe** (Task 7). Core's `QueryOptions` gained `timeout` and `max_bytes`, and Core gained `explain_read_only`; the MCP server opens DuckDB restricted, with `json` linked statically. `icu` couldn't follow (see Follow-ups).
- **The settings panel uses two Tauri commands, `cli_info` and `install_cli`**, not a `core_call` group (Task 9): both answer from the app bundle and show native dialogs, which `seaquel-rpc` can't reach.
- **`npm run tauri` goes through `scripts/tauri.mjs`** (Task 8 follow-up). `tauri dev` waits at most ~180 s for Vite and the sidecar now builds first, so on a cold target dir the first run timed out. The wrapper builds the sidecar before Tauri starts. `build-cli.mjs` also sets `MACOSX_DEPLOYMENT_TARGET` as `tauri build` does, so the release build compiles DuckDB once.
- **The disk filled up during Task 8** (other agents' target directories, up to 56 GB each). Its first pass couldn't build or test anything; it was rerun on a clean target dir after the disk was freed.

**Task 7 findings.** Run 1 made its calls through an MCP client against a temp workspace with the five server engines, SQLite and DuckDB, snapshotting the databases, `seaquel.db` and `known_hosts` around each case. Run 2 repeated it after the fixes: 2,085 tool calls in 29 case groups.

- **Critical: SQL Server `\r` comments.** `SELECT 1 AS a -- x\rDELETE … COMMIT COMMIT` passed the read-only check and reached SQL Server, which ends the comment at the `\r`: the scanner ended `--` at a lone `\r` only on Postgres and DuckDB. The in-app AI had the same hole. Fixed in `seaquel-sql` (fix 20 in `bugfixes.json`).
- **EXPLAIN wrote.** Run 1 advanced a MariaDB sequence through a plain EXPLAIN (`NEXTVAL` in a derived table); Postgres commits what a folded function does while planning, and SQLite and DuckDB ran statements after the first. Fixed with `explain_read_only`.
- **DuckDB reached the file system.** `read_csv`, `read_text`, `glob`, a path as a table and `ATTACH` read files outside the database, and run 1 left an autoinstalled `sqlite_scanner` extension in `~/.duckdb`. Fixed by the restricted instance.
- **A timed-out query kept running** on Postgres and MySQL after the call gave up. Fixed with the server-side timeout and cancel.
- **Memory was bounded only by rows**: 1,000 rows of 1 MB took 1.2 GB. Fixed with `max_bytes`.
- **Held in both runs:** reaching an unexposed connection through name tricks (case, whitespace, an id for a name); secrets in results, errors or stderr at trace level; writes to `seaquel.db` or `known_hosts` (only the WAL `-shm` file's timestamp changed); and stdout framing with newlines and oversized results. In run 2 the database and file snapshots stayed the same, and none of the five issues above came back.

**Bug fixes per area.** Older than phase 4 unless marked new:

- **Connecting:** passwords with `%`, `+`, `&`, `$` or `,` broke the reconnect (Task 2, fixed in the TS too); SQL Server `verify-ca` and `verify-full` accepted any certificate (Task 2, fixed in the TS too); `ConnectConfig`'s `Debug` printed the password and connection string (Task 3); a failed keychain read connecting without the password, short secrets redacted inside ordinary text, and a dropped `disconnect` leaking its tunnel (Task 3 review, new).
- **Read-only queries:** the SQL Server `\r` bypass, DuckDB file access and a query outliving its cancel on Postgres and MySQL (probe); EXPLAIN writing and no byte budget (probe, new with MCP); a slow `ROLLBACK` after a truncation (Task 5, new).
- **Storage:** two pools or processes racing a pending migration (phase 3 follow-up, Task 4).
- **MCP server (all new):** no output byte limits, silent drops of malformed input, saved queries listed without schema sharing, a keychain prompt counted against the timeout, a hang after SIGTERM, unlisted `{{name}}` parameters, `sqlx::query` logging SQL at WARN, the DuckDB lockdown not wired in, and a `-32700` reply without `"id": null` (Task 6 review and follow-ons).
- **Build:** the first `tauri dev` timing out on a cold build (Task 8, new).

**Decisions made during execution**

- **Every MCP connection opens DuckDB restricted**, and the in-app AI doesn't: the lock can't be undone on the editor's running instance, whose file functions need external access. The in-app gap stays documented.
- **`json` is linked into DuckDB statically for every build**, desktop included. It costs ~2.7 s of build time and ~700 KB in the CLI binary, and makes `LOAD json` a no-op.
- **A failed keychain read fails the connect** in `connect_saved`, where the TS connects without the password; the fixtures keep the TS rule for the pure functions.
- **Test hooks exist only in debug builds** of the CLI (`cfg!(debug_assertions)`), so the shipped binary can't be pointed at a secrets file.
- **The keychain prompt pauses the call's timeout** for up to 5 minutes (`SecretWait`), since the user may be reading it.

**Release notes**

These ship in the same release as phase 3's (2026.9.2 is the last release before both), so phase 3's notes still apply as written, including upgrading from before 2026.4.5 and the `2026.4.5-beta.1` files.

New (desktop):

- **An MCP server for Claude Desktop, Claude Code and other MCP hosts.** The app now includes a command line tool, `seaquel-cli`. Its `mcp` command lets an MCP host list the connections you choose, read their schemas, run read-only queries and saved queries, and EXPLAIN a query, using the passwords and SSH settings saved in the app. Set it up in **Settings → MCP**, which builds the Claude Desktop config and the `claude mcp add` command for you.
- **It only reads.** Queries go through the same read-only check as the AI assistant and run in the database's read-only mode. Results stop at 1,000 rows (100 by default), 64 KB per cell and about 4 MB per result, and a query is cancelled after 60 seconds.
- **It sees only what you pick:** the connections and projects you check in Settings → MCP (`--connection` and `--project`), and nothing else. Each connection's AI sharing settings apply too; "Allow AI to run read-only queries" is off by default, so turn it on for the connections you want it to query.
- **On macOS the first query on a connection with a saved password asks for keychain access.** Choose "Always Allow" so it doesn't ask again.
- **SSH connections work for hosts the app already trusts.** The server never trusts a new host key; connect once in the app first.
- **DuckDB connections are locked down in the MCP server:** only the database file's own tables and views, with no other files, no extensions, and no time zone support (functions that need a time zone and `TIMESTAMPTZ` arithmetic fail). The app's own DuckDB is unchanged.
- **After installing or updating, open the app once before using the MCP server.** The server never changes your data file, so it refuses to start when an update is pending.
- **Install Command Line Tool…** in the app menu puts `seaquel-cli` on your `PATH`: a link in `/usr/local/bin` on macOS, in `~/.local/bin` for the Linux AppImage. deb and rpm packages install `/usr/bin/seaquel-cli`. On Windows the settings panel shows the full path, and nothing is added to `PATH`.

Fixes that affect the app:

- **Security: the AI's read-only check missed SQL hidden behind a carriage return on SQL Server.** In a `--` comment ended by a lone `\r`, SQL Server runs the rest of the line, and the check didn't see it, so the assistant could be led to run a write. On MySQL, MariaDB and SQLite, where the comment really runs past a `\r`, SQL with code-looking text after a `\r` inside a comment is now refused too.
- **SQL Server's `verify-ca` and `verify-full` SSL modes now check the server certificate.** They accepted any certificate before. A connection in one of those modes to a server with a self-signed or untrusted certificate now fails; use `prefer` to encrypt without checking. Through an SSH tunnel these modes now fail as `require` always has, because the name is checked against the tunnel's local address; a fix is planned.
- **Saved passwords containing `%`, `+`, `&`, `$` or `,` reconnect again.** They were put into the connection string unencoded, so `50%off` became an invalid escape and the automatic reconnect failed.
- **The AI assistant fetches at most 1,000 rows per query and says when there were more.** It used to fetch up to 100,000 rows, of which it shows the model five, and failed past that.
- **Stopping or cutting short an AI query on PostgreSQL, MySQL or MariaDB now stops it on the server.** Before, the database kept running it until it next tried to send rows.

Self-hosted web:

- **Two requests opening one user's data during an update no longer race.** Both could try to apply a pending migration, and one failed. Nothing to change in configuration.

---

## Follow-ups (not in this phase)

- **DuckDB `icu` on the MCP server.** Time zones (`current_setting('TimeZone')`) and `TIMESTAMPTZ` arithmetic fail on the restricted instance. duckdb-rs's `icu` feature needs `bundled-cmake`, which requires a duckdb-rs git checkout with the DuckDB sources the crates.io package leaves out: a git dependency, and cmake on every runner and in the Dockerfile. `seaquel-engine-duckdb/tests/restricted.rs` pins the gap and flips when icu is linked.
- **`data_version` polling and `StorageChanged`**, for the first second writer (phase 5 or phase 7's `seaquel conn add`).
- **A per-connection write opt-in for MCP**, stored in the workspace and never a tool argument, with its own review and probe.
- **Windows `PATH`** through NSIS and WiX installer hooks.
- **Secrets from a running app over IPC**, if the keychain prompt proves annoying (open question 2's alternative). A new local IPC surface with its own authentication.
- **Dashboard tools** once dashboards are in Core (phase 5), and moving the in-app assistant onto the same tool registry (phase 6).
- **The GUI's connect path moves onto `connect_saved`** (phase 5). The quirks the fixtures kept (their README) become decisions then.
- **A TLS server-name override for MSSQL through an SSH tunnel.** `require`, `verify-ca` and `verify-full` verify the certificate, but a tunnelled connection dials `127.0.0.1`, so the name check fails. Core's `ConnectConfig` needs the name to verify against (the row's host), in the app and in `connect_saved`.
- **MSSQL truncation still reads the whole result.** With `max_rows` (and `max_bytes`), SQL Server keeps the first rows but reads and drops the rest, so escape detection can run. The Task 5 review measured 3M rows truncated to 10 in 2.8 s. A big enough result hits the 60 s read-only timeout and fails instead of truncating. Bounding it needs an attention (cancel) that still leaves the connection able to run `READ_ONLY_END`.
- **The in-app AI's DuckDB can still read files.** It runs on the editor's instance, whose file functions need external access, and the lock can't be set on a running instance. It stays a documented gap (the AI safety plan's "DuckDB egress" follow-up); only a separate instance for the AI would close it.
- **Check the Arabic keychain note.** `settings_mcp_keychain_note` in `messages/ar.json` keeps "Always Allow" in English. Check the button's label on macOS in Arabic and use it if it differs.
- **Check the PowerShell snippets on Windows.** The `claude mcp add` line's quoting (single quotes, a quoted `--`, `&` before a quoted path) was checked by reasoning and vitest only, not in PowerShell through npm's `claude.ps1`.
