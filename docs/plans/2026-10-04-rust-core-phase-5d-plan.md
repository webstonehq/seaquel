# Phase 5d Implementation Plan: everything stored goes through Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** 5d-1 implemented, manual checks passed (the owner); 5d-2 planned in full (re-surveyed at `8e178a8`, see "5d-2: re-survey") and **ready to execute**; not started. The owner answered Q1–Q12 and, for 5d-2, Q13–Q19 on 2026-10-04 ("Answered questions"). Where the code departs from the text below, the repo is authoritative; see 5d-1's "Execution notes", "Release notes", "Checkpoint" and "Manual checks", and "Follow-ups" at the end.

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
- **TypeScript.** A `LibraryService` seam like 5b's `QueryRunner` and 5c's `EditService`: `CoreLibrary` on desktop and web, `TsLibrary` over sql.js for the demo until phase 8. 5d-2 extends the same seam. A `ChangeFeed` applies other windows' changes to the view models (Decision 18). `PersistenceManager` shrinks in 5d-1 and goes in 5d-2 (its shared-repo save moves into `shared-repo-manager.svelte.ts`, Task 6a).

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

Items 13–18 are the first survey, as of `1cf3ddb`. 5d-2 was re-surveyed at `8e178a8`, after 5d-1; "5d-2: re-survey" in the 5d-2 section replaces these items where they differ (35 project-save calls in 12 files, not 43 in 15; overrides are dead code; chat and message ids have no prefix).

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

### Q13–Q19. 5d-2, after the re-survey

Asked after the 5d-2 re-survey at `8e178a8`. The owner answered Q13, Q14, Q16 and Q17 directly, each as recommended, and took the recommendation on Q15, Q18 and Q19 without being asked.

#### Q13. Connection overrides

The re-survey found them dead: `SharedConnectionManager` is never constructed, so no override is loaded or saved, and its keychain writes would use the shared template's id, not the connection's. Options were: retire them (A); move them to Core as planned, still unused (B); wire the feature up (C).

**Answer (owner, asked): A, retire them.** `shared-connection-manager.svelte.ts` is deleted, the `connectionOverrides*` storage methods and `PersistenceManager`'s override functions go, and the `connection_overrides` table and its storage functions stay, so stored rows survive and older releases still open the file. CLAUDE.md's line about override credentials is corrected in Task 8 (Decision 25).

#### Q14. The active connection

It changes on every click, connect and disconnect. Options were: per window, in the view state (A); shared and applied live (B); shared, stored only (C).

**Answer (owner, asked): A, per window.** The connection order stays shared per project (Decision 22).

#### Q15. `windowForget`

`pagehide` fires on a reload too, so forgetting on it makes a reloaded tab copy another tab's state. Options were: drop it (A); forget after a delay (B); keep it (C).

**Answer (taken as recommended): A, dropped.** The 30-day and count prunes bound the rows.

#### Q16. Workflows' stored rows

A query node keeps up to 10,000 rows, and each chart node a second copy. Options were: keep the query nodes' rows, stop storing chart copies, refuse past 16 MiB on web (A); 1,000 rows per node on web (B); no rows (C).

**Answer (owner, asked): A.** Desktop is unlimited. Workflows saved before 5d-2 that hold chart copies load as they are; the next save drops the copies (Decision 23).

#### Q17. Chat history on web

Options were: a byte budget per chat (A); per user (B); counts only (C).

**Answer (owner, asked): A, 64 MiB per chat on web.** Past it, saving the next turn is refused with a message saying to start a new chat. Desktop is unlimited (Decisions 24 and 27).

#### Q18. Themes across windows

**Answer (taken as recommended): live.** A theme picked or edited in one window or tab applies in the others at once, like other settings. The theme editor's preview stays local to the main window.

#### Q19. AI API keys

**Answer (taken as recommended): as Decision 8.** On desktop Core writes the keychain inside the settings call; on web the vault stays in the browser and Core only deletes a removed provider's vault rows (Decision 20).

---

## Decisions (2026-10-04)

Settled with the answers above.

### Shared by both slices

#### 1. Ids, times and validation in Core (Q2, Q3)

- Ids keep today's shapes: `conn-`, `project-`, `label-`, `saved-`, `ver-`, `dashboard-`, `dver-`, `workflow-`, `chat-` and `msg-`, each followed by a uuid. The executor reads the existing prefixes from the code and keeps them. Times come from the `Executor` (`iso_timestamp`).
  - 5d-2 re-survey: chats and messages have **no** prefix today (plain `crypto.randomUUID()`, `ai-chat-manager.svelte.ts:18`, `ui-state.svelte.ts:80`, `:302`), and neither do AI providers (`components/settings/ai/ai-provider-section.svelte:89`); user themes are `theme-<uuid>` (`stores/theme.svelte.ts:128`). Core keeps those shapes. Message ids are the one id the GUI still makes (Decision 24).
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
- **Kinds** in 5d-2: `projectState`, `workflow`, `setting` (ids are the keys), `aiSettings`, `theme`, `dashboard` (versions included), `chat` (scope is the connection), `chatMessages` (scope is the chat), `onboarding`, `tutorial` and `importState`. No `connectionOverride` kind: overrides are retired (Q13). `storage` stays for the vault, license and shared-repo writes. Re-survey: `StoredKind` has only the 5d-1 kinds today (`crates/seaquel-workspace/src/library.rs:212-224`), and every 5d-2 write currently emits `storage` (`storage_change`, `crates/seaquel-rpc/src/workspace.rs:661-734`), which the GUI ignores (`library/sync.ts:19`).
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

Tabs and project state, saved workflows, app-state keys, AI settings (with their API keys), themes, onboarding, tutorial progress, import state, dashboards and their versions, and AI chats and messages.

- **Connection overrides are retired** (Q13). The re-survey found `SharedConnectionManager` is never constructed (`rg "new SharedConnectionManager" src` finds nothing, nor does history), so `state.connectionOverrides` (`state.svelte.ts:175`) stays `{}` and nothing loads, saves or reads an override or its keychain entries. Moving dead code into Core would add calls nobody makes. Decision 25 says what happens instead.
- **What stays in the storage group:** the vault (`vaultState*`, `userCredentials*`), `licenseLoad`/`licenseSave`, `sharedReposLoadAll`/`sharedReposSaveAll` (which also writes the `activeRepoId` app-state key, `crates/seaquel-storage/src/queries/shared_repos.rs:48`), and `queryHistoryLoadByConnection`. They keep emitting `storage` (or `history`) events. Nothing else is left there.

#### 20. The `settings` group

- **App-state keys become a closed set of typed settings**, and an unknown key is refused. The set, by stored key (the key names don't change; `SettingKey` serialises as the stored text):
  - `editorKeybindingMode`: `default`, `vim` or `emacs`;
  - `pending_changes_enabled`: `"true"` or `"false"`;
  - `skippedUpdateVersion`: a version string (`max_name_bytes`);
  - `query_version_limit`, `dashboard_version_limit`: a whole number from 0 to 100,000 as text;
  - `license_nudge`: a JSON object, kept as-is (`RawValue`), whole value;
  - `lastActiveProjectId`: read-only here. `windowActivate` (Decision 22) writes it in the same transaction as the window's own row, so older releases and a new window still find it;
  - `connectionStringSecretsNotice`: read, and cleared with `null`; any other value is refused. It is Core's (Decision 12a), and the notice (`stores/connection-secrets-notice.svelte.ts:65`) is its only GUI writer.

  Core's other keys (`connectionStringSecrets{Upgraded,Vacuum,Checkpoint}`) and `activeRepoId` are refused by `settingSet`. `settingSet { key, value }` validates each value; `null` deletes the row (today a `null` leaves a row holding NULL, `app_state.rs:28-37`; loads read both as unset). `appStateGet`/`appStateSet` leave the storage group; the storage gate's probe (`storage/storage-gate.svelte.ts:98`) reads `lastActiveProjectId` through `settingGet`.
  - **Version limits and 0.** Since 5d-1's Task 1 follow-up the settings UI clamps `query_version_limit` and `dashboard_version_limit` to at least 10 on save, so it can no longer choose "keep all" (0). Core still treats a stored 0 as "keep all" (files written before the clamp, or another writer), for both limits (Decision 21), and parses both with `parse_version_limit` (`crates/seaquel-workspace/src/library.rs:1543`). `settingSet` accepts 0 so a hand-set value round-trips; only the UI clamps.
- **AI settings are split into targeted calls** so two windows don't clobber the provider list: `aiProviderCreate`, `aiProviderUpdate`, `aiProviderRemove`, and `aiSettingsPatch` for `enabled`, `shareSchemaGlobally` and `shareDataGlobally`. They are still stored as the one `aiSettings` record, rewritten inside the transaction from the stored copy, so the MCP server's reader (`crates/seaquel-mcp/src/exposed.rs:208-259`) doesn't change.
  - **The rewrite keeps what it doesn't know.** Core reads the record as a JSON object of `RawValue`s, changes only the fields the call names, and writes the rest back byte for byte, so a newer release's fields survive.
  - **The legacy cleanup moves into Core's read** (`stores/ai-settings.svelte.ts:36-46`: drop each provider's `model` and `provider`, `type = type ?? provider ?? "anthropic"`), as does today's fallback: a record that isn't JSON, isn't an object, or whose `providers` isn't absent, `null` or a null-free array reads as the defaults (`enabled` true, schema sharing on, data sharing off), exactly the cases `global_sharing_from` treats as defaults. The next write then stores defaults plus the change, as today's save does (re-survey: the store sets `loaded` before parsing, `:32`, so this is what happens now).
  - Provider ids stay plain uuids and Core makes them. A provider is `{name, type: "anthropic" | "openai-compatible", baseUrl?}`; unknown fields are refused.
  - **API keys follow Decision 8** (Q19; re-survey: today the store writes `ai-api-key:<id>` from TypeScript after the record, `:73-75`, `:82-89`, `:96`). On desktop, `aiProviderCreate`/`Update` take `apiKey: Clearable<String>` and write the keychain in the call, before the record (a failed record write takes the entry back); `aiProviderRemove` deletes the entry after the commit, best effort. On web `apiKey` is `NOT_SUPPORTED`, the vault keeps the key in the browser (`services/vault/vault-keyring.ts:115-122`, scope `ai-api-key-provider`), and `aiProviderRemove` deletes that provider's `user_credentials` rows in its transaction, as `connectionRemove` does.
- **Themes:** `themePreferencesSet { lightThemeId, darkThemeId }` (the single `theme_preferences` row), `userThemeCreate`, `userThemeUpdate` and `userThemeRemove`, each one `user_themes` row (re-survey: they are rows already, `themes.rs:37-62`; only the save replaces them all). Core makes `theme-<uuid>` ids. Removing the light or dark theme in use resets that preference to its default in the same transaction.
- **Onboarding:** `onboardingPatch` merges the named top-level fields into the stored JSON object (the store has seven fields set by separate setters, `stores/onboarding.svelte.ts:6-13`, `:51-81`). A stored record that isn't an object reads as the defaults, as today.
- **Tutorial progress:** `tutorialSave { lessonId, challengeId, state }` (one row), `tutorialRemoveLesson`, `tutorialReset`. `state` stays text, and Core doesn't parse it.
- **Import state:** `importStateSave { source, hasOfferedImport, lastCheckTimestamp }`, with `source` one of `tableplus`, `dbeaver`.
- **Themes apply live** in every window (Q18): a `theme` event reloads the preferences and user themes and re-applies the active one.
- The events: `setting` (ids: the key), `aiSettings`, `theme`, `onboarding`, `tutorial`, `importState` (Decision 16). Every write answers with the whole record or row it wrote, so the GUI can record its `seq` (a theme write answers with the preferences and every user theme).

#### 21. Dashboards

- `dashboardCreate`, `dashboardUpdate` (a patch; widgets, viewport and date filter are whole values) and `dashboardRemove`. Core makes `dashboard-<uuid>` ids.
- **Names clash within a project** (Q3, `NAME_TAKEN`), through a stored `name_key` (the 5d-1 follow-up): `0002` adds the column, its index `(project_id, name_key)` and the older-release trigger, a new data step `backfill_dashboard_name_keys` fills it, and `refill_name_keys` covers dashboards too. Dashboards that already share a name stay.
- **Versions.** `dashboardUpdate` records a version of the previous state (name, description, widgets, viewport, date filter; today's snapshot shape, `utils/dashboard-versions.ts:13-21`) in the same transaction, numbered `MAX(version) + 1` inside it, **only when the patch says `captureVersion: true`**.
  - Re-survey: the TypeScript versions before a rename, a widget added, updated or removed, a date filter and a restore (`dashboard-manager.svelte.ts:570-599`), and not on a move, resize, pan or zoom, which also change `widgets` or `viewport`. Versioning every change of those fields, as this decision first said, would make a version per drag. The GUI keeps choosing, and Core does the numbering and pruning.
  - It then prunes by `dashboard_version_limit`, fixing the ignored setting (already fixed in TypeScript by 5d-1 Task 1), with 0 keeping everything as for queries. The 0 rule is a listed change: today 0 deletes every dashboard version (`utils/version-prune.ts:49-59`).
  - The call returns the dashboard, the new version and the pruned ids (`DashboardUpdated`).
- `starred` and `shared` are patch fields. `starred` alone doesn't touch `updated_at` (as saved queries, Decision 11).
- An update or remove of a missing dashboard is `DASHBOARD_NOT_FOUND`. That also fixes a re-survey bug: a whole-row upsert queued during a delete's await re-inserts the deleted row (`dashboard-manager.svelte.ts:94-122`).
- Widgets are stored without their run state, as today (`dashboard-serialize.ts:8-13` strips `result`, `isLoading`, `error`, `lastRefreshed`). Core doesn't parse them.
- `dashboardRemove` on a beta-era file (no foreign key on `dashboards.project_id`) needs nothing more: versions cascade from the dashboard.
- The git projection stays in TypeScript.

#### 22. View state per window (Q12 B)

**What it covers.** One window's open tabs (with their text), pane layout and active ids per project, plus the window's active project. Saved workflows are not view state (Decision 23), and neither is the connection order. The order stays in `project_state`, shared by the project's windows as today: it is how the project's sidebar looks, not what a window has open. It gets its own call, `projectSidebarSet { projectId, connectionOrder }` in `library` (kind `project`), and the legacy mirror below keeps whatever order that row holds. **The active connection is per window** (Q14): it is in the view state, since it changes on every click, connect and disconnect (`setActiveForProject`, `connection-manager.svelte.ts:918-924`, six callers). The legacy mirror writes the saving window's active connection to `project_state.active_connection_id`, so an older release sees the last window's.

**Window identity.** A window id is `^[A-Za-z0-9_-]{1,64}$`, the same form as the origin (Decision 18), and it is the origin.
- **Desktop.** The webview label (`src/lib/core/origin.ts:51-55`). The main window's label (`main`) is the same after every restart, so it gets its tabs back.
  - Today the only other windows are the theme editor and the log viewer, which hold no project tabs and never save view state.
  - If the app later opens more app windows, their opener gives each a label (`main-2`, …), and a label reused after a restart gets that window's last state back. Nothing more is needed for 5d.
- **Web.** A per-tab id, `win-<uuid>`, kept in `sessionStorage`, so a reload keeps its tabs and a new browser tab starts fresh.
  - **This replaces the per-load origin** (re-survey): today's web origin is random per page load (`webPageOrigin`, `origin.ts:41-44`), sent on every `/api/rpc` call (`storage/rust-client.ts:99`) and on the socket URL (`core/http.ts:125`). The window id becomes that origin, so it must be settled before the page's first Core call: `window-id.ts` resolves it (with the duplicate check below) before the storage gate, and `webPageOrigin()` returns it.
  - Browsers copy `sessionStorage` when a tab is duplicated (and on "reopen closed tab"), so two live tabs can start with one id. At load, a tab announces its id on a `BroadcastChannel`. If another live tab answers holding it, the newer tab makes a new id and starts as a new window (below).
  - The check takes up to 100 ms, once, before the storage gate.
  - After a reload the page has the same origin as before it. An event from the old page's last writes is then skipped as its own; harmless, since the new page lists everything after it loads.

**What a new window starts with: a copy of the project's most recently used window**, its tabs, text and layout. A new tab or a first launch after the upgrade then shows what the user last had, which is what they see today. The copy is independent from then on.
- "Most recently used" means the `window_state` row of that project saved last (`write_seq`, Task 7 probe fixes; first planned as the latest `updated_at`).
- A window's active project, when the window is new, is the most recently used window's active project, else `lastActiveProjectId`; the page reads it with `windowGet` (Calls, below).

**Storage: a numbered migration, `0002_window_state.sql`** (expand-only; the second file in `migrations/`, after 5d-1's `0001_name_keys.sql`). It also carries the dashboards' `name_key` (Decision 21), `idx_saved_canvases_project` (the re-survey found `saved_canvases` had no index on `project_id`), and `idx_ai_messages_chat_time`, so a chat's messages read in `timestamp, rowid` order without a sort (Task 3 review).

```sql
CREATE TABLE IF NOT EXISTS windows (
  window_id TEXT PRIMARY KEY,
  active_project_id TEXT,              -- no foreign key: a removed project just falls back
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_windows_updated ON windows(updated_at);
CREATE TABLE IF NOT EXISTS window_state (
  window_id TEXT NOT NULL REFERENCES windows(window_id) ON DELETE CASCADE,
  project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  state TEXT NOT NULL,                 -- JSON: tabs with their text, layout, active ids
  rev INTEGER NOT NULL DEFAULT 0,      -- the page's save counter (below)
  updated_at TEXT NOT NULL,
  PRIMARY KEY (window_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_window_state_project ON window_state(project_id, updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_saved_canvases_project ON saved_canvases(project_id);
CREATE INDEX IF NOT EXISTS idx_ai_messages_chat_time ON ai_messages(chat_id, timestamp);
ALTER TABLE dashboards ADD COLUMN name_key TEXT;
CREATE INDEX IF NOT EXISTS idx_dashboards_name_key ON dashboards(project_id, name_key);
-- plus dashboards_name_key_stale, as 0001's triggers
```

- The user is the file: a web user's `meta.db` holds only their windows, so no user column is needed.
- `state` is one JSON blob, the design doc's `ui_state` blob, which spares one table per tab type. Its shape is today's `PersistedProjectState` minus the saved workflows, the connection order and the starred-shared legacy lists; it keeps `activeConnectionId` and `activeView`. It is written and read byte for byte (`RawValue`). Core parses it only to write the legacy mirror and to check the limits.
- The migration runs on the beta-era baseline too (the migrations README's rule), and makes `seaquel-cli mcp` refuse a file until the app has opened it, as 0001 did.

**Existing users lose nothing.**
- The first load of a project in a window with no row falls back in order:
  1. the most recently used window's row;
  2. today's `project_state` and `tabs` rows;
  3. empty (the GUI then adds the starter tabs, as today).

  So the first window after the upgrade gets exactly today's tabs, with no data step.
- **Older releases keep working.** Every view-state save also writes the legacy `project_state` row and `tabs` rows for that project, in the same transaction, as today's `project_state::save` does, without `saved_canvases`, keeping the stored connection order and writing the saving window's active connection. An older release opening the file sees the most recently saved window's tabs, as it would today.
  - The mirror skips a tab whose id repeats within the state instead of failing the save (re-survey: `tabs`' key is `(id, project_id)`, and one repeat fails today's whole save, `project_state.rs:256-383`). The window's own row keeps the state as sent.
  - DuckDB extensions tabs stay in the window's row (today they are dropped, `project_state.rs:202-203`, a listed change) and stay out of the mirror, which has no column for them.
- **A save before the load is refused in the GUI.** A window saves a project's view state only after its `windowStateLoad` for that project answered. Otherwise a save fired between switching the active project and its load (re-survey: `project-manager.svelte.ts:546` sets the project before `:551-553` loads it) would write an empty state as this window's row and as the mirror, and the first window after the upgrade would lose today's tabs.

**Ordering: `rev`.** Each save carries `rev`, the page's counter for that window and project, starting from the `rev` its load answered. Core writes a save only when its `rev` is higher than the stored one, and answers `stale: true` otherwise. That lets the `pagehide` save (below), which can't wait for the write queue, overtake a queued save without an older state landing after it.

**Cleanup, bounded.** Inside each view-state save's transaction, after the write:
- delete windows not used for 30 days (`idx_windows_updated`);
- keep at most `max_windows` per user (web 50, desktop 20), the most recent;
- keep at most `max_window_states_per_project` (web 20, desktop 20), deleting the oldest `window_state` rows past them;
- never delete the saving window, or on desktop `main`.

Each step is one indexed `DELETE` with a `LIMIT`-bounded subquery, so the work per save is bounded. The legacy rows are never pruned. The windows limits join `StateLimits` (Decision 27).

**Calls** (the `ui` group):
- `windowStateLoad { windowId, projectId }` returns the state, its `rev`, whether it was copied (and from where: `window`, `legacy` or `empty`), and the `seq`. A copied state is written as this window's row at once, so it doesn't change under the window before its first save.
- `windowStateSave { windowId, projectId, rev, state }` replaces this window's row and the legacy mirror, and answers `{stale}`.
- `windowActivate { windowId, projectId }` sets the window's active project and `updated_at`, and writes `lastActiveProjectId`.
- `windowGet { windowId }` (added with 5d-2 Task 2's review) answers the window's active project, which the page asks for once, before its first `windowStateLoad`: the window's own `active_project_id`; for a window with none (a new one), the active project of the most recently used window that has one (since the Task 7 probe fixes, the last `windows` write committed, `write_seq`; first `windows.updated_at`); else `lastActiveProjectId`; else `null`. It writes nothing, and the GUI checks the id against the projects it listed, as today. It answers `{ activeProjectId, from }`, `from` being `window`, `recent` or `lastActive` (`null` with no id).
- **"Most recently used" is the last write committed** (Task 7 probe fixes: `write_seq`, one past the table's or the project's highest, so windows saving in one millisecond order by commit and a new window copies what the legacy mirror shows; first planned as `updated_at` only, which now drives only the 30-day prune), bumped by writes: `windowStateSave`, `windowActivate` and a `windowStateLoad` that copies a row (the copy is a write). A load of the window's own row writes nothing and bumps nothing.
- `windowStateSave` and a copying `windowStateLoad` create the window's `windows` row when it has none, with `active_project_id` NULL; only `windowActivate` sets `active_project_id`.
- **No `windowForget`** (Q15): `pagehide` fires on a reload too, so forgetting on it would make a reloaded tab copy another tab's state as "most recent". The 30-day and count prunes bound the rows instead.
- The window id must equal the call's origin (Decision 18). A call naming another window's id is refused (`INVALID_ARGUMENT`), so one tab can't overwrite another's view.

**Events.**
- A view-state write emits `projectState` with `scope` the project and `ids` the window id, like every write (Decision 16).
- `ChangeFeed` applies a `projectState` event only when its id is this window's own id and its origin isn't. That happens only when the duplicate-id check hasn't finished yet. Every other window ignores it, so there is no cross-window reload.
- A change to the connection order still goes to every window of the project, as a `project` change. `projectsList` rows don't hold the order, so the refetch for a `project` event also reads the project's sidebar row (`projectSidebarGet`, or the order folded into `projectsList`; Task 4 picks one and says which).

**The rest of today's behaviour stays.**
- The 500 ms debounce per window and project.
- The load guard, now keyed by project within the window (`windowState:<projectId>`).
- On web, a pending save is sent from `pagehide` with `fetch(…, {keepalive: true})`, outside the write queue (`rev` orders it). Browsers cap the keepalive bodies in flight at 64 KiB together; past that the page falls back to a normal `fetch` and hopes, and a text-heavy tab is covered by the 500 ms saves while typing. Desktop keeps its awaited flush on close (`routes/(app)/+layout.svelte:164-187`).
- Only projects with a pending save are flushed. Today `flush()` saves every project the page ever loaded, one after another (`persistence-manager.svelte.ts:176-199`), including removed ones and with the global `activeView` (re-survey bugs; Task 1).

#### 23. Saved workflows

- Saved workflows leave the project state: `workflowCreate`, `workflowUpdate` and `workflowRemove` over `saved_canvases`, one row each, and `workflowsList { projectId }`.
- The stored `data` stays today's `SavedWorkflow` JSON (`types/workflow.ts:97-106`), whose `id`, `projectId`, `createdAt` and `updatedAt` Core now sets: it reads the draft's top level as a JSON object of `RawValue`s, sets those four and writes the rest byte for byte. A draft is `{projectId, workflow}`, the workflow without those fields; an update replaces everything but them.
- Core makes `workflow-<uuid>` ids. Names aren't checked for clashes (Q3 doesn't list workflows).
- `project_state::save` stops touching `saved_canvases`. The frozen fixture's function keeps its old behaviour.
- **Result rows** (Q16). A result node's rows stay in the saved JSON (up to 10,000, `WORKFLOW_MAX_ROWS`, `workflow-manager.svelte.ts:29`). A chart node's copy of them (`:535-558`) is no longer stored:
  - **On save** the GUI stores a chart node with `rows: []` when its `sourceNodeId` names a node in the same workflow that holds rows; `columns` and `chartConfig` stay. A chart whose source isn't in the workflow, or holds no rows, keeps its rows (nothing to rebuild them from).
  - **On load** a chart node with no rows takes `columns` and `rows` from its source node, as `updateDownstreamChartNodes` does after a run, without recalculating `chartConfig`.
  - **Workflows saved before 5d-2** keep their chart copies and load unchanged: a chart node that has rows uses them. The copies are dropped by the next `saveWorkflow` of that workflow (the user saving it), not by other writes: until Task 6b, the project-state save sends the saved workflows as they are in memory, so a rename or another workflow's save keeps them.
  - A chart whose source is itself a chart keeps its rows, since the source chart's own copy may be dropped in the same save. No data step: nothing is lost by keeping them, and Core doesn't parse the JSON.
  - This is GUI work (Task 1), so the fixtures record it and Core stays opaque to the workflow JSON.
  - **Web cap:** `max_workflow_bytes`, 16 MiB of the stored JSON after the copies are dropped. Past it the save is refused (`INVALID_ARGUMENT` naming the limit), nothing is stored, and the GUI says the workflow's results are too large to save on this server and to clear or narrow them first. Desktop has no cap.
- `saveWorkflow` becomes async (it returns the workflow today, `:595-682`), and its callers wait for Core's id.

#### 24. AI chats

- `chatCreate`, `chatUpdate` (title, `touched` for `updatedAt`) and `chatRemove`, and `chatsList { connectionId }`, `chatMessagesList { chatId }`. Chat ids stay plain uuids, made by Core.
- `chatMessagesPut { chatId, messages }` upserts the listed messages by id instead of replacing the chat, with only the messages that changed.
  - **Message ids are the GUI's.** A turn shows the user's message and the assistant's placeholder before anything is stored (`ui-state.svelte.ts:80`, `:302`), so the GUI makes their ids (plain uuids, as today). Core checks each id's form (`^[A-Za-z0-9_-]{1,64}$`) and refuses an id that belongs to another chat.
  - `ChatMessageDraft` is its own type with `deny_unknown_fields` (`PersistedAIMessage` has none): `{id, role, content, timestamp, query?, dashboardId?}`.
  - **When it's sent** (re-survey; this decision first said "at the end of a turn and on approvals"): at the end of a turn (`onDone`, `:385`), on an error (`:394`), on Stop (`:64`), and when a turn is aborted with an approval pending (`:326`). Approvals themselves save nothing. 5d-2 adds one: the page's close flush puts the streaming chat's messages, which today's `flush()` doesn't (`persistence-manager.svelte.ts:196-198` saves chat rows only).
- `chatMessagesRemove { chatId, ids }` is for anything the UI deletes.
- Messages read `ORDER BY timestamp, rowid` (re-survey: `ai_chats.rs:72` orders by timestamp alone, so a user message and its placeholder made in one millisecond can come back swapped). An upsert keeps an existing message's rowid, and `put` inserts new ones in list order.
- **Web budget** (Q17): `max_chat_bytes`, 64 MiB of a chat's stored message content. `chatMessagesPut` sums the chat's stored content bytes (`SUM(length(CAST(content AS BLOB)))` over `idx_ai_messages_chat`, at most 5,000 rows), minus the messages it replaces, plus the new ones, inside its transaction. Past the budget the whole put is refused (`INVALID_ARGUMENT` naming `max_chat_bytes`) and nothing is stored. The GUI then shows "This chat is full. Start a new chat to continue.", keeps the refused turn on screen (not stored), and disables sending in that chat with a "New chat" button. `chatMessagesList` answers the chat's stored bytes too, so a full chat opens disabled. Desktop has no budget.
- The `aiMessages:*` load guard goes.
- An open chat that is streaming ignores events for itself until the turn ends, then refetches. Deleting a chat that is streaming, here or in another window, aborts the stream first (re-survey: `deleteChat` never aborts, `ai-chat-manager.svelte.ts:56-84`, and a pending approval's promise then never settles).

#### 25. Connection overrides

**Retired** (Q13). The re-survey found the feature is dead code: `SharedConnectionManager` is never constructed, and its `saveCredentials` would key the keychain by the shared template's id (`shared-connection-manager.svelte.ts:162-168`), while connections are keyed by their own (local) ids. So:
- `shared-connection-manager.svelte.ts` is deleted, with `state.connectionOverrides` and the `ConnectionOverride` type if nothing else uses them;
- the `connectionOverrides*` storage methods (TS client and `StorageRequest`) and `PersistenceManager`'s override functions go;
- the `connection_overrides` table and its storage functions stay (expand-only; the frozen fixtures and older releases), and no `library` call is added;
- CLAUDE.md's line that override credentials still go through the `secret` group is corrected (Task 8);
- the 5d-1 follow-up "override credentials" closes. If per-machine overrides come back as a feature, they get `overrideSave`/`overrideRemove` in `library` with `SecretChanges` then.

#### 26. The demo

`TsLibrary` grows the 5d-2 methods over the same sql.js repositories, plus a TypeScript `TsState` or the same class, whichever keeps `ts-library.ts` readable (it is 1,280 lines). Its events are local: it emits nothing, since the demo is one page. The demo's window id is a constant (`demo`), since the demo has no second window that shares its file.

#### 27. Web limits, 5d-2

`StateLimits`, set with `CoreBuilder::state_limits`, with none on desktop, the CLI or MCP, except the window counts (desktop 20 windows, 20 states per project). `WEB_STATE_LIMITS`:

| Limit | Value |
|---|---|
| a window's view state per save (tabs with their text, layout) | 8 MiB; one tab's text 2 MiB; 500 tabs |
| windows (Decision 22) | 50 per user, 20 window states per project, unused for 30 days pruned |
| a saved workflow's JSON (after chart copies are dropped, Q16) | 16 MiB (`max_workflow_bytes`); 1,000 per user |
| a dashboard's widgets, viewport and filter together | 4 MiB; 1,000 per user; versions share `max_version_bytes`' rule (16 MiB per dashboard) |
| an AI message's content | 1 MiB; 5,000 messages per chat; 64 MiB of content per chat (`max_chat_bytes`, Q17); 10,000 chats per user |
| a setting's value, the AI settings record, one user theme | 256 KiB; 200 user themes; 50 AI providers |
| names (dashboards, workflows, chat titles, providers, themes) | `max_name_bytes` from `LibraryLimits` (1 KiB) |

A call past a limit is refused with `INVALID_ARGUMENT` naming it, before anything is read (counts after, inside the transaction, as 5d-1 does). The probe measures each at its bound.

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

Revised after the re-survey (Decisions 20–25). Every params struct has `deny_unknown_fields` and a hand-written `Debug`; a `RawValue` body is opaque and not checked for fields.

```rust
// crates/seaquel-workspace/src/state.rs
pub struct DashboardDraft { pub project_id: String, pub name: String, pub description: Option<String>,
    pub widgets: Box<RawValue>, pub viewport: Box<RawValue>, pub date_filter: Option<Box<RawValue>> }
pub struct DashboardPatch { pub name: Option<String>, pub description: Clearable<String>,
    pub widgets: Option<Box<RawValue>>, pub viewport: Option<Box<RawValue>>,
    pub date_filter: Clearable<Box<RawValue>>, pub starred: Option<bool>, pub shared: Option<bool>,
    #[serde(default)] pub capture_version: bool }              // Decision 21: the GUI says when
pub struct DashboardUpdated { pub dashboard: PersistedDashboard,
    pub version: Option<PersistedDashboardVersion>, pub pruned_version_ids: Vec<String> }

pub struct WorkflowDraft { pub project_id: String, pub workflow: Box<RawValue> }  // SavedWorkflow minus id, projectId, times
pub struct ChatDraft { pub connection_id: String, pub title: String }
pub struct ChatPatch { pub title: Option<String>, #[serde(default)] pub touched: bool }  // touched: updatedAt = now
pub struct ChatMessageDraft { pub id: String, pub role: String /* user | assistant */, pub content: String,
    pub timestamp: String, pub query: Option<String>, pub dashboard_id: Option<String> }
pub enum SettingKey { /* Decision 20's closed set, serialised as the stored key text */ }
pub struct AiProviderDraft { pub name: String, #[serde(rename = "type")] pub ty: String, pub base_url: Option<String> }
pub struct AiProviderPatch { pub name: Option<String>, #[serde(rename = "type")] pub ty: Option<String>,
    pub base_url: Clearable<String> }
pub struct AiSettingsPatch { pub enabled: Option<bool>, pub share_schema_globally: Option<bool>,
    pub share_data_globally: Option<bool> }
pub struct StateLimits { /* Decision 27; each Option, default none */ }

pub fn read_ai_settings(raw: Option<&str>) -> AiSettings;   // the legacy cleanup and today's fallback
pub fn legacy_mirror(state: &RawValue) -> Result<MirrorRows, StateError>;   // tabs to write, repeats skipped

// library group, 5d-2 methods
DashboardsList { project_id },
DashboardVersionsList { project_id } -> Seqd<Vec<PersistedDashboardVersionMeta>>,   // Task 7: no snapshots
DashboardVersionGet { dashboard_id, version_id } -> Seqd<PersistedDashboardVersion>, // Task 7: one whole

DashboardCreate { dashboard: DashboardDraft }, DashboardUpdate { id, patch: DashboardPatch }, DashboardRemove { id },
WorkflowsList { project_id } -> Seqd<Vec<PersistedWorkflowMeta>>,   // Task 7: no bodies
WorkflowGet { workflow_id } -> Seqd<RawValue>,                     // Task 7: one whole
WorkflowRename { workflow_id, name } -> Seqd<PersistedWorkflowMeta>, // Task 7 review: only the name
WorkflowCreate { workflow: WorkflowDraft },
WorkflowUpdate { id, workflow: Box<RawValue> }, WorkflowRemove { id },
ChatsList { connection_id }, ChatMessagesList { chat_id },   // answers the messages and the chat's stored bytes
ChatCreate { chat: ChatDraft }, ChatUpdate { id, patch: ChatPatch }, ChatRemove { id },
ChatMessagesPut { chat_id, messages: Vec<ChatMessageDraft> }, ChatMessagesRemove { chat_id, ids: Vec<String> },
ProjectSidebarSet { project_id, connection_order: Vec<String> },   // the active connection is per window (Q14)
// no override calls: retired (Q13)

// settings group
SettingGet { key: SettingKey }, SettingSet { key: SettingKey, value: Option<String> },
AiSettingsGet, AiSettingsPatch { patch: AiSettingsPatch },
AiProviderCreate { provider: AiProviderDraft, #[serde(default)] api_key: Clearable<String> },   // api_key: desktop only
AiProviderUpdate { id, patch: AiProviderPatch, #[serde(default)] api_key: Clearable<String> },
AiProviderRemove { id },
ThemesGet, ThemePreferencesSet { light_theme_id, dark_theme_id },
UserThemeCreate { theme: Box<RawValue> }, UserThemeUpdate { id, theme: Box<RawValue> }, UserThemeRemove { id },
OnboardingGet, OnboardingPatch { patch: Box<RawValue> /* a JSON object; its top-level fields replace */ },
TutorialList, TutorialSave { lesson_id, challenge_id, state: Option<String> },
TutorialRemoveLesson { lesson_id }, TutorialReset,
ImportStateGet { source }, ImportStateSave { source, has_offered_import, last_check_timestamp },

// ui group (Q12 B, Decision 22). window_id must equal the call's origin.
WindowStateLoad { window_id, project_id }
    -> Seqd<WindowStateLoaded { state: Option<Box<RawValue>>, rev: u64, copied_from: Option<CopiedFrom /* window | legacy | empty */> }>,
WindowStateSave { window_id, project_id, rev: u64, state: Box<RawValue> } -> Seqd<{ stale: bool }>,
WindowActivate { window_id, project_id },
WindowGet { window_id } -> { active_project_id: Option<String>, from: Option<"window" | "recent" | "lastActive"> },
```

- JSON bodies (widgets, workflows, themes, onboarding, the license nudge) stay `RawValue`, byte for byte, as the storage group keeps them. Where Core sets fields (a workflow's id and times, the AI settings record, an onboarding patch) it rewrites only the top level.
- `StorageRequest` loses `appState*`, `projectState*`, `dashboards*`, `dashboardVersions*`, `aiChats*`, `themes*`, `onboarding*`, `tutorial*`, `importState*` and `connectionOverrides*` (32 variants, `crates/seaquel-rpc/src/workspace.rs:253-373`). What remains is `queryHistory*`, `sharedRepos*`, `license*`, `vaultState*` and `userCredentials*`, and every write among them emits `StorageChanged`.
- Task 7 probe fixes: `DashboardUpdated.version` is a `PersistedDashboardVersionMeta`; `PersistedDashboardVersionMeta {id, dashboardId, version, createdAt, widgetCount: Option<u32>, bytes}` and `PersistedWorkflowMeta {id, projectId, name, createdAt?, updatedAt?, bytes}` live in `seaquel-types`; `DASHBOARD_VERSION_NOT_FOUND` (404) is new; `SeqdJsonList` is gone; `windowStateSave`'s `rev` is at most 2^53 - 1.
- New error codes: `DASHBOARD_NOT_FOUND`, `WORKFLOW_NOT_FOUND`, `CHAT_NOT_FOUND`, `THEME_NOT_FOUND`, `AI_PROVIDER_NOT_FOUND` (404 on web). `NAME_TAKEN` gains dashboards.

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
- **5d-2 cases, at least 60:** the full list, with the old-data seeds, is in 5d-2's Task 2. In short: view state (every tab type, layouts, active ids, the legacy canvas fields, the legacy mirror, the first load in a new window); workflows; dashboards with versions and prunes; chats and messages; every setting key; AI settings with legacy and malformed records; themes, onboarding, tutorial and import state.
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
  - (5d-2) workflows outside the project state;
  - (5d-2) a dashboard version only when the patch asks; dashboard `NAME_TAKEN`; a missing dashboard `DASHBOARD_NOT_FOUND` instead of re-inserted;
  - (5d-2) `settingSet` with `null` deleting the row; unknown keys refused;
  - (5d-2) the legacy mirror skipping repeated tab ids; extensions tabs kept in the window's row;
  - (5d-2) messages ordered by timestamp, then insertion.
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
- (5d-2) a view-state save accepted for a window id other than the caller's origin, a prune that can delete the saving window or `main`, the legacy mirror not written, or a first load that doesn't fall back to today's rows;
- (5d-2) a view-state save sent before that window's load of the project answered, a flush that saves projects with no pending change, or a save whose `rev` isn't checked;
- (5d-2) the web origin still made per page load instead of being the window id, or resolved after the first Core call;
- (5d-2) a 5d-2 write left on `st.pool()` instead of a `WriteTx` (the storage group's single statements bypass the write mutex and take their `seq` after the commit, `crates/seaquel-rpc/src/workspace.rs:631-645`);
- (5d-2) a write answer's `seq` recorded for a list it doesn't hold whole (`chatMessagesPut`'s messages for the chat's list, a dashboard's new version for the project's versions): read again, as 5d-1's label writes do;
- (5d-2) the `aiSettings` record rewritten from the GUI's copy, a field Core doesn't know dropped, or the MCP reader not pinned against Core's writes;
- (5d-2) an AI API key written from TypeScript on desktop, or sent to Core from web;
- (5d-2) a count or name check that scans (every count is on an indexed column or a whole small table, measured at the limits);
- (5d-2) a deleted-elsewhere dashboard, workflow, chat or theme left open: its tabs close (in every project, as `connection-tabs-cleanup.ts` does), a streaming chat aborts first, and an active theme falls back to the default;
- (5d-2) a new kind with no handler on `onResubscribed`, or a store that keeps saving whole records it hasn't reloaded.

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

**Status (Task 8):** done. CLAUDE.md, the design doc's status line, storage notes and "Phase 5d-1 cost", the execution notes, release notes, checkpoint and manual checks below, the consolidated follow-ups and the effort log's totals are written. The full check list ran except the live workspace tests, which the owner asked to skip; see "Checkpoint (5d-1)". The owner ran the manual checks below; all pass.

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

**Manual checks:** all pass (the owner).

**Not run:** the release workflow and a signed build.

---

## 5d-2: state, settings, dashboards and chats

Starts after 5d-1's checkpoint (done). Line numbers below are as of `8e178a8` and were re-read for this section; items 13–18 of "What the code shows" are the older survey.

### 5d-2: re-survey (`8e178a8`)

**Tabs and project state.**
- The save is `PersistenceManager.persistProjectState` (`persistence-manager.svelte.ts:360-414`), debounced 500 ms per project by `scheduleProject` (`:146-158`) and sent as `projectStateSave` (`storage/rust-client.ts:415-427`, workflows encoded by `toStorable`, `:354-361`). Rust replaces the state row, every `tabs` row and every `saved_canvases` row of the project in one transaction (`crates/seaquel-storage/src/queries/project_state.rs:216-404`). The load is `load` (`:69-206`), called only through `persistence-manager.svelte.ts:429-436` under the `projectState:*` guard (`LoadKey`, `:36`).
- **35 calls schedule it, in 12 files** (the first survey's 43 in 15 counted the wrapper, the flushes and the shared-repo timer). All in `src/lib/hooks/database/`:
  - `base-tab-manager.svelte.ts:83`, `:103`, `:125` (`appendTab`, `remove`, `setActive`, inherited by every tab manager);
  - `query-tabs.svelte.ts:92`, `:127` (every text change), `:232`, `:261`, `:279`, `:289`;
  - `dashboard-tabs.svelte.ts:68`, `:89`; `create-table-tabs.svelte.ts:170`; `tab-ordering.svelte.ts:131`;
  - `pane-manager.svelte.ts:98`, `:244`, `:265`, `:387`; `ui-state.svelte.ts:402`;
  - `workflow-manager.svelte.ts:651`, `:680`, `:747`, `:765` (saved workflows);
  - `saved-queries.svelte.ts:137`, `:166`, `:400`; `dashboard-manager.svelte.ts:471`, `:491`;
  - `connection-manager.svelte.ts:409`, `:634`, `:923` (`setActiveForProject`), `:1189`, `:1208` (`reorder`);
  - `project-manager.svelte.ts:475`, `:909`, and `:537`, the one immediate save (the old project, on a switch).

  The wrapper is `scheduleProjectPersistence` (`hooks/database.svelte.ts:113-115`), handed to 20 constructors. Flushes: `routes/(app)/+layout.svelte:143` (`beforeunload`, not awaited), `:173` (desktop close, awaited), `hooks/database.svelte.ts:525` (`destroy()`, which nothing calls).
- Project switch: `ProjectManager.setActive` (`project-manager.svelte.ts:530-563`) saves the old project, loads the new one's data, sets `activeProjectId` (`:546`), writes `lastActiveProjectId` (`:548`), then loads its state (`:551-553`, `loadProjectState` `:937-1183`). Starter tabs: `starter-tabs.svelte.ts:65-83`.
- `PersistedProjectState`: `crates/seaquel-types/src/storage.rs:392-489`, TS `src/lib/types/project.ts:96-166`. Schema: `project_state` (`schema.rs:110`), `tabs` (`:134`, key `(id, project_id)`), `saved_canvases` (`:221`, no index on `project_id`).
- Web close: only `beforeunload` (`+layout.svelte:280`), no `pagehide`, `keepalive` or `sendBeacon`. `flush()` saves every loaded project in turn behind the write queue, so at most the first request leaves before the page goes.
- The origin: `src/lib/core/origin.ts` (`webPageOrigin` random per load, `pageOrigin` the label on desktop), used at `storage/rust-client.ts:99`, `core/http.ts:125`, `hooks/database.svelte.ts:358`. No window id, `sessionStorage` or `BroadcastChannel` anywhere.

**Saved workflows.** `workflow-manager.svelte.ts:595-682` (save, sync, `workflow-<uuid>` at `:658`), `:732-748` (delete), `:753-766` (rename), all through the project save. State: `savedWorkflowsByProject` (`state.svelte.ts:110`), restored at `project-manager.svelte.ts:1048-1049` (with `savedCanvases`). Rows: up to `WORKFLOW_MAX_ROWS` (10,000, `:29`) per result node, copied again into each chart node (`:535-558`). A workflow that fails `fromStorable` is dropped on load (`rust-client.ts:337-343`) and so deleted by the next save.

**Dashboards.** `dashboard-manager.svelte.ts`: create `:67-92` (`dashboard-<uuid>`), every mutation `await persistDashboard` (`:627-634`, whole-row upsert, `errorToast` on failure), delete `:94-122`, star `:433-446`, share and unshare `:455-492`, versions `captureVersion` `:570-599` (numbered from memory, `:574-575`; `dver-<uuid>`, `utils/dashboard-versions.ts:32`) and `pruneVersions` `:606-623` (`dashboard_version_limit` through `persistence-manager.svelte.ts:458-467`). Load: `state-restoration.svelte.ts:235-247` (`loadDashboards` at `dashboard-manager.svelte.ts:402` has no caller). Storage: `queries/dashboards.rs` (upsert `:49`, remove `:68`), `dashboard_versions.rs` (insert `:57`, prune `:71`). Widgets are stored without run state (`dashboard-serialize.ts:8-13`); no rows are stored. Git: `shared-dashboard-manager.svelte.ts`, reconciled at `project-manager.svelte.ts:717-737`.

**AI chats.** `ai-chat-manager.svelte.ts` (create `:13-40`, delete `:56-84`, title `:103-117`), chat rows upserted on a 500 ms timer per connection (`persistence-manager.svelte.ts:502-530`), messages replaced whole (`:532-572`, `replace_all_messages`, `ai_chats.rs:93-118`). A turn saves at its end (`ui-state.svelte.ts:385`), on an error (`:394`), on Stop (`:64`) and on an abort with an approval pending (`:326`); never per chunk. A stored message is text plus `query` and `dashboardId`; tool calls and results are never stored (`services/ai/index.ts:236`). Ids are plain uuids.

**Settings.**
- `app_state` keys the GUI writes: `aiSettings` (`stores/ai-settings.svelte.ts:66`), `editorKeybindingMode` (`stores/editor-settings.svelte.ts:28`), `pending_changes_enabled` (`stores/pending-changes-settings.svelte.ts:22`), `skippedUpdateVersion` (`stores/update.svelte.ts:60`), `license_nudge` (`stores/license-nudge.svelte.ts:133`, on every query run, `query-execution.svelte.ts:781`), `query_version_limit` and `dashboard_version_limit` (`components/settings/general/query-history-section.svelte:42`, `:59`), `lastActiveProjectId` (`persistence-manager.svelte.ts:343`), `connectionStringSecretsNotice` (cleared, `stores/connection-secrets-notice.svelte.ts:65`). Core's own: `connectionStringSecrets{Upgraded,Notice,Vacuum,Checkpoint}` (`crates/seaquel-core/src/upgrade.rs:55-72`), `query_version_limit` (read, `crates/seaquel-core/src/library.rs:993`), and `activeRepoId` (`queries/shared_repos.rs:48`).
- AI settings: type `src/lib/types/ai.ts:3-22`; whole record on every change, no debounce (`:69-109`); keys `ai-api-key:<id>` written from TypeScript after the record. The MCP server reads the record tolerantly (`crates/seaquel-mcp/src/exposed.rs:208-259`, tests `:272-343` and `tests/tools.rs:1046-1070`).
- Themes: `stores/theme.svelte.ts` (500 ms debounce `:267-275`, `persist` `:277-291` writes the preferences and then replaces every user theme, `themes.rs:50-62`). The editor window only emits events to the main window (`windows/theme-editor/+page.svelte:62`, `:73`, `:96-101`; listeners `+layout.svelte:205-236`).
- Onboarding (`stores/onboarding.svelte.ts`, whole record, desktop only), tutorial (`stores/tutorial-progress.svelte.ts`, per row), import state (`stores/tableplus-import.svelte.ts:121-131`, `dbeaver-import.svelte.ts`), license (`stores/license.svelte.ts:304-323`, stays in the storage group).
- The GUI ignores every `storage` event (`library/sync.ts:19`), so every store that saves a whole record or list can overwrite another tab's change: AI settings, user themes, onboarding, license, the license nudge.

**Connection overrides: dead code.** `SharedConnectionManager` (`shared-connection-manager.svelte.ts`) is never constructed; `state.connectionOverrides` stays `{}`; `persistConnectionOverride` and friends (`persistence-manager.svelte.ts:628-673`) are called only by a test (`library-persistence.svelte.test.ts:322`). CLAUDE.md's note that override credentials "still go through the `secret` group" describes code that doesn't run.

**Rust today.** None of the ten 5d-2 query modules has a `WriteTx` function except `app_state::set_in` (`app_state.rs:19`); their single statements run on `st.pool()` and bypass the write mutex (`codec::begin` is used only by the multi-statement saves). `StoredKind` has the 5d-1 kinds only. The machinery 5d-2 reuses: `Storage::write`/`WriteTx` (`crates/seaquel-storage/src/write.rs:31-116`, `Reader` `:167-194`), `WRITE_WAIT` (`open.rs:87`), the change counter (`crates/seaquel-core/src/changes.rs:125-209`), `announce` and `record_storage_write` (`workspace.rs:439-474`), the event bounds (`crates/seaquel-workspace/src/library.rs:76-85`, `changes.rs:103-122`), `LibraryLimits` (`library.rs:139-156`) and `WEB_LIBRARY_LIMITS` (`crates/seaquel-server/src/lib.rs:111-120`), the per-user hub (`crates/seaquel-server/src/workspaces.rs:168-285`), and `deny_unknown_fields` on the envelope and every group but `db` (`crates/seaquel-rpc/src/workspace.rs:136-154`, `library.rs:43-116`).

#### Bugs the re-survey found

Data loss first. Task 1 fixes the ones marked **T1**; the rest go with the task that rewrites the code.

1. **A save of a project whose state isn't loaded yet writes empty tabs** over the stored ones: `setActive` sets the project (`project-manager.svelte.ts:546`) before its state loads (`:551-553`), and the guard only covers failed or pending loads (`persistence-manager.svelte.ts:57-61`). The same window exists at startup (`:163` → `:167`). **T1**, and a rule in Decision 22.
2. **Closing a web tab loses tab, workflow and chat changes**: `beforeunload` doesn't wait, `flush()` saves projects one at a time behind the queue, no `keepalive`. **T1** (keepalive for the active project's pending save).
3. **The global `activeView` is saved into other projects** (`persistence-manager.svelte.ts:388`): a timer that fires after a switch, or `flush()`, stores the current project's view as another's. **T1**.
4. **`flush()` saves every project the page ever loaded** (`:188-190`), including a removed one (its `queryTabsByProject` entry stays, `query-execution.svelte.ts:235-240`), which fails on the foreign key and can use up the one request that leaves on unload. `setActive`'s immediate save leaves the old timer running (bug 3 again). **T1**: flush only pending projects, and clear a project's timer when saving it now.
5. **Saved workflows and user themes are replaced whole** from a stale copy by another tab's save, as saved queries were before 5d-1. 5d-2's Tasks 4 and 6.
6. **AI settings, onboarding, license and the license nudge are saved whole** from a stale copy (another tab's changes are lost). Tasks 4 and 6, except the license (stays in the storage group; listed as a follow-up).
7. **A deleted dashboard can come back**: an upsert queued while `deleteDashboard` awaits its remove re-inserts the row (`dashboard-manager.svelte.ts:94-122`). Task 4 (`DASHBOARD_NOT_FOUND`).
8. **Dashboard version numbers collide** after a failed versions load or from another tab (`:574-575`); the failed insert stays in memory. Task 4.
9. **The shared-dashboard reconcile re-saves every dashboard on every activation**: `reconcileWithGitFiles` always returns a new array (`shared-dashboard-manager.svelte.ts:196`), so `reconciled !== dashboards` is always true (`project-manager.svelte.ts:729`), and `persistProjectDashboards` stops at the first failure (`persistence-manager.svelte.ts:416-427`). Once each save is a Core call with an event, that is one event per dashboard per activation in every window. **T1**: save only what changed.
10. **A chat stream that fails outside the provider's handling leaves the UI streaming**: `sendAIMessageService` runs with `void` and no catch (`ui-state.svelte.ts:334`), and `fetch` sits outside any try (`services/ai/providers.ts:94-103`). Nothing is saved and `isAIStreaming` stays true. **T1**: route the rejection to `onError`, which saves.
11. **Deleting a chat mid-stream doesn't stop it** (`ai-chat-manager.svelte.ts:56-84`); a pending approval never settles, and a message save already running can re-insert messages for the deleted chat (a logged foreign-key failure). **T1**: abort first.
12. **A turn in progress is lost on quit**: `flush()` saves chat rows, not messages (`persistence-manager.svelte.ts:196-198`). **T1**.
13. **A failed load of the active chat's messages at startup refuses every save for it all session** (`state-restoration.svelte.ts:212-230` loads it outside the retrying path). Task 6 (the guard goes).
14. **Messages with one timestamp can load swapped** (`ai_chats.rs:72`). Task 3 (`ORDER BY timestamp, rowid`).
15. **Onboarding on web shows a false "couldn't be loaded" toast**: the store is initialised only on desktop (`routes/(app)/+layout.svelte:97-107`), and `completeWizard` (`components/connection-tab-view.svelte:176`, `:258`, `:268`) and `setLearnEnabled` (`components/settings/features/features-section.svelte:28`) reach `skipUnloadedSave` on web. **T1**: skip onboarding writes on web, as the load is skipped.
16. **The license nudge's `respond` and `snooze` save without the loaded check** (`stores/license-nudge.svelte.ts:109-122`), so after a failed load they write zeroed counts. **T1**.
17. **One malformed tutorial row empties the whole progress** (`stores/tutorial-progress.svelte.ts:36`, `:43-46`), after which `resetLesson` does nothing. **T1**: skip the bad row.
18. **The dashboards list isn't cleared by an empty load** (`state-restoration.svelte.ts:239-246` restores only non-empty results), and the first activation loads a project's data twice (`project-manager.svelte.ts:543`, `:944`). Task 6, where the feed's reloads need an empty list to apply.
19. **DuckDB extensions tabs are dropped** by the Rust save and load (`project_state.rs:202-203`) while their ids stay in `tabOrder` and the layout. Decision 22 keeps them in the window's row.
20. **One repeated tab id or workflow id fails the whole project save** (`tabs`' key, `saved_canvases.id` is global, `project_state.rs:391`). Decisions 22 and 23.
21. **Starter tabs swap `state.activeProjectId`** while adding a project's defaults (`starter-tabs.svelte.ts:70-82`). **T1**: pass the project id.
22. **A stale `activeWorkflowId` after a project switch makes a save create a copy** instead of updating (`workflow-manager.svelte.ts:622-654`; `WorkflowState` is global). Task 6.
23. **Settings stores without a guard** (editor settings, pending changes) let a setter called before the load be overwritten by it, and the version-limit section has no error handling (`query-history-section.svelte:10-60`). Task 6.
24. **Two tabs setting up the web vault at once** replace each other's salt and verifier (`services/vault/vault-state.svelte.ts:160-181`, `vault_state.rs:30-43`). The vault stays in TypeScript: a follow-up.
25. **Shared dashboards: edits never reach the git file**, only share, unshare and delete do, and the reconcile then takes the stale file with no newer-than check (`shared-dashboard-manager.svelte.ts:211-222`); a rename leaves the old file. Q7 keeps git in TypeScript: a follow-up, like the saved-query rename 5d-1 fixed.
26. **Dead code:** connection overrides (above), `DashboardManager.loadDashboards`, `license.revertToPersonal` (`stores/license.svelte.ts:195-207`), `UseDatabase.destroy()`.

#### Stored data to expect (for Task 2)

5d-1's added scope came from rows older releases left. What the re-survey found the 5d-2 loads already tolerate, and Core's reads must tolerate the same way (a write checks its input; a read never refuses a stored row, it skips or defaults it as today):
- `aiSettings` with `model`/`provider` on providers, not JSON, not an object, or `providers` holding `null` (the store and MCP read defaults).
- `user_themes` rows that don't parse (skipped, `themes.rs:37-46`); `onboarding_state` that doesn't parse (defaults, `codec.rs:205-226`); `tutorial_progress.state` that doesn't parse (today it empties everything: bug 17).
- `project_state` with `activeView: "canvas"`, `canvasTabs`, `savedCanvases`, `activeCanvasTabId` (`project-manager.svelte.ts:50-54`, `:1035-1061`), NULL `tab_order`, empty `pane_layout`; `saved_canvases` rows that don't parse or don't decode (skipped).
- `dashboards.starred` NULL (every file), `dashboards.project_id` NULL and without a foreign key (beta-era files), dashboards sharing a name.
- `ai_messages` with equal timestamps; `app_state` rows holding NULL.

Task 2 seeds each of these and records what today's code does with it.

### Order and estimates

Sized from 5d-1's logged times (effort log, design doc "Phase 5d-1 cost"), each first pass scaled by how much more or less the task holds than its 5d-1 twin.

| # | Task | First pass | 5d-1's twin (logged first pass) | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | TS fixes from the re-survey, and chart nodes stored without rows (Q16) | 0.4–0.55 h | 1 (0.27 h, 8 items; here 13) | — | 2, 3 |
| 2 | State fixtures, the recorder, the old-data seeds | 0.45–0.65 h | 2 (0.35 h; more entities, the seeds) | 1 | 3 |
| 3 | Storage: `0002`, targeted queries on `WriteTx`, the legacy mirror, the dashboard name-key step | 0.6–0.8 h | 3 (0.47 h; ten modules, a migration) | — | 1, 2 |
| 4 | Core: the `state` module, settings, AI settings, themes, dashboards, workflows, chats, window state, limits, events | 1.3–1.6 h | 4 (1.2 h; more methods, no secrets upgrade) | 2, 3 | — |
| 5 | RPC: `settings` and `ui` groups, `library` additions, 32 storage variants out, `types:gen` | 0.5–0.7 h | 5 (0.95 h, of which event delivery; now plumbing only) | 4 | — |
| 6a | GUI: window identity, view state, the 35 project-save calls, `PersistenceManager` out | 1.1–1.4 h | 6 (1.9 h for four managers and the feed) | 5 | 6b's stores part |
| 6b | GUI: dashboards, workflows, chats, the settings stores, `ChangeFeed` kinds, `TsLibrary` | 1.3–1.6 h | 6 | 5 | 6a (stores only) |
| 7 | Probe | 0.7–0.9 h | 7 (0.65 h; more checks) | 6a, 6b | — |
| 8 | Docs, measurement, checkpoint | 0.5–0.65 h | 8 (0.5 h) | all | — |
| | **First passes** | **6.85–8.85 h** | 5d-1: 6.3 h | | |
| | Review fixes (~70% of first passes, as 5d-1 ran) | 4.8–6.2 h | 5d-1: 3.8 h | | |
| | Probe fixes (work) | 1.5–2 h | 5d-1: 1.5 h | | |
| | Live suite waits (one affected-crates run per Core/storage review round, one full run at the checkpoint) | 2–2.5 h | 5d-1: ~2.3 h | | |
| | Old data and owner decisions still to come (5d-1's Decision 12a took 0.85 h; Q13–Q19 are answered) | 0.3–0.7 h | | | |
| | **Total** | **~15.3–20.3 h** | 5d-1: 14.8 h | | |

Expect about 17.5 h. That is above the design doc's 11–14 h, which kept first passes "as the plan has them" (3.9–5.95 h). The re-survey sizes them from 5d-1's actual first passes instead, and Task 6 is split in two because 5d-1's Task 6 alone ran 1.9 h against 0.9–1.3 h. **Both slices together: about 32 h.**

The riskiest tasks:
- **Task 6a:** window identity has to be settled before the first Core call, the load-before-save rule touches every tab manager, and the fallback load must give the first window after the upgrade exactly today's tabs.
- **Task 4:** the `aiSettings` record rewritten from the stored copy byte-compatibly for the MCP reader; the legacy mirror; `rev`; the bounded prunes.
- **Task 6b:** a streaming chat meets the feed; eight stores change their load and save; dashboards with an unsaved local change meet remote updates.

Cuts if time runs short: the `rev` ordering (keep only the load-before-save rule), and the onboarding patch (whole value, last writer wins; it is desktop-only today).

### Task 1: Fixes in TypeScript first

Bugs the re-survey found that TypeScript alone fixes, so the fixtures record the fixed behaviour. Data loss first.

**Files:**
- `persistence-manager.svelte.ts`:
  - a project's state is saved only after this page loaded it: a per-project `loaded` set, filled when `loadProjectState` answers (with a row or `null`) and cleared on removal; `persistProjectState` returns early (logged, not toasted) for a project not in it (bug 1);
  - `flush()` saves only projects with a pending timer, active project first (bug 4);
  - `activeView` is kept per project in memory (`activeViewByProject`, set on switch and by `ui-state.svelte.ts:402`) and saved from there (bug 3);
  - `persistProjectState` clears the project's own timer (bug 4);
  - `flush()` also saves the streaming chat's messages (bug 12).
- `routes/(app)/+layout.svelte`: on web, `pagehide` sends the active project's pending save with `keepalive: true` when its body is under 60 KiB, outside the write queue (bug 2). `RustStorageClient` gets a `sendKeepalive(method, params)` next to `call`.
- `project-manager.svelte.ts`: the reconcile saves only dashboards whose content changed (compare the persisted form), and a failed one doesn't stop the rest (bug 9). `shared-dashboard-manager.svelte.ts` returns the same array when nothing changed.
- `ui-state.svelte.ts`: `sendAIMessageService`'s rejection goes to `onError` (bug 10). `ai-chat-manager.svelte.ts`: `deleteChat` aborts a stream on that chat first (bug 11).
- `stores/onboarding.svelte.ts`: writes are skipped on web, where nothing is loaded (bug 15). `stores/license-nudge.svelte.ts`: `respond` and `snooze` check `initialized` (bug 16). `stores/tutorial-progress.svelte.ts`: a row that doesn't parse is skipped and logged (bug 17). `starter-tabs.svelte.ts`: `initializeDefaults(projectId)` no longer swaps the active project (bug 21).
- `workflow-manager.svelte.ts` (Q16, Decision 23): the save stores a chart node with `rows: []` when its source node in the same workflow holds rows; the load fills an empty chart from its source. A workflow with stored chart copies loads unchanged.
- **Follow-up from Task 2's review:** `project-manager.svelte.ts` adds a project's starter tabs only when its saved state holds no tab of any saved type (`hasSavedTabs`: query, schema, explain, ERD, statistics, workflow and the old `canvasTabs`, dashboard, create-table, data and DuckDB extensions tabs), plus the unsaved settings tabs. Before, the check read the page's lists, which hadn't counted workflow, statistics and extensions tabs and read the dashboard, create-table and data tabs before they were restored, so a project with only those tabs got the starter tabs again on every load. Tests: `which saved projects get starter tabs` (`starter-tabs.svelte.test.ts`); the fixtures' `view-state/only-a-workflow-tab`, `only-a-dashboard-tab` and `starter-tabs-closed`.

**Tests first** (they fail before the fix):
- `a project's state isn't saved before its load answers` (a switch with a slow load; the stored tabs survive);
- `flush saves only projects with a pending change`, `a removed project isn't saved by flush`;
- `each project keeps its own activeView`;
- `pagehide sends the pending save with keepalive` (web), `a large pending save isn't sent with keepalive`;
- `the reconcile saves only changed dashboards`;
- `a stream that throws ends the turn and saves it`, `deleting a streaming chat aborts it`, `flush saves the streaming chat's messages`;
- `onboarding on web writes nothing and shows no toast`, `the license nudge's answer isn't saved after a failed load`, `one bad tutorial row keeps the others`;
- `starter tabs for a project leave the active project alone`;
- `a saved chart node stores no rows when its source holds them`, `a chart node reopens with its source's rows`, `a workflow saved with chart copies still loads, and its next save drops them`, `a chart whose source isn't in the workflow keeps its rows`.

**Run:** `mise exec -- npx vitest run src/lib/hooks/database src/lib/stores src/lib/components`; `mise exec -- npm run check` 0/0; the autofixer on changed `.svelte` files.

**Review:** no project-state save can run for a project this page hasn't loaded; the keepalive path sends at most one request and never a body over the cap; the reconcile compares persisted forms, not objects.

**Things this task could quietly skip:** the chart node that holds rows its source doesn't (keep them); the startup path (`project-manager.svelte.ts:163-167`) as well as the switch; clearing the `loaded` entry on project removal; the demo (no `pagehide` path needed there, but the `loaded` rule applies).

### Task 2: State fixtures

**Files:** `crates/seaquel-workspace/tests/fixtures/state/` (`README.md`, `changes.json`, and case files `view-state.json`, `workflows.json`, `dashboards.json`, `chats.json`, `settings.json`, `ai-settings.json`, `themes.json`, `misc.json` for onboarding, tutorial and import state, `old-data.json`), and the recorder artifact `docs/plans/artifacts/2026-10-04-record-state-fixtures.test.ts.txt`, run as `src/lib/hooks/database/record-state.test.ts` with `FREEZE_STATE=1`. Same format as `library/` (its README's case and step fields, `<id:n>` keeping the prefix, `<now>`, a fake clock and uuid counter); each step's Core request is the 5d-2 wire form (Decisions 20–24).

**Cases, at least 60:**
- view state: each tab type saved and loaded; pane layouts (one pane, several); every active id; `activeView` per project; the legacy canvas fields; an extensions tab; a repeated tab id; a save before the load (refused since Task 1); the Rust replay checks the legacy mirror rows equal today's save's `project_state` and `tabs` rows (without `saved_canvases`), and that the first load in a new window returns today's rows;
- workflows: create, rename, update with result rows (bigint and bytes cells, through `toStorable`), a chart on a result node (stored without rows), a pre-5d-2 workflow holding chart copies (loads, and loses them only on its next `saveWorkflow`; a project-state save before that sends the copies as held in memory), delete, one that doesn't decode, two in one project;
- dashboards: create, each patch field, star alone, share and unshare, delete, a version per versioned edit and none for a move or pan, prune at 3 and at 0 under both limits, a delete during a pending edit, two with one name;
- chats: create, retitle, delete, a turn (user message, then the assistant's at the end), an error turn, Stop, an abort with an approval pending, two messages with one timestamp; (the 64 MiB budget is web-only and pinned by Core tests, not fixtures);
- settings: each key with a good and a bad value, `null`, an unknown key, `query_version_limit` 0;
- AI settings: add, update and remove a provider with and without an API key (recorded keychain), the privacy flags, `enabled`, a record with legacy fields, one that isn't JSON, one with `providers: [null]`, two tabs editing different providers (recorded as today's loss);
- themes: add, update, delete (in use and not), preferences, a row that doesn't parse;
- onboarding, tutorial (with a bad row), import state round trips;
- old data: every seed in "Stored data to expect".

**Run:** record twice, byte-identical; `git diff --stat` shows only the fixtures and the artifact; `mise exec -- npx vitest run src/lib/hooks/database` passes once the copy is deleted.

**Review:** every entity and every re-survey bug with a stored effect has a case; `changes.json` names each intended difference with its Decision (the list in "Parity fixtures"); the README names the recorder's commit, the normalising rules and what isn't recordable (events, `seq`, origins, `rev`).

**Things this task could quietly skip:** the old-data seeds; the keychain calls of AI keys; a beta-era file for dashboards; the mirror comparison needing today's exact `tabs` rows, not just the state JSON.

### Task 3: Storage

**Files:**
- `crates/seaquel-storage/migrations/0002_window_state.sql` (Decision 22's SQL, plus the dashboards' stale-key trigger) and the migrations README.
- `src/data_steps.rs`: `backfill_dashboard_name_keys`, appended; `lib.rs`: `refill_name_keys` covers dashboards.
- `src/queries/`, every write taking `&mut WriteTx`, every read `impl Into<Reader>`:
  - `dashboards::{get, list, insert, update, delete, with_name_key, count}`; `dashboard_versions::{append, list_meta, list_by_project, delete_ids}` (numbering `MAX(version) + 1` inside the transaction);
  - `saved_canvases.rs` (new): `{get, list, insert, update, delete, count}`;
  - `ai_chats::{get, insert, update, delete, count, put_messages, delete_messages, message_count, message_chat_ids, content_bytes}`, and `load_messages` ordered by `timestamp, rowid`;
  - `app_state::{get, set_in, delete_in}`; `themes::{preferences, set_preferences, list, get, insert, update, delete, count}`; `onboarding::{get, set}`; `tutorial::{list, save, remove_lesson, remove_all}`; `import_state::{get, save}` (all in a `WriteTx`);
  - `windows.rs` (new): `{get, touch, set_active_project, prune}`; `window_state.rs` (new): `{get, most_recent, put_if_newer, prune_for_project}`;
  - `project_state::{sidebar, set_connection_order, write_legacy_mirror}`: the mirror writes the state row and the `tabs` rows from `MirrorRows`, keeps the stored connection order, writes the state's `activeConnectionId` (Q14), never touches `saved_canvases`, and skips a repeated tab id.
- `project_state::save`, `ai_chats::replace_all_messages`, `themes::save_user_themes` and the other replaced writes stay for the frozen repo fixtures only.
- Tests: `tests/state.rs` (new), `tests/baseline.rs` (0002 on every frozen release, beta-era included), `tests/open.rs` (the read-only open refuses a file with 0002 pending).

**Tests first:**
- `migration_0002_applies_on_every_release_schema`, `a_read_only_open_refuses_a_file_with_0002_pending`;
- `the_dashboard_name_key_step_fills_old_rows`, `an_older_release_renaming_a_dashboard_nulls_its_key`, `refill_covers_dashboards`;
- `dashboard_versions_number_after_the_highest_inside_the_transaction`, `dashboard_delete_takes_its_versions_on_a_beta_file`;
- `put_messages_upserts_and_keeps_order_on_equal_timestamps`, `a_message_id_of_another_chat_is_reported`;
- `the_legacy_mirror_equals_todays_save_without_canvases`, `the_mirror_keeps_the_stored_connection_order`, `the_mirror_skips_a_repeated_tab_id`;
- `put_if_newer_ignores_an_older_rev`;
- `content_bytes_uses_the_chat_index` (`EXPLAIN QUERY PLAN`, and timed on a chat of 5,000 1 MiB-bounded messages);
- `most_recent_uses_the_index` (`EXPLAIN QUERY PLAN`), `each_prune_is_one_bounded_delete_and_spares_the_given_window`;
- `setting_delete_removes_the_row`;
- `every_5d2_write_takes_a_write_tx` (a test that holds a `WriteTx` and checks each write waits).

**Run:** `cargo test -p seaquel-storage`; CI clippy; `EXPLAIN QUERY PLAN` output pasted in the review for every count, name lookup and prune.

**Review:** no 5d-2 write on `st.pool()`; the migration expand-only and working on the beta baseline; every scan indexed; the frozen fixtures untouched; `baseline.rs`'s `MIGRATION_COLUMNS` gains `dashboards.name_key`.

**Things this task could quietly skip:** the stale-key trigger for dashboards; the beta-era baseline for `0002` (dashboards there have no foreign key and `project_id` is last); the data step's linear-time test; `delete_in` versus writing NULL.

### Task 4: Core, state, settings and window view state

**Files:**
- `crates/seaquel-workspace/src/state.rs` (new, pure): the drafts, patches and params types of "The wire and the API", their checks, `read_ai_settings`, the AI settings rewrite, the onboarding merge, `legacy_mirror`, the dashboard version prune (sharing 5d-1's `version_prune`), `StateLimits`, `StoredKind`'s new kinds (in `library.rs`, where the enum lives), the setting key set and their value checks; `tests/state_plan.rs`.
- `crates/seaquel-core/src/state.rs` (new): one `Workspace` method per call, each on 5d-1's pattern (`library.rs:586-616`: check, then `write()`, `take_seq()` under the lock, reads through `&mut tx`, commit, `announce`); `CoreBuilder::state_limits`.
- `crates/seaquel-core/src/lib.rs`: `refill_name_keys` after the open, as today (`:622`).
- `crates/seaquel-server/src/lib.rs`: `WEB_STATE_LIMITS` in `web_core()`.
- `crates/seaquel-mcp/tests/tools.rs`: the MCP reader against records Core wrote.
- Tests: `crates/seaquel-core/tests/{state,settings,window_state}.rs`.

**Tests first:**
- Pure:
  - `replays_every_fixture_check` with `changes.json` exactly;
  - `unknown_setting_keys_are_refused`, `each_setting_value_is_checked`, `core_owned_keys_are_refused`;
  - `ai_settings_read_matches_todays_load` (the legacy and malformed seeds), `the_rewrite_keeps_fields_it_doesnt_know_byte_for_byte`;
  - `the_mirror_of_a_state_equals_todays_rows`, `a_repeated_tab_id_is_skipped`;
  - `limits_are_the_interfaces`, `checks_never_panic` (proptest over the drafts), `debug_shows_no_text_names_or_values`.
- Core:
  - `replays_every_fixture`;
  - `two_windows_editing_different_providers_both_land`, `an_api_key_is_written_before_the_record_and_taken_back_on_failure` (desktop, `MemoryStore`), `an_api_key_on_web_is_not_supported`, `removing_a_provider_deletes_its_key_or_vault_rows`;
  - `a_dashboard_star_is_saved_without_touching_updated_at`, `a_version_only_when_asked`, `dashboard_limit_zero_keeps_all`, `an_update_of_a_removed_dashboard_is_not_found`, `dashboard_names_clash_within_a_project`;
  - `a_workflow_gets_core_id_and_times_and_keeps_the_rest_byte_for_byte`, `project_state_writes_leave_workflows_alone`;
  - `messages_are_upserted_not_replaced`, `a_message_id_of_another_chat_is_refused`, `removing_a_chat_takes_its_messages`;
  - `a_put_past_the_chat_budget_is_refused_and_stores_nothing`, `replacing_a_message_counts_its_new_size_not_both`, `desktop_has_no_chat_budget`, `chat_messages_list_answers_the_stored_bytes`;
  - `a_workflow_past_16_mib_is_refused_on_web`, `desktop_has_no_workflow_cap`;
  - `removing_the_theme_in_use_resets_the_preference`;
  - `a_new_window_copies_the_most_recent_then_legacy_then_empty`, `the_copy_is_written_as_the_windows_row`, `a_window_save_writes_the_legacy_mirror_and_keeps_the_sidebar`, `an_older_rev_is_stale`, `pruning_is_bounded_and_spares_the_saving_window_and_main`, `a_window_id_other_than_the_origin_is_refused`, `window_activate_writes_last_active_project_id`;
  - `limits_refuse_before_anything_is_read`, `counts_are_checked_inside_the_transaction`.
- Events: `every_state_write_emits_one_event_after_commit`, `a_refused_write_emits_nothing`, `a_view_state_event_names_only_its_window`, `message_events_are_scoped_to_the_chat_and_bounded` (6,000 messages put: `ids: None`), `seq_follows_commit_order_across_library_and_state_writes`.
- MCP: `the_global_default_written_by_core_is_what_mcp_reads` (in `crates/seaquel-mcp/tests/tools.rs`, next to `:1046-1070`).
- Logs: `no_text_names_json_or_keys_in_logs` (`capture_logs`, canaries in tab text, widget JSON, message content, theme JSON, setting values and API keys).

**Run:** `cargo test -p seaquel-workspace -p seaquel-storage`; `cargo test -p seaquel-core --features seaquel-runtime/tokio`; `cargo test -p seaquel-mcp -p seaquel-cli`; CI clippy, both wasm32 lines, `npm run crates:check`.

**Review:** validation before the first write; no pool call and no keychain call inside a `WriteTx` (the API key is written before `write()` and taken back after a failed commit); `seq` taken under the lock; each answer holds whole what its `seq` covers; the MCP tests unchanged and the new one passing; the legacy fallbacks match today's loads on every seed; no Core lock across an await.

**Notes from Task 3's review (storage as built):**
- `window_state::put_if_newer` answers `Put { written, rev }`: on a stale save `rev` is the stored one. `windowStateSave` answers `{stale, rev}` with it, and **a stale save writes nothing else**: no legacy mirror, no prune.
- `project_state::write_legacy_mirror` takes a `PersistedProjectState` (storage can't name a `seaquel-workspace` type), ignoring its connection order, starred lists, workflows and extensions tabs; `legacy_mirror` produces that type. It returns the count of repeated tab ids it skipped.
- Themes: `themes::insert`/`update` store the JSON under the row id they are given. Core must keep the theme JSON's top-level `id` equal to that row id (set it when it makes `theme-<uuid>`, and on update), since today's load and the frozen `save_user_themes` read the id from the JSON.
- The prunes take the cutoff time and the spared window ids from Core (`windows::prune(tx, unused_before, max_windows, spare)`, `window_state::prune_for_project(tx, project, max, spare)`): the saving window always, and `main` on desktop.

**Things this task could quietly skip:** keeping unknown `aiSettings` fields; the providers-with-`null` fallback; `rev`; `lastActiveProjectId` in `windowActivate`; the vault rows of a removed provider on web; the dashboard versions' byte budget on web; the chat budget counting a replaced message once; `StateLimits` in the server's test Core as well as `web_core()`.

### Task 5: RPC and transports

**Files:**
- `crates/seaquel-rpc/src/{settings,ui}.rs` (new), `library.rs` (the 5d-2 methods), `workspace.rs` (`Request::Settings`, `Request::Ui`, the 32 retired storage variants, `storage_change` shrinking to the vault, license, shared repos and history), `db.rs` (`CoreEvent` unchanged but for the kinds).
- `crates/seaquel-server/src/error.rs`: the new 404s. `routes/rpc.rs`: nothing new beyond the groups; `/rpc` serves them.
- `src-tauri/src/lib.rs`: the storage-backed arm serves the two groups with the webview label.
- `src/lib/storage/{client,rust-client,sqljs-client}.ts`: the retired methods reject `NOT_SUPPORTED` until Task 6 (as 5d-1's Task 5 did), `STORAGE_METHOD_KIND` shrinks.
- `npm run types:gen`.

**Tests first:**
- rpc: wire snapshots of every new method; `Clearable` and `RawValue` round-trips; `unknown_request_fields_are_refused` extended to `settings`, `ui` and every new `library` method (the test at `crates/seaquel-rpc/tests/library.rs:270-311`); `a_retired_storage_method_is_unknown` for all 32; `debug_redacts_every_new_params_type`.
- server: `another_users_ids_are_not_found` for every new call (B's file unchanged, `sqlite3`); `a_window_id_that_isnt_the_origin_is_refused_over_http`; `a_state_write_reaches_every_socket_of_that_user_and_none_of_another`; `the_web_state_limits_apply`; `an_api_key_over_web_is_not_supported`; `state_calls_log_group_method_and_code_only`.
- src-tauri: `core_call_serves_settings_and_ui_with_the_webview_origin`.

**Run:** `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio`; `mise exec -- npm run cli:build && cargo test -p seaquel --lib`; `types:gen` twice, no diff the second time; `npm run check` 0/0.

**Review:** the envelope and every new params type refuse unknown fields; no new route and no new forwarded header in Node; `dispatch_workspace` still refuses SSH, git and licensing; nothing but vault, license, shared-repo and history writes left in the storage group.

**Things this task could quietly skip:** the server's `another_users_ids_are_not_found` for the `ui` calls (a window id of user B named by A); the retired variants' TS stubs, which leave desktop and web unable to load tabs between Tasks 5 and 6 (don't release in between).

**Notes from Task 5 (as built):**
- **Wire.** `Request::Settings` and `Request::Ui` (`crates/seaquel-rpc/src/{settings,ui}.rs`) and 18 `library` additions (Decisions 21, 23, 24, plus `projectSidebarGet` next to `projectSidebarSet`, Task 4's pick for the `project` refetch). Every params object refuses unknown fields; `settingGet`/`settingSet`'s `key` is a `String` on the wire (`SettingKey` only in the generated TypeScript), so an unknown key is Core's `INVALID_ARGUMENT` naming it; `settingSet`'s `value` must be present (`null` deletes). `apiKey` is `Clearable` (absent keep, `null` delete). JSON bodies are `RawValue` both ways; answers holding one are typed `SeqdJson`/`SeqdJsonList` (`{value: unknown, seq}`) in TypeScript. `chatMessagesRemove` answers the count removed. The three groups' request and response `Debug` shows the method (and a response's `seq`) only.
- **Retired: 35 storage methods, not 32** (the plan's count missed three): `aiChats*` (6), `appState*` (2), `connectionOverrides*` (4), `dashboardVersions*` (4), `dashboards*` (4), `importState*` (2), `onboarding*` (2), `projectState*` (3), `themes*` (4), `tutorial*` (4). The storage group keeps `queryHistory*`, `sharedRepos*`, `license*`, `vaultState*` and `userCredentials*` (15), and `storage_change` covers only those.
- **Statuses:** `DASHBOARD_NOT_FOUND`, `WORKFLOW_NOT_FOUND`, `CHAT_NOT_FOUND`, `THEME_NOT_FOUND`, `AI_PROVIDER_NOT_FOUND` 404; `STORAGE_FULL` 507 was already mapped. The new calls count toward the per-user in-flight bytes and the body limits like every call, and not toward the four-at-once edit cap.
- **The web origin is still per page load** (`webPageOrigin()`), so on web a `ui` call works only when it names that id, and a tab's view state doesn't survive a reload. **Task 6a must make it the window id** before the first Core call (Decision 22); nothing else in the transport changes. `RustStorageClient.saveWindowStateKeepalive` (the `pagehide` send) already carries `X-Seaquel-Origin`, like every call.
- **The TypeScript seam for 6a/6b:** `RustStorageClient.settings(method, params)` and `.ui(method, params)` next to `.library(...)`, writes on the same queue (`windowStateLoad` counts as a write, since a first load copies a row), `SETTINGS_METHOD_KIND`/`UI_METHOD_KIND`, and `sendKeepaliveRequest(request, what)` for any group.
- **What is broken on desktop and web until 6a/6b** (don't release in between): `RustStorageClient`'s retired methods reject with `NOT_SUPPORTED` without sending anything, except `appState.get`, which reads through `settingGet` so the storage gate's probe still sees `LEGACY_STORAGE`/`STORAGE_CORRUPT`/`NO_DATA_DIR`, and settings in the closed set still load (`aiSettings` isn't in it, so the AI settings store's load fails). So: open tabs, layout and saved workflows neither load (failed-load guard, so nothing is saved over them) nor save, and the `pagehide` keepalive sends nothing; every setting write fails (key bindings, pending changes, version limits, `lastActiveProjectId`, the license nudge, the skipped update, the connection-secrets notice's clear); AI settings, themes (the defaults apply), onboarding, tutorial progress, TablePlus/DBeaver import state, dashboards and their versions, and AI chats and messages neither load nor save. The library, history, shared repos, license, vault, secrets and `db` calls work as before. The demo is untouched (sql.js).

### Task 6a: The GUI's view state and window identity

**Files:**
- `src/lib/core/window-id.ts` (new): desktop, the webview label; web, `win-<uuid>` in `sessionStorage` with the `BroadcastChannel` duplicate check (100 ms), resolved once before the storage gate. `src/lib/core/origin.ts`: `webPageOrigin()` returns the window id. `routes/(app)/+layout.svelte` (and the root layout, where the gate runs) awaits it.
- `src/lib/hooks/database/window-state.svelte.ts` (new): the debounced per-window, per-project save through `ui.windowStateSave`, `rev`, the load-before-save rule, the `pagehide` keepalive send (replacing Task 1's), and the load through `ui.windowStateLoad` with the fallback it reports.
- `persistence-manager.svelte.ts`: **goes.** Its serializers move to `window-state.svelte.ts`; the shared-repo save moves into `shared-repo-manager.svelte.ts` (it stays in the storage group); dashboards, chats and overrides go with 6b and Decision 25.
- The 35 call sites: the wrapper at `hooks/database.svelte.ts:113-115` now schedules the window-state save, so the 29 tab and layout calls need no change beyond the constructor; the four workflow calls go to 6b; `connection-manager.svelte.ts:634`, `:1208` and `project-manager.svelte.ts:475`, `:909` (the connection order) call `library.projectSidebarSet` at once; `:923` and `:1189` (the active connection) schedule the window-state save like any tab change (Q14).
- `project-manager.svelte.ts`: `setActive` and `initialize` call `ui.windowActivate`; `loadProjectState` reads `windowStateLoad` and still restores the legacy canvas fields; `lastActiveProjectId` is no longer written from here.
- `library/sync.ts`: `projectState` events handled by Decision 22's rule; `project` events refetch the sidebar row too.
- `storage/load-guard.ts`: `LoadKey` loses `projectState:*` and gains `windowState:*`.
- The demo: `TsLibrary` (or `TsState`) serves the `ui` calls over sql.js with window id `demo`.

**Tests first (vitest):**
- `a tab's layout isn't moved by another tab`; `a new tab starts with the most recent window's tabs`; `a reload keeps the tab's own tabs`; `a duplicated tab gets a new window id before its first call`; `the first window after the upgrade gets today's tabs`; `a call naming another window's id is refused`;
- `no view-state save before its load answers` (the switch and startup cases); `an older queued save doesn't overwrite the pagehide save` (`rev`);
- `the connection order is saved at once and appears in another tab`; `each window keeps its own active connection` (Q14);
- `the web origin is the window id on every call and on the socket`;
- the storage-gate, failed-load and starter-tab tests pass, rewritten where they named `projectState`.

**Run:** `npm run check` 0/0; `CI=1 mise exec -- npx vitest run`; `npx oxlint --type-aware --type-check --deny-warnings`; the autofixer; `build`, `build:web`, `build:demo`; live on desktop and web: the manual checks' view-state items.

**Note from Task 3's review:** a `stale` answer to `windowStateSave` carries the stored `rev`. The page moves its counter past it (the next save sends `rev + 1`) or reloads the window's state; otherwise every later save from that page stays stale.

**Review:** the window id settled before the first Core call on web; the load-before-save rule on every path that can schedule; `rev` continuing from the load, and past a stale answer's `rev`; no replace-all save left (`rg "projectState\.save|persistProjectState" src -g '!*.test.ts'` empty).

**Note (from Task 1's review):** Task 1's keepalive skips a save whose body is over 60 KiB (logged as a `warn` with the size) and leaves it to the ordinary flush. Today the project state carries the saved workflows, so a project with workflow results is nearly always over the cap. Once workflows leave the project state (Decision 23), the window's view state is much smaller and the keepalive covers most projects; the size check and the `warn` stay.

**Things this task could quietly skip:** the socket URL's origin (`core/http.ts:125`); the startup path; `activeView` per project in the stored state; the extensions tabs in the state; the desktop close flush (awaited, now through `window-state`).

**Follow-up from Task 2's review:** `hasSavedTabs` (`project-manager.svelte.ts`) counts a saved tab whether or not the restore keeps it. It should count only tabs that survive the restore filters (a schema, ERD, statistics, workflow, create-table, data or extensions tab whose `connectionId` is missing or names a connection that's gone; a dashboard tab with no `dashboardId`), so a project whose saved tabs are all dropped still gets its starter tabs. Add a case for it with the window-state work.

**Notes from Task 4's review (Core as built):**
- **Over-limit states stay saveable.** A view state stored before the web limits (or copied from one on the first load) can be past `max_view_state_bytes`, `max_tabs` or `max_tab_text_bytes`. Core still accepts a save that is no larger than the window's stored row, item by item (the whole state against the stored state, a tab's text against the same tab id's stored text), and refuses only growth past a limit (`INVALID_ARGUMENT` naming the limit, and `tab <id>` for a tab's text). The GUI surfaces such a refusal **once** per project and limit, naming the limit and the tab, and keeps saving (the user trims the tab to get under it); it doesn't toast on every 500 ms save.
- **`windowStateLoad` `empty` over an unreadable own row** answers that row's `rev` (not 0): count up from the answered `rev` in every case.
- The legacy mirror writes an `activeView` of `canvas` as `workflow`; the window's row keeps the state as sent.

**Notes from Task 6a (as built):**
- **Window id** (`src/lib/core/window-id.ts`): desktop the webview label, web `win-<uuid>` in `sessionStorage` (key `seaquel.windowId`) with the claim/answer check on the `seaquel-window-ids` `BroadcastChannel` (100 ms, skipped for a freshly made id), demo `demo`. `windowIdReady()` resolves it once; `windowId()` is the settled id or `null`. `webPageOrigin()` returns it (`null` before it settles) and `newOrigin`/`ORIGIN_PATTERN` moved there (re-exported from `origin.ts`). It is settled before the first Core call three ways: `StorageGate`'s probe and the `(app)` layout's `onMount` await it (the root layout makes no storage call, so nothing was added there), `httpCoreTransport` awaits it before every `fetch`, and `HttpCoreClient` builds its default socket URL only when it is known (it waits otherwise). `sendKeepaliveRequest` sends nothing before it settles.
- **`WindowStateManager`** (`window-state.svelte.ts`) replaces `PersistenceManager` for the view state: debounce, load guard (`windowState:<projectId>`), the load-before-save rule, `rev` (from the load's answer; past a stale answer's `rev`, then the page's state is scheduled again), flush of pending projects only (active first), the `pagehide` keepalive, and the serializers. A save refused for a limit (`max_*` in Core's message) is shown once per project and limit (new key `view_state_save_refused`); others are logged. `activeProject()` is `windowGet`, `activate()` is `windowActivate` (skipped while the projects are an in-memory stand-in). A standalone window (`/windows/…`, the theme editor) runs it disabled: no `ui` call at all, so it never gets a row.
- **The seam:** `UiService` in `library/types.ts`, `CoreUi` (the queued `ui()` calls and `saveWindowStateKeepalive`) and `TsUi` (the demo), picked by `getUi()`/`setUi()` in `library/index.ts`. `LibraryService` gained `getProjectSidebar`, `setProjectSidebar` and **`listWorkflows`** (a read only: the view state no longer carries saved workflows, so `loadProjectState` reads them from `workflowsList` and decodes with `fromStorable`). `RecordingLibrary` and `TsLibrary` implement the three.
- **The demo** (`TsUi`): new `windows` and `window_state` tables in the sql.js schema (`IF NOT EXISTS`, so existing demo files get them), Core's rules for the origin check, copy-on-first-load (most recent window, then `project_state`/`tabs` through `projectStateRepo.load` minus the four non-view fields, then empty), `rev` and `windowGet`/`windowActivate` (with `lastActiveProjectId`). No legacy mirror, no prunes, no limits (one window, no older release reads it). The GUI tests use `TsUi` over sql.js with several window ids on one database.
- **`ProjectManager`:** `initialize` asks `windowGet` (then checks the id against the listed projects) and calls `windowActivate`; `setActive` saves the old project's view state, then `windowActivate`s and loads. `lastActiveProjectId` is no longer written from TypeScript. `loadProjectState` reads the view state, the sidebar (`projectSidebarGet`, `seq` rule under `projectSidebar:<id>`) and the workflows in parallel; the legacy canvas fields are still restored. `refreshFromLibrary` (a `project` event, and `reloadAll`) also refetches the connection order of loaded projects. `reloadViewState` reads a project's view state again (the active one; another is read when opened). `LibrarySync` calls it for Decision 22's own-id `projectState` rule, which **never fires in practice**: Core refuses a `ui` write whose window id isn't the caller's origin, so such an event always carries this page's origin, and the feed skips own-origin events first. The handler is kept (with a comment) in case either check changes.
- **Connection order** is stored at once (`storeConnectionOrder` in `library/view.ts`, new key `connection_order_save_failed` on failure) from the four listed sites: `ConnectionManager.reorder` and the TablePlus/DBeaver import, `ProjectManager`'s git-path clear and imported shared connection. Appending a new connection to the in-memory order isn't stored, as today: the sidebar puts unknown ids last. The active connection is in the view state (Q14).
- **Task 2 follow-up done:** `hasSavedTabs(saved, known)` counts only tabs the restore keeps, and the restore itself now also drops a connection-bound tab naming a connection that's gone, but only once the connections are known (`ConnectionManager.loaded`; at startup the project loads before them).
- **Deviation: `persistence-manager.svelte.ts` is deleted, but what 6b owns moved to `dashboard-chat-persistence.svelte.ts` (`DashboardChatPersistence`), marked at its top as 6b's to finish and delete:** the dashboard saves, version limit, version insert/prune and loads; the AI chats' debounced saves, message saves, loads (with the `aiMessages:*` guard), removal and the streaming chat's flush; and the retired override functions. They still use the retired storage stubs (`NOT_SUPPORTED` on desktop and web). `UseDatabase.persistence` is that class now; `UseDatabase.flush()`/`saveOnPageHide()` run the window state, the shared repos' save (moved into `SharedRepoManager`: `loadPersistedRepos`, `persistRepos`, `flushPersistence`) and it, and the layout calls those. History loads read `getStorage().queryHistory` in `StateRestorationManager`.
- **For 6b:** the four workflow calls still schedule a window-state save, which no longer carries saved workflows, so **workflow edits aren't stored anywhere until 6b** (on every build, the demo included); loading already works. `ProjectManager` takes the dashboard store as its optional fourth argument for the reconcile. `StorageClient.projectState` is gone (the storage client test treats the `projectStateRepo.*` fixture steps like 5d-1's retired library calls); the other retired repositories stay for 6b.
- **Tests:** `window-id.test.ts`, `window-state.svelte.test.ts` (the spec's list except the order test, which is in `library/sync.test.ts` with the feed), a storage-gate case, the `hasSavedTabs` cases; `project-state-save.svelte.test.ts` is replaced by `window-state.svelte.test.ts`; `failed-load`, `library-persistence`, `library-replay`, `dashboard-reconcile`, `sync` and the connection manager suites moved off `PersistenceManager`. Most of the new tests were written alongside the code rather than strictly first; the storage-gate and `hasSavedTabs` cases were seen failing before the change.

**Task 6a review fixes (as built):**
- **I1:** `windowStateLoad` answering no longer makes a project saveable. It stays pending until `ProjectManager.loadProjectState` has put the state in memory and calls `WindowStateManager.markLoaded(projectId, restored)` (a restore that throws leaves it unsaveable). So a save fired while the sidebar or workflows read is still out sends nothing, on startup and on a switch. The restore is now `restoreViewState`.
- **M1:** each claim on the window-id channel carries a random nonce; a page still checking answers a claim for the same id, and the lower nonce makes a new id, so two pages checking one id at once don't both keep it.
- **M2:** once the window id settles, the stream socket opens only if a subscriber or stream still needs it (`socketNeeded()`).
- **M3:** the own-id `projectState` note above is corrected; code comments say the same.
- **M4:** `flush` also waits for each project's save already on its way, and saves again the projects a stale answer re-scheduled, at most two more rounds.
- **M5:** a stale answer for a project this page hasn't changed since its restore (no save was scheduled after `markLoaded`) reads the window's view state again instead of saving over it, so an old page's late `pagehide` save survives the reload that followed it. A project changed here is saved again past the stored `rev`, as before. **M5 covers only a stale save with no change since the restore:** a save caused by a change (the user's edit, or auto-reconnect's `setActiveForProject`) still counts past the stored `rev` and overwrites the old page's newer `pagehide` save. That is accepted.
- **N1 (re-review):** a reload of a project already shown here (`load(…, {reload: true})` from `reloadViewState`) whose read fails leaves the tabs on screen and the project saveable at its rev; a first load or a switch keeps the old behaviour (empty state, saves refused).
- **N2 (re-review):** the save `setActive` makes on its way out is `saveNow(…, {leaving: true})`; a stale answer to it, or to any save of a project that is no longer active, doesn't reload: the project just counts as not loaded (`dropLoad`), and the next switch reads it. A reload also checks the active project again after its reads, before it cancels runs and restores.
- **M6:** before a restore drops tabs naming connections the page doesn't list, it reads those connections once (`ConnectionManager.refreshFromLibrary(missing)`), and drops only the ones still missing.

### Task 6b: Dashboards, workflows, chats, settings, and the feed

**Files:**
- `library/{types,core-library,ts-library}.ts`: the 5d-2 `library` methods; new `settings` and `ui` clients (`CoreSettings`, or methods on the same seam; the executor picks one and keeps the demo's twin beside it).
- `dashboard-manager.svelte.ts`: create, patch (with `captureVersion` where it versions today), star, share, remove, restore through Core; versions spliced from the answer and the list re-read (not the answer's `seq`); `stopAllAutoRefresh` on a remote delete. `state-restoration.svelte.ts`: dashboards and versions through `dashboardsList`/`dashboardVersionsList`, an empty result applied (bug 18), loaded once per activation.
- `workflow-manager.svelte.ts`: `saveWorkflow` async through `workflowCreate`/`workflowUpdate`, delete and rename through Core, `activeWorkflowId` checked against the project (bug 22).
- `ai-chat-manager.svelte.ts`, `ui-state.svelte.ts`: `chatCreate` at once, `chatUpdate` for titles and times, `chatMessagesPut` at the points Decision 24 lists, `chatRemove` after aborting.
- The stores: `ai-settings`, `theme`, `onboarding`, `tutorial-progress`, `editor-settings`, `pending-changes-settings`, `update`, `license-nudge`, `tableplus-import`, `dbeaver-import`, `connection-secrets-notice`, and `components/settings/general/query-history-section.svelte` (with `errorToast`), each through `settings`, each applying another tab's change at once (bug 23: a setter before the load waits for it).
- `services/keyring.ts`: the AI key calls go on desktop (Core writes them); web keeps the vault.
- `library/sync.ts` (`LibrarySync`): the new kinds, and `reloadAll` covering them on `onResubscribed` and a new epoch:
  - `dashboard`: refetch the project's dashboards if loaded and update them in place; a deleted one closes its tabs in every project. (Changed in the 6b review: no banner. A refused edit reverts to the stored state and another window's change applies once this page's writes to the dashboard have answered, so a dashboard never holds an unsaved local change.)
  - `workflow`: refetch; a deleted one unlinks its tabs (they keep their canvas);
  - `chat` and `chatMessages`: refetch the connection's chats or the chat's messages, except the chat streaming here, which refetches when its turn ends; a deleted chat that is open switches to another, aborting a stream first;
  - `setting`, `aiSettings`, `theme`, `onboarding`, `tutorial`, `importState`: reload the record and apply it; a deleted active theme falls back to the default.
- Messages for `max_chat_bytes` ("This chat is full. Start a new chat to continue.") and `max_workflow_bytes` in `library/messages.ts`; the chat panel's full state with a "New chat" button.
- New i18n keys, through `i18n-translator`.
- Overrides retired (Q13, Decision 25): delete `shared-connection-manager.svelte.ts`, the override functions in `persistence-manager.svelte.ts` (gone with it in 6a), `connectionOverrides` in `storage/{client,rust-client,sqljs-client}.ts` and `state.connectionOverrides`; `library-persistence.svelte.test.ts:322`'s case goes.

**Tests first (vitest):**
- `a workflow saved in one tab appears in the other`, `a theme added in one tab appears in the other`, `two tabs adding providers keep both`, `a dashboard renamed in one tab is renamed in the other`, `a dashboard deleted elsewhere closes its tabs`, `a chat deleted elsewhere while streaming stops and switches`, `a streaming chat ignores its own events until the turn ends`;
- `a dashboard move records no version, a widget edit does`, `a message put carries only the changed messages`;
- `a full chat says so, keeps the turn on screen and disables sending`, `a full chat opens disabled`, `a workflow too large to save says so and keeps it open`;
- `a theme changed in one tab applies in the other` (Q18);
- `settings from another tab apply at once`, `a setting set before its load waits for it`;
- `every new kind reloads on resubscribe and a new epoch`;
- `the demo replays the state fixture cases`;
- the AI, run, dashboard and workflow suites pass.

**Run:** as 6a, plus the manual checks' items for dashboards, chats, workflows and settings.

**Review:** the skip list; `rg "replaceAllMessages|saveUserThemes|appState\.set|dashboards\.save|crypto\.randomUUID" src/lib/hooks/database src/lib/stores -g '!*.test.ts' -g '!library/ts-library.ts' -g '!library/ts-settings.ts' -g '!library/ts-state.ts' -g '!library/ts-ui.ts'` shows only GUI-made ids (messages, widgets, tabs, panes, nodes, streams) and the reconcile placeholders Core replaces; no store saves a whole record.

**Things this task could quietly skip:** `connection-secrets-notice` and the storage gate's probe moving to `settings`; the theme editor window's save path (it goes through the main window's store); the license nudge's per-query write (now `settingSet`, still whole); an open chat's messages on `onResubscribed` while it streams.

**Notes from Task 4's review (Core as built):**
- **Chart copies are stripped GUI-side, on `saveWorkflow` only** (corrected in the 6b review; this note first said every `workflowUpdate`). Core stays opaque to the workflow JSON; `saveWorkflow` drops a chart node's rows when its source in the workflow holds them, and a rename sends the stored workflow as held, so a workflow saved before 5d-2 keeps its copies until its next `saveWorkflow` (Decision 23, `workflows/pre-5d-2-chart-copies`). Trade-off: on web, a workflow over 16 MiB only because of old copies can't be renamed to a longer name (the rename grows it past `max_workflow_bytes`); saving it once drops the copies.
- **Over-limit workflows and dashboards stay saveable.** `workflowUpdate` past `max_workflow_bytes` and `dashboardUpdate` past `max_dashboard_bytes` are accepted when the stored result is no larger than what's stored now; only growth is refused (`INVALID_ARGUMENT` naming the limit). A create gets no allowance. The GUI shows such a refusal once per workflow or dashboard, naming the limit and the workflow or dashboard, and says to clear or narrow its results.
- **`STORAGE_FULL`** (507 on web): the user's `meta.db` reached `SEAQUEL_USER_DB_MAX_BYTES` (2 GiB by default). Nothing of the call was written; show it as an error toast saying storage is full.

**Notes from Task 6b (as built, updated after its reviews):**
- **The seam.** `LibraryService` gained the dashboard, workflow and chat calls; a new `SettingsService` (`library/types.ts`) with `CoreSettings` (`core-settings.ts`, the queued `settings()` calls) and the demo's `TsSettings` (`ts-settings.ts`), picked by `getSettings()`/`setSettings()`. The demo's dashboards, workflows and chats are `TsState` (`ts-state.ts`), which `TsLibrary` holds and runs through its queue (Decision 26: kept apart for readability). `RecordingLibrary` implements the new calls in memory.
- **`dashboard-chat-persistence.svelte.ts` and `shared-connection-manager.svelte.ts` are deleted.** `StorageClient` keeps `queryHistory`, `sharedRepos`, `license`, `vaultState` and `userCredentials` only; every retired stub (and `retiredStorageMethod`) is gone from `rust-client.ts` and `sqljs-client.ts`, and `state.connectionOverrides` with them (the demo's repositories stay for the frozen repo fixtures, which `client.test.ts` runs through them). The storage gate's probe is `settingGet("lastActiveProjectId")`.
- **Dashboards** (`dashboard-manager.svelte.ts`): create through Core (a refusal is shown and answers `null`); "New Dashboard" (the four callers, the dashboard view's tab, which is then renamed to the name Core stored, and the AI's create) sends `renameIfTaken` and takes the next free "<name> (n)". Each edit is a patch of what it changed, `captureVersion` on rename, widget add/update/remove, date filter and restore (not on move, resize, pan, zoom, star or share); the answer's version is spliced in, pruned ids dropped, and the stored row shows unless a later edit here is on its way.
  - **No banner** (removed in review). A refused edit reverts to the stored state: once this page's writes to the dashboard have answered, the fields the refused patch named (and the tabs' name on a rename) show the row read again, so two refused edits in flight don't bring each other back. Another window's change applies once this page's writes to that dashboard have answered; a row skipped because a write started meanwhile is read again once that write answers. So a dashboard never holds an unsaved local change for another window's change to clash with.
  - One deleted elsewhere stops its runs and closes its tabs in every project (`closeDashboardTabs`, generalised from `closeConnectionTabs`). A failed shared-file delete is said; the dashboard is still removed. Loads apply an empty list (bug 18) and `setActive` reads a project's data once per activation. `max_dashboard_bytes` is said once per dashboard; names and count limits get their own wording (`limitMessage`).
  - **The git reconcile** (`shared-dashboard-manager.svelte.ts`, `storeReconciled`): a shared row is paired with a file in two passes: by the file's `name` field (case-insensitively), then, for files and shared rows still unpaired, by the path the row would be written to (`dashboardNameToFilename(name)` equal to the file's basename, the function `writeDashboardFile`/`deleteDashboardFile` use), so a case-only rename on either side keeps the pair and dashboards whose names slug to one path keep their own files. A new file is one `dashboardCreate` with `shared: true` under the file's exact name, without `renameIfTaken`. When Core answers `NAME_TAKEN` (a local dashboard has the name) the file is skipped and said once per file per session, naming both and saying that renaming the local dashboard lets the shared one appear. Nothing Core made is dropped, nothing is paired by a fallback name, and the reconcile's own list (with `file:<path>` placeholders) is never shown: each dashboard shows as Core answers it. **For the owner to confirm** (the coordinator's decision).
- **Workflows**: `saveWorkflow(name?)` is async (`workflowCreate`/`workflowUpdate`, `toStorable` body without `id`, `projectId`, times); `activeWorkflowId` is looked up in every project the page holds, so after a switch the save updates the workflow the canvas shows where it lives, under its own name when none is given (bug 22). Rename and delete are their own calls; the four calls no longer schedule a view-state save. A rename sends the stored workflow as held (chart copies go only on `saveWorkflow`, Task 4's corrected note). `max_workflow_bytes` is said once per workflow and the canvas stays. A refetch reads again a row it skipped for this page's write.
- **Chats** (`ai-chat-manager.svelte.ts`): `createChat`, `ensureActiveChat` and `UIStateManager.sendAIMessage` are async (Core's chat id before the first message; two quick sends share one create; `sendAIMessage` answers false when nothing was sent, and the input keeps its text); titles and turn ends are `chatUpdate {title?, touched}` (a refusal is said); `persistMessages` puts only messages whose stored form differs from what was last loaded or sent (`state.aiMessagesSent`), so a failed put's messages go with the next one. A chat read again waits for this page's puts and keeps the shown messages that aren't stored as they are, merged by time. `ChatMessages.full` **comes from Core** (`chat_is_full`: stored bytes plus `max_message_bytes` past `max_chat_bytes`, or the count at `max_messages_per_chat`; never on the desktop): a chat opens full from it, and a read never clears a flag a refusal set. A `max_chat_bytes` or `max_messages_per_chat` refusal marks it full (said once, the turn kept on screen; the panel's banner with "New chat" disables input). A message whose content passes `max_message_bytes`, or whose query passes `max_query_bytes`, is said once per message and left out of later puts; a refusal whose bytes can't be read is said as an error. Stop saves the streaming chat; sending in another chat saves the streaming chat's partial turn first. The `aiMessages:*` guard is gone. The close flush puts the streaming chat's messages.
- **Settings stores** all go through `settings`; the store classes are exported. `settings-sync.ts` holds `onStoredChange`/`applyStoredChange` (stores register per kind; `LibrarySync` calls it), `WriteOrder` (only the latest write's answer shows, no flicker back; a read answering while a write is on its way is read again after it; a read after a refused write shows the stored value again) and `StoredSetting`, built on it. Themes, onboarding (which re-reads after a failed write) and tutorial progress use `WriteOrder`; the version limits are `VersionLimitsStore` (`stores/version-limits.svelte.ts`). AI settings: `addProvider(input)` answers Core's id; on web a vault write that fails after Core stored the provider throws `ProviderKeyNotSavedError` and the form edits that provider (no duplicate on retry); `updateProvider` sends only changed fields, `apiKey` `""` → `null`; on the desktop the key rides the Core call and `TauriKeyringService`'s AI set/delete reject; Core deletes a removed provider's vault rows. The store keeps unknown record fields. Themes: every write immediate, `flush()` waits for writes in flight, the theme editor's save toasts only once stored; a theme event re-applies the active theme (Q18). `STORAGE_FULL` is a toast from the settings stores' failed writes (`toastIfStorageFull`) and from every library refusal worded by `libraryErrorMessage`.
- **`LibrarySync`** follows `dashboard`, `workflow`, `chat`, `chatMessages` and the six settings kinds, and `reloadAll` covers them. A `chatMessages` event for the chat streaming here is held until its turn is stored, then read.
- **6a gap closed here:** a view-state save first stores the project's connection order when the page's differs from the one last read or stored (`storeConnectionOrderIfChanged`).
- **The TypeScript replay** (`state-replay.svelte.test.ts`) is the recorder's harness and cases rewired to 6b, comparing every step of the 112 cases (377 steps) after `changes.json`: `outcome.ok`, the dumped tables (JSON columns parsed, `<id:n>` bound), `files` and `view`. Not compared: `project_state` and `tabs` (`TsUi` writes no legacy mirror, 6a) and secrets; API keys are dropped before `TsSettings`. Two named exemptions, outcome only: `dashboards/delete-during-pending-edit#1`, `settings/editor-keybinding-mode#2` (refusals the managers show instead of throwing). `view-state/new-window-fallback` step 5 is corrected in `changes.json` (its `core` is what a GUI on Core sends; the Rust replay takes a `core` key from an entry).
- Also (second re-review): the restore after a refused dashboard edit isn't applied when a write to it is on its way or its read is older than the last recorded `seq`; another window's rename renames this window's tabs for the dashboard; a refused setting shows the stored value again (`settings/editor-keybinding-mode` step 3's view is `null` in `changes.json`); a tutorial write that shows nothing records no `seq`; closing a dashboard within the viewport debounce saves the pan instead of dropping it.
- **Known issues, left for Task 7's probe and a follow-up** (pre-existing or accepted):
  - sharing a local dashboard named like an existing git file overwrites that file;
  - renaming a shared dashboard doesn't rename its file, so the next reconcile recreates the old name as a new shared dashboard and unshares the renamed row (a case-only rename keeps the pair);
  - a save answer landing mid-drag can snap a widget back (the stored row shows unless a later edit is on its way; a drag in progress isn't one).
  - two dashboards whose names slug to one path (`nameToFilename` drops non-ASCII letters and punctuation: "Отчёт" and "Продажи" are both `untitled.json`, "Sales" and "Sales!" collide) write the same file, and the last write wins. The reconcile pairs files by their `name` field first (case-insensitively) and falls back to the slug only for files and shared rows still unpaired, so the pairing itself holds.
- **For Task 7:** probe `chatMessagesList.full` at each bound, two refused dashboard edits in flight, and the reconcile's skip notice (a git file named like a local dashboard). The shared-query reconcile's `saved-<uuid>` (`shared-query-manager.svelte.ts:180`) is a placeholder Core replaces (5d-1), left as it is.
- **For Task 8:** CLAUDE.md's "Connections in the GUI" and storage sections (the `SettingsService` seam, the retired storage properties, the override line), and the `LoadKey` list (no `aiMessages:*`).

### Task 7: Probe

A separate agent, as 5d-1's (two users, `SEAQUEL_WORKSPACE_CAP=2`, only the browser-facing endpoints plus two `/rpc/stream` sockets per user), for the new calls:

- **Cross-user.** Every new call naming user B's ids (dashboards, workflows, chats, messages, themes, providers, window ids) from A's session is not found or refused, and B's file is unchanged.
- **Input.** NUL, 1 MiB names, lone surrogates, unknown fields at every level, `Clearable` as `{}`, a setting of the wrong type, a `rev` of `-1` and `1e300`, a window id of 65 characters, a message id of another chat, widgets that aren't JSON.
- **Two tabs at once.** 100 rounds of: dashboard edits (last writer wins on widgets, both converge, version numbers unique), provider adds (all kept), theme adds (all kept), a chat streaming in one tab while the other deletes it, workflow saves.
- **Windows.** 60 browser tabs opened and closed: at most 50 window rows, each save's prune timed, the legacy mirror always equal to the latest save; a view-state save naming another tab's window id refused; a tab closed within 500 ms of typing keeps the text (keepalive); a reload keeps its tabs while another tab was used later.
- **Budgets.** A chat filled to 64 MiB refuses the next put and stores nothing; a workflow of 16 MiB after the chart copies saves and one byte more is refused; a pre-5d-2 workflow with chart copies loads.
- **Limits at scale.** 1,000 dashboards with 100 versions each, 1,000 workflows (one of 16 MiB), 10,000 chats, one chat of 5,000 messages, 200 themes, 50 providers: time the list loads, a create at full size (the name check), a save's prune, and record the file size.
- **Events.** One per write, none per refusal; a 5,000-message put is one event with `ids: None`; the 8 MiB socket bound holds with 16 MiB workflow saves.
- **Older release.** 2026.9.x opens the file afterwards and shows the latest window's tabs and every workflow and dashboard.
- **MCP.** `seaquel-cli mcp` refuses the file until the app opened it (`0002`), then follows the global AI sharing set through `aiSettingsPatch`.
- **Leaks.** No tab text, widget JSON, message content, theme JSON, setting value, API key or origin in either server log.

**Probe fixes (as built).** The probe (effort log, "5d-2 Task 7 probe") found two list answers that grew without bound within every web limit, and five smaller holes.
- **Lists without bodies (owner's choice).** `dashboardVersionsList` answers each version's id, dashboard, number, time, `widgetCount` and snapshot `bytes` (`PersistedDashboardVersionMeta`), and `workflowsList` each workflow's id, project, `name` (`""` when the stored JSON has no text name), times and `bytes` (`PersistedWorkflowMeta`). Two new reads answer one body: `dashboardVersionGet {dashboardId, versionId}` (the version with its snapshot; `DASHBOARD_NOT_FOUND` for a missing dashboard, the new `DASHBOARD_VERSION_NOT_FOUND`, 404, for a version that dashboard doesn't have, another project's dashboard's included) and `workflowGet {workflowId}` (the stored JSON byte for byte; `WORKFLOW_NOT_FOUND` for a missing one or one that doesn't read). Neither takes a project id, so "another project's id" applies to the version only; another user's id is not found, as every call. `dashboardUpdate`'s new version is metadata too: its snapshot is the dashboard before the edit, which the page just showed, and the history reads a snapshot only when it compares or restores one. `workflowCreate`/`workflowUpdate` still answer the whole workflow.
  - **No body is parsed to list them.** Migration `0003_window_order_and_list_meta.sql` adds `saved_canvases.meta` (the workflow's `{name, createdAt, updatedAt}` as JSON, written with its data in a second statement; a `saved_canvases_meta_stale` trigger clears it if anything else changes `data`) and `dashboard_versions.widget_count`, and fills both for the rows already there. A NULL means "not known" (a row an older release wrote): the list computes it from that row's body with the same SQL (`META_OF_DATA`, `WIDGET_COUNT_OF_SNAPSHOT`), so only those rows are read. Sizes come from `octet_length` (the record header).
  - **GUI.** `savedWorkflowsByProject` holds `SavedWorkflowSummary` rows. `WorkflowManager.loadWorkflow` is async and reads the body (`getWorkflow`); only the latest open lands (a counter, bumped by `clearWorkflow` too); a failure is said (`workflow_open_failed`) and a `WORKFLOW_NOT_FOUND` also refetches the list. `renameWorkflow` first read the stored body and wrote it back with the new name; the review moved it into Core (`workflowRename`, below). `saveWorkflow` and `findSaved` (bug 22) need only the id and name, which the summary has; `LibrarySync`'s `workflow` refetch applies summaries, and a workflow deleted elsewhere still unlinks the canvas. No view-state tab names a saved workflow (workflow tabs hold only `{id, name, connectionId}`; the canvas is global), so nothing restores a body. A workflow whose body doesn't decode is now listed and says so when opened (it was dropped from the list before). The history keeps `DashboardVersion` without `snapshot` (with `widgetCount`); `DashboardManager.loadVersion` reads one (`dashboard_version_open_failed`, `dashboard_version_unreadable`), and a version pruned or a dashboard removed elsewhere refetches the project's list. The dashboard view fetches the one or two versions picked and drops a pick that a later one overtook. The demo (`TsLibrary`, `TsState`) and `RecordingLibrary` answer the same shapes.
  - **Fixtures.** The Rust replay's `workflowsList`/`dashboardVersionsList` checks compare ids and pass unchanged; its snapshot drops the new columns after checking each is its row's. The TypeScript replay reads each listed workflow's body through `workflowGet` to count its nodes, and restores through `loadVersion`. `changes.json` gets a `view` for `workflows/does-not-decode` steps 0–2 and `old-data/saved-canvases-bad-rows` steps 0–1 (`workflow-bad` now listed), with a Corrections entry in the fixtures README.
- **"Most recent" is the last write committed.** `windows` broke equal-millisecond ties by the later rowid and `window_state` by the earlier one, and an upsert keeps its rowid, so `windowGet`, a new window's copy and the legacy mirror could disagree. `0003` adds `write_seq` to both tables (one past the highest in `windows`, or in the project's `window_state` rows, taken inside the write; a stale save takes none) with indexes `idx_windows_write_seq` and `idx_window_state_project_seq`, and numbers the rows already there in the order the old queries read them. "Most recent" and the count prunes go by `write_seq DESC, rowid DESC`; the 30-day prune still goes by `updated_at`. This changes Decision 22's "`updated_at` only": a write committed later is more recent whatever the clock says, which is what the mirror shows.
- **Lone surrogates in view state.** `WindowStateManager` sends `wellFormedJson(buildState(…))` (new in `core/client.ts`, beside `wellFormed`: every string and key made well-formed) on the queued save and the `pagehide` keepalive; the page keeps showing what it has.
- **`rev` at most 2^53 - 1** (`MAX_REV`, `check_rev`, before anything is read): past it `INVALID_ARGUMENT`.
- **Smaller checks.** A workflow body's `name` must be text (missing or `null`: "needs a name"; another type: "is text"), in Core and the demo. A theme or workflow name holding a lone surrogate says so instead of "needs a name". `onboardingPatch`'s `userBackground` must be `none`, `datagrip` or `dbeaver` (`ONBOARDING_BACKGROUNDS`, the GUI's `UserBackground`). `projectSidebarSet` still accepts connection ids that don't exist, on purpose: another window may have just created the connection, and the sidebar sorts only the connections it has by the order (`projectConnections`), so an id naming none is ignored.
- **Review fixes (as built).**
  - *One row could fail a whole `workflowsList`* (Important): SQLite's `->>` returns an escaped lone surrogate as CESU-8 bytes and `json_object` copies a stored non-UTF-8 byte through, so decoding `meta` as a string failed the list with `STORAGE_ERROR`. It is now selected `AS BLOB` and read lossily (U+FFFD); the version list reads its ids and times the same way.
  - *Rename is Core's*: `workflowRename {workflowId, name}` (new; `workflowUpdate` carries a whole body, so a name-only patch didn't fit its shape) changes only the stored JSON's `name` and `updatedAt`, on the row read inside the `WriteTx`, everything else byte for byte (`rename_workflow_json`), and answers `PersistedWorkflowMeta`. It checks the name like other library names (not empty, no NUL, `max_name_bytes` on web) and the workflow size (growth only), is `WORKFLOW_NOT_FOUND` for a missing or unreadable workflow (another user's included), emits one `workflow` event, and its params refuse unknown fields; the request's `Debug` shows the method only. The demo (`TsState.renameWorkflow`) does the same. The GUI rename no longer reads or writes the body, so a save another window makes between two renames stays.
  - *Open racing a delete*: `deleteWorkflow` invalidates an open of the same workflow still reading its body (`openingId`), so a deleted workflow never becomes the open one.
  - *NULL list metadata is refilled*: an older release's replace-all workflow save writes rows without `meta`. `0003` (edited before release; it had only ever run on scratch copies) now marks unreadable bodies (`meta` `'null'`, `widget_count` `-1`) instead of leaving NULL, and adds partial indexes on the NULLs; `refill_list_meta` (after `refill_name_keys`, in Core's writable open and a capped web open; not a recorded step) checks them with two index lookups and fills them.
  - *Long migrations*: `0003`'s fill holds the migrator's lock about 3 s per GB (~6.5 s on a 2 GiB web file), past the 5 s busy timeout. `Storage::open` now retries `BEGIN IMMEDIATE` on `SQLITE_BUSY` up to 12 busy timeouts (`MIGRATION_WAIT_ATTEMPTS`), so a second opener waits it out (migrations README). Re-review: only when the baseline or a migration has work to do; an up-to-date file still fails after one busy timeout, so a lock another process holds doesn't stall startup for a minute (and three with the gate's retries). After a downgrade, `refill_list_meta` holds the write lock while it parses the rewritten bodies (about 1.5 s for 480 MiB), at open, behind the gate.
  - *Duplicate keys*: a rename of a hand-edited row with two `name` keys already leaves one, set to the new name: the stored object is read with duplicates collapsed to the last value (as `JSON.parse` keeps it). Pinned by a test; no code change.
  - The dead `savedCanvases` field of `LegacyPersistedProjectState` went; Decision 22's "most recently used" lines point at `write_seq`.
- **Sizes, measured on a live web build** (the probe's own files, copied): on user B's file (2,001 versions holding 293 MiB of snapshots, 31 workflows holding 479 MiB) `dashboardVersionsList` went from 293.5 MiB (~930 ms, ~300 MiB more server RSS per call) to 385 KiB (48 ms) and `workflowsList` from ~480 MiB to 6.1 KiB (75 ms), with no RSS growth across calls; `workflowGet` of the 16 MiB workflow took 30 ms. At the web caps (user C: 1,000 workflows, 1,000 dashboards, 99,503 versions) `workflowsList` is 187 KiB (~10 ms) and `dashboardVersionsList` 18.4 MiB (~200 ms). The lists now scale with the row count, not the bodies; what still bounds `dashboardVersionsList` is the number of versions (`dashboard_version_limit` up to 100,000 per dashboard). A per-dashboard list would bound it further; not done here.

### Task 8: Docs, measurement, checkpoint

- **CLAUDE.md:** the `settings` and `ui` groups, window identity (the origin is the window id on web), view state per window with its fallback and legacy mirror, `rev`, `StateLimits`, the new kinds, what stays in the storage group, `PersistenceManager` gone, the `LoadKey`s left, the chat and workflow budgets, chart nodes stored without rows, and the overrides correction (the line saying a shared connection's override credentials still go through the `secret` group is removed: overrides are retired).
- **Design doc:** the status line; "Phase 5d-2 cost" and "Phase 5d cost" for both slices.
- **This plan:** execution notes, release notes (tabs per window, with the active connection per window; the first window keeps today's tabs; web: a chat past 64 MiB asks for a new chat, a workflow past 16 MiB isn't saved; changes to dashboards, workflows, chats and settings appear live; dashboard names clash; the dashboard 0 rule; the CLI refusing the file again until the app opened it; the web limits), checkpoint, manual checks.
- **Migrations README:** `0002` and `backfill_dashboard_name_keys`.
- **Effort log:** 5d-2 rows and totals. **The full check list**, with one full live run.

**Status (Task 8):** done. CLAUDE.md, the migrations README, the fixtures README (one correction: the settings steps' views), the design doc's status line, "As built in phase 5d-2", "Phase 5d-2 cost" and "Phase 5d cost", the execution notes, release notes, checkpoint and manual checks below, the follow-ups and the effort log's totals are written. The 5d-2 probe-fix notes, which had been written under 5d-1's Task 7, moved under Task 7 above. The full check list ran with one live run; four MSSQL TLS tests failed on this machine's certificate store (see "Checkpoint (5d-2)"). The manual checks are the owner's.

### Manual checks (5d-2)

For the owner, after Task 8. Covers Tasks 1–6b and the probe fixes.

**Setup.**
- Databases and credentials as in "Manual checks (5d-1)".
- Desktop data dir: `D="$HOME/Library/Application Support/app.seaquel.desktop.dev"`. **Before the first launch of this build**, back it up: `cp -R "$D" /tmp/sq-5d1`. Read the file with `sqlite3 "$D/seaquel.db" "…"` (fine while the app runs).
- The sidecar: `npm run cli:build`.
- Web: `npm run build:web:full`, then `SEAQUEL_WORKSPACE_CAP=2 npm run start:web`, at `http://localhost:8787`; user A's file is `users/<A id>/meta.db` (`sqlite3 auth.db "select id, email from user"`).

**CLI and MCP** (first: they need a file this build hasn't opened yet)

- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5d1 src-tauri/binaries/seaquel-cli-aarch64-apple-darwin mcp </dev/null` fails with "… Open the Seaquel app once …" (`STORAGE_NEEDS_UPGRADE`).
- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5d1 npm run tauri dev`, wait for the app to load, quit. `sqlite3 /tmp/sq-5d1/seaquel.db "select version from _sqlx_migrations"` prints 1, 2 and 3, and the same CLI command now exits without an error.
- [ ] With the normal data dir, turn "Share data with AI" off globally in Settings → AI, then ask Claude (through the `claude mcp add` line from Settings → MCP) for a row count with `run_query` on an exposed Postgres connection: it fails with `DATA_SHARING_OFF`. Turn it on: rows come back, without restarting the server.

**Desktop** (`npm run tauri dev`, on `$D`)

- [ ] **The first launch keeps your tabs.** The tabs you had in 5d-1 are open. `sqlite3 "$D/seaquel.db" "select window_id, active_project_id from windows"` shows one row, `main`, with the active project; `select project_id, rev from window_state` has a row for each project you've opened since.
- [ ] **Tabs survive a restart.** Open a few query tabs with text, split the pane, pick another tab, switch to a second project and back, quit and relaunch: the tabs, text, layout and active tab are back in both projects, and each project shows the view (editor, workflow, dashboard) it was on.
- [ ] **The theme editor window.** Settings → Appearance → create a theme: the editor opens in its own window. Change a colour and save: the main window applies it (when it's the active theme) and it survives a restart. `select window_id from windows` still shows only `main`; the editor and the log viewer never store view state.
- [ ] **Workflows.** Build a workflow with a query node and a chart on it, run it and save it: `select json_extract(value, '$.rows') from saved_canvases, json_each(json_extract(data, '$.nodes')) where saved_canvases.id = '<id>' and json_extract(value, '$.type') = 'chart'` prints `[]`, and after a restart the workflow opens with its chart drawn. A workflow saved before this build (in `/tmp/sq-5d1`, or one whose chart row isn't `[]`) opens with its chart. Rename it from the list: `select json_extract(data, '$.name'), json_extract(meta, '$.name') from saved_canvases where id = '<id>'` shows the new name twice.
- [ ] **Dashboards.** Click "New Dashboard" twice: the second is "New Dashboard (2)". Renaming it to `new dashboard` is refused with a message naming the other one. A star survives a restart. Moving or resizing a widget adds no row to `select version from dashboard_versions where dashboard_id = '<id>'`; a rename, adding a widget and changing the date filter each add one.
- [ ] **Version history, fetched on demand.** Open a dashboard's version history: the list shows each version's time and widget count at once. Pick two versions to compare, then restore the older one: the dashboard shows it, and one more version appears (the state before the restore). With the history open, delete one version by hand (`sqlite3 "$D/seaquel.db" "delete from dashboard_versions where id = '<id>'"`), then pick it: an error says the version couldn't be opened, and the list reloads without it.
- [ ] **AI chats.** Ask something, then ask again and press Stop half-way: after a restart both turns are there, the stopped one cut where it stopped (`select role, length(content) from ai_messages where chat_id = '<id>' order by timestamp, rowid`).
- [ ] **AI providers and their keys.** Add a provider with an API key. Its id is in `select value from app_state where key = 'aiSettings'`, and `security find-generic-password -s app.seaquel.desktop -a ai-api-key:<id> -w` prints the key. Change the key in the form: the command prints the new one. Remove the provider: the command says the item could not be found.
- [ ] **Settings survive a restart:** key bindings, pending changes, both version limits (a value under 10 is saved as 10), the light and dark theme.
- [ ] **A shared dashboard named like a local one.** In a shared (git) project, create a local dashboard "Sales". In the repo, copy one of `.seaquel/projects/<project>/dashboards/*.json` to `sales-shared.json` and set its `"name"` to `"Sales"`, then switch to another project and back: a warning says the shared dashboard "Sales" isn't shown because a local one has that name, and it doesn't appear again until the next launch. Rename the local one to "Sales local" and switch projects again: the shared "Sales" appears.

**Web, two tabs of user A**

- [ ] **Window ids.** In each tab's devtools, `sessionStorage.getItem("seaquel.windowId")` is a `win-<uuid>`, different per tab, and the same after a reload. `sqlite3 users/<A id>/meta.db "select window_id from windows"` lists both.
- [ ] **Tabs per tab.** Tab 2's open tabs don't change when tab 1 opens or closes tabs, and survive tab 2's reload. A new browser tab starts with the tabs of the one used last. Duplicate tab 1 (the browser's "Duplicate"): the copy's `seaquel.windowId` differs from tab 1's, and tabs opened in the copy don't appear in tab 1 after tab 1 reloads.
- [ ] **Closing right after typing.** Type into a query tab in tab 1 and close the browser tab within half a second. Open a new tab: the text is there.
- [ ] **Active connection and order.** Each tab keeps its own active connection across reloads; reordering connections in tab 1 shows in tab 2.
- [ ] **Settings and themes live.** An AI provider added in tab 1 and another in tab 2 are both kept in both (`select count(*) from user_credentials` goes up for each key). A theme added in tab 1 appears in tab 2, and one picked in tab 1 applies in tab 2.
- [ ] **Dashboards live.** A dashboard renamed in tab 1 is renamed in tab 2, its tab title too; one deleted in tab 1 closes in tab 2.
- [ ] **A chat deleted while streaming.** Start a long answer in tab 1 and delete that chat in tab 2: tab 1 stops and switches to another chat.
- [ ] **A workflow rename during another tab's save.** Open the same saved workflow in both tabs. In tab 1 add a node and save; within a second, rename it in tab 2. After reloading both, the workflow has tab 2's name and tab 1's node (`select json_extract(data, '$.name'), json_array_length(json_extract(data, '$.nodes')) from saved_canvases where id = '<id>'`).
- [ ] **Version history on web:** compare and restore, as on desktop.
- [ ] **A full chat.** Stop the server and fill a chat to 64 MiB:
  ```sh
  sqlite3 users/<A id>/meta.db "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 64)
    INSERT INTO ai_messages (id, chat_id, role, content, timestamp)
    SELECT 'fill-' || i, '<chat id>', 'assistant', printf('%.*c', 1048576, 'x'), '2030-01-01T00:00:00.000Z' FROM n"
  ```
  Start it and open the chat: "This chat is full. Start a new chat to continue." shows with a New chat button, and the input is disabled after a reload too. New chat works.
- [ ] **B sees none of A's** dashboards, workflows, chats, themes or providers, and B's tabs don't change while A writes.

**Older release**

- [ ] Quit the dev app. Back up the release data dir (`R="$HOME/Library/Application Support/app.seaquel.desktop"`, `cp -R "$R" /tmp/sq-release`), copy `$D/seaquel.db` over `$R/seaquel.db` (remove `$R/seaquel.db-wal` and `-shm` first), and open the installed 2026.9.x app: it shows the tabs of the window saved last, and every workflow and dashboard. Quit it and put `/tmp/sq-release` back.

**Demo** (`npm run build:demo`, then `npm run preview:demo`)

- [ ] Tabs, a workflow with a chart, a dashboard (with a version) and a chat survive a reload.

### Owner answers (5d-2)

The six questions the re-survey raised, and the three taken as recommended, are answered: Q13–Q19 in "Answered questions". Nothing is open.

### Execution notes (5d-2)

Executed like 5d-1: task by task with subagents, Tasks 1–3 overlapping, Task 6 split into 6a and 6b, a review after each task (Tasks 4 and 6b had several rounds), a probe on a four-user web instance, one round of probe fixes with two reviews, and this checkpoint. Where the result departs from the text above, the repo is authoritative. Per-task times and surprises are in `2026-10-04-phase-5d-effort.md`; the measured cost is in the design doc ("Phase 5d-2 cost").

**What went differently from the plan**

- **Overrides retired, not moved** (Q13). The re-survey found `SharedConnectionManager` was never constructed, so the override calls the plan had for Core went; the table stays, unused.
- **35 storage methods retired, not 32** (Task 5). The storage group keeps 15: query history, shared repos, the license, the vault's state and credentials.
- **Task 6 in two parts.** 6a moved the view state and deleted `PersistenceManager`, leaving dashboards and chats in a stopgap `DashboardChatPersistence` on the retired storage stubs, which 6b replaced and deleted. Between 5 and 6b neither desktop nor web could load or save tabs, dashboards, chats or settings; nothing was released in between.
- **The write transaction** (Task 4 reviews). The per-user file cap (`SEAQUEL_USER_DB_MAX_BYTES`, `STORAGE_FULL` 507) needed `WriteTx` to send `BEGIN`/`COMMIT`/`ROLLBACK` itself, since SQLite rolls back on `SQLITE_FULL` and sqlx's transaction then left the pooled connection unusable. The rewrite first sent `BEGIN IMMEDIATE` before its guard existed, so a write cancelled while it waited left a transaction open on the connection; the re-review caught it.
- **Over-limit rows stay saveable.** A view state, workflow or dashboard stored before the web limits (or copied from one) can be saved as long as it doesn't grow; the GUI says a limit refusal once, naming the limit.
- **Refused edits show the stored row again** (6b reviews). The planned "changed in another window" banner for dashboards went: a refused edit reverts once this page's writes have answered, so a dashboard never holds an unsaved change for a remote one to clash with. Settings stores do the same (`WriteOrder`).
- **Chat fullness comes from Core** (`ChatMessages.full`, `chat_is_full`), so a chat opens full whatever the page last saw.
- **Dashboard drafts gained `renameIfTaken` and `shared`** (6b review), for "New Dashboard" and the git reconcile, whose pairing took three rounds: by the file's `name` field, then by the path the row would be written to; a file whose name a local dashboard holds is skipped and said once per session. **For the owner to confirm.**
- **Probe fixes** (Task 7, notes under it): the version and workflow lists answer metadata only, with `dashboardVersionGet`, `workflowGet` and `workflowRename` for one body; migration `0003` adds the list metadata and `write_seq`, which replaced `updated_at` as "most recent"; `refill_list_meta` covers rows an older release writes; the migration lock waits up to a minute only while schema work is pending; `rev` is capped at 2^53 − 1 and view state is sent well-formed.
- **The probe-fix notes were written under 5d-1's Task 7**; Task 8 moved them to 5d-2's.
- **Live tests start fast now.** A macOS Developer Tools setting removed the 30–60 s start-up per test binary that made 5d-1's live run take over 2 hours (5d-1 follow-up "Live test start-up").

**Decisions made during execution**

- **Lists without bodies** (owner, Task 7 probe): metadata lists plus one-body reads, rather than paging whole bodies.
- **The git reconcile's pairing and skip rule** (coordinator, 6b review): awaiting the owner's confirmation.
- **A per-user size cap on web** (Task 4 review): 2 GiB by default, a backstop behind the per-call limits.

### Release notes (5d-2)

For the release after 5d-1's. Earlier notes still apply as written.

Changes you may notice:

- **Each window and browser tab keeps its own tabs.** Open tabs, their text, the layout and the active connection are saved per window (desktop) or per browser tab (web), per project. A reload keeps a tab's own; a new browser tab starts with the tabs of the one you used last, and a duplicated tab goes its own way from then on. The first launch after upgrading shows the tabs you had. The connection order in the sidebar is still shared.
- **Dashboards, workflows, chats and settings update live** in your other windows and tabs, themes included: a theme picked in one tab applies in the others.
- **Dashboard names are unique within a project**, compared without regard to case or spaces at the ends. "New Dashboard" takes the next free "New Dashboard (2)". Dashboards that already share a name stay as they are.
- **A dashboard version limit of 0 keeps every version** (it used to delete them all). Two windows no longer overwrite each other's version numbers, and a dashboard deleted while a save was in flight stays deleted.
- **Version history and workflows open faster.** Lists no longer carry every snapshot or workflow body; a version is read when you compare or restore it, and a workflow when you open it. A workflow that can't be read is now listed and says so when opened, instead of disappearing.
- **Workflow charts are saved without a second copy of their data**; they redraw from their source node. Workflows saved before this release keep their copies until you save them again.
- **AI chats and providers are saved per change.** Two windows no longer delete each other's messages or providers, a stopped answer is saved where it stopped, and deleting a chat that is answering stops it.
- **A shared (git) dashboard named like one of your local dashboards isn't shown.** You get a message naming both; rename the local dashboard and the shared one appears.
- **The command line tool (`seaquel-cli mcp`) again needs the app to open your data once** after upgrading. Until then it stops with "Open the Seaquel app once…".
- **Older releases still open your data** and show the tabs of the window saved last.
- **The first open after upgrading can take a few seconds on a large file**: it indexes saved workflows and dashboard versions once, about 3 s per GB of them.

Self-hosted web:

- **A chat past 64 MiB of messages is full**: the next message is refused and the chat asks you to start a new one. **A workflow past 16 MiB** (its results included) isn't saved; the message says to clear or narrow its results.
- **New limits** (`400 INVALID_ARGUMENT`, naming the limit): 8 MiB per tab's saved view, 2 MiB per tab's text, 500 tabs; 1,000 workflows; 4 MiB per dashboard, 1,000 dashboards, 16 MiB of versions per dashboard; 1 MiB per chat message, 5,000 messages per chat, 10,000 chats; 256 KiB per setting, 200 themes, 50 AI providers. A tab, workflow or dashboard already past a limit can still be saved while it doesn't grow. At most 50 browser tabs per user keep saved tabs; tabs unused for 30 days are forgotten.
- **Each user's `meta.db` is capped** at `SEAQUEL_USER_DB_MAX_BYTES` (2 GiB by default, 64 MiB to 1 TiB). A write past it fails with `507 STORAGE_FULL` and stores nothing.
- **New error codes on `/api/rpc`:** `DASHBOARD_NOT_FOUND`, `DASHBOARD_VERSION_NOT_FOUND`, `WORKFLOW_NOT_FOUND`, `CHAT_NOT_FOUND`, `THEME_NOT_FOUND`, `AI_PROVIDER_NOT_FOUND` (404) and `STORAGE_FULL` (507); `NAME_TAKEN` covers dashboards. The `settings` and `ui` groups are new, and 35 storage-group methods are gone (only a custom client could notice).
- **The `X-Seaquel-Origin` header and `?origin=` parameter** now carry the browser tab's id (`win-<uuid>`), which stays the same across that tab's reloads. It still names no user.
- **The first open of each user's file after upgrading** runs the new migrations inside its lock: about 3 s per GB of saved workflows and dashboard versions, so about 6.5 s for a file at the 2 GiB cap. A second request for that user in that window waits instead of failing.

Known issues:

- Sharing a local dashboard named like an existing git file overwrites that file.
- Renaming a shared dashboard doesn't rename its file, so the next reconcile brings the old name back as a new shared dashboard and unshares the renamed one (a change of case only keeps the pair). Edits to a shared dashboard never reach its file (older).
- Two dashboards whose names map to one file name (non-Latin names all become `untitled.json`; "Sales" and "Sales!") write the same file; the last write wins.
- A save answer landing while you drag a widget can snap it back.
- On web, a workflow over 16 MiB only because of chart copies saved by an older release can't be renamed to a longer name; saving it once drops the copies.
- If a browser tab's last save before a reload arrives after the reloaded page has already changed something, the reloaded page's state wins and that last save is lost (accepted in Task 6a, M5).

### Checkpoint (5d-2)

The full check list, run on 2026-09-30 one step at a time on the shared `scratchpad/p5a/target`, npm through `mise exec`:

| Check | Result |
|---|---|
| `npm run crates:check` | pass: 24 crates |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio`, live (the `ci.yml` env, `SEAQUEL_TEST_REQUIRE_ENGINES=1`, `SEAQUEL_TEST_SSH`, the compose databases seeded with `npm run e2e:db:seed -- postgresql mysql mariadb sqlserver duckdb`) | 1,791 passed, 3 ignored, **4 failed**: the MSSQL TLS tests below. About 70 minutes in all: the first attempt (16:45–17:00, ~13 minutes of it compiling) stopped at the first failing target, and the rerun with `--no-fail-fast` took 56 minutes |
| `cargo test -p seaquel-core --features storage,workspace --test state` | pass: 38 |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` | pass |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run types:gen` twice | pass: the 207 generated files are identical after the second run |
| `npm run check` | pass: 0 errors, 0 warnings (4,707 files) |
| `npx oxlint --type-aware --type-check --deny-warnings` | pass: 0 diagnostics in 644 files |
| `CI=1 npx vitest run` | pass: 1,991 tests in 114 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |
| `npm run cli:build`, `cargo check -p seaquel` | pass |
| `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 44 |

**The four failures** are `row_6_mssql_over_ssh_checks_the_certificate_as_the_server` (`seaquel-core`'s `connect`) and the three `tls_server_name` tests of `seaquel-engine-mssql` (`a_bracketed_ipv6_host_is_dialled`, `without_it_the_tls_name_is_host`, `the_tls_name_is_tls_server_name_while_the_socket_goes_to_host`). Each panics inside tiberius with `could not load platform certs: … code: -36` before it reaches the server: macOS's certificate store, the environment issue 5d-1's checkpoint describes. This time they failed outside the Bash sandbox too, so they weren't seen passing on this machine; CI's Linux engines job runs them.

The TypeScript replay of the state fixtures (`state-replay.svelte.test.ts`) passes with the eighteen settings views filled in, and fails when one of them is changed (checked by hand on two).

**Manual checks:** pending (the owner).

**Not run:** the release workflow and a signed build.

---

## Follow-ups (not in 5d)

Consolidated at 5d-1's checkpoint and updated at 5d-2's. Items marked **5d-2** were that slice's and are closed; the rest are later.

From 5d-1:
- **5d-2: override credentials.** Re-survey: the code that writes them is never run (`SharedConnectionManager` is never constructed). Retired (Q13, Decision 25); closed.
- **5d-2: dashboard names.** Done: `name_key` in `0002`, `backfill_dashboard_name_keys`, `refill_name_keys` (Decision 21, Task 3).
- **5d-2: survey the stored data.** Done from the code ("Stored data to expect" in the 5d-2 section); Task 2 seeds each case.
- **5d-2: the second migration** makes `seaquel-cli mcp` refuse the file again until the app has opened it. Done: the 5d-2 release notes say so (`0002` and `0003`).
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
- **Live test start-up.** Each live test binary took 30–60 s to start on the owner's machine, so a full live run took about 2 hours. Closed before 5d-2's checkpoint: a macOS Developer Tools setting (the terminal allowed to run software that doesn't meet the system's security policy) removed the wait; see "Checkpoint (5d-2)" for the run's time.

From 5d-2's execution (later):
- **Shared dashboards and their files.** Sharing a local dashboard named like an existing git file overwrites the file; renaming a shared dashboard leaves its old file, which the next reconcile brings back as a new shared dashboard; names that slug to one file write the same file. The reconcile's pairing and skip rule awaits the owner's confirmation (Task 6b).
- **A widget can snap back** when a save answer lands mid-drag; a drag in progress should count as a pending edit.
- **`dashboardVersionsList` is bounded by the version count**, not the bodies: 18.4 MiB at the web caps (99,503 versions). A per-dashboard list would bound it further.
- **A late `pagehide` save loses to a changed reload** (Task 6a, M5): accepted.
- **Twelve `changes.json` steps still have `view: null`** (eight in dashboard cases, three in view-state cases, one in `chats/two-tabs-same-chat`), so the replays don't compare what the page shows there. The eighteen settings steps were filled in at the checkpoint; these weren't reviewed then.

From the 5d-2 re-survey (later, not in 5d-2):
- **Shared dashboards' edits never reach the git file**, and the reconcile takes the stale file without a newer-than check; a rename leaves the old file (re-survey bug 25). Q7 keeps git in TypeScript.
- **Two tabs setting up the web vault at once** replace each other's salt and verifier (bug 24).
- **The license record and the license nudge stay whole values**, last writer wins; the nudge's counts can lose a query run when two tabs run at once.
- **Dead code:** `DashboardManager.loadDashboards`, `license.revertToPersonal`, `UseDatabase.destroy()` (and the override manager, per Q1).

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
