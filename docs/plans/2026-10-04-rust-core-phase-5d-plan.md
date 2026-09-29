# Phase 5d Implementation Plan: everything stored goes through Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** 5d-1 implemented, manual checks pending (the owner); 5d-2 not started. The owner answered Q1–Q12 on 2026-10-04 ("Answered questions"). Where the code departs from the text below, the repo is authoritative; see 5d-1's "Execution notes", "Release notes", "Checkpoint" and "Manual checks", and "Follow-ups" at the end.

**Goal:** On desktop and web, every write to the metadata file goes through a targeted Core call, and every such write tells the user's other windows and tabs what changed. Core checks the input, assigns ids and times, writes in one transaction, and on desktop writes the keychain in the same call. Nothing the GUI stores is replaced whole from a stale in-memory copy any more, and a second window or tab picks up a change without a reload. The TypeScript keeps the forms, the lists it shows, the editors and the git file projection. It no longer builds rows, ids or versions.

**Two slices.** Each ships on its own, with its own probe and checkpoint:

- **5d-1: the library and `StorageChanged`.** Connections, projects, custom labels, saved queries and their versions. Change events for every write, echo suppression and the ordering rule, with the GUI refreshing library lists. Task 1 fixes every bug the survey found that TypeScript alone can fix, the two-tab deletion and the silent failures first.
- **5d-2: state, settings, dashboards and chats.** Tabs and project state, saved workflows, settings (app-state keys, AI settings, themes, onboarding, tutorial progress, import state), dashboards and their versions, AI chats and messages, and connection overrides. They use 5d-1's write transaction, events and seam.

"The split" below gives the reasons.

**Architecture:**
- **`seaquel-workspace::library`** (5d-1) and **`seaquel-workspace::state`** (5d-2) are pure. They hold the drafts and patches, validation, how a patch applies to a stored row, the version and prune rules, and the limits. Parity fixtures recorded from today's TypeScript pin them.
- **`seaquel-storage`** gets a write transaction (`Storage::write`, `BEGIN IMMEDIATE`) and targeted queries on it. The replace-all functions stay only for their frozen fixtures. A data step drops the pre-5a connection strings.
- **Core.** It adds one `Workspace` method per write. Each reads, checks and writes inside one storage transaction, then emits `WorkspaceEvent::StorageChanged` with the kind, the ids, the writer's origin and a sequence number. Nothing in the event is a value. Secrets go to the workspace's `SecretStore` (desktop). On removal, the vault's ciphertext rows go too (web).
- **`seaquel-rpc`.** New `library` (5d-1), `settings` and `ui` (5d-2) groups, all unary, served by `dispatch_workspace` on both transports. `CoreEvent::StorageChanged` goes over `core_events` (desktop) and `/rpc/stream` (web). The storage group keeps its loads, plus the writes that stay out of scope (vault, license, shared repos). Those writes emit events too.
- **TypeScript.** A `LibraryService` seam like 5b's `QueryRunner` and 5c's `EditService`: `CoreLibrary` on desktop and web, `TsLibrary` over sql.js for the demo until phase 8. 5d-2 extends the same seam. A `ChangeFeed` applies other windows' changes to the view models (Decision 18). `PersistenceManager` shrinks in 5d-1 and goes in 5d-2.

**Tech Stack:** Rust (`seaquel-types`, `seaquel-storage`, `seaquel-secrets`, `seaquel-workspace`, `seaquel-core`, `seaquel-rpc`, `seaquel-server`, `src-tauri`), TypeScript/Svelte 5, the Node proxy, vitest, sql.js for the recorder and the demo.

**Inputs:**
- The design doc: "Core, workspaces and state" (storage ownership, `StorageChanged`, secrets, GUI state), "The RPC surface for GUIs", phase 5 in "Migration plan", "Phase 5c cost" and its "What this means for the next slice".
- The phase 5c plan and effort log (structure and the estimating basis), and the 5b plan's history switch (Decision 13, Task 3), the model for moving off replace-all saves.
- A read-only survey of every manager and store that writes storage, the storage queries behind them, and the event plumbing, checked on 2026-10-04 (below).

**Naming.** `library` holds the user's documents, `settings` the per-user records and `ui` the per-project view state. The design doc's `ConnectionService`, `ProjectService`, `DashboardService` and `UiStateStore` map onto them.

---

## What the code shows

All line numbers are as of `1cf3ddb`. The owner's unrelated changes are taken as the current state. The one this plan touches is that saved connections no longer prefetch keychain passwords at startup (`connection-manager.svelte.ts:153-156`).

### The library (5d-1)

1. **Connections are saved one row at a time, by upsert of the whole row.** `PersistenceManager.persistConnection` (`persistence-manager.svelte.ts:714-791`) builds the whole `PersistedConnection` from the in-memory object and calls `connections.save`. That upserts the row and replaces its labels (`crates/seaquel-storage/src/queries/connections.rs:87-133`). Callers:
   - `add` (`connection-manager.svelte.ts:289-391`), with an id it makes itself (`:294`);
   - `connectExisting` (`:446-520`), after every reconnect;
   - `update` (`:526-573`) and `toggleLocalOnly` (`:844-871`);
   - the label calls (`label-manager.svelte.ts:62-136`) and `setConnectionAIModel` (`hooks/database.svelte.ts:461-474`);
   - the imports (item 10).

   Because every save writes every field, a field left off the object is cleared. The load carries each field over for that reason (`connection-manager.svelte.ts:194-196`).
2. **Secrets are written from TypeScript, after the row.** `persistConnection` saves the row, then sets or deletes keyring entries by the save flags (`:760-786`). On desktop that goes through `core_call`'s `secret` group. On web it goes to the vault, which encrypts in the browser and stores ciphertext in `user_credentials` (`services/keyring.ts:1-80`). Removal deletes the row, then the entries (`persistence-manager.svelte.ts:793-814`).
3. **`persistConnection` and `removePersistedConnection` never throw.** Both wrap their work in `withErrorHandling` (`:724`, `:794`). That catches, toasts and returns `{ok: false}` (`src/lib/errors/handler.ts:79-100`), and no caller reads the result. So:
   - `add`'s vault-cancel rollback (`connection-manager.svelte.ts:372-384`) can't run;
   - a failed save leaves the connection only in memory;
   - a failed removal takes it out of memory anyway;
   - the imports count failures as successes (`tableplus-import-dialog.svelte:57-61`).
4. **The pre-5a string migration runs in TypeScript on every load** (`connection-manager.svelte.ts:205-219`). Core's builder has no such rule (`crates/seaquel-workspace/src/connections.rs:149-297`). So the MCP server connects with the stale string on a file the app hasn't loaded since 5a.
5. **Projects are saved by upserting the whole list.** `persistProjects` (`persistence-manager.svelte.ts:345-365`) calls `projects.saveAll`. That upserts each project and deletes and re-inserts its `project_labels` (`projects.rs:66-103`), but never deletes a project. Removal is `projects.remove`, which cascades except on files that started on `v2026.4.5-beta.1` (`projects.rs:105-115`). `ProjectManager.remove` (`project-manager.svelte.ts:369-406`) removes each connection, then the state, then the project, in separate calls.
6. **Custom labels live on the project** (`project-manager.svelte.ts:450-521`). `removeCustomLabel` strips the label from connections in memory only (`:490-498`). `connection_labels` has no foreign key to `project_labels` (`schema.rs:102-108`). The three predefined labels are a TypeScript constant (`types/project.ts:33-37`).
7. **Saved queries are saved by replacing the project's list, on a 500 ms timer.** Every change in `SavedQueryManager` (`saved-queries.svelte.ts:35-286`) schedules the project save. That save calls `savedQueries.saveAll` whenever the project's queries are loaded (`persistence-manager.svelte.ts:446-454`), and `saveAll` deletes every stored query not in the list (`saved_queries.rs:58-101`).
8. **Query versions are built in TypeScript, outside the save.** Numbers come from the in-memory list (`saved-queries.svelte.ts:60-62`). diff-match-patch writes a keyframe every 10 (`utils/query-versions.ts:7-37`). The version is inserted at once (`:87-96`), and the prune plan is computed in TypeScript (`storage/rust-client.ts:412-421`, `utils/version-prune.ts:20-45`) with `query_version_limit` (default 100, 0 keeps all). The query text follows 500 ms later. Every save of a linked query adds a version, changed text or not.
9. **Reads.** Connections and projects load all at once; saved queries and versions load per project on activation (`state-restoration.svelte.ts:260-280`).
10. **Imports build rows in the dialogs.** TablePlus and DBeaver (`tableplus-import-dialog.svelte:22-70`, `dbeaver-import-dialog.svelte:22-68`) make the id `conn-${host}-${port}` (`services/tableplus-import.ts:108-111`). They skip a row that exists and don't add it to the connection order. Shared-project imports (`project-manager.svelte.ts:527-691`) de-duplicate project names (`:531-538`).
11. **Shared projects and git stay in TypeScript** (Q7). The YAML and `.sql` projections are written with `plugin-fs` and named by `nameToFilename` (`services/config-file-parser.ts:335-343`, `shared-repo-manager.svelte.ts:866-1011`, `shared-query-manager.svelte.ts:56-101`). `shared_repos` is saved whole (`persistence-manager.svelte.ts:827-839`).
12. **MCP and the CLI only read.** They resolve `--connection` and `--project` once at startup (`crates/seaquel-mcp/src/exposed.rs:53-84`). They re-read rows, the `aiSettings` record and saved queries on every call (`server.rs:246`, `exposed.rs:187-218`, `tools/saved.rs:165`, `:189`).

### State, settings, dashboards and chats (5d-2)

13. **Tabs, layout and saved workflows are one replace-all save per project.** `persistProjectState` (`persistence-manager.svelte.ts:399-459`) sends the project state, every tab (query tabs with their text) and every saved workflow. `project_state::save` then does `INSERT OR REPLACE` on the state row and deletes and re-inserts `tabs` and `saved_canvases` (`crates/seaquel-storage/src/queries/project_state.rs:213-400`). The save is debounced per project (`:137-149`). There are 43 calls that schedule it, across 15 files: every tab manager, the pane manager, tab ordering, `ui-state`, the workflow manager, the dashboard manager and more. A failed load turns it off (`LoadKey` `projectState:*`, `:402-404`). The web tab-close flush isn't awaited (`hooks/database.svelte.ts:483-489`).
14. **Saved workflows are stored inside that save** (`saved_canvases`, `project_state.rs:385-400`). The workflow manager edits `savedWorkflowsByProject` and schedules the project save (`workflow-manager.svelte.ts:624-651`, `:668-680`, `:736-747`, `:757-765`). A saved workflow keeps its nodes' result rows, up to 10,000 per query node since 5c.
15. **Dashboards are upserted whole, one row per change.** `persistDashboard` (`dashboard-manager.svelte.ts:611-617`) writes the widgets and viewport JSON. Delete is `dashboards.remove` (`:91-116`). Versions are built in TypeScript like query versions but hold whole snapshots (`:560-608`), numbered from the in-memory list (`:564-565`). Errors are logged and not shown.
16. **AI chats.** Chats are upserted on a debounce (`persistence-manager.svelte.ts:601-629`). Messages are replaced whole at the end of each turn and on approvals (`:631-671`; callers `ui-state.svelte.ts:64`, `:326`, `:385`, `:394`), under the `aiMessages:*` load guard.
17. **Settings are app-state keys and single records:**
    - `aiSettings` is the whole AI settings record (providers, defaults, sharing) as JSON. It is written whole on every change (`stores/ai-settings.svelte.ts:9`, `:62-67`), with its own loaded flag (`:11-17`, `:55-60`). API keys live in the keyring. The MCP server reads this record.
    - Other app-state keys:
      - `editorKeybindingMode` (`stores/editor-settings.svelte.ts:5`);
      - `pending_changes_enabled` (`stores/pending-changes-settings.svelte.ts:3`);
      - `skippedUpdateVersion` (`stores/update.svelte.ts:4`);
      - `license_nudge` (JSON, `stores/license-nudge.svelte.ts:22`);
      - `query_version_limit` and `dashboard_version_limit` (`components/settings/general/query-history-section.svelte:10-56`);
      - `lastActiveProjectId` (`persistence-manager.svelte.ts:374-395`).
    - Themes: preferences plus user themes, saved whole on a debounce (`stores/theme.svelte.ts:270-291`). The theme editor window doesn't write storage. It emits Tauri events to the main window (`routes/(app)/windows/theme-editor/+page.svelte:62`, `:96`).
    - Onboarding (`stores/onboarding.svelte.ts:97`) and license state (`stores/license.svelte.ts:319`) are single JSON records. Tutorial progress is one row per challenge (`stores/tutorial-progress.svelte.ts:180`). Import state is one row per source (`stores/tableplus-import.svelte.ts:117`, `stores/dbeaver-import.svelte.ts:118`).
18. **Connection overrides are already targeted.** One upsert per shared connection (`persistence-manager.svelte.ts:853-897`); errors are logged, not shown.

### Events and windows

19. **Core has one workspace event.** `WorkspaceEvent::ConnectionClosed` (`crates/seaquel-core/src/workspace.rs:150-159`) goes to every subscriber (`:384-413`) and reaches the GUIs as `CoreEvent::ConnectionClosed` (`crates/seaquel-rpc/src/db.rs:323-345`).
    - Desktop: `pump_events` sends every event to every webview sink (`src-tauri/src/lib.rs:198-201`, `:236-246`), registered per webview label through `core_events` (`:469-480`).
    - Web: a per-user hub sends every event to each of the user's `/rpc/stream` sockets (`crates/seaquel-server/src/workspaces.rs:162-196`, `:534-545`).
    - TypeScript: `CoreClient.events` only knows `connectionClosed` (`src/lib/core/client.ts:84-89`).
20. **Writers today.**
    - Desktop has one main window. The theme editor and log viewer are child windows that don't write storage (`utils/child-window.ts`).
    - The CLI opens the file read-only.
    - On web, **every browser tab of a user is a writer** on the same workspace, each with its own in-memory copies.
    - Nothing ties a write to the window that made it. Node forwards only `content-type` and `X-Seaquel-User` to Rust (`src/routes/api/rpc/+server.ts:68-72`).
21. **Web ownership holds.** Each user's file is `DATA_DIR/users/<id>/meta.db`, and the id is checked before it becomes a path (`crates/seaquel-server/src/workspaces.rs:96-121`, `:426-430`). Every id a call carries resolves only inside that file, and the vault's ciphertext lives there too. Nothing caps how much a user stores beyond `/api/rpc`'s 20 MiB body.

### Bugs and gaps the survey found

Data loss, first:
- **Two web tabs delete each other's saved queries.** Tab B saves its project for any reason (a tab switch schedules it), and `saveAll` deletes the queries tab A made since B loaded (items 7, 13). The same goes for custom labels (item 5), open tabs, layout and saved workflows (item 13), the AI settings record (item 17) and user themes.
- **Saves and removals fail silently** (item 3), and so do dashboard, override and chat writes (items 15, 16, 18).
- **Closing a web tab can drop the last 500 ms of changes**: saved queries, tabs, workflows, chats (the flush isn't awaited).
- **Dashboard stars are never saved.** `toggleDashboardStarred` only schedules the project save, which doesn't write dashboards (`dashboard-manager.svelte.ts:426-436`).
- **The dashboard version limit setting does nothing.** Versions are pruned at a constant 100 (`dashboard-manager.svelte.ts:590`). The setting is stored (`query-history-section.svelte:56`) and never read. The two limits also mean different things at 0: 0 keeps every query version and deletes every dashboard version (`storage/client.ts:133-137`, `:192`).
- **Version numbers collide.** Query and dashboard versions take their numbers from the in-memory list. After a failed versions load (no guard, `persistence-manager.svelte.ts:524-531`), or from a second tab, the insert fails on the unique constraint, and only a log line says so. A new query saved twice within 500 ms inserts a version before its row exists.
- **Removing a custom label leaves its id on connections** (item 6).
- **Imports skip connections that share a host and port** (item 10), and don't add them to the connection order.
- **Renaming a shared saved query duplicates it.** The old `.sql` file stays (`saved-queries.svelte.ts:112-120`, `query-tabs.svelte.ts:88-104`), and the next reconcile makes it a new query (`shared-query-manager.svelte.ts:156-195`).
- **Names with no Latin letters share a file** in a shared repo (`nameToFilename` gives `untitled`).
- **Project removal isn't atomic**, and misses saved queries and dashboards on beta-era files (item 5).
- **The MCP server can use a pre-5a string** (item 4).
- **Removal callers don't await.** `components/sidebar/manage/connections.svelte:110` and `components/empty-states/connection-card.svelte:86` don't await removal, so once it can fail the rejection is unhandled.
- **Latent:** sharing, updating and unsharing a connection write into the active project's repo, not the connection's (`shared-repo-manager.svelte.ts:867-873`, `:923-929`, `:971-977`).

---

## Answered questions (2026-10-04)

The owner answered Q1, Q3, Q5 and Q6 directly and took the recommendation on the rest. Each question keeps the options that were weighed, so later changes start from them.

### Q1. Which entities move?

Options were: the library only (A); A plus tabs and project state (B); A plus dashboards and chats (C); connections only (D).

**Answer (owner, asked): everything stored.** That means the library, tabs and project state, settings (app state, AI settings), dashboards, AI chats and connection overrides, plus saved workflows, which are stored inside the project state (item 14).

Shared repos and git stay in TypeScript (Q7). Three stores stay as they are, since moving them fixes nothing:
- **vault state and credentials:** browser crypto; Core only deletes ciphertext on removal;
- **license state:** written only by the licensing flow, one record;
- **shared repos:** Q7.

Their writes still emit events (Decision 16).

### Q2. Does Core own ids and validation?

**Answer (taken as recommended): yes.**
- Core assigns ids and times, keeping today's id shapes, and validates.
- Two ids stay fixed: the default project (`default-seaquel`, created by `projectEnsureDefault`) and the demo's `demo-connection`.
- Engine-specific checks beyond the type stay at connect time, since a saved row may be incomplete.

### Q3. Duplicate names

**Answer (owner, asked): refused with `NAME_TAKEN`.**
- Names are compared after trimming and Unicode case-folding.
- Where: connections within a project, saved queries within a project and folder, projects, dashboards within a project, and custom labels within a project.
- Rows that already share a name stay. Imports add " (2)": Core picks the free name (Decision 13).

### Q4. Moving off replace-all saves

**Answer (taken as recommended):** each slice switches its entities in one change, as 5b did for history.
- Writes go out at once instead of after 500 ms, and no rows are migrated.
- The storage functions stay for the frozen fixtures; their `StorageRequest` variants go.
- The one exception is a window's view state, which is replaced whole by design: it belongs to that window alone (Decision 22).

### Q5. `StorageChanged`

Options were: no events yet (A); in-process events (B); B plus `data_version` polling (C).

**Answer (owner, asked): yes, in 5d**, as in-process events (B).
- Core emits one for every stored write: the kind, the ids and the origin, never values.
- The user's other windows and tabs refresh what changed: desktop windows over `core_events`, web tabs over `/rpc/stream`.
- The CLI and MCP stay read-only, so there is no second process to poll. C waits for phase 7's `seaquel conn add`.
- The design is in Decisions 16–18.

### Q6. Secrets on save

**Answer (owner, asked): written inside the Core call**, as recommended.
- On desktop, the keychain is written in the call, secrets first, then the row.
- On web the vault stays in the browser. It is unlocked before a create, so a cancel saves nothing. Core deletes the vault's rows on removal.

### Q7. Shared projects and git

**Answer (taken as recommended): stay in TypeScript.** The reconcile writes through the new calls. Task 1 fixes the rename that duplicates a shared query.

### Q8. Imports

**Answer (taken as recommended): parsing stays; creation goes through Core.** Duplicates are matched on type, host, port, database and user.

### Q9. The demo

**Answer (taken as recommended): a `LibraryService` seam**, with `TsLibrary` over sql.js. 5d-2 extends it.

### Q10. Versions

**Answer (taken as recommended):**
- Core writes query versions as keyframes: the previous text whole, numbered inside the transaction, only when the text changes.
- Pruning keeps back to the nearest keyframe, so no diff code is needed in Rust.
- Dashboard versions are already whole snapshots; Core numbers and prunes them the same way.

### Q11. Web limits

**Answer (taken as recommended): per-interface limits, none on desktop**, extended to the 5d-2 entities (Decision 15 and Decision 27).

### Q12. Open tabs across windows

Two web tabs of one user, or later two desktop windows, share one stored set of open tabs, pane layout and active tab per project. Today the last writer wins, and nobody sees the other's tabs until a reload.

- **A. Shared, last writer wins, not live-synced.** Core stores the view state per project; a window refreshes it only on opening or switching to the project.
- **B. Per window.** The view state is stored per window as well as per project, keyed by an id the window keeps. That needs a new table, a rule for what a new window starts with, and cleanup of windows that never come back.
- **C. Shared and live-synced.** Opening a tab in one window opens it in the other.

I recommended A.

**Answer (owner, asked, 2026-10-04): B, per window.** Each desktop window and each web browser tab keeps its own open tabs and layout. The design is in Decision 22. The documents a tab points at (saved queries, dashboards, workflows, chats) are still shared and synced by events.

---

## Decisions (2026-10-04)

Settled with the answers above.

### Shared by both slices

#### 1. Ids, times and validation in Core (Q2, Q3)

- Ids keep today's shapes: `conn-`, `project-`, `label-`, `saved-`, `ver-`, `dashboard-`, `dver-`, `workflow-`, `chat-` and `msg-`, each followed by a uuid. The executor reads the existing prefixes from the code and keeps them. Times come from the `Executor` (`iso_timestamp`).
- Validation happens before anything is written:
  - names are trimmed and non-empty, and no string may hold a NUL (`INVALID_ARGUMENT`);
  - `NAME_TAKEN` (Q3) names the other row's id (the first in rowid order when several already share the name). Since the 5d-1 probe fixes the key is stored (`name_key` columns on connections, projects and saved queries, migration `0001_name_keys.sql`, backfilled by the `backfill_name_keys` data step), so the check is an index search, not a fold of every name in the project; labels, which live in the project's JSON, still compare in memory. Names compare by `name_key`: trimmed, NFC-normalised, then fully Unicode case-folded, so `Straße` equals `STRASSE` and NFC and NFD forms are equal. Task 4 picks a small crate for normalisation and folding (it must build for wasm32). A saved query's NULL folder and `""` are the same folder. A rename to the row's own name in another case isn't a clash, and rows that already share a name can still be patched (2026-10-04, Task 2 review);
  - a missing parent or row is refused: `PROJECT_NOT_FOUND`, `SAVED_CONNECTION_NOT_FOUND` (existing), `SAVED_QUERY_NOT_FOUND`, `LABEL_NOT_FOUND`, `DASHBOARD_NOT_FOUND`, `WORKFLOW_NOT_FOUND`, `CHAT_NOT_FOUND`;
  - each entity has its own shape rules, in its slice's decisions.

#### 2. Patches, not rows

An update carries only what changes. A field left out is kept, and `null` clears a clearable field. The patch is applied to the row read inside the transaction, so two windows changing different fields both land. Whole-value fields (a dashboard's widgets, a workflow's data, the view state) are replaced whole, and the last writer wins on them.

Note (Task 6 review): a connection's `labelIds` is one such whole value. A patch carries the whole list, so two windows adding different labels to one connection at once end with the last writer's list.

#### 3. One write transaction per call

`Storage::write` gives a `WriteTx` (`BEGIN IMMEDIATE`). A call reads what it needs, checks it, writes and commits inside it, then emits its event (Decision 16). The GUI sends these calls through the same write queue as storage writes (`RustStorageClient.enqueueWrite`), so the order it issues them in holds.

#### 4. The switch (Q4)

Each slice moves its entities in one change. The GUI calls the new methods at once, with no debounce. The replaced `StorageRequest` variants go; their storage functions stay for the frozen repo fixtures, skipped there the way 5b's `RETIRED` set was. A load guard goes when nothing it protects is replaced whole any more.

### 5d-1: the library

#### 5. Scope

Connections (fields, labels, AI settings, the local-only flag, `lastConnected`), projects, custom labels, saved queries and their versions.

#### 6. The `library` group

Write methods:
- connections: `connectionCreate`, `connectionUpdate`, `connectionRemove`;
- projects: `projectCreate`, `projectEnsureDefault`, `projectUpdate`, `projectRemove`;
- labels: `labelCreate`, `labelUpdate`, `labelRemove`;
- saved queries: `savedQueryCreate`, `savedQueryUpdate`, `savedQueryRemove`.

Read methods, which carry the change sequence number (Decision 17): `connectionsList`, `projectsList`, `savedQueriesList`, `queryVersionsList`. The matching storage loads go.

#### 7. Connection rules

- `type` must be one of the six engines. On web, only the engines the Core was built with are allowed (`ENGINE_NOT_AVAILABLE`).
- `port` must be a whole number from 0 to 65535.
- Label ids must be the predefined three or the project's own. An unknown id is refused, where today `setConnectionLabels` drops it silently (`label-manager.svelte.ts:118-120`), a listed change.
- The connection string is stripped of passwords on every write. The TypeScript `stripConnectionStringSecrets` leaves the save path; the demo keeps it.
- `connected: true` in a patch sets `lastConnected` to now.
- A connection can't move to another project.

#### 8. Secrets (Q6)

```text
create:  secrets set (desktop) → insert row → commit;  row fails → delete the secrets just set
update:  secrets set (desktop) → patch row → commit → delete secrets whose flag is now off (best effort)
remove:  delete row (cascades) → commit → delete db:/ssh:/ssh-key: entries (desktop) or the
         user_credentials rows keyed by the id (web), best effort, logged by code
```

- Setting a secret whose flag is off after the patch is refused (`INVALID_ARGUMENT`).
- Setting one on web is `NOT_SUPPORTED` (no store there).
- On web, `add` unlocks the vault before `connectionCreate`, and writes the ciphertext after it.

#### 9. Removal

`connectionRemove`:
- deletes the row (history, chats and labels cascade) and its secrets;
- doesn't close a Core connection opened from it: the GUI disconnects first. Other windows disconnect it on the event (Decision 18).

`projectRemove`:
- refuses the last project (`LAST_PROJECT`);
- in one transaction, deletes the project's saved queries, dashboards and saved workflows by `project_id` (beta-era files have no foreign key there), then the project, whose cascade takes the rest;
- then deletes its connections' secrets;
- returns the removed connection ids.

#### 10. Labels

- Each call writes one `project_labels` row. The project row, and so its `updated_at`, is left alone (confirmed in Task 2's review; today the whole project is saved with `updated_at` now, a listed change).
- `labelRemove` also deletes the label id from every connection's `connection_labels` in the same transaction (by label id, not only the project's connections, so no row points at a missing label), and returns the connections that had it. Task 3's `strip_from_connections` follows this.
- Core knows the predefined ids; their names and colours stay in TypeScript.

#### 11. Saved queries and versions (Q10)

- Parameters are `{name, type, defaultValue?, description?}`, with a known type and unique names. Tags are a list of strings.
- `savedQueryUpdate` with a changed `query` appends a keyframe holding the previous text. It is numbered `MAX(version) + 1` inside the transaction, then pruned to `query_version_limit`, keeping back to the nearest keyframe.
- The call returns the row, the new version and the pruned ids.
- An unchanged text adds no version, a listed change.
- `starred` and `shared` are patch fields. The git projection runs after the call succeeds.
- `updated_at`: an update of the text, name, parameters or other fields, and `shared`, set it to now; a patch of only `starred` leaves it, as today (confirmed in Task 2's review).
- **Size (5d-1 probe fix).** Every version is a full keyframe, so at the default limit of 100 one saved query of `max_query_bytes` (2 MiB) could hold about 200 MiB of history, and a user of 50,000 such queries far more. On the web, `LibraryLimits::max_version_bytes` (16 MiB, Decision 15) also prunes by bytes: after appending, the newest versions stay only while their stored bytes together fit the budget (`octet_length` of the snapshot or diff, read from the record header), always at least the newest one, then back to a keyframe as before. An old diff chain (versions stored before Core's keyframes) can therefore outlast the budget: while the oldest kept version is a diff, its keyframe and every diff between stay, whatever their bytes. New saves are keyframes, so once about 8 of a large query's new versions fill the budget, the old chain falls out of the kept window and is pruned whole. Typical queries (a few KiB) keep all 100. The desktop has no budget. It bounds one query's history, not the file: 50,000 queries of 2 MiB is still 100 GB of text before any history, which is the per-user limits' business, not this one's.
- **Existing version history isn't repaired.** Task 2 found that after a prune the TypeScript diffs later versions against a wrongly resolved text. At limits 9 and above (the default is 100) that only shows in the window until a reload, but below 10 (reachable because the input's `min="10"` wasn't enforced) stored diffs were corrupted. Damaged rows can't be told from good ones, so there is no data step and no repair; Core's keyframes stop new damage, and the settings now clamp the limit to at least 10 on save (a Task 1 follow-up).

#### 12. Legacy connection strings

A data step, `drop_legacy_built_connection_strings`, ports `isLegacyBuiltString` into `seaquel-storage/src/connection_string.rs`. `initializePersistedConnections` loses its migration and its saves.

#### 12a. Secrets left in stored strings (owner, 2026-10-04: move to the keychain, then strip)

Before phase 5a the TypeScript stripped only a URL's user-info password, and since then a row is re-saved only on connect, on edit, or by the legacy-string load. So a row saved before 5a and not touched since can still hold a secret in `connections.connection_string`: libpq `password=`, `sslpassword`, DuckDB credential options, a TablePlus `+ssh` path password, a password in a fragment or in an opaque host. The phase 3 data step catches only `;`-separated `Password`/`Pwd` pairs and URL password parameters. Task 6 removes the TypeScript strip, so from then on Core's writes strip (`connections::insert`/`update` use `strip_connection_string_secrets`, an exact port). The rows already stored are handled once, by Core. **A storage data step can't do it**: it can't reach the keychain, and desktop must not strip a password before it is stored safely.

- **Storage (Task 3, done):**
  - `strip_connection_string_secrets`, the exact port;
  - `split_connection_string_secret -> Option<SecretSplit>`, which is `None` when the string holds no secret. Otherwise `SecretSplit { db, ssh, unmovable, stripped }`:
    - `db` is one database password **the driver reads**: a URL's user-info password (for a TablePlus `+ssh` URL, its path's, which becomes the database URL), or a Postgres URL's `password` parameter;
    - `ssh` is one SSH password (a TablePlus `+ssh` URL's user info);
    - `unmovable` means something else is lost by stripping: any key=value password (ADO/MSSQL, libpq), a `pwd` or `sslpassword` parameter, a `password` parameter outside Postgres, DuckDB credentials, two different passwords of one kind, a password where the URL parser doesn't look, or a string the strip blanks;
    - `stripped` is always the strip's result.

    Its `Debug` shows which parts are present, never a value;
  - `connections::with_secret_in_string`, which gives the ids and engines of the rows to upgrade, never their strings.
- **Core: a one-time upgrade when a writable workspace opens** (Task 4). Owner, 2026-10-04: **strip and list.** After it, no plaintext secret stays in `seaquel.db`.
  - **Desktop (a secret store):** for each listed row, **no keychain call inside a `WriteTx`** (a keychain prompt must never hold the write lock):
    1. Read the row on the pool and split its string.
    2. Outside any transaction, for `db` (then `ssh`): read `db:<id>` (`ssh:<id>`).
       - No entry: write the split value. If that write fails, skip the row (string and flags untouched); the next open retries it.
       - An entry equal to the split value (a crash between an earlier keychain write and its row commit): nothing to write; set the flag below.
       - A different entry: keep it and don't overwrite it; leave the flag as it is, and list the connection in the notice.
    3. Open a `WriteTx`. Re-read the string through the transaction; if it changed since step 1 (another window saved the row), skip the row this pass. Otherwise write the stripped string and the flags (`save_password = 1` for a stored or matching `db`, `save_ssh_password = 1` for `ssh`) in one row update, append the id to the notice list when it's listed (deduplicated, in this same `WriteTx`), and commit.
    - `unmovable`: the string is stripped all the same, and the connection is listed in the notice.
  - **Web (no store):** strip every listed row and list every one in the notice, each in its own `WriteTx` that re-reads the string first, as on desktop. The user enters the password on the next connect, as for any row without a saved secret.
  - **Once:** an `app_state` key (`connectionStringSecretsUpgraded`, Core's own) is written when a pass leaves nothing to retry. Until then each open runs the listing query again, which finds only the rows whose keychain write failed, so the upgrade is idempotent. The ids for the notice go in another `app_state` key (`connectionStringSecretsNotice`, a JSON id list, deduplicated and written in the row's own `WriteTx`), which the GUI reads and clears.
  - Each row changed emits a `connection` `StorageChanged` event, with no origin.
  - **Nobody hears those events on the web** (5d-1 probe finding, kept as documented behaviour): the upgrade runs inside `open_workspace`, and the server subscribes to the workspace (`forward_events`) only after it opened. That's fine, because no window can hold a list older than the upgrade from the same workspace: a window's lists come from `*List` calls made after the open, and the upgrade's writes carry the new workspace's `epoch`. A window that still holds lists from before (the server restarted or evicted the workspace) sees a new `epoch` on its next response or event, or reconnects its socket, and reloads everything either way (Decision 17). On the desktop the upgrade also runs before the GUI's first list. Subscribing before the upgrade would change nothing a window can see.
  - Nothing of a string or a secret goes to a log, an error, an event or `Debug`: ids, engines and counts only.
- **The CLI and MCP** open read-only and never run it. They keep reading a not-yet-upgraded row's string as today, until the app has opened the file.
- **Not UTF-8:** `with_secret_in_string` skips a string whose bytes aren't UTF-8 (as the data steps do), so such a row keeps any secret it holds. Only a hand-edited file can have one.

#### 13. Imports (Q8)

TablePlus and DBeaver connections are created through `connectionCreate` in the active project and added to its connection order. Duplicates are matched on type, host, port, database and user. Failures are listed.

- **Names.** An import draft (`ConnectionDraft` or `ProjectDraft`) carries its own name and `renameIfTaken: true`. Core then stores the first free name inside the transaction: the name, else `"<name> (2)"`, `"<name> (3)"`, … compared by `name_key`. That covers two drafts of one import with one name, a clash in case only, and the shared-project import (which today de-duplicates by exact name in `importFromGitRepo`). The GUI does no folding. Without the flag a taken name is `NAME_TAKEN`.
- **Local-only.** TablePlus and DBeaver imports are local-only, like a wizard's connection (a Task 1 follow-up; they were stored as shared). Shared-template imports stay shared.

#### 14. Shared projects (Q7)

Stay in TypeScript. Imported templates become `connectionCreate` calls with `sharedConnectionId`, and the reconcile's results become create and update calls.

#### 15. Web limits, library (Q11)

`LibraryLimits`, set with `CoreBuilder::library_limits`, with none on desktop, the CLI or MCP. `WEB_LIBRARY_LIMITS`:

| Limit | Value |
|---|---|
| `max_name_bytes` (names, label names, folders, each tag) | 1 KiB |
| `max_field_bytes` (host, database, user, string, SSL mode, key path, description) | 64 KiB |
| `max_query_bytes` (a saved query's text) | 2 MiB, as `WEB_RUN_LIMITS.max_text_bytes` |
| `max_list_items` (labels on a connection, parameters, tags, a project's custom labels) | 1,000 |
| `max_connections`, `max_projects`, `max_saved_queries` per user | 10,000, 1,000, 50,000 |
| `max_version_bytes` (one saved query's versions together; 5d-1 probe fix) | 16 MiB: 8 versions of a 2 MiB query, 100 of a 160 KiB one |

A call past a limit is refused with `INVALID_ARGUMENT` naming it, before anything is read. The 5c outer limits stay: 20 MiB per `/api/rpc` body, and 40 MiB of bodies in flight per user in Node and in Rust. Library calls don't join the four-at-once edit cap.

#### 16. `StorageChanged`: what Core emits (Q5)

```rust
WorkspaceEvent::StorageChanged(StorageChange)
pub struct StorageChange {
    pub kind: StoredKind,
    pub scope: Option<String>,     // the project, connection or chat the ids belong to
    pub ids: Option<Vec<String>>,  // None: reload the kind within the scope
    pub origin: Option<String>,    // the writer's window or tab; None for Core's own writes
    pub seq: ChangeSeq,            // Decision 17
}
```

- **Kinds** in 5d-1: `connection`, `project`, `label`, `savedQuery` (its versions included), `history`, and `storage` (a write through the storage group, with `ids` holding the method's key where there is one).
- **Kinds** in 5d-2: `projectState`, `workflow`, `setting` (ids are the keys), `aiSettings`, `theme`, `dashboard` (versions included), `chat`, `chatMessages` (scope is the chat), `connectionOverride`, `onboarding`, `tutorial` and `importState`.
- **Every write emits exactly one event, after its commit.** That covers library calls, storage-group writes that stay (vault, license, shared repos, and the rest until 5d-2 moves them), and history appends from `db.run` and `db.applyChanges`. A failed or refused call emits nothing.
- **Size.** At most 100 ids per event, none over 1 KiB and at most 16 KiB of ids together (`MAX_EVENT_IDS`, `MAX_EVENT_ID_BYTES`, `MAX_EVENT_IDS_BYTES`); past any of those, `ids: None`. A scope over 1 KiB widens the event to `scope: None, ids: None`, a reload of the kind everywhere. (5d-1 probe fix: the storage group's keys are the caller's, and an 8 MiB `appStateSet` key became an 8 MiB id copied into every socket's queue; 40 writes with 4 paused sockets took the server from 150 to 674 MiB.) The web server also bounds each socket's queue by bytes (`LISTENER_EVENT_BYTE_BOUND`, 8 MiB, next to the 1,024-event bound); past either, the socket closes with 1013 `EVENTS_LAGGED`.
- **No values in the event.** Ids, kinds and the origin only. A name or a key is never an id.
- **The CLI and MCP** don't subscribe to events and never write, so they don't emit.

#### 17. Ordering: the change sequence

- Each workspace keeps a `ChangeSeq { epoch, n }`. The `epoch` is the workspace's random id, and `n` a counter.
  - A write takes the next `n` while it holds the `BEGIN IMMEDIATE` lock, so `n` follows commit order.
  - After the commit it publishes `n` with `fetch_max`.
  - A read records the published `n` before its SELECTs, so what it returns is at least that new.
- Every write response and every `*List` response carries its `seq`.
- **The GUI keeps, per row, the `seq` it last applied.** It applies a write response, a list result or a refetched row only when its `seq` is higher. So a refetch that started before its own write committed can't overwrite that write's newer answer.
- **A different `epoch`** means a new workspace: the web server evicted and reopened it, or the app restarted. The GUI then reloads every list it holds.
- **Missed events.** The GUI reloads every list it holds after its `/rpc/stream` socket reconnects, or after a desktop webview reloads. It can't know what it missed.

#### 18. The GUI side: `ChangeFeed`

- **Echo suppression.** Each window or tab has an origin id: the webview label on desktop (the `core_call` command reads it itself), and a random id per page on web. The web client sends it as `X-Seaquel-Origin`, and Node forwards it after checking it against `^[A-Za-z0-9_-]{1,64}$`. It is the third header Node lets through (`src/routes/api/rpc/+server.ts:68-72`).
  - A window ignores events carrying its own origin, since its write's response already updated it.
  - A lying origin can only hide changes from that user's own window. It is not a security boundary.
  - **Not a secret** (Task 5 review): the web socket carries the page's origin in its URL (`/api/rpc/stream?origin=…`, since a browser can't set WebSocket headers), so it can appear in a reverse proxy's access log. That's acceptable: it's a random id made per page load, it names no user, and it has no use outside that user's own session (Node checks the session first, and another user's origin changes nothing for them).
- **Refresh.** `ChangeFeed` subscribes once per page (`CoreClient.events` gains `storageChanged`). It groups events per kind and scope for 100 ms, then refetches the named ids, or the list when `ids` is `None`, and applies them by the `seq` rule.
- **In-flight writes.** While the window has a write in flight for a row, it holds back applying refetches of that row until the write answers. Then the higher `seq` wins.
- **Something being edited.** A form (the connection tab in edit mode, the project settings, a label editor) keeps its own copy.
  - Note (Task 6): the app has no label edit form (labels are only created in the picker, and removed), so only the connection tab (edit and reconnect) and the project settings show the banner; there is no per-label revision tracking.
  - On a change to its row, it shows "Changed in another window" with a Reload button.
  - Saving still sends a patch of the fields the user changed, so the other window's other fields survive.
  - Everything not being edited updates in place.
- **Deleted elsewhere.**
  - A connection: it disconnects and closes its schema tabs, with a toast.
  - The active project: the window switches to another project, with a toast.
  - A saved query linked to a tab: the tab keeps its text and loses the link.
  - A label: it disappears from connections.
- **Kinds without a live view** (history of a connection not shown, a project not active) mark the list stale. The next view reloads it.

### 5d-2: state, settings, dashboards and chats

#### 19. Scope

Tabs and project state, saved workflows, app-state keys, AI settings, themes, onboarding, tutorial progress, import state, dashboards and their versions, AI chats and messages, and connection overrides.

#### 20. The `settings` group

- **App-state keys become a closed set of typed settings**, and an unknown key is refused. The set:
  - `editorKeybindingMode`;
  - `pendingChangesEnabled`;
  - `skippedUpdateVersion`;
  - `queryVersionLimit`;
  - `dashboardVersionLimit`;
  - `lastActiveProjectId`;
  - `licenseNudge`, whose JSON shape is kept as-is.

  The stored key names don't change. `settingSet { key, value }` validates each value's type.
  - **Version limits and 0.** Since 5d-1's Task 1 follow-up the settings UI clamps `queryVersionLimit` and `dashboardVersionLimit` to at least 10 on save, so it can no longer choose "keep all" (0). Core still treats a stored 0 as "keep all" (files written before the clamp, or another writer), for both limits (Decision 21).
- **AI settings are split into targeted calls** so two windows don't clobber the provider list: `aiProviderUpsert`, `aiProviderRemove` and `aiSettingsPatch` for the top-level fields. They are still stored as the one `aiSettings` record, rewritten inside the transaction from the stored copy, so the MCP server's reader (`exposed.rs:187-218`) doesn't change. The legacy-field cleanup in `ai-settings.svelte.ts:34-45` moves into Core's read.
- **Themes:** `themePreferencesSet`, `userThemeUpsert`, `userThemeRemove`, over the same stored list, rewritten in the transaction.
- **Onboarding, tutorial progress and import state:** one call each, over today's rows.

#### 21. Dashboards

- `dashboardCreate`, `dashboardUpdate` (a patch; widgets, viewport and date filter are whole values) and `dashboardRemove`.
- `dashboardUpdate` records a version of the previous state in the same transaction, numbered inside it, when the widgets, viewport, date filter or name change.
- It then prunes by `dashboard_version_limit`, fixing the ignored setting, with 0 keeping everything as for queries. Both are listed changes.
- `starred` and `shared` are patch fields, which fixes the lost star.
- The git projection stays in TypeScript.

#### 22. View state per window (Q12 B)

**What it covers.** One window's open tabs (with their text), pane layout and active ids per project, plus the window's active project. Saved workflows are not view state (Decision 23), and neither are the connection order and active connection. Those two stay in `project_state`, shared by the project's windows as today: they are how the project's sidebar looks, not what a window has open. They get their own call, `projectSidebarSet { projectId, connectionOrder?, activeConnectionId? }` in `library` (a patch, kind `project`), and the legacy mirror below keeps whatever that row holds.

**Window identity.** A window id is `^[A-Za-z0-9_-]{1,64}$`, the same form as the origin (Decision 18), and it is the origin.
- **Desktop.** The webview label. The main window's label (`main`) is the same after every restart, so it gets its tabs back.
  - Today the only other windows are the theme editor and the log viewer, which hold no project tabs and never save view state.
  - If the app later opens more app windows, their opener gives each a label (`main-2`, …), and a label reused after a restart gets that window's last state back. Nothing more is needed for 5d.
- **Web.** A per-tab id, `win-<uuid>`, kept in `sessionStorage`, so a reload keeps its tabs and a new browser tab starts fresh.
  - Browsers copy `sessionStorage` when a tab is duplicated (and on "reopen closed tab"), so two live tabs can start with one id. At load, a tab announces its id on a `BroadcastChannel`. If another live tab answers holding it, the newer tab makes a new id and starts as a new window (below).
  - The check takes up to 100 ms before the first view-state load.

**What a new window starts with.** Options:
- **a copy of the project's most recently used window**, its tabs, text and layout; or
- **empty**, with the starter tabs a project with no state gets today.

**Recommendation and decision: the copy.** A new tab or a first launch after the upgrade then shows what the user last had, which is what they see today. Empty would make every new browser tab look like a reset. The copy is independent from then on.
- "Most recently used" means the window row with the latest `updated_at` for that project.
- A window's active project, when the window is new, is the most recently used window's active project, else `lastActiveProjectId`.

**Storage: a numbered migration, `0002_window_state.sql`** (expand-only; the second file in `migrations/`, after 5d-1's `0001_name_keys.sql`).

```sql
CREATE TABLE IF NOT EXISTS windows (
  window_id TEXT PRIMARY KEY,
  active_project_id TEXT,              -- no foreign key: a removed project just falls back
  updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS window_state (
  window_id TEXT NOT NULL REFERENCES windows(window_id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  state TEXT NOT NULL,                 -- JSON: tabs with their text, layout, active ids
  updated_at TEXT NOT NULL,
  PRIMARY KEY (window_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_window_state_project ON window_state(project_id, updated_at DESC);
```

- The user is the file: a web user's `meta.db` holds only their windows, so no user column is needed.
- `state` is one JSON blob, the design doc's `ui_state` blob, which spares one table per tab type. Its shape is today's `PersistedProjectState` minus the saved workflows, the connection order and the active connection. It is written and read byte for byte (`RawValue`).
- The migration runs on the beta-era baseline too (the migrations README's rule).

**Existing users lose nothing.**
- The first load of a project in a window with no row falls back in order:
  1. the most recently used window's row;
  2. today's `project_state` and `tabs` rows;
  3. empty.

  So the first window after the upgrade gets exactly today's tabs, with no data step.
- **Older releases keep working.** Every view-state save also writes the legacy `project_state` row and `tabs` rows for that project, in the same transaction, as today's `project_state::save` does (without `saved_canvases`). An older release opening the file sees the most recently saved window's tabs, as it would today. The cost is today's write.

**Cleanup, bounded.** Inside each view-state save's transaction, after the write:
- delete window rows not used for 30 days;
- keep at most `max_windows` per user (web 50, desktop 20), the most recent;
- keep at most `max_window_states_per_project` (web 20, desktop 20), deleting the oldest `window_state` rows past them;
- never delete the saving window, or on desktop `main`.

Each step is one indexed `DELETE`, so the work per save is bounded. The legacy rows are never pruned. The windows limits join `StateLimits` (Decision 27).

**Calls** (the `ui` group):
- `windowStateLoad { windowId, projectId }` returns the state, whether it was copied (and from where: `window`, `legacy` or `empty`), and the `seq`. A copied state is written as this window's row at once, so it doesn't change under the window before its first save.
- `windowStateSave { windowId, projectId, state }` replaces this window's row and the legacy mirror.
- `windowActivate { windowId, projectId }` sets the window's active project and `updated_at`.
- `windowForget { windowId }` is sent on `pagehide` for a web tab that is closing rather than reloading. It is best effort, because a browser can't tell those apart: a reload loses nothing, since the row is copied back as "most recent". The 30-day prune catches what it misses.
- The window id must equal the call's origin (Decision 18). A call naming another window's id is refused (`INVALID_ARGUMENT`), so one tab can't overwrite another's view.

**Events.**
- A view-state write emits `projectState` with `scope` the project and `ids` the window id, like every write (Decision 16).
- `ChangeFeed` applies a `projectState` event only when its id is this window's own id and its origin isn't. That happens only when the duplicate-id check hasn't finished yet. Every other window ignores it, so there is no cross-window reload.
- A change to the connection order still goes to every window of the project, as a `project` change.

**The rest of today's behaviour stays.**
- The 500 ms debounce per window and project.
- The `projectState:*` load guard, now keyed by project within the window.
- On web, a pending save is sent from `pagehide` with `fetch(…, {keepalive: true})`, so closing the tab doesn't drop it. Browsers cap a keepalive body at 64 KiB; past that, the state is saved on each change of a tab's text instead of only on close.

#### 23. Saved workflows

- Saved workflows leave the project state: `workflowCreate`, `workflowUpdate` (the whole workflow JSON), `workflowRemove` over `saved_canvases`, one row each.
- `project_state::save` stops touching `saved_canvases`. The frozen fixture's function keeps its old behaviour.
- Result rows stay in the saved JSON, capped on web (Decision 27).

#### 24. AI chats

- `chatCreate`, `chatUpdate` (title, timestamps) and `chatRemove`.
- `chatMessagesPut { chatId, messages }` upserts the listed messages by id instead of replacing the chat. It is sent at the end of a turn and on approvals, as today, with only the messages that changed.
- `chatMessagesRemove { chatId, ids }` is for anything the UI deletes.
- The `aiMessages:*` load guard goes.
- An open chat that is streaming ignores events for itself until the turn ends, then refetches.

#### 25. Connection overrides

`overrideSave` and `overrideRemove` go through `library`. Their errors are shown instead of logged.

#### 26. The demo

`TsLibrary` grows the 5d-2 methods over the same sql.js repositories. Its events are local: it emits nothing, since the demo is one page.

#### 27. Web limits, 5d-2

`StateLimits`, set per interface, with none on desktop:

| Limit | Value |
|---|---|
| a window's view state per save (tabs with their text, layout) | 8 MiB; one tab's text 2 MiB; 500 tabs |
| windows (Decision 22) | 50 per user, 20 window states per project, unused for 30 days pruned |
| a saved workflow's JSON | 16 MiB; 1,000 per user |
| a dashboard's widgets, viewport and filter together | 4 MiB; 1,000 per user |
| an AI message's content | 1 MiB; 5,000 messages per chat; 10,000 chats per user |
| a setting's value, the AI settings record, one user theme | 256 KiB; 200 user themes; 50 AI providers |

The probe measures them.

#### 28. Logs

Activity names, ids, counts and codes only. Every draft, patch and params type has a hand-written `Debug` that shows ids, kinds and which fields are present, never names, hosts, users, strings, query or tab text, widget JSON, message content, settings values or secrets. Each slice has a `capture_logs` test with canaries in each field. Events are never logged.

---

## The split

**Recommendation: two slices, 5d-1 then 5d-2**, each with its own fixtures, probe, checkpoint and manual checks.

- **5d-1 ships the risky new machinery on few kinds.** The write transaction, the events, the `seq` rule, the origin header through Node and the `ChangeFeed` are all new. Proving them on connections, projects and saved queries (few rows, clear forms) lets the probe find their problems before 43 tab-save calls depend on them.
- **5d-1 ends the worst loss.** The two-tab deletion of saved queries and labels, the silent failures, and the MCP string are fixed when 5d-1 ships, not after both slices.
- **5d-2 is mostly GUI churn**, which is a different risk: 15 files of debounced saves, seven stores, the dashboards and the chats. Its storage and Core work repeats 5d-1's patterns.
- **5d-2 carries the per-window view state** (Q12): a numbered migration, window identity and pruning, none of which 5d-1 needs.
- **One slice would be about twice 5c**, with a single probe at the end. 5c's lesson was that the probe and its fixes are the largest item. Two probes find problems while they are still small.

The cost is one extra checkpoint and docs pass, about 0.6 h.

---

## The wire and the API

New Rust types live in `seaquel-workspace::library` and `::state` (serde, `ts-rs` behind the `ts` feature), re-exported as `seaquel_core::domain::{library, state}`, and wrapped by `seaquel-rpc`. Rows returned are today's `seaquel_types::storage` types, unchanged.

The wire rules follow 5a–5c:
- `method` comes before `params` at every level, and bodies are parsed from the raw bytes;
- a method with no params leaves `params` out;
- optional fields use `skip_serializing_if` with `ts(optional)`, and `Clearable` fields use `ts(optional = nullable)`.

### Shared

```rust
/// Absent: keep. `null`: clear. A value: set.
pub type Clearable<T> = Option<Option<T>>;

/// Decision 17.
pub struct ChangeSeq { pub epoch: String, #[ts(type = "number")] pub n: u64 }
pub struct Seqd<T> { pub value: T, pub seq: ChangeSeq }       // list and write results

// crates/seaquel-storage
impl Storage { pub async fn write(&self) -> Result<WriteTx, StorageError>; }   // BEGIN IMMEDIATE

// crates/seaquel-core
pub struct WriteOrigin(Option<String>);        // from the transport; never logged
impl Workspace { pub fn change_seq(&self) -> ChangeSeq; }   // published
pub enum WorkspaceEvent { ConnectionClosed { .. }, StorageChanged(StorageChange) }

// crates/seaquel-rpc
pub enum CoreEvent {
    /* … */
    StorageChanged { kind: StoredKind, scope: Option<String>, ids: Option<Vec<String>>,
                     origin: Option<String>, seq: ChangeSeq },
}
pub async fn dispatch_workspace(core: &Core, ws: &Workspace, req: Request, origin: WriteOrigin)
    -> Result<Response, RpcError>;             // gains `origin`
```

- Desktop: `core_call` passes the calling webview's label as the origin.
- Web: `POST /rpc` reads `X-Seaquel-Origin` (Node forwards it after the format check; Rust checks it again), else none.
- The TS `CoreClient.events` handler takes the union of `connectionClosed` and `storageChanged`.

### 5d-1: `library`

```rust
// crates/seaquel-workspace/src/library.rs
pub struct ConnectionDraft {
    pub project_id: String, pub name: String,
    #[serde(rename = "type")] pub ty: String,
    pub host: String, pub port: f64, pub database_name: String, pub username: String,
    pub ssl_mode: Option<String>, pub connection_string: Option<String>,
    pub ssh_tunnel: Option<SshTunnelConfig>,
    #[serde(default)] pub save_password: bool,
    #[serde(default)] pub save_ssh_password: bool,
    #[serde(default)] pub save_ssh_key_passphrase: bool,
    #[serde(default)] pub label_ids: Vec<String>,
    pub is_local_only: Option<bool>, pub shared_connection_id: Option<String>,
    pub ai_share_schema: Option<bool>, pub ai_share_data: Option<bool>,
    pub active_ai_provider_id: Option<String>, pub active_ai_model: Option<String>,
    #[serde(default)] pub connected: bool,
    #[serde(default)] pub rename_if_taken: bool,       // Decision 13: imports; Core picks "<name> (n)"
}
pub struct ConnectionPatch {
    pub name: Option<String>, #[serde(rename = "type")] pub ty: Option<String>,
    pub host: Option<String>, pub port: Option<f64>,
    pub database_name: Option<String>, pub username: Option<String>,
    pub ssl_mode: Clearable<String>, pub connection_string: Clearable<String>,
    pub ssh_tunnel: Clearable<SshTunnelConfig>,
    pub save_password: Option<bool>, pub save_ssh_password: Option<bool>,
    pub save_ssh_key_passphrase: Option<bool>,
    pub label_ids: Option<Vec<String>>, pub is_local_only: Option<bool>,
    pub ai_share_schema: Clearable<bool>, pub ai_share_data: Clearable<bool>,
    pub active_ai_provider_id: Clearable<String>, pub active_ai_model: Clearable<String>,
    #[serde(default)] pub connected: bool,
}
/// Desktop only. Absent: keep; `null`: delete; a string: set. `Debug` shows only which.
pub struct SecretChanges { pub db: Clearable<String>, pub ssh: Clearable<String>, pub ssh_key: Clearable<String> }

pub struct ProjectDraft { pub name: String, pub description: Option<String>, #[serde(default)] pub rename_if_taken: bool }
pub struct ProjectPatch { pub name: Option<String>, pub description: Clearable<String>, pub git_repo_path: Clearable<String> }
pub struct ProjectRemoved { pub connection_ids: Vec<String> }
pub struct LabelDraft { pub name: String, pub color: String }        // #rrggbb
pub struct LabelPatch { pub name: Option<String>, pub color: Option<String> }
pub struct LabelRemoved { pub connection_ids: Vec<String> }

pub struct SavedQueryDraft {
    pub project_id: String, pub name: String, pub query: String,
    pub parameters: Option<Vec<PersistedQueryParameter>>, pub description: Option<String>,
    pub database_type: Option<String>, pub tags: Option<Vec<String>>, pub folder: Option<String>,
    #[serde(default)] pub starred: bool, #[serde(default)] pub shared: bool,
}
pub struct SavedQueryPatch {
    pub name: Option<String>, pub query: Option<String>,
    pub parameters: Clearable<Vec<PersistedQueryParameter>>, pub description: Clearable<String>,
    pub database_type: Clearable<String>, pub tags: Clearable<Vec<String>>, pub folder: Clearable<String>,
    pub starred: Option<bool>, pub shared: Option<bool>,
}
pub struct SavedQueryUpdated {
    pub query: PersistedSavedQuery,
    pub version: Option<PersistedQueryVersion>,
    pub pruned_version_ids: Vec<String>,
}
pub struct LibraryLimits { /* Decision 15; each Option<usize>, default none */ }

pub fn check_connection(row: &PersistedConnection, custom_labels: &[ConnectionLabel], limits: &LibraryLimits) -> Result<(), LibraryError>;
pub fn apply_connection_patch(row: &mut PersistedConnection, patch: &ConnectionPatch, now: &str);
pub fn check_saved_query(row: &PersistedSavedQuery, limits: &LibraryLimits) -> Result<(), LibraryError>;
pub fn name_key(name: &str) -> String;                      // trimmed, NFC, full Unicode case folding
pub fn version_prune(versions: &[VersionMeta], keep: u32) -> Vec<String>;
```

```rust
// crates/seaquel-storage/src/queries, each taking `&mut WriteTx` (reads also the pool)
connections::{get, insert, update, delete, names_in_project, count, ids_in_project}
projects::{get, insert, insert_if_missing, update, delete_with_orphans, count}
project_labels::{list, insert, update, delete, strip_from_connections}   // new file
saved_queries::{get, insert, update, delete, names_in_folder, count}
query_versions::{append_keyframe, list_meta, delete_ids}
user_credentials::remove_all_for_key                                      // gains a WriteTx form
// connection_string.rs: is_legacy_built_string; data_steps.rs: drop_legacy_built_connection_strings
```

```rust
// crates/seaquel-core
impl CoreBuilder { pub fn library_limits(self, limits: LibraryLimits) -> Self; }
impl Workspace {   // every write takes `origin: &WriteOrigin` and returns Seqd<…>
    pub async fn create_connection(&self, core: &Core, origin: &WriteOrigin, draft: ConnectionDraft, secrets: SecretChanges) -> Result<Seqd<PersistedConnection>, CoreError>;
    pub async fn update_connection(&self, core: &Core, origin: &WriteOrigin, id: &str, patch: ConnectionPatch, secrets: SecretChanges) -> Result<Seqd<PersistedConnection>, CoreError>;
    pub async fn remove_connection(&self, core: &Core, origin: &WriteOrigin, id: &str) -> Result<Seqd<()>, CoreError>;
    // create_project, ensure_default_project, update_project, remove_project,
    // create_label, update_label, remove_label,
    // create_saved_query, update_saved_query, remove_saved_query: the same shape.
    // list_connections, list_projects, list_saved_queries(project), list_query_versions(project): Seqd<Vec<…>>.
}
```

```rust
// crates/seaquel-rpc/src/library.rs
#[serde(tag = "method", content = "params", rename_all = "camelCase")]
pub enum LibraryRequest {
    ConnectionsList, ProjectsList,
    SavedQueriesList { project_id: String }, QueryVersionsList { project_id: String },
    ConnectionCreate { connection: ConnectionDraft, #[serde(default)] secrets: SecretChanges },
    ConnectionUpdate { id: String, patch: ConnectionPatch, #[serde(default)] secrets: SecretChanges },
    ConnectionRemove { id: String },
    ProjectCreate { project: ProjectDraft }, ProjectEnsureDefault,
    ProjectUpdate { id: String, patch: ProjectPatch }, ProjectRemove { id: String },
    LabelCreate { project_id: String, label: LabelDraft },
    LabelUpdate { project_id: String, label_id: String, patch: LabelPatch },
    LabelRemove { project_id: String, label_id: String },
    SavedQueryCreate { query: SavedQueryDraft },
    SavedQueryUpdate { id: String, patch: SavedQueryPatch },
    SavedQueryRemove { id: String },
}
// Request::Library, served by dispatch_workspace on both targets. StorageRequest loses
// connectionsLoadAll, connectionsSave, connectionsRemove, projectsLoadAll, projectsSave,
// projectsSaveAll, projectsRemove, savedQueriesLoadByProject, savedQueriesSaveAll,
// savedQueriesRemoveByProject, queryVersionsLoadByQuery, queryVersionsLoadByProject,
// queryVersionsInsert and queryVersionsPrune.
```

```ts
// src/lib/hooks/database/library/types.ts (5d-1; 5d-2 adds its methods)
export interface LibraryService {
  listConnections(): Promise<Seqd<PersistedConnection[]>>;
  createConnection(draft: ConnectionDraft, secrets?: SecretChanges): Promise<Seqd<PersistedConnection>>;
  updateConnection(id: string, patch: ConnectionPatch, secrets?: SecretChanges): Promise<Seqd<PersistedConnection>>;
  removeConnection(id: string): Promise<Seqd<null>>;
  // projects, labels, saved queries and versions: as the Rust methods
}
export interface ChangeFeed {
  subscribe(kind: StoredKind, handler: (change: StorageChange) => void): () => void;
}
```

New error codes and web statuses:
- `NAME_TAKEN` and `LAST_PROJECT`: 409;
- `PROJECT_NOT_FOUND`, `SAVED_QUERY_NOT_FOUND` and `LABEL_NOT_FOUND`: 404;
- `SAVED_CONNECTION_NOT_FOUND`: 404 (existing).

`src-tauri` serves the group through its storage-backed arm (`src-tauri/src/lib.rs:316-319`) and passes the webview label.

### 5d-2: `library` additions, `settings` and `ui`

```rust
// crates/seaquel-workspace/src/state.rs
pub struct DashboardDraft { pub project_id: String, pub name: String, pub description: Option<String>,
    pub widgets: Box<RawValue>, pub viewport: Box<RawValue>, pub date_filter: Option<Box<RawValue>> }
pub struct DashboardPatch { pub name: Option<String>, pub description: Clearable<String>,
    pub widgets: Option<Box<RawValue>>, pub viewport: Option<Box<RawValue>>,
    pub date_filter: Clearable<Box<RawValue>>, pub starred: Option<bool>, pub shared: Option<bool> }
pub struct DashboardUpdated { pub dashboard: PersistedDashboard,
    pub version: Option<PersistedDashboardVersion>, pub pruned_version_ids: Vec<String> }

pub struct WorkflowDraft { pub project_id: String, pub data: Box<RawValue> }   // the SavedWorkflow JSON
pub struct ChatDraft { pub connection_id: String, pub title: String }
pub struct ChatPatch { pub title: Option<String>, pub touched: bool }         // touched: updatedAt = now
pub struct SettingKey(/* closed enum, serialised as today's key text */);
pub struct AiSettingsPatch { /* the record's top-level fields except providers, each optional */ }
pub struct StateLimits { /* Decision 27 */ }

// library group, 5d-2 methods
DashboardsList { project_id }, DashboardVersionsList { project_id },
DashboardCreate { dashboard: DashboardDraft }, DashboardUpdate { id, patch: DashboardPatch }, DashboardRemove { id },
WorkflowsList { project_id }, WorkflowCreate { workflow: WorkflowDraft },
WorkflowUpdate { id, data: Box<RawValue> }, WorkflowRemove { id },
ChatsList { connection_id }, ChatMessagesList { chat_id },
ChatCreate { chat: ChatDraft }, ChatUpdate { id, patch: ChatPatch }, ChatRemove { id },
ChatMessagesPut { chat_id, messages: Vec<PersistedAIMessage> }, ChatMessagesRemove { chat_id, ids: Vec<String> },
OverridesList, OverrideSave { r#override: PersistedConnectionOverride }, OverrideRemove { shared_connection_id },

// settings group
SettingGet { key: SettingKey }, SettingSet { key: SettingKey, value: Option<String> },
AiSettingsGet, AiProviderUpsert { provider: Box<RawValue> }, AiProviderRemove { id }, AiSettingsPatch { patch },
ThemesGet, ThemePreferencesSet { light_theme_id, dark_theme_id },
UserThemeUpsert { theme: Box<RawValue> }, UserThemeRemove { id },
OnboardingGet, OnboardingSet { state: Box<RawValue> },
TutorialList, TutorialSave { lesson_id, challenge_id, state: Option<String> },
TutorialRemoveLesson { lesson_id }, TutorialReset,
ImportStateGet { source }, ImportStateSave { source, has_offered_import, last_check_timestamp },

// library group
ProjectSidebarSet { project_id, connection_order: Option<Vec<String>>, active_connection_id: Clearable<String> },

// ui group (Q12 B, Decision 22). window_id must equal the call's origin.
WindowStateLoad { window_id, project_id } -> Seqd<WindowStateLoaded { state: Option<Box<RawValue>>, copied_from: Option<CopiedFrom /* window | legacy | empty */> }>,
WindowStateSave { window_id, project_id, state: Box<RawValue> },   // also writes the legacy project_state/tabs mirror
WindowActivate { window_id, project_id }, WindowForget { window_id },
```

- JSON bodies (widgets, workflows, themes, onboarding) stay `RawValue`, byte for byte, as the storage group keeps them.
- `StorageRequest` loses the matching variants. The remaining storage writes are `vaultState*`, `userCredentials*`, `licenseSave` and `sharedReposSaveAll`, and all of them emit `StorageChanged`.

---

## Parity fixtures

Record today's TypeScript first, after each slice's Task 1, so Core is pinned against it.

- **How it records.** A vitest recorder drives the real managers and stores over the demo's `SqljsStorageClient` on an in-memory sql.js file, with a recording keyring and a recording shared-repo stub. The sql.js repositories are already pinned to the Rust ones by the frozen repo fixtures, so the rows it reads back are what Rust storage would hold.
  - Each case records the operations in, and out: the stored rows after each operation, the keyring calls, the file writes, and the errors and toasts.
  - Ids become `<id:n>` and times `<now>`.
  - The recorder is an artifact, copied in to run and then deleted:
    - 5d-1: `docs/plans/artifacts/2026-10-04-record-library-fixtures.test.ts.txt`, run with `FREEZE_LIBRARY=1`;
    - 5d-2: `…-record-state-fixtures.test.ts.txt`, run with `FREEZE_STATE=1`.
- **Where:** `crates/seaquel-workspace/tests/fixtures/library/` (5d-1) and `…/state/` (5d-2), each with a `README.md` and a `changes.json`.
- **5d-1 cases, at least 50:**
  - connections:
    - add for each type, with and without a string, SSH password and key auth, each save flag;
    - update of each field, with the AI flags set, cleared and left out;
    - labels, the AI model, local-only;
    - remove, with its cascade and keychain;
  - projects: add, rename, git path set and cleared, remove with contents, remove on a beta-era file, the last project;
  - labels: create, rename, recolour, remove while in use;
  - saved queries:
    - create, update text, update name only, update parameters;
    - star, share and unshare, delete with versions;
    - the 11th version, prune at 3 and at 0;
  - imports: two databases on one host and port, one already present, one failing; a shared-project import with a taken name;
  - legacy strings for each engine.
- **5d-2 cases, at least 50:**
  - view state: every tab type, layouts, active ids, the legacy canvas fields; the Rust replay also checks the legacy mirror rows match what today's save writes, and the first load in a new window returns today's rows (the no-loss rule);
  - workflows: create, update, delete, one with result rows;
  - dashboards: create, each patch field, star, delete, versions and prune at 0 and 3 under both settings;
  - chats: create, retitle, delete, a turn's messages, an approval;
  - settings: each key with good and bad values; the AI settings with legacy provider fields; two provider edits;
  - themes, onboarding, tutorial and import state round trips;
  - overrides.
- **`changes.json` lists each intended difference with its Decision.** The Rust replay asserts exactly these differ; a new one found in the Core task is a finding to report. Expected entries:
  - Core ids and times (compared by shape);
  - an unknown label refused; `NAME_TAKEN`;
  - a removed label's ids gone;
  - no version for unchanged text, and keyframes;
  - the keyframe-preserving prune;
  - beta-era removal;
  - a keychain failure leaving the row;
  - (5d-2) the dashboard star saved; the dashboard limit read, with 0 keeping all;
  - (5d-2) messages upserted rather than replaced (a message the TypeScript dropped from its list stays until `chatMessagesRemove`);
  - (5d-2) workflows outside the project state.
- **Replays.** Rust runs each case's operations as Core calls on a temp file and a `MemoryStore`. The TypeScript runs `TsLibrary` on the same cases, and the view models' state.
- **Events have no TypeScript to record.** They are pinned by Core tests: one event per write, after commit, `seq` increasing in commit order, none on refusal.

---

## Ground rules

5c's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules;
- parallel-agent file ownership, with small re-read edits to shared files;
- tests never touch the real keychain, data dir or `~/.ssh`;
- no secrets, names, hosts, strings, text or values in `Debug`, errors, logs or events;
- the full check list;
- effort log: `docs/plans/2026-10-04-phase-5d-effort.md`, with 5d-1 and 5d-2 rows.

### Constraints the executors must obey

- **Core builds for wasm32.** No `tokio::spawn`, `Instant` or `SystemTime` in Core crates. Times come from the `Executor`, ids from `uuid`. `seaquel-workspace::{library, state}` do no I/O and never panic on input. The change counter is an atomic, not a lock held across an await.
- **Storage rules.**
  - New queries are new `seaquel-storage` functions.
  - One numbered migration, `0002_window_state.sql` (Decision 22; 5d-1's probe fixes took `0001_name_keys.sql`), under the migrations README's rules: expand-only, working on the beta-era baseline, with a case in `tests/baseline.rs`. The read-only CLI refuses a file until the app has run it (`STORAGE_NEEDS_UPGRADE`), as the README says; the MCP server's own tests cover that.
  - The data step follows the migrations README.
  - The frozen fixtures stay as they are.
  - Everything is expand-only: older releases open the file afterwards, and keyframe versions, NULL strings, stripped labels and workflows outside the state save are all things older releases already read.
- **Secrets.** Keys only in `validate_key`'s forms. Tests use `MemoryStore`.
- **Web.**
  - The new groups join `/rpc`, with no new route.
  - Node gains exactly one forwarded header, `X-Seaquel-Origin`, checked for format. `FORWARDED_PATHS` doesn't change.
  - `dispatch_workspace` keeps refusing SSH, git and licensing.
  - The 5a–5c socket and body limits stay.
- **The UI does no domain work** on desktop and web: no ids, name checks, versions or prune plans in the managers (the demo's `TsLibrary` excepted).
- **npm through mise.** No wasm crate changes, so `SEAQUEL_WASM_PREBUILT=1` is fine once `pkg/` is current.
- **One shared `CARGO_TARGET_DIR`:** `/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`.
- **The MSSQL `tls_server_name` live tests** fail inside the Bash sandbox (-36); run the full live suite outside it for checkpoints.
- **The demo stays on sql.js.**

### Things a task could quietly skip

Reviews check each of these by name:

- a manager or store still calling a retired storage write, or `saveAll`/`replaceAllMessages`/`saveUserThemes` (`rg` in each GUI task's review);
- `crypto.randomUUID()` left in the managers for any stored id;
- a patch that sends the whole row;
- desktop secrets written from TypeScript;
- removal leaving keychain entries or vault rows behind;
- the beta-era case in `projectRemove` untested on a beta-era fixture;
- `NAME_TAKEN` folding only ASCII;
- a write that emits no event, or emits before its commit, or emits on a refusal;
- a storage-group write that stays without an event;
- `seq` taken after the commit instead of under the lock;
- the string-secrets upgrade (Decision 12a): a keychain call inside a `WriteTx`, a row update without re-reading the string, stripping on desktop before the keychain write succeeded, a different existing entry overwritten or its flag set, moving a password the driver doesn't read, overwriting an existing `db:<id>` or `ssh:<id>` entry, a save flag not set, an SSH password left in a `+ssh` string, an unmovable secret left in place, a string or password in a log, error, event or `Debug`, a second open that writes again, or the notice never shown or never cleared;
- a predefined label id (`dev`, …) passed to `labelRemove` and stripped from every connection;
- a read through `&storage` (the pool) while a `WriteTx` is open, instead of through `&mut tx`: it misses the transaction's own writes, and on a small pool it waits for the connection the transaction holds (Task 3 review);
- the GUI applying a refetch without the `seq` check, or not reloading on a new `epoch` or a socket reconnect;
- Node forwarding `X-Seaquel-Origin` without the format check;
- the web limits missing from `web_core()` and the server's test Core;
- a library call that bypasses the write queue;
- the demo left on the old managers;
- (5d-2) a view-state save accepted for a window id other than the caller's origin, a prune that can delete the saving window or `main`, the legacy mirror not written, or a first load that doesn't fall back to today's rows.

---

## 5d-1: the library and `StorageChanged`

### Order and estimates

The estimates are sized from 5c's logged per-task times, as the design doc advises. Each first pass is compared with the nearest 5c task. Review fixes are budgeted at about 20% of first passes (5c: 22%), and probe fixes at about 40% (5c: 42%).

| # | Task | First pass | Nearest 5c task | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | TS fixes: silent failures, the two-tab deletion stopgap, labels, imports, shared rename, dashboard star and limit | 0.3–0.45 h | 1 (~0.25 h), more items | — | 3 |
| 2 | Library fixtures and the recorder | 0.3–0.5 h | 2 (~0.4 h) | 1 | 3 |
| 3 | Storage: `WriteTx`, library queries, the data step | 0.3–0.5 h | 3, no live suites | — | 1, 2 |
| 4 | Core: `library`, secrets, limits, events, `seq`, origin | 0.8–1.2 h | 4 (~1.1 h) | 2, 3 | — |
| 5 | RPC `library` group, origin through both transports and Node, `StorageChanged` delivery, `types:gen` | 0.45–0.7 h | 5 (~0.8 h) | 4 | — |
| 6 | GUI onto `LibraryService`, `ChangeFeed` for library kinds, the demo's `TsLibrary` | 0.9–1.3 h | 6 (~0.8 h), plus the feed | 5 | — |
| 7 | Probe: two users, two tabs editing at once | 0.25–0.4 h | 7 (~0.25 h) | 6 | — |
| 8 | Docs, measurement, checkpoint | 0.5–0.65 h | 8 (~0.65 h) | all | — |
| | Review fixes (~20%) | 0.75–1.15 h | | | |
| | Probe fixes (~40%) | 1.5–2.3 h | | | |
| | **Total** | **~6–9 h** | | | |

The first passes add up to 3.8–5.7 h. Expect about 7 h. The riskiest tasks:
- **Task 4:** the transaction boundaries, secrets ordering, and taking `seq` under the lock.
- **Task 6:** four managers change, `add` changes order (the row exists only after Core returns its id), and the feed applies remote changes under forms and in-flight writes. Expect its review to find lifecycle issues: a create landing after a project switch, an event for a row with a write in flight, a deleted-elsewhere connection that is mid-query.
- **The probe:** two tabs writing at once is new territory.

### Task 1: Fixes in TypeScript first

This task makes the fixtures record the fixed behaviour. The two-tab deletion and the silent failures come first.

**Files:**
- **Two-tab deletion, stopgap until Task 6.** `persistence-manager.svelte.ts`: `persistProjectState` sends `savedQueries.saveAll` only when this window changed its saved queries since it last loaded or saved them, using a per-project dirty flag set by `SavedQueryManager`. A tab switch in a stale tab then no longer deletes another tab's queries. The loss remains only when both tabs edit queries, until Task 6. `persistProjects` gets the same flag for label and project edits.
- **Silent failures.**
  - `persistConnection` and `removePersistedConnection` rethrow after their toast.
  - `add` rolls back on a failed save. The web path unlocks the vault before saving, so the cancel branch works.
  - `remove` keeps the connection when the removal fails.
  - `components/sidebar/manage/connections.svelte` and `components/empty-states/connection-card.svelte` await removal and use `errorToast`.
  - `dashboard-manager.svelte.ts` `persistDashboard` and `deleteDashboard`, and the override save, show their failures with `errorToast`.
- `project-manager.svelte.ts`: `removeCustomLabel` saves the connections it changed.
- Imports: `conn-<uuid>` ids; duplicates on type, host, port, database and user; the connection added to the order with its maps; failures counted.
- `saved-queries.svelte.ts`, `query-tabs.svelte.ts`: renaming a shared query deletes the old `.sql` file first.
- `dashboard-manager.svelte.ts`: `toggleDashboardStarred` saves the dashboard; `captureVersion` prunes by `dashboard_version_limit`. What 0 means stays as today until 5d-2 (Decision 21).
- **Follow-ups from Task 2's review** (done before the fixtures were re-recorded): TablePlus and DBeaver imports are local-only (`importConnections`); clearing a project's git path takes the connections it removes out of the connection order (`setGitRepoPath`); the version limit settings clamp to at least 10 on save (`utils/version-limit.ts`, `query-history-section.svelte`). Tests: `an imported connection is local-only, like one made in the wizard`, `takes the connections it removes out of the project's connection order`, `clampVersionLimit`.

**Tests first** (they fail before the fix):
- `a stale tab's project save doesn't delete another tab's saved query` (two managers on one sql.js storage);
- `a failed save of a new connection leaves nothing in memory and disconnects`, `a cancelled vault unlock saves no connection`, `a failed removal keeps the connection`;
- `removing a custom label saves the connections that had it`;
- `two TablePlus connections on one host and port both import`, and the DBeaver one, `an import whose save fails isn't counted`;
- `renaming a shared query leaves one .sql file`;
- `starring a dashboard survives a reload`, `the dashboard version limit setting is used`.

**Run:** `mise exec -- npx vitest run src/lib/hooks/database src/lib/components src/lib/services`; `mise exec -- npm run check` 0/0; the autofixer on the changed `.svelte` files.

**Review:** no ignored `withErrorHandling` result on a save or remove path; no id built from host and port.

### Task 2: Library fixtures

**Files:** the `library/` fixtures and the recorder artifact.

**Run:** record twice, byte-identical. `git diff --stat` shows only the fixtures and the artifact. `mise exec -- npx vitest run src/lib/hooks/database` passes once the copy is deleted.

**Review:** every library method in "What the code shows" has a case. The README names each file's coverage, the recorder's commit and the normalising rules.

### Task 3: Storage

**Files:**
- `crates/seaquel-storage/src/{lib,open}.rs`: `Storage::write` and `WriteTx`.
- `crates/seaquel-storage/src/queries/{connections,projects,project_labels,saved_queries,query_versions,user_credentials}.rs`: 5d-1's functions. `project_labels.rs` is new, and `projects::save_all` calls into it.
- `crates/seaquel-storage/src/connection_string.rs`: `is_legacy_built_string`, ported from `utils/connection-string-rules.ts:50-84`.
- `crates/seaquel-storage/src/data_steps.rs`: `drop_legacy_built_connection_strings`, plus the migrations README.
- Tests: `tests/library.rs` (new) and `tests/baseline.rs` (the step on every frozen release schema).

**Tests first:**
- `a_write_transaction_serialises_two_writers` (two pools, as `tests/open.rs`);
- `update_changes_one_row_and_its_labels`, `delete_cascades_history_chats_and_labels`;
- `delete_with_orphans_removes_saved_queries_dashboards_and_workflows_on_a_beta_file`;
- `strip_from_connections_removes_the_label_from_every_connection` (by label id; changed in Task 2's review from "only that project's");
- `append_keyframe_numbers_after_the_highest`, `list_meta_reads_no_text`;
- `the_data_step_drops_exactly_the_legacy_strings`, `the_data_step_is_linear`;
- `a_read_only_open_refuses_a_file_with_the_step_pending`.

**Run:** `cargo test -p seaquel-storage`; CI clippy.

### Task 4: Core, the library and change events

**Files:**
- `crates/seaquel-workspace/src/library.rs`, `tests/library_plan.rs`.
- `crates/seaquel-core/src/library.rs` (new): the methods and `CoreBuilder::library_limits`.
- `crates/seaquel-core/src/changes.rs` (new): `ChangeSeq`, `StorageChange`, the counter.
- `crates/seaquel-core/src/workspace.rs`: `WorkspaceEvent::StorageChanged`.
- `crates/seaquel-core/src/run.rs`, `edits.rs`: history appends emit.
- `crates/seaquel-rpc/src/workspace.rs`: storage-group writes emit (by the Rust twin of `STORAGE_METHOD_KIND`).
- `crates/seaquel-server/src/lib.rs`: `WEB_LIBRARY_LIMITS`.
- `crates/seaquel-core/src/upgrade.rs` (new): the one-time move of secrets out of stored strings (Decision 12a), run when a writable workspace opens.
- Tests: `crates/seaquel-core/tests/{library,changes,string_secrets}.rs`.

**Tests first:**
- Pure:
  - `replays_every_fixture_check` with `changes.json` exactly;
  - `a_name_that_differs_only_in_case_or_spaces_is_taken` (Unicode too);
  - `a_nul_anywhere_is_refused`, `port_must_be_whole_and_in_range`, `unknown_label_ids_are_refused`;
  - `a_patch_keeps_absent_fields_and_clears_nulls`, `version_prune_keeps_back_to_a_keyframe`;
  - `limits_are_the_interfaces`, `checks_never_panic`.
- Core:
  - `replays_every_fixture`, `create_returns_core_ids_and_times`, `two_patches_of_different_fields_both_land`;
  - `a_keychain_failure_leaves_the_row_unchanged`, `a_failed_insert_deletes_the_secret_it_set`, `a_flag_turned_off_deletes_its_secret`, `a_secret_set_on_web_is_not_supported`;
  - `remove_deletes_keychain_entries`, `remove_deletes_vault_rows`;
  - `project_remove_refuses_the_last_project`, `label_remove_strips_connections_in_the_same_transaction`, `label_remove_refuses_a_predefined_id` (before `strip_from_connections`, which strips by id in every project);
  - `an_unchanged_text_adds_no_version`, `versions_are_numbered_inside_the_transaction`;
  - `web_refuses_sqlite_and_duckdb_rows`, `limits_refuse_before_anything_is_read`.
- The string-secrets upgrade (Decision 12a), on a temp file and a `MemoryStore`:
  - `a_database_password_moves_to_db_then_the_string_is_stripped` (`save_password` is 1);
  - `an_ssh_password_moves_to_ssh_then_the_string_is_stripped` (`save_ssh_password` is 1);
  - `an_equal_keychain_entry_sets_the_flag_and_strips` (the crash-between case) and `a_different_keychain_entry_is_kept_the_flag_left_and_the_row_listed` (db and ssh);
  - `no_keychain_call_holds_the_write_lock` (a `MemoryStore` whose `set` blocks while another write must still commit) and `a_row_changed_between_read_and_write_is_skipped`;
  - `a_keychain_failure_leaves_the_row_and_the_next_open_retries_it`;
  - `an_unmovable_secret_is_stripped_and_listed_in_the_notice` (desktop), and `web_strips_every_listed_row_and_lists_it`;
  - `no_plaintext_secret_is_left` (after the upgrade, `with_secret_in_string` is empty and a byte scan of the file finds no canary);
  - `the_upgrade_is_idempotent` (a second open changes nothing and writes no keychain entry) and `it_is_recorded_once_nothing_is_left`;
  - `a_read_only_workspace_never_runs_it`;
  - `a_split_row_still_connects` (the builder puts `db:<id>` into the stripped string: URL, ADO key=value, libpq, `+ssh` path);
  - `an_ssh_password_row_still_connects_through_its_tunnel` (the tunnel authenticates with `ssh:<id>`, `SEAQUEL_TEST_SSH`);
  - `no_secret_in_logs_errors_events_or_debug` (canaries in each string form).
- Events:
  - `every_write_emits_one_event_after_commit` (a subscriber sees the row when it reads on the event);
  - `a_refused_or_failed_write_emits_nothing`;
  - `seq_follows_commit_order_under_concurrent_writers` (many tasks, one file);
  - `a_read_seq_is_never_newer_than_its_data`;
  - `the_origin_is_carried`, `over_100_ids_is_a_kind_reload`;
  - `history_appends_and_storage_writes_emit`;
  - `events_carry_no_values`.
- Logs: `no_names_hosts_strings_text_or_secrets_in_logs` (`capture_logs`, canaries).

**Run:** `cargo test -p seaquel-workspace -p seaquel-storage`; `cargo test -p seaquel-core --features seaquel-runtime/tokio`; `cargo test -p seaquel-mcp -p seaquel-cli`; CI clippy, both wasm32 lines, `npm run crates:check`.

**Review:** validation before the first write; every write in one transaction; `seq` taken under the lock and published after commit; no Core lock across an await; the MCP tests unchanged; the string-secrets upgrade writes the keychain (`db:` and `ssh:`) before its row update, never overwrites an entry, leaves no plaintext secret, and is idempotent; no `&storage` (pool) call while a `WriteTx` is held (a nested write now fails after `WRITE_WAIT`, 30 s, instead of hanging); the predefined label ids refused before `strip_from_connections`; strings that aren't UTF-8 skipped by `with_secret_in_string`, so they keep any secret (noted in Decision 12a).

**Notes from Task 3's review:**
- Inside a `WriteTx`, read only through `&mut tx`. `Storage::write` queues on an in-process mutex before taking a pool connection, so a waiting writer holds no connection, but a pool read made while the caller holds a `WriteTx` still needs a second connection.
- `saved_queries::names_in_folder` treats no folder and `""` as one folder. It returns names as stored, and trimming and case-folding for `NAME_TAKEN` stay in Core (`name_key`).
- `Storage::write` on read-only storage fails at once with `STORAGE_READ_ONLY`.

### Task 5: RPC, transports and delivery

**Files:**
- `crates/seaquel-rpc/src/{library,workspace,db}.rs`: `Request::Library`, `CoreEvent::StorageChanged`, `dispatch_workspace`'s `origin`; the retired storage variants go.
- `crates/seaquel-server/src/routes/rpc.rs`: reads and checks `X-Seaquel-Origin`. The per-user hub already delivers every workspace event to each of the user's sockets. `error.rs` gets the statuses.
- `src/routes/api/rpc/+server.ts`: forwards `X-Seaquel-Origin` after the format check. Test: `rpc.test.ts`.
- `src-tauri/src/lib.rs`: `core_call` passes the webview label as the origin; `pump_events` already goes to every sink.
- `src/lib/core/{client,http,tauri}.ts`: `events` takes both event types; the web transport sends the page's origin.
- `npm run types:gen`.

**Tests first:**
- rpc:
  - wire snapshots, `Clearable` round-trips, `Debug` redaction;
  - `a_retired_storage_method_is_unknown`;
  - `storage_changed_serialises_without_values`.
- server:
  - `another_users_ids_are_not_found` (every library call, B's file unchanged);
  - `a_write_reaches_every_socket_of_that_user_and_none_of_another`;
  - `a_bad_origin_header_is_ignored`;
  - `the_web_library_limits_apply`;
  - `library_calls_log_group_method_and_code_only`.
- Node: `forwards_a_valid_origin_and_drops_a_bad_one`, `still_drops_a_client_sent_user`.
- src-tauri: `core_call_serves_library_with_the_webview_origin`, `a_write_reaches_every_webview_sink`.

**Run:** `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio`; `mise exec -- npm run cli:build && cargo test -p seaquel --lib`; `types:gen` twice with no diff the second time; `npm run check` 0/0. If removing variants breaks `check` before Task 6, stub the TS methods to reject with `NOT_SUPPORTED`, as 5c's Task 5 did.

### Task 6: The GUI onto `LibraryService`, and `ChangeFeed`

**Files:**
- New `src/lib/hooks/database/library/{types,core-library,ts-library,change-feed,index}.ts` and their tests.
- `connection-manager.svelte.ts`:
  - `add`: connect, schema, `createConnection`, then state;
  - `connectExisting`: `connected: true`;
  - `update`, `remove`, `toggleLocalOnly`, `initializePersistedConnections` (the migration goes).
- `label-manager.svelte.ts`, `hooks/database.svelte.ts` (`setConnectionAIModel`).
- `project-manager.svelte.ts`: initialisation through `projectEnsureDefault`, CRUD, labels, git imports.
- `saved-queries.svelte.ts`, `query-tabs.svelte.ts` (rename), `shared-query-manager.svelte.ts` (the reconcile's writes).
- `persistence-manager.svelte.ts`, `state-restoration.svelte.ts`: the library parts go. `LoadKey` loses `projects` and `savedQueries:*`. Task 1's dirty flag goes with the saves it guarded.
- `storage/{client,rust-client,sqljs-client}.ts`: the retired methods go; library calls share `enqueueWrite`.
- The import dialogs and `services/deep-link.ts`.
- The connection edit tab, the project settings and the label editor: the "Changed in another window" banner.
- The one-time notice for Decision 12a: read `connectionStringSecretsNotice` after load, show the affected connections by name once ("Seaquel removed secrets from these connections' saved strings; you'll be asked for the password on the next connect, and other credentials, such as DuckDB keys, need re-entering in the connection"), then clear the key. New i18n keys, through `i18n-translator`.
- New i18n keys, translated.

**Tests first (vitest):**
- `adding a connection creates it in Core and shows Core's id`; `a failed create disconnects and leaves nothing`;
- `an edit sends only the changed fields`; `a saved query is written at once`;
- `another tab's saved query appears without a reload`; `this tab's own write doesn't trigger a refetch`;
- `a refetch older than this tab's write answer is dropped` (the `seq` rule); `a new epoch reloads every list`; `a socket reconnect reloads every list`;
- `a connection removed elsewhere disconnects here`; `the active project removed elsewhere switches project`;
- `a form shows the banner when its row changes elsewhere, and its save sends only its fields`;
- `NAME_TAKEN shows the translated message`; `imports create through Core and report failures`;
- `the demo library replays the fixture cases`;
- the storage-gate, failed-load, AI and run tests pass, with the failed-load cases for saved queries and projects rewritten.

**Run:** `npm run check` 0/0; `CI=1 mise exec -- npx vitest run`; `npx oxlint --type-aware --type-check --deny-warnings`; the autofixer; `build`, `build:web`, `build:demo`; live on desktop and web: the manual checks' items.

**Notes from Task 5's review:**
- `ChangeFeed` subscribes to `CoreClient.events` and `onResubscribed` **before** its first `*List` call. `onResubscribed` runs each time the event channel starts (`initial` the first time; then after a web socket reconnect, including the server's 1013 `EVENTS_LAGGED` close of a socket that fell behind, or a desktop re-registration): the feed reloads every list it holds then. `onEventsUnavailable` (`ACCESS_LOST`, `TOO_MANY_TABS`, `EVENTS_UNAVAILABLE`) means updates stopped: show "not receiving updates" until the next `onResubscribed`.
- History events: `queryHistorySetFavorite`'s has the row id and no scope, so the feed finds the row by id; `queryHistoryAppend`'s and Core's own appends are scoped to the connection; `queryHistoryRemoveByConnection`'s is a reload of that connection's history.
- The web socket URL carries the page origin (`/api/rpc/stream?origin=…`), so a run's history event carries it too; skip it by origin like any other.

**Review:** the skip list; `rg "saveAll|crypto.randomUUID" src/lib/hooks/database -g '!*.test.ts' -g '!library/ts-library.ts'` shows no library id and no replace-all library save.

### Task 7: Probe

A separate agent runs a two-user web instance (`SEAQUEL_WORKSPACE_CAP=2`, no origin variables). It uses only the browser-facing endpoints plus two `/rpc/stream` sockets per user, and records evidence for each check:

- **Cross-user.** Every library call naming user B's ids from A's session is not found, and nothing is written in B's file (`sqlite3` on both). A's writes produce no event on B's sockets.
- **Input.** Test names and fields with NUL, quotes, `../`, 1 MiB of text, lone surrogates and case-only duplicates; ports of `1e9`, `-1`, `22.5` and `NaN`; unknown labels; a `Clearable` sent as `{}`; unknown fields; an `X-Seaquel-Origin` of 10 KB or holding newlines. Each is refused or stored as given, and nothing else changes.
- **Two tabs editing at once.** Two sessions of one user:
  - create, rename and delete saved queries, labels and connections at once, 100 rounds: nothing lost, version numbers unique;
  - each tab's lists converge within a second of the last write;
  - a tab never reloads its own write;
  - patches of different fields of one connection both land;
  - a query deleted in one tab while the other renames it ends deleted, and the renaming tab shows the not-found error.
- **Events.** Count against writes: exactly one per write, none per refusal. The `seq` rises in commit order. After eviction (`SEAQUEL_WORKSPACE_CAP=2`, a third user) the epoch changes and both tabs reload.
- **Limits.** Each web limit is refused past its bound. Create 10,000 connections and 50,000 saved queries, then time the list loads and a burst of 1,000 events to two sockets, and record the user's file size.
- **Versions.** Record the stored size after 100 saves of a 4 KB query.
- **Removal.** Remove a project with 100 connections and 1,000 saved queries: record the time, and check that no rows are left.
- **Leaks.** No name, host, user, string, query text, secret or origin value in either server log.
- **String secrets (Decision 12a).** Seed a web user's file with the pre-5a string forms (libpq `password=`, DuckDB `s3_secret_access_key`, `+ssh` path password, `#` fragment password, a URL user-info password). Open the workspace: every listed row is stripped, the notice holds their ids, no log line holds a canary, and a second open changes nothing. On desktop, the same with a test keychain, plus a `+ssh` URL with an SSH password: database passwords land as `db:<id>` and SSH ones as `ssh:<id>`, an existing entry is kept, unmovable secrets are stripped and listed, a failing keychain leaves the rows for the next open, and afterwards no canary is in `seaquel.db`.

Probe fixes are budgeted separately.

### Task 8: Docs, measurement, checkpoint

- **Docs notes (from Task 4's review):**
  - On desktop the one-time secrets upgrade (Decision 12a) runs inside `open_workspace`, so any keychain prompt it causes appears while the storage gate is waiting at startup. Its scrub (`secure_delete` on the row updates, then `VACUUM` and `wal_checkpoint(TRUNCATE)`, each retried on the next open until it succeeds) holds the write lock and stays on that path.
  - `max_list_items` also caps a project's custom labels; add it to Decision 15's table.
  - The GUI must never send `SecretChanges` on web: Core answers `NOT_SUPPORTED` to any, a delete included, and the vault stays in the browser.
- **Docs note (from Task 6's review):** on web, the vault's ciphertext is written after `connectionCreate`/`connectionUpdate` answers. If that write fails (it's shown with a toast), the row keeps `savePassword` on with no ciphertext behind it, and the next connect asks for the password as if none were saved.
- **Docs notes (from the 5d-1 probe fixes):**
  - Migration `0001_name_keys.sql` and the `backfill_name_keys` data step make `seaquel-cli mcp` refuse a file (`STORAGE_NEEDS_UPGRADE`) until the app has opened it once, like the data step below. Say so with it.
  - The `name_key` columns: what they hold, the NULL fallback, the stale-key triggers for older releases, and that changing `name_key` (a Unicode update in `unicase` or `unicode-normalization`) needs a data step that recomputes them (`crates/seaquel-storage/migrations/README.md`).
  - Every group's request params but `db`'s now refuse unknown fields (`deny_unknown_fields`, `INVALID_ARGUMENT`), and so does the request envelope. The `db` group's params still ignore them: its callers pass objects built from GUI state, and checking every one was left out.
  - `StorageChanged` size bounds and the per-socket byte bound (Decision 16), and `max_version_bytes` (Decisions 11 and 15).
- **Docs note (from Task 3):** once the `drop_legacy_built_connection_strings` data step ships, `seaquel-cli mcp` refuses a file (`STORAGE_NEEDS_UPGRADE`, `DataStepPending`) until the app has opened it once. Say so wherever the MCP setup is documented.

- **CLAUDE.md:** the `library` group, `LibraryService`, `ChangeFeed`, `StorageChanged` (kinds, `seq`, origin, echo, missed events), ids and validation in Core, patches, secrets on save, removal, versions as keyframes, the data step, `LibraryLimits`, the origin header in Node.
- **Design doc:** the status line; "Phase 5d-1 cost"; `StorageChanged` no longer reserved.
- **This plan:** execution notes and release notes. The release notes name:
  - duplicate names refused;
  - changes from other tabs appearing live;
  - imports no longer skipping connections;
  - saved queries written at once;
  - versions only on changed text;
  - the dashboard star and version limit fixed;
  - the web limits and the new header.
- **Effort log:** the totals. **The full check list.**

**Status (Task 8):** done. CLAUDE.md, the design doc's status line, storage notes and "Phase 5d-1 cost", the execution notes, release notes, checkpoint and manual checks below, the consolidated follow-ups and the effort log's totals are written. The full check list ran except the live workspace tests, which the owner asked to skip; see "Checkpoint (5d-1)". The manual checks are the owner's.

### Manual checks (5d-1)

For the owner, after Task 8.

**Setup.**
- Databases: `docker compose -f e2e/test-databases/docker-compose.yml up -d`, then `MSSQL_PASSWORD='Seaquel_Test_123!' npm run e2e:db:seed -- all`.
- Credentials: Postgres `postgres@127.0.0.1:5432/seaquel_test`, no password needed (the server trusts every login, so any saved password works). MySQL `root@127.0.0.1:3306/seaquel_test` and MariaDB `root@127.0.0.1:3307/seaquel_test`, no password. SQL Server `sa` / `Seaquel_Test_123!` on `127.0.0.1:1433`, database `seaquel_test`, trusting the certificate.
- Desktop data dir: `D="$HOME/Library/Application Support/app.seaquel.desktop.dev"`. **Before the first launch of this build**, back it up: `cp -R "$D" /tmp/sq-5c`. The checks read the file with `sqlite3 "$D/seaquel.db" "…"` (fine while the app runs) and the keychain with `security find-generic-password -s app.seaquel.desktop -a db:<id> -w`.
- The sidecar: `npm run cli:build` (gives `src-tauri/binaries/seaquel-cli-aarch64-apple-darwin`, a debug build).

**CLI and MCP** (first: they need a file this build hasn't opened yet)

- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5c src-tauri/binaries/seaquel-cli-aarch64-apple-darwin mcp </dev/null` fails with "… needs an update this program can't make … Open the Seaquel app once …" (`STORAGE_NEEDS_UPGRADE`).
- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5c npm run tauri dev`, wait for the app to load, quit. The same CLI command now exits without an error.
- [ ] With the normal data dir, add the server with `claude mcp add` from Settings → MCP (as in 5c) and ask for a row count through `run_query` on an exposed Postgres connection: rows come back.

**Desktop** (`npm run tauri dev`, on `$D`)

- [ ] **The secrets upgrade** (before launching this build on `$D`). Pick two saved Postgres connections, A and B (`sqlite3 "$D/seaquel.db" "select id, name, type from connections"`), make sure A has no keychain entry (`security delete-generic-password -s app.seaquel.desktop -a db:<A>`; "not found" is fine), and seed pre-5a strings:
  ```sh
  sqlite3 "$D/seaquel.db" "
  UPDATE connections SET connection_string = 'postgres://postgres:canary-one@127.0.0.1:5432/seaquel_test', save_password = 0 WHERE id = '<A>';
  UPDATE connections SET connection_string = 'host=127.0.0.1 port=5432 user=postgres password=canary-two dbname=seaquel_test' WHERE id = '<B>';
  DELETE FROM app_state WHERE key IN ('connectionStringSecretsUpgraded', 'connectionStringSecretsNotice');"
  ```
  Launch. A keychain prompt may appear while the app starts; allow it. Then:
  - `security find-generic-password -s app.seaquel.desktop -a db:<A> -w` prints `canary-one`;
  - `sqlite3 "$D/seaquel.db" "select id, connection_string, save_password from connections where id in ('<A>', '<B>')"` shows no password in either string, and `save_password` 1 for A;
  - the "Saved passwords moved" notice names B only; after "Got it", `sqlite3 "$D/seaquel.db" "select key, value from app_state where key like 'connectionString%'"` shows only `connectionStringSecretsUpgraded|1`;
  - `grep -a -c canary "$D"/seaquel.db*` prints 0 for every file;
  - A connects without asking for a password; B asks.
  - Restart: no notice and no keychain prompt.
- [ ] **Add, edit, remove.** Add a Postgres connection "PG one" with a saved password: its id is `conn-<uuid>` and `security find-generic-password -s app.seaquel.desktop -a db:<id> -w` finds the password. Rename it, change the port, add a label: `sqlite3 "$D/seaquel.db" "select name, port from connections where id = '<id>'"` follows each change. Turn "save password" off and save: the keychain entry is gone. Remove it: the row, its history (`select count(*) from query_history where connection_id = '<id>'`) and the keychain entry are gone.
- [ ] **Duplicate names.** In the same project, a connection named `pg ONE ` is refused with "Another connection in this project is already called "PG one"."; in another project it's allowed. A saved query `STRASSE` next to `Straße` in the same folder is refused, and allowed in another folder. The same for a second project and a second custom label with a case-only difference.
- [ ] **Projects and labels.** Make a project with a custom label, put the label on a connection, then remove the label: the connection loses it and `select count(*) from connection_labels where label_id = '<label id>'` is 0, after a restart too. Remove the project (with connections and saved queries): `select count(*) from saved_queries where project_id = '<id>'` is 0 and its connections' `db:` entries are gone. The last project can't be removed ("The last project can't be deleted.").
- [ ] **Saved queries and versions.** Save a query and change its text three times: three new rows in `select version from query_versions where saved_query_id = '<id>'`; saving it unchanged adds none. The version history of a query saved before this build still shows its old diffs. Star, rename and delete work.
- [ ] **Imports.** TablePlus and DBeaver, each with two databases on `localhost:5432` and one named like an existing connection: all import, each local-only (`select name, is_local_only from connections` shows 1), and the clash is stored as "`<name> (2)`". Importing again adds nothing.
- [ ] **Shared project.** Rename a shared query: one `.sql` file in the repo, and no duplicate after a pull.
- [ ] **Dashboards.** A star survives a restart; a dashboard version limit of 10 keeps 10.
- [ ] **The header** shows no "Not receiving updates" badge (it appears only when event registration fails; the web check below triggers it).
- [ ] **Two windows** (optional; dev only, since the app opens no second app window yet). Temporarily add `"main-2"` to `"windows"` in `src-tauri/capabilities/default.json`, run, and in the main window's devtools console: `window.__TAURI_INTERNALS__.invoke("plugin:webview|create_webview_window", {options: {label: "main-2", url: "/", title: "Seaquel 2", width: 1200, height: 800}})`. Then:
  - a saved query created in one window appears in the other within a second;
  - with window 1 on a connection's edit tab, rename that connection in window 2: window 1 shows "Changed in another window." with Reload, and saving a port change in window 1 keeps window 2's name;
  - delete a connected connection in window 1: window 2 disconnects it with a toast and closes its tabs, in every project.

  Revert `default.json` afterwards. Both windows still share one set of open tabs per project until 5d-2.

**Web** (`npm run build:web:full`, then `SEAQUEL_WORKSPACE_CAP=2 npm run start:web`, at `http://localhost:8787`). User A in one browser, user B in another. User ids: `sqlite3 auth.db "select id, email from user"`; A's file is `users/<A id>/meta.db` (both under the repo root, the default `DATA_DIR`).

- [ ] **The desktop list** (add, edit, remove, duplicates, projects and labels, saved queries and versions) for Postgres, MySQL/MariaDB and SQL Server, reading `users/<A id>/meta.db` instead of `$D/seaquel.db`.
- [ ] **The vault.** Add a connection with a saved password and cancel the vault unlock: nothing is saved (`select count(*) from connections` unchanged). Add it again and unlock: `select count(*) from user_credentials` goes up. Remove it: its `user_credentials` rows are gone.
- [ ] **Two tabs of A.**
  - A query saved in tab 1 appears in tab 2 within a second, without a reload, and switching tabs and projects in tab 2 doesn't remove it.
  - Rename a connection in tab 2 while tab 1 has its edit tab open: tab 1 shows the banner; saving tab 1's port change keeps tab 2's name.
  - Delete a connected connection in tab 1: tab 2 disconnects it with a toast.
  - Delete the project tab 2 has open: tab 2 switches project with a toast.
- [ ] **The header badge.** Open 9 tabs of A: the ninth shows "Not receiving updates from other windows" (8 sockets per user). Close another tab: the badge goes within about 30 s.
- [ ] **B sees none of A's** connections, projects or queries, and B's tabs don't change while A writes.
- [ ] **The web limits.** `python3 -c "print('x' * 1100, end='')" | pbcopy` and paste it as a connection name: refused with "The … is longer than allowed here (max_name_bytes: 1024 bytes)." `python3 -c "print('select 1;\n' * 240000)" | pbcopy`, paste into a query tab and save it: refused naming `max_query_bytes`.
- [ ] **The secrets upgrade on web.** Stop the server, then `sqlite3 users/<A id>/meta.db "UPDATE connections SET connection_string = 'postgres://postgres:canary-web@127.0.0.1:5432/seaquel_test' WHERE id = '<id>'; DELETE FROM app_state WHERE key IN ('connectionStringSecretsUpgraded', 'connectionStringSecretsNotice');"`. Start it and sign in as A: the notice names the connection, `grep -a -c canary-web users/<A id>/meta.db*` prints 0 for every file, and connecting asks for the password.

**Demo** (`npm run build:demo`, then `npm run preview:demo`)

- [ ] Create, rename and delete a project, a custom label, a saved query (with a few text changes, then its version history) and a connection edit: all survive a reload, and a case-only duplicate name is refused.

### Execution notes (5d-1)

The slice was executed task by task with subagents, Tasks 1, 2 and 3 overlapping, with a review after each task (Tasks 3 and 5 had more than one round), a probe on a two-user web instance, one round of probe fixes with its review, and this checkpoint. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-10-04-phase-5d-effort.md`; the measured cost is in the design doc ("Phase 5d-1 cost").

**What went differently from the plan**

- **Secrets in stored strings became Decision 12a** (Task 3's review). The plan assumed the pre-5a strings needed only the legacy-string step. Rows saved before 5a and not touched since can hold secrets the old TypeScript never stripped, so Task 6 couldn't simply stop stripping in TypeScript. The owner decided on 2026-10-04 to move them to the keychain and then strip and list them. That took three storage rounds (the exact strip port, checked against 66 inputs recorded from the TypeScript; `split_connection_string_secret`; only passwords the driver reads move, key=value strings are always unmovable, empty values aren't secrets) and Core's one-time upgrade. Stripping wasn't enough to get the canaries out of the file: SQLite kept old copies in untouched pages, so the upgrade runs row updates with `secure_delete`, then `VACUUM` and `wal_checkpoint(TRUNCATE)`, each with its own `app_state` key and retried on the next open (the rebuild at most three times).
- **The write lock** (Task 3 review). A queued writer used to hold its pool connection while SQLite busy-waited; writers now queue on an in-process mutex first and give up after `WRITE_WAIT` (30 s, which needed tokio's `time` feature in storage).
- **Labels** (Task 2 review). `labelRemove` strips by label id in every project, not only the label's own, so no row points at a missing label; Task 3's test was renamed to match. Label writes leave the project's `updated_at` alone, and an unknown label id on a connection is `LABEL_NOT_FOUND`.
- **Import names moved into Core** (Task 2 review): `renameIfTaken` instead of the GUI's " (2)", compared by `name_key`, which became NFC plus full case folding. TablePlus and DBeaver imports are local-only, a clearing of a project's git path takes its connections out of the order, and the version limit settings clamp to at least 10 (Task 1 follow-ups).
- **Version history isn't repaired** (Task 2 review). The recorder showed that after a prune the TypeScript stored diffs against the wrong base; the review narrowed the damage to limits 2–8. Damaged rows can't be told from good ones, so there's no data step.
- **The change sequence** publishes the highest number below which every write has finished (a small in-flight set), not a plain counter, since storage-group writes and history appends number themselves after their commit (Task 4).
- **Wire details** (Tasks 4–6). `SAVED_CONNECTION_NOT_FOUND`'s wire value stays `CONNECTION_NOT_FOUND`. ts-rs prints `optional = nullable` on `Option<Option<T>>` as `T | null | null`, so `Clearable` fields use `ts(optional)`. `RpcError` gained an optional `takenBy` in Task 6, since `NAME_TAKEN`'s id was dropped at the RPC boundary and the GUI would have had to fold names itself.
- **Event delivery** (Task 5 and its reviews). The desktop's event pump starts when the storage workspace opens, not on the first `db` call. The web hub's per-socket queue is bounded (1,024 events) and a full one closes the socket with 1013 `EVENTS_LAGGED`; the socket loop takes an event only when the outbox has room, so a `cancel` is still read, and refusals waiting for that room are capped at 64 (1008 `TOO_MANY_PENDING`). Browsers can't set WebSocket headers, so the page origin rides `/api/rpc/stream?origin=`, which Node checks and turns into `X-Seaquel-Origin`. `onResubscribed` fires on the first subscription too (`initial`), since a list can finish before the socket opens. Runs and applies carry the origin, so the running window skips its own history event.
- **The GUI** (Task 6 and its review). `add` loads the schema before `connectionCreate`, since the row's id is Core's. The demo's `TsLibrary` shares one sql.js database with the demo's storage client and keeps the fixed `demo-connection` through `putDemoConnection`. A label write's answer holds only the label, so recording its `seq` for the project hid another tab's rename; label writes and version splices now read again what they changed. Closing a removed connection's tabs edits every project's records directly (`connection-tabs-cleanup.ts`), since the tab managers work on the active project only. The app has no label edit form, so only the connection tab and the project settings show the banner. Task 1's dirty-flag stopgap went with the saves it guarded.
- **Probe fixes** (Task 7):
  - Event memory: an 8 MiB storage-group key became an 8 MiB event id copied into every paused socket's queue (150 → 674 MiB with 4 sockets). Events now carry at most 100 ids, none over 1 KiB and 16 KiB together, else a kind reload; a scope over 1 KiB widens to every scope; and each socket's queue is also bounded at 8 MiB (`LISTENER_EVENT_BYTE_BOUND`). The same run peaked at 124 MiB.
  - The duplicate-name check folded every name in the project per write (saved-query creates at 46 ms by 50,000). `0001_name_keys.sql` stores `name_key` with indexes, `backfill_name_keys` fills old rows, and triggers set a key to NULL when an older release renames a row; storage sets it again. The review of the fixes added `refill_name_keys` on every writable open (after a downgrade), and lookups that read names as bytes, so a name that isn't UTF-8 matches nothing instead of failing. The frozen schema fixtures stop at the baseline, so `tests/baseline.rs` compares against fixture plus migrations, and the test-only migrations moved to `9001`+.
  - Unknown fields: every group's params but `db`'s, and the envelope, now refuse them.
  - Versions: `max_version_bytes` (16 MiB on web) bounds one saved query's history, since every version is now a full copy.
  - Kept: the upgrade's events reach no web socket (Decision 12a explains why that's safe).

**Decisions made during execution**

- **Strip and list** (owner, 2026-10-04): secrets in pre-5a strings move to the keychain where the driver reads them, and everything else is stripped and named in a one-time notice (Decision 12a).
- **No repair of version history** (Decision 11): no data step, a clamp on the setting instead.
- **Override credentials stay in TypeScript until 5d-2**, with the overrides (Decision 25); a shared connection's override secrets are the one desktop keychain write left in the GUI.
- **The upgrade's events aren't delivered on web** (Decision 12a): no window can hold a list older than the upgrade.

### Release notes (5d-1)

For the release after 5c's. Earlier phases' notes still apply as written.

Changes you may notice:

- **Changes show up in your other windows and tabs.** A connection, project, label or saved query changed in one browser tab (or desktop window) appears in the others within about a second, without a reload. A form you're editing says "Changed in another window." and offers Reload instead of overwriting; saving it keeps the other window's changes to fields you didn't touch. A connection deleted elsewhere is disconnected, with a message, and a project deleted elsewhere is switched away from. If a tab stops receiving updates it says so in the header.
- **Duplicate names are refused.** Two connections or custom labels in one project, two saved queries in one folder, or two projects can no longer share a name, compared without regard to case or spaces at the ends (so `Straße` and `STRASSE` clash). Names that already clash stay as they are.
- **Saved queries are saved at once**, not half a second later, and two tabs editing saved queries no longer delete each other's. A save that fails now says so; so do failed connection saves and removals, which used to fail silently.
- **Version history** gets a new version only when the text changes, keeps each version as a full copy of the text, and its limit is at least 10. Older history still shows.
- **Removing a custom label** removes it from its connections for good.
- **Dashboard stars are saved**, and the dashboard version limit setting is used.
- **TablePlus and DBeaver imports** bring in every connection, including several databases on one host and port, add them to the connection list's order, name a clash "Name (2)", and are local-only, like a connection made in the wizard.
- **Saved passwords in old connection strings are moved.** The first time this release opens your data, it moves any password left in a connection string saved by an older release to the keychain (desktop) and removes secrets from the saved strings. macOS may ask once for keychain access while the app starts. Connections whose credentials couldn't be moved (for example DuckDB keys, or a password in a `key=value` string) are listed once; you'll be asked for the password on the next connect, and other credentials need re-entering in the connection. On the web every such connection asks for its password again.
- **The command line tool (`seaquel-cli mcp`) needs the app to open your data once** after upgrading. Until then it stops with "Open the Seaquel app once…".

Self-hosted web:

- **New limits on the library** (`400 INVALID_ARGUMENT`, naming the limit): 1 KiB per name, label, folder and tag, 64 KiB per other connection or query field, 2 MiB of saved query text, 1,000 items per list (labels, parameters, tags, a project's labels), and per user 10,000 connections, 1,000 projects and 50,000 saved queries. Each saved query's version history is kept to 16 MiB, newest first, so a large query keeps fewer than the version limit.
- **New error codes on `/api/rpc`:** `NAME_TAKEN` (409, with `takenBy`), `LAST_PROJECT` (409), `STORAGE_READ_ONLY` (409), `PROJECT_NOT_FOUND`, `SAVED_QUERY_NOT_FOUND` and `LABEL_NOT_FOUND` (404). Requests with unknown fields are refused with `INVALID_ARGUMENT`, except in the `db` group. The storage group's library methods are gone (only a custom client could notice).
- **A new header and socket parameter.** Browser tabs send a random per-page id as `X-Seaquel-Origin` on `/api/rpc` and as `?origin=` on `/api/rpc/stream`, so a tab can skip its own changes. Node forwards it only when it's 1–64 of `[A-Za-z0-9_-]`. The `?origin=` value can appear in a reverse proxy's access log; it names no user and is useless outside that user's session.
- **Event sockets are bounded.** A tab whose socket falls more than 1,024 events or 8 MiB behind is closed with 1013 `EVENTS_LAGGED` and reconnects and reloads; a client that keeps sending while the server waits to write is closed with 1008 `TOO_MANY_PENDING`.
- **Saved passwords (the vault).** The vault is unlocked before a connection is created, so cancelling the unlock saves nothing. The password is written to the vault after the connection is saved; if that write fails (a message says so), the connection is kept and asks for its password on the next connect.
- **Secrets in old connection strings are removed** from each user's `meta.db` when their workspace first opens. When anything was removed, the file is then rebuilt once (`VACUUM`) to drop old copies, which can take a moment on a large file.

### Checkpoint (5d-1)

The full check list, run on 2026-09-29 one step at a time on the shared `scratchpad/p5a/target`, inside the Bash sandbox, npm through `mise exec`, except the live workspace tests, which the owner asked to skip (a full run takes about 2 hours on this machine, each live test binary taking 30–60 s to start):

| Check | Result |
|---|---|
| `npm run crates:check` | pass |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, live | **not run** (owner's call). The last full live run in the effort log is the probe fixes' (~2.3 h), before the probe-fix follow-ups |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` | pass |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run types:gen`, generated types unchanged | pass (the 180 files are identical before and after) |
| `npm run check` | pass: 0 errors, 0 warnings (4,658 files) |
| `npx oxlint --type-aware --type-check --deny-warnings` (CI's lint step) | pass: 0 diagnostics in 622 files |
| `CI=1 npx vitest run` | pass: 1,829 tests in 99 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |
| `npm run cli:build`, `cargo check -p seaquel` | pass |
| `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 43 passed |

**Not run, so not checked here:** the live suites (Postgres, MySQL, MariaDB, SQL Server, DuckDB, SSH), and with them Core's live library and upgrade tests that need `SEAQUEL_TEST_*`. Run `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio` with the `ci.yml` env before committing, or let CI's engines job do it. Expect the three MSSQL `tls_server_name` tests (`a_bracketed_ipv6_host_is_dialled`, `without_it_the_tls_name_is_host`, `the_tls_name_is_tls_server_name_while_the_socket_goes_to_host`) to fail inside the Bash sandbox with `could not load platform certs: … code: -36` (macOS trust settings), the known environment issue; they pass outside it and on CI's Linux.

CI doesn't run oxfmt. `oxfmt --check` passes on CLAUDE.md, `load-guard.ts` and the effort log, and flags the design doc and this plan for their tables and list spacing, the plans' own style (as in 5b and 5c); left as they are.

**Manual checks:** pending (the owner).

**Not run:** the release workflow and a signed build.

---

## 5d-2: state, settings, dashboards and chats

Starts after 5d-1's checkpoint. The file lists below are the survey's. The executor re-checks the line numbers against the tree after 5d-1.

### Order and estimates

These are sized from 5c's per-task times, with 5d-1's own times replacing them where 5d-1 has logged them before 5d-2 starts.

| # | Task | First pass | Nearest 5c task | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | TS fixes found re-surveying after 5d-1 | 0.15–0.25 h | 1 | 5d-1 | 3 |
| 2 | State fixtures and the recorder | 0.35–0.55 h | 2 (~0.4 h) | 1 | 3 |
| 3 | Storage: the window migration and queries with pruning, dashboards, versions, workflows, messages, settings records | 0.4–0.65 h | 3, no live suites | — | 1, 2 |
| 4 | Core: `state` (dashboards, workflows, chats, overrides, settings, AI settings, themes, window view state and its fallback), limits, events | 0.8–1.2 h | 4 (~1.1 h) | 2, 3 | — |
| 5 | RPC `settings` and `ui` groups, `library` additions, `types:gen` | 0.3–0.5 h | 5, the plumbing done | 4 | — |
| 6 | GUI: window identity, the 43 project-save calls, seven stores, dashboards, chats, workflows, `ChangeFeed` kinds, `TsLibrary` | 1.15–1.7 h | 6 (~0.8 h), more files | 5 | — |
| 7 | Probe | 0.25–0.45 h | 7 | 6 | — |
| 8 | Docs, measurement, checkpoint | 0.5–0.65 h | 8 | all | — |
| | Review fixes (~20%) | 0.8–1.2 h | | | |
| | Probe fixes (~40%) | 1.55–2.4 h | | | |
| | **Total** | **~6.25–9.5 h** | | | |

The first passes add up to 3.9–5.95 h, about 0.4–0.65 h more than before Q12's answer: the migration, window identity (the `BroadcastChannel` check), the fallback load and the pruning. Expect about 7.5 h. **Both slices together: about 12.3–18.7 h; expect about 14.5 h.** The riskiest tasks:
- **Task 6:** the tab managers' saves stay debounced but lose the saved workflows. Every store changes its load and save. The chat's streaming turn meets the feed.
- **Task 4:** keeping the `aiSettings` record byte-compatible for the MCP reader while splitting its writes.

### Tasks, in outline

Same shape as 5d-1's. What each adds:

1. **TS fixes** found re-surveying after 5d-1, and the web tab-close save through `keepalive` (Decision 22), so the last 500 ms of tab changes survive.
2. **Fixtures:** the `state/` set ("Parity fixtures").
3. **Storage:**
   - `dashboards::{insert, update, delete, names_in_project, count}`;
   - `dashboard_versions::{append, list_meta, delete_ids}`;
   - `saved_canvases::{insert, update, delete, count}`;
   - `migrations/0002_window_state.sql`; `windows::{get, touch, delete}`, `window_state::{get, most_recent, put, prune}`, and `project_state::save_legacy_mirror` (the state row and tabs, keeping the stored connection order and active connection, never `saved_canvases`);
   - `ai_chats::{insert, update, delete, put_messages, delete_messages}`;
   - the settings records read and written inside a `WriteTx`.

   Tests as 5d-1's Task 3, with `project_state::save`'s frozen behaviour kept for its fixture.
4. **Core:**
   - the `state` module and methods;
   - the `aiSettings` record rewritten from its stored copy inside the transaction, pinned by `exposed.rs`'s reader test;
   - the dashboard version rule;
   - message upserts;
   - `StateLimits`;
   - one event per write.

   Tests:
   - `two_windows_editing_different_providers_both_land`;
   - `a_streaming_chats_messages_are_upserted_not_replaced`;
   - `a_dashboard_star_is_saved`, `dashboard_limit_zero_keeps_all`;
   - `project_state_save_leaves_workflows_alone`;
   - `unknown_setting_keys_are_refused`;
   - `a_new_window_copies_the_most_recent_then_legacy_then_empty`, `a_window_save_writes_the_legacy_mirror_and_keeps_the_sidebar`, `pruning_is_bounded_and_spares_the_saving_window_and_main`, `a_window_id_other_than_the_origin_is_refused`, `a_view_state_event_names_only_its_window`;
   - logs and events as in 5d-1.
5. **RPC:**
   - the two groups and the `library` additions;
   - the storage variants retired;
   - the server's `another_users_ids_are_not_found` for every new call;
   - `types:gen`.
6. **GUI:**
   - `PersistenceManager` goes. View-state saves go through `ui.windowStateSave` (still debounced), the connection order through `library.projectSidebarSet`.
   - A `window-id.ts` module: the webview label on desktop; on web the `sessionStorage` id with the `BroadcastChannel` duplicate check, and `windowForget` plus the keepalive save on `pagehide`.
   - Workflow, dashboard and chat managers call `LibraryService`.
   - `ai-settings`, `theme`, `onboarding`, `tutorial-progress`, `editor-settings`, `pending-changes-settings`, `update`, `license-nudge` and the import stores call `settings`.
   - `ChangeFeed` covers the new kinds:
     - an open dashboard reloads unless it has an unsaved local change, which shows the banner;
     - an open chat waits for its turn to end;
     - settings apply at once;
     - view state never from another window (Decision 22).
   - `TsLibrary` grows the methods.
   - Tests mirror 5d-1's, plus: `a workflow saved in one tab appears in the other`, `a theme added in one tab appears in the other`, `a tab's layout isn't moved by another tab`, `a new tab starts with the most recent window's tabs`, `a reload keeps the tab's own tabs`, `a duplicated tab gets a new window id`, `the first window after the upgrade gets today's tabs`, `a call naming another window's id is refused`.
7. **Probe:** 5d-1's checks for the new calls, plus:
   - two tabs editing one dashboard (the last writer wins on widgets, and both converge);
   - two tabs adding AI providers (both kept);
   - a chat streaming in one tab while the other deletes it;
   - a 16 MiB workflow;
   - 5,000 messages;
   - tab-close within 500 ms of a change (nothing lost);
   - 60 browser tabs opened and closed: the user holds at most 50 window rows, each save's prune stays bounded, and the legacy mirror always matches the latest save;
   - a view-state save naming another tab's window id is refused;
   - an older release (2026.9.x) opening the file afterwards shows the latest window's tabs.
8. **Docs and checkpoint:** as 5d-1's, and the design doc's "Phase 5d cost" for both slices.

### Manual checks (5d-2)

- [ ] **Desktop.**
  - Tabs, layout and the active tab survive a restart.
  - A workflow saved with results reopens with them.
  - A dashboard edit, star and version history work.
  - An AI chat's messages survive a restart.
  - Settings (key bindings, pending changes, version limits, AI providers, themes) survive a restart.
- [ ] **Web, two tabs.**
  - An AI provider added in tab 1 and another in tab 2 are both kept.
  - A dashboard renamed in tab 1 is renamed in tab 2.
  - Tab 2's open tabs don't change when tab 1 opens tabs, and survive tab 2's reload.
  - A new browser tab starts with the tabs of the one used last. A duplicated tab gets its own tabs from then on.
  - After the upgrade, the first window shows the tabs you had before it.
  - Closing tab 1 right after typing in a query tab keeps the text.
- [ ] **MCP.** The global AI sharing default set in the app is what `seaquel-cli mcp` follows.
- [ ] **Demo.** Tabs, a workflow, a dashboard and a chat survive a reload.

---

## Follow-ups (not in 5d)

Consolidated at 5d-1's checkpoint. Items marked **5d-2** belong to that slice; the rest are later.

From 5d-1:
- **5d-2: override credentials.** A shared connection's override secrets are still written to the desktop keychain from TypeScript (`shared-connection-manager.svelte.ts`); they move into `overrideSave` with the overrides (Decision 25).
- **5d-2: dashboard names.** Dashboards get `NAME_TAKEN`; give them a `name_key` column in `0002` rather than folding names per write, which the probe showed is quadratic at scale.
- **5d-2: survey the stored data.** 5d-1's added scope came from rows older releases left (secrets in strings, damaged version diffs, labels left on connections). Check project state, dashboards, chats and settings rows for the same before Task 2.
- **5d-2: the second migration** makes `seaquel-cli mcp` refuse the file again until the app has opened it; the release notes must say so again.
- **A connection's labels are one whole value.** Two windows adding different labels to one connection at once end with the last writer's list (Decision 2); a `labelAdd`/`labelDrop` pair would merge them.
- **The `db` group's params still ignore unknown fields.** Its callers pass objects built from GUI state; checking each would let it refuse them too.
- **Damaged version history stays.** Diffs stored at limits 2–8 before the clamp can't be told from good ones (Decision 11). A version that doesn't apply could be shown as "unavailable" instead of wrong text.
- **An old diff chain outlasts the web's byte budget** until about 8 new saves of a large query push it out (Decision 11).
- **`max_version_bytes` bounds one query, not the file.** 50,000 saved queries of 2 MiB is still 100 GB of text per user; a per-user byte budget would bound it.
- **The upgrade's events reach no web socket** (Decision 12a). Harmless today; if the upgrade ever runs after windows have loaded their lists, subscribe first.
- **Strings that aren't UTF-8 keep any secret** (Decision 12a); only a hand-edited file has one.
- **A failed web vault write after a save** leaves `savePassword` on with nothing stored; the next connect asks for the password. Writing the vault first would need Core's id before the row exists.
- **The "Not receiving updates" badge on desktop** appears only when `core_events` can't be registered, which can't be triggered by hand; a test hook would let the manual checks cover it.
- **A Unicode update to `unicase` or `unicode-normalization`** changes `name_key` and needs a data step that recomputes the columns (migrations README).
- **Live test start-up.** Each live test binary takes 30–60 s to start on the owner's machine, so a full live run takes about 2 hours; finding why (code signing or antivirus scanning of fresh binaries are the usual suspects) would save most of every checkpoint's wait.

Carried from the plan and earlier slices:
- **`data_version` polling** once a second process writes (phase 7's `seaquel conn add`).
- **More desktop app windows.** Labels per window are ready (Decision 22); opening a second app window is a feature of its own. Until then the desktop's two-window check needs the dev-only capability edit in the manual checks.
- **The shared-repo projection in Core** (Q7 B), and **import readers in Core** (Q8 B), for the CLI.
- **`nameToFilename`** gives every name without Latin letters `untitled`.
- **The MCP server** resolves its connections once at startup. With events available in-process only, it still needs a restart to see a new connection; polling would close that.
- **Field-level merging** for whole-value fields (dashboard widgets, workflows) instead of last writer wins.
- **The share, update and unshare paths** write into the active project's repo (latent).
- **Still open from 5b and 5c:** the row-returning statement kind, estimated totals shown as estimates, cancelling the statement in flight on the server, Node holding every `/api/rpc` body it reads.
- **Phase 8** deletes `TsLibrary` with `TsEditService` and `TsQueryRunner`.
