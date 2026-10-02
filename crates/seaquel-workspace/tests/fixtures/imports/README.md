# imports fixtures

These files record what today's TypeScript makes of TablePlus's and DBeaver's connection files: the candidates the import dialogs list, before anything is saved. Phase 5e moves the readers out of `src-tauri` and the mapping out of TypeScript into Core (`seaquel_workspace::imports`, Core's `imports.rs` and the `imports` RPC group; Decision 47). The create side, saving the chosen ones, is pinned by `../library/imports.json` (5d-1, frozen) and stays as it is apart from what `changes.json` there and Decision 47 say. See `docs/plans/2026-10-05-rust-core-phase-5e-plan.md`, "Imports", bug 22, Decisions 47 and 48, and Task 4a.

**The fixtures are frozen.** Change a case only when Core is meant to behave differently, say why in `changes.json`, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-05-record-shared-fixtures.test.ts.txt`, the same file that records `../shared` (see that README for how to run it). It ran on `cfc7294` plus the phase 5e working tree after Tasks 1 and 3. Two runs gave byte-identical files. After the Task 2 review the set was recorded again: the first recording's cases came out byte-identical and in place, and the review's cases follow them.

For each case it mocks `$lib/api/tauri`'s `readTablePlusConfig` or `readDbeaverConfig` to answer the case's input, then calls the real `discoverTablePlusConnections(existing)` or `discoverDbeaverConnections(existing)`. So the real `parseTablePlusConnections`, `toTablePlusConnection`, `mapToImportable` (both), `parseDbeaverConnections`, `tablePlusTlsModeToSslMode` and `isAlreadySaved` run unmodified.

## The cases

`tableplus.json` (15 cases) and `dbeaver.json` (13), each `{name, note?, input, plist?, existing, output}`:

| field | meaning |
| --- | --- |
| `input` | TablePlus: the JSON `read_tableplus_config` hands over today (the plist decoded by the `plist` crate, then `serde_json`), `null` when there's no file, or `{"error": message}` when the command fails. DBeaver: the text of `data-sources.json`, `null` for no file, or `{"error": message}` |
| `plist` | TablePlus only: the same input as an XML plist, under `plist/`. The replay first decodes it with the `plist` crate and checks that `serde_json` gives exactly `input`, which pins the step that leaves `src-tauri`. `plist/unreadable.plist` is cut short and must fail to decode |
| `existing` | the project's saved connections (`id`, `type`, `host`, `port`, `databaseName`, `username`) the duplicate check compares with |
| `output` | today's list: each `TablePlusImportableConnection` or `ImportableConnection` with its `original` entry, `isDuplicate` and `selected`. A port of `NaN` is recorded as the string `"NaN"` |

TablePlus: every driver (PostgreSQL, MySQL, MariaDB, SQLite, SQL Server; Redis and MongoDB left out; `postgresql` in lower case left out); ports (default, custom, an integer, trailing letters, a leading space, `1e3`, `-1`, not a number, missing); each TLS mode (0, 1, 2, unmapped 3, `"2"` as text, `"on"`, on MySQL, ignored on SQL Server and SQLite); SSH with a password and with a key, a text port, an integer port, no address, off, the flag as text, on SQLite (ignored); SQLite paths (`DatabasePath`, `DatabaseName` as the fallback, neither); duplicates by each of the five fields, an empty host read as `localhost`, and two entries equal to one saved connection; the `host:port` name fallback; typed plist values (numbers and booleans taken as text, an array or dict as empty); entries without an `ID` and entries that aren't dicts; an empty list; a dict instead of a list; no file; a plist that doesn't decode; ports at and past 0–65535 (`65535`, `65536`, `70000`, `0`); two entries sharing an `ID`.

DBeaver: every provider (`postgresql`, `postgres`, `mysql`, `mariadb`, `sqlite`, `mssql`, `sqlserver`, `duckdb`, `PostgreSQL` in another case; `oracle` and `generic` left out); ports (default, custom, a number, `${port}`, `1e3`, a leading space, `-1`, empty); no `configuration`; a URL-only configuration (not read: a follow-up); SSH handlers (not read: a follow-up); duplicates by each of the five fields; no `connections` key; an empty `connections`; an empty file; text that isn't JSON; no file; a read that fails; ports at and past 65535 (`65535`, `65536`, `70000` as a number).

The paths are TablePlus's `~/Library/Application Support/com.tinyapp.TablePlus/Data/Connections.plist` (macOS only) and DBeaver's `workspace6/General/.dbeaver/data-sources.json` under `~/Library/DBeaverData` (macOS), `~/AppData/Roaming/DBeaverData` (Windows) or `~/.local/share/DBeaverData` (Linux), as `src-tauri` reads them today. The recorder never reads them; Core resolves them from an injected home (`ImportPaths { home }`).

## Replay rules

`seaquel-workspace/tests/imports_plan.rs` replays every case through `tableplus_candidates` (on the decoded `input`) or `dbeaver_candidates` (on the input's bytes) and `mark_duplicates` (with `existing`), and compares the result with the case's entry in `changes.json`, exactly. Core's `tests/imports.rs` does the same through `importsCandidates` with the file laid out under a temp home (the `plist/` file for TablePlus), plus `{found: false}` with no file.

## changes.json

Every case has an entry, because Decision 47 changes the answer's shape for all of them. The `*` entry says how:

- `{found: false}` when there's no file.
- `{found: true, unreadable: "<message>"}` when the file can't be read, doesn't decode, isn't JSON (an empty DBeaver file included) or isn't a list of TablePlus entries. `<message>` is any non-empty text naming no path.
- Otherwise `{found: true, candidates}`: today's list in today's order, with unsupported drivers still left out.

A candidate has:

- `key`: for TablePlus, `id:<ID>` (the entry's `ID` as text), or `pos:<n>` (its 0-based position in the list) for a `noId` or `duplicateId` entry; the prefixes keep the two kinds apart, so no `ID` collides with a position. For DBeaver, the connection's key, as it is.
- Today's `name`, `type`, `host`, `port`, `databaseName`, `username`, `sslMode` and `sshTunnel`.
- `duplicateOf`: the id of the saved connection with the same five fields. It replaces `isDuplicate` and `selected`.
- `problem`, when the candidate can't be imported:
  - `invalidPort`, with `port: 0`, for a port that isn't a number or is outside 0–65535 (`-1`, `65536`, `70000`; bug 22). Every other port follows `parseInt`, as today: `5432abc` is 5432, `1e3` is 1, ` 15432` is 15432.
  - TablePlus only: `noId` for an entry without an `ID` (today it's dropped), and `duplicateId` for each entry whose `ID` another entry has. Both take a `pos:<n>` key, and both come before `invalidPort`.
  - Added after the Task 4a review (coordinator's decision, Decision 47): `invalidSshPort` for a TablePlus tunnel port outside 0–65535 (`parseInt` keeps `-5` or `70000`; `NaN` and 0 still become 22), with the tunnel's `port` 0, and `noName` for a DBeaver connection whose name is missing, `null` or blank after JavaScript's trim. `connectionCreate` refuses both today. The order is `noId`/`duplicateId`, then `invalidPort`, then these two. No recorded case has either shape, so `tests/imports_plan.rs` pins them (`an_ssh_port_out_of_range_is_a_problem`, `a_dbeaver_connection_without_a_name_is_a_problem`).

`importsCreate` refuses a key whose candidate has a problem (`INVALID_ARGUMENT`, naming the key), whatever the GUI sends. Each case's `expected.output` is the answer, written out. The review corrected `tableplus/entries-without-id` (its entry without an `ID` is now listed as `noId`) and `tableplus/ports` and `dbeaver/ports` (`-1` is now `invalidPort`). The re-review gave every TablePlus key its `id:` or `pos:` prefix.

## Differences no case records

Bug fixes in `seaquel_workspace::imports` that no recorded input reaches, listed so the Core replay (Task 5) doesn't trip on them. Each is pinned in `tests/imports_plan.rs`:

- A driver or provider named like an `Object.prototype` member (`toString`, `constructor`, `__proto__`) is dropped as unsupported. Today `DRIVER_MAP[driver]` returns the inherited function and the mapper goes on with nonsense.
- A DBeaver `host`, `name`, `database` or `user` that isn't text becomes its `String()` (`12`, `true`, `a,,1` for an array, `[object Object]`), where today the raw value passed through. A missing `name` is `""`, which is now `noName`.
- DBeaver JSON whose top level is `null`, an array or a string is unreadable. Today `null` threw and the others gave an empty list.
- A DBeaver `connections` object or array that doesn't decode (a key with a lone surrogate escape) is unreadable. Today `JSON.parse` accepts it.
- Only `provider`, `name` and `configuration`'s `host`, `port`, `database` and `user` are decoded. So a number past f64's range or deep nesting anywhere else in a connection keeps it, as in JavaScript, and such a number in a field that is read is `Infinity`. A read field that can't be decoded at all (a lone surrogate) counts as absent. An entry whose own keys don't decode is left out.

## What can't be recorded

- The create side and its duplicate check inside one transaction (bug 23: two windows importing at once) are Core's tests; today's create is `../library/imports.json`.
- Real TablePlus and DBeaver files from users' machines; the Task 8 probe tries the owner's.
- TablePlus binary plists: the inputs are XML plists, which the `plist` crate decodes the same way.
