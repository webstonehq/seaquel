# Rust Core Phase 2b Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task-by-task.

**Goal:** Replace node-sql-parser and the TypeScript SQL scanners with two Rust crates. `seaquel-sql` is pure Rust and holds statement splitting, `{{param}}` substitution, the statement checks, `parse_create_table` and the sqlparser-rs AST helpers. `seaquel-wasm` exposes it to the Svelte app as one WebAssembly module, loaded before the app mounts on desktop, web and the demo. The editor, query runner, Visual tab, query builder, tutorial and table editor call it synchronously, as they call the TS today. Fix the bugs the spike and this plan's research found along the way.

**Architecture:** Phase 2b of `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md`, shaped by the spike (`docs/plans/2026-09-27-phase-2b-spike.md`).

- **`crates/seaquel-sql`** (pure, builds for wasm32). A hand scanner ported from `src/lib/engine/sql-scan.ts` is the only tokenizer. Splitting, statement at cursor, `{{param}}` handling, row-limit detection and the statement checks sit on top of it. sqlparser-rs 0.63 is used only where an AST is needed: the tutorial and query builder `ParsedQuery`, the Visual tab's AST, and column sources. Its Rust API works in UTF-8 byte offsets, like any Rust string API.
- **`crates/seaquel-wasm`** (wasm-bindgen glue). It exports thin functions that take and return strings and JSON. Every position that crosses the boundary is a UTF-16 offset. The conversion is in `crates/seaquel-wasm/src/offsets.rs`, and no byte offset or sqlparser `Location` leaves the crate.
- **`src/lib/sql/`** (new). A TypeScript module with the same function names and signatures the call sites use today, so the switch is mostly an import change. `src/lib/wasm/` holds the init code and the generated `pkg/`.
- **Init.** `wasm-bindgen --target web`, a Vite `?url` import, and `await init()` in the root `src/routes/+layout.ts` `load`. Nothing mounts before it resolves, so every call after that can be synchronous.

**Tech Stack:** Rust (sqlparser 0.63 with `default-features = false, features = ["std"]`, wasm-bindgen 0.2.128, ryu-js, ts-rs 12), wasm-bindgen-cli, wasm-opt from binaryen, TypeScript/Svelte 5, vitest. No Docker databases: 2b touches no engine crate.

**Inputs:** the spike report and its code in `spike/` (`sql-spike`, `wasm-stub`, `harness`); the design doc's "Phase 2 cost" and "Phase 2b estimate" sections; phase 1's and phase 2's ground rules and execution notes.

---

## Open questions

Each has a recommendation. The plan is written as if the recommendation is taken; if it isn't, only the named task changes.

**Answered (2026-09-27):** the user took the recommendation on 1, 3 and 4 (build on demand; parsing only, with the canvas, `criteria.ts` and `buildSql` staying in TS; inline raw text in `$tag$` strings). 2 needs no answer unless 2b slips.

1. **Commit the built WASM, or build it on demand?** Phase 1 committed the generated TS types and checks them in CI. The same trade-off looks different for a 1.56 MB binary: every rebuild adds a new blob to git history, and a byte-for-byte staleness check would fail on every stable Rust release, because `rust-toolchain.toml` follows `stable`. Building on demand needs the wasm32 target, a pinned wasm-bindgen-cli and wasm-opt wherever the frontend is built: dev machines, the frontend CI job, four release runners, the Docker image and the website's `demo:update`. Rust is already required for the frontend (`npm run check` runs `types:gen` through cargo), and the release runners compile Rust anyway. **Recommendation: build on demand, keep `src/lib/wasm/pkg/` in `.gitignore`**, with one Node build script that checks the toolchain and says exactly what to install (Task 7). If you'd rather commit, Task 7 drops the Docker, release and CI toolchain steps and adds a CI check that compares a hash of the crate sources recorded next to the committed `pkg/`, not the binary.
2. **Hotfix the Visual tab's `[object Object]` and `LIMIT NaN` in TS now?** No. Tasks 6 and 10 replace `sql-ast-parser.ts`, and these bugs have been in the product since the node-sql-parser 5.x upgrade. Only if 2b won't ship in the next release: then it's about 20 lines in `expressionToString` and the LIMIT branch.
3. **Should the query builder canvas stay in TS?** Yes, as the spike says. `query-builder-parsed-sql.ts` and the other hooks turn a `ParsedQuery` into canvas state (`SvelteSet`, UUIDs, positions), which is GUI state under design decision 8. `criteria.ts` reads the builder snapshot and the SQL text, never the AST, so it stays too. `query-builder-sql.ts` (`buildSql`, string building, no parser) also stays for 2b. Moving it belongs with the builder's identifier quoting, a phase 2 follow-up.
4. **A `{{param}}` inside a Postgres `$tag$…$tag$` string** (bug fix 13). Today it becomes `$1` inside the string, and Postgres fails with a bind-count error. Either inline the value's raw text, as DuckDB already does (with the same check that the value can't contain the closing tag), or raise a `ParameterSubstitutionError`. **Recommendation: inline, like DuckDB**, so a `DO $$ … {{p}} … $$` block works and the two engines behave the same. Raw inline inside `$tag$` is an injection path by design: the value's text becomes code if the string is itself run as SQL (a `DO` body, `EXECUTE`). That's accepted because the user writes both the query and the value; the tag check only stops a value from closing the string.

## Decisions (2026-09-27)

1. **node-sql-parser goes everywhere, the tutorial included.** The spike's port gave the same tutorial `ParsedQuery` on 91 of 91 tutorial queries, and every lesson criterion gave the same verdict. Keeping it for the tutorial would keep 420 KB gzipped of JS next to the WASM for nothing.
2. **Port at parity, quirks included.** The tutorial parser drops IN, IS NULL and BETWEEN filters, shifts the AND/OR connector by one filter, loses HAVING's table and FROM-subquery aliases, and keeps a decimal's text (`50.0`). The builder and the criteria depend on all of it, so fixing it is its own change with its own review (Follow-ups). The visual AST's formatting also matches the TS output, where it isn't a numbered fix: `IN ((subquery))` and `EXISTS((subquery))`, `x BETWEEN (1, 5)`, a `!=` kept as written, and `schema: null`/`alias: null` on INSERT/UPDATE sources. Task 2 found more of these than the spike's six (its `[object Object]` pattern hid them); `crates/seaquel-sql/tests/fixtures/README.md` lists them. Decided at the Task 2 checkpoint: these stay at parity, while casts (fix 15) and placeholders (fix 6) are fixed.
3. **One hand scanner.** `seaquel-sql` ports `sql-scan.ts`, which got 15 of 15 per-engine quoting cases right. The editor's `splitSqlStatements` state machine, which ignores its engine argument, is dropped. sqlparser's tokenizer isn't used for splitting: it errors on unterminated strings, which is the normal state of the editor while someone types.
4. **The query builder parses in the connection's dialect**; the tutorial stays in PostgreSQL mode. sqlparser's PostgreSQL dialect rejects backticks, so this is required, not optional.
5. **One module, loaded eagerly.** Init takes 3–27 ms, so there's no lazy loading. A trapped call re-instantiates the module from the already-compiled `WebAssembly.Module`, so one bad input can't leave the editor without a splitter.
6. **UTF-16 across the boundary, bytes inside Rust.** `seaquel-sql` works in UTF-8 bytes. `seaquel-wasm` converts at the boundary (`offsets.rs`). Statement text isn't sent back to JS; the wrapper slices the original string with the returned ranges, so a lone surrogate typed mid-edit survives (wasm-bindgen turns it into U+FFFD on the way in, which is also one UTF-16 unit, so ranges stay aligned).
7. **Tauri's CSP gets `'wasm-unsafe-eval'` in `script-src`.** Without it WebKit and Chromium refuse to compile the module (the spike reproduced both errors). The token lets the page compile WebAssembly and nothing else: `eval()` and `new Function` stay blocked, which is what `'unsafe-eval'` would have opened. Browsers added it for exactly this case. The web build sends no CSP, and the demo already runs DuckDB-WASM and sql.js.
8. **JSON strings across the boundary**, parsed on the TS side (33 µs for a join/group/having visual AST in the spike). TS types for everything that crosses come from Rust through ts-rs into `src/lib/types/generated/`, as for the wire types. They must match today's hand-written interfaces field for field, so the call sites don't change.
9. **Column sources: Rust resolves the column references, TS looks up the primary keys.** The wasm export returns, per output column, the table reference (`schema?`, `table`) and the column name. The wrapper finds the table in the cached schema list and reads its primary keys, as `findTable` does today. That keeps the whole schema cache from being serialized across the boundary on every query result. This is lookup, not SQL work.
10. **Bug-fix rule, as in phases 1 and 2:** fix anything that returns wrong data, silently drops a user's change or generates invalid SQL. Each fix gets a number, a fixture case with a reason in `bugfixes.json`, and a test. Everything else the ports reproduce, and odd behaviour goes to the Follow-ups. The builder dropping clauses it has no node for (`QUALIFY`, `GROUP BY ALL`, `OFFSET … FETCH`, as it always dropped a `LIMIT`'s `OFFSET` or an IN list) isn't a numbered fix: sqlparser's PostgreSQL dialect now accepts those clauses, where node-sql-parser rejected the query, so three non-tutorial tutorial.json entries parse without them. `tests/ast_parity.rs` pins their exact output (Task 6); warning about the loss is a Follow-up.
11. **Values in `{{param}}` substitution use the `Value` wire format.** The wrapper encodes with `encodeParam` and decodes bind values with `decodeCell`. A `Date` is turned into its ISO text before the call; every substituter already treats a `Date` as that text. `Bytes`, `Json` and `Array` values are rejected with an error. No caller passes them today (Task 4 checks: the only source of values is `coerceValue`, which gives null, numbers, booleans and strings). For JSON and arrays the TS would have inlined `[object Object]`; bytes it inlined on SQL Server (`0x…`) and DuckDB (`from_hex('…')`), a path no caller reached (Task 8 updated the two copied `query-params.test.ts` cases).

---

## Bug fixes

These are the only intended differences from the TS. Tasks refer to them by number, and `crates/seaquel-sql/tests/fixtures/bugfixes.json` describes each in its `about`. Fixes found while recording or porting get the next numbers, starting at 15.

**Visual tab** (`db/sql-ast-parser.ts`). The first four come from node-sql-parser 5.x's changed AST and have been live since that upgrade. In the demo today, `SELECT p.name, p.price FROM demo.products p WHERE p.price > 10 ORDER BY p.price` shows `p.[object Object] > 10` in the WHERE node and a LIMIT node reading `NaN`.

1. **Column names.** They print as `[object Object]`, because 5.x wraps them in `{ expr: { value } }` (144 of 172 corpus entries).
2. **No LIMIT.** A query without one gets a LIMIT node with `count: NaN`, because `limit` is now `{ value: [] }` (134 entries).
3. **Function calls.** Window and other non-aggregate calls print as `""`. A plain function call (`UPPER`, `COALESCE`, …) is worse: its name is an object in 5.x, the aggregate check throws on it, and the query gets no visualization at all (found in Task 2).
4. **DISTINCT.** It's never detected, because `distinct` is an object now.
5. **MySQL `LIMIT 5, 10`.** It's offset 5, count 10; the TS reads it the other way round.
6. **Dollar-quoted strings and placeholders.** Every `var` node prints as written: `$$it's$$`, `$q$…$q$`, `$1`, SQLite `$name`/`@name`, MySQL `@v`/`@@version` printed as `""` (so `WHERE id = $1` showed `id = `). A `:name` placeholder (a `param` node) printed as `$name`; it prints as `:name`.
7. **SQL Server `TOP n`.** It becomes the LIMIT node.
8. **DuckDB parses as DuckDB.** The TS parses DuckDB in PostgreSQL mode, so `GROUP BY ALL`, `QUALIFY` and other DuckDB syntax fail. That affects the Visual tab and column sources (inline editing).

**Query builder**

9. **Dialect.** Builder SQL parses in the connection's dialect: `` `Sales`.`customers` `` on MySQL and `[Sales].[customers]` on SQL Server round-trip. The second fails on both sides today, and the first worked only because node-sql-parser's PostgreSQL mode accepts backticks.

**Scanner** (`db/sql-parser.ts`, `db/query-utils.ts`, `db/query-params.ts`, `services/ai/context.ts`)

10. **Splitting follows the engine's quoting.** The editor's splitter ignores its `dbType`. On MySQL it splits inside `#` comments and backtick names and gets backslash escapes wrong. It also splits inside SQL Server bracket names, Postgres `E'…'` strings and nested block comments. A backtick or bracket name holding a `'` swallows the rest of the script into one statement. That affects Run, Run all, statement at cursor, EXPLAIN and the statement count. Also (Task 2 review): a MySQL/MariaDB executable comment (`/*! … */`, and `/*M! … */` on MariaDB) is code, so a `;` inside it splits, as the mysql and mariadb clients split it; and a Postgres/DuckDB `--` comment ends at `\r` as well as `\n`. From the Task 3 and 4 reviews: on MySQL/MariaDB `--` starts a comment only before ASCII space or a control character (`[\x00-\x20\x7F]`), so `--` + NBSP is minus minus (checked on MySQL 8.4 and MariaDB 11; the TS took any JS whitespace, which let `SELECT 1 --\u00A0x, LOAD_FILE(…)` past the read-only check); a `$tag$` may be any length (the TS scanner looked at 64 UTF-16 units, so a longer tag read as a word); and DuckDB's `{{param}}` substitution uses the shared scanner, so its `--` comment ends at `\r` too. Worth a line in the release notes, since MySQL and SQL Server scripts will split differently.
11. **Destructive-statement check.** The check before running a script, and `detectQueryType`, see through strings and comments. Today `DELETE FROM t -- WHERE id = 1` and `UPDATE t SET note = 'see WHERE'` aren't flagged, so they run on every row with no confirmation dialog. On MySQL a leading `# comment` hides a `DELETE` from the check and makes `SELECT` count as "other". Extended in the Task 2 review, since each of these ran with no confirmation:
    - `EXPLAIN ANALYZE DELETE`/`UPDATE` (also `EXPLAIN (ANALYZE …)`).
    - `ALTER TABLE t DROP c` without `COLUMN` (not `DROP CONSTRAINT`, `DROP DEFAULT`, `DROP NOT NULL`, …).
    - A data-modifying CTE, `WITH d AS (DELETE … RETURNING *) SELECT …`.
    - Only the DELETE's or UPDATE's own WHERE counts (at its own parenthesis depth, before its statement ends), not a CTE's or a subquery's.
    - `MERGE … THEN DELETE` (new reason `merge_delete`).
    - `DROP MATERIALIZED VIEW` (`drop_view`), `DROP SEQUENCE` (new `drop_sequence`), `DROP FUNCTION`/`PROCEDURE` (new `drop_function`), `DROP TEMPORARY TABLE` (`drop_table`).
    - SQL Server without `;`: `SELECT 1 DELETE FROM t`, `SET NOCOUNT ON UPDATE t SET a = 1`. Not `ON DELETE CASCADE`, `ON DUPLICATE KEY UPDATE` or `FOR UPDATE`.
    - The contents of a MySQL executable comment count as code.
    - Not flagged (re-review): a trigger's or policy's event list (`AFTER UPDATE`, `INSTEAD OF DELETE`, `INSERT OR UPDATE`, a list after `,`), `GRANT`/`REVOKE` privilege lists, T-SQL `IF UPDATE(col)` and `UPDATE STATISTICS`, and `EXPLAIN (…) DELETE` without `ANALYZE`. `EXPLAIN ANALYZE` and `EXPLAIN (ANALYZE …)` stay flagged.
12. **Source table for inline editing** (`extractTableFromSelect`). It takes the first top-level `FROM`, per engine. Today it takes the first `FROM` anywhere:
    - `SELECT EXTRACT(YEAR FROM created_at) FROM orders` gives the table `created_at`.
    - A subquery in the SELECT list gives the subquery's table (`users` for `SELECT o.id, (SELECT u.name FROM users u …) FROM orders o`).
    - A string gives whatever the string holds.
    - Backtick, bracket and non-ASCII names give nothing, so those queries can't be edited inline.
13. **`{{param}}` on Postgres and SQLite skips comments and quoted names.** The bind path finds strings with a scanner that only knows `'`. `SELECT 1 -- {{x}}` becomes `SELECT 1 -- $1` with one bind value, which Postgres rejects. `SELECT "it's", {{x}}, 'a{{x}}'` becomes `' || $1 || '` outside the string and the literal text `'a$1'` inside it. `E'…'` and `$tag$` strings are recognized (see open question 4); in an `E'…'` literal the rest reopens as `E'`, so its escapes keep their meaning (`' || $n || E'`). The forced-inline path (`forceInline`, the Visual tab and parsing) on Postgres, SQLite, MySQL and MariaDB takes the same contexts from the tokenizer, so `-- don't\nSELECT {{p}}` quotes the value, which it didn't; a negative number goes in parentheses (`1-{{p}}` with −1 was `1--1`); on MySQL `\` is doubled in a quoted value and a `"…"` string doubles `"`, and an executable comment is code (extended at the Task 2 checkpoint and review). MySQL's bound path, SQL Server and DuckDB already got this in phase 2; in 2b every path takes its contexts from the shared scanner. On MariaDB a `{{p}}` inside `/*M! … */` is substituted on the bound path too (MariaDB runs it; the TS copied it through), as inside `/*! … */` already.
    - **Values can't break out of their literal** (Task 4 review; breakouts confirmed live on Postgres, DuckDB and MySQL). A `$tag$` string must still end where it did once its values are in: `SELECT $$Cost: ${{p}}$$` with `$ || 'INJ' --` passed the "contains the tag" check but formed `$$` with the template's `$` and returned `Cost: INJ`. The same goes for the tag's other edge (`x$` before a template `$`), a partial tag (`$ta{{p}}` with `g$…`) and an unterminated string. Each is refused with `The value for {{p}} forms the dollar-quote tag $$ with the text next to it; use a different tag or a '…' string`. In a string that takes backslash escapes (`E'…'` on Postgres and DuckDB, `'…'` and `"…"` on MySQL/MariaDB), a `{{` after an odd run of `\` is escaped text, not a parameter: filling it left the `\` to escape the first quote of the value's doubled `''`. It's copied as it is, and on MySQL's bound path the CONCAT text takes only the unescaped parameters. A `SqlDecimal` whose text isn't `^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$` (so `NaN` too) is refused wherever its text goes into the SQL (`The value for {{p}} is not a finite decimal number`); bound, it goes out as it is. A bigint inlines as its digits (negatives in parentheses), like a number, not as the quoted string `escapeValueForInline` made of it. Extraction stays engine-agnostic, so the parameter dialog still asks for an escaped `\{{p}}` (Follow-ups).
    - **A value inlined in code is spaced off its neighbours** (`adjacent`, the Task 4 reviewer's randomized pass). A quoted value right after a quote merged with that literal through `''`: `E'a'{{p}}` with `\') OR 1=1 --` became `E'a''\'') OR 1=1 --'`, where the E-string's `\'` ends the literal early. It executed on the DuckDB CLI (from the TS) and was the same on Postgres forced inline. A space now goes before an inlined quoted value that follows `'`, `"` or `` ` ``, on every engine (SQL Server's `N'…'` already separates it). A value ending in a word character (a number, `NULL`, `1`/`0`) followed by a word character or `$` fused into one word (`NULL$$…`, `1$t$`), so a space goes after it too. A bound `$n` isn't spaced: `$1$t$` is a syntax error on Postgres, not a breakout, and it's the TS output (Follow-ups).
14. **The AI's read-only check runs on tokens** (`validateReadOnlyQuery`). Every statement must start with `SELECT` or `WITH`. No statement may contain a data- or schema-changing keyword outside strings and comments: the current list plus `MERGE`, `COPY`, `CALL`, `DO`, `EXECUTE`, `SET`, `LOAD`, `INSTALL`, `ATTACH`, `DETACH`, `PRAGMA`, `VACUUM`, `REPLACE`, `LOCK`, `FOR UPDATE`. `SELECT … INTO` stays blocked. Today `SELECT 1; COPY t TO PROGRAM '…'` passes the check. With "allow all queries" on, the AI then runs it without asking. Today `SELECT 'DROP TABLE' AS s` is refused; that false positive goes away too. Extended in the Task 2 review, after live bypasses of the first model:
    - **Stance:** this check gates which statements run; it is not a sandbox. User-defined functions can't be covered, and blocklists can't be complete. DuckDB's `read_csv('https://…' || …)` can send data out through httpfs, and it stays allowed. The Follow-ups' first item enforces read-only in the database, which is the real guard, though it doesn't stop network egress either.
    - MySQL/MariaDB: any input containing `/*!` or `/*M!` is refused (`SELECT 1; /*! DELETE FROM rv */` deleted rows on MySQL 8 and MariaDB 11). The input is scanned twice, with the default sql_mode and with `NO_BACKSLASH_ESCAPES` + `ANSI_QUOTES`, and refused if either reading is. Postgres is likewise scanned with `standard_conforming_strings` on and off (backslash escapes in `'…'`).
    - A word after a `.` is a name (`t.set`) only when a name comes before the `.`: `SELECT 1. INTO t` created a table.
    - SQL Server runs statements without `;` (`SELECT 1 AS k KILL 9999` ran KILL), so the T-SQL statement words are blocked there too: `KILL`, `SHUTDOWN`, `DBCC`, `BACKUP`, `RESTORE`, `DENY`, `RECONFIGURE`, `WAITFOR`, `RECEIVE`, `WRITETEXT`, `UPDATETEXT`, `BEGIN`, `COMMIT`, `ROLLBACK`, `SAVE` (TRANSACTION), `DECLARE`, `IF`, `WHILE`, `USE`, `SETUSER`, `REVERT`, `ADD`, `OPEN`, `CLOSE`, `DEALLOCATE`, `CHECKPOINT`, `ENABLE`, `DISABLE`, and `END`/`MOVE` before `CONVERSATION`. `FETCH` stays allowed (`OFFSET … FETCH`); `RAISERROR` and `PRINT` are harmless.
    - Functions with outside effects are blocked as a name followed by `(`, qualified or not: `dblink` and `dblink_*`, `lo_import`, `lo_export`, `pg_terminate_backend`, `pg_cancel_backend`, `set_config`, `setval`, `nextval`, `pg_reload_conf`, `pg_read_file`, `pg_read_binary_file`, `pg_ls_dir`, `OPENQUERY`, `OPENROWSET`, `OPENDATASOURCE`, `load_extension`, `LOAD_FILE`; from the re-review, `lo_unlink`, `lo_put`, `lo_from_bytea`, `lo_create`, the replication-slot functions (`pg_create_physical_replication_slot`, `pg_create_logical_replication_slot`, `pg_drop_replication_slot`, `pg_logical_slot_get_changes`, `pg_logical_slot_peek_changes`), `pg_notify`, `pg_advisory_lock`, `pg_advisory_xact_lock`, `pg_file_write`, `pg_file_unlink`, `pg_file_rename`, `pg_stat_reset*`, `pg_switch_wal`, `pg_promote`, `pg_rotate_logfile`, `pg_import_system_collations`, `pg_sleep`, MySQL `SLEEP`, `BENCHMARK` and `GET_LOCK`. A quoted name before `(` counts too, unquoted and compared case-insensitively (`"setval"(`, `` `load_file`( ``), and a Postgres `U&"…"` name before `(` is refused outright.
    - `REPLACE(` and MySQL's `INSERT(` are string functions, not statements.
    - Number tokens follow fix 18, so `1INTO` and `1.5INTO` no longer hide INTO.

**Visual tab**, found while recording (Task 2):

15. **Casts show their type.** `CAST(x AS integer)` and `x::date` printed as `CAST(x AS unknown)` (5.x gives the target as an array). They print as `CAST(x AS INTEGER)` and `x::DATE`, with the type as node-sql-parser upper-cases it plus its length and scale (`VARCHAR(10)`, `NUMERIC(10,2)`, `TEXT[]`). A `::` cast in GROUP BY was dropped (`created_at::date` showed `created_at`); it's kept. `BETWEEN (1, 5)`, `IN ((subquery))` and `!=` stay at parity (decision 2). Found while recording (Task 2).

**Table editor** (`db/parse-create-table.ts`), found while recording (Task 2). Both run on the DDL our own generators write, which is what the SQL pane shows first.

16. **`parseCreateTableSql` stops dropping data.**
    - A `SERIAL`, `SMALLSERIAL` or `BIGSERIAL` column keeps its type as written. The TS listed `SERIAL` among the constraint keywords, so `id serial` got type `""`: every Postgres table our DDL generator writes with a serial key, and the e2e schema. The engines' column-type lists have `serial` and `bigserial`, and the editor matches type names case-insensitively.
    - A column after a comment in the column list is kept (the TS dropped it).
    - `CONSTRAINT name PRIMARY KEY (…)` and `CONSTRAINT name UNIQUE (…)` are read (the TS skipped them, so the key was lost).
    - An inline `REFERENCES t (c)` on a column is a foreign key (the SQLite e2e schema writes its keys that way). Without a column list the referenced column is `""`.
    - A column's `COLLATE name` goes into its `collation` and no longer bleeds into the type (`b text COLLATE "C"` had type `text COLLATE "C"`). The SQL Server engine's own DDL writes `COLLATE`.
17. **`parseCreateTableSql` reads every name our generators write.** A quoted name may hold its doubled quote (`""`, ` `` `, `]]`), which is unescaped; the TS returned `null` for the whole table, including the escaped DDL phase 1 and 2 fixed in the engines. A three-part `catalog.schema.table` name parses with the schema `catalog.schema` (DuckDB's attached catalogs, as the DuckDB engine names them). Index and constraint names follow the same rules, so an index whose name or table holds a space is no longer dropped. Backtick and bracket names with commas or parentheses no longer split the column list. A three-part name's catalog or schema that holds `.` or `"` is double-quoted in `schemaName` (`"my.cat".main`), as DuckDB's `part()` writes it, so the DuckDB engine's `quote_schema` round-trips it.
18. **Number tokens end where the numeric literal ends** (Task 2 review). `sqlTokens` ran a number on through word characters, so `1INTO` was one word and hid `INTO` from every check (`SELECT x = 1INTO #q` ran on SQL Server, `SELECT 1.5INTO @z` on MySQL). A token that starts with a digit (or `.` and a digit) takes digits, an optional `.` and digits, and an optional exponent, and ends there. Per engine (re-review): on SQL Server `0x` plus hex digits and `$` plus a number (money) are numbers too (`SELECT a=0x1INTO #q` and `a=$1INTO #m` created tables). On MySQL/MariaDB a digit run followed by a word character is one word, a name like `2fa_codes` (MySQL reads `1INTO` as a name too), unless the literal has a `.` or an exponent (`1.5INTO`, `1e1INTO` split); a digit-led run after a `.` is a name (`db.2fa_codes`); and `@` is a word character only at the start of a word (`INTO@x` splits). This changes the tokenizer Task 3 ports, so it also changes `hasRowLimit` (`WHERE 1=1LIMIT 5` limits its rows); the fixtures mark every case it changes.
19. **Word characters follow each engine's identifier rule** (Task 3 review). `sqlTokens` took one UTF-16 unit matching `[\p{L}\p{N}_$@]`, so a combining mark or an astral letter ended a word: `ตารางที่` read as `ตารางท`, NFD `café` as `cafe`, and `FROM 𝒯able` found no table. Inline editing targeted the wrong table, and it disagreed with sqlparser's column references. Now: on Postgres, DuckDB and SQLite any non-ASCII character is a word character (their lexers take any byte >= 0x80); on MySQL/MariaDB any non-ASCII BMP character (U+0080–U+FFFF), not an astral one; on SQL Server the TS set plus `\p{M}`, with astral letters and marks read by code point. A Postgres/DuckDB `$tag$` may hold any non-ASCII character too. Whitespace stays JS whitespace. For safety, `read_only_error` scans with both the old and the new word rule and refuses if either refuses (like the sql_mode readings), so a mark glued to a keyword (`INTO` + U+0301 on SQL Server) can't hide it.

---

## What moves, and who calls it

From a grep of `src/`. "Stays" means it stays in TS.

| Today                                                                                                                                     | Callers                                                                                                                                                                                                                                                            | In 2b                                                                                                                         |
| ----------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- |
| `db/sql-parser.ts` `splitSqlStatements`                                                                                                   | `components/query-editor/view-state.svelte.ts:67` (statement count, a `$derived`), `components/query-editor/execution.svelte.ts:60`, `hooks/database/query-execution.svelte.ts:754`                                                                                | `scan::split_statements` (fix 10)                                                                                             |
| `db/sql-parser.ts` `getStatementAtOffset`                                                                                                 | `components/query-editor/execution.svelte.ts:39,84`, `components/query-editor/explain-visualize.svelte.ts:36`, `hooks/database/resolve-query.ts:29`                                                                                                                | `scan::statement_at`                                                                                                          |
| `db/query-params.ts` `extractParameters`, `hasParameters`                                                                                 | `save-query-dialog.svelte:64`, `query-editor/param-dialog.svelte.ts:26`, `standalone-query-editor.svelte:86,87`, `services/query-file-parser.ts:260`; `hasParameters` also `query-editor/execution.svelte.ts:24,41`, `query-editor/explain-visualize.svelte.ts:19` | `params::extract_parameters`, `has_parameters`                                                                                |
| `db/query-params.ts` `substituteParameters`, `ParameterSubstitutionError`                                                                 | `standalone-query-editor.svelte:102`, `hooks/database/resolve-query.ts:37,41`, `hooks/database/query-execution.svelte.ts:571,783`                                                                                                                                  | `params::substitute` (fix 13); the error class stays in TS and the wrapper throws it                                          |
| `db/query-params.ts` `createDefaultParameters`, `coerceValue`                                                                             | `query-editor/param-dialog.svelte.ts:27`, `standalone-query-editor.svelte:88`, `parameter-input-dialog.svelte:49`                                                                                                                                                  | Stay (no SQL); move to `src/lib/sql/parameters.ts`                                                                            |
| `db/query-params.ts` `escapeValueForInline`, `escapeValueForMssql`                                                                        | internal only (`escapeValueForMssql` is deprecated, no callers)                                                                                                                                                                                                    | Ported as private functions, not exported                                                                                     |
| `db/query-utils.ts` `detectQueryType`, `isSelectQuery`                                                                                    | `hooks/database/query-execution.svelte.ts` (261, 363, 595, 600, 614, 789, 794, 805, 984)                                                                                                                                                                           | `statements::query_type` (fix 11)                                                                                             |
| `db/query-utils.ts` `extractTableFromSelect`                                                                                              | `hooks/database/query-execution.svelte.ts:109,491`                                                                                                                                                                                                                 | `statements::table_from_select` (fix 12)                                                                                      |
| `db/query-utils.ts` `isDestructiveStatement`, `findDestructiveStatements`, types `QueryType`, `DestructiveReason`, `DestructiveStatement` | `query-editor/execution.svelte.ts:61,86`, `destructive-query-confirm-dialog.svelte`, `hooks/database/pending-changes.svelte.ts`, `types/pending-changes.ts`, `types/query.ts`                                                                                      | `statements::destructive_reason` (fix 11); types generated                                                                    |
| `engine/sql-scan.ts` `hasRowLimit`, `countQuery` (`stripTrailingOrderBy` and `sqlTokens` are internal)                                    | `hooks/database/query-execution.svelte.ts:101,423,467`                                                                                                                                                                                                             | `scan::has_row_limit`, `count_query`                                                                                          |
| `services/ai/context.ts` `validateReadOnlyQuery`                                                                                          | `services/ai/index.ts:125`                                                                                                                                                                                                                                         | `read_only::read_only_error` (fix 14)                                                                                         |
| `db/parse-create-table.ts` `parseCreateTableSql`                                                                                          | `components/create-table-view.svelte:176`                                                                                                                                                                                                                          | `create_table::parse_create_table` (fixes 16, 17)                                                                             |
| `tutorial/sql-parser.ts` `parseSql`, types `ParsedQuery` etc.                                                                             | `query-builder/sql-editor.svelte:65,66,86`, `query-editor/visual-query-panel.svelte:40`, `standalone-query-editor.svelte:74`; types in `hooks/query-builder-parsed-sql.ts`, `hooks/query-builder.svelte.ts`                                                        | `ast::parse_builder_query` (fix 9); types generated                                                                           |
| `db/sql-ast-parser.ts` `parseQueryForVisualization`, `getParseError`                                                                      | `hooks/database/visualize-tabs.svelte.ts:56,59,79,82`                                                                                                                                                                                                              | `ast::parse_visual`, `ast::parse_error` (fixes 1–8, 15); `ParsedQueryVisual` generated, replacing `types/visualize.ts`'s      |
| `db/column-sources.ts` `resolveColumnSources`                                                                                             | `hooks/database/query-execution.svelte.ts:149` (via `resolveColumnSources` at 263, 530)                                                                                                                                                                            | `ast::column_refs` + a TS primary-key lookup (decision 9)                                                                     |
| `db/pending-change-description.ts` `describePendingChange`                                                                                | `hooks/database/pending-changes.svelte.ts:49`, `hooks/database/query-crud.svelte.ts:227,315`                                                                                                                                                                       | Stays, unchanged: moves to `hooks/database/pending-change-description.ts` (English labels over SQL our own builders generate) |
| `db/index.ts`, `db/duckdb.ts`, `db/alter-table.ts`, `db/crud-helpers.ts` (+ test)                                                         | the demo's `TsEngineClient`                                                                                                                                                                                                                                        | Stay, demo-only, until phase 8. `index.ts` has to stay too: it's the `DatabaseAdapter` interface and `getAdapter`             |
| `engine/qualified-table.ts`, `engine/reserved-words.ts`, the local `PAGINATE` in `rust-engine-client.ts`                                  | engine clients                                                                                                                                                                                                                                                     | Stay (mirrors of the Rust dialects; Follow-ups)                                                                               |
| `tutorial/criteria.ts`, `hooks/query-builder-*.ts`                                                                                        |                                                                                                                                                                                                                                                                    | Stay (open question 3)                                                                                                        |

About 3,500 lines of TS go: `tutorial/sql-parser.ts` 1,213, `query-params.ts` ~680 of 715, `sql-ast-parser.ts` 532, `parse-create-table.ts` 306, `sql-parser.ts` 254, `sql-scan.ts` 195, `column-sources.ts` 181, `query-utils.ts` 112, the read-only check ~30. Also node-sql-parser.

---

## Ground rules for whoever executes this

- **No git writes.** The owner forbids `git add`/`commit`/`mv`/`stash`/branches/worktrees. Use plain `mv`/`cp`/`rm`. Each task ends with a checkpoint: summarise the changes and the verification, then let the user review. Read-only git is fine.
- Run everything from the repo root. Never edit `src/lib/components/ui/*`. Error toasts: `errorToast`. Run the Svelte MCP `svelte-autofixer` on every changed `.svelte` file. Run oxfmt on changed TS.
- `seaquel-sql` and `seaquel-wasm` follow `crates/clippy.toml` (no `Instant`, `SystemTime`, `tokio::spawn`, `std::thread::spawn`) and must build for `wasm32-unknown-unknown`. No `regex` crate in `seaquel-sql`: it adds noticeably to the module. Measure with the Task 7 build script before adding any dependency.
- **Nothing in `seaquel-sql` or `seaquel-wasm` may panic on user input.** A panic in the module is a trap (`panic = "abort"`). Every scanner test also runs every prefix of its input, because the editor calls these on every keystroke.
- **Parallel agents.** Tasks 3, 4, 5 and 6 edit the same crate. Each agent owns only its own files (listed per task) and uses its own `CARGO_TARGET_DIR` outside the repo (for example `$TMPDIR/seaquel-target-task6`). In phase 2, agents sharing one target dir stalled on cargo's build lock. The inner loop is `cargo test -p seaquel-sql`, which doesn't build DuckDB. Keep your module compiling whenever you run cargo. If the build fails in a file you don't own, wait and rerun; don't edit it. The full check list runs at the checkpoint, one agent at a time.
- **No Docker databases.** No task touches an engine crate, so the live engine tests aren't needed, and phase 2 lost time to a hung Docker. The one Docker use is the image build in Task 7. Run it in the background with a time limit, and if Docker hangs, report it instead of retrying in a loop.
- **TDD.** Write the failing test first: a fixture case, a Rust unit test or a vitest case.
- **Parity rule.** Rust output equals the recorded TS output (Task 2's frozen fixtures), except the numbered bug fixes, each with a documented exception and a test. If a fixture disagrees with your port, the fixture wins, unless the difference is one of those fixes. Error messages users see (`ParameterSubstitutionError`, the read-only message) match the TS text exactly.
- **Frozen fixtures.** Once Task 2's checkpoint is accepted, `crates/seaquel-sql/tests/fixtures/*.json` changes only when behaviour is meant to change. Say which fix, and why, in `bugfixes.json`.
- **User-facing strings.** Any new `messages/en.json` key is translated with the `i18n-translator` agent before the checkpoint.
- **Full check list before every checkpoint:**
  - `npm run crates:check`
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`
  - `cargo clippy --target wasm32-unknown-unknown -p seaquel-types -p seaquel-runtime -p seaquel-engine -p seaquel-sql -p seaquel-wasm -- -D warnings`
  - `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio` (no `SEAQUEL_TEST_*` needed)
  - `cargo check -p seaquel`
  - `npm run wasm:build` (from Task 7 on)
  - `npm run check`
  - `npx vitest run`
  - `npx oxlint --type-aware --type-check --deny-warnings`
  - From Task 7 on, in tasks that touch the frontend: `npm run build`, `NODE_OPTIONS=--max-old-space-size=8192 npm run build:web` and `npm run build:demo`
- **Effort log.** Every implementer appends one line per task to `docs/plans/2026-09-27-phase-2b-effort.md` (the spike's line is already there): task, wall time, Rust lines added, TS lines removed, surprises. Task 13 needs these numbers.

---

## Order and estimates

| #   | Task                                                                            | Estimate     | Needs | Can run alongside |
| --- | ------------------------------------------------------------------------------- | ------------ | ----- | ----------------- |
| 1   | Create the crates                                                               | 0.5–1 h      | —     | 2                 |
| 2   | Freeze the TS baseline                                                          | 1.5–2.5 h    | —     | 1                 |
| 3   | Scanner and statement checks                                                    | 1.5–3 h      | 1, 2  | 5, 6, 7           |
| 4   | `{{param}}` substitution                                                        | 1.5–3 h      | 3     | 5, 6, 7           |
| 5   | `parse_create_table`                                                            | 1–1.5 h      | 1, 2  | 3, 4, 6, 7        |
| 6   | AST helpers on sqlparser                                                        | 2.5–3.5 h    | 1, 2  | 3, 4, 5, 7        |
| 7   | Build and load the module everywhere                                            | 1.5–2.5 h    | 1     | 2–6               |
| 8   | Exports, offsets and `src/lib/sql`                                              | 1.5–2 h      | 3–7   | —                 |
| 9   | Switch the editor and query runner                                              | 1–1.5 h      | 8     | 10                |
| 10  | Switch the Visual tab, builder, tutorial and table editor; drop node-sql-parser | 1–1.5 h      | 8     | 9                 |
| 11  | CI                                                                              | 0.25–0.5 h   | 9, 10 | —                 |
| 12  | Docs and cleanup                                                                | 0.5–0.75 h   | 11    | —                 |
| 13  | Measure                                                                         | 0.25 h       | 12    | —                 |
|     | Review fixes (phase 2 ran at about a quarter)                                   | 3.5–5.5 h    |       |                   |
|     | **Total**                                                                       | **~18–29 h** |       |                   |

That matches the spike's revised estimate. The spike's rows map like this: baseline is Task 2; scanners and `parse_create_table` are Tasks 3–5; AST helpers are Task 6; `seaquel-wasm` is Tasks 7–8; switch and delete are Tasks 9–10. Tasks 11–13 weren't in the spike's table.

Task 7 is the one with unknowns: the toolchain on four release runners, Docker and a real Tauri window. It starts right after Task 1, so the plumbing is settled before the ports need it. Tasks 9 and 10 split `query-execution.svelte.ts` so they don't collide: column sources go with Task 9, which owns that file.

---

## Part A — Groundwork

### Task 1: Create the crates

**Files:**

- Create `crates/seaquel-sql/`:
  - `Cargo.toml`
  - `src/lib.rs`, `src/engine.rs` (`SqlEngine`)
  - `src/scan.rs`, `src/statements.rs`, `src/read_only.rs` (Task 3)
  - `src/params.rs` (Task 4)
  - `src/create_table.rs` (Task 5)
  - `src/ast/{mod.rs,util.rs,dialect.rs,tutorial.rs,visual.rs,column_refs.rs}` (Task 6)
- Create `crates/seaquel-wasm/`: `Cargo.toml`, `src/lib.rs`, `src/offsets.rs`.
- Modify: `Cargo.toml` (members, workspace deps, `[profile.wasm-release]`), `rust-toolchain.toml`, `scripts/check-crate-deps.mjs` and `scripts/check-crate-deps.test.mjs`, `.github/workflows/ci.yml` (the wasm32 clippy line), `package.json` (`types:gen`), `vite.config.js` (`test.exclude`).

**Steps:**

1. `seaquel-sql` depends on:
   - `sqlparser = { version = "0.63", default-features = false, features = ["std"] }`. The spike found `recursive-protection` and `visitor` made no size difference; the parser's own recursion limit (50) still applies.
   - `serde`, `serde_json`, `seaquel-types` (for `Value` and `CreateTableDefinition`), and `ryu-js` (Task 4 formats numbers the way JS does).
   - `ts-rs`, optional behind a `ts` feature, as in `seaquel-types`.
2. `SqlEngine` has one variant per `DatabaseType`: `postgres`, `mysql`, `mariadb`, `sqlite`, `mssql` and `duckdb`. MariaDB uses sqlparser's MySQL dialect, since sqlparser has none of its own. An unknown string is an error at the wasm boundary, not a silent fallback.
3. Stub every public function in the API listed in Tasks 3–6, with its final signature and a `todo!()` body. The parallel tasks then only fill in bodies. Stubs must pass clippy (underscore the unused parameters).
4. `seaquel-wasm` is a `cdylib` + `rlib` that depends on `seaquel-sql`, `serde_json` and `wasm-bindgen = "=0.2.128"`. That's the version `Cargo.lock` already has, through chrono. The spike's stub pinned 0.2.129, but it lived outside the workspace; inside it, the exact pin fixes the version for the whole workspace. It has one export, `version()`, for Task 7 to load.
5. Add `[profile.wasm-release]` to the workspace `Cargo.toml`: `inherits = "release"`, `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`. Don't put these under `[profile.release]`: that would shrink and slow `seaquel-server` and the desktop app too.
6. Add `targets = ["wasm32-unknown-unknown"]` to `rust-toolchain.toml`.
7. Update the crate rules:
   - `seaquel-sql` joins `PURE`.
   - Add `WASM_GLUE = new Set(["seaquel-wasm"])`: it may depend on pure crates only. If engine dialects come later (Follow-ups), the rule changes then.
   - Engines may already use `seaquel-sql`.
   - Tests: the new workspace passes; `seaquel-wasm → seaquel-core` and `seaquel-sql → seaquel-engine-postgres` are rejected.
8. CI's "Pure crates build for wasm32" step adds `-p seaquel-sql -p seaquel-wasm`.
9. `types:gen` adds `-p seaquel-sql --features seaquel-sql/ts`.
10. `vite.config.js`: `test.exclude` adds `spike/**`. Today `npx vitest run` picks up the three spike harness files, which run cargo and rewrite `spike/harness/out`.

**Tests first:** the crate-rule tests; then `cargo clippy --target wasm32-unknown-unknown -p seaquel-sql -p seaquel-wasm -- -D warnings` on the stubs.

**Review:** check the stubbed API against Tasks 3–6. Check that `Cargo.lock` gained sqlparser and ryu-js and no second wasm-bindgen.

**Checkpoint.**

### Task 2: Freeze the TypeScript baseline

The spike's harness becomes a recorder, and its outputs become frozen fixtures in the new crate, as the engine fixtures were. It has to run before Tasks 9 and 10 delete the TS.

**Files:**

- Create `scripts/sql-fixtures/`:
  - `corpus.ts`: `spike/harness/corpus.ts` moved here. 172 AST entries: 91 tutorial, 57 builder round trips, 24 editor statements.
  - `scanner-corpus.ts`
  - `acceptance.ts`: the 140 repo statements from `spike/harness/acceptance.test.ts`.
  - `sql.fixtures.test.ts`
  - `visual-fixed.ts`, `split-model.ts` (models for the bug fixes, below)
- Create `crates/seaquel-sql/tests/fixtures/*.json`, `bugfixes.json` and `README.md`.
- Modify `.oxfmtrc.json`: ignore `crates/seaquel-sql/tests/fixtures`.

`sql.fixtures.test.ts` writes the fixtures when `RECORD_FIXTURES=1`. Otherwise it checks that today's TS still produces them, so it runs in plain `npx vitest run` until the TS is gone. It imports the real TS: `parseSql`, `parseQueryForVisualization`, `getParseError`, `resolveColumnSources`, the scanners, `substituteParameters`, `parseCreateTableSql`, `applyParsedSqlToState`, `buildSql` and the lesson criteria. Hard-code nothing environment-specific. No database is needed.

| Fixture                              | What it records                                                                                                                                                                                                           |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `tutorial.json`                      | `parseSql` for the 172 AST entries, with `validTableNames` as the entry says (tutorial list or `null`)                                                                                                                    |
| `builder.json`                       | For the 57 round trips: the entry's engine, `parseSql` (PostgreSQL mode, as today) and the SQL `buildSql` regenerates                                                                                                     |
| `criteria.json`                      | Every lesson criterion's verdict for the 91 tutorial entries                                                                                                                                                              |
| `visual.json`                        | `parseQueryForVisualization` for all 172 entries, in the entry's engine                                                                                                                                                   |
| `parse-error.json`                   | `getParseError`: `null` or not, per entry. Messages differ between the parsers and aren't compared                                                                                                                        |
| `column-sources.json`                | `resolveColumnSources` against the spike's column schema (every tutorial table in `public`, `Sales`, `shop`, `dbo`, `main`)                                                                                               |
| `split.json`                         | `splitSqlStatements` per engine: every statement's `sql`, `index`, `startOffset` and `endOffset`                                                                                                                          |
| `statement-at.json`                  | `getStatementAtOffset` for **every** UTF-16 offset of each scanner input, per engine                                                                                                                                      |
| `row-limit.json`, `count-query.json` | `hasRowLimit`, `countQuery` per engine                                                                                                                                                                                    |
| `statements.json`                    | `detectQueryType`, `isDestructiveStatement` and `extractTableFromSelect`                                                                                                                                                  |
| `read-only.json`                     | `validateReadOnlyQuery`                                                                                                                                                                                                   |
| `params.json`                        | `extractParameters`, `hasParameters` and `substituteParameters` per engine, with `forceInline` on and off. Values in the wire format. Outputs are `{ sql, bindValues }` or `{ error }`                                    |
| `create-table.json`                  | `parseCreateTableSql`                                                                                                                                                                                                     |
| `acceptance.json`                    | Whether sqlparser parses each of the 140 repo statements in its engine's dialect. It isn't a TS output: it guards against a sqlparser upgrade that stops parsing SQL we ship. Recorded from the spike: 138 parse, 2 don't |

**The scanner corpus** (`scanner-corpus.ts`), each input run for all six engine ids:

- The spike's 15 splitter inputs.
- Every input in `src/lib/engine/sql-scan.test.ts` (49 cases) and `src/lib/db/query-params.test.ts` (31 cases).
- The cases the phase 2 reviews added to `query-params.ts`:
  - MSSQL `N'…'` for non-ASCII text.
  - `TOP {{n}}`.
  - `1-{{p}}` with −1, which must not become `1--1`.
  - `{{p}}` in comments and `[names]`.
  - DuckDB `$tag$` with a value holding the tag.
  - MySQL charset introducers, adjacent literals and `/*! … */`.
- The six e2e `schema.sql` files, whole.
- The sample queries.
- New per-engine edge cases:
  - `#` mid-word on MySQL vs `#temp` on T-SQL, `/*!…*/` on MySQL.
  - `E'\''`; `$1` vs `$tag$`; nested comments.
  - `]]` in SQL Server names vs SQLite `[…]`, which has no escape.
  - A quote inside a backtick or bracket name.
  - An unterminated string, comment and name at the end.
  - Only comments; empty; `;;`; CRLF.
  - `東京` and `😀` before and after the cursor.

Aim for at least 150 inputs.

**Expected outputs for the bug fixes** come from small models, as in phase 2. `bugfixes.json` records each case the model changes, with the fix number:

- `visual-fixed.ts` is a copy of `sql-ast-parser.ts` with fixes 1–4 applied, each edit tagged with its number (fixes 6 and 15 were added at the checkpoint). The recorder writes `visual.json` from the model. Each case lists the fixes that make it differ from the unmodified TS. The recorder fails if the model differs in a way the spike's four patterns don't explain (the patterns are in `spike/harness/harness.test.ts`, `visualCategory`).
- `split-model.ts` splits on `sqlTokens` from `sql-scan.ts`, which was right on 15 of 15. The recorder writes the split and statement-at fixtures from the model. Each case where the TS splitter disagrees is marked fix 10.
- Fixes 5–9 and 11–14 are written by hand in `bugfixes.json`, a few cases each. Take the fix 9 cases from the spike's round trips with the engine dialect (45 of 57 regenerate exactly vs 44 today).
- As done: fixes 11–14 changed hundreds of per-engine scanner cases, so they got models too (`statements-model.ts`, `params-model.ts`), and fixes 16 and 17 have `create-table-fixed.ts`. The recorder lists every case a model changes in `bugfixes.json`'s `modelCases`; the hand-written `cases` cover fixes 5, 7, 8 and 9 and a few extra inputs for 3, 6 and 15.

**Steps:**

1. Move the corpus, write the recorder and models.
2. `RECORD_FIXTURES=1 npx vitest run scripts/sql-fixtures`.
3. `npx vitest run scripts/sql-fixtures` without the variable must pass.
4. Write `README.md`: where each fixture came from, the case format, and that the recorder is deleted with the TS (Task 12 keeps a copy in `docs/plans/artifacts/`).

**Review:** spot-check ten `visual.json` cases against the running demo's Visual tab. Check that no `bugfixes.json` case is there without a fix number and a reason. Check that the scanner corpus has at least one unterminated input per quote style per engine.

**Checkpoint.**

---

## Part B — `seaquel-sql`

### Task 3: Scanner and statement checks

**Files:** `crates/seaquel-sql/src/{scan.rs,statements.rs,read_only.rs}`, `crates/seaquel-sql/tests/scan_parity.rs`.

**API** (byte offsets, `end` exclusive):

```rust
pub fn tokens(sql: &str, engine: SqlEngine) -> Vec<Token>;             // port of sqlTokens
pub fn split_statements(sql: &str, engine: SqlEngine) -> Vec<Statement>;
pub fn statement_at(sql: &str, offset: usize, engine: SqlEngine) -> Option<Statement>;
pub fn has_row_limit(sql: &str, engine: SqlEngine) -> bool;
pub fn strip_trailing_order_by(sql: &str, engine: SqlEngine) -> &str;
pub fn count_query(sql: &str, engine: SqlEngine) -> String;
pub fn query_type(sql: &str, engine: SqlEngine) -> QueryType;
pub fn destructive_reason(sql: &str, engine: SqlEngine) -> Option<DestructiveReason>;
pub fn table_from_select(sql: &str, engine: SqlEngine) -> Option<TableRef>;
pub fn read_only_error(sql: &str, engine: SqlEngine) -> Option<&'static str>;
```

`Statement` carries `index`, the TS's `start` and `end` (see below), and the range of the trimmed text.

**Steps:**

1. Port `sql-scan.ts` as the one tokenizer, rule for rule; its header comment lists each engine's quoting. These rules change (see `scripts/sql-fixtures/scan-model.ts`): number tokens (fix 18); word characters per engine (fix 19, `wordCharLen`, and the `$tag$` rule `dollarTag`); a Postgres/DuckDB `--` comment ends at `\r` too, a MySQL/MariaDB `--` needs ASCII space or a control character after it, and a `$tag$` has no length limit (fix 10); and an option to read a MySQL/MariaDB executable comment (`/*!`, `/*!50700`, and `/*M!` on MariaDB only) as code, which the splitter, the statement checks, the read-only check and forced inline use and `has_row_limit` doesn't (Follow-ups). The read-only check also needs MySQL's `NO_BACKSLASH_ESCAPES` + `ANSI_QUOTES` reading, Postgres's `standard_conforming_strings = off` reading, and each reading with the TS word rule as well (fix 19). Add a token kind for comments, which `tokens` skips and the splitter needs. Unterminated strings, comments and names run to the end of the input. It's a total function.
2. `split_statements` splits on `;` tokens and reproduces the TS splitter's shape exactly outside fix 10:
   - `startOffset` (`Statement::start`) is one past the previous top-level `;`, including a `;` that ended a dropped (empty or comment-only) statement; 0 for the first.
   - `Statement::end` is the byte offset of the terminator: the `;`, or `sql.len()` when there isn't one, so it is always a char boundary. The TS `endOffset` is `length - 1` for an unterminated last statement; that mapping (`end == len` to UTF-16 `len16 - 1`) is one line in `seaquel-wasm` and the parity harness, not in `seaquel-sql`, because `len - 1` can land inside a multi-byte char (`…東京`, a trailing `-- ✓`) and converting it would panic. No TS caller reads `startOffset`/`endOffset`.
   - `sql` is the trimmed text.
   - Comment-only statements are dropped and the rest re-indexed.

   `statement_at` keeps the TS rules for a cursor before, between and after statements (`start <= offset <= end`). It accepts any offset, past the end or inside a char: it compares numbers and never slices at `offset`.

   Trimming and whitespace follow JS: `trim()` and `/\s/` treat U+FEFF and the other JS whitespace characters as whitespace, and Rust's `str::trim`/`char::is_whitespace` don't. Use one JS-whitespace predicate for parity.

3. `query_type`, `destructive_reason` and `table_from_select` work on significant tokens, with the TS keyword lists and fix 11's additions (fixes 11 and 12; the rules are in `scripts/sql-fixtures/statements-model.ts`). `DestructiveReason` gains `drop_sequence`, `drop_function` and `merge_delete`. `table_from_select` unquotes per engine and returns the first top-level `FROM`'s table. It returns nothing for a subquery, a join's second table or a table function, as the TS regex effectively did for the common cases.
4. `read_only_error` implements fix 14 (the rules are in `scripts/sql-fixtures/statements-model.ts`, `readOnlyModel`) and returns the TS message, `Only read-only SELECT queries are permitted`.
5. No splitting on `BEGIN … END`, MySQL `DELIMITER` or SQL Server `GO`. Neither the TS nor the spike handles them (Follow-ups).
6. Types: derive ts-rs (behind the `ts` feature) on `QueryType`, `DestructiveReason` and `TableRef`, run `npm run types:gen`, and diff the output against today's TS types. No other task generates them.

**Tests first:**

- `scan_parity.rs` over the fixtures, with the bug fixes applied from `bugfixes.json`.
- For every scanner-corpus input and every prefix of it: no panic, every returned range inside the input and on char boundaries, and the statements plus what's between them rejoin to the input.
- A 1 MB script splits in well under the 60 ms sqlparser's tokenizer took natively.

**Review:** phase 2's reviews found real quoting bugs in exactly this kind of code, twice, and none had a failing test until the reviewer wrote one. Before reading the code, write ten adversarial inputs per engine. Then compare the tokenizer with `sql-scan.ts` branch by branch:

- `#` mid-word on MySQL, where it starts a comment, vs a word character elsewhere.
- `[` escaping on SQLite (none) vs SQL Server (`]]`).
- `E'` only after a non-word character.
- `$1` vs `$tag$`.
- Backslash escapes in MySQL `"…"` strings.
- The `.limit` rule (`t.limit` is a column).
- Nested comments on Postgres, DuckDB and SQL Server only.

**Checkpoint.**

### Task 4: `{{param}}` substitution

Needs Task 3's tokenizer.

**Files:** `crates/seaquel-sql/src/params.rs`, `crates/seaquel-sql/tests/params_parity.rs`.

**API:**

```rust
pub fn extract_parameters(sql: &str) -> Vec<String>;
pub fn has_parameters(sql: &str) -> bool;
pub fn substitute(sql: &str, values: &[(String, Value)], engine: SqlEngine, force_inline: bool)
    -> Result<Substituted, SubstitutionError>; // Substituted { sql, bind_values: Vec<Value> }
```

**Steps:** port `query-params.ts`, one engine path at a time, and take the literal and comment boundaries from the tokenizer instead of each path's own loop:

- **`extract_parameters` / `has_parameters`.** Engine-agnostic, as today: every `{{name}}` in the text, comments included (Follow-ups).
- **Postgres and SQLite, bound.** `$n` numbered by first use, and `' || $n || '` inside a `'…'` literal. Fix 13: comments and quoted names are copied untouched, `E'…'` is a literal, and a `$tag$` string follows open question 4.
- **Forced inline** (Postgres, SQLite, MySQL and MariaDB with `forceInline`). `escapeValueForInline`, including the `insideString` rules, with the contexts from the tokenizer (fix 13): a comment or quoted name is left alone (a MySQL executable comment is code), a negative number in code goes in parentheses, a `'…'` literal takes the value's text, an `E'…'` literal also doubles `\`, a MySQL string doubles `\` (and a `"…"` one doubles `"` instead of `'`), and a `$tag$` string the raw text with the tag check. The rules are in `scripts/sql-fixtures/params-model.ts`.
- **MySQL and MariaDB.** `?` per occurrence. A literal holding parameters becomes one `?` bound to its CONCAT text, NULL if any value is NULL. The `?` swallows a charset introducer (`N'…'`, `_utf8mb4 '…'`) and adjacent literals. Executable comments (`/*! … */`) are substituted, other comments and backtick names aren't. `MYSQL_ESCAPES` is kept, including `\%` and `\_`. On the shared scanner: a `/*! … */` comment is code, and on MariaDB `/*M! … */` too (fix 13); `--` + NBSP isn't a comment (fix 10). The rules are in `scripts/sql-fixtures/params-model.ts` (`substituteMysqlBound`).
- **SQL Server.** Inline. `N'…'` when the literal then holds non-ASCII text, which is what fixed `東京` reading back as `??`. A negative number goes in parentheses. A value is spaced off a word it would run into. Brackets and nested comments are copied as they are.
- **DuckDB.** Inline. In a `$tag$` string the value's raw text goes in, with the tag check and its error message. `E'…'` takes backslash escapes. Same negative-number and spacing rules. On the shared scanner (fix 10): a `--` comment ends at `\r` too, and a `$tag$` follows the shared tag rules (any length; fix 19's characters). SQL Server moves onto the shared scanner with no change in output. The rules are in `scripts/sql-fixtures/params-model.ts` (`substituteInline`).
- **Numbers print as JS prints them** (`String(n)`): use `ryu-js`, so `1e21` is `1e+21`, `-0` is `0` and `0.1` stays `0.1`. Decimals use `plainDecimal`. Bigints print their digits.
- **Values** per decision 11. Grep the callers of `substituteParameters` and record in the effort log that none passes bytes, JSON or arrays. Fix 13's value rules (above) apply on every engine. `Value` can't tell a JS `bigint` from a `number` holding an integer, so a bigint within ±(2^53−1) binds as a number and a number past 2^53 as a bigint (same digits). A JSON integer beyond i64 decodes as `Decimal`, so it inlines as a quoted string, not digits. No caller passes either (`coerceValue` gives numbers).

**Tests first:** `params_parity.rs` over `params.json`, which includes all 31 `query-params.test.ts` cases and the phase 2 review cases. Also the Task 3 prefix test, run through `substitute`.

**Review:** phase 2's reviews of these substituters found `TOP {{n}}` broken by binding, substitution inside comments and bracket names, `1-{{p}}` with −1 written as `1--1`, and DuckDB's inliner missing `$tag$` and `E'…'`. The reviewer writes new cases in each of those families for every engine before reading the port, and checks every error message against the TS text.

**Checkpoint.**

### Task 5: `parse_create_table`

**Files:** `crates/seaquel-sql/src/create_table.rs`, `crates/seaquel-sql/tests/create_table_parity.rs`.

`parseCreateTableSql` backs the table editor's SQL pane, which users edit by hand, and it wasn't measured in the spike. Port it on the tokenizer, not sqlparser. sqlparser's `DataType` prints type names its own way (`INT`, `CHARACTER VARYING(100)`), and the table editor matches type names against each engine's column types. Also no `regex` crate. The signature stays engine-agnostic (`parse_create_table(sql: &str) -> Option<CreateTableDefinition>`). The TS accepts `"…"`, `` `…` `` and `[…]` names on every engine, and so does the port. The result is `seaquel_types::CreateTableDefinition`, the type the RPC already sends.

**Corpus** (Task 2's `create-table.json`):

- Every `ddl-create.json` output in the five engine crates' frozen fixtures (the DDL our generators produce, which is what the SQL pane shows first).
- The e2e `CREATE TABLE` statements.
- Hand-written cases: comments between columns, `CONSTRAINT` names, composite keys, `DEFAULT` with commas and parentheses, `ENUM('a,b')`, `DECIMAL(10,2)`, a name with `]]` or `""`, `CHECK`, trailing commas.

Record what the TS does, and don't assert a round trip. If recording shows the TS returns wrong data (a column dropped, a type cut short), that's a fix numbered from 15, with a case in `bugfixes.json`. Recording found two: fixes 16 and 17. `create-table.json` holds their expected output (from `scripts/sql-fixtures/create-table-fixed.ts`), with each changed case marked.

**Fixes:** 16 (`SERIAL` types, columns after comments, `CONSTRAINT … PRIMARY KEY/UNIQUE`, inline `REFERENCES`, `COLLATE` into `collation`) and 17 (doubled quotes in names, three-part names with DuckDB-quoted parts, index names with spaces). The rules are in `scripts/sql-fixtures/create-table-fixed.ts`. Everything else stays at parity, including the schema defaulting to `public` on every engine.

**Review:** give the pane's own DDL for each engine (from the frozen fixtures) to the port and compare field by field with the recorded TS.

**Checkpoint.**

### Task 6: AST helpers on sqlparser

**Files:**

- `crates/seaquel-sql/src/ast/*`, from `spike/sql-spike/src/{ast_util.rs,dialect.rs,tutorial.rs,visual.rs,column_sources.rs}`.
- `crates/seaquel-sql/tests/ast_parity.rs`.
- The new files in `src/lib/types/generated/`.

**API:**

```rust
pub fn parse_builder_query(sql: &str, engine: SqlEngine, tutorial_schema: &TutorialSchema,
                           valid_tables: Option<&[String]>) -> Option<ParsedQuery>;
pub fn parse_visual(sql: &str, engine: SqlEngine) -> Result<Option<ParsedQueryVisual>, String>;
pub fn column_refs(sql: &str, engine: SqlEngine) -> Option<Vec<Option<ColumnRef>>>;
pub fn parse_error(sql: &str, engine: SqlEngine) -> Option<String>;
```

**Steps:**

1. Move the spike's modules in, and keep its two mapper fixes: a decimal literal keeps its text, and aggregate names are upper-cased.
2. Settle the formatting differences toward the TS (decision 2). The spike's port differs from `visual.json` in more places than its six: `IN ((subquery))`, `EXISTS((subquery))`, `BETWEEN (1, 5)`, `!=` as written, and `schema`/`alias: null` on INSERT/UPDATE sources. Casts and placeholders follow fixes 15 and 6 instead: `CAST(x AS INTEGER)`, `x::DATE`, `$1`, `:name`.
3. `parse_builder_query` takes the engine (fix 9); the tutorial passes `postgres`. `TutorialSchema` (table → columns) comes from `src/lib/tutorial/schema.ts` through the wrapper, as in the spike, because the TS expands `t.*` from it.
4. `column_refs` returns what decision 9 describes; the test applies the TS lookup to compare with `column-sources.json`.
5. Types. Derive ts-rs on `ParsedQuery`, `ParsedTable`, `ParsedJoin`, `ParsedFilter`, `ParsedOrderBy`, `ParsedGroupBy`, `ParsedHaving`, `ParsedSelectAggregate`, `ParsedColumnAggregate`, `ParsedSubquery`, `ParsedCTE`, `ParsedQueryVisual` and its parts, and `ColumnRef`.
   - Optional fields use `#[ts(optional)]` where the TS has `?:`.
   - `FilterOperator`, `JoinType`, `AggregateFunction` and `HavingOperator` are Rust enums, so they generate the same string unions as `$lib/types`, not `string`.
   - `ParsedCte` generates as `ParsedCTE` (`#[ts(rename = "ParsedCTE")]`).
   - `limit: Option<serde_json::Number>` needs `#[ts(type = "number | null")]`: the workspace ts-rs has no serde-json feature.
   - Run `npm run types:gen`.
   - Don't switch any TS import yet (Task 10), but diff each generated type against today's interface.

**Tests first:**

- `ast_parity.rs`. Tutorial 91 of 91. Builder: 56 of 57 in PostgreSQL mode plus fix 9. Visual: all 172 with fixes 1–8 and 15. Column sources: all 172, the two DuckDB entries under fix 8.
- `acceptance.json`: 138 of 140 parse.
- The prefix test over the AST corpus, and a 5,000-deep `((((…` input: parse errors, never a panic or a stack overflow.

**Review:** read the generated TS types against `tutorial/sql-parser.ts` and `types/visualize.ts`, field by field (`null` vs absent matters to `statesEqual` in `sql-editor.svelte`). Check that no node-sql-parser quirk the builder relies on was lost: the filter-connector shift, first-table fallback for unqualified columns, and window `SUM()` counted as an aggregate.

**Checkpoint.**

---

## Part C — `seaquel-wasm`

### Task 7: Build and load the module everywhere

Starts after Task 1 and runs alongside Part B, using the `version()` stub.

**Files:**

- Create `scripts/build-wasm.mjs`, `src/lib/wasm/index.ts` and `src/lib/wasm/vitest-setup.ts`.
- Modify:
  - `package.json`: scripts; pin `binaryen` as a devDependency. It's only in `node_modules` today through `@bytecodealliance/jco-transpile`.
  - `.gitignore`, `.oxlintrc.json`, `.oxfmtrc.json`: `src/lib/wasm/pkg/`.
  - `vite.config.js`: `test.setupFiles`.
  - `src/routes/+layout.ts`, `src-tauri/tauri.conf.json`.
  - `Dockerfile`, `.dockerignore`.
  - `.github/workflows/ci.yml`, `.github/workflows/release.yml`.

**Steps:**

1. **`scripts/build-wasm.mjs`** is Node, not bash (the release builds on Windows too). It:
   - reads the wasm-bindgen version from `Cargo.lock` and compares it with `wasm-bindgen --version`. On a mismatch or a missing binary it prints the exact `cargo install wasm-bindgen-cli --version <v> --locked` line and exits 1. `--bindgen-version` just prints the version, for CI.
   - runs `cargo build --profile wasm-release --target wasm32-unknown-unknown -p seaquel-wasm`, respecting `CARGO_TARGET_DIR`.
   - runs `wasm-bindgen --target web --out-dir src/lib/wasm/pkg`.
   - runs `wasm-opt -Oz` with the spike's feature flags (`--enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext`).
   - skips the bindgen and opt steps when the raw `.wasm`'s hash matches a stamp in `pkg/`.
   - with `--opt-only`, runs only wasm-opt on an existing `pkg/` (Docker).
   - with `SEAQUEL_WASM_PREBUILT=1`, checks that `pkg/` exists and exits (Docker's Node stage).
   - prints the sizes, as the spike's `build.sh` did.

   Measure a cold build and a warm rebuild after a one-line change in `seaquel-sql`. If the warm rebuild takes more than about 20 s, add a `--fast` mode for the dev scripts (no LTO, no wasm-opt).

2. **npm scripts:**
   - `wasm:build`.
   - `predev`, `predev:web`, `predev:demo`, `prebuild`, `prebuild:web`, `prebuild:demo` and `pretest` run it. `tauri dev` and `tauri build` go through `npm run dev`/`npm run build`, so they get it too.
   - `check` becomes `npm run wasm:build && npm run types:gen && …`, since svelte-check needs `pkg/`'s `.d.ts`.
3. **`src/lib/wasm/index.ts`:**
   - `initSeaquelWasm(fetch)` calls `init({ module_or_path: fetch(wasmUrl) })`, with `wasmUrl` from `./pkg/seaquel_wasm_bg.wasm?url`. Using `load`'s `fetch` avoids the SvelteKit warning the spike saw.
   - It keeps the compiled `WebAssembly.Module`, and exports `wasm()`, which throws `seaquel-wasm used before init` if called too early.
   - A helper runs a call and, on a `WebAssembly.RuntimeError`, logs it, re-instantiates with `initSync({ module })` and rethrows. The next call works.
4. **`src/routes/+layout.ts`** keeps `prerender`/`ssr` and adds `export const load = async ({ fetch }) => { await initSeaquelWasm(fetch); return {}; };`. `(app)/+layout.ts`, `+layout.server.ts` and the theme editor's `+layout@.svelte` all sit under it.
5. **CSP (decision 7).** `script-src 'self' 'wasm-unsafe-eval'` in `src-tauri/tauri.conf.json`. Nothing else in the policy changes.
6. **vitest.** `vitest-setup.ts` calls `initSync({ module: readFileSync(<pkg>/seaquel_wasm_bg.wasm) })`. If the file is missing, it throws `run npm run wasm:build first`. It runs for every test file, which costs about 2 ms.
7. **CI, frontend job:**
   - `targets: wasm32-unknown-unknown`.
   - Install wasm-bindgen-cli at the version the script prints (`taiki-e/install-action` if it has that version, else `cargo install --locked`).
   - `npm run wasm:build` before `npm run check`.
   - Add `npm run build:demo`. It's the cheapest target that exercises the `?url` import under a base path.
8. **`release.yml`:**
   - Add `wasm32-unknown-unknown` to the toolchain step's `targets`.
   - Add a step that installs wasm-bindgen-cli with `cargo install --locked` at the pinned version. Prebuilt binaries don't cover the Windows ARM runner.
   - tauri-action's `beforeBuildCommand` (`npm run build`) runs `prebuild`.
9. **Dockerfile.** The Node stage has no Rust.
   - Add a `wasm-builder` stage `FROM chef`: `rustup target add wasm32-unknown-unknown`, install wasm-bindgen-cli at the pinned version, `cargo build --profile wasm-release --target wasm32-unknown-unknown -p seaquel-wasm`, `wasm-bindgen --target web`.
   - The Node stage copies that `pkg/` to `src/lib/wasm/pkg/`, runs `node scripts/build-wasm.mjs --opt-only` (one wasm-opt version everywhere: the npm one), then `SEAQUEL_WASM_PREBUILT=1 npm run build:web`.
   - `.dockerignore` allowlists `rust-toolchain.toml` and `scripts/build-wasm.mjs`.
10. **Website.** `demo:update` in `seaquel-app/main/packages/marketing` runs `npm run build:demo` in this repo, so `prebuild:demo` covers it and the website repo doesn't change. Check with `rm -rf src/lib/wasm/pkg && npm run build:demo` from a clean `pkg/`.
11. **Load it in every target.** Use the spike's `browser-check.mjs` from a scratch copy:
    - desktop static build with the new Tauri CSP applied, in Chromium and WebKit;
    - web (a running adapter-node server);
    - demo (`--base /demo`);
    - `npm run dev`.

    Then a **real Tauri window**, which the spike didn't try: `npm run tauri:dev`, and a `npm run tauri build -- --debug` bundle on macOS. Check that the `.wasm` loads with the CSP and that Tauri serves it as `application/wasm`; the spike only read that in Tauri's source. WebView2 and WebKitGTK get their first run in the release build (Task 13's manual checks).

**Tests first:** a vitest case that `version()` returns the crate version, which proves the setup file works; a Node script check that `initSeaquelWasm` resolves.

**Review:**

- `docker build .` completes and the container serves the `.wasm`.
- The frontend CI job passes on a branch.
- Every place that runs `vite build` gets the module: grep for `vite build` and `build:` in the workflows, the Dockerfile and `package.json`.
- The CSP change is exactly one token.

**Checkpoint.**

### Task 8: Exports, offsets and `src/lib/sql`

Needs Tasks 3–7.

**Files:**

- `crates/seaquel-wasm/src/{lib.rs,offsets.rs}`.
- `src/lib/sql/{index.ts,parameters.ts}`.
- `src/lib/sql/index.test.ts` and `src/lib/sql/offsets.test.ts`.
- Copies of `query-params.test.ts` and `sql-scan.test.ts` pointed at `$lib/sql`; the originals go in Task 9.
- `src/lib/types/generated/*`.

**Steps:**

1. `offsets.rs` from the spike: `utf16_to_byte`, `byte_to_utf16` and `location_to_utf16`. Offsets past the end clamp to the end, and an offset inside a surrogate pair rounds down.
2. **Exports**, all synchronous, JSON strings in and out, UTF-16 positions only:
   - `split_statements`, `statement_at` (with the trimmed text's range)
   - `extract_parameters`, `has_parameters`, `substitute_parameters` (`{sql, bindValues}` or `{error}`)
   - `query_type`, `destructive_reason`, `table_from_select`
   - `has_row_limit`, `count_query`
   - `read_only_error`
   - `parse_create_table`
   - `parse_builder_query`, `parse_visual`, `parse_error`, `column_refs`

   `parse_error` rewrites the `Line: L, Column: C` in sqlparser's message with a UTF-16 column, so the number matches the editor's column (sqlparser's columns count chars). The rewrite is one `seaquel-wasm` helper, applied to both `parse_visual`'s `Err` and `parse_error`, with a hand parser (no regex).

3. **`src/lib/sql/index.ts`** keeps today's names and signatures:
   - `splitSqlStatements`, `getStatementAtOffset` (the wrapper builds `sql` by slicing the input with the returned range).
   - `extractParameters`, `hasParameters`, `substituteParameters` (throws `ParameterSubstitutionError` on `{error}`).
   - `detectQueryType`, `isSelectQuery`, `isDestructiveStatement`, `findDestructiveStatements`, `extractTableFromSelect`.
   - `hasRowLimit`, `countQuery`, `validateReadOnlyQuery`, `parseCreateTableSql`.
   - `parseSql` (with an optional `engine`, defaulting to `postgres`), `parseQueryForVisualization`, `getParseError`, `resolveColumnSources` (decision 9's lookup).

   The functions that need the engine for fixes 11, 12 and 14 get it as a new argument. `parameters.ts` holds `createDefaultParameters`, `coerceValue` and the `ParameterSubstitutionError` class.

4. Values: `encodeParam` on the way in (a `Date` first becomes its ISO text), `decodeCell` on bind values coming out. `substituteParameters` throws every substitution error as a `ParameterSubstitutionError`, including a value `Value::from_wire` can't decode and a refused bytes, JSON or array value. `resolve-query.ts` shows only that class's message, so any other error class would surface as an unexplained failure.
5. **Ids from `parse_create_table`.** Rust gives placeholder ids (`column-N`, `index-N`, `fk-N`), unique only within one definition. `parseCreateTableSql` replaces every column, index and foreign-key id with `crypto.randomUUID()`, as the TS did: split panes can drag a column from one table editor into another, so ids must be globally unique. `parse_create_table` also returns `None` (so `null`) past its step budget on a pathological input, rather than freeze the SQL pane, which parses on every keystroke.

**Tests first:**

- Rust: the spike's offset tests, plus a round trip for every offset of `SELECT '東京' AS city;\nSELECT '😀' AS face;\nSELECT 3`.
- vitest, statement at cursor: for every UTF-16 offset of that string, `getStatementAtOffset` gives the statement a pure-JS reference gives. With the cursor at the start of statement 2 (UTF-16 21, byte 25) it's statement 2, not statement 1.
- vitest, lone surrogates: a lone `\uD83D` mid-statement, and at the end of the buffer. Ranges stay aligned, and the wrapper's `sql` still holds the lone surrogate, because it's sliced from the input.
- vitest, parameters: a bigint, a `SqlDecimal`, a `Date`, `NaN` and `null` round-trip through `substituteParameters`.
- The copied `query-params.test.ts` and `sql-scan.test.ts` pass against `$lib/sql` unchanged. A case changed by a bug fix is updated with a comment naming the fix.
- TODO (from the Task 7 review): a real trap test. Add a hidden test-only export (for example `__test_trap(kind)`, `#[doc(hidden)]`, behind a cargo feature the vitest build turns on, or at least never called by `src/lib/sql`) that panics and that overflows the stack, and a vitest case that calls it through `callWasm`, sees the throw (`RuntimeError`, `RangeError` for the overflow), and then gets a correct result from the next call. Task 7's tests only throw from JS.

**Review:** grep `crates/seaquel-wasm` for any `usize` returned or serialized without going through `offsets.rs`. Check that no export can panic on a malformed JSON argument: it returns `{error}` instead.

**Checkpoint.**

---

## Part D — Switch the call sites

### Task 9: Switch the editor and query runner

**Files:**

- `components/query-editor/{view-state.svelte.ts,execution.svelte.ts,explain-visualize.svelte.ts,param-dialog.svelte.ts}`
- `components/{save-query-dialog.svelte,standalone-query-editor.svelte,parameter-input-dialog.svelte,destructive-query-confirm-dialog.svelte}`
- `hooks/database/{query-execution.svelte.ts,resolve-query.ts,pending-changes.svelte.ts,query-crud.svelte.ts}`
- `services/query-file-parser.ts`, `services/ai/{context.ts,index.ts}`
- `types/{pending-changes.ts,query.ts}`
- Move: `db/pending-change-description.ts` → `hooks/database/pending-change-description.ts`.
- Delete:
  - `db/sql-parser.ts`, `db/query-params.ts`, `db/query-params.test.ts`, `db/query-utils.ts`
  - `engine/sql-scan.ts`, `engine/sql-scan.test.ts`
  - `db/column-sources.ts`
  - the `validateReadOnlyQuery` body in `services/ai/context.ts`

**Steps:**

1. Swap the imports to `$lib/sql`.
2. Pass the connection's type where fixes 11, 12 and 14 need it. The AI tool call needs the connection type added to its params.
   - Fix 11's new `DestructiveReason` values (`drop_sequence`, `drop_function`, `merge_delete`) need labels in `destructive-query-confirm-dialog.svelte`: new `messages/en.json` keys, translated with the `i18n-translator` agent.
3. `resolveColumnSources` moves with `query-execution.svelte.ts` (decision 9).
4. Nothing in `src/` may import the deleted paths. `standalone-query-editor.svelte`'s `parseSql` stays for Task 10.
5. **Caller changes** (from the Task 8 review; line numbers as of Task 8):
   - `components/query-editor/execution.svelte.ts:61`: `findDestructiveStatements(statements, dbType)`; `dbType` is in scope.
   - `execution.svelte.ts:86`: `isDestructiveStatement(currentStatement.sql, dbType)`; in scope.
   - `hooks/database/query-execution.svelte.ts`:
     - `:109` `resolveSourceTable(baseQuery)` has no connection. Add a `dbType` parameter; its callers `:262` (via `createStreamingSeed`) and `:491` need it.
     - `:249` `createStreamingSeed(...)` has no connection. Add `dbType` at its call sites `:620`, `:812` and `:988`, where `connection` is in scope.
     - `:261` `detectQueryType(baseQuery)` sits inside `createStreamingSeed` and gets fixed by the item above.
     - `:363` `detectQueryType(baseQuery, connection.type)`.
     - `:491` `extractTableFromSelect(baseQuery, connection.type)`.
     - `:595`, `:600`, `:614`: `isSelectQuery`/`detectQueryType(query, connection.type)`.
     - `:789`, `:794`, `:805`: pass `dbType` (in scope at `:751`).
     - `:984`: `isSelectQuery(existingResult.statementSql, connection.type)`.
     - `:145-149`: the private `resolveColumnSources` reads `state.activeConnection`, not the connection that is executing. That's pre-existing, but thread the executing connection's type through while you're in there.
   - `services/ai/index.ts:125`: `validateReadOnlyQuery(query, dbType)`. The params have only `connectionName`, so add the connection type to the tool params and pass it from the caller.
   - `components/destructive-query-confirm-dialog.svelte:14`: `Record<DestructiveReason, …>` needs `drop_sequence`, `drop_function` and `merge_delete`. The type check forces this; add the messages and translations (step 2).
   - Type-only import swaps: `types/pending-changes.ts:6`, `types/query.ts:6`, `hooks/database/pending-changes.svelte.ts:2`.
   - `parameter-input-dialog.svelte:20` (`coerceValue`) and `resolve-query.ts:3-4` (already has `dbType`) only need the import swap.
   - Delete the originals `db/query-params.test.ts` and `engine/sql-scan.test.ts`; the copies in `src/lib/sql` replace them.
   - What the wrapper does when the module fails (a trap, or a bad call): the functions called from `$derived`/`$effect` or per keystroke return the TS's "couldn't parse" answer and log (`splitSqlStatements` `[]`, `getStatementAtOffset` `null`, `extractParameters` `[]`, `hasParameters` `false`, `detectQueryType` `"other"`, `extractTableFromSelect` `null`); `validateReadOnlyQuery` fails closed (the refusal message); `substituteParameters` throws `ParameterSubstitutionError`; `isDestructiveStatement`, `findDestructiveStatements`, `hasRowLimit` and `countQuery` throw, so the runner reports the error instead of running unconfirmed or rewritten SQL.

**Tests first:** a vitest case per fix on the call path: the destructive check on `DELETE FROM t -- WHERE id = 1`, the source table for the `EXTRACT` query, and `validateReadOnlyQuery` on `SELECT 1; COPY …`. Also `src/lib/services/ai/context.test.ts` gains read-only cases; it has none today.

**Manual (at the checkpoint):**

- On desktop, a MySQL script with `#` comments and a backtick name holding `'` runs statement by statement.
- Run at cursor after `東京` and `😀`.
- `{{p}}` on Postgres inside a comment and a string.

**Checkpoint.**

### Task 10: Switch the Visual tab, query builder, tutorial and table editor; drop node-sql-parser

**Files:**

- `hooks/database/visualize-tabs.svelte.ts`, `types/visualize.ts` (re-export the generated types)
- `components/query-builder/sql-editor.svelte`, `components/query-editor/visual-query-panel.svelte`, `components/standalone-query-editor.svelte`
- `hooks/query-builder.svelte.ts` (an `engine` field, default `postgres`), `hooks/query-builder-parsed-sql.ts` (types)
- `components/create-table-view.svelte`, `package.json`, `package-lock.json`
- Create `src/lib/tutorial/criteria.test.ts`.
- Delete `tutorial/sql-parser.ts`, `db/sql-ast-parser.ts`, `db/parse-create-table.ts`.

**Steps:**

1. The Visual tab calls `parseVisualQuery` from `$lib/sql`, which gives `{ visual, parseError }` from one parse (`parseQueryForVisualization` followed by `getParseError` parses twice).
2. **Query builder dialect (fix 9).** `QueryBuilderState` gets `engine`:
   - `visual-query-panel.svelte` takes it as a prop from its parent, which knows the connection.
   - `standalone-query-editor.svelte` parses with `qb.engine`, which it leaves at the default, `postgres` (deviation, Task 10): it's the tutorial's editor, and its executor is the tutorial DuckDB, but decision 4 keeps tutorial SQL in PostgreSQL mode. `executor.dbType` still picks the `{{param}}` substitution.
   - The tutorial leaves the default, `postgres`.
   - `sql-editor.svelte` passes `qb.engine` to every `parseSql`.
3. The table editor calls `parseCreateTableSql` from `$lib/sql`.
4. Remove `node-sql-parser` from `package.json` and run `npm install`.
5. `criteria.test.ts` is the permanent TS-side parity check:
   - The 91 tutorial entries from `crates/seaquel-sql/tests/fixtures/tutorial.json` go through `parseSql` → `applyParsedSqlToState` → the lesson criteria, and every verdict must equal `criteria.json`.
   - The 57 round trips must regenerate the SQL in `builder.json`, except the fix 9 cases.

   This test stays after the recorder is deleted, because the criteria and the canvas hooks stay in TS.

6. **Caller changes** (from the Task 8 review; line numbers as of Task 8):
   - `components/query-builder/sql-editor.svelte:65, 66, 86`: `parseSql(..., { validTableNames, engine: qb.engine })`. Needs the new `QueryBuilderState.engine`.
   - `components/query-editor/visual-query-panel.svelte:40`: needs an `engine` prop from its parent. Also fix the `null` parse drop from the Follow-ups.
   - `components/standalone-query-editor.svelte:74`: `engine: qb.engine` (`postgres`, see step 2), not `executor.dbType`.
   - `hooks/database/visualize-tabs.svelte.ts:56-82`: `dbType` is already passed, so it's an import swap, plus the combined `parseVisualQuery` call (step 1).
   - `create-table-view.svelte:176`: import only. `hooks/query-builder*.ts`: type imports only.
   - On a module failure `parseSql`, `parseQueryForVisualization` and `parseCreateTableSql` return `null`, `getParseError` `"Unable to parse SQL query"`, `parseVisualQuery` both, and `resolveColumnSources` `undefined`, as the TS did for input it couldn't parse.

**Tests first:** `criteria.test.ts`, failing until the switch.

**Verify:**

- `grep -r node-sql-parser src package.json` is empty.
- The desktop build's JS shrinks by about node-sql-parser's 2.5 MB raw, while the `.wasm` adds 1,710,767 B raw (628,214 B gzip -9, measured at Task 8's review with everything exported; the spike's module was 1.56 MB and 561 KB gzip). Record both in the effort log.

**Manual:**

- The demo's Visual tab for the spike's `demo.products` query shows `p.price` and no LIMIT node.
- A MySQL builder with a backticked table round-trips.
- Three lessons pass in the tutorial, on desktop and in the demo.
- The table editor's SQL pane still round-trips a MySQL and a SQL Server table.

**Checkpoint.**

---

## Part E — Close-out

### Task 11: CI

- The rust job already runs `cargo test --workspace`, so it runs `seaquel-sql`'s parity tests. The wasm32 clippy line has both new crates (Task 1).
- The frontend job builds the module, type-checks and runs vitest with it, and builds the demo target (Task 7).
- The generated-types staleness check covers the new types without changes.
- **Manual:** the first CI run on GitHub, and a release dry run (`workflow_dispatch` on a branch, if the owner agrees) to see wasm-bindgen-cli install on all runners.

**Checkpoint.**

### Task 12: Docs and cleanup

- **CLAUDE.md:**
  - Architecture: `seaquel-sql` and `seaquel-wasm`.
  - A rule next to "UI code never does dialect work itself": UI code never scans or parses SQL; it uses `$lib/sql`.
  - The toolchain: the wasm32 target, wasm-bindgen-cli at the `Cargo.lock` version, `npm run wasm:build`.
  - The Tauri CSP token and why it's there.
  - "Updating the Demo": the machine needs the wasm toolchain.
- **Design doc:** phase 2b status at the top. The `seaquel-sql` row of the crate table says what moved and what stayed (`criteria.ts`, the builder hooks, `buildSql`). Lines 25–26, 111 and 113 still name TS files phase 2b deleted (`parse-create-table.ts`, `column-sources.ts`, `db/sql-parser.ts`, `db/query-utils.ts`, `db/sql-ast-parser.ts`, `tutorial/sql-parser.ts`, …); say where each went.
- **This plan:** execution notes, in phase 2's format.
- **Recorder:** copy `scripts/sql-fixtures/*` to `docs/plans/artifacts/2026-09-27-sql-recorder-*.txt`, delete `scripts/sql-fixtures/`, and point `crates/seaquel-sql/tests/fixtures/README.md` at the copies.
- **Spike:** copy `spike/wasm-stub/browser-check.mjs` and `demo-editor-check.mjs` to `docs/plans/artifacts/` as `.mjs.txt`. They're how the manual load checks were run. Delete `spike/`, and remove the `spike` entry from `.oxlintrc.json` `ignorePatterns`. The report stays in `docs/plans/`.
- A line for the release notes about fix 10: MySQL and SQL Server scripts split correctly now, so a script that ran as one statement may run as several.

**Checkpoint.**

### Task 13: Measure

- **Phase 2b cost** in the design doc:
  - time per task vs the ~18–29 h estimate, and vs the design doc's 27–45 h;
  - Rust lines added and TS lines removed;
  - bugs found, by source (spike, plan research, recording, review);
  - the final module's size (raw, gzip, brotli) and init time in Chromium and WebKit, measured again with the spike's scripts;
  - what was harder than expected.
- Whether the Follow-ups change the plan for phase 3.

Final verification: the full check list, plus the manual checks below, listed for the user.

**Manual checks** (desktop is `npm run tauri:dev` and a `tauri build` bundle; web is `npm run dev:web:full`; demo is `npm run dev:demo`):

- **Every target:** the app loads with no WASM or CSP error in the console, and the Visual tab and the builder work.
- **Editor:**
  - Run at cursor with `東京` and `😀` in earlier statements.
  - Run all on a MySQL script with `#` comments and a SQL Server script with `[bracket;names]`.
  - EXPLAIN of the statement at the cursor.
  - The statement count while typing an unterminated string.
- **Parameters:** `{{p}}` on each engine, including MSSQL `東京`, and Postgres `{{p}}` inside a comment.
- **Destructive check:** `DELETE FROM t -- WHERE id = 1` asks for confirmation.
- **Inline editing:** a result from `SELECT EXTRACT(YEAR FROM created_at), id FROM orders` is editable against `orders`.
- **Visual tab:** no `[object Object]` and no `NaN`. MySQL `LIMIT 5, 10`. SQL Server `TOP 10`. DuckDB `GROUP BY ALL`.
- **Query builder:** a MySQL backticked table and a SQL Server bracketed one round-trip. The tutorial, three lessons, on desktop and in the demo.
- **Table editor:** edit the SQL pane on MySQL and SQL Server.
- **AI with "allow all queries" on:** `SELECT 1; COPY …` is refused.
- **Release build:** on Windows (WebView2) and Linux (WebKitGTK), the app loads. These are the first runs there.

**Status (Task 13):** done. The "Phase 2b cost" section of the design doc has the measured time (~14.9 h logged for Tasks 1–13, ~16.2 h with the spike, against ~18–29 h here and 27–45 h in the design doc; review fixes ~6.5 h of it, over their 3.5–5.5 h), lines, bugs by source, the final module (1.71 MB, 634 KB gzip, 484 KB brotli; init 4–7 ms in Chromium and 21–42 ms in WebKit on desktop, web and demo) and the recommendation to do the two high-priority AI Follow-ups before phase 3. The full check list passed, apart from one run of `cargo test` where two Postgres smoke tests raced each other (`unique_checkbox_runs` drops every other `seaquel_uq_*` schema while the other two tests use theirs; older than phase 2b, passed on the rerun and on 8 of 8 repeats). The manual checks below are not run yet.

---

## Execution notes (2026-09-27)

The plan was executed task by task, with a review after each and usually a round of review fixes. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-09-27-phase-2b-effort.md`. The measured cost is Task 13's and goes in the design doc.

**What went differently from the plan**

- **Parity fixtures caught no port bugs**, as in phases 1 and 2. Every Rust port and the TS wrapper matched every recorded case on the first run, apart from the planned fixes and a few pinned exceptions: four non-tutorial entries sqlparser's PostgreSQL mode parses differently (`ast_parity.rs` names them), and `Value` limits such as a JS bigint within 2^53 binding as a number. The bugs came from recording, the models and the reviews: the reviewers' adversarial and randomized inputs found the word-rule, `--`, dollar-tag and value breakout problems, and two quadratic blowups.
- **More models than planned.** Task 2 planned models for fixes 1–4 and 10 only. Fixes 11–14 changed hundreds of per-engine scanner cases, so they got models too (`scan-model.ts`, `statements-model.ts`, `params-model.ts`), as did fixes 16 and 17 (`create-table-fixed.ts`). The recorder ran each model with one fix or rule left out, so every changed case in `bugfixes.json` names the fix behind it. The fixtures were re-recorded after the Task 2, 3 and 4 reviews; recording twice gave byte-identical files.
- **`parse_create_table` isn't on the tokenizer** (Task 5). The TS was a dozen regexes over three small scanners of its own, and its output depends on regex details (where a lazy `DEFAULT (.+?)` stops, keywords matched without word boundaries). The port keeps those scanners and runs each regex through a small backtracking matcher with JavaScript's semantics. Replacing it with a real parser is a Follow-up.
- **Recovering from a trap needed a glue patch** (Task 7). wasm-bindgen 0.2.128's `initSync` returns early once a module is set, and `--experimental-reset-state-function` calls a `__wbindgen_start` this crate doesn't export. See the decisions below.
- **A failed init shows a page, not SvelteKit's 500** (Task 7 review): the error, a copy button and "Try again". `/login`, `/signup` and `/airgap-setup` render without the module.
- **`db/query-params.ts` and `db/column-sources.ts` went in Task 10**, not Task 9: the standalone editor still imported the first. The AI tool's `databaseType` became required, and the tool refuses every query when it's missing.
- **The standalone editor parses in PostgreSQL mode** (Task 10), not in its executor's engine. Its only executor is the tutorial's DuckDB, and decision 4 keeps tutorial SQL in PostgreSQL mode. `executor.dbType` still picks the `{{param}}` substitution.
- **A missing wasm toolchain keeps an existing `pkg/`.** `npm run wasm:build` warns and exits 0 when cargo, the wasm32 target or the right wasm-bindgen is missing but a finished `src/lib/wasm/pkg/` exists, so a local check run can pass on a stale module (Task 10 hit this). CI, the release runners and the Docker image can't: each starts from a checkout without `pkg/` (it's gitignored, `.dockerignore` excludes it, and rust-cache saves only `target/` and `~/.cargo`), so a missing toolchain fails the build there.
- **The recorder and the spike are gone** (Task 12). `scripts/sql-fixtures/` is kept as `artifacts/2026-09-27-sql-recorder-*.txt`, and the spike's two Playwright load checks as `artifacts/2026-09-27-spike-browser-check.mjs.txt` and `artifacts/2026-09-27-spike-demo-editor-check.mjs.txt`. The `seaquel-sql` fixtures are frozen. `src/lib/sql/parity.test.ts` and `src/lib/tutorial/criteria.test.ts` check them from the TS side.

**Bug fixes per area.** Numbered as in the list above and `crates/seaquel-sql/tests/fixtures/bugfixes.json`:

- **Visual tab, fixes 1–8 and 15.** 1–4 (`[object Object]`, `LIMIT NaN`, function calls, DISTINCT) and 5, 7 and 8 (MySQL `LIMIT 5, 10`, `TOP`, DuckDB syntax) came from the spike, as did `$$…$$` printing as `""` in 6. Found while recording: a query calling any non-aggregate function got no visualization at all (3), placeholders `$1`, `@v` and `:name` (6), and casts printing as `unknown` (15).
- **Query builder, fix 9** (dialect): the spike.
- **Scanner, fixes 10–14, 18 and 19.** 10 (splitting ignores the engine) came from the spike. 11–14 came from plan research. Their extensions came from reviews: the Task 2 review added executable comments, `--` ending at `\r`, the destructive-check cases (EXPLAIN ANALYZE, `DROP c` without `COLUMN`, data-modifying CTEs, `MERGE … THEN DELETE`, SQL Server without `;`) and the read-only bypasses confirmed live on MySQL, MariaDB and SQL Server; the Task 3 and 4 reviews added MySQL's `--` rule, dollar tags of any length and the fix 13 value rules. 18 (number tokens) came from the Task 2 review and 19 (word characters) from the Task 3 review.
- **Table editor, fixes 16 and 17:** found while recording, on the DDL our own generators write.

Outside the numbered list:

- AI dashboard widgets (`add_widget`, `update_widget`) ran the model's query with no read-only check (Tasks 9/10 review).
- `visual-query-panel.svelte` dropped the initial SQL when it didn't parse (fixed in Task 10).
- The runner resolved column sources against the active connection, not the one executing the query (Task 9).
- Found in our own new code before it shipped: `destructive_reason` was quadratic (1.2 s on 690 KB, now 7 ms), the CREATE TABLE matcher was quadratic on long whitespace (62 s on 50k spaces), and the first fallback contract failed open on the run path (below).

**Decisions made during execution**

- **The fallback contract** (Task 8 review). When the module fails, trap or bad call, the wrapper answers as the TS did for input it couldn't parse. Functions called per keystroke or from `$derived` return their empty value and log: `splitSqlStatements` `[]`, `getStatementAtOffset` `null`, `extractParameters` `[]`, `hasParameters` `false`, `detectQueryType` `"other"`, `extractTableFromSelect` `null`, `parseSql` and `parseCreateTableSql` `null`. `validateReadOnlyQuery` fails closed with its refusal. `substituteParameters` throws `ParameterSubstitutionError`. `isDestructiveStatement`, `findDestructiveStatements`, `hasRowLimit` and `countQuery` throw, so the runner reports the error instead of running unconfirmed or rewritten SQL.
- **Strict variants on the run path** (Tasks 9/10 review). The lenient answers failed open: a trapped `statement_at` made Run Current and EXPLAIN ANALYZE skip the destructive check and run the whole buffer. Anything that decides what SQL runs uses `splitSqlStatementsOrThrow`, `getStatementAtOffsetOrThrow`, `detectQueryTypeOrThrow` and `resolveQueryOrThrow`. Tests trap single exports through a `callWasm` proxy.
- **The CREATE TABLE matcher has a step budget** (Task 5 review): 2M steps plus 100 per character (ordinary DDL uses under 40). Past it `parse_create_table` returns `None`, so the SQL pane shows nothing parsed rather than freezing. `\s+` is possessive and each class caches its current run, which made the known blowups linear.
- **Operator chains are capped at a nesting depth of 2,000** (Task 6 and its review). sqlparser's recursion limit doesn't cover left-deep chains (`a AND b AND …`, `+`, `::`, `UNION`), and 20,000 links overflowed its own stack. `ast::util::parse` counts the depth from the tokens first and refuses more as a parse error. The cap started at 1,000; once the printer walked casts, IS, IN, BETWEEN, LIKE and `AT TIME ZONE` iteratively, the first native overflow on a 1 MB stack was a WHERE `OR` chain at a depth of ~10,160 by the guard's count, so 2,000 leaves a 5× margin and still accepts a 1,000-term `id = n OR …`.
- **The module has a 2 MB stack** (Task 7). `crates/seaquel-wasm/build.rs` passes `-zstack-size=2097152` for wasm32 only, so no other crate's build changes and Docker needs no `.cargo/config.toml`. `build-wasm.mjs` reads `__stack_pointer`'s initial value and fails unless it's 2 MiB. sqlparser's own 50-level recursion traps at 512 KB, which left less than a 2× margin on the old 1 MB default.
- **Trap recovery patch** (Task 7). `build-wasm.mjs` appends a five-line `__seaquel_reinstantiate()` to the wasm-bindgen glue, which instantiates the already-compiled `WebAssembly.Module` again. The script fails if the glue's shape changes. `callWasm` re-instantiates after any throw, not just a `RuntimeError`: a stack overflow in V8 is a `RangeError`, and a frame past the shadow stack is `RuntimeError: memory access out of bounds`. A vitest case traps a debug build with a `test-trap` feature; it's skipped without the toolchain locally and fails in CI.
- **AI dashboard widgets get the read-only check** (Tasks 9/10 review). `add_widget` and `update_widget` refuse a query that `readOnlyError` refuses, as `run_query` does, with "allow all queries" on or off, and fail closed without a connection type. They still don't ask for approval.
- **The standalone editor's dialect** is `qb.engine`, which it leaves at `postgres` (see above).
- **Fix 13's value rules** (Task 4 reviews). `tag-edge`: a `$tag$` string must still end where it did once its values are in, which refuses `$$Cost: ${{p}}$$` with `$ || 'INJ' --` (a live breakout), the tag's other edge, partial tags and unterminated strings. `escaped`: in a string that takes backslash escapes, `{{` after an odd run of `\` is text, not a parameter. `adjacent`: an inlined quoted value right after `'`, `"` or `` ` `` gets a space before it, and a value ending in a word character gets one before a following word or `$`, so `E'a'{{p}}` can't merge with the literal before it (it executed on the DuckDB CLI). A bound `$n` isn't spaced (Follow-ups).

**Release notes**

- Fix 10: MySQL and SQL Server scripts now split where the server would split them. `#` comments, backtick and bracket names, and backslash escapes no longer hide or invent a `;`, so a script that used to run as one statement may now run as several.

**Manual checks.** Not run yet; for the user, from Task 13's list (desktop is `npm run tauri:dev` and a `tauri build` bundle, web is `npm run dev:web:full`, demo is `npm run dev:demo`):

- Every target: the app loads with no WASM or CSP error in the console, and the Visual tab and the builder work.
- Editor: run at cursor with `東京` and `😀` in earlier statements; Run all on a MySQL script with `#` comments and a SQL Server script with `[bracket;names]`; EXPLAIN of the statement at the cursor; the statement count while typing an unterminated string.
- Parameters: `{{p}}` on each engine, including MSSQL `東京`, and Postgres `{{p}}` inside a comment.
- Destructive check: `DELETE FROM t -- WHERE id = 1` asks for confirmation.
- Inline editing: a result from `SELECT EXTRACT(YEAR FROM created_at), id FROM orders` is editable against `orders`.
- Visual tab: no `[object Object]` and no `NaN`; MySQL `LIMIT 5, 10`; SQL Server `TOP 10`; DuckDB `GROUP BY ALL`.
- Query builder: a MySQL backticked table and a SQL Server bracketed one round-trip; the tutorial, three lessons, on desktop and in the demo.
- Table editor: edit the SQL pane on MySQL and SQL Server.
- AI with "allow all queries" on: `SELECT 1; COPY …` is refused.
- Release build: the app loads on Windows (WebView2) and Linux (WebKitGTK). These are the first runs there.
- CI (Task 11): the first run of `ci.yml` on GitHub, and a release dry run (`workflow_dispatch` on a branch) to see wasm-bindgen-cli install on every runner, Windows ARM from source.

---

## Follow-ups (not in phase 2b)

- **High priority: enforce read-only in the database when the AI's "allow all queries" is on.** Fix 14 gates which statements run; it isn't a sandbox, and user-defined functions can't be covered. Run the AI's queries in a read-only transaction or session: Postgres `BEGIN READ ONLY`, MySQL/MariaDB `START TRANSACTION READ ONLY`, SQLite `PRAGMA query_only`, DuckDB a read-only connection (SQL Server has no equivalent; a read-only login). None of these stops network egress (DuckDB `read_csv('https://…')` through httpfs); that needs httpfs off for the AI's connection, or a network policy.

- **High priority: the AI read-only check and the query run on different connections.** The AI tool checks a query against the connection that was active when the message was sent (`params.databaseType`), but `executeRawQuery` runs it on whichever connection is active when the tool call executes. Switching connections in between checks a query under one engine's rules and runs it on another. Bind the tool call to the connection id it was checked against, and refuse if that connection is gone. (Found in Task 9; older than phase 2b.)
- **Fixed in 2b (Tasks 9/10 review): AI dashboard widgets skipped the read-only check.** `add_widget` and `update_widget` stored the model's `query`, and the dashboard ran it at once, with no check and no approval. Both now refuse a query that `readOnlyError(query, databaseType)` refuses, as `run_query` does, with "allow all queries" on or off, and fail closed without a connection type. They still don't ask for approval; with the first item above, the widget runs would need the same read-only session. (Older than phase 2b.)
- **Native stack for the AST helpers.** sqlparser itself overflows a 1 MB native stack on nested FROM subqueries from about depth 25, before its own recursion limit (50) trips. In wasm the module's 2 MB stack covers it with more than 2x margin. If `seaquel-sql`'s AST helpers ever run natively (Core, the CLI, the MCP server), run them on a thread with at least 4 MB of stack, or use sqlparser's `recursive-protection` feature.

**Tutorial and query builder**

- The tutorial parser's quirks, kept for parity:
  - IN, IS NULL, IS NOT NULL and BETWEEN filters are dropped;
  - the AND/OR connector shifts by one filter;
  - HAVING loses its table and FROM subqueries their alias;
  - `products.*` comes back as the column list;
  - `ORDER BY 2`, `GROUP BY 1`, `JOIN … USING`, a compound `ON` and `HAVING` on an alias aren't understood.

  Fixing them changes lesson verdicts, so it's a change with its own review against `criteria.test.ts`. Normalising `50.0` to `50` would pass three criteria the TS fails today.

- `buildSql` to Rust together with identifier quoting. The builder leaves column references, aliases and filter values unquoted (phase 2 follow-up).
- `{{min_price}}` in builder SQL doesn't parse in either parser.
- A "lossy parse" signal: `parse_builder_query` could report which clauses it dropped (`QUALIFY`, `GROUP BY ALL`, `OFFSET … FETCH`, IN lists, …), so the builder warns before a canvas edit overwrites `customSql` with SQL that lacks them.
- ~~`visual-query-panel.svelte` (the `onMount` at lines 38–47) drops the initial SQL when `parseSql` returns `null`: the `catch` that sets `customSql` only runs on a throw, and `parseSql` never throws. An existing bug; cheap to fix in Task 10 (set `customSql` when the parse is `null`).~~ Fixed in Task 10: `customSql` is set whether or not the SQL parses.

**Visual tab**

- The filter tree repeats each subtree's text at every level (`visual.rs`, `Printer::filter`), so its JSON grows quadratically with the length of an AND/OR chain: about 12 KB of top-level text but about 6 MB in all for a 1,000-term `id = n OR …` chain. That's the TS's shape (parity).
- `QuerySource.schema` and `alias` are typed `?: string` (as in `types/visualize.ts`), but INSERT and UPDATE sources send `null`. The nodes only test them for truthiness; the type should say `string | null`.

**Scanner**

- MySQL `DELIMITER`, `BEGIN … END` bodies and SQL Server's `GO` aren't handled by the splitter.
- On Postgres the bound `$n` placeholder isn't spaced off a following `$`, word or preceding word (`$1$t$`, `x$1`): the server reads trailing junk and the query fails with a syntax error. The scanner reads it differently from the server, but it doesn't break a value out. Spacing it like an inlined value would fix it and change the TS output.
- `extractParameters` lists a `{{p}}` inside a comment, or an escaped `\{{p}}` in a string that takes backslash escapes (fix 13), so the parameter dialog can ask for a value no engine uses. Skipping them needs the engine, which not every caller has (`save-query-dialog.svelte`).
- `WITH … SELECT` counts as "other" in `detectQueryType`, as in the TS.
- A pending change's label shows truncated SQL on MySQL and SQL Server, because `describePendingChange` only knows `"…"` names.
- `/*! … */` counts as a comment for `hasRowLimit`, though MySQL runs it (the splitter and the statement checks read it as code since the Task 2 review).
- The MSSQL crate's three hand scanners (`mentions_use`, `must_start_batch`, `paginate`) can move onto `seaquel_sql::scan`. Engines may depend on `seaquel-sql` already.

**Table editor**

- Replace `parse_create_table`'s regex matcher with a direct parser once the TS is gone and its semantics are ours to change. The matcher reproduces the TS regexes exactly, quirks included: `NOT NULL` inside a string default (`DEFAULT 'NOT NULL'`) makes the column non-nullable, `DEFAULT` and `PRIMARY KEY` match inside longer words, a default that spans lines is dropped, and `GENERATED … AS (…)` is dropped. A parser on the tokenizer would fix these and wouldn't need the step budget.

**`seaquel-wasm`**

- **Engine dialects in the module.** The design puts identifier quoting and pagination in `seaquel-wasm`. Today `rust-engine-client.ts` keeps local mirrors (`PAGINATE`, `qualified-table.ts`, `reserved-words.ts`), checked against the Rust fixtures. Moving them needs each engine crate's driver behind a Cargo feature, so the dialect builds for wasm32 without sqlx, tiberius or bundled DuckDB. That's the same split phase 8 needs for DuckDB.
- Monaco completion ranking still runs in `monaco-sql-languages`.
- sqlparser's DuckDB dialect doesn't nest block comments, so the Visual tab reports a parse error for such a query.
- `PRAGMA table_info(x)` and a column typed `UNSIGNED BIG INT` don't parse (they don't in node-sql-parser either).

**Build**

- adapter-node runs with `precompress: false`, so the web build sends the 1.56 MB `.wasm` uncompressed unless a proxy compresses it. Precompress, or compress in `server.js`.
- Repeat-visit code caching, memory use and slow machines weren't measured.

**Tests**

- `crates/seaquel-engine-postgres/tests/smoke.rs`: `unique_checkbox_runs` starts by dropping every `seaquel_uq_*` schema but its own, while `unique_checkbox_from_metadata` and `unique_checkbox_skips_partial_and_include_indexes` run in parallel in the same binary and use theirs. It failed once in Task 13's check run (`schema … does not exist`). Older than phase 2b. **Fixed after Task 13:** the other two tests now use their own prefixes (`seaquel_uqm_`, `seaquel_uqp_`), which `seaquel\_uq\_%` doesn't match; the smoke binary passed 5 runs in a row.
