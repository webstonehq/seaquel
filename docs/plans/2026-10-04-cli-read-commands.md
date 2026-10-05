# CLI Commands, Part 1 Implementation Plan

**Status:** Implemented (2026-10-05); CLAUDE.md's `seaquel-cli` bullet describes the code. Where
it differs from the tasks below: `Prompter`'s `password` and `confirm` are async and read the
terminal on a thread nothing waits for, so Ctrl+C stops a waiting prompt (Unix puts echo back;
Windows is a follow-up). The table footers are Task 10's list, including
`first 1,000 rows (12 ms); more weren't counted; --limit 0 for all` when Core couldn't count, and
`nothing to run` also goes to stderr. `schema`'s `kind` is Core's spelling (`table`, `view`,
`materialized-view`). A positional SQL that is a single flag-shaped word (`--yse`) is a usage
error, so a mistyped flag can't replace piped SQL. A second Ctrl+C while a stopped command closes
its connections skips the close (`close_unless_stopped`). A `CREDENTIALS_REQUIRED` is asked for
only when Core's message names a password. After the final review: lookups by a command's
argument are worded without a flag (`Connection "x": …`, ending "Pass the id instead."), while
`mcp`'s keep `--connection`/`--project`; `--limit` stops at 99,999 (a usage error past it); two
statements' tables are separated by a blank line; stderr is written through `session::say`, which
ignores a closed stderr; JSON's `statement` stays 0-based.

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.
> The user's rules apply: no git operations (no add, commit or push) and no worktrees. Where the
> usual workflow says "commit", stop and report instead.

**Goal:** Add `conn list`, `conn test`, `schema`, `saved list|show` and `query` to `seaquel-cli`,
the part of phase 7b that runs on read-only storage.

**Architecture:** Each command opens the desktop app's `seaquel.db` read-only, as `mcp` does,
through `seaquel-terminal`'s Core builder, and calls existing Core APIs. Nothing new goes into Core.
`conn add`, `conn import`, `export` and `ask` are out of scope: the first two need writable
storage, `export` needs the export formats moved to Rust, and `ask` needs a model client in the CLI.

**Tech stack:** Rust, clap 4, tokio, `seaquel-core` (read-only `Workspace`, `Workspace::run`,
`Workspace::test`, `ConnectionHandle`), `rpassword` 7 for masked prompts, `unicode-width` for the
table layout.

---

## Decisions (agreed 2026-10-04)

1. **`query` runs SQL as typed.** It isn't read-only: the user wrote the SQL. Core's
   `CONFIRM_REQUIRED` guards destructive statements. In a terminal the CLI lists them and asks.
   Anywhere else it refuses, and `--yes` sends `confirmed: true`.
2. **No history.** `RunParams.history` is `None`, so Core records nothing and storage stays
   read-only. CLI runs don't show up in the app's history.
3. **Output.** `--format table|json`. The default is `table` when stdout is a terminal and `json`
   otherwise. No CSV until `export` moves the GUI's formats into Rust.

### Further choices this plan makes (change them here before you start)

- **Connections belong to no window.** Connects carry no origin, like MCP's.
- **Prompts only when interactive.** Interactive means stdin and stderr are both terminals and
  `--no-input` isn't set. SQL piped through stdin therefore turns prompts off: a missing password
  then fails with a message, and a destructive run needs `--yes`. There is no password environment
  variable or flag in this part (a follow-up if scripts need one).
- **An unknown SSH host key** shows the host and fingerprint and asks "Trust this host? [y/N]".
  Yes retries with `HostKeyPolicy::Trust(fp)`, so Core records the key in `~/.ssh/known_hosts`,
  as the TUI does. `HOST_KEY_MISMATCH` is never offered.
- **`query` pages by default.** `--limit N` (default 1000) is the run's `page_size`, so a SELECT
  shows its first N rows and Core's count. `--limit 0` streams every row; Core has no cap there.
- **Exit codes.** 0 for success. 1 for any failure, a statement error included. 2 for a usage error
  (clap's). 130 when stopped by SIGINT or SIGTERM.
- **stdout carries only results**: rows, lists, a saved query's SQL, `ok`. Prompts, footers,
  notices and errors go to stderr. Errors are worded `seaquel-cli <command>: CODE: message`.

## Command surface

```
seaquel-cli conn list [--project P] [--format F]
seaquel-cli conn test <CONNECTION> [--no-input]
seaquel-cli schema <CONNECTION> [TABLE] [--format F] [--no-input]
seaquel-cli saved list [--project P] [--format F]
seaquel-cli saved show <QUERY> [--project P]
seaquel-cli query -c <CONNECTION> [SQL] [-f FILE] [--saved QUERY [--project P]]
                  [--param NAME=VALUE]... [--limit N] [--yes] [--format F] [--no-input]
```

A connection, project or saved query is named by id, else by its exact, case-sensitive name. A name
that two rows share is refused with both ids, as `mcp --connection` already does
(`seaquel_mcp::exposed`).

`query`'s SQL comes from exactly one of: the positional `SQL`, `-f FILE` (`-f -` is stdin),
`--saved QUERY`, or stdin when it isn't a terminal and none of the others is given. Anything else is
a usage error.

`--param NAME=VALUE` binds `{{NAME}}` as text. When the SQL has `{{…}}` parameters
(`seaquel_core::sql::params::extract_parameters`) and a name has no `--param`, the CLI refuses
before connecting (`MISSING_PARAMETERS`, naming them). It never relies on Core binding NULL.

### `query --format json`

JSON Lines: one object per statement, written when the statement finishes.

```json
{"statement":0,"sql":"SELECT …","columns":["id","s"],"rows":[[1,"a"]],"totalRows":52331,"countEstimated":false,"truncated":true,"elapsedMs":12.4}
{"statement":1,"sql":"UPDATE …","rowsAffected":3,"elapsedMs":1.1}
{"statement":2,"sql":"SELECT …","error":{"code":"SQL_ERROR","message":"…"}}
```

`statement` is Core's index of the statement in the text, 0-based; the table format's
`statement N:` counts from 1. `truncated` is true when fewer rows were sent than `totalRows` (a page). With
`countEstimated: true` the count failed and `totalRows` is only a lower bound (Core read one row
past the page), not a count. Cells: `null`, booleans,
integers within ±2^53 and finite floats stay JSON. Everything else is `cell_text`'s string, the
GUI's text for that cell (bigint and decimal as strings, bytes as `\x…` hex, JSON as text). Nothing
is cut.

### `query --format table`

One aligned table per statement that returns columns, on stdout. A cell is cut at 60 display
columns with `…`, and newlines and tabs show as `↵` and `→`. A footer goes to stderr:
`3 rows (12 ms)`, `1,000 of 52,331 rows (12 ms); --limit 0 for all`, or
`3 rows affected (1 ms)`. A statement error goes to stderr as `statement 2: CODE: message`.

The table buffers a statement's rows to size its columns, and so does JSON Lines. With `--limit 0`
memory grows with the result; large dumps are `export`'s job.

## Files

- Create: `crates/seaquel-cli/src/session.rs`: runtime, read-only workspace, keychain notice, close
- Create: `crates/seaquel-cli/src/prompt.rs`: `Prompter` trait, terminal and scripted versions
- Create: `crates/seaquel-cli/src/output.rs`: `Format`, table renderer, JSON cells
- Create: `crates/seaquel-cli/src/resolve.rs`: connection, project and saved query by name or id
- Create: `crates/seaquel-cli/src/connect.rs`: connect and test with prompts and retries
- Create: `crates/seaquel-cli/src/conn.rs`: `conn list`, `conn test`
- Create: `crates/seaquel-cli/src/schema.rs`
- Create: `crates/seaquel-cli/src/saved.rs`
- Create: `crates/seaquel-cli/src/query.rs`
- Create: `crates/seaquel-cli/tests/commands.rs`
- Create: `crates/seaquel-terminal/src/words.rs`: `host_key_fingerprint`, `destructive_reason`
- Modify: `crates/seaquel-cli/src/lib.rs`, `crates/seaquel-cli/Cargo.toml`
- Modify: `crates/seaquel-mcp/src/exposed.rs`, `crates/seaquel-mcp/src/duckdb_helper.rs`
- Modify: `crates/seaquel-terminal/src/lib.rs`
- Modify: `crates/seaquel-tui/src/state/dialogs.rs`, `crates/seaquel-tui/src/state/text.rs` (use the moved functions)
- Modify: `crates/seaquel-cli/tests/cli.rs` (help text)
- Modify: `CLAUDE.md`, `README.md`, `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md`

---

### Task 1: Expose name lookup and the DuckDB rewording from `seaquel-mcp`

The CLI may depend on `seaquel-mcp` (`INTERFACE_LIBS`), so it reuses that crate's id-then-name
lookup and its `ENGINE_NOT_INSTALLED` wording.

**Files:**
- Modify: `crates/seaquel-mcp/src/exposed.rs`
- Modify: `crates/seaquel-mcp/src/duckdb_helper.rs`

**Step 1: Write the failing test** in `exposed.rs`'s test module (create `#[cfg(test)] mod tests`
if there is none):

```rust
#[test]
fn find_connection_is_public_and_names_both_ids() {
    let rows: Vec<PersistedConnection> = ["a", "b"]
        .iter()
        .map(|id| serde_json::from_value(serde_json::json!({
            "id": id, "projectId": "p1", "name": "twin", "type": "sqlite",
            "host": "", "port": 0, "databaseName": "", "username": "",
            "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        })).unwrap())
        .collect();
    let e = super::find_connection(&rows, &[], "twin").unwrap_err();
    assert_eq!(e.code, AMBIGUOUS_CONNECTION);
    assert!(e.message.contains("\"a\"") && e.message.contains("\"b\""), "{}", e.message);
    assert_eq!(super::find_connection(&rows, &[], "a").unwrap().id, "a");
}
```

**Step 2: Run it.** `cargo test -p seaquel-mcp --lib find_connection_is_public` should pass
already, since it is in-crate. It pins the behaviour before the visibility change.

**Step 3: Make these `pub`:** `find_connection`, `find_project`, `lookup` and `Found` in
`exposed.rs`. In `duckdb_helper.rs` add a public wrapper that takes the engine id instead of an
`Exposed`, and have `connect_error` call it:

```rust
/// `e`, reworded when it is a DuckDB connection's helper that is missing
/// or refused (`engine` is the saved row's `type`). For `seaquel-cli`'s
/// commands as for the MCP tools.
pub fn reword_connect_error(core: &Core, engine: &str, version: &str, e: ToolError) -> ToolError {
    if engine != DUCKDB || e.code != ENGINE_NOT_INSTALLED {
        return e;
    }
    match core.duckdb_helper_status() {
        Ok(status) if install_fixes(&status) => {
            ToolError::new(ENGINE_NOT_INSTALLED, not_installed_message(version))
        }
        Ok(DuckdbHelperStatus::Installed { .. }) => {
            let message = refused_message(version, &e.message);
            ToolError::new(ENGINE_NOT_INSTALLED, message)
        }
        _ => e,
    }
}

pub(crate) fn connect_error(core: &Core, c: &Exposed, version: &str, e: ToolError) -> ToolError {
    reword_connect_error(core, &c.engine, version, e)
}
```

**Step 4: Run** `cargo test -p seaquel-mcp` and `cargo clippy -p seaquel-mcp --all-targets`. Both
should pass.

---

### Task 2: Move the host-key fingerprint and destructive-reason words into `seaquel-terminal`

Both terminal binaries need them, and `seaquel-tui` can't be a dependency.

**Files:**
- Create: `crates/seaquel-terminal/src/words.rs`
- Modify: `crates/seaquel-terminal/src/lib.rs`
- Modify: `crates/seaquel-tui/src/state/dialogs.rs:122-130`, `crates/seaquel-tui/src/state/text.rs:550-568`

**Step 1: Write `words.rs` with its tests first.** Move the bodies unchanged from the TUI:

```rust
//! Words both terminal binaries use for Core's answers.

/// The `SHA256:…` fingerprint in an `UNKNOWN_HOST_KEY` message.
pub fn host_key_fingerprint(message: &str) -> Option<String> {
    let label = "Fingerprint: SHA256:";
    let at = message.rfind(label)? + label.len();
    let rest = &message[at..];
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
        .unwrap_or(rest.len());
    (len > 0).then(|| format!("SHA256:{}", &rest[..len]))
}

/// What a destructive statement does, in a few words.
pub fn destructive_reason(reason: seaquel_core::sql::statements::DestructiveReason) -> &'static str {
    use seaquel_core::sql::statements::DestructiveReason as R;
    match reason {
        R::DropTable => "drops a table",
        R::DropIndex => "drops an index",
        R::DropView => "drops a view",
        R::DropSchema => "drops a schema",
        R::DropDatabase => "drops a database",
        R::DropSequence => "drops a sequence",
        R::DropFunction => "drops a function",
        R::DropColumn => "drops a column",
        R::Truncate => "empties a table",
        R::DeleteNoWhere => "DELETE without WHERE",
        R::UpdateNoWhere => "UPDATE without WHERE",
        R::MergeDelete => "MERGE that deletes",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_fingerprint_at_the_end_of_the_message() {
        let m = "Unknown host key for bastion:22. Fingerprint: SHA256:abc+/9=";
        assert_eq!(host_key_fingerprint(m).as_deref(), Some("SHA256:abc+/9="));
        assert_eq!(host_key_fingerprint("no key here"), None);
    }
}
```

Check that `seaquel_core::sql::statements` is reachable with `seaquel-terminal`'s Core features
(the `sql` re-export is unconditional).

**Step 2: Export them** from `lib.rs`: `mod words; pub use words::{destructive_reason, host_key_fingerprint};`.

**Step 3: Point the TUI at them.** Replace the body of `dialogs::fingerprint` with a call to
`seaquel_terminal::host_key_fingerprint`, or replace its callers and delete it. Do the same for
`text::destructive_reason`. Keep the TUI's existing tests passing.

**Step 4: Run** `cargo test -p seaquel-terminal -p seaquel-tui` and the clippy for both. Both
should pass, with no snapshot changes.

---

### Task 3: The CLI's dependencies, subcommands and session

**Files:**
- Modify: `crates/seaquel-cli/Cargo.toml`
- Modify: `crates/seaquel-cli/src/lib.rs`
- Create: `crates/seaquel-cli/src/session.rs`
- Test: `crates/seaquel-cli/tests/cli.rs`

**Step 1: Dependencies.** Add these to `[dependencies]`:

```toml
# `query`'s cells (`seaquel_core::ai::tools::format`, MCP's cell rendering):
# the registry only, no model client. seaquel-mcp turns it on already;
# named here so a package-by-package clippy doesn't depend on that.
# (Extend the existing seaquel-core line's features with "ai".)
seaquel-types = { path = "../seaquel-types" }
serde_json = { workspace = true }
uuid = { workspace = true }
unicode-width = { workspace = true }
# Masked password prompts on the terminal (`/dev/tty`, the console on
# Windows).
rpassword = "7"
zeroize = "1"
```

Move `seaquel-types` and `serde_json` out of `[dev-dependencies]`, since they're now normal
dependencies. Run `npm run crates:check` and `scripts/check-native-deps.sh`. Neither should
complain: `seaquel-types` is a pure crate, and rpassword isn't on any ban list.

**Step 2: Write the failing help test** in `tests/cli.rs`:

```rust
#[test]
fn help_lists_the_new_commands() {
    let out = cli(&["--help"]);
    let help = text(&out.stdout);
    for command in ["conn", "schema", "saved", "query", "mcp", "duckdb"] {
        assert!(help.contains(command), "{command}: {help}");
    }
}
```

Run `cargo test -p seaquel-cli --test cli help_lists`. It should fail.

**Step 3: Add the clap types to `lib.rs`.** Add `Conn(ConnArgs)`, `Schema(SchemaArgs)`,
`Saved(SavedArgs)` and `Query(QueryArgs)` to `Command`, each with doc comments (clap shows them),
and dispatch them to `conn::run`, `schema::run`, `saved::run` and `query::run`. Shared flags:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FormatArg { Table, Json }

#[derive(Debug, Args)]
pub struct OutputArgs {
    /// table (the default on a terminal) or json (the default otherwise).
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
}

#[derive(Debug, Args)]
pub struct InputArgs {
    /// Never ask for anything: a missing password, an unknown SSH host key
    /// or a destructive statement fails instead.
    #[arg(long)]
    pub no_input: bool,
}
```

For `QueryArgs`, use clap's `ArgGroup` so `SQL`, `--file` and `--saved` are mutually exclusive.
`--limit` is `u32`, default `1000`. `--param` is `Vec<String>`, parsed in `query.rs`.

**Step 4: Write `session.rs`**, shared by every new command:

```rust
//! A command's Core and the app's data, opened read-only (as `mcp` does),
//! and closed again.

pub const TEST_HOOKS_PREFIX: &str = "SEAQUEL_CLI_TEST";
/// How long closing connections may take after a command or a signal.
pub const CLOSE_WAIT: Duration = Duration::from_secs(5);

pub struct Session {
    pub core: Arc<Core>,
    pub ws: Arc<Workspace>,
    pub secret_wait: Arc<SecretWait>,
}

impl Session {
    pub async fn open() -> Result<Self, CoreError> {
        let hooks = TestHooks::from_env(TEST_HOOKS_PREFIX);
        let dir = seaquel_terminal::data_dir()?;
        let core = Arc::new(
            seaquel_terminal::core_builder(CoreOptions::default().with_hooks(&hooks)).build(),
        );
        let secret_wait = SecretWait::new();
        let spec = WorkspaceSpec::new(&dir)
            .with_storage_options(StorageOptions { read_only: true, ..StorageOptions::default() })
            .with_secrets(secret_wait.watch(hooks.secret_store().await.map_err(|m| CoreError::new("SECRET_STORE_ERROR", m))?));
        let ws = core.open_workspace(spec).await?;
        Ok(Self { core, ws, secret_wait })
    }

    /// Close every connection this command opened, then storage, within
    /// [`CLOSE_WAIT`].
    pub async fn close(self) {
        let _ = tokio::time::timeout(CLOSE_WAIT, self.ws.close_all(&self.core)).await;
        self.ws.close().await;
    }
}

/// A multi-threaded runtime, `f` on it, then a bounded shutdown so a
/// Postgres cancel Core spawned can still go out.
pub fn block_on<F: Future<Output = ExitCode>>(name: &str, f: F) -> ExitCode { /* as mcp::run */ }
```

Check `hooks.secret_store()`'s error type in `seaquel-terminal/src/hooks.rs` and adapt the
`map_err`. `mcp.rs` treats it as a `String`.

Add a keychain notice, a task spawned for the session's life: when `secret_wait.pending_for()`
passes 250 ms, print once to stderr `Waiting for the system keychain (macOS may ask behind this
window)…`. Watch `secret_wait.changed()`.

`STORAGE_NEEDS_UPGRADE` and `STORAGE_NOT_FOUND` from `open` print their message with the hint
`Open the Seaquel app once, then run this again.`. Use exit code 1.

**Step 5: Stub the four `run` functions** so each prints `not yet` and returns 1. Run the help test.
It should pass.

---

### Task 4: Output, the table and the JSON cells

**Files:**
- Create: `crates/seaquel-cli/src/output.rs`

**Step 1: Write the failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use seaquel_types::Value;

    #[test]
    fn json_cells_keep_what_json_holds_exactly() {
        assert_eq!(json_cell(&Value::Int(42)), serde_json::json!(42));
        assert_eq!(json_cell(&Value::Int(1 << 60)), serde_json::json!("1152921504606846976"));
        assert_eq!(json_cell(&Value::Null), serde_json::Value::Null);
        assert_eq!(json_cell(&Value::Bytes(vec![0xde, 0xad])), serde_json::json!("\\xdead"));
    }

    #[test]
    fn a_table_aligns_by_display_width_and_cuts_long_cells() {
        let t = table(&["id", "name"], &[vec!["1".into(), "日本".into()], vec!["22".into(), "x".repeat(80)]]);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "id  name");
        assert!(lines[2].starts_with("1   日本"));
        assert!(lines[3].ends_with('…'));
        assert_eq!(unicode_width::UnicodeWidthStr::width(lines[3]), 4 + 60);
    }

    #[test]
    fn table_text_shows_newlines_and_tabs() {
        assert_eq!(table_text("a\nb\tc"), "a↵b→c");
    }
}
```

Check the actual `Value` variant names in `crates/seaquel-types/src/value.rs` and adjust them.

**Step 2: Run** `cargo test -p seaquel-cli --lib output`. It should fail to compile.

**Step 3: Implement it.**

```rust
pub const MAX_CELL_WIDTH: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format { Table, Json }

impl Format {
    /// The flag, else table on a terminal and JSON otherwise.
    pub fn pick(arg: Option<crate::FormatArg>) -> Self {
        match arg {
            Some(crate::FormatArg::Table) => Format::Table,
            Some(crate::FormatArg::Json) => Format::Json,
            None if std::io::stdout().is_terminal() => Format::Table,
            None => Format::Json,
        }
    }
}

/// A cell as JSON: what JSON holds exactly stays JSON, the rest is the
/// GUI's text for it. Never cut.
pub fn json_cell(v: &Value) -> serde_json::Value {
    use seaquel_core::ai::tools::format::cell_text;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => (*b).into(),
        Value::Int(i) if i.unsigned_abs() <= seaquel_types::MAX_SAFE_INTEGER as u64 => (*i).into(),
        Value::Float(f) if f.is_finite() => (*f).into(),
        Value::Text(s) => s.clone().into(),
        other => cell_text(other).into(),
    }
}

/// `\n` as `↵`, `\t` as `→`, other control characters as `�`.
pub fn table_text(s: &str) -> String { /* map chars */ }

/// Columns separated by two spaces, a rule of `─` under the header, each
/// cell cut to MAX_CELL_WIDTH display columns with `…`. Trailing spaces trimmed.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String { /* … */ }
```

Take the cut at a character boundary, measuring with `UnicodeWidthChar`. The `…` counts as one
column, so a cut cell is exactly `MAX_CELL_WIDTH` wide.

**Step 4: Run the tests.** They should pass.

---

### Task 5: `conn list`

**Files:**
- Create: `crates/seaquel-cli/src/resolve.rs`
- Create: `crates/seaquel-cli/src/conn.rs`
- Test: `crates/seaquel-cli/tests/commands.rs`

**Step 1: Write the sandbox and the failing test.** Start `tests/commands.rs` with a sandbox based
on `tests/stdio.rs`'s:

- a project `p1` "Main" and a project `p2` "Other";
- `c-lite` "lite" (SQLite file with table `t (id INTEGER, s TEXT)`, rows `(1,'one'),(2,NULL)`);
- `c-twin-a`/`c-twin-b` both "twin";
- `c-pg` "pg nopass" (Postgres on `127.0.0.1:99999`, `savePassword: false`);
- `c-duck` "duck" (`:memory:`);
- saved queries in `p1`: `sq-1` "count t" (`SELECT count(*) AS n FROM t`), `sq-2` "by id"
  (`SELECT s FROM t WHERE id = {{id}}`), written with `st.write()`, `saved_queries::insert`,
  `tx.commit()`.

Use helpers `run(&self, args) -> Output` (blocking `std::process::Command` with the sandbox env and
stdin set to `Stdio::null()`, so the run is never interactive), plus `stdout`/`stderr` as strings.

```rust
#[tokio::test]
async fn conn_list_prints_every_connection_as_json_when_piped() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"lite") && names.contains(&"duck"), "{names:?}");
    let lite = rows.iter().find(|r| r["id"] == "c-lite").unwrap();
    assert_eq!(lite["projectName"], "Main");
    assert_eq!(lite["type"], "sqlite");

    let out = sb.run(&["conn", "list", "--project", "Other", "--format", "table"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).lines().count(), 2, "header and rule only: {}", stdout(&out));
}
```

**Step 2: Run** `cargo test -p seaquel-cli --test commands conn_list`. It should fail.

**Step 3: Write `resolve.rs`**, thin wrappers over `seaquel_mcp::exposed`:

```rust
pub fn connection<'a>(rows: &'a [PersistedConnection], projects: &[PersistedProject], wanted: &str)
    -> Result<&'a PersistedConnection, CoreError>
{
    seaquel_mcp::exposed::find_connection(rows, projects, wanted)
        .map_err(|e| CoreError::new(e.code, e.message))
}
pub fn project<'a>(projects: &'a [PersistedProject], wanted: &str) -> Result<&'a PersistedProject, CoreError> { /* same */ }
/// A saved query by id, else exact name, in `project` or in every project.
pub fn saved_query<'a>(rows: &'a [(PersistedSavedQuery, String)], wanted: &str) -> Result<&'a PersistedSavedQuery, CoreError> {
    // `lookup` with SAVED_QUERY_NOT_FOUND / AMBIGUOUS_SAVED_QUERY naming each id and project
}
```

The messages say `--connection` and `--project`. That wording is fine for the CLI too.

**Step 4: Write `conn.rs`'s `list`.** Use `Session::open`, then `ws.list_projects()` and
`ws.list_connections()` (`.value`). Filter with `--project` if given. Each output row is `id`,
`name`, `type`, `projectId`, `projectName`, `host`, `port` (`null` when 0), `database`, `user`, and
`ssh` (true when the row's `ssh_tunnel` is enabled; parse it as `seaquel-tui`'s `conn_item` does).
JSON is an array printed with `serde_json::to_string_pretty`. The table columns are NAME, TYPE,
HOST, DATABASE, PROJECT, ID. Never print the connection string.

**Step 5: Run the test.** It should pass.

---

### Task 6: Connecting, with prompts

**Files:**
- Create: `crates/seaquel-cli/src/prompt.rs`
- Create: `crates/seaquel-cli/src/connect.rs`

**Step 1: Write `prompt.rs`:**

```rust
/// What the CLI may ask. `Terminal` asks on the terminal; `None` answers
/// nothing (not interactive, or --no-input). Tests use `Scripted`.
pub trait Prompter {
    fn interactive(&self) -> bool;
    /// A masked line; `None` when nothing can be asked or the user gave up.
    fn password(&mut self, prompt: &str) -> Option<Zeroizing<String>>;
    /// y/N on stderr; false when nothing can be asked.
    fn confirm(&mut self, question: &str) -> bool;
}

pub fn for_command(no_input: bool) -> Box<dyn Prompter> {
    let interactive = !no_input && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if interactive { Box::new(Terminal) } else { Box::new(NoPrompts) }
}
```

`Terminal::password` uses `rpassword::prompt_password`. An empty answer counts as given (trust auth
exists); an I/O error counts as `None`. `Terminal::confirm` writes `question [y/N] ` to stderr and
reads one line from stdin. Only `y` and `yes`, case-insensitive, count as yes. Add
`#[cfg(test)] pub struct Scripted { passwords: VecDeque<Option<String>>, confirms: VecDeque<bool>, asked: Vec<String> }`.

**Step 2: Write the failing tests for the decision logic** in `connect.rs`. Keep it pure, as the
TUI's `failed` is:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn row(save_password: bool) -> RowFacts { RowFacts { engine: "postgres".into(), is_file: false, save_password, ssh: None } }

    #[test]
    fn a_row_that_saves_no_password_is_asked_first() {
        assert_eq!(ask_before(&row(false), &Typed::default(), false), Some(Kind::Db));
        assert_eq!(ask_before(&row(true), &Typed::default(), false), None);
        // No keychain this session: ask for what it would hold.
        assert_eq!(ask_before(&row(true), &Typed::default(), true), Some(Kind::Db));
    }

    #[test]
    fn next_step_after_a_failure() {
        let typed = Typed::default();
        assert_eq!(next(&row(true), &typed, "CREDENTIALS_REQUIRED", "password required"), Next::Ask(Kind::Db));
        assert_eq!(next(&row(true), &typed, "UNKNOWN_HOST_KEY", "… Fingerprint: SHA256:abc"), Next::Trust("SHA256:abc".into()));
        assert_eq!(next(&row(true), &typed, "HOST_KEY_MISMATCH", "…"), Next::Fail);
        assert_eq!(next(&row(true), &typed, "SECRET_STORE_UNAVAILABLE", "…"), Next::NoStore);
        assert_eq!(next(&row(true), &typed, "AUTH_ERROR", "…"), Next::Fail);
    }
}
```

Port `to_ask` and the relevant arms of `failed` from `seaquel-tui/src/state/connect.rs:273-292,
400-470` as `ask_before` and `next`. Kinds are `Db`, `Ssh` and `SshKey`. Unlike the TUI, a wrong
password is not asked for again: a CLI fails and the user reruns.

**Step 3: Run** `cargo test -p seaquel-cli --lib connect`. It should fail, then pass once
implemented.

**Step 4: Write the driver loop:**

```rust
pub enum Mode { Connect, Test }

/// Connect (or test) the saved connection `row`, asking for what's missing
/// when `prompter` can. At most four attempts, and each secret is asked once.
pub async fn open(s: &Session, row: &PersistedConnection, mode: Mode, prompter: &mut dyn Prompter)
    -> Result<Option<String>, CoreError>
{
    let facts = RowFacts::of(row);
    let mut typed = Typed::default();      // Zeroizing<String>s
    let mut no_store = false;
    let mut host_key = HostKeyPolicy::KnownOnly;
    for _ in 0..4 {
        if prompter.interactive() {
            if let Some(kind) = ask_before(&facts, &typed, no_store) {
                typed.set(kind, prompter.password(&label(kind, &row.name)).ok_or_else(cancelled)?);
            }
        }
        let req = ConnectRequest::saved(&row.id).with_secrets(typed.supplied()).with_host_key(host_key.clone());
        let result = match mode {
            Mode::Connect => s.ws.connect(&s.core, req).await.map(Some),
            Mode::Test => s.ws.test(&s.core, req).await.map(|()| None),
        };
        let e = match result { Ok(v) => return Ok(v), Err(e) => e };
        match (prompter.interactive(), next(&facts, &typed, &e.code, &e.message)) {
            (true, Next::Ask(kind)) => typed.set(kind, prompter.password(&label(kind, &row.name)).ok_or_else(cancelled)?),
            (true, Next::NoStore) => no_store = true,
            (true, Next::Trust(fp)) => {
                let (host, port) = facts.ssh_host();
                if !prompter.confirm(&format!("The SSH host {host}:{port} isn't known. Its key's fingerprint is {fp}. Trust this host?")) {
                    return Err(e);
                }
                host_key = HostKeyPolicy::Trust(fp);
            }
            _ => return Err(not_interactive_hint(reword(s, row, e))),
        }
    }
    Err(CoreError::new("CONNECT_FAILED", "Gave up after four attempts."))
}
```

`reword` is `seaquel_mcp::duckdb_helper::reword_connect_error(&s.core, &row.ty, VERSION, …)`.
`not_interactive_hint` appends to `CREDENTIALS_REQUIRED`: ` Run this in a terminal to be asked for
it, or save the password in the Seaquel app.`. It appends to `UNKNOWN_HOST_KEY`:
` Run this in a terminal to check and trust the host key.`. Check `typed.supplied()` against
`SuppliedSecrets`'s fields.

---

### Task 7: `conn test`

**Files:**
- Modify: `crates/seaquel-cli/src/conn.rs`
- Test: `crates/seaquel-cli/tests/commands.rs`

**Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn conn_test_prints_ok() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "lite"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "ok\n");
}

#[tokio::test]
async fn conn_test_without_a_terminal_doesnt_ask_and_says_how() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "pg nopass"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).is_empty());
    let err = stderr(&out);
    assert!(err.starts_with("seaquel-cli conn test: "), "{err}");
}

#[tokio::test]
async fn an_ambiguous_name_lists_both_ids() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "twin"]);
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("AMBIGUOUS_CONNECTION") && err.contains("c-twin-a") && err.contains("c-twin-b"), "{err}");
}

#[tokio::test]
async fn duckdb_without_the_helper_says_how_to_install_it() {
    let sb = sandbox().await;
    let out = sb.run(&["conn", "test", "duck"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("seaquel-cli duckdb install"), "{}", stderr(&out));
}
```

For `pg nopass` the exact code depends on the engine: Postgres with no password and nothing
listening fails at the socket. Assert only on the prefix and the exit code, as above. The
`CREDENTIALS_REQUIRED` hint is covered by Task 6's unit tests.

**Step 2: Run the tests.** They should fail.

**Step 3: Implement it.** Resolve the row, call `connect::open(…, Mode::Test, …)`, print `ok` on
stdout, then close the session.

**Step 4: Run the tests.** They should pass.

---

### Task 8: `schema`

**Files:**
- Create: `crates/seaquel-cli/src/schema.rs`
- Test: `crates/seaquel-cli/tests/commands.rs`

**Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn schema_lists_tables_and_describes_one() {
    let sb = sandbox().await;
    let out = sb.run(&["schema", "lite"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let tables: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(tables.iter().any(|t| t["name"] == "t" && t["kind"] == "table"), "{tables:?}");

    let out = sb.run(&["schema", "lite", "t"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let table: Json = serde_json::from_str(&stdout(&out)).unwrap();
    let cols: Vec<&str> = table["columns"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(cols, ["id", "s"]);

    let out = sb.run(&["schema", "lite", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("TABLE_NOT_FOUND"));
}
```

**Step 2: Run the tests.** They should fail.

**Step 3: Implement it.** Connect with `Mode::Connect`, then call
`s.ws.engine(&s.core, &id)?.schema_tables()`.

- Without `TABLE`, print each table's `schema`, `name`, `kind` (Core's own spelling:
  `table`, `view`, `materialized-view`) and `rowCount` (estimate, may be null). The table columns are SCHEMA, NAME,
  KIND, ROWS.
- With `TABLE`, match `arg == t.name` or `arg == format!("{}.{}", t.schema, t.name)` against the
  list, so DuckDB's `catalog.schema` names work without splitting on `.`. With none, return
  `TABLE_NOT_FOUND`. With several, return `AMBIGUOUS_TABLE` listing `schema.name` for each. Then
  call `table_metadata(&t.schema, &t.name)`. JSON is
  `{"schema","name","columns":[SchemaColumn…],"indexes":[SchemaIndex…]}`, serialized from the
  types as they are. The table output has columns NAME, TYPE, NULL, DEFAULT, KEY (`PK`, `FK →
  table.column`), a blank line, then INDEX, COLUMNS, UNIQUE. Check `SchemaIndex`'s fields in
  `crates/seaquel-types/src/dialect.rs`.

Map `DbError` to `CoreError` the way `seaquel-tui`'s `db_error` does.

**Step 4: Run the tests.** They should pass.

---

### Task 9: `saved list` and `saved show`

**Files:**
- Create: `crates/seaquel-cli/src/saved.rs`
- Test: `crates/seaquel-cli/tests/commands.rs`

**Step 1: Write the failing tests:**

```rust
#[tokio::test]
async fn saved_lists_and_shows() {
    let sb = sandbox().await;
    let out = sb.run(&["saved", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows: Vec<Json> = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["projectName"], "Main");

    let out = sb.run(&["saved", "show", "count t"]);
    assert_eq!(stdout(&out), "SELECT count(*) AS n FROM t\n");
}
```

**Step 2: Run the tests.** They should fail.

**Step 3: Implement it.** Load the projects (filtered by `--project`), then call
`ws.list_saved_queries(&core, &p.id)` for each. `list` prints `id`, `name`, `folder`, `projectId`,
`projectName`, `parameters` (from `extract_parameters(&q.query)`) and `description`; the table
columns are NAME, FOLDER, PROJECT, PARAMS, ID. `show` prints the query text exactly, with one
newline added when it doesn't end in one. No connection is needed for either.

**Step 4: Run the tests.** They should pass.

---

### Task 10: `query`

**Files:**
- Create: `crates/seaquel-cli/src/query.rs`
- Test: `crates/seaquel-cli/tests/commands.rs`

**Step 1: Write the failing integration tests:**

```rust
#[tokio::test]
async fn query_prints_one_json_line_per_statement() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "SELECT id, s FROM t ORDER BY id; SELECT 1 AS one"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let lines: Vec<Json> = stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["columns"], json!(["id", "s"]));
    assert_eq!(lines[0]["rows"], json!([[1, "one"], [2, null]]));
    assert_eq!(lines[1]["rows"], json!([[1]]));
}

#[tokio::test]
async fn limit_pages_and_says_so() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "--limit", "1", "SELECT id FROM t ORDER BY id"]);
    let line: Json = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["rows"], json!([[1]]));
    assert_eq!(line["totalRows"], 2);
    assert_eq!(line["truncated"], true);
}

#[tokio::test]
async fn a_destructive_run_needs_yes_when_not_interactive() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "DELETE FROM t"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    let count = sb.run(&["query", "-c", "lite", "SELECT count(*) AS n FROM t"]);
    assert!(stdout(&count).contains("[[2]]"), "nothing ran: {}", stdout(&count));

    let out = sb.run(&["query", "-c", "lite", "--yes", "DELETE FROM t"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let line: Json = serde_json::from_str(stdout(&out).trim()).unwrap();
    assert_eq!(line["rowsAffected"], 2);
}

#[tokio::test]
async fn a_failing_statement_fails_the_command_but_the_rest_runs() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "SELECT nope FROM t; SELECT 1 AS one"]);
    assert_eq!(out.status.code(), Some(1));
    let lines: Vec<Json> = stdout(&out).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert!(lines[0]["error"]["code"].is_string());
    assert_eq!(lines[1]["rows"], json!([[1]]));
}

#[tokio::test]
async fn saved_queries_run_with_their_parameters() {
    let sb = sandbox().await;
    let out = sb.run(&["query", "-c", "lite", "--saved", "by id"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("MISSING_PARAMETERS") && stderr(&out).contains("id"));
    let out = sb.run(&["query", "-c", "lite", "--saved", "by id", "--param", "id=1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("\"one\""));
}

#[tokio::test]
async fn sql_can_come_from_stdin() {
    let sb = sandbox().await;
    let out = sb.run_with_stdin(&["query", "-c", "lite"], "SELECT 7 AS n");
    assert!(stdout(&out).contains("[[7]]"), "{}", stdout(&out));
}
```

`--param id=1` binds the text `'1'`. SQLite compares `id = '1'` as integer affinity, so the row
matches. On engines that don't coerce, users write `CAST({{id}} AS …)` as in the app. Add
`run_with_stdin` to the sandbox helpers.

**Step 2: Run the tests.** They should fail.

**Step 3: Write the pure parts with unit tests.**
- `parse_param("a=b=c") -> ("a", "b=c")`. A string with no `=` is a usage error.
- `missing_parameters(sql, given) -> Vec<String>`, in text order with no repeats.
- `Footer` text, one test per case: `3 rows (12 ms)`, `1 row (0 ms)`,
  `1,000 of 52,331 rows (12 ms); --limit 0 for all`,
  `first 1,000 rows (12 ms); more weren't counted; --limit 0 for all` when `countEstimated`,
  `3 rows affected (1 ms)`, and `done (1 ms)` for a statement with neither rows nor a count.

**Step 4: Write the run loop:**

```rust
pub fn run(args: QueryArgs) -> ExitCode {
    session::block_on("query", async move {
        tokio::select! {
            code = query(args) => code,
            () = session::stopped() => { eprintln!("seaquel-cli query: stopped"); ExitCode::from(130) }
        }
    })
}
```

`session::stopped()` is `duckdb.rs`'s `stopped`. Move it into `session.rs` and have `duckdb.rs` use
it. When stopped, the `query` future is dropped, which drops Core's run stream. Core cancels the
statement (Postgres `pg_cancel_backend`, MySQL `KILL QUERY`) on a task it spawned, so
`block_on`'s `shutdown_timeout` lets that go out. The dropped `Session` still owns open
connections. Hold it in an `Arc` outside the `select!` and call `close()` after it in both arms.

`query(args)`:
1. Read the SQL from the chosen source. If it is empty after trimming, print `nothing to run` and
   exit 0.
2. Check parameters: `missing_parameters`, then `MISSING_PARAMETERS` before opening anything.
3. Open the `Session`, resolve the connection, then `connect::open(Mode::Connect)`.
4. Build `RunParams { connection_id, stream_id: uuid v4, text, target: RunTarget::All, params:
   (!given.is_empty()).then(…Value::Text…), page_size: args.limit, confirmed: args.yes,
   defer_writes: false, history: None }` and iterate `s.ws.run(&s.core, params)`. Use
   `run_from` with `WriteOrigin::none()` if `run` isn't the right entry point; check
   `crates/seaquel-core/src/run.rs:100-120`.
5. Fold events into a `Statement { index, sql, columns, rows, done: Option<…>, error }`. On
   `StatementDone` or `StatementError`, print it (JSON line, or table on stdout plus footer on
   stderr) and drop its rows. `StatementDeferred` can't happen (`defer_writes: false`); treat it
   as an error if it does.
6. On `Error { code: "CONFIRM_REQUIRED", destructive, destructive_total }`, list up to 20 on
   stderr as `  3. drops a table: DROP TABLE x` (the SQL's first line, cut at 80 characters),
   plus `…and N more`. Interactive and confirmed: run again with `confirmed: true` (Core checked
   before anything ran, so nothing ran). Otherwise print
   `seaquel-cli query: CONFIRM_REQUIRED: … Pass --yes to run them.` and exit 1.
7. On any other `Error`, print it and exit 1. On `Done { succeeded, statements }`: with
   `statements == 0`, print `nothing to run` and exit 0; otherwise exit 0 if `succeeded`, else 1.
   A stream that ends with neither is `CANCELLED`, exit 1.
8. Call `session.close()`.

`Batch` adds `rows`, and the first batch's `columns` win. A statement with no columns prints no
table and no JSON `columns`/`rows` keys. It still gets its footer, and `rowsAffected` when
present.

**Step 5: Run** `cargo test -p seaquel-cli`. Everything should pass.

**Step 6: Check Ctrl+C by hand** against the e2e Postgres. Start the containers
(`docker compose -f e2e/test-databases/docker-compose.yml up -d postgres`), save a connection in
the dev app, and run `cargo run -p seaquel-cli -- query -c <name> "SELECT pg_sleep(60)"`. Press
Ctrl+C. Expected: `stopped`, exit 130 (`echo $status` in fish), and in `pg_stat_activity` the
backend is no longer running `pg_sleep`. Also check the prompts by hand: a connection with
`savePassword: false` asks for the password masked, and `DELETE FROM` asks y/N.

---

### Task 11: Docs and the full check

**Files:**
- Modify: `CLAUDE.md` (the `seaquel-cli` bullet; the `seaquel-terminal` bullet for `words.rs`)
- Modify: `README.md` (the "Command line" section near line 558: one short example per command)
- Modify: `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` (Status line and Phases:
  "Phase 7b, part 1"; Open work keeps `conn add|import`, `export`, `ask`; decision 10's commands)

**Step 1: Update the docs.** For the CLAUDE.md `seaquel-cli` bullet, describe the new subcommands,
the read-only storage they share with `mcp`, that runs record no history, the interactive rule,
the output and exit-code contract, and that `query` is not read-only (confirmation only).

**Step 2: Run the full checks:**

```bash
cargo test -p seaquel-cli -p seaquel-mcp -p seaquel-terminal -p seaquel-tui
cargo clippy -p seaquel-cli --all-targets
cargo clippy -p seaquel-terminal --all-targets
cargo clippy -p seaquel-tui --all-targets
cargo clippy -p seaquel-mcp --all-targets
npm run crates:check
scripts/check-native-deps.sh
```

All of them should pass. Report the output. Don't commit.

---

## Out of scope (part 2)

- `conn add` and `conn import`: a second-process writable open for those subcommands only, plus
  `connectionCreate` with `SecretChanges`.
- `export`: the GUI's CSV, JSON and SQL formats moved into a Rust crate with fixtures recorded from
  `src/lib/utils/export-formats.ts`, then `--format csv` here.
- `ask`: `ai-native` in the CLI, and the rule for whether its SQL runs.
- History for CLI runs, a password flag or environment variable for scripts, and TUI connection
  create/edit.
