# library fixtures

These files record what today's TypeScript does with the library: saved connections (their fields, labels, AI settings, the local-only flag, `lastConnected` and secrets), projects, custom labels, saved queries and their versions, the TablePlus, DBeaver and shared-project imports, and the pre-5a connection strings. Phase 5d-1 moves that work into Core (`seaquel_workspace::library`, the `Workspace` library methods and the `library` RPC group), and these cases pin it the way `../edits` pinned the grid's edits. See `docs/plans/2026-10-04-rust-core-phase-5d-plan.md`, "Parity fixtures" and Task 2.

**The fixtures are frozen.** After 5d-1 the GUI writes the library through Core, and the TypeScript survives only in the demo (`TsLibrary`) until phase 8. Change a case only when Core is meant to behave differently, say why in `changes.json` and in "`changes.json`" below, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-04-record-library-fixtures.test.ts.txt`, a vitest file. It ran on `1cf3ddb` plus the phase 5d working tree after Task 1 (saves and removals that throw after their toast, `add` rolling back a failed save, the web vault unlocked before the save, `removeCustomLabel` saving the connections it changed, imports with `conn-<uuid>` ids, the five-field duplicate check and the connection order, the shared-query rename, the stale-tab stopgap, and the review's follow-ups: TablePlus and DBeaver imports local-only, a cleared git path taking the removed connections out of the connection order, and the version limit settings clamped to at least 10 on save). To rerun it, copy it to `src/lib/hooks/database/record-library.test.ts`, run it with `FREEZE_LIBRARY=1`, and delete the copy. It needs a tree that still has the TypeScript library managers.

It runs the real code:

- `ConnectionManager` (`add`, `update`, `reconnect`, `autoReconnect`, `remove`, `toggleLocalOnly`, `importConnections`, `initializePersistedConnections`), `LabelManager`, `ProjectManager` (`initialize`, `add`, `update`, `setGitRepoPath`, `remove`, `addCustomLabel`, `updateCustomLabel`, `removeCustomLabel`, `importFromGitRepo`, `importSharedConnections`, `importSingleSharedConnection`), `SavedQueryManager`, `StateRestorationManager` and `PersistenceManager`, wired as `UseDatabase` wires them;
- the demo's storage client (`createSqljsStorageClient`) on an in-memory sql.js file made by `bootstrapSqljsDatabase`, with foreign keys on. The frozen repo fixtures (`crates/seaquel-storage/tests/fixtures/repos`) pin those repositories to the Rust ones, so the rows here are what Rust storage would hold;
- the desktop keyring, `TauriKeyringService`, whose `callSecret` goes to a recording in-memory store. So `secretCalls` are the `secret` requests desktop sends Core today, and `secretStore` is what the keychain holds after each step.

Stubbed or recorded:

- **Providers and the engine.** `connect` answers a Core id (`core-1`, …), `disconnect` and `test` do nothing, and `getEngineClient` answers an empty schema list. No database is reached.
- **Shared repos.** The `SharedRepoManager` methods the library calls (`shareConnection`, `unshareConnection`, `updateSharedConnection`, `initRepo`, `loadQueriesFromRepo`, `setRemoteUrl`, `exportProject`, `removeRepo`) and the saved queries' file projection (`writeQueryFile`, `deleteQueryFile`, with the path `nameToFilename` gives) record their calls in `files` and touch no disk. `@tauri-apps/plugin-fs` and `$lib/services/git` are no-ops. The shared-project cases (`sharedRepo: true`) start with one repo in memory, `repo-1` at `/repos/team`, holding a project "Team" with two shared connections, `repo-1:prod` (`sslMode: require`) and `repo-1:bastion` (an SSH tunnel).
- **`setConnectionAIModel`** lives on `UseDatabase`, which isn't built; its four lines are copied into the recorder and call the real `persistConnection`.
- **Two windows** (`saved-query/two-tabs-both-create`): a second set of managers over the same storage, opened inside the step. `view` is the first window's.
- **The web vault** (`add/web-vault-cancelled`, `target: "web"`): `getVault().ensureUnlocked()` throws `VaultCancelledError`. No other case runs as web.
- **Injected failures** (`inject`): `add/keychain-failure` makes the keychain's `set` throw `SECRET_STORE_ERROR: keychain locked`; `import/tableplus-two-databases-one-host` makes the storage save of the draft named "Broken" throw `STORAGE_ERROR: disk full`.
- **Toasts** are captured, the logger and `console.error` silenced, `crypto.randomUUID` is a counter, and the clock is fake: pinned at 2030-01-01T00:00:00Z. After every step the recorder lets pending promises finish and advances the clock so the 500 ms project save fires (six rounds of 600 ms), so the saved-query writes that save does are in that step's rows. Before step 0 it does the same for whatever loading wrote, then clears the toasts, secret calls and files. Seeds go in with plain `INSERT`s. Nothing else is cleared or reset between steps: the managers, the storage and the store carry over. There is no write queue here: the sql.js client writes as it is called (`RustStorageClient`'s queue isn't involved).

Two runs gave byte-identical files.

## Normalising

- Ids made during a case are `<id:n>`, numbered in order of first appearance in the case's JSON, keeping their prefix: `conn-<id:1>`, `project-<id:1>`, `label-<id:1>`, `saved-<id:1>`, `ver-<id:3>`. Seeded ids (`p1`, `c1`, `q1`, `label-a`, `v1`, …) stay.
- Times from the pinned clock are `<now>`. Seeded times (`2024-01-01T00:00:00.000Z`) stay, so a time that didn't change can be told from one that did.
- Rows are the raw SQLite rows (column names and stored values: `0`/`1` for booleans, JSON as text), ordered by the key in the recorder's `TABLES`. A table with no rows is left out, so an absent table is an empty one.

## A case

| field | meaning |
| --- | --- |
| `name`, `note` | what the case is |
| `file` | `current` (a fresh file) or `v2026.4.5-beta.1` (a file that started on that release; see "Replay rules") |
| `seed` | rows inserted, in the recorder's table order (`projects`, `project_labels`, `connections`, `connection_labels`, `saved_queries`, `query_versions`, `query_history`, `ai_chats`, `ai_messages`, `dashboards`, `dashboard_versions`, `saved_canvases`, `project_state`, `tabs`, `app_state`), before anything loads. Most cases have `p1` "Main" and `p2` "Other", and `lastActiveProjectId` `p1`. `query_version_limit` in `app_state` is the prune setting |
| `secrets` | the keychain before the case |
| `sharedRepo`, `target` | the shared-repo state above; `web` for the one web case |
| `before` | the rows after the managers loaded, before step 0 |
| `steps[].op`, `args` | the TS call and its input |
| `steps[].inject` | a failure the recorder forced (above) |
| `steps[].library` | the Core call a GUI on Core sends for the step (`{method, params}` in the plan's `LibraryRequest` shape), a list of them in order, or `null` for none. The recorder's mapping of each TS call onto the plan's wire: connection drafts spell the AI model fields `activeAIProviderId` and `activeAIModel` as the stored row does, import drafts (TablePlus, DBeaver, shared templates, and the project a shared-project import makes) carry their own name and `renameIfTaken: true` (Core picks the free name), TablePlus and DBeaver drafts are `isLocalOnly: true`, secrets are `SecretChanges` (`db`, `ssh`, `sshKey`), and a patch holds exactly the fields the step changed. `<id:n>` here names an id made by an earlier step, or by an earlier call in the same list |
| `steps[].outcome` | `{ok: true, value}` (the TS call's return) or `{ok: false, error}` (its message) |
| `steps[].rows` | the tables after the step |
| `steps[].secretCalls`, `secretStore` | the keychain calls the step made, and the keychain after it |
| `steps[].files` | the shared-repo and file-projection calls, in order |
| `steps[].toasts` | `{kind, message}` |
| `steps[].view` | what the window holds after the step: the active project, projects (with `description` and `gitRepoPath` as `null` when unset, where the TS object has `undefined`) and their custom labels, connections (id, project, name, labels, connected), each project's connection order, saved queries (id, name, starred, shared) and versions (`<queryId>#<n>`) |

## Files

| file | cases | covers |
| --- | --- | --- |
| `connections.json` | 57 | `add` for each engine (fields only), with a string holding a password (Postgres, MySQL, an SQL Server key=value string, DuckDB with a secret option), each save flag with and without a secret typed, SSH password and key auth, labels (predefined and custom) and AI flags, another project, a missing project, a duplicate name (case and Unicode folding, and full folding: "STRASSE" against "Straße") and the same name in another project, an empty name, a NUL, a port out of range and a fractional one, a keychain failure, a cancelled web vault; `update` of the name, host/port/database/user, type, SSL mode set and cleared, string set and cleared, SSH tunnel added (with its secret) and removed, AI flags set, cleared and left out, the save-password flag on and off (with the keychain), a duplicate name, a bad port, a shared connection's YAML, labels and AI model kept, a saved password kept when the form sends none, a rename to its own name in another case, and a patch to one of two rows that already share a name; label add/remove/set, an unknown label, another project's label; the AI model; local-only off and on (with the shared YAML); `reconnect` from the form and `autoReconnect` (`lastConnected`); `remove` with its cascade (history, chats, messages, labels) and keychain entries, a shared one, an unknown id; add then update then remove |
| `projects.json` | 24 | `initialize` on an empty file (the default project) and with projects; add, a duplicate name, one equal only after NFC normalisation, an empty name; rename, a taken name, description set and cleared, an unknown id; git path set (repo registered, shared connections exported) and cleared (imported connections and their secrets removed, repo removed), a new repo; remove with contents (connections with secrets and history, labels, saved queries and versions, a dashboard, a saved workflow, its state), the active project, on a beta-era file, the last project, an unknown id; custom labels: create, rename, recolour, remove while in use (in this project and another), a duplicate name, a rename to a taken name, a bad colour, an unknown label |
| `saved-queries.json` | 25 | create (plain, from a tab with parameters), a duplicate name, the same name in another folder, a rename to a taken name, a NULL folder and `""` as one folder, an empty name, duplicate parameter names; update the text, the name only, rename from the tab, parameters set and cleared; star and unstar, share and unshare (the `.sql` file), rename a shared query, delete with versions, delete a shared one, an unknown id; eleven versions (the 11th keyframe), prune at 3, prune at 0, after stored diffs, prune with stored diffs at 3; two windows both creating; create and update within 500 ms |
| `imports.json` | 9 | TablePlus: two databases on one host and port, one already saved, one whose save fails, another engine with an SSL mode; DBeaver with an SSH tunnel; an imported name the project already has, one that differs only in case, and two drafts of one import with one name; a shared project whose name is taken ("Main (2)", linked, its shared connections imported) and one whose name differs only in case ("main"); a linked project's shared connections (one already imported); one shared connection imported twice |
| `legacy-strings.json` | 1 | one load of fifteen rows: the pre-5a built string for Postgres (default port, a port and SSL mode, the `postgresql` scheme, a username filled in from the string, an encoded username), MySQL with an SSL mode, MariaDB on a port, SQL Server, SQLite and DuckDB in memory, all dropped; a string with an extra parameter, another host, an SQL Server key=value string, a DuckDB option and no string, all kept |

116 cases, 148 steps.

## Replay rules

Two replays read these files, and each compares its own fields. A field neither lists isn't compared. Where a case is in `changes.json`, its expected fields replace the recorded ones first (see below). The TS messages, toasts and `before` aren't compared by either.

### The Rust replay (`seaquel-workspace/tests/library_plan.rs`, `seaquel-core/tests/library.rs`)

- **The file.** `current`: a fresh `Storage`. `v2026.4.5-beta.1`: `crates/seaquel-storage/tests/fixtures/schemas/v2026.4.5-beta.1.sql` loaded, then opened through `Storage::open` (the baseline leaves `saved_queries.project_id` and `dashboards.project_id` without a foreign key). The recorder made that shape on sql.js by rebuilding those two tables without the foreign key; nothing else about a beta-era file matters to these cases.
- **The seed** is inserted with plain SQL in the table order above, and `secrets` go into a `MemoryStore`. Desktop target: Core has the store. `legacy-strings.json` needs its rows in the file before the data step `drop_legacy_built_connection_strings` runs: insert them, remove the step's `_seaquel_data_steps` row, and open the file again.
- **Seed rows** are plain `INSERT`s; no seed repeats a key.
- **Steps.** Each `library` call (or each of a list, in order) goes to the workspace's library method with a fresh origin. A step whose `library` is `null` isn't replayed (`add/web-vault-cancelled`); the rest of the case still is. A draft named in `inject` is left out of the step's list already, so the other drafts are replayed as recorded.
- **Ids.** A `<id:n>` token is bound to the Core id the create that made it returns (the first place it appears in an outcome value or in rows; within a list, the earlier call's response), and replaced by that id in later `library` params. Every id Core makes must have the recorded prefix and a v4 uuid. `<version:n>` in `changes.json` is any `ver-<uuid>` id.
- **Times.** `<now>` matches any time the `Executor` gave during the case; a seeded time must come back unchanged.
- **Outcome:** `ok`, and for a refusal the `code` `changes.json` gives (every recorded failure that Core replays is listed there, with Core's code). `takenBy` is the id a `NAME_TAKEN` names. The value isn't compared (Core returns rows where the TS returned an id or `true`).
- **Rows, library tables:** `projects`, `project_labels`, `connections`, `connection_labels`, `saved_queries` and `query_versions`, whole, after every step.
- **Rows, what hangs off them:** `query_history`, `ai_chats`, `ai_messages`, `dashboards`, `dashboard_versions` and `saved_canvases` by their ids, after every step (they change only through a removal's cascade here). `project_state` and `tabs` only as "no row of a removed project remains": the TS writes them from its own debounced project save, which Core doesn't do. `app_state` isn't compared.
- **`secretStore`**, whole, after every step. `secretCalls` aren't compared: today a save with a flag off deletes entries that can't exist, and Core doesn't have to.
- **`files` and `view`** aren't compared: the git projection and the view stay in TypeScript.

### The TypeScript replay (Task 6's vitest over `TsLibrary`, the demo's)

- **The same `op` with the same `args`**, through the managers after Task 6 with `TsLibrary` over sql.js, the same stubs and the same clock and uuid rules, and the seed inserted the same way. A `null` in `args` stands for `undefined` in the TS call (JSON has no `undefined`): `project/description-set-and-cleared` records `updates: {description: null}` for `update("p1", {description: undefined})`; the replay maps it back before calling.
- **Rows** as the Rust replay compares them, after `changes.json`: `TsLibrary` keeps Core's rules for the demo.
- **`outcome.ok`, `files` in order, and `view` after every step.** For a case in `changes.json`, `view` isn't compared where the entry changes the rows; the rows are.
- **Secrets aren't compared**: the demo has no keychain. On desktop and web the GUI's own tests (Task 6) check that no `secret` or vault write for a connection is sent outside the Core call.

## What can't be recorded from the TS

- **Events, `seq` and origins** (Decisions 16–18): there is nothing in the TS to record. Core's tests pin them (`changes.rs`).
- **Web vault rows on removal** (Decision 8): the recorder runs desktop. `remove_deletes_vault_rows` in Task 4.
- **Web limits** (Decision 15), **`ENGINE_NOT_AVAILABLE` on web**, and moving a connection to another project (no TS call does it): Task 4's own tests.
- **A failed storage write inside Core's transaction** (the injected "Broken" import is left out of the replay instead).

## `changes.json`

Each entry is the name of a case where Core is meant to differ, with the Decision (from the phase 5d plan) that makes it differ, why, and `expected.steps`: for each step that differs (by index), the fields as Core should produce them. `outcome` replaces the outcome; each table in `rows` replaces that table (the library tables are all given; a cascade table only where it changes); `secretStore` replaces the store. The replays assert that exactly these steps differ from the recording (under the rules above), and that they then match `expected`. The entry `*` is the id and time rule above, which applies to every case.

- **Decision 1 and Q3, validation and names (30 cases):** refusals before anything is written, with the rows and keychain as before the step. Names compare with `name_key`: trimmed, NFC-normalised and fully Unicode case-folded.
  - `NAME_TAKEN`, naming the other row (`takenBy`): `add/duplicate-name` (" ärger db " against "Ärger DB"), `add/duplicate-name-full-case-folding` ("STRASSE" against "Straße"), `project/add-duplicate-name-nfd` ("Cafe" + U+0301 against "Café"), `update/duplicate-name`, `project/add-duplicate-name`, `project/rename-to-taken-name`, `label/create-duplicate-name`, `label/rename-to-taken-name`, `saved-query/create-duplicate-name`, `saved-query/rename-to-taken-name`, and `saved-query/null-and-empty-folder-are-one` (a NULL folder and `""` are one folder). Not refused, as today: the same name in another project (`add/duplicate-name-other-project`) or folder (`saved-query/create-same-name-other-folder`), a rename to the row's own name in another case (`update/rename-to-own-name-other-case`), and a patch to one of two rows that already share a name (`update/existing-duplicates-stay`).
  - `INVALID_ARGUMENT`: empty names (`add/empty-name`, `project/add-empty-name`, `saved-query/create-empty-name`), a NUL (`add/nul-in-name`; sql.js stored the name cut at the NUL), ports (`add/port-out-of-range`, `add/port-fraction`, `update/port-out-of-range`, Decision 7), a colour that isn't `#rrggbb` (`label/create-bad-colour`), duplicate parameter names (`saved-query/create-duplicate-parameters`, Decision 11).
  - Missing rows: `PROJECT_NOT_FOUND` (`add/missing-project`, where today the insert fails on the foreign key; `project/update-unknown-id` and `project/remove-unknown-id`, silent today), `SAVED_CONNECTION_NOT_FOUND` (`remove/unknown-id`), `SAVED_QUERY_NOT_FOUND` (`saved-query/delete-unknown`), `LABEL_NOT_FOUND` (`label/remove-unknown`).
  - Decision 7, labels on a connection: `LABEL_NOT_FOUND` for an id that is neither predefined nor the project's (`add/unknown-label-id`, stored today; `labels/set-with-unknown-id`, dropped silently today; `labels/other-projects-label`, ignored silently today).
  - Decision 9: `project/remove-last` is `LAST_PROJECT` (today `remove` returns `false`).
- **Decision 2, patches (1 case):** `update/ai-flags-left-out` keeps the AI flags the patch leaves out; today `update` clears them. `update/secret-kept-when-absent` isn't listed: a patch with no secret keeps the stored one, as today.
- **Decision 8, secrets first (1 case):** `add/keychain-failure` is `SECRET_STORE_ERROR` with nothing stored; today the row is saved and a warning says the password wasn't.
- **Decision 9, beta-era removal (1 case):** `project/remove-beta-era` deletes the project's saved queries (and so their versions) and dashboards by `project_id`.
- **Decision 10, labels (3 cases):** `label/create`, `label/rename-and-recolour` and `label/remove-in-use` write only `project_labels` (and the stripped `connection_labels`), so the project's `updated_at` stays; today the whole project is saved with `updated_at` now. `labelRemove` strips the label id from every connection, so no row points at a missing label: in `label/remove-in-use`, `c3` (in `p2`, seeded with `p1`'s `label-a`) loses it too, where today only `p1`'s connections are stripped.
- **Decision 11 and Q10, versions (8 cases):** every version Core writes is a keyframe of the previous text (`eleven-versions`, `prune-at-3`, `versions-after-existing`), none for an unchanged text (`update-name-only`, `update-parameters`), numbered after the row exists (`create-then-update-within-500ms`), limit 0 keeping all (`prune-at-0`), and the prune keeping back to the nearest keyframe (`prune-keeps-back-to-keyframe`: v1 stays because v2 and v3 are diffs on it). Stored diffs are left as they are. Starring leaves `updated_at` (Decision 11), as today, so `star-and-unstar` isn't listed.
- **Decision 4, no replace-all saves (1 case):** `saved-query/two-tabs-both-create` keeps both windows' queries.
- **Decision 13 and Q3, imports (4 cases):** import drafts carry `renameIfTaken`, and Core stores the first free name (" (2)", " (3)", … by `name_key`) inside the transaction: `import/name-taken` ("Local (2)"), `import/name-taken-case-only` ("local (2)"), `import/two-drafts-same-name` (the second "Shop" is "Shop (2)") and `import/shared-project-name-case-only` (the project "main" next to "Main" is "main (2)"; today the de-duplication is exact). The GUI does no folding. `import/shared-project-name-taken` isn't listed: today's "Main (2)" is what Core picks.

Nothing else is meant to differ; a new difference found in Task 4 or 6 is a finding to report, not an entry to add.

## Bugs recorded here

Recorded as today's TypeScript does them, and corrected in `changes.json`:

- **After a prune, the in-memory version list is wrong, and at small limits the stored history is too.** The storage prune turns the oldest survivor into a keyframe, but the in-memory list keeps it as a diff whose base is gone. The next version's diff is made against the text that list resolves to. When no keyframe (every 10th version) is among the versions kept in memory, that text is wrong and the stored diff is corrupt: "SELECT 4SELECT 3" (`prune-at-3`, version 5) and "SELECT SELECT e" (`prune-keeps-back-to-keyframe`, version 5). Checked outside these fixtures (twice the limit plus 25 saves, then the stored history resolved): stored rows went wrong at limits 2, 3 and 8, and stayed right at 9, 10, 11, 20 and 100. At 9 and above, including the default of 100, only the window's view of its oldest in-memory versions is wrong until a reload. Limits under 10 were reachable because the setting's `min="10"` wasn't enforced; the settings now clamp to 10 on save. Damaged rows already stored can't be told from good ones, so nothing repairs them: no data step, and Core's keyframes only stop new damage.
- **`query_version_limit` 0 keeps only the first version.** Storage keeps everything at 0, but the in-memory prune drops every version, so the next number is 1 again and every later insert fails on the unique constraint, logged only (`prune-at-0`). The settings can no longer save 0; a stored 0 still behaves this way until Core.
- **A new query saved twice within 500 ms loses its first version** (the insert runs before the row exists; `create-then-update-within-500ms`). The plan's survey found this.
- **Two windows that both create queries delete each other's** (`two-tabs-both-create`), the loss Task 1's stopgap leaves.
- **A keychain failure leaves a row whose password wasn't saved** (`add/keychain-failure`).

## Known quirks pinned here

Today's behaviour, recorded on purpose and not changed by a Decision:

- **Shared-template imports aren't local-only.** A connection imported from a repo's shared template is stored with `is_local_only` 0 (it belongs to the repo); TablePlus and DBeaver imports are local-only, like a wizard's.
- **Starring a saved query leaves `updated_at`**; updating, renaming, sharing and unsharing set it. Decision 11 keeps this.
- **Stored strings lose their secrets** (`stripConnectionStringSecrets`): an SQL Server key=value string comes back with a trailing `;`, and a DuckDB option naming a secret goes.
