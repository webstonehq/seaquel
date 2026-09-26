# seaquel-storage migrations

Numbered schema and data migrations for the metadata database, run by `sqlx::migrate!` every time `Storage::open` opens a file, right after the baseline (`src/schema.rs`) and before the data steps (below).

- **The baseline is frozen.** It brings every file any release ever wrote up to the schema of 2026.9.x. Every schema change after that is a new file here, never another step in `schema.rs`.
- **Names:** `NNNN_description.sql`, numbered from `0001` with no gaps. sqlx reads the number before the first `_` as the version and runs files in version order.
- **Never edit a migration that has shipped.** sqlx stores each file's checksum in `_sqlx_migrations` and refuses to open a file whose recorded checksum doesn't match. Fix a mistake with a new migration.
- **Each file runs in its own transaction.** It is recorded as applied only if it succeeds. A file that starts with `-- no-transaction` runs outside one; don't use that without a reason written in the file.
- **A file must work on every database the baseline can produce**, including files that started on `v2026.4.5-beta.1`: there, `saved_queries.project_id` and `dashboards.project_id` are nullable, last in column order and have no foreign key (see `tests/fixtures/README.md`). Name columns explicitly; never rely on column order.
- **Migrations are expand-only.** Going back to an older release has to keep working: releases 2026.4.5 through 2026.9.x already open files a newer build changed, and the migrator ignores applied versions it doesn't know (`set_ignore_missing(true)`). So a migration may add tables, columns, indexes and rows, and must not:
  - drop or rename a table, column or index;
  - add a `NOT NULL` column without a default;
  - change what an existing column means or how its values are encoded.
- **A breaking change needs an explicit gate.** If a change can't be expand-only, it also writes a new `schema_version` value, and the release that ships it teaches `Storage::open` to refuse a file whose highest version is above what it knows, with its own error code, before touching it. Builds older than that gate can't refuse, which is why the rule above comes first.
- **Two processes opening at once.** If two processes open the same file at the moment a new migration is pending, both may try to apply it; one wins, and the other's open fails with `STORAGE_ERROR` and leaves the file as the winner wrote it. Opening again succeeds. Until `seaquel mcp` exists no second process writes the file; phase 4 serialises opens.
- **Two pools in one process race the same way.** sqlx's migrate lock is a no-op on SQLite, so nothing stops two pools on one file from applying a pending migration at once. The web server can open a second pool on a user's file while an evicted one is still finishing a request (`crates/seaquel-server/src/workspaces.rs`). If a SQL migration is pending at that moment, the loser's open fails, that request gets one 500 (`STORAGE_ERROR`), and the next request opens cleanly. Data steps don't race: they run under `BEGIN IMMEDIATE`. Phase 4 serialises opens for both cases.
- **Tests:** `tests/baseline.rs` opens every frozen release schema. Add a case for a migration that moves data.

None yet.

## Data steps

A data step is a one-off rewrite of stored rows done in Rust. `Storage::open` runs the pending ones right after the numbered migrations (`src/data_steps.rs`), all in one `BEGIN IMMEDIATE` transaction, and records each by name in `_seaquel_data_steps(name TEXT PRIMARY KEY, applied_at TEXT)`, so each runs once per file. The step creates that table the first time it runs; the baseline doesn't, and schema comparisons skip it, as they skip `_sqlx_migrations`. A file that has had every step only gets a read at open.

**Which one to write:**

- **A SQL migration** for schema changes (tables, columns, indexes) and for data changes plain SQL does in one set-based statement.
- **A data step** when the rewrite needs Rust logic: parsing, a function the queries already use (so the stored rows and new writes agree by construction), or anything a recursive CTE would have to imitate. SQL that walks strings character by character is quadratic in SQLite, and it can't be fixed after it ships, because sqlx checksums the file.

**Rules for data steps:**

- **Append only.** Add a step to the end of `STEPS` with a new name. Never rename, reorder or remove one that has shipped: a file records names, not code.
- **Its code can change** after it ships (a data step has no checksum), but files that already ran it won't run it again. If they need the fix too, add a new step.
- **Address rows by `rowid`**, since ids can be NULL on hand-edited files. Write only the rows that change, and leave alone text that doesn't decode as UTF-8.
- **Keep it linear** in rows and string length. `tests/repos.rs` has a timing test for the first step.
- **Expand-only, like migrations.** Older releases open the file afterwards, so a step may rewrite values within their existing meaning and must not change what a column holds.

## Steps

- **`strip_connection_string_passwords`** (phase 3 Task 4, Decision 13.1 of the phase 3 plan). Applies `strip_connection_string_password` (`src/connection_string.rs`), the function `connections::save` applies to every save from now on, to each stored `connections.connection_string`. It removes `Password` and `Pwd` pairs (any case) from key=value strings (`Server=…;Password=…`), which the TypeScript stored with their password because it only knew how to strip one from a URL. It also removes URL passwords and `password`/`pwd` query parameters. It leaves SQLite strings alone, along with strings that hold no password.
