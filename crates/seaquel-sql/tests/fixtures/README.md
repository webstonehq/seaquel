# seaquel-sql parity fixtures

> **Frozen.** These files pin what the TypeScript SQL code did (with the
> numbered bug fixes applied) before `seaquel-sql` replaced it in phase 2b. The
> TypeScript and the recorder are gone, so the fixtures can't be recorded
> again. Change one only when the Rust behaviour is meant to change: say which
> fix, and why, in `bugfixes.json`. Numbering and reasons: "Bug fixes" in
> `docs/plans/2026-09-27-rust-core-phase-2b-plan.md`.

Who reads them:

- `cargo test -p seaquel-sql`: `scan_parity.rs`, `params_parity.rs`,
  `create_table_parity.rs` and `ast_parity.rs`.
- `npx vitest run`: `src/lib/sql/parity.test.ts` runs every file through the
  TypeScript wrapper and the built module (UTF-16 offsets included), and
  `src/lib/tutorial/criteria.test.ts` checks `criteria.json` and
  `builder.json` against the lesson criteria and `buildSql`, which stay in
  TypeScript.

## The recorder

`scripts/sql-fixtures/sql.fixtures.test.ts` ran the TS, and the bug-fix models
next to it, and wrote these files (`RECORD_FIXTURES=1 npx vitest run
scripts/sql-fixtures`). Record mode also ran the phase 2b spike's
`accept-cli` through cargo for `acceptance.json`. Recording twice gave
byte-identical files.

It imported the real TS (`parseSql`, `parseQueryForVisualization`,
`getParseError`, `resolveColumnSources`, `splitSqlStatements`,
`getStatementAtOffset`, `hasRowLimit`, `countQuery`, `detectQueryType`,
`isDestructiveStatement`, `extractTableFromSelect`, `validateReadOnlyQuery`,
`extractParameters`, `hasParameters`, `substituteParameters`,
`parseCreateTableSql`, `applyParsedSqlToState`, `buildSql` and the lesson
criteria), so it was deleted with that TS in phase 2b (Task 12). A copy of
every file in `scripts/sql-fixtures/` is kept as
`docs/plans/artifacts/2026-09-27-sql-recorder-<file>.txt`, for example
`2026-09-27-sql-recorder-params-model.ts.txt`. The file names below are the
recorder's.

## Corpora

- **AST corpus** (`corpus.ts`, from the spike): 172
  entries. 91 tutorial (55 solutions, 8 hint queries, 28 variants), 57 query
  builder round trips (37 builder states through `buildSql`, and 4 of them
  with each engine's qualified table names), 24 editor statements in each
  engine's dialect.
- **Scanner corpus** (`scanner-corpus.ts`): 392 inputs,
  each run for all six engine ids. The spike's 13 splitter inputs (15 cases
  there), every input in `src/lib/engine/sql-scan.test.ts` and
  `src/lib/db/query-params.test.ts` (with the values each test passes; both
  now live on as copies in `src/lib/sql/`), the
  cases phase 2's reviews added to `query-params.ts`, the six e2e
  `schema.sql` files whole, the 18 sample queries, and per-engine edge cases:
  `#` comments and `#temp` names, `/*! … */`, `E'…'`, `$1` vs `$tag$`,
  nested comments, `]]`, quotes in backtick and bracket names, one input
  ending inside each kind of unterminated string, name and comment, only
  comments, empty input, `;;`, CRLF, `東京` and `😀`, `GO`, `BEGIN … END`,
  `DELIMITER`, MySQL executable comments (`exec:…`), and the plan's examples for fixes 10–14 and 18 (ids `fix10:…` to
  `fix14:…`). An input that repeats an earlier one's SQL (with no values of
  its own) is left out.
- **CREATE TABLE corpus** (`create-table-corpus.ts`):
  124 statements. Every `ddl-create.json` output in the engine crates' frozen
  fixtures and the `ddl-create` outputs in their `bugfixes.json` (the DDL the
  table editor's SQL pane shows first), the e2e schema files' CREATE TABLE
  statements, and hand-written cases.
- **Acceptance** (`acceptance.ts`): the spike's 140
  statements from the repo (18 sample queries, 118 statements of the e2e
  schema files split with the TS splitter, 4 demo dashboard queries).

No input holds a lone surrogate (serde_json refuses them; Task 8 tests them at
the wasm boundary). Parameter values hold no `Date` (the wrapper turns one
into its ISO text first, decision 11, and every substituter already treated a
`Date` as that text) and no bytes, JSON or arrays (rejected under decision 11).

## Files

Every file is `{ "cases": [{ "name", "input", "output" }] }`, plus shared
inputs at the top level where noted. `null` stands for the TS's `null` and
`undefined`; a field that was `undefined` is absent.

| File | Cases | Records |
| --- | --- | --- |
| `tutorial.json` | 172 | `parseSql` (PostgreSQL mode) for every AST entry. `input.validTables` is `"tutorial"` (the tutorial's table list, top-level `tutorialTables`) or `null` (every table). Top-level `tutorialSchema` is table → columns, which `t.*` expands from |
| `builder.json` | 57 | The round trips: `input.engine` (the connection's), `input.tableRef` (the table-name resolver that generated the SQL); `output.parsed` is `parseSql` in PostgreSQL mode, as today, and `output.sql` what `applyParsedSqlToState` + `buildSql` regenerate from it |
| `criteria.json` | 91 | Every lesson criterion's verdict for the tutorial entries (`criterionId → bool`), from `parseSql` → `applyParsedSqlToState` → `check`; a `null` parse checks an empty builder state |
| `visual.json` | 172 | `parseQueryForVisualization` in the entry's engine, from the fixes 1–4, 6 and 15 model |
| `parse-error.json` | 172 | Whether `getParseError` gives a message (`true`) or `null` (`false`). Messages differ between the parsers and aren't compared |
| `column-sources.json` | 172 | `resolveColumnSources` against top-level `schemas` (every tutorial table in `public`, `Sales`, `shop`, `dbo` and `main`) |
| `split.json` | 392 | `splitSqlStatements`, per engine: `output.<engine>` is the statements (`sql`, `index`, `startOffset`, `endOffset`). Offsets are UTF-16 code units, as the TS's are; the last statement's `endOffset` is the input's UTF-16 length minus one, also when it's unterminated. From the fix 10 model (and 18 where number tokens change it) |
| `statement-at.json` | 392 | `getStatementAtOffset` for every UTF-16 offset `0..=length`, per engine, as runs `[from, to, index]` (inclusive; `index` is the statement's in `split.json`, `null` when there is none). UTF-16 offsets: an offset inside a surrogate pair gives the same answer as the pair's other offset, so the Rust side, which works in bytes, answers the same for every byte of that char. From the fix 10 model |
| `row-limit.json` | 392 | `hasRowLimit`, per engine, from the scanner model (fixes 18 and 10; a MySQL executable comment is still a comment here, a Follow-up) |
| `count-query.json` | 386 | `countQuery`, per engine (not the six whole schema files), from the scanner model |
| `statements.json` | 392 | Per engine: `queryType` (`detectQueryType`), `destructive` (`isDestructiveStatement`), `table` (`extractTableFromSelect`, `{ schema?, table }`). From the fixes 11 and 12 models |
| `read-only.json` | 392 | `validateReadOnlyQuery`, per engine: the message or `null`. From the fix 14 model |
| `params.json` | 638 | `input.kind` `"extract"` (392): `extractParameters` and `hasParameters`, engine-agnostic. `"substitute"` (246): `substituteParameters` for every input with parameters, per value set and `forceInline`, per engine; `output.<engine>` is `{ sql, bindValues }` or `{ error }` (the `ParameterSubstitutionError` message). Values in and bind values out are in the Value wire format (`encodeParam`). From the fix 13 model |
| `create-table.json` | 124 | `parseCreateTableSql`, without the `id`s (the TS makes random UUIDs; compare without them). From the fixes 16 and 17 model |
| `acceptance.json` | 140 | `output.parses`: whether sqlparser (the spike's port, the engine's dialect) parses the statement. Not a TS output: it guards against a sqlparser upgrade that stops parsing SQL we ship. 138 parse; `PRAGMA table_info(your_table)` and the SQLite all-types table (`UNSIGNED BIG INT`) don't |
| `bugfixes.json` | | See below |

## Bug fixes

Where a model decides the output, the fixture already holds the fixed output,
and the case says which fixes changed it. `fixes` has two shapes: an array
(`"fixes": [1, 2]`) in the AST and CREATE TABLE files, and a per-engine map
in the scanner files (`"fixes": { "mysql": [10] }`), whose `output` is per
engine too. On the Rust side that's an untagged enum. The models (recorder
files, see above):

- `visual-fixed.ts`: a copy of `sql-ast-parser.ts` with fixes 1–4, 6 and 15,
  each edit tagged `FIX n`. The recorder fails if the TS differs from the
  model with 6 and 15 off in a way the spike's four patterns (`visualCategory`
  in the spike's harness, copied into the recorder) don't explain; it attributes fixes 6 and
  15 by running the model with each left out.
- `scan-model.ts`: `sqlTokens` with fix 18 (a number token ends where the
  numeric literal ends), fix 19 (word characters per engine), fix 10's comment
  and tag rules (a Postgres/DuckDB `--` comment ends at `\r`; a MySQL/MariaDB
  `--` needs ASCII space or a control character after it; a `$tag$` of any
  length), and, for the split, statement checks, read-only check and
  substitution, a MySQL/MariaDB executable comment (`/*! … */`; `/*M! … */` on
  MariaDB) read as code. Also `hasRowLimit` and `countQuery` over those
  tokens. Fixes 10 (its scanner rules), 18 and 19 can be switched off; the
  recorder attributes them by running each model with each left out.
- `split-model.ts`: fix 10, splitting on the scanner model's `;` tokens with the TS
  splitter's offsets and trimming.
- `statements-model.ts`: fixes 11, 12 and 14 on the scanner model's tokens.
  The read-only check scans every reading with both the fix 19 and the TS
  word rule.
  Fix 11 adds three reasons to `DestructiveReason`: `drop_sequence`,
  `drop_function` and `merge_delete`.
- `params-model.ts`: every engine's substitution on the scanner model's
  contexts, with the TS's value formatting. Fix 13 on Postgres/SQLite (bound
  and forced inline), MySQL/MariaDB forced inline, and MariaDB's `/*M! … */`
  on the bound path; fix 10 for DuckDB (its `--` comment ends at `\r`) and
  MySQL's `--` rule. SQL Server's output is unchanged. Fix 13's value rules
  on every engine, each switchable (`withoutValueRule`) so the recorder
  names the one that changed a case: a value that forms the `$tag$` with the
  text next to it is refused (`tag-edge`); `\{{p}}` in a string that takes
  backslash escapes is text, not a parameter (`escaped`); a decimal that
  isn't a finite decimal number is refused where its text goes into the SQL
  (`decimal`); a bigint inlines as its digits (`int-digits`); a value inlined
  in code is spaced off a quote before it and a word or `$` after it
  (`adjacent`).
- `create-table-fixed.ts`: a copy of `parse-create-table.ts` with fixes 16
  and 17 (checked to equal the TS with both off; attributed by leaving each
  out).

`bugfixes.json` has three parts:

- `about`.
- `cases`: written by hand: fixes 5, 7, 8 and 9, which have no model, and
  extra inputs for fixes 3, 6 and 15 that no corpus entry covers. The recorder never changes
  them, and checks that each has a fix number and a reason. A case with
  `replaces` (`"<file>: <case name>"`) has the same input as that fixture case
  and supersedes its output. A case without it is standalone: an extra input
  whose `ts` is what the TS gave (the recorder checked it on every run).
- `modelCases`: generated by the recorder, one per fixture case (and engine)
  a model changed: `fixes`, the fixture `case`, `engine`, a `reason`, and
  `diff`, where the TS differed (`path: <TS> → <fixed>`, at most 8 lines).

So a Rust parity test reads a fixture file, replaces each case named by a
`bugfixes.json` `replaces`, runs the standalone cases of its kind too, and
compares everything. It doesn't need `modelCases`.

## What the fixtures pin that may look wrong

Parity means these TS quirks are expected output (decision 2), not bugs to fix
in the port:

- **Visual AST** (`visual.json`): `IN ((subquery))` and `EXISTS((subquery))`;
  `!=` kept as written; `x BETWEEN (1, 5)`; `schema: null` and `alias: null`
  on INSERT/UPDATE sources. Decided at the Task 2 checkpoint to keep these;
  the spike's port prints them differently. Casts (fix 15) and placeholders
  (fix 6) are fixed instead.
- **Tutorial parser**: the quirks in the plan's Follow-ups (IN, IS NULL and
  BETWEEN filters dropped, the connector shift, `50.0` kept as text, …).
- **`parseCreateTableSql`**: the schema defaults to `public` on every
  engine; type names keep their case as written; `GENERATED … AS (…)` is
  dropped. (Its lost data is fixes 16 and 17; fix 16 reads `COLLATE` into
  `collation`.)
