# seaquel-storage fixtures

These files record what the TypeScript metadata storage did before `seaquel-storage` replaced it: the schema every release created, what today's upgrade path does to each of them, and what each of the 19 repositories stores and loads. The TypeScript was the spec. The Rust crate has to match it.

**The fixtures are frozen.** The script that made them needs the TS storage code, which phase 3 deletes, so they can't be re-recorded. Change a fixture only when the Rust behaviour is meant to differ from the TS, and say why in the change (and in the section below). Never regenerate one to make a failing test pass.

## How they were made

`docs/plans/artifacts/2026-09-29-freeze-storage-baseline.mjs.txt` ran once from the repo root at `fecb1b4` (phase 3, Task 2). It ran the real code, not a copy of it:

- **Each release's code.** For every tag, `git archive <tag> src/lib` was bundled with esbuild (types stripped, `$lib` resolved). Today's tree was bundled in place. Inside `MigrationManager`, `$lib/storage` pointed at the real repositories, with `getDatabase()` returning the database under test and the logger silenced.
- **Engine.** better-sqlite3 12.11.1 (SQLite 3.53.2), through the same `SqliteDatabase` adapter the web server uses (`src/lib/server/storage.ts`). The repository cases went through the real `openUserDb`, so they got the web server's bootstrap: WAL, `busy_timeout = 5000`, `foreign_keys = ON`, `initializeSchema`, and a `schema_version` row of 4.
- **Second backend.** Every repository case also ran through the demo's real `WebSqliteDatabase` over sql.js, with `localStorage` replaced by a Map. Where the two disagreed, the case had a `demoDiffers` field (see "Surprises"); Task 8 fixed the cause and removed them.
- **Not run:** the desktop path (`TauriSqliteDatabase` over `db_query`/`db_execute`), which needs Tauri. It binds and decodes these column types the same way, but that wasn't checked.
- **No legacy JSON.** No legacy JSON files existed in these runs, so `migrateJsonToSqlite` was skipped, and so was the `app_state['json_migration_done']` write in `db.ts`.
- **Timezone.** `TZ=UTC`, because `new Date("2024-01-02 03:04:05")` reads local time.

The output is deterministic: two runs gave byte-identical files.

## Files

### `schemas/<release>.sql`

The schema of a fresh file after its first launch on `v2026.4.5-beta.1`, `v2026.4.5`, `v2026.4.8`, `v2026.9.1`, `v2026.9.2` and `current` (today's tree). Each was made the way that release's `db.ts` did it: `initializeSchema`, then the version row `db.ts` inserted, then that release's `MigrationManager.migrateIfNeeded`.

- The file is `sqlite_master`'s `sql` in rowid (creation) order, followed by the `schema_version` rows. `migrated_at` is fixed at `2000-01-01 00:00:00`.
- Each file loads as-is with `sqlite3_exec`.
- `v2026.9.1`, `v2026.9.2` and `current` are identical apart from the header.

**Version rows.** beta.1's `db.ts` inserted `SCHEMA_VERSION` (1), and its `MigrationManager` then ran v2 and v3 and inserted 3. So a beta.1 file has the rows 1 and 3, not a single 3. Every later release inserted 4 on a fresh file. beta.1's first launch also created the default project (`default-seaquel`, "Seaquel"). That row is data, so it isn't in the dump; `upgrade-differences.json`'s `firstLaunchRows` has it.

### `schemas/upgraded/<release>.sql`

Each release's file after today's existing-file path: `initializeSchema` (which runs `upgradeSchema`), then today's `MigrationManager.migrateIfNeeded` (the v3 and v4 column adds, plus the version row).

`v2026.4.5-beta.1-via-v2026.4.5.sql` is the beta.1 file after v2026.4.5's code and then today's. That's the path beta.1 users took if they installed v2026.4.5 through v2026.4.7 before a later release.

Running the upgrade a second time changed nothing in any case.

### `schemas/upgrade-differences.json`

Every structural difference between an upgraded file and a fresh `current` file. For each table it compares the column list and order, `PRAGMA table_xinfo` (type, not null, default, pk), `foreign_key_list`, `index_list`/`index_xinfo`, and the `CREATE` text with whitespace collapsed. It also compares the index set.

### `upgrades/v2026.4.5-beta.1-data.json`

A beta.1 file seeded with data (`seedSql`), then upgraded directly (`direct`) and through v2026.4.5 (`viaV2026_4_5`). It shows the data moves that Decision 3 ports:

- `saved_queries` and `dashboards` get `project_id` from their connection;
- the orphan dashboard, whose connection doesn't exist, is deleted;
- `active_canvas_tab_id` becomes `active_workflow_tab_id`, and `active_view` `canvas` becomes `workflow`;
- the version rows end as 1, 3, 4.

### `repos/<repo>.json`

One file per repository in `src/lib/storage/repos/`, 76 cases in all:

| file | repo | cases |
|---|---|---|
| `projects.json` | `projectsRepo` | 6 |
| `app-state.json` | `appStateRepo` | 3 |
| `connections.json` | `connectionsRepo` | 7 |
| `connection-overrides.json` | `connectionOverridesRepo` | 4 |
| `project-state.json` | `projectStateRepo` | 9 |
| `saved-queries.json` | `savedQueriesRepo` | 7 |
| `query-versions.json` | `queryVersionsRepo` | 6 |
| `query-history.json` | `queryHistoryRepo` | 5 |
| `shared-repos.json` | `sharedReposRepo` | 3 |
| `themes.json` | `themeRepo` | 3 |
| `license.json` | `licenseRepo` | 2 |
| `onboarding.json` | `onboardingRepo` | 2 |
| `tutorial.json` | `tutorialRepo` | 2 |
| `import-state.json` | `importStateRepo` | 1 |
| `dashboards.json` | `dashboardsRepo` | 4 |
| `dashboard-versions.json` | `dashboardVersionsRepo` | 3 |
| `ai-chats.json` | `aiChatsRepo` | 4 |
| `vault-state.json` | `vaultStateRepo` | 3 |
| `user-credentials.json` | `userCredentialsRepo` | 2 |

Each case starts from a fresh `current` database with nothing in it but `schema_version` (4), and runs its `steps` in order:

- `{"call": "<repo>.<method>", "args": [...], "result"?: ..., "error"?: {"message", "code"}}` is a repository call. The `db` argument is left out. A call that returns nothing has no `result`.
- `{"sql": "...", "params": [...]}` is raw SQL run through `SqliteDatabase.execute`. It sets up rows the save path would never write: bad JSON, flag values other than 0/1, legacy values.

`rows` is the state of the listed tables after the last step. It gives `columns`, `rows` (values) and `types` (SQLite's `typeof` for each value), sorted by every column in order (`ORDER BY 1, 2, …`), so it doesn't depend on rowids. Compare storage classes too: a whole number in the REAL column `query_history.execution_time` is `real`, not `integer`.

Some cases also have:

- `notes`: behaviour worth reading before porting;
- `demoDiffers` (removed in Task 8, see the change log): the steps and tables where sql.js (the demo) gave a different answer.

**Value encoding** in `args` and `result`:

- A JS `Date` is `{"$date": "<toISOString()>"}`, or `{"$date": null}` for an Invalid Date. Only `connections.lastConnected` loads as a Date. On the wire it's the ISO string, and a stored string that isn't in `toISOString` form comes back re-serialised.
- bigint, `Uint8Array`, `SqlDecimal`, NaN and ±Infinity are `$lib/values` `toStorable` tags (`{"$sq": "bigint", "v": "…"}`, …). A plain object that already has a `$sq` key is wrapped in `{"$sq": "json", "v": …}`. These only occur inside `savedWorkflows`, and that's also exactly how `saved_canvases.data` stores them.
- An object key whose value is `undefined` is left out, so "absent" and "undefined" are the same thing. `null` is kept.
- One exception to exact values: a saved workflow without an id gets `workflow-<random uuid>`, recorded as `workflow-<random-uuid>` (`project-state/workflow-without-id`). Match it by pattern.

**Comparing** (Task 4):

- Loaded objects are compared as JSON values: key order doesn't matter, but presence does.
- `rows` are compared exactly, `types` included.
- For an error, only its presence is part of the contract. The `message` and `code` come from the JS runtime (`JSON.parse`) or from better-sqlite3, and a failed batch must leave `rows` as recorded (a rollback).

### `row-shapes.json`

- `repos`: each repository method's declared parameters and result, read by the TypeScript checker from `src/lib/storage/repository.ts`, with the `db` argument dropped.
- `types`: every named `Persisted*` type they use, field by field. Each field has its TS type, `optional`, the JSON kinds it can take (`Date` means a JS Date that crosses as an ISO string; `absent` means the key may be missing) and `enum` for string-literal unions. `SavedWorkflow` is opaque: it's stored as JSON text, and Rust passes it through.
- `observed`: for each load method, the keys it actually returned across the cases, with `always: true` when every sample had one.

Task 4's ts-rs types must regenerate to the same shapes. Where `observed` and the declared type disagree, `observed` is the behaviour. For example, `PersistedSavedQuery.starred` is declared optional but is always present, and `PersistedConnection.isLocalOnly` is only ever `true` or absent.

## Surprises found while recording

Recorded as the TS behaves. None were fixed.

### Upgrades

1. **Today's code can't open a `v2026.4.5-beta.1` file.** `upgradeSchema` fails with `no such table: ai_messages`, on every launch.
   - Its column adds run before the `CREATE TABLE`s, and `PRAGMA table_info` on a missing table returns no rows. So it runs `ALTER TABLE ai_messages ADD COLUMN dashboard_id` against a table beta.1 never had.
   - This has been true since v2026.4.8, which added that column add.
   - The upgrade isn't one transaction, so it leaves the file half-upgraded: the `connections`, `project_state`, `saved_queries` and `dashboards` column adds before it are applied, and nothing after them is (`schemas/upgraded/v2026.4.5-beta.1.sql`).
   - Going through v2026.4.5 first works.
   - Task 3's baseline has to decide. The simplest fix is to skip column adds for tables that don't exist yet, since the `CREATE TABLE` that follows makes them complete. Decision 3's claim that every release's file upgrades holds only with that fix.
2. **Files that started on beta.1 keep a different `saved_queries` and `dashboards`,** even when upgraded through v2026.4.5:
   - `project_id` was added by `ALTER TABLE`, so it's nullable, has no foreign key and is the last column;
   - so deleting a project doesn't cascade to those rows on such files;
   - the indexes are the same as on a fresh file;
   - `dashboards.starred` is `INTEGER DEFAULT 0` without `NOT NULL` on every file, fresh ones included.

   This is the plan's example of "behaviour to keep". The Rust baseline shouldn't rebuild these tables unless someone decides to.
3. **Column order differs on upgraded files.** Every file from before v2026.4.8 (v2026.4.5, and beta.1 through it) has `project_state.connection_order` last. beta.1 files also have `active_create_table_tab_id` and `active_data_tab_id` late, and `saved_queries`/`dashboards` reordered. The `CREATE` text also differs where `ALTER TABLE ADD COLUMN` appended text (`query TEXT , dashboard_id TEXT`). Read columns by name, never by position, and compare schemas structurally.
4. v2026.4.8, v2026.9.1 and v2026.9.2 files upgrade to exactly today's schema.

### The demo backend (sql.js)

5. **Foreign keys are off in the demo after its first write.** `WebSqliteDatabase` calls `db.export()` after every write to persist to `localStorage`, and sql.js's `export()` closes and reopens the database. That resets `PRAGMA foreign_keys` to 0. So in the demo:
   - `ON DELETE CASCADE` never fires: deleting a project or connection leaves its children behind;
   - rows with a missing parent are accepted.

   Ten cases differed for this reason alone, and they were marked `demoDiffers`. **Fixed in Task 8** (see the change log). Task 8's "the two clients must agree" test has to expect this, or the demo's client has to re-enable the pragma after each export. That would be a behaviour change in `web-sqlite.ts`, which the demo keeps until phase 8.
6. sql.js throws on an `undefined` bind parameter, while better-sqlite3 and the Tauri path send NULL. No case passes one (`args` never contain `undefined` in a bound position), but a caller that omits a field typed `string | null` in `projectStateRepo.save` would fail only in the demo.

### Repository behaviour to port exactly

7. **Flags.** `bool` columns load as `value === 1`, so a stored 2 loads as `false`.
   - `isLocalOnly` loads `true` or absent, never `false`.
   - `aiShareSchema`/`aiShareData` load `true`, `false` (0) or absent (NULL).
   - The `save*` flags always load as booleans.
   - A NULL `dashboards.starred` loads as `false`.
8. **JSON columns.**
   - Bad JSON falls back to the column's default: absent, `[]` or `null`, depending on the column.
   - Stored `'null'` parses to `null` and is returned as `null`, not the fallback: `saved_queries.tags`, `connections.ssh_tunnel`, `project_state.connection_order`.
   - An empty `ssh_tunnel` string loads as absent.
   - `vault_state.kdf_params` uses a bare `JSON.parse`, so bad JSON makes `vaultStateRepo.load` throw.
9. **`dateFilter` versus `description`.** `dateFilter` loads `null` when unset (the key is present). `description` and the other `nullable` columns load absent.
10. **`projectStateRepo`:**
    - Schema and data tabs store `tableName` in `name` too.
    - Workflow tabs are stored as `tab_type = 'canvas'`.
    - `active_visualize_tab_id` is always written NULL.
    - `connectionTabs`, `activeConnectionTabId` and the `extensionsDuckdb*` fields aren't stored.
    - Tabs of other types (`visualize`) are dropped on load.
    - `active_view = 'canvas'` is returned as-is (only `upgradeSchema` rewrites it).
    - A query tab's `queryId` is `saved_query_id ?? shared_query_id`.
    - The row is written with `INSERT OR REPLACE`.
    - Saved workflows go through `toStorable`/`fromStorable`. A canvas whose tag won't decode is dropped on load; a `$sq` object without `v` loads as a plain object.
    - A failed batch (missing project, duplicate tab id) rolls back as a whole.
11. **`savedQueriesRepo.saveAll`** upserts by id, so saving project B with a query id that belongs to project A moves the query to B (`saved-queries/save-all-moves-query-between-projects`). Kept queries keep their versions; dropped ones cascade.
12. **`queryVersionsRepo.pruneOldVersions`** resolves texts with diff-match-patch before deleting, then promotes the oldest survivor to a keyframe.
    - diff-match-patch works in UTF-16 code units, and the patch text is `%`-escaped UTF-8. The Rust port has to resolve the same text for astral characters; the case uses 🚀. That needs a diff-match-patch port with JS semantics (or keeping the promotion in TS).
    - `keepCount = 0` does nothing here, but deletes every version in `dashboardVersionsRepo.pruneOldVersions`.
    - **Replaying these cases (Task 4).** Under the plan's split (the "Task 2 findings" block after Decision 3), TS computes the prune and storage only executes `QueryVersionsPrune { saved_query_id, delete_ids, promote: Option<{ id, snapshot }> }`. So replay a `pruneOldVersions` step like this. Take the rows before it. The delete ids are the versions missing from the next recorded `loadByQuery` result. `promote` is the oldest survivor whose recorded `snapshot` became non-null, with that snapshot as its text. Run the Rust prune with those, then compare `rows`. The diff-match-patch resolution itself stays covered on the TS side.
13. **Load order.** `tutorialRepo.loadAll`, `connectionsRepo.loadAll`, `projectsRepo.loadAll` and the other loads without `ORDER BY` return rowid order. `INSERT OR REPLACE` gives a replaced row a new rowid, so an overwritten tutorial pair moves to the end. `connectionsRepo.loadAll` returns `labelIds` in the order of `connection_labels`' primary-key index (`connection_id, label_id`), not in insertion order: a save of `["lbl-prod","lbl-2"]` loads as `["lbl-2","lbl-prod"]`. Port the statements as they are, and the order follows.
14. **`lastConnected`.**
    - It's parsed with `new Date(text)`, so a stored string without a zone reads as local time, and an unparseable one gives an Invalid Date.
    - It's written as `toISOString()` from a Date, or passed through as-is if it isn't one.

## When a fixture may change

Only when the Rust behaviour is meant to differ from the TypeScript: a bug fixed on purpose (surprise 1, for example, or Decision 13.1's password stripping, which changes stored `connection_string` values), or a documented decision. Edit the fixture by hand, keep the rest of the file as it is, and record here what changed and why:

- **Phase 3 Task 8: the ten `demoDiffers` fields are gone.** The demo's foreign keys are fixed: `web-sqlite.ts` turns `PRAGMA foreign_keys` back on after each `export()` (surprise 5; the plan's "Task 2 findings"). With that, sql.js gives the recorded results and rows in every case, which `src/lib/storage/client.test.ts` checks, so the fields no longer describe anything. Only those fields were removed; every step, result and row is unchanged.
