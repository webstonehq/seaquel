# DuckDB Helper Implementation Plan: DuckDB out of the terminal binaries

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** planned; owner answered Q1–Q8; ready to execute; not started. The owner took every recommendation on 2026-10-03 ("Answered questions"). Surveyed on 2026-10-03 at HEAD `60f44da` (Phase 6) with phase 7a's work uncommitted in the tree; line numbers are as of that tree. Spikes ran the same day (see "Spikes"); their code, scripts and raw results are in the session scratchpad (`…/scratchpad/duckhelper/`: `bench/`, `run-bench.sh`, `results.jsonl`, `results-linux.jsonl`, `serbench*.jsonl`, `lifecycle.txt`, `sizes/`), with the build directories deleted. No spike touched the real keychain, data dir or `~/.ssh`; every DuckDB in them was in memory.

**Goal:** `seaquel-tui` and `seaquel-cli` stop linking DuckDB. A small `seaquel-duckdb` helper binary, version-matched to them, holds DuckDB; it is downloaded the first time a user connects to a DuckDB database (size and SHA-256 checked, as the app's CLI download is), and the terminal binaries talk to it over stdin/stdout pipes. A user who never opens a DuckDB file downloads about 8 MB for the TUI instead of 19 MB. A user who does gets the same behaviour as today: the existing DuckDB suites run against the remote driver and pass.

**The owner's question, answered first: pipes.** Measured on an M4 Max and in a Linux VM on the same machine, the channel is not where the time goes. Turning DuckDB's Arrow into Seaquel `Value`s costs 362 ms for 10M rows and has to happen in any design; moving the same rows through a pipe as Arrow IPC costs about 50 ms more than reading them in process, and because the helper produces while the client decodes, a 10M-row stream finished in 723 ms through a pipe against 773 ms in one process running the same decode loop (958 ms through today's `DuckdbDriver`). Shared memory was no faster (715 ms) and brought slot bookkeeping that deadlocked in the spike. A socketpair with 4 MiB buffers moves raw bytes 3× faster than a pipe, but with macOS's default 8 KiB buffers it is 5× slower, and with big buffers a `SELECT 1` waited 9 ms behind buffered batches; it also has no Windows equivalent that a child can inherit. TCP adds 10 µs per round trip on macOS and a listening port. Pipes are the one channel that is the same on macOS, Linux and Windows, has no name anything else can open, and is already within a few percent of the fastest option. The numbers are in "Spikes"; Q1 has the options.

**Architecture:**
- **`seaquel-engine-duckdb`** gets two features beside `native` and `browser`: `remote`, the client half (a `Driver` over a child process; no `duckdb` crate), and `helper`, the server half (needs `native`; the call loop the helper binary runs). The native driver's blocking code (open, binds, prepare, transactions, the read-only path, the interrupt guard) is split from its `Value` decoding, so the native driver and the helper run the same DuckDB code and differ only in where the Arrow chunks go. The browser driver's IPC reader (`browser/ipc.rs`) moves up to `ipc.rs` and serves both the browser and the remote driver; `dialect.rs`, `introspect.rs` and `decode.rs` are shared by all three drivers.
- **`crates/seaquel-duckdb`** (new): the `seaquel-duckdb` binary, a dozen lines calling the engine crate's `helper::serve(stdin, stdout)`. Its own crate class in the crate rules ("engine host": may use its engine crate and the pure crates only).
- **Wire:** length-prefixed frames on the child's stdin and stdout. Control messages are JSON (`ConnectConfig`, `Value` params, `BatchStatement`s and `DbError` already have serde); rows are Arrow IPC stream messages, decoded on the client by `decode.rs` with `Kind::of_field`, which `decode_from_ipc_matches_decode_from_duckdb` already proves equal to the native decoding for every typed-cell case.
- **One helper process per open DuckDB connection**, spawned by `Engine::open`, closed by `Driver::close` or drop (Q2). Calls carry ids; the helper runs the main session's calls in turn on one thread, as the native driver does, and read-only calls on per-call clones beside them; each streaming call has a credit window so one stream never fills the pipe in front of another call's answer.
- **`seaquel-core`:** the `engine-duckdb` feature becomes the native driver only; a new `engine-duckdb-remote` feature and `CoreBuilder::duckdb_helper(DuckdbHelper { dir, version })` register the remote engine; `Core::duckdb_helper_status()` and `Core::duckdb_helper_install(progress)` (behind `duckdb-helper-install`, which needs `seaquel-http`) find, download and verify it.
- **`seaquel-http`:** a `release_asset` module, the download and check `src-tauri/src/cli_download.rs` does today, made async, streaming, with progress and the extra roots and proxy plan the crate already has.
- **`seaquel-terminal`** builds Core with `with_plugins(|id| id != "duckdb")` and `.duckdb_helper(…)`, so the terminal binaries get the remote engine whatever Cargo unified. A connect to a DuckDB connection with no helper installed fails at once with `ENGINE_NOT_INSTALLED`; the TUI turns that into a download dialog (Q5), the MCP server into a tool error naming `seaquel-cli duckdb install` (Q4).
- **Release:** `build-cli.mjs --bin seaquel-duckdb`; `release.yml` builds, signs and uploads `seaquel-duckdb-<triple>[.exe]` (gzip, Q8) with the CLI and the TUI.
- **The desktop app keeps its in-process DuckDB** (Q6). The demo's browser driver is untouched except that its IPC reader moves one directory up.

**Tech stack:** Rust (`seaquel-engine-duckdb`, `seaquel-duckdb`, `seaquel-core`, `seaquel-http`, `seaquel-terminal`, `seaquel-tui`, `seaquel-cli`, `seaquel-mcp`), duckdb-rs 1.10505 (DuckDB 1.5) with `bundled` and `json` as today, arrow-ipc/arrow-array 58.4 (the versions `Cargo.lock` has), tokio's `process` for the child, `flate2` (already in `Cargo.lock`) for the gzip asset. No TypeScript changes.

**Inputs:**
- Phase 7a's "Binary size" study and its follow-ups ("A terminal build without DuckDB (variant D)", "The TUI's connection picker marks engines a build lacks").
- The design doc's "Why compile-time plugins": it rejected subprocess plugins for third-party plugins, on complexity. This plan doesn't reopen that: the helper is one first-party engine behind the same `Engine`/`Driver` traits, chosen for binary size, and nothing in Core knows the driver is remote. The design doc's "Plugin kinds" and "Terminal binaries: licensing and distribution" get an "as built" note (Task 10).
- Phase 8's browser driver (`src/browser/`), which is already a `Driver` over a message bridge with Arrow IPC results: its `ipc.rs` is reused as is, its cancel-on-drop rule (a cancel posted synchronously from `Drop`, ordered before the next call) is copied.
- `src-tauri/src/cli_download.rs` for the download and check; `release.yml`'s `publish-cli` job and the per-target CLI and TUI steps for the release.
- The phase 6 and 7a effort logs for the estimate.

**Naming.** "The helper" is the `seaquel-duckdb` process. "The client" is the remote driver in the TUI's or CLI's process. "The terminal binaries" are `seaquel-cli` and `seaquel-tui`. "A call" is one `Driver` method invocation crossing to the helper. "The main session" is the helper's one DuckDB connection for ordinary calls; "a clone" is a per-call `try_clone()` for the read-only path, as natively.

---

## What the code shows

### DuckDB in the terminal binaries today

1. **Every Core dependency in the terminal crates takes Core's default features**, which are every engine (`crates/seaquel-core/Cargo.toml:8`): `seaquel-terminal` (`Cargo.toml:12`), `seaquel-tui` (`:16`), `seaquel-cli` (`:17`) and `seaquel-mcp` (`:11`). Removing DuckDB needs `default-features = false` on all four plus the other engines named; with that, `cargo tree -p seaquel-tui -e normal` and `-p seaquel-cli` show no `duckdb`, `libduckdb-sys` or `arrow` crate (checked in a scratch copy).
2. **`seaquel-terminal::core_builder` registers engines through `with_default_plugins()`** (`crates/seaquel-terminal/src/core.rs:38`), and its test pins all five ids (`:77-81`). `with_plugins(allow)` (`crates/seaquel-core/src/lib.rs:666`) exists for the server's "features aren't a security boundary" rule and fits here too: a workspace test build compiles the native DuckDB into the TUI's test binary anyway.
3. **Core has no DuckDB-specific code** beyond registering `seaquel_engine_duckdb::engine()` under `engine-duckdb` (`lib.rs:44`, `:687-688`). Connect goes through `Engine::open` under `CONNECT_TIMEOUT`, 30 s (`lib.rs:382`); a download inside `open` would race that limit, which is why the download happens before the connect (Decision 10).
4. **The CLI has no HTTP client**, and CI checks it (`ci.yml:103-108`: `seaquel-cli` must not depend on `reqwest`). The TUI has one through `ai-native` (`seaquel-http`).

### The native driver

5. **Its DuckDB work is blocking functions over a `Connection`** (`crates/seaquel-engine-duckdb/src/driver.rs`): `open`/`open_sessions` with `duckdb_config`, `restricted` and `create_if_missing` (`:218`, `:262`, `restricted_config` `:166`), binds (`to_duckdb_param`), `prepare` with the cancelled check, `query_capped` (`:441`), `stream_blocking` (`:491`, 5,000-row batches), `transaction_blocking` with the `txid_current()` probe (`:555`), `read_only_blocking` on a clone with `SELECT * FROM query(?) LIMIT cap+1` (`:657`) and `explain_read_only_blocking`. Each produces `Value`s through `ResultReader` (`:313`), which reads `stmt.step()` chunks and decodes each cell with kinds from DuckDB's **logical types** (`kind_of`).
6. **Cancel is `blocking.rs`'s `Call`/`Worker` pair**: dropping the `Call` interrupts DuckDB and flags the worker, only while that call holds the connection, so a late interrupt can't reach the next call. The helper needs exactly this, driven by a cancel frame instead of a dropped future.
7. **Introspection is driver-generic**: `introspect::calls::{list_schemas, schema_tables, table_metadata, statistics, explain}` take `&dyn Driver` and run SQL through `query` (`introspect.rs:740-826`). A remote driver gets introspection by implementing `query`; the helper needs no introspection messages.
8. **`Statement::execute` materialises the whole result before the first chunk** (the driver's own comment at `:477-483`). duckdb-rs 1.10505 also has `stream_arrow` (streaming execution), whose iterator **panics** on a fetch error, an interrupt included (seen 30 times in the spike's cancel runs); `Statement::step()` after it returns the error instead.

### The browser driver as prior art

9. **`browser/ipc.rs`'s `Columns` and `IpcStream`** turn IPC bytes into rows under a `RowCap` with `Kind::of_field`, and are built natively for tests already. `KindRules { decimal38_is_hugeint }` exists only for DuckDB-WASM 1.4.3; native DuckDB 1.5 with `arrow_lossless_conversion` marks HUGEINT with extension metadata, so the remote driver uses the default rules.
10. **`decode_from_ipc_matches_decode_from_duckdb`** (`ipc.rs:325`) writes native DuckDB's Arrow as IPC and checks that reading it back with `Kind::of_field` gives the native driver's cells for every typed-cell case, chunked results, an empty result and an ENUM. That is the remote driver's decoding, already pinned.
11. **What doesn't carry over:** the literal binds (`binds.rs`; the helper binds natively, as the desktop does), the ENUM re-run through `runQuery` (native IPC carries dictionaries), and the single-threaded `Rc` plumbing. What does: the cancel-on-drop guard posting its request before returning, one turn at a time on the main session, read-only calls on their own connection rolled back on every outcome.

### Distribution today

12. **`cli_download.rs`** reads the asset's size and `digest` (`sha256:…`) from GitHub's release API, streams into a temp file in the target directory with a 200 MiB cap, checks size and hash, marks it 0755 and renames it into `<data_local_dir>/<identifier>/bin/`, with a `.version` file beside it. It uses reqwest's blocking client with a `Seaquel/<version>` User-Agent. No quarantine attribute is set (reqwest doesn't), so Gatekeeper doesn't assess the file; the release signs it with the Developer ID and the hardened runtime anyway.
13. **`release.yml`** builds, codesigns (macOS, `--options runtime`, `src-tauri/macos/entitlements.plist`) or trusted-signs (Windows) and uploads `seaquel-cli-<triple>` and `seaquel-tui-<triple>` for six targets (`:112-159`), then `publish-cli` uploads every `seaquel-cli-*` and `seaquel-tui-*` artifact (`:189-209`).
14. **`check-crate-deps.mjs`** has no class for a binary that hosts an engine; every crate must be classified. CI's "Terminal binaries' dependencies" step (`ci.yml:86-109`) is where "no `libduckdb-sys`" belongs.

---

## Spikes (2026-10-03)

**Machine and conditions.** Apple M4 Max (16 cores), 64 GB, macOS 26.1, on mains power, nothing else heavy running. rustc 1.96.1. Timings use the default `release` profile (no LTO), one fresh client process per run so peak RSS is per run; large workloads ran 3 times and the table shows the median of the last two. Linux: the same machine's Docker VM (OrbStack, kernel 7.0, aarch64, 16 vCPU, 16 GB), Debian bookworm, `rust:1.96-bookworm`, built offline from vendored crates. DuckDB was duckdb-rs 1.10505.0 (`bundled`, `json`), in memory, with `arrow_lossless_conversion` on, as the native driver opens it. The spike helper speaks a framed protocol (`[kind][stream][len][payload]`), runs one worker thread and one DuckDB connection per stream id, and writes Arrow IPC with `arrow_ipc::writer::StreamWriter`; the client decodes with a copy of `decode.rs` and `Kind::of_field`. "In-process (driver)" is today's `DuckdbDriver::query_stream`; "in-process (same decode)" is duckdb-rs's Arrow decoded by the same loop the client uses, in one process.

Channels: **pipe** (the child's stdin/stdout), **socketpair** (AF_UNIX `SOCK_STREAM`, the child's end inherited as fd 3; macOS default 8 KiB buffers), **socketpair 4M** (`SO_SNDBUF`/`SO_RCVBUF` 4 MiB both ends), **TCP** (loopback, `TCP_NODELAY`), **shm** (a 64 MiB `shm_open` object, unlinked at once and inherited as fd 4, or `memfd` on Linux; four 16 MiB slots, IPC written straight into a slot and read in place with `Buffer::from_custom_allocation`; descriptors and releases over the socketpair). `SOCK_SEQPACKET` wasn't tried: it caps a message at the socket buffer, so frames would need fragmenting, and it exists only on Linux. Arrow Flight wasn't tried: it is gRPC over HTTP/2 with protobuf framing on top of the same IPC bytes, so it can only add to the socket's numbers.

**S1. The channel alone.** A 4 GiB pump in 1 MiB frames, and an empty request/reply (`PING`), 10,000 round trips:

| Channel | macOS throughput | macOS RTT p50 / p99 | Linux throughput | Linux RTT p50 / p99 |
|---|---|---|---|---|
| pipe | 7.3 GB/s | 4.0 / 8.5 µs | 10.3 GB/s | 1.3 / 1.7 µs |
| socketpair | 1.5 GB/s | 4.7 / 9.5 µs | 11.5 GB/s | 1.6 / 2.9 µs |
| socketpair 4M | 24.7 GB/s | 4.6 / 9.6 µs | 12.4 GB/s | 1.6 / 3.0 µs |
| TCP loopback | 17.3 GB/s | 14.5 / 52.8 µs | 17.7 GB/s | 2.3 / 7.1 µs |
| shm (control over socketpair) | — | 4.5 / 9.6 µs | — | 1.5 / 2.9 µs |

Every channel moves bytes far faster than anything downstream consumes them (S3: the client decodes about 360 MB/s of IPC into `Value`s). macOS's default socketpair buffer is the one configuration that could matter, and only for wide cells.

**S2. Small calls.** `SELECT 1`, 10,000 calls after 200 warm-up, through the full path (request frame, DuckDB, IPC schema and batch, decode to `Value`):

| | macOS p50 / p99 | Linux p50 / p99 |
|---|---|---|
| pipe | 51.8 / 64.8 µs | 87.8 / 125.3 µs |
| socketpair | 50.8 / 63.6 µs | 91.4 / 136.0 µs |
| socketpair 4M | 52.0 / 65.7 µs | 90.7 / 139.0 µs |
| TCP | 65.5 / 94.7 µs | 82.8 / 123.2 µs |
| shm | 51.8 / 65.7 µs | 61.4 / 125.3 µs |
| in-process (driver, `query`) | 45.5 / 62.2 µs | 76.8 / 613.9 µs |
| in-process (duckdb-rs, no driver) | 34.5 / 46.0 µs | 42.8 / 79.5 µs |

A call costs about 6 µs more than today's driver on macOS and 11 µs on Linux. The browsing the TUI does (a page, a table's metadata: a few calls each) can't feel that.

**S3. Large results.** Wall time from the request to the last row decoded into `Value`s, first batch in brackets; "wire" is the bytes that crossed.

`big3`: 10M rows × (BIGINT, DOUBLE, short VARCHAR), 262 MB of IPC.

| | macOS | Linux |
|---|---|---|
| pipe | 723 ms (179 ms) | 723 ms (169 ms) |
| socketpair | 873 ms | 614 ms |
| socketpair 4M | 718 ms | 656 ms |
| TCP | 719 ms | 611 ms |
| shm | 715 ms | 699 ms |
| pipe, streaming execution | **563 ms (0.9 ms)** | 569 ms (0.9 ms) |
| socketpair 4M, streaming | 561 ms (1.0 ms) | 441 ms (1.0 ms) |
| in-process (driver) | 958 ms (180 ms) | 2,212 ms (187 ms) |
| in-process (same decode) | 773 ms (181 ms) | 601 ms (166 ms) |
| in-process (same decode), streaming | 757 ms (0.9 ms) | 612 ms (0.9 ms) |
| pipe, rows as JSON `Value`s | 1,286 ms (352 MB) | 2,210 ms |
| pipe, rows as bincode `Value`s | 1,461 ms (485 MB) | 2,227 ms |

Transfer alone (the client reads frames and doesn't decode): pipe 320 ms against 270 ms for duckdb-rs's Arrow in process, so **moving 262 MB costs about 50 ms** on macOS (IPC encode 38 ms, decode 16 ms, the rest the copy).

`mixed20`: 1M rows × 20 columns (integers, BOOLEAN, DOUBLE, FLOAT, DECIMAL(18,3), two VARCHARs, UUID, DATE, TIMESTAMP, TIMESTAMPTZ, INTERVAL, BLOB, LIST, STRUCT, HUGEINT, a NULL-heavy column, TIME), 218 MB of IPC: macOS pipe 2,039 ms, socketpair 4M 2,104, TCP 2,102, shm 2,097, streaming pipe 1,753; in process 2,173 (driver) and 2,041 (same decode). Linux pipe 1,862, socketpair 1,830, TCP 1,773, shm 1,772, streaming pipe 1,779; in process 3,727 (driver) and 1,810. The decode dominates (1,593 ms of it on macOS, S5); the channel is noise.

Wide cells, where the copy is all there is:

| | `widetext`: 2,000 × 64 KiB VARCHAR (125 MB) macOS / Linux | `wideblob`: 100 × 1 MiB BLOB (100 MB) macOS / Linux |
|---|---|---|
| pipe | 206 / 324 ms | 30 / 67 ms |
| socketpair (8 KiB on macOS) | 279 / 322 ms | 87 / 68 ms |
| socketpair 4M | 193 / 313 ms | 21 / 68 ms |
| TCP | 213 / 318 ms | 31 / 69 ms |
| shm | 272 / 315 ms | 86 / 68 ms |
| in-process (driver) | 177 / 278 ms | 12 / 36 ms |

The helper path costs 18–46 ms per 100 MB of cells here: one IPC copy into a frame, the kernel copy, and the `Value` copy the in-process path also makes. Shared memory lost to the pipe on macOS: the helper waited for slots the client released one frame later.

**Peak RSS.** Non-streaming, the helper holds what DuckDB materialised (big3: 335–347 MB) and the client 9 MB; in process, one process holds 336–349 MB. With streaming execution the helper held 25 MB for big3 (30 MB mixed20). Wide cells: widetext's helper peaked at 908 MB and the client at 134 MB against 784 MB in process (an IPC copy of a 2,048-row chunk of 64 KiB cells is 128 MB in flight), which is why Decision 3 caps a frame at 8 MiB by slicing batches.

**S4. Cancel.** A `sum` over 3e9 rows, cancelled 150 ms in, timed from sending the cancel to receiving the helper's error (sent after DuckDB returned): pipe p50 68 µs, p99 1.7 ms (Linux 63 µs, 1.2 ms); in process, from `interrupt()` to `query` returning: 59 µs, 1.5 ms (Linux 37 µs, 1.0 ms). The p99 is DuckDB's interrupt check interval in both cases. Mid-stream (cancel after big3's first batch, then drain until the call's end): pipe 236 µs with 5 frames already in flight (Linux 517 µs, 3 frames); with streaming execution 220 µs.

**S5. Serialisation alone** (one process, the query's Arrow batches already in memory), macOS:

| | big3 (10M rows) | mixed20 (1M × 20) | wideblob (100 × 1 MiB) |
|---|---|---|---|
| Arrow IPC encode / decode | 38 / 16 ms (262 MB) | 25 / 7 ms (218 MB) | 10.6 / 0.02 ms (100 MB) |
| Arrow → `Value` rows (`decode.rs`) | 362 ms | 1,593 ms | 4.9 ms |
| `Value` rows → JSON / back | 322 / 987 ms (352 MB) | 369 / 1,565 ms (379 MB) | 63 / 42 ms (133 MB) |
| `Value` rows → bincode / back | 571 / 611 ms (485 MB) | 524 / 595 ms (426 MB) | 53 / 64 ms (100 MB) |

Linux, big3: IPC 57 / 21 ms, Arrow → `Value` 670 ms, JSON 312 / 1,084 ms, bincode 724 / 651 ms. **Arrow IPC is close to free; anything that ships `Value`s pays the decode in the helper and a full encode and decode on top**, and leaves the decode on one side instead of overlapping it with DuckDB.

**S6. Several calls on one channel.** `SELECT 1` on a second stream id while big3 streams on the first, 300 samples:

| | macOS p50 / p99 | Linux p50 / p99 |
|---|---|---|
| pipe, materialised big3 | 54 / 108 µs | 57 / 1,733 µs |
| pipe, streaming big3 | 244 / 447 µs | 378 / 3,723 µs |
| socketpair 4M, materialised | 55 / 283 µs | 62 / 2,040 µs |
| socketpair 4M, streaming | **8.9 ms** / 10.3 ms (78 samples before big3 ended) | **7.6 ms** / 12.6 ms |

A small answer waits behind whatever is already in the channel. A pipe holds 64 KiB; a 4 MiB socket buffer holds 4 MiB of batches the client decodes at about 470 MB/s, so 9 ms. Big buffers are the wrong trade for an interactive client; per-call credit windows (Decision 5) keep the queue short whatever the channel. The shm multiplex runs hung: a slot taken by the second stream's reply was never released by the harness, and the first stream waited for a free slot forever. A real implementation can account for that, but it is a class of bug the pipe doesn't have.

**S7. Lifecycle.** The parent killed with `kill -9` mid-stream: the helper, blocked in a write or in DuckDB, was gone 24–25 ms later (its reader thread saw EOF and exited; measured with a Python clock, so the true figure is lower). The helper killed mid-stream: the client's next read hit EOF 15–25 ms later. Both over pipes and socketpairs. One trap found on the way: with a socketpair, the client's own `try_clone()` of its end kept the channel open after it dropped the writer, so the helper never saw EOF; pipes have separate read and write ends and don't have this failure.

**S8. Startup and sizes.** Spawn, handshake and an in-memory DuckDB open: macOS 9.4–9.9 ms p50 on every channel (p99 10.2–13.4 ms), Linux 4.7 ms. The first exec of a freshly copied binary on macOS (what a download is) took 16.1–17.5 ms, then 9.8 ms warm. For comparison, the in-process driver's first `open` in a fresh process took 36 ms on macOS and 11 ms on Linux. The CONNECT_TIMEOUT is 30 s; neither matters.

Sizes, macOS arm64, `terminal-release`-style profile (fat LTO, one codegen unit, stripped):

| Binary | Raw | gzip -9 | xz -9 |
|---|---|---|---|
| Today's `seaquel-tui` / `seaquel-cli` (7a's variant B) | 50.12 / 46.17 MB | 18.89 / 17.15 MB | — |
| `seaquel-tui` without DuckDB (rebuilt here) | 16.21 MB | 7.76 MB | — |
| `seaquel-cli` without DuckDB | 12.25 MB | 6.02 MB | — |
| The remote client's half (arrow-ipc, arrow-array, `decode.rs`), over a bare binary | +1.54 MB | +0.54 MB | — |
| `seaquel-cli` + a reqwest download and SHA-256 (Q4) | +0.71 MB | +0.34 MB | — |
| Helper: the spike protocol + IPC writer | 34.48 MB | 11.40 MB | 6.90 MB |
| Helper with the native driver linked in (a stand-in for `seaquel-duckdb`) | 35.11 MB | 11.70 MB | 7.09 MB |

So after the change: the TUI about 17.8 MB raw (8.3 MB gzip), the CLI about 14.5 MB (6.9 MB, with Q4 A), and the helper about 35 MB (11.7 MB gzip) once, shared by both. The no-DuckDB TUI and CLI took 106 s and 71 s to build; the fat-LTO helper 78 s.

**S9. What the spikes settle for the design.**
- Pipes (Q1 A).
- Arrow IPC for rows, decoded on the client (Decision 3); JSON for control, which only carries small things.
- The helper should stream (`execute_streaming`) for `query_stream`: first batch in under 1 ms instead of 180 ms and the helper's memory at 25 MB instead of 335 MB for big3 (Q3). Through `Statement::step()`, never the panicking iterator.
- Frames capped by slicing batches (widetext's 128 MB chunk); credit windows per streaming call (S6).
- EOF is enough for the helper to notice its parent is gone (S7); Linux gets `PR_SET_PDEATHSIG` as a backstop.
- Today's native driver is slower than a plain decode loop over the same Arrow (958 against 773 ms on macOS, 2,212 against 601 ms on Linux for big3). Not this plan's problem, but the session split in Task 1 is the moment to see why (a follow-up).

**Windows (not measured; no Windows machine here).** The channel is the child's stdin and stdout, created by `CreatePipe` through tokio's `Command`, inherited by the helper only and closed with the parent. Anonymous pipes on Windows are synchronous, so tokio reads them on its blocking pool, one thread per stream of bytes; that is fine for one or a few helpers. Expected throughput is lower than on macOS or Linux, but S3 needs only the decode rate (about 360 MB/s for big3, 110 MB/s for mixed20). AF_UNIX exists on Windows 10 1803+ but has no `socketpair` and can't pass a handle, so it would need a path in the filesystem with its own access rules; named pipes need a name and a DACL. The plan assumes anonymous pipes at 1 GB/s or more and puts a Windows measurement in the manual checks (and a CI benchmark in the follow-ups); if they turn out slow, a named pipe created with `FILE_FLAG_FIRST_PIPE_INSTANCE`, a random name and an owner-only DACL is the fallback, behind the same framing.

---

## Answered questions (2026-10-03)

The owner answered Q1–Q8 on 2026-10-03, each with the recommendation. Each keeps the options that were weighed, so later changes start from them.

### Q1. The channel

- **A: the child's stdin and stdout (anonymous pipes).** Same code on every platform (`tokio::process`), nothing anyone else can open, EOF on both sides when either dies. Within a few percent of the fastest channel on every workload that decodes rows (S3); 18–46 ms slower per 100 MB of wide cells than in process. The helper must never print to stdout (a rule the MCP server already lives by). Cost: in the plan.
- **B: a socketpair on Unix (4 MiB buffers), pipes on Windows.** Faster on raw bytes on macOS (S1) and on wide BLOBs (21 against 30 ms per 100 MB), the same elsewhere, and worse latency for a small call behind a stream unless credit windows hold the queue short (they do, Decision 5). Two transports to test and keep, and the `try_clone` EOF trap (S7). +1–1.5 h.
- **C: shared memory with a pipe for control.** No gain measured (S3, S6), slot accounting across calls (the spike deadlocked), and a third mechanism per platform (`shm_open`, `memfd`, file mappings). +3–4 h.

**Recommended: A.** It answers the question as asked: no channel measured is meaningfully faster for what Seaquel does with the rows, and the pipe is the simplest secure one on all three platforms. B stays possible later behind the same framing if a wide-BLOB workload ever matters.

**Answer (owner): A.** Decisions 2, 3 and 5.

### Q2. How many helper processes

- **A: one per open DuckDB connection.** `Engine::open` spawns it, `close` or drop ends it. A crash, an OOM kill or a DuckDB abort takes down one connection; a restricted instance (the MCP server's) never shares a process with an unrestricted one; two connections to one file meet DuckDB's own file lock across processes and fail cleanly. Costs about 10 ms and about 20 MB of RSS per connection, and one more process per open DuckDB connection.
- **B: one per Core, every DuckDB connection inside it.** One spawn, shared memory, but a crash ends every DuckDB connection, and restricted and unrestricted instances share an address space. Same code size, more lifecycle rules.
- **C: one per workspace.** Neither benefit.

**Recommended: A.** The TUI holds one connection at a time and the MCP server a handful; 10 ms and 20 MB each is nothing next to what DuckDB itself uses.

**Answer (owner): A.** Decisions 4 and 8.

### Q3. Streaming execution

S3: with `execute_streaming` the first batch arrives in under 1 ms instead of after DuckDB materialised the result (180 ms for big3), and the helper peaks at 25 MB instead of 335 MB.
- **A: in the shared session code, for `query_stream` only**, so the desktop's native driver gets it too. Its error can come mid-stream instead of before the first batch (the stream already carries errors, and Core's runs already handle one), and a page that Core cuts at `pageSize + 1` stops DuckDB earlier. +0.3 h in Task 1, plus the existing suites as the check.
- **B: the helper only.** The two drivers then differ in when a failing query fails and in memory; the parity suites would need to allow it.
- **C: not now.** Today's behaviour on both.

**Recommended: A.** It is the largest user-visible win in the spikes, and doing it in shared code keeps one behaviour.

**Answer (owner): A.** Decision 1 and Task 1.

### Q4. How the CLI (the MCP server) gets the helper

The MCP server can't ask anything; a DuckDB connection without the helper is a tool error.
- **A: `seaquel-cli duckdb install` (and `status`).** Adds `seaquel-http` to the CLI, +0.71 MB raw, +0.34 MB gzip (S8), and CI's "no reqwest in the CLI" rule goes. The tool error says to run it.
- **B: no download in the CLI.** The error says to connect once from the TUI, or to use the app's install (C), or the README's manual steps. No new dependency; a user with only the CLI needs a second tool or `curl`.
- **C: A, and the app's "Install Command Line Tool…" fetches the helper beside the CLI** (`cli_download.rs` taking the asset name, one more download in the same flow). +0.5 h in Task 8; the desktop's own DuckDB is unaffected.

**Recommended: C.** The app is how most people get the CLI, so most MCP users never see the error; A covers the rest at a cost now measured to be small.

**Answer (owner): C.** Decisions 10, 11, 13 and 15; Task 8.

### Q5. The TUI's first DuckDB connect

- **A: ask.** A dialog ("DuckDB support is a separate download of 11.7 MB for seaquel-tui 2026.x.y. Download now?"), then progress with Esc to cancel, then the connect. A failure says why and offers Retry.
- **B: download without asking**, with the progress box. One key less; a network request the user didn't ask for.
- **C: refuse and name a command** (`seaquel-tui --install-duckdb`). Nothing happens inside the TUI.

**Recommended: A.** It is the moment the user decided to use DuckDB, and the dialog states the size before anything is fetched.

**Answer (owner): A.** Decision 12; Task 7.

### Q6. The desktop app

- **A: keep the in-process DuckDB.** No change for the app; the native driver keeps its tests; the shared session code (Task 1) means a fix to one is a fix to both.
- **B: ship the helper in the app bundle and use the remote driver.** Crash isolation (a DuckDB abort or segfault today takes the whole app with it; `catch_unwind` catches only Rust panics), and the app binary loses about 30 MB that the bundle then carries as a sidecar instead: no download saved. +1.5–2 h (Tauri `externalBin` again, the signing and the macOS bundle layout).
- **C: download the helper on demand in the app too.** The app and every update shrink by about 11–12 MB compressed; the first DuckDB connect needs a network, which the app doesn't need for anything else today. +2.5–3.5 h (B plus the GUI's dialog, i18n in every locale, the offline story).

**Recommended: A now**, with B or C as a follow-up once the helper has shipped in the terminal binaries for a release. The spikes show no speed reason to keep DuckDB in process, so the follow-up is about crash isolation and download size, not performance.

**Answer (owner): A.** Decision 15; B and C stay in the Follow-ups.

### Q7. How the download is checked

- **A: as `cli_download.rs` does**: the asset's size and the `sha256:` digest GitHub's release API reports, over HTTPS, and the file is moved into place only after both match. The agreed approach.
- **B: A plus a signed `SHA256SUMS`**, signed in `release.yml` with the updater's minisign key (already a release secret), its public key compiled into the terminal binaries. Protects against a tampered GitHub release, and lets an offline install (`--from FILE`) be checked without the API. +1–1.5 h (signing step, verifier, key rotation note).
- **C: A plus, on macOS, `SecStaticCodeCheckValidity` with the team ID requirement before the first spawn.** Platform-specific; nothing on Linux.

**Recommended: A**, with B as a follow-up that should also cover the CLI download and the `curl` instructions.

**Answer (owner): A.** Decision 10; B stays in the Follow-ups.

### Q8. The release asset's format

- **A: the bare binary**, like `seaquel-cli-<triple>`: 35 MB.
- **B: gzip**, `seaquel-duckdb-<triple>.gz`: 11.7 MB. `flate2` is already in `Cargo.lock`; the digest is the `.gz`'s, and the client decompresses while hashing the compressed bytes.
- **C: xz**: 7.1 MB, but a new decoder crate (`lzma-rs` or `xz2` with C).

**Recommended: B.** The helper is never fetched by hand, so nothing needs the bare file, and gzip saves two thirds for a crate already in the tree. (The CLI and TUI stay bare: the README tells people to `curl` them.)

**Answer (owner): B.** Decisions 10 and 14.

---

## Decisions (2026-10-03)

Settled with the answers above (every one the recommendation). Numbered from 1.

### Scope

#### 1. Scope

- In: the session split in `seaquel-engine-duckdb` and the shared IPC reader; the `remote` and `helper` features; the `seaquel-duckdb` binary and its crate class; Core's `engine-duckdb-remote`, `duckdb_helper`, `duckdb_helper_status`, `duckdb_helper_install`; `seaquel-http`'s release-asset download; the terminal binaries without DuckDB; the TUI's download dialog; `seaquel-cli duckdb install|status` and the MCP error; the app's CLI install fetching the helper (Q4 C); the release steps; parity suites against the remote driver; streaming execution for `query_stream` (Q3 A).
- Not in: the desktop app on the helper (Q6); a signed checksum file (Q7 B); OS sandboxing of a restricted helper; a Windows benchmark in CI; the native driver's decode speed (S9). All in Follow-ups.

### The protocol

#### 2. The channel (Q1 A)

The helper's stdin is its input and stdout its output; nothing else is ever written to stdout (a `println!` in the helper is a review failure, as in the MCP server). The helper's stderr is piped to the client, read in a task and discarded except for its size; the client logs only the helper's exit status and codes, since a duckdb-rs panic message can quote DuckDB's error text, which can quote SQL. The child is spawned with `kill_on_drop(true)`, its working directory and environment inherited (relative DuckDB paths resolve as before, and DuckDB's S3 and `~/.duckdb` settings come from the user's environment as they do today), and on Windows with `CREATE_NO_WINDOW`.

#### 3. Frames and encodings

- A frame is `[len u32 LE][kind u8][call u32 LE][payload]`, `len` counting the kind, call and payload. Frames above 16 MiB are a protocol error that ends the process on either side (`HELPER_PROTOCOL`).
- **Control** frames carry JSON: `hello`, `open`, `query`, `stream`, `execute`, `transaction`, `readOnly`, `explainReadOnly`, `cancel`, `credit`, `close` from the client; `helloOk`, `opened`, `executed`, `committed`, `done`, `error` from the helper. `error` carries `DbError` (code and message) or `TransactionError { index, error }`.
- **Rows** go as Arrow IPC stream messages: one `schema` frame, then `batch` frames, each one IPC message. The helper slices a batch so no message passes 8 MiB (`MAX_BATCH_FRAME`); a single row larger than that goes alone (one row per frame, up to the 16 MiB frame cap), and a row past that is `RESULT_TOO_LARGE`.
- The client decodes with the shared `ipc::Columns` (`Kind::of_field`, default `KindRules`) and applies `RowCap` itself, as both other drivers do. Bytes for `max_bytes` are counted on decoded rows (`row_bytes`), so the cap means the same as natively.
- No `Debug` of a frame payload, anywhere: control types get hand-written `Debug` (call id, kind, counts).

#### 4. Processes and sessions (Q2 A)

One helper per `Engine::open`. The helper opens DuckDB with the native `open_sessions` (so `duckdb_config`, `restricted`, `create_if_missing` and `arrow_lossless_conversion` behave as natively), then serves calls: `query`, `stream`, `execute` and `transaction` run in turn on the main session's thread, as the native driver's calls take turns; `readOnly` and `explainReadOnly` each get a clone on a thread of their own, as natively. Calls are identified by the client's call id; the helper rejects a reused live id.

#### 5. Flow control

Each streaming call has a credit window of 2 batch frames (`STREAM_CREDIT`, the native driver's `STREAM_BUFFER`): the helper sends at most that many frames ahead of the client's `credit` frames, which the client sends as it hands batches to Core. A small answer then waits behind at most two frames of other calls (S6). The client's reader task demultiplexes frames into per-call bounded channels and never blocks on one call's channel; a call whose receiver is gone is cancelled.

#### 6. Cancel and drop

- Each in-flight call on the client holds a guard; dropping it before the call ended **posts** a `cancel` frame synchronously into the writer task's unbounded queue (the browser driver's `post_cancel` rule: it is ordered before any later call's request). `query_stream`'s `CancellationToken` does the same.
- The helper maps `cancel` to that call's `blocking::Call` drop (interrupt only while that call holds its connection; the flag checked after prepare and between chunks), so late cancels can't reach the next call. A `cancel` for an unknown or finished call is ignored. The call still ends with exactly one `error` or `done` frame, which the client discards if nobody waits.
- A dropped transaction call rolls back in the helper, as natively; a dropped read-only call rolls back and drops its clone.

#### 7. Handshake and versions

The client sends `hello { protocol: 1, version }` with its own app version (`seaquel_terminal::VERSION`'s source, passed into Core); the helper answers `helloOk { protocol, version, duckdb }` or exits 3. The client refuses a helper whose `version` differs (`ENGINE_NOT_INSTALLED`, "DuckDB support for seaquel-tui 2026.x.y isn't installed"), so a stale or foreign file never runs a query. The handshake is bounded at 5 s.

#### 8. Lifecycle and failures

- The helper exits when its stdin reaches EOF or a write to stdout fails (Rust ignores SIGPIPE, so that is an `EPIPE` error), whatever it is running (S7). On Linux it also sets `PR_SET_PDEATHSIG(SIGKILL)` and checks `getppid()` right after. The client's ends are close-on-exec (std's default for piped stdio), so a `$EDITOR` the TUI starts can't keep the pipes open.
- **A helper that dies** (signal, abort, OOM kill, protocol error) fails every waiting call with `CONNECTION_CLOSED` ("The DuckDB helper stopped (signal 9). Reconnect to continue."), and every later call on that driver at once with the same. Core's `ConnectionClosed` event stays reserved (CLAUDE.md); the TUI shows the error and offers reconnect.
- `Driver::close` sends `close`, waits up to 2 s for the exit, then kills. Dropping the driver kills (`kill_on_drop`).

#### 9. Where the helper lives

- `<data_local_dir>/<APP_IDENTIFIER>/bin/duckdb/<version>/seaquel-duckdb[.exe]`, beside the app's CLI copy (`cli_download::installed_path`), with `bin/duckdb` and each version folder 0700 and the file 0700 (Windows: the user's profile ACLs). One folder per version, because a hand-downloaded TUI and the app's CLI can be different versions; a successful install removes version folders older than the two newest.
- Before each spawn the client checks, on Unix, that the file and its folders up to `bin/` belong to the user and aren't group- or world-writable (`symlink_metadata`, no symlinks), else `ENGINE_NOT_INSTALLED` with "the DuckDB helper's folder has unsafe permissions". It doesn't re-hash on each spawn: anyone who can write that folder can already replace the TUI.
- Debug builds only: `SEAQUEL_TUI_TEST_DUCKDB_HELPER` / `SEAQUEL_CLI_TEST_DUCKDB_HELPER` (through `TestHooks`) point at a built helper, for tests.

#### 10. Download and install (Q4 C, Q7 A, Q8 B)

- `seaquel_http::release_asset::fetch(name, version, dest, progress) -> Result<Installed, InstallError>`: the release metadata from `api.github.com/repos/webstonehq/seaquel/releases/tags/v<version>`, the asset by exact name, its size (≤ 64 MiB compressed) and `sha256:` digest; then the download, streamed through a SHA-256 hasher and a gzip decoder into a temp file in the version folder; size and digest checked; 0700; fsync; rename. Progress is `(bytes, total)` about every 64 KiB. The User-Agent is `Seaquel/<version>`, nothing else identifying. Proxies and extra roots come from `seaquel-http`'s client (`HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS`). Cancel by dropping the future (the temp file goes).
- Core exposes it as `Core::duckdb_helper_install(progress)` behind `duckdb-helper-install` (needs `seaquel-http`); `Core::duckdb_helper_status()` (`Installed { path } | Missing | Outdated`) needs no HTTP.
- **Before the connect, not inside it:** `RemoteEngine::open` fails at once with `ENGINE_NOT_INSTALLED` when the file is missing or wrong; the interface downloads, then connects again. The 30 s `CONNECT_TIMEOUT` never covers a download.
- Offline: `seaquel-cli duckdb install --from FILE --sha256 HEX` installs a file the user copied over, checked against the hash they give (the release page shows it).
- The app's `cli_download.rs` gets the helper asset in the same "Install Command Line Tool…" flow, into the same folder layout (Q4 C). Moving `cli_download.rs` itself onto `release_asset` is a follow-up.

### Crates

#### 11. Features, crates and rules

- `seaquel-engine-duckdb` features: `native` (unchanged: duckdb-rs, the in-process driver), `remote` (the client: `tokio` with `process`, `io-util`, `sync`, `rt`; `arrow-ipc`, `arrow-buffer`; `serde`; **no `duckdb`**), `helper` (`native` + the serve loop + `arrow-ipc`'s writer), `browser` (unchanged). Modules: `session.rs` (Task 1's split), `ipc.rs` (moved from `browser/`), `wire.rs` (frames and control types, shared by `remote` and `helper`), `remote/` (the driver), `helper.rs` (the loop). A build with none of `native`, `remote`, `browser` still fails to compile.
- `crates/seaquel-duckdb`, the bin crate (`seaquel-duckdb`, `src/main.rs` only): `seaquel-engine-duckdb` with `helper`, `log` to nowhere, `--version` printing the app version (its `build.rs` reads `src-tauri/Cargo.toml` as `seaquel-terminal`'s does) and exiting. Started with a TTY on stdin it prints "seaquel-duckdb is started by Seaquel; it isn't run by hand" to stderr and exits 2.
- `seaquel-core`: its `seaquel-engine-duckdb` dependency takes `default-features = false`; `engine-duckdb = ["dep:seaquel-engine-duckdb", "seaquel-engine-duckdb/native"]` (still a default feature, so the app and the server's rule don't change), `engine-duckdb-remote = ["dep:seaquel-engine-duckdb", "seaquel-engine-duckdb/remote"]`, `duckdb-helper-install = ["engine-duckdb-remote", "dep:seaquel-http"]`. `CoreBuilder::duckdb_helper(DuckdbHelper { dir, version })` registers the remote engine under the id `duckdb`. Combining it with `browser` is a `compile_error!`.
- `seaquel-terminal`: `seaquel-core` with `default-features = false` and `engine-postgres`, `engine-mysql`, `engine-sqlite`, `engine-mssql`, `engine-duckdb-remote`, `duckdb-helper-install`; `core_builder` uses `with_plugins(|id| id != "duckdb")` then `.duckdb_helper(…)`, so a test build that unified the native driver in still registers the remote one, once. `seaquel-tui`, `seaquel-cli` and `seaquel-mcp` take `default-features = false` on their own Core dependencies.
- `check-crate-deps.mjs`: a new class, `ENGINE_HOSTS = {"seaquel-duckdb"}`, which may depend on `seaquel-engine-duckdb` and the pure crates only, and nothing may depend on. Its own test gains the case.
- CI's "Terminal binaries' dependencies" step bans `duckdb` and `libduckdb-sys` (substring `duckdb` minus `seaquel-engine-duckdb`) from both binaries, and (Q4 A) drops the `reqwest` ban on the CLI. A new line checks `cargo tree -p seaquel-engine-duckdb --no-default-features --features remote -e normal` has no `libduckdb-sys`. Clippy lines for `-p seaquel-duckdb` and for the engine crate with `remote` alone.

### Interfaces

#### 12. The TUI (Q5 A)

- `ENGINE_NOT_INSTALLED` from a connect opens `Modal::InstallDuckdb { size }` (the size from `duckdb_helper_status`, which reads the release metadata only when the dialog opens; without a network the dialog says so and offers Retry). Enter downloads: a progress box with bytes and a bar, Esc cancels (the future is dropped). Success connects again with the pending connect's typed secrets; failure shows the reason worded (no network, proxy refused, release not found for this version, digest mismatch, disk) with Retry and Cancel.
- `state/` stays pure: the download is an `Effect::InstallDuckdb`, its progress `Msg::InstallProgress`, its end `Msg::Installed(Result)`. Text in `state/text.rs`, keys in the keymap table.
- A crashed helper (`CONNECTION_CLOSED`) in any panel shows the problem dialog with Reconnect.

#### 13. The CLI and the MCP server (Q4 A)

- `seaquel-cli duckdb install [--from FILE --sha256 HEX]` prints progress to stderr (a line per 10%), the installed path to stdout, and exits non-zero with the reason; `seaquel-cli duckdb status` prints `installed <path>`, `missing` or `outdated`.
- The MCP server's tool error for a DuckDB connection without the helper is `ENGINE_NOT_INSTALLED: DuckDB support isn't installed for seaquel-cli <version>. Run "seaquel-cli duckdb install", or use Install Command Line Tool in the Seaquel app.` At startup, when an exposed connection is DuckDB and the helper is missing, one stderr line says the same; the server still starts.
- `restricted` reaches the helper's `open` unchanged; MCP's restricted tests run against it (Task 5).

#### 14. Release

`build-cli.mjs` takes `--bin seaquel-duckdb` (package `seaquel-duckdb`, the `terminal-release` profile, copied to `src-tauri/binaries/seaquel-duckdb-<triple>[.exe]`, then gzipped to `.gz` with `--gzip`). `release.yml`, per target: build, sign (codesign with the hardened runtime and no entitlements; Windows trusted-signing) the bare binary, gzip it, upload the artifact; `publish-cli` downloads `seaquel-duckdb-*` too. The per-target DuckDB compile moves from the CLI/TUI build to the helper's, so a release job compiles DuckDB once per target, as today (the two binaries shared one compile).

#### 15. The desktop app (Q6 A)

Unchanged: Core with `engine-duckdb` (native). Its `cli_install` flow fetches the helper for the CLI (Decision 10). No GUI changes, no new strings except the install flow's error text (in every locale, via the i18n agent).

#### 16. Logs

The client logs `activity=duckdb.helper` with `event` (`spawn`, `ready`, `exit`, `crash`, `install`), the exit status, durations, byte counts and codes. Never SQL, values, paths, frame payloads or the helper's stderr. The helper logs nothing.

---

## Ground rules

From phase 7a, unchanged:
- **No git writes, by anyone executing this plan.** Read-only `status`, `diff`, `log` and `show` only; no worktrees. Undo by editing back.
- **No subagents spawned by implementers.**
- The crate rules (`npm run crates:check`; `seaquel-duckdb` classified).
- Parallel tasks own their files and make small, re-read edits to shared ones (`Cargo.toml`, `check-crate-deps.mjs`, `ci.yml`, CLAUDE.md).
- **Tests never touch the real keychain, data dir, `~/.ssh` or home**; every test that installs sets `SEAQUEL_DATA_DIR` or passes a temp folder, and DuckDB tests keep `restricted.rs`'s temp `HOME`.
- **No secrets, SQL, values, paths or names in `Debug`, errors, logs or events.**
- **No real network in tests.** The download tests serve the release metadata and the asset from a local `TcpListener` (the AI tests' `LoopbackOnly` pattern); the one real download is a probe item.
- One shared `CARGO_TARGET_DIR` for the phase (`…/scratchpad/duckdb-helper/target`), npm and cargo through `mise exec --`.
- TDD: each task's tests are written first and seen failing.
- Effort log: `docs/plans/2026-10-10-duckdb-helper-effort.md`.

Added here:
- **The helper never writes to stdout except frames.** No `println!`, no logger to stdout.
- **One behaviour, two transports.** A DuckDB suite that passes natively passes remotely, or the difference is listed in `crates/seaquel-engine-duckdb/tests/REMOTE.md` with its reason and a Decision; the default is that there is none.
- **Release builds of the terminal binaries contain no DuckDB.** CI checks the dependency tree; Task 9 also checks the built binaries' size.

### Things a task could quietly skip

- the helper reading `stmt` through duckdb-rs's Arrow iterator (it panics on an interrupt: 30 panics in S4's runs) instead of `step()`;
- a cancel sent from an `async` path instead of posted from `Drop` (it can arrive after the next call's request);
- the credit window missing on one streaming path (read-only, `query`), so a large answer queues in front of others;
- a frame cap enforced on one side only;
- the client's stderr reader logging the helper's text;
- `kill_on_drop` missing, or the helper not exiting on EOF while DuckDB runs (its reader is a thread of its own);
- `ENGINE_NOT_INSTALLED` raised after a spawn attempt that took the 30 s connect timeout;
- the version check skipped when the folder name matches;
- the install moving the file before checking size and digest, or leaving a temp file on cancel;
- permissions: the folder created 0755 by `create_dir_all` and never tightened;
- `seaquel-terminal`'s builder registering `duckdb` twice in a workspace test build (it panics: `EngineRegistry::register`);
- a release step that builds or signs `seaquel-duckdb` for some targets but not all, or an upload pattern that misses the `.gz`;
- Windows: `.exe` in every name, `CREATE_NO_WINDOW`, the helper's EOF exit tested on Windows in CI (`cargo test -p seaquel-engine-duckdb --features remote,helper` on the Windows runner).

---

## Order and estimates

Sized from the phase 7a effort log (first passes at 0.8–1.1× their estimates where the spikes had settled the mechanics; review fixes 43% of first passes) and phase 6's (first passes at about half, review fixes 64%). Here the spikes settled the channel, the encoding, the decode parity and the lifecycle, and the parity suites already exist; the new code is plumbing with many failure paths (cancel, crash, version, permissions, download), which is where reviews find things. First passes are sized near their estimates and review fixes at ~50%.

| # | Task | First pass | Nearest logged task | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | Engine crate: `session.rs` split (Arrow sink vs `Value` sink), `ipc.rs` moved, streaming execution for `query_stream` (Q3 A), `wire.rs` | 1.6–2.3 h | phase 8's driver split (the shared decoder), 7a T1 (1.0 h) | — | — |
| 2 | The helper: `helper.rs` serve loop (calls, turns, clones, cancel, credits, frame slicing, handshake, EOF, pdeathsig); `crates/seaquel-duckdb`; crate class; `build-cli.mjs --bin seaquel-duckdb` | 1.5–2.1 h | 7a T2 (1.5 h, a new bin crate and its wiring) | 1 | 3 |
| 3 | The remote driver: spawn, handshake, reader and writer tasks, call guards, every `Driver` method, `ENGINE_NOT_INSTALLED`, crash handling, permissions check, Core's `engine-duckdb-remote` and `duckdb_helper` | 2.0–2.8 h | phase 8 T3–T4 (the browser driver) | 1 | 2 |
| | **Checkpoint H-1** | | | | |
| 4 | Install: `seaquel-http::release_asset`, Core's `duckdb_helper_status`/`install`, `--from`, pruning | 1.0–1.4 h | phase 6 T2 (0.5 h, `seaquel-http`), `cli_download.rs` | 3 | 5 |
| 5 | Parity: the engine suites and MCP's DuckDB tests against the remote driver, a Core-level remote test, Windows CI line, `REMOTE.md` | 1.0–1.5 h | 7a's suite moves | 2, 3 | 4 |
| 6 | Terminal binaries: features off, `seaquel-terminal`'s builder, CI dependency steps, sizes | 0.5–0.8 h | 7a T2's crate-rule work | 3 | 7 |
| 7 | The TUI's install dialog and the crash path | 0.8–1.2 h | 7a T3's dialogs (2.0 h for many more) | 4, 6 | 8 |
| 8 | The CLI (`duckdb install|status`, MCP error and startup line); the app's CLI install fetching the helper (Q4 C); `release.yml` | 0.9–1.3 h | 7a T2's release steps, phase 4's CLI work | 4, 6 | 7 |
| 9 | Probe | 1.2–1.8 h | 7a T8 (1.9 h), phase 6 T9 (1.65 h) | 5, 7, 8 | — |
| 10 | Docs, measurements, Checkpoint H-2 | 0.6–0.9 h | 7a T9 (0.7 h) | all | — |
| | **First passes** | **11.1–16.1 h** | | | |
| | Review fixes (~50% of Tasks 1–8) | 4.7–6.7 h | 7a: 43%; phase 6: 64% | | |
| | Probe fixes | 1.2–2.0 h | 7a: 3.4 h (more surface); phase 6: 3.5 h | | |
| | **Total** | **~17.0–24.8 h** | | | |

**Expect about 19.5 h:** H-1 (Tasks 1–3 and their reviews) about 9 h, H-2 about 10.5 h. If first passes run at 0.8× as in 7a's Tasks 1–2, about 17 h; past 25 h if the session split (Task 1) disturbs the native driver's suites or the Windows runner finds a pipe problem.

Option costs of the answers not taken, kept for later changes: Q1 B +1–1.5 h, C +3–4 h; Q2 B +1 h (shared-process lifecycle); Q3 B 0 h but a `REMOTE.md` difference, C −0.3 h; Q4 A only −0.5 h, B −1 h (and the CI rule stays); Q5 B −0.2 h, C −0.4 h; Q6 B +1.5–2 h, C +2.5–3.5 h; Q7 B +1–1.5 h, C +0.5 h; Q8 A −0.2 h, C +0.3 h.

The riskiest parts:
- **Task 1's split** touches the driver the desktop app uses; the native suites are the net. Streaming execution changes when a failing query fails.
- **Task 3's cancel ordering**: a cancel overtaken by the next call's request would interrupt the wrong statement; the helper's per-call `Call` guard is the backstop, the posted cancel the rule.
- **Windows**: nothing in this plan has run there; the CI line in Task 5 is the first time.
- **The release steps** can't run before a tag.

Cut if time runs short, in order: pruning old versions (keep every version); `--from` offline install (the README's manual copy instead); the startup stderr line in the MCP server. Q4 C's app change can move to a follow-up without touching anything else.

---

## Task 1: The session split, the shared IPC reader, streaming execution

**Files:**
- `crates/seaquel-engine-duckdb/src/session.rs` (new): what `driver.rs` holds today minus the `Value` decoding: `open`, `open_sessions`, `restricted_config`, `user_config`, binds, `prepare`, `transaction_blocking`, the read-only and explain-read-only scaffolding (BEGIN READ ONLY, the wrapper, ROLLBACK, refusal mapping), all generic over a `ChunkSink` (`fn columns(&mut self, &Statement)`, `fn chunk(&mut self, StructArray) -> Result<Flow, DbError>`, `fn finish`);
- `driver.rs`: the native driver as `session` + a `ValueSink` (today's `ResultReader`, logical-type kinds, `RowCap`); behaviour unchanged except Q3;
- `ipc.rs` (moved from `browser/ipc.rs`; `browser/` imports it), built under `browser`, `remote` or test;
- `wire.rs` (new): frame read/write (sync and tokio), the control types with serde and hand-written `Debug`, `MAX_FRAME`, `MAX_BATCH_FRAME`, `STREAM_CREDIT`, `PROTOCOL`;
- `Cargo.toml`: the `remote` and `helper` features (empty bodies until Tasks 2–3), arrow-ipc's writer under `helper`.

**Tests first:**
- every existing test in the crate passes unchanged (`smoke`, `live`, `values`, `transaction`, `read_only*`, `restricted*`, `duckdb_config`, `cells_fixture`, the parity tests, the IPC tests from their new path);
- `session`: an `ArrowSink` test sink receives the same chunks the `ValueSink` decodes, for every typed-cell case (decoding the sink's chunks with `Kind::of_field` equals the native rows: `decode_from_ipc_matches_decode_from_duckdb`'s comparison, driven through the session);
- streaming (Q3 A): `query_stream`'s first batch arrives before a 50M-row query finishes (a time bound with slack, or a row count at first batch less than the total); a query failing after its first chunk yields batches, then the error; cancel after the first batch interrupts (the stream ends within 1 s on a 3e9-row query); the iterator is never used (a test that interrupts mid-stream sees an error, not a panic);
- `wire`: round trips of every control type; a frame over `MAX_FRAME` is refused on read and on write; `Debug` of a `query` with a marker in SQL and params doesn't show the marker.

**Run:** `cargo test -p seaquel-engine-duckdb` (default features), `--no-default-features --features browser` natively (the IPC tests), the wasm32 clippy line for `browser`; `cargo test -p seaquel-core` (the desktop's paths through the native driver); clippy.

**Review:** `driver.rs` is only the sink and the trait impl; the panicking iterator isn't called; `read_cell`'s `catch_unwind` stays in the `ValueSink`.

**Things this task could quietly skip:** the cancelled check after `prepare` in the streaming path (`execute_streaming` is another place DuckDB clears its interrupt flag); the empty-result columns (`StreamBatch.columns` on the final batch when no chunk came); `STREAM_BUFFER` semantics with streaming execution (still 2 in flight).

## Task 2: The helper and `seaquel-duckdb`

**Files:**
- `crates/seaquel-engine-duckdb/src/helper.rs` (new, `helper` feature): `serve(input: impl Read, output: impl Write) -> ExitCode`: a reader thread (frames in, EOF or a protocol error ends the process), a writer behind a mutex (whole frames, flushed), the main session's thread with a job queue, one thread per read-only call, the per-call `Call` registry for `cancel`, credits per streaming call, batch slicing to `MAX_BATCH_FRAME`, `hello`/`open`/`close`;
- `crates/seaquel-duckdb/` (new): `Cargo.toml`, `build.rs` (the version), `src/main.rs` (TTY refusal, `--version`, `PR_SET_PDEATHSIG` on Linux, `serve`);
- root `Cargo.toml` (member); `scripts/check-crate-deps.mjs` and its test (`ENGINE_HOSTS`); `scripts/build-cli.mjs` and its test (`--bin seaquel-duckdb`, `--gzip`); `package.json` (`duckdb-helper:build`).

**Tests first** (in-process: `serve` over `os_pipe`/`std::io::pipe` pairs, so no binary is needed):
- handshake: wrong protocol exits 3 with nothing on stdout but the refusal frame; `version` echoed;
- `open` with `restricted` refuses a non-allowed `duckdb_config` key (`INVALID_CONNECTION`), as natively;
- each call kind against an in-memory database, answers equal to the native driver's for the same SQL (decoded through `ipc::Columns`);
- two calls interleaved: a read-only call answers while a main-session stream is paused on credit;
- credit: with no `credit` frames the helper sends exactly 2 batch frames and waits; a `cancel` then ends the call with one `error`;
- cancel ordering: `cancel(1)` followed at once by `query(2)`: call 2 runs to completion (the interrupt can't reach it);
- frame slicing: a 2,048-row chunk of 64 KiB cells arrives as frames ≤ 8 MiB; a single 12 MiB cell arrives alone; a 20 MiB cell is `RESULT_TOO_LARGE`;
- EOF on input while a 3e9-row query runs: `serve` returns within 1 s;
- the binary (`tests/bin.rs` in `seaquel-duckdb`, via `CARGO_BIN_EXE_seaquel-duckdb`): `--version` prints the app version; started with a TTY it refuses (Unix, under `script`); writes nothing to stdout before `hello`;
- crate rules: `seaquel-duckdb` depending on `seaquel-core` fails `crates:check`.

**Run:** `cargo test -p seaquel-engine-duckdb --features helper -p seaquel-duckdb`; `npm run crates:check`; the build script's test; `node scripts/build-cli.mjs --release --bin seaquel-duckdb`, size into the effort log (expect ~35 MB, S8).

**Review:** no `println!`/`print!` in the helper or its crate; every thread's panic becomes that call's `error` (the main session's thread survives it, as the native worker does).

## Task 3: The remote driver

**Files:**
- `crates/seaquel-engine-duckdb/src/remote/` (new, `remote` feature): `mod.rs` (`remote_engine(locator) -> Arc<dyn Engine>`, `RemoteEngine`, `HelperLocator { dir, version }`), `process.rs` (permissions check, spawn with `kill_on_drop`, stderr drain, handshake with timeout, exit status), `conn.rs` (writer task with an unbounded queue, reader task demultiplexing, the dead flag), `driver.rs` (`RemoteDriver`: every `Driver` method, introspection through `introspect::calls`, `explain_read_only`'s one-statement check before sending, `RowCap` on the client);
- `crates/seaquel-types` (or `seaquel-engine`): `DbError::engine_not_installed(engine, version, reason)`, code `ENGINE_NOT_INSTALLED`;
- `crates/seaquel-core/Cargo.toml` and `lib.rs`: the features of Decision 11, `CoreBuilder::duckdb_helper`, the `browser` `compile_error!`;
- `crates/seaquel-core/tests/duckdb_remote.rs` (new).

**Tests first** (`SEAQUEL_TEST_DUCKDB_HELPER` names a built helper; missing it the tests skip, and `SEAQUEL_TEST_REQUIRE_ENGINES=1` fails them):
- a missing file, a file of the wrong version (a fake helper script answering another version), a folder with mode 0777, a symlink: `ENGINE_NOT_INSTALLED`, each within 100 ms, nothing spawned for the permission cases;
- a fake helper that never answers `hello`: `ENGINE_NOT_INSTALLED` after the 5 s bound, the process killed;
- drop the driver: the helper process is gone within 1 s; `close`: it exits on its own;
- kill the helper mid-stream: the stream ends with `CONNECTION_CLOSED` naming the signal; the next `query` fails the same at once;
- drop a `query_stream` after its first batch: the helper's query stops (a following call on the main session starts within 100 ms);
- a dropped `transaction` rolls back (the table unchanged afterwards);
- Core: `with_plugins(|id| id != "duckdb").duckdb_helper(…)` connects a `duckdb` target through `Workspace::connect`; `run`/`page`/`table_page`/`apply_changes` against it (one case each, the native cases' SQL).

**Run:** `cargo build -p seaquel-duckdb`, then `SEAQUEL_TEST_DUCKDB_HELPER=… cargo test -p seaquel-engine-duckdb --no-default-features --features remote` and `-p seaquel-core --test duckdb_remote`; clippy for the engine crate with `remote` only (and `cargo tree` showing no `libduckdb-sys`).

**Review:** every guard posts its cancel without awaiting; the reader task never awaits a full per-call channel; no frame payload in a log or error.

### Checkpoint H-1

Every DuckDB test in the engine crate passes against the remote driver (Task 5's switch can be applied by hand here); the spike's numbers reproduced within 20% through the real driver (big3 streamed through Core's `query_stream`, `SELECT 1` through `query`), recorded in the effort log; `cargo tree -p seaquel-engine-duckdb --no-default-features --features remote` has no `libduckdb-sys`.

## Task 4: Install

**Files:** `crates/seaquel-http/src/release_asset.rs` (new); `crates/seaquel-core/src/duckdb_helper.rs` (new: `duckdb_helper_status`, `duckdb_helper_install`, `install_from_file`, pruning); Core's `Cargo.toml` (`duckdb-helper-install`).

**Tests first** (a local `TcpListener` serving the metadata and the asset; `RELEASES_BASE` injectable in tests only):
- a good asset installs: the file decompressed, 0700, folders 0700, `status` says installed;
- digest mismatch, size mismatch (short and long), a body larger than the metadata says, a missing digest, a malformed digest, a 404 release, a missing asset name: each refused with its own error, nothing at the target path, no temp file left;
- the download dropped half-way: no file, no temp file;
- progress is reported, monotonic, ending at the total;
- `--from` with the right and wrong `sha256`;
- pruning keeps the two newest versions and removes older ones, and never the running version.

**Run:** `cargo test -p seaquel-http -p seaquel-core --features duckdb-helper-install`.

## Task 5: Parity

**Files:** `crates/seaquel-engine-duckdb/tests/common/engine.rs` (new: `engine()` picks native or remote from `SEAQUEL_TEST_DUCKDB_DRIVER`, remote from `SEAQUEL_TEST_DUCKDB_HELPER`); the eleven test files that call `seaquel_engine_duckdb::engine()` (`smoke`, `live`, `values`, `transaction`, `read_only`, `read_only_max_rows`, `read_only_explain`, `restricted`, `restricted_home`, `duckdb_config`, `dialect_parity`, …) switch to it; `crates/seaquel-mcp/tests/tools.rs`'s DuckDB cases run once per driver; `tests/REMOTE.md`; `ci.yml`: a `duckdb-remote` job (build the helper, run the engine crate's tests and MCP's DuckDB tests with the remote switch, `SEAQUEL_TEST_REQUIRE_ENGINES=1`), on Linux, macOS and Windows runners.

**Tests first:** the switch itself (a test asserting which driver ran, so a misconfigured CI job can't pass by testing native twice); the restricted suite's file escapes against the helper (the helper inherits `HOME`; the temp `HOME` rule still holds).

**Run:** both drivers, all DuckDB targets; MCP's suite.

**Review:** no test skipped for the remote driver without a `REMOTE.md` line and a Decision.

## Task 6: The terminal binaries without DuckDB

**Files:** `crates/seaquel-terminal/{Cargo.toml,src/core.rs}` (Decision 11; the version passed to the locator; the data-local dir from `seaquel_core`); `seaquel-tui`, `seaquel-cli`, `seaquel-mcp` `Cargo.toml`s; `ci.yml`'s dependency step; `TestHooks` (`_DUCKDB_HELPER`).

**Tests first:** `core.rs`: the ids are the five, `duckdb` once, in a workspace build with the native driver unified in; a connect to DuckDB with no helper is `ENGINE_NOT_INSTALLED` in under 100 ms; the test hook only in debug builds.

**Run:** `cargo test -p seaquel-terminal -p seaquel-tui -p seaquel-cli -p seaquel-mcp`; the dependency step locally; `node scripts/build-cli.mjs --release` and `--bin seaquel-tui`: sizes into the effort log (expect ~14.5 and ~17.8 MB).

## Task 7: The TUI's install dialog

**Files:** `crates/seaquel-tui/src/state/{connect.rs, dialogs.rs, text.rs, keymap.rs}`, `src/state/install.rs` (new), `src/runtime/effects.rs`, `src/view/` (the dialog and progress box), `tests/snapshots/`.

**Tests first:** `update` tests: `ENGINE_NOT_INSTALLED` from a connect opens the dialog with the pending connect kept; Enter starts `Effect::InstallDuckdb`; progress messages move the bar; Esc cancels and returns to the picker; success re-issues the connect with the same typed secrets; each failure kind shows its text with Retry; `CONNECTION_CLOSED` on a connected DuckDB shows Reconnect. Render snapshots of the dialog, progress and failure at 148×42 and 80×24. A runtime test with the local test server installing into a temp data dir.

**Run:** `cargo test -p seaquel-tui`.

## Task 8: The CLI, the app's install, the release

**Files:** `crates/seaquel-cli/src/{lib.rs, duckdb.rs}` (new subcommand); `crates/seaquel-mcp` (the error text, the startup line); `src-tauri/src/{cli_download.rs, cli_install.rs}` (the helper asset in the install flow, the new folder layout) and its locale strings; `.github/workflows/release.yml` (build, sign, gzip, upload per target; `publish-cli`'s pattern).

**Tests first:** `duckdb status` on a missing, an installed and an outdated helper; `install` against the local server, stdout carrying only the path; MCP: a DuckDB tool call with no helper returns `isError` with the text; `tests/stdio.rs` still sees only JSON-RPC on stdout with the startup line present; `cli_download`'s asset name test gains `seaquel-duckdb-<triple>.gz`.

**Run:** `cargo test -p seaquel-cli -p seaquel-mcp -p seaquel`; `release.yml` is read against the CLI's and TUI's steps line by line (targets, identity, no entitlements file, `.exe`, `.gz`, the upload pattern).

## Task 9: Probe

- **The real thing:** the release-profile TUI and CLI and a release-profile helper, on macOS arm64 and Linux (x86_64 container), installed by the TUI's dialog from a local server serving a real built asset; a DuckDB file of 1 GB: browse, page, filter, stage and commit, run, explain, `:all` on 10M rows (the 100,000-row cap and the cancel), extensions tab equivalent (`INSTALL`/`LOAD` through a run).
- **Failure paths:** kill the helper during a run, a page and a commit; kill the TUI (`kill -9`) during a stream and check no helper remains; Ctrl+Z with a helper running; `$EDITOR` open while the TUI is killed (the helper must still exit); a full disk during install (a small tmpfs); a proxy that refuses (`HTTPS_PROXY` to a closed port).
- **MCP:** the CLI's server with a restricted DuckDB connection through the helper: the file-escape cases of `restricted.rs` by hand through tool calls; a missing helper's tool error.
- **Measurements:** the spike's table rerun through Core (big3, mixed20, widetext, `SELECT 1`, cancel), helper RSS, spawn time, and the one real download from a GitHub release (a draft or the latest tag) with its time.
- **Logs:** `tui.log` at trace with markers in SQL, a cell, a file path and a connection name: none present.
- Windows: if a Windows machine is available, the install and one query (otherwise a manual check).

Findings go into this plan with fix tasks.

## Task 10: Docs, measurements, Checkpoint H-2

- **CLAUDE.md:** `seaquel-engine-duckdb`'s three drivers and four features; `seaquel-duckdb` and its crate class; Core's new features and calls; the terminal binaries' "Terminal binaries" paragraph (sizes, the helper, where it installs); the CI steps; `ENGINE_NOT_INSTALLED`.
- **README:** "DuckDB in the terminal": the first-connect download, `seaquel-cli duckdb install`, offline `--from`, where it lives, how to remove it.
- **The design doc:** "Why compile-time plugins" and "Plugin kinds" get an "As built" note (one first-party engine out of process, same traits); "Terminal binaries" gets the new sizes; a cost section from the effort log.
- **Checkpoint H-2:** the workspace with live engines; both DuckDB drivers' suites; every clippy line (wasm32 too); `crates:check`; both dependency steps; the release-profile binaries' sizes and the helper's; `npm run check`, vitest (unchanged, but the browser module rebuilds with the moved IPC reader).

---

## Manual checks (the owner's)

What a pty and CI can't show:
- From the first tagged draft release: the TUI's first DuckDB connect downloads the helper on macOS (no Gatekeeper prompt: the file has no quarantine attribute) and on Linux; `codesign -dv` on the installed helper shows the Developer ID and the hardened runtime.
- The app's "Install Command Line Tool…" puts the CLI and the helper in place; `seaquel-cli duckdb status` agrees; Claude Desktop's MCP server opens a DuckDB connection.
- Windows: the TUI downloads and runs the helper; Task Manager shows one `seaquel-duckdb.exe` per DuckDB connection and none after quitting; a `SELECT * FROM range(10000000)` streams (note the time against the macOS number, for the Windows assumption in "Spikes").
- Behind a corporate proxy with its own CA (`HTTPS_PROXY`, `NODE_EXTRA_CA_CERTS`), if one is at hand.

---

## Follow-ups (not in this plan)

- **The desktop app on the helper** (Q6 B or C), for crash isolation and, with C, about 11–12 MB less per download and update.
- **A signed `SHA256SUMS`** (Q7 B) for the CLI, TUI and helper downloads and the README's `curl` steps.
- **`cli_download.rs` onto `seaquel-http::release_asset`**, one download path for the app and the terminal binaries.
- **OS sandboxing of a restricted helper** (Linux landlock and seccomp, macOS `sandbox_init`): the MCP server's DuckDB could then be denied the file system outside its database at the OS level, not only by DuckDB's settings. Possible only now that DuckDB has its own process.
- **The native driver's decode speed** (S9: 958 against 773 ms on macOS, 2,212 against 601 ms on Linux for the same rows): per-cell `catch_unwind`, `Vec` per row and the logical-type walk are the suspects.
- **A Windows channel benchmark in CI** (the spike's harness on the Windows runner), to replace the assumption in "Spikes".
- **The helper for SQLite?** No: SQLite is about 1.5 MB and the engine everybody uses; noted only because someone will ask.
- **The TUI's connection picker marking engines a build lacks** (7a's follow-up) is no longer needed: every terminal build registers `duckdb`.
