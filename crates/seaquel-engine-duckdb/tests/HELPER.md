# DuckDB through the helper

Every native interface (the desktop app, the TUI and the CLI's MCP server)
reaches DuckDB the same way: the remote driver (`src/remote/`, "the
client") over a `seaquel-duckdb` child process ("the helper", the loop in
`src/helper.rs` running `src/session.rs`). The in-process native driver is
gone; this file was `REMOTE.md`, which listed how the two differed. What it called differences
are now the limits below, stated as behaviour with the tests that pin them.

Every integration suite here that opens DuckDB gets its engine from
`common/engine.rs`, which installs the helper and returns the remote
engine. Decoding is checked against the frozen typed-cell fixture and
literal results, not against another driver (`fixtures/README.md`, "The
decoding reference").

## Running them

```bash
cargo build -p seaquel-duckdb

# Every suite through the helper, with no DuckDB in the test binaries
# (`remote` is the default feature):
cargo test -p seaquel-engine-duckdb

# The same, plus the helper's own loop and session code in process:
cargo test -p seaquel-engine-duckdb --features remote,helper
```

- The helper is `SEAQUEL_TEST_DUCKDB_HELPER`, else the `seaquel-duckdb`
  built beside the test binary (`target/<profile>/`, which `cargo test
  --workspace` builds). With neither, every suite fails naming `cargo build
  -p seaquel-duckdb`; nothing is skipped (`suite_helper.rs` pins the
  lookup). Each engine `common/engine.rs` makes installs it (a hard link,
  else a copy) as `bin/duckdb/<version>/seaquel-duckdb[.exe]` in a folder of
  its own under `CARGO_TARGET_TMPDIR`, laid out and checked as a real
  install is.
- The helper inherits the environment when it starts, so the restricted
  suites' temp `HOME` (and `USERPROFILE` on Windows) reaches it.
  `restricted.rs`'s `duckdb_s_home_is_the_temp_one` checks that DuckDB's `~`
  is the temp one.
- `seaquel-mcp`'s `tests/tools.rs` and Core's tests find the helper the
  same way (Core's `tests/common/duckdb.rs`; Core skips its DuckDB cases
  without one unless `SEAQUEL_TEST_REQUIRE_ENGINES` is set).

## Limits

### A request over the frame is refused (`INVALID_ARGUMENT`)

A call's SQL and bound values travel as one JSON control frame, at most
16 MiB (`MAX_FRAME`; the payload is `MAX_PAYLOAD`, 5 bytes less). A request
whose JSON is larger is refused by the client before anything is sent, with
`INVALID_ARGUMENT` ("The statement and its values take N bytes, more than
the … bytes the DuckDB helper takes in one call."), naming no SQL, and the
connection goes on. Core splits a script into statements, so only one huge
statement (a pasted `INSERT … VALUES` dump) reaches it.

- Why: frames above 16 MiB are a protocol error on either side, so the
  cap bounds what either process allocates for one frame.
- Pinned by: `frame_limits.rs`
  (`a_statement_past_the_frame_is_refused_before_it_is_sent`, a 17 MiB
  statement, no SQL in the message) and `remote.rs` (a 17 MiB request).

### A row over the frame is `RESULT_TOO_LARGE`

The helper sends rows as Arrow IPC batch frames. A batch is sliced so no
frame passes 8 MiB (`MAX_BATCH_FRAME`); a single row larger than that goes
alone, up to the 16 MiB frame; a row past that fails the call with
`RESULT_TOO_LARGE`, and the connection goes on.

- Why: as above.
- Pinned by: `frame_limits.rs` (`a_row_past_the_frame_is_too_large`, a
  20 MiB cell; `a_row_under_the_frame_arrives`, a 12 MiB one), `remote.rs`,
  and `seaquel-mcp`'s `duckdb_runs_in_the_helper`.

### Wording only: the ENUM message in `IpcStream` (unreachable, not pinned)

The client reads the helper's frames with the shared IPC reader
(`ipc.rs`), which the browser driver uses too. When a stream with a
dictionary column can't be read, its error says the column "is an ENUM,
which DuckDB in the browser can't send yet". The helper always sends an
ENUM's dictionary ahead of its batches (`helper.rs`: new dictionaries go
first, in a frame of their own), so the client can't reach this message,
and no remote test does; if it ever did, the message would name the
browser.

- Why: the client decodes with the shared `ipc::Columns`.
- **Exempt from pinning**, being unreachable through the helper. The
  message itself is pinned on the driver that produces it: the browser
  driver's live suite, `src/lib/engine/engine-duckdb-browser.test.ts`,
  "an ENUM a non-SELECT returns gives the message (it already ran)". The
  helper's ENUM path (dictionaries arriving and decoding) is pinned by
  `remote.rs`, the shared suites' ENUM cells and the reference's ENUM
  result.

### Also as it is

- **Lossy Arrow settings.** A session that resets `arrow_lossless_conversion`
  can't change the cells: the schema frame carries the column kinds read
  from DuckDB's logical types (`src/kinds.rs`, frozen in
  `fixtures/kinds.json`), so UHUGEINT and BIT decode the same either way
  (`values.rs`'s CAST comparison through `common/cast.rs`, and `remote.rs`'s
  `cells_decode_the_same_whatever_the_session_s_arrow_settings`).
- **`last_insert_id`** is `None` (`smoke.rs`'s
  `execute_has_no_last_insert_id`).

## Failure modes

What running DuckDB in a process of its own adds. The shared suites don't
cover these; `tests/remote.rs`, the helper's own tests and Core's
`duckdb_remote.rs`, `lost_connection.rs` and `exclusive_reconnect.rs` do.

- The helper missing, of another version, or in a folder others can write:
  `ENGINE_NOT_INSTALLED` at open. One that doesn't answer
  `hello` in time: `ENGINE_UNAVAILABLE`.
- The helper dying (a signal, a protocol break): every waiting and later
  call fails with `CONNECTION_CLOSED`; the app and the
  terminal binaries stay up. `Driver::closed()` resolves with that error
  when the helper ends without `close` (and with `None` after `close`, a
  closing helper let go, or a dropped driver), and Core then takes the
  connection out and announces it once as `ConnectionClosed` with
  `CONNECTION_CLOSED` (pinned by `remote.rs`'s `closed_*` tests, Core's `lost_connection.rs` and `duckdb_remote.rs`).
- At most 16 read-only calls (`readOnly`, `explainReadOnly`) run in one
  helper at once, each on a clone and a thread of its own. The client
  sends no more than that: a 17th waits for a slot, which a call holds
  until its last frame arrives (a dropped call's included, once the helper
  has let it go). The helper's `TOO_MANY_REQUESTS` past 16 is only a
  backstop (pinned by `read_only_calls_past_sixteen_wait_for_a_slot` and `dropped_read_only_calls_free_their_slots`).
- A file a live helper of this process holds isn't opened a second time:
  the open is refused at once with `CONNECTION_ERROR` "This DuckDB file is
  already open in another connection. Disconnect it first." (no path),
  whatever the path's spelling or a hard link (files are compared by
  device and inode on Unix, by canonical path elsewhere) and from any
  engine value in the process. `:memory:` opens as often as asked. The
  file is free again once its connection is closed or dropped, or its
  helper died; from the start of its connection's `close` an open waits
  for the helper instead of being refused (below). In process, the
  deleted native driver's second open succeeded and lost committed writes
  (pinned by `a_second_open_of_an_open_file_is_refused_at_once`, `a_hard_link_to_an_open_file_is_refused_at_once` and `a_connect_beside_a_closing_connection_of_the_same_file_waits_and_opens`).
  Where the key can mislead: on a network filesystem (NFS, SMB) device and
  inode numbers may not be stable or unique across mounts, so two paths to
  one file could compare different (the second open then meets DuckDB's
  lock, if the filesystem honours it, and the 10 s retry); an inode freed
  by a deleted file can be reused by a new one, which only matters while
  the old file's helper is still alive, and that helper holds the old
  inode open, so the number isn't free yet; on Windows a hard link has its
  own canonical path, so two links to one file aren't matched and the
  second open meets DuckDB's lock instead.
- A client that ends with `process::exit` (the app's quit) leaves its
  helpers to read EOF, checkpoint and exit 0 within their 60 s bound
  (pinned by `a_client_that_exits_leaves_its_helper_to_checkpoint`).
- Started from an AppImage (`APPIMAGE` set), the helper's environment loses
  the `LD_LIBRARY_PATH` and `LD_PRELOAD` entries under `$APPDIR`:
 it links the system's libraries and can outlive
  the AppImage's mount while it checkpoints. Pinned by
  `process.rs`'s `appimage_library_paths_are_kept_from_the_helper`.
- A client that stops reading for 30 s while frames wait loses its helper
  (exit 5, the wedge), then `CONNECTION_CLOSED`.
- The helper dying mid-call answers that call with `CONNECTION_CLOSED`
  whether or not DuckDB finished it: a `transaction` or `execute` may have
  committed. Core passes the code on (an apply's failed change; a page whose
  count fails with it fails the statement instead of estimating), and the
  TUI and the GUI treat such an apply as interrupted: the queue is kept and
  marked "may be partly applied".
- `close` and the end of input wait for DuckDB's close checkpoint up to
  60 s in the helper. The client waits 2 s; after that a helper that took
  `close` (its output ended) is left to finish on its own, not killed, so a
  large WAL is written into the file rather than replayed on the next open.
 A wedge (exit 5) doesn't wait, and the next open replays the
  WAL.
- While a helper let go after `close` is still closing a file, DuckDB's
  lock on that file is held. In this
  process, an open of the same file waits for that helper to exit, at
  most 25 s (under Core's 30 s connect timeout), then fails with
  `CONNECTION_ERROR` "DuckDB is still saving this file. Try again in a few
  seconds." (no path; pinned by
  `a_file_still_saving_past_the_wait_is_refused_in_plain_words`), and stops
  waiting when the open is dropped. Across processes (a TUI or MCP server restarted
  meanwhile), an open that meets DuckDB's lock conflict is tried again
  every 250 ms for up to 10 s, then fails with `CONNECTION_ERROR` "DuckDB
  is still closing this file in another Seaquel process. Try again in a
  few seconds.", which names no process id or path. A file held by any
  other program also gets that wording after 10 s. A file a live helper of
  this process holds is refused at once instead (above). A window that
  reconnects the same saved connection to a file closes its older
  connection right before the open, after the checks that can refuse
  without opening anything (`Engine::exclusive_file` and `Engine::preflight`; pinned by Core's `exclusive_reconnect.rs` and `a_window_reconnecting_a_duckdb_file_replaces_its_old_connection`).
