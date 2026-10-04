# The DuckDB suites on both drivers

Every integration suite here that opens DuckDB gets its engine from
`common/engine.rs`, so the same tests run on the native driver (duckdb-rs in
the test process) and on the remote one (a `seaquel-duckdb` helper process,
the DuckDB helper plan). The rule (the plan's ground rules): a suite that
passes natively passes remotely, or the difference is listed below with its
reason and the Decision behind it. Nothing is skipped for the remote driver.

## Running them

```bash
# Native (the default):
cargo test -p seaquel-engine-duckdb

# Remote, with no DuckDB linked into the test binaries:
cargo build -p seaquel-duckdb
SEAQUEL_TEST_DUCKDB_DRIVER=remote \
SEAQUEL_TEST_DUCKDB_HELPER="$CARGO_TARGET_DIR/debug/seaquel-duckdb" \
  cargo test -p seaquel-engine-duckdb --no-default-features --features remote

# Both drivers' own tests (the helper's loop, the client's calls, tests/remote.rs)
# with the suites on either driver:
SEAQUEL_TEST_DUCKDB_HELPER=… cargo test -p seaquel-engine-duckdb --features remote,helper
```

- `SEAQUEL_TEST_DUCKDB_DRIVER` is `native` or `remote`. Unset, it is `native`
  when the build has the `native` feature, else `remote`. Any other value, or
  a driver the build lacks, panics.
- `SEAQUEL_TEST_DUCKDB_HELPER` names a built helper. Each engine the switch
  makes installs it (a hard link, else a copy) as
  `bin/duckdb/<version>/seaquel-duckdb[.exe]` in a folder of its own under
  `CARGO_TARGET_TMPDIR`, laid out and checked as a real install is. The
  remote driver without it panics; `tests/remote.rs` skips instead (and
  fails under `SEAQUEL_TEST_REQUIRE_ENGINES`).
- `driver_switch.rs` tells the drivers apart by what they do (the two
  differences below), so a run that meant `remote` can't pass on `native`.
- The helper inherits the environment when it starts, so the restricted
  suites' temp `HOME` (and `USERPROFILE` on Windows) reaches it.
  `restricted.rs`'s `duckdb_s_home_is_the_temp_one` checks that DuckDB's `~`
  is the temp one on either driver.
- `seaquel-mcp`'s `tests/tools.rs` follows the same variable through Core
  (`CoreBuilder::duckdb_helper`), and Core's `tests/duckdb_remote.rs` is
  remote only.

## Differences

### A request over the frame is refused (`INVALID_ARGUMENT`)

A call's SQL and bound values travel as one JSON control frame, at most
16 MiB (`MAX_FRAME`; the payload is `MAX_PAYLOAD`, 5 bytes less). A request
whose JSON is larger is refused by the client before anything is sent, with
`INVALID_ARGUMENT` ("The statement and its values take N bytes, more than
the … bytes the DuckDB helper takes in one call."), and the connection goes
on. The native driver has no such limit.

- Why: Decision 3. Frames above 16 MiB are a protocol error on either side,
  so the cap bounds what either process allocates for one frame.
- Pinned by: `driver_switch.rs` (`the_suites_run_on_the_driver_they_ask_for`,
  a 17 MiB statement) and `remote.rs` (a 17 MiB request, no SQL in the
  message).

### A row over the frame is `RESULT_TOO_LARGE`

The helper sends rows as Arrow IPC batch frames. A batch is sliced so no
frame passes 8 MiB (`MAX_BATCH_FRAME`); a single row larger than that goes
alone, up to the 16 MiB frame; a row past that fails the call with
`RESULT_TOO_LARGE`, and the connection goes on. The native driver returns
the row.

- Why: Decision 3, as above.
- Pinned by: `driver_switch.rs` (`a_row_past_the_frame_is_too_large_only_remotely`,
  a 20 MiB cell) and `remote.rs` (a 12 MiB row arrives, a 20 MiB one doesn't).

### Wording only: the ENUM message in `IpcStream` (unreachable, not pinned)

The remote client reads the helper's frames with the shared IPC reader
(`ipc.rs`), which the browser driver uses too. When a stream with a
dictionary column can't be read, its error says the column "is an ENUM,
which DuckDB in the browser can't send yet". The helper always sends an
ENUM's dictionary ahead of its batches (`helper.rs`: new dictionaries go
first, in a frame of their own), so the remote driver can't reach this
message, and no remote test does; if it ever did, the message would name
the browser.

- Why: Decision 3 (the client decodes with the shared `ipc::Columns`).
- **Exempt from pinning**, being unreachable through the helper. The
  message itself is pinned on the driver that produces it: the browser
  driver's live suite, `src/lib/engine/engine-duckdb-browser.test.ts`,
  "an ENUM a non-SELECT returns gives the message (it already ran)". The
  remote side's ENUM path (dictionaries arriving and decoding) is pinned by
  `remote.rs` and the shared suites' ENUM cells.

## Not differences

- **Lossy Arrow settings.** A session that resets `arrow_lossless_conversion`
  used to make the remote driver misread UHUGEINT and BIT. Since Checkpoint
  H-1 the schema frame carries the native driver's own column kinds, so both
  drivers decode the same (`values.rs`'s CAST comparison, shared through
  `common/cast.rs`, runs on both).
- **`last_insert_id`** is `None` on both (`smoke.rs`'s
  `execute_has_no_last_insert_id`).

## Remote-only failure modes

These have no native counterpart, so the shared suites don't cover them;
`tests/remote.rs` and the helper's own tests do.

- The helper missing, of another version, or in a folder others can write:
  `ENGINE_NOT_INSTALLED` at open (Decisions 7 and 9). One that doesn't answer
  `hello` in time: `ENGINE_UNAVAILABLE`.
- The helper dying (a signal, a protocol break): every waiting and later
  call fails with `CONNECTION_CLOSED` (Decision 8). In process, the same
  failure would end the app.
- A client that stops reading for 30 s while frames wait loses its helper
  (exit 5, the wedge), then `CONNECTION_CLOSED`.
- The helper dying mid-call answers that call with `CONNECTION_CLOSED`
  whether or not DuckDB finished it: a `transaction` or `execute` may have
  committed. Core passes the code on (an apply's failed change; a page whose
  count fails with it fails the statement instead of estimating), and the
  TUI and the GUI treat such an apply as interrupted: the queue is kept and
  marked "may be partly applied" (probe F1, F2).
- `close` and the end of input wait for DuckDB's close checkpoint up to
  60 s in the helper. The client waits 2 s; after that a helper that took
  `close` (its output ended) is left to finish on its own, not killed, so a
  large WAL is written into the file rather than replayed on the next open
  (probe F3). Natively the in-process close finishes the checkpoint. A
  wedge (exit 5) doesn't wait, and the next open replays the WAL.
- While a helper let go after `close` is still closing a file, DuckDB's
  lock on that file is held (review I1 of the probe fixes). In this
  process, an open of the same file (its canonical path) waits for that
  helper to exit, at most 65 s (the helper's 60 s and a margin), and stops
  waiting when the open is dropped; through Core, the 30 s connect timeout
  can end the wait first. Across processes (a TUI or MCP server restarted
  meanwhile), an open that meets DuckDB's lock conflict is tried again
  every 250 ms for up to 10 s, then fails with `CONNECTION_ERROR` "DuckDB
  is still closing this file in another Seaquel process. Try again in a
  few seconds.", which names no process id or path. Natively there is
  nothing to wait for: the close finishes in process. A file held by any
  other program also gets that wording after 10 s.
