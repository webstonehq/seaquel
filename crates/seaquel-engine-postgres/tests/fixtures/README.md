# Postgres dialect fixtures

The JSON files here were reformatted to one case per line on 2026-10-04, with no value changed.

`docs/plans/` (the plans, and the recorders' copies in `docs/plans/artifacts/`) was deleted on 2026-10-04. The paths under it named below are in git history: `git show ae7f269:<path>`. The same day, the `about` text of `bugfixes.json` (no test reads it) gained ", now in git history at ae7f269" after the plan it names; nothing else in the file changed.

These JSON files were recorded from the TypeScript `PostgresAdapter`
(`src/lib/db/postgres.ts`) during phase 1 of the Rust core work, by running it
against the e2e Postgres database. They pinned down what the Rust port had to
reproduce: SQL generation, DDL, CRUD builders, and the parsers for schema,
columns, indexes, statistics and EXPLAIN output.

The TypeScript adapter was deleted at the end of phase 1, so these files can no
longer be re-recorded. They are frozen regression fixtures now, read by
`tests/dialect_parity.rs` and `tests/introspect_parity.rs` (and `bugfixes.json`
also by `tests/smoke.rs`).

`bugfixes.json` is the exception: it was written by hand and holds the cases
where the Rust dialect deliberately differs from the old TypeScript output (see
"Bug fixes" in `docs/plans/2026-09-25-rust-core-phase-1-plan.md`).

The recorder was `scripts/fixtures/postgres-dialect.fixtures.test.ts`, run with
`npm run fixtures:postgres`. It was removed together with the TypeScript adapter
it imported. A reference copy was kept at
`docs/plans/artifacts/2026-09-25-postgres-fixture-recorder.ts.txt` (renamed so
no tooling runs it). Phase 2 replaced it with one recorder for every engine,
kept at `docs/plans/artifacts/2026-09-27-recorder-*.txt`.

Change a fixture only when the Rust behaviour is meant to change, and say why
in the same change.

## Changes

- **Phase 5b, Task 4 review (`paginate.json`, every case):** the limit now
  goes on its own line (`…\nLIMIT n OFFSET m`). A trailing `--` comment
  swallowed the LIMIT: `SELECT * FROM big -- note` paged as
  `… -- note LIMIT 101 OFFSET 0`, which fetched the whole table. Only the
  whitespace before `LIMIT` changed.
