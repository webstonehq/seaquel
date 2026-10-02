# Phase 5e Implementation Plan: shared repos and imports in Core

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** implemented (Tasks 1–9; checkpoint 2026-10-01, see "Checkpoint"); the owner's manual checks passed. Owner answered Q20–Q32. Surveyed on 2026-09-30 at HEAD `5d876af` (5d-2, committed); the only uncommitted changes then were the owner's edits to the 5d plan and the design doc. Line numbers are as of `5d876af`.

**Goal:** The `.seaquel` projection of a shared project (which files map to which rows, and the reconcile in both directions) runs in Core, and so do the TablePlus and DBeaver import readers. Every edit of a shared row reaches its file, every file change reaches its row, each side's changes are told apart from the other's, and nothing is written into a repo the row doesn't belong to. The webview no longer reads or writes the repo's files, and the CLI can call both through Core. The TypeScript keeps the dialogs, the sync button and the view of git status.

**The last slice of phase 5.** After it, nothing the GUIs store or project is decided in TypeScript on desktop and web; the demo keeps its twins until phase 8 (5e adds none: the demo has no shared projects and no imports).

**Architecture:**
- **`seaquel-workspace::shared`** (pure): the five file formats (query, dashboard, connection template, project, labels), file names, pairing, and the sync plan: given a scan of the project's directory, its rows and the content both sides had at the last sync, which rows and which files change. **`seaquel-workspace::imports`** (pure): TablePlus and DBeaver entries to import candidates. Parity fixtures recorded from today's TypeScript pin both.
- **`seaquel-git`** gains a `tree` module: scanning a project's `.seaquel` directory and applying file writes, bounded, never through a symlink. It already owns the working tree and is desktop-only behind Core's `git` feature.
- **`seaquel-storage`**: migration `0004_shared_links.sql` (where each shared row's file is, and the content it last synced), and targeted `shared_repos` writes on `WriteTx`. The replace-all repo save stays only for its frozen fixture.
- **Core**: `shared.rs` (link, unlink, scan, sync, the repo lock, and publishing a shared row's file from inside the library calls that change it) and `imports.rs` (finding and reading the other tools' files, candidates, import). Both need a new `LocalFiles` policy, which only the desktop and the CLI grant.
- **`seaquel-rpc`**: new `shared` and `imports` groups, desktop only; `dispatch_workspace` refuses them unless the Core was built with `LocalFiles::Allowed`. The storage group loses `sharedReposLoadAll`/`sharedReposSaveAll`.
- **TypeScript**: a `SharedService` seam (`CoreShared` on desktop, `NoShared` elsewhere). `shared-query-manager.svelte.ts`, `shared-dashboard-manager.svelte.ts`, the three file parsers, `yaml-utils.ts` and the two import mappers go; `SharedRepoManager` shrinks to the repo list, git calls and sync state. The main window loses the `fs` permissions only the projection used.

**Tech Stack:** Rust (`seaquel-workspace`, `seaquel-storage`, `seaquel-git`, `seaquel-core`, `seaquel-rpc`, `src-tauri`), TypeScript/Svelte 5, vitest with an in-memory file system for the recorder.

**Inputs:**
- The design doc: "Core, workspaces and state" (`SharedRepoService`), "Interfaces after the move" (`read_dbeaver_config` and `read_tableplus_config` leave `src-tauri`; `seaquel conn import dbeaver|tableplus` in "CLI (first cut)"), phase 5, and "Phase 5d cost" ("budget review fixes at about two-thirds of first passes; plan a probe with production-sized files").
- The 5d plan: its structure, Q7 and Q8 (git and import parsing stayed in TypeScript so 5d could stay about storage), Decisions 13 and 14, the 5d-2 re-survey's bug 25, the 6b notes on the dashboard reconcile, and "Follow-ups (not in 5d)".
- The 5d effort log, for the estimate.
- A read-only survey of the projection, the git group, the shared-repo storage, the import readers and their callers, below.

**Naming.** "The projection" is the mapping between rows and files. "Sync" is one reconcile of a linked project's directory against its rows, in both directions. "Publish" is writing (or deleting) one row's file after a change to that row. "Base" is the content both sides had at the last sync.

---

## What the code shows

Line numbers are as of `5d876af`.

### The `.seaquel` tree

```text
<repo>/.seaquel/
  labels.yaml | labels.yml          repo-wide labels; read, never written by the app
  projects/<dir>/
    project.yaml | project.yml      name, description; missing: the name is <dir>
    connections/<file>.yaml|.yml    a connection template, no credentials
    queries/**/<file>.sql           YAML frontmatter, then the query
    dashboards/**/<file>.json       {name, description?, widgets, viewport, dateFilter?}
```

1. **Queries** (`services/query-file-parser.ts`). A file is `---\n<frontmatter>\n---\n<query>` (`FRONTMATTER_REGEX`, `:24`). The frontmatter has `name`, `description`, `database`, `tags` (an inline list) and `parameters` (a list of `name`, `type`, `default` or `defaultValue`, `description`) (`:110-197`). Without frontmatter the name is the file name with `-` and `_` as spaces (`:46-52`). The folder is the path below `queries/` (`:54-61`). The writer (`:81-104`) puts a field only when it has a value.
2. **Dashboards** (`services/dashboard-file-parser.ts`). Pretty-printed JSON with the widgets' run state stripped (`:56-69`). A file that doesn't parse is skipped (`:47-50`); one without a name takes the file name (`:29-32`).
3. **Connection templates** (`services/config-file-parser.ts:165-254`): `name`, `type` (default `postgres`), `host` (default `localhost`), `port` (default per engine, `:347-359`), `databaseName`, `sslMode`, `labels` and `sshTunnel {enabled, host, port}`. The reader drops `CREDENTIAL_FIELDS` (`username`, `password`, `connectionString`, `sshPassword`, `sshKeyPassphrase`, `sshUsername`; `types/shared-queries.ts`) at both levels. A template's id is `<repoId>:<path>` (`:241`), and an imported connection keeps it as `sharedConnectionId`.
4. **Projects and labels** (`:36-144`, writers `:259-282`). A shared project's id is `<repoId>:.seaquel/projects/<dir>` (`:137`).
5. **The YAML is line-based**, without a library (`services/yaml-utils.ts`): a value in quotes loses its outer quotes and nothing else (`:11-20`); a value with `:`, `#`, `"`, `[`, `]`, a newline or an edge space is written in double quotes with `"` as `\"` (`:41-55`).
6. **File names** are `nameToFilename(name)` (`config-file-parser.ts:335-343`): lower case, everything but `a-z0-9`, spaces and `-` dropped, runs of spaces and `-` as one `-`, and `untitled` when nothing is left. The project directory is the same function of the **local** project's current name, computed on every use (`shared-query-manager.svelte.ts:27-32`, `shared-dashboard-manager.svelte.ts:23-28`, `shared-repo-manager.svelte.ts:939`, `:995`, `:1043`, `project-manager.svelte.ts:418`). No path is stored for any row.

### What maps to what, and when

7. **A local project links to a repo by its path** (`projects.git_repo_path`). `ProjectManager.setGitRepoPath` (`project-manager.svelte.ts:405-539`) makes `projects/<slug>/{connections,queries,dashboards}`, registers the path as a repo (`shared-repo-manager.svelte.ts:248-285`) or reuses one, reads the remote URL, and on the first link exports every connection that isn't local-only as a template (`:467-483`, `exportProject` `:845-923`). Clearing the path deletes the connections imported from the repo's templates and forgets the repo (`:484-538`).
8. **Shared queries.** A saved query with `shared` set is written to `queries/<folder>/<slug>.sql` when it is shared (`saved-queries.svelte.ts:323-335`), saved from its tab (`:93-99`) or renamed (`:267-292`, the 5d-1 fix that deletes the old path), and deleted with it (`:232-236`) or when unshared (`:340-352`). The reconcile (`shared-query-manager.svelte.ts:141-210`) pairs files with shared rows by `<folder>/<name>` in lower case; a paired file whose text differs replaces the row's text, description, database, tags and parameters (`:162-176`); an unpaired file becomes a new shared query (`:177-195`); a shared row with no file is unshared (`:198-207`). `ProjectManager.storeReconciledQueries` sends the changes through the library (`project-manager.svelte.ts:796-857`); Core keeps the previous text as a version.
9. **Shared dashboards.** The file is written when a dashboard is shared (`dashboard-manager.svelte.ts:664-681`) and deleted when it is unshared or deleted (`:683-700`, `:259-267`). The reconcile (`shared-dashboard-manager.svelte.ts:185-291`, 5d-2 6b) pairs by the file's `name` field, then by the path the row would be written to; a paired file whose content differs replaces the widgets, viewport, description and date filter (`:245-257`, patch `dashboard-manager.svelte.ts:47-62`, without `captureVersion`); an unpaired file is created shared, and a `NAME_TAKEN` is said once per file per session (`:189-236`).
10. **Connection templates.** Editing a connection that isn't local-only rewrites its template (`connection-manager.svelte.ts:827-831` → `shared-repo-manager.svelte.ts:985-1028`), renaming the file when the name's slug changes; removing one deletes it (`:897-903`); the local-only toggle writes or deletes it (`:1125-1139`). Templates are imported as connections (`project-manager.svelte.ts:864-962`) on a deep link, on a shared-project import, and when the project settings save a new git path (`project-settings-tab-view.svelte:150-156`); one already imported (same `sharedConnectionId`) is skipped, and nothing updates an imported one.
11. **When the reconcile runs:** at startup for the active project (`hooks/database.svelte.ts:482-512`), when a linked project is activated (`project-manager.svelte.ts:602-610`) and after a shared-project import (`:748`). Not after a pull, a commit or a conflict resolution: those reload the scan cache only (`shared-repo-manager.svelte.ts:345-351`, `:1147-1157`). Each scan reads every project directory of the repo, one IPC call per directory, file and `stat` (`:483-527`, `:744-832`).
12. **The repo list** is JSON rows in `shared_repos` plus `app_state['activeRepoId']` (`crates/seaquel-storage/src/queries/shared_repos.rs`), loaded at startup and saved whole on a 500 ms timer (`shared-repo-manager.svelte.ts:88-137`) by every change, including `updateRepo` (`:1070-1075`), which every status refresh calls (`:451`). The storage group serves `sharedReposLoadAll`/`sharedReposSaveAll` (`crates/seaquel-rpc/src/workspace.rs:283-288`, `:652-658`); the save emits a `storage` event the GUI ignores. `activeRepoId` is set when a linked project is activated (`project-manager.svelte.ts:607`), when a project is linked (`:436`, `:446`) and from storage at startup (`hooks/database.svelte.ts:489`), and cleared only when a project is unlinked (`:536`) or its repo removed (`shared-repo-manager.svelte.ts:319-321`).
13. **Git** is the `git` RPC group (`crates/seaquel-rpc/src/git.rs`), served by `dispatch_git` on desktop before storage opens (`src-tauri/src/lib.rs:326-328`) and refused on web. `SharedRepoManager` serialises pull, push and commit per repo in the page (`:142-159`). A fast-forward pull checks out with `force()` (`crates/seaquel-git/src/ops.rs:153`).
14. **File access** is `@tauri-apps/plugin-fs` from the page. The main window may read, write, delete, rename and list any path (`src-tauri/capabilities/default.json:19-50`, `"path": "**"`). `readDir`, `exists`, `mkdir` and `rename` have no user outside the projection; `readTextFile`, `writeTextFile`, `writeFile` and `remove` do (themes import, exports, the ERD viewer). There is no `fs:allow-stat`.

### Imports

15. **The readers are Tauri commands** (`src-tauri/src/lib.rs:524-583`): DBeaver's `workspace6/General/.dbeaver/data-sources.json` under the platform's DBeaver data dir, read as text; TablePlus's `Connections.plist` on macOS only, decoded with the `plist` crate (`src-tauri/Cargo.toml:46`) and handed over as JSON. The CLI can't reach either.
16. **The mapping is TypeScript** (`services/tableplus-import.ts`, `services/dbeaver-import.ts`): a driver or provider table (`:10-16`, `:12-21`), default ports, TablePlus's TLS modes (`utils/connection-string.ts:61-63`) and SSH settings (`tableplus-import.ts:113-124`), and the five-field duplicate check (`services/connection-import.ts`). Since 5d-1 the drafts go to `connectionCreate` with `renameIfTaken`, local-only, and are appended to the connection order (`connection-manager.svelte.ts:599-637`).

### Web, the demo, the CLI and MCP

17. **Web and the demo have no shared projects** (the settings tab, the import buttons and the menu items check `isTauri()`; `dispatch_workspace` refuses `git`). `SharedRepoManager` is still built on web and its calls return early because no repo is active.
18. **The CLI and MCP** read storage only. `list_saved_queries` and `run_saved_query` list shared queries as rows, so a file a teammate added reaches the MCP server only after the app synced it. An import or a sync from the CLI would be the second process that writes the file, which needs writable storage there and `data_version` polling in the app (the design doc defers both to phase 7).

### Bugs and gaps the survey found

Data loss first. Task 1 fixes the ones marked **T1** before anything is recorded; the rest are recorded as they are and fixed in Core (listed in `changes.json`). "By reading" means seen in the code, not reproduced; Task 2's recorder reproduces each.

1. **A local edit of a shared dashboard is undone at the next activation.** Edits are never written to the file (only sharing writes it, item 9), and the reconcile then copies the file's widgets, viewport, description and filter over the row without capturing a version, so the edit is gone for good. The re-survey's bug 25 understated this. **T1.**
2. **Pull discards uncommitted shared edits** (by reading). A fast-forward checks out with `force()` (item 13), which overwrites the files the projection wrote and the user hasn't committed; the next reconcile then takes the old file (a query keeps the lost text as a version; a dashboard loses it). **T1** (a safe checkout, Decision 38).
3. **A conflicted pull damages rows.** Nothing checks for conflicts before reconciling: conflict markers become a shared query's text, and a dashboard file that no longer parses is skipped, so its dashboard is unshared. **T1** (skip the reconcile while the repo has conflicted files).
4. **A scan error reads as a deletion.** Directory errors are logged and skipped (`shared-repo-manager.svelte.ts:784-787`, `:829-831`), and the reconcile unshares every shared row whose file it didn't see.
5. **Writes go into the wrong repo.** `activeRepoId` survives switching to a project without a git path and is restored at startup (item 12), and the share, update and unshare paths write under the *active* project's slug in *that* repo. Since `connections.is_local_only` defaults to 0 for rows saved before the local-only default (`crates/seaquel-storage/src/schema.rs:88`, `:404`), every save of such a connection in an unlinked project writes its host, port, database and SSH host into another project's team repo, and removing it deletes a same-named template there. **T1.**
6. **Linking a project duplicates its connections.** The first link exports templates without linking the connections to them, and the settings save then imports every template as a new connection, so each comes back as "<name> (2)" (items 7 and 10).
7. **The import of a project's templates takes every project's.** `importSharedConnections` walks all shared projects of the repo (`project-manager.svelte.ts:872-886`), so linking one project imports the others' templates too.
8. **A project linked under another name looks at the wrong directory.** The directory is the slug of the local name (item 6): a shared project imported as "Main (2)", or one whose `project.yaml` name doesn't slug to its directory, reads `projects/main-2/`, which the link creates empty in the team repo (`:418-422`), and never sees its files.
9. **Renaming a linked project renames its directory for everyone** (`project-manager.svelte.ts:355-398`). Every teammate's link is by their own local name, so at their next activation all their shared rows are unshared. A rename onto an existing directory fails into an empty `catch`.
10. **File names collide.** Every name without a Latin letter or digit is `untitled`, and "Sales" and "Sales!" are both `sales`, so two shared queries or dashboards write one file and the last write wins; the reconcile then unshares the other. A file whose path isn't the slug of its name (a teammate's hand-written `my_query.sql`, named "my query" from its file name) is written back as `my-query.sql` beside it, and the next reconcile pairs both files with the one row (the search doesn't skip paired rows, `shared-query-manager.svelte.ts:158-160`), so the text flips between them.
11. **A renamed shared dashboard leaves its old file**, which the next reconcile brings back as a new shared dashboard while the renamed one is unshared; sharing a local dashboard named like an existing file overwrites it (5d-2 6b's known issues).
12. **Pulled changes don't show until a project switch or a restart** (item 11), and only the active project is ever reconciled.
13. **Windows line endings drop the frontmatter.** `FRONTMATTER_REGEX` needs `\n`, so a file checked out with `core.autocrlf` is read as a query whose text starts with the frontmatter and whose name comes from the file name.
14. **Quoted values don't survive a round trip.** A name holding `"` is written as `"a \"b\""` and read back as `a \"b\"`, which no longer pairs with its row: the reconcile makes a second query and unshares the first. A tag holding a comma is split in two; a description holding a newline is cut at it.
15. **Names compare with `toLowerCase`** while Core's `NAME_TAKEN` uses `name_key`: a file named "STRASSE" next to a row "Straße" is created, refused and toasted on every activation (`project-manager.svelte.ts:843-853`); dashboards say it once per session.
16. **Teammates' template changes never arrive.** An imported connection is never updated from its template; a renamed template is imported again; a removed one leaves its connection pointing at nothing.
17. **Symlinks are followed** (by reading). The scan reads any `*.sql` entry that isn't a directory (`shared-repo-manager.svelte.ts:766-769`), so a symlink committed to a repo makes Seaquel read its target into a saved query, which the MCP server then lists to a model; writes follow a symlinked directory out of the repo.
18. **The webview can read and write any file** (item 14). Only the projection needs `readDir`, `exists`, `mkdir` and `rename`.
19. **`stat` is refused** (by reading: no `fs:allow-stat`), so every scanned file's `updatedAt` is unknown and the dashboard reconcile stamps the time of the reconcile (`shared-dashboard-manager.svelte.ts:238-243`).
20. **The repo list is saved whole after every status refresh** (item 12): every five minutes and after every file write, a replace-all save and a `storage` event to every window.
21. **`labels.yaml` and a template's `labels` are read and never used.** The state that holds them has no reader (`state.svelte.ts:677-693`), `exportProject`'s `labels` option is never passed (`project-manager.svelte.ts:475-480`), and an imported connection gets no labels (`:915-942`).
22. **Imports fail silently.** Nothing found, a file that can't be read and a file that doesn't parse all give an empty list (`tableplus-import.ts:56-59`, `dbeaver-import.ts:56-59`), and the dialog only opens when something was found (`stores/tableplus-import.svelte.ts:58-66`), so the button does nothing. A DBeaver port that isn't a number (a `${variable}`) is `NaN` and Core refuses the create without a reason the dialog shows. DBeaver's SSH handlers, URL-only configurations and projects other than `General` aren't read.
23. **The import's duplicate check runs in the page** (`connection-manager.svelte.ts:607-611`), so two windows importing at once both import.
24. **A deep link to a shared dashboard that isn't stored** opens a tab on the scan's `repoId:path` id, which no stored dashboard has (`services/deep-link.ts:224-227`).
25. **Dead code:** `SharedQueryManager.createFolder`; `SharedDashboardManager.createDashboard`, `deleteDashboard`, `getDashboard`, `shareDashboard`, `unshareDashboard` (only the file write and delete are wired, `hooks/database.svelte.ts:320-323`); `SharedRepoManager.repoExistsAtPath` and `updateRepoSettings`; `isValidConfigPath`.

---

## Answered questions (2026-09-30)

The owner answered all seven on 2026-09-30, each with the recommended option. Each keeps the options that were weighed, so later changes start from them. Decisions 29–52 follow the answers.

### Q20. Both sides changed since the last sync

Core records the content both sides had at the last sync (Decision 34), so it can tell a local change from a teammate's. Options were: the file wins and the local version is kept in history (A); the local version wins and rewrites the file (B); both kept, the local one as "<name> (conflict)" (C, ~0.3 h more).

**Answer (owner): A.** The row takes the file's content. Its previous content is kept as a version (a query keyframe, a dashboard version), and the sync's notice names it: "Changed here and in the repo: the repo's version is shown, and yours is in its history." The same rule covers rows with no recorded base yet (the first sync after upgrading) when row and file differ, so a dashboard whose edits never reached its file (bug 1) shows the file's state and keeps the local one as a version.

### Q21. File names

Today every name without `a-z0-9` is `untitled` (bug 10). Options were: letters and digits of every script (A); ASCII transliteration with `deunicode` (B, ~250 KB of tables); ASCII as today with numbered collisions (C).

**Answer (owner): A.** Lower case, NFC; any other run becomes `-`; empty gives `untitled`; a name already used in that directory (compared case-insensitively, as macOS and Windows file systems do) gets `-2`, `-3`. "Отчёт" is `отчёт.sql`, "Sales!" next to "Sales" is `sales-2.sql`. Windows' reserved names and trailing dots are avoided. Existing files keep their paths (Decision 33); only new files and renames use the rule. Older releases read these files (they scan every file and pair by the name inside); an older release editing one writes its own slug beside it, as today.

### Q22. A stable id in each shared file

Without one, a rename made on a teammate's machine arrives as a new shared item while the old row is unshared (today's behaviour). Options were: write an id (A, ~0.4 h); pair by path, then name only (B).

**Answer (owner): A.** `id: <uuid>` in a query's frontmatter and a template's YAML, `"id"` in a dashboard's JSON, written when Core first writes the file and kept after. Pairing goes by id, then path, then name. Older releases ignore the key; an older release that rewrites the file drops it, and pairing falls back to path and name. Core writes the id again the next time it writes that file for a real change, never only to add the id (Task 2 review, M3).

### Q23. Connection templates

Today a template is imported once and never updated (bug 16). Options were: sync them like queries (A); as today (B); A with an Update badge (C, ~0.5 h more UI).

**Answer (owner): A.** Host, port, database, SSL mode and SSH host and port follow the template unless changed here since the last sync (Q20's rule otherwise). The user name, secrets, labels and AI settings stay local. A new template in the linked project's directory is imported at every sync; a removed template leaves its connection, unlinked and local-only, named in the notice.

### Q24. A shared dashboard's viewport

Once edits reach the file (bug 1), every pan or zoom would show as a pending change. Options were: pan and zoom alone don't write (A); every change writes (B); no viewport in files (C).

**Answer (owner): A.** A change to the name, description, widgets (moves and resizes included) or date filter writes the file with the current viewport; a viewport-only change stays local. Decision 34 leaves the viewport out of the content hash, so a teammate's pan or zoom alone doesn't change the row either.

### Q25. Renaming a linked project

Today a rename renames the shared directory for everyone (bug 9). Options were: the local name only (A); `project.yaml` follows and the directory stays (B); as today (C).

**Answer (owner): B.** Core stores the directory (Decision 33), so the link survives any rename; `project.yaml`'s name follows; teammates see the new name in the import dialog, and their own local names stay.

### Q26. The CLI in 5e

Options were: nothing new (A); read-only subcommands (B, ~1 h); writing commands with `data_version` polling (C, ~4–6 h).

**Answer (owner): A.** A Core test drives imports and the shared calls the way `seaquel-cli` builds its workspace (read-only storage, keychain): candidates and scans work, writes answer `STORAGE_READ_ONLY`. Phase 7 adds commands.

### Q27. A template changed both here and in the repo

Added in the Task 2 review (2026-09-30).

**Answer (owner).** The template wins. The connection's host, port, database, SSL mode and SSH host and port take the template's values, and the sync's notice names the connection and lists the values it replaced. It never lists the user name or any secret. This is Q20's rule applied to templates.

### Q28. A template's name or type changed in the repo

Added in the Task 2 review (2026-09-30).

**Answer (owner).** A template's `name:` follows, which renames the connection, unless the connection was renamed here since the last sync; then Q27/Q20's rule applies (the template wins, and the notice names the name it replaced). A changed `type:` is not applied. A notice says the template now names another database type, and the connection stops following that template: it is unlinked and kept, local-only.

### Q29. A retyped template

Added after the Task 2 review (2026-09-30).

**Answer (owner): import it as new.** When a template's `type:` changes, the connection is unlinked and kept local-only (Q28), and the same sync imports the retyped template as a new connection under its name (`renameIfTaken`, e.g. "Warehouse (2)"). The notice mentions the new connection.

### Q30. Which connections a first link shares

Added after the Task 2 re-review (2026-10-01).

**Answer (owner): ask at link time.** When a project is first linked to a repo, the link dialog lists the project's connections with checkboxes. All are ticked except those marked `is_local_only`. Only the ticked ones are exported as templates and become explicitly shared. After that Decision 53 holds: only explicitly shared connections reach the repo.

### Q31. What unlinking does to the project's connections

Added in the Task 7 review (2026-10-01). Task 5's unlink removed every connection linked to the project's templates, so a relink, or an unlink, deleted connections the user had made and shared from here, with their passwords.

**Answer (owner): keep yours, ask about the rest.** Connections that were the project's before it was linked (exported at link time, or shared later from this project) stay: unlinked, made local-only, their keychain passwords kept. Connections the repo brought (made by a sync or an import from a template) are listed in a confirmation dialog and removed only if the user confirms; otherwise they stay like the others. Core records each link's origin ("exported from here" or "imported from the repo") so it can tell them apart, and `unlinkProject` takes the user's choice and answers what was kept and what was removed. A relink (unlink, then link) never deletes the user's own connections, even when the link fails after the unlink.

### Q32. A connection deep link to a project no one links here

Added in the Task 7 review (2026-10-01).

**Answer (owner): import the project, after confirming.** The import dialog opens with that directory ticked; once the project is imported (which imports its templates), the connection opens. Cancelling does nothing.

---

## Decisions (2026-09-30)

Settled with the answers above. Numbered from 29: 5d's end at 28 (Logs).

### Scope and placement

#### 29. Scope

- The projection: projects (link, unlink, import, rename), saved queries, dashboards and connection templates, in both directions, plus the repo list.
- The TablePlus and DBeaver readers and the import itself.
- Picked up from the follow-ups: the re-survey's bug 25, 5d-2's shared-dashboard file issues, "share/update/unshare write into the active project's repo", `nameToFilename`, the replace-all repo list, and the safe pull and conflict guard the projection needs to be correct ("Follow-ups", end).
- Not in scope: git itself (clone, push, credentials, conflict resolution stay as they are, apart from Decision 38), shared labels (Decision 46), the web vault, and new CLI commands (Q26).

#### 30. Where the code goes

- **Pure, in `seaquel-workspace`:** `shared` (formats, names, hashing, pairing, `plan_sync`, `plan_publish`) and `imports` (TablePlus entries as JSON, DBeaver's file as bytes, to candidates; the duplicate check). No I/O, never panics, builds for wasm32.
- **File I/O in `seaquel-git`** (`tree.rs`): `scan(root, dir) -> RawScan` and `apply(root, ops) -> Applied`, on blocking threads like the rest of the crate. It is already desktop-only, behind Core's `git` feature, and the one crate that touches the working tree.
- **Core:** `shared.rs` behind `git` and `storage`; `imports.rs` behind a new `imports` feature (`dirs`, `plist`, `std::fs`). `src-tauri` and `seaquel-cli` enable `imports`; `seaquel-server` doesn't. The `plist` dependency leaves `src-tauri`.
- `npm run crates:check` needs no new class: the new code lives in existing crates.

#### 31. `LocalFiles`, a policy with no default

`CoreBuilder::local_files(LocalFiles::Allowed)` lets Core read and write the user's files: repos and other tools' config files. A Core built without it answers `NOT_SUPPORTED` to every `shared` and `imports` call and publishes nothing, whatever features Cargo unified (as `ConnectPolicy` does). The desktop and the CLI pass `Allowed`; the web server and the tests that don't need it pass nothing. `dispatch_workspace` checks it again before dispatching either group.

### Files

#### 32. File access rules

- Everything stays under `<repo>/.seaquel/`. A path from a row or a request is checked before use: relative, `/`-separated, no `..`, `.`, empty or hidden component, no backslash, NUL or control character, no Windows reserved name or trailing dot or space, at most 1,024 bytes. A folder name that fails is refused with `INVALID_ARGUMENT`.
- **No symlinks.** Every component is checked with `symlink_metadata`, and files are opened with `O_NOFOLLOW` on Unix. A symlink anywhere on the path is skipped on read (and named in the notice) and refused on write.
- **Bounds.** At most 20,000 files per project directory, 16 MiB per file and 256 MiB per scan; past one, the rest is skipped and the notice says so. Only `.sql`, `.json`, `.yaml` and `.yml` files are read; a file that isn't UTF-8 is skipped and named.
- **Writes are atomic:** a temp file in the same directory, then a rename. Deletions keep the bytes until the row write commits (Decision 37).
- **Names compare case-insensitively** when Core picks a new path, since the default file systems on macOS and Windows do (5d-1's rename fix).

#### 33. Links are stored: migration `0004_shared_links.sql`

```sql
ALTER TABLE projects      ADD COLUMN shared_dir TEXT;      -- the directory under .seaquel/projects/
ALTER TABLE saved_queries ADD COLUMN shared_path TEXT;     -- repo-relative path of its file
ALTER TABLE saved_queries ADD COLUMN shared_base TEXT;     -- hash of the content at the last sync
ALTER TABLE saved_queries ADD COLUMN shared_file_id TEXT;  -- the file's id (Q22)
ALTER TABLE dashboards    ADD COLUMN shared_path TEXT;
ALTER TABLE dashboards    ADD COLUMN shared_base TEXT;
ALTER TABLE dashboards    ADD COLUMN shared_file_id TEXT;
ALTER TABLE connections   ADD COLUMN shared_base TEXT;     -- its template's path is in shared_connection_id
ALTER TABLE connections   ADD COLUMN shared_file_id TEXT;
CREATE INDEX IF NOT EXISTS idx_saved_queries_shared_path ON saved_queries(project_id, shared_path);
CREATE INDEX IF NOT EXISTS idx_dashboards_shared_path ON dashboards(project_id, shared_path);
```

- Expand-only, and it runs on the beta-era baseline. Like `0001`–`0003` it makes `seaquel-cli mcp` refuse a file until the app has opened it once.
- **NULL means today's rule**: a project's directory is the slug of its name, a row's file is the slug path of its name, and there is no base (Q20's rule then applies when row and file differ). No data step: the first sync fills the columns from what it pairs.
- The rows' `Persisted*` types gain `sharedPath` (optional), so the GUI's deep links can find a row by path; the 5d fixture replays drop the new columns after checking them, as they do for `name_key`.

#### 34. Pairing and the three-way sync

- **Content and hash.** Each kind's content is the fields its file holds (a query: name, description, database, tags, parameters, text; a dashboard: name, description, widgets without run state, date filter, not the viewport (Q24: a viewport-only change is neither written nor counted as a change); a template: name, type, host, port, database, SSL mode, SSH host and port). Its hash is SHA-256 over the canonical file Core would write for it, so files written by older releases or by hand hash the same as Core's when they say the same thing.
  - **Defined exactly (Task 2 review, I5):** a file hashes as `write(parse(text))` and a row as `write(parse(write(row)))`, where `write` is Core's writer for the kind's content (without the id, and for a dashboard without the viewport) and `parse` is its reader. Every value `write` produces must read back unchanged, so a row and the file Core wrote for it always hash the same. That holds whatever line endings or edge whitespace the row holds: the writer writes `\n` only, and the reader trims the body. The format fixtures pin it (`write-query/crlf-body`, `queries/trailing-whitespace-and-crlf`).
- **Pairing**, each file with at most one row and each row with at most one file: by id (Q22), then stored path, then name (`name_key`, within the folder for queries), then the slug path. Two files claiming one row: the one at the stored path wins and the other is reported, not imported.
- **The rule**, with `R` the row's hash, `F` the file's and `B` the base:

| Case | Result |
|---|---|
| `R = B`, `F = B` | nothing |
| `R ≠ B`, `F = B` | write the file (a local change) |
| `R = B`, `F ≠ B` | update the row from the file; a query keeps its previous text as a version (Core's keyframe rule), a dashboard captures a version |
| `R ≠ B`, `F ≠ B`, `R = F` | nothing but the base |
| `R ≠ B`, `F ≠ B`, `R ≠ F`, or no base and `R ≠ F` | the file wins; the row's previous content becomes a version and the notice names it (Q20) |
| a shared row, its file gone, with a base, `R = B` | a teammate removed it: the row stays, unshared, named in the notice |
| a shared row, its file gone, with a base, `R ≠ B` | the same; the local change is in the row |
| a shared row, its file gone, path set and base NULL (its share's write failed) | write the file (Task 4b review, M5) |
| a shared row, its file gone, neither path nor base (an older release's row) | a teammate removed it: unshared, named in the notice |
| a file with no row | a new shared row under the file's exact name; a name another row holds skips it and says so once per file per session (5d-2's rule) |
| an update from a file whose name another row holds (in the plan's final state) | the row takes the rest and keeps its name; the base is the row's own hash; `NameTaken` once per session; each later sync tries again (Task 4b review, flag 4) |
| a name-only change from a teammate, and a local content change (the base is the file under the row's name) | they merge, field by field: the file takes the row's content under the teammate's name, and the next sync gives the row that name; no conflict, no version (coordinator's decision after the Task 4b re-review) |
| the file unreadable, unparseable, refused by the library's checks (`invalid`) or skipped (Decision 32) | nothing on either side; named in the notice; its row stays out of pairing |

- After a sync both sides agree, and each pair's base is the hash they share.
- **Notices.** A file is named at most once per session, whatever the notice. A file that claims a row already paired with another file (by name, or the slug path) is `Unpaired` and isn't imported: a create would be `NAME_TAKEN` by construction. `NameTaken` is for a file that claims no row and whose name a row that isn't shared has (Task 2 review, M1).

#### 35. When a sync runs

On activating a linked project, at startup for the active one, after linking or importing, and **after a pull, a commit or a conflict resolution**, for every project linked to that repo (bug 12). The background refresh runs one when the status shows a change. A repo with conflicted files isn't synced: the call answers `conflicted: true`, nothing is written on either side, and the GUI shows the conflict dialog it already has (bug 3).

#### 36. Publishing from inside the library calls

A library call that changes a shared row or its sharing (`savedQueryUpdate`, `savedQueryRemove`, `dashboardUpdate`, `dashboardRemove`, `connectionUpdate`, `connectionRemove`, `projectUpdate` for a linked project's `project.yaml` (Q25), and the `shared`/`isLocalOnly` patches) publishes after its commit, on desktop only (Decision 31): it takes the repo lock, reads the row again, plans the file operation, applies it and records the base. So every interface gets it, and edits reach the file (bugs 1 and 11).
- The answer's `Seqd` gains an optional `projection: {status: "written" | "deleted" | "failed", code?, message?}`. A failed write leaves the row stored and the GUI says so ("Saved. The shared file couldn't be written: …"); the next sync writes it, since the row no longer matches its base.
- Nothing publishes for a project without a link, or a row that isn't shared (bug 5).
- A rename moves the file (write the new path, then delete the old one unless it's the same path ignoring case).

#### 37. Order of removals

Unsharing or deleting a shared row deletes its file first, keeping the bytes, then writes the row; if the row write fails the file is put back. Sharing and edits write the row first, then the file (Decision 36). The keychain ordering of 5d's Decision 8 is the model.

#### 38. One lock per repo, and a safe pull

- Core keeps an async mutex per repo path. Sync, publish, and the `git` group's pull, commit and conflict resolution take it, so a checkout never races a file write. The page's own per-repo lock stays for the sync button's state.
- **A fast-forward checks out safely** (`CheckoutBuilder::safe()`): local changes that would be overwritten refuse the pull with `PULL_ERROR` "Commit or discard your changes to … first", naming at most ten paths (bug 2). The sync button commits first when it offers both.

#### 39. Names (Q21)

`seaquel_workspace::shared::file_stem` implements the chosen rule and `free_path` the collision suffix, both checked against the case-insensitive set of paths in the directory. Existing files are never renamed by a sync; only a rename of the row moves its file.
- **Details fixed by the fixtures (Task 2 and its review):**
  - The stem keeps letters, marks and digits (`\p{L}`, `\p{M}`, `\p{N}`), after NFC, lower case and NFC again.
  - Windows' reserved device names get `-file`: `con`, `prn`, `aux`, `nul`, `com0`–`com9`, `lpt0`–`lpt9`, and `com`/`lpt` followed by `¹`, `²` or `³`.
  - **Byte cap (I8).** A file name is at most 255 bytes. `file_stem` cuts the stem at a character boundary to 250 bytes (255 less `.json`/`.yaml`). `free_path` cuts it again when a `-n` suffix would take the whole name past 255. A `-` left at the end of a cut goes.
  - `free_path` compares names case-insensitively and after NFC, as APFS and NTFS do, so a teammate's NFD `ärger.sql` takes the name `ärger` (M2).
  - A rename whose new path equals the stored one ignoring case keeps the stored path.

### Projects, connections, dashboards

#### 40. Linking, unlinking, importing projects

- `sharedLinkProject {projectId, path, share}` registers the repo by path (or reuses it), picks the directory (an existing one whose `project.yaml` name has the project's `name_key`, else a free one by Decision 39), writes `project.yaml` if missing, exports the connections `share` names as templates **and links each to its template** (bug 6), stores `shared_dir`, and syncs. `share` is the link dialog's ticked connections (Q30: all but the local-only ones are ticked at first). It must name connections of the project (`INVALID_ARGUMENT` otherwise), and naming one already linked is a no-op. A connection left unticked isn't exported and stays local, under Decision 53. Pinned by `link/first-link-exports-templates` (ticked) and `link/unticked-connection-stays-local`.
- `sharedImportProjects {path, dirs}` creates one project per directory (`renameIfTaken`) with `shared_dir` set to that directory (bug 8), imports **that directory's** templates (bug 7), and syncs it.
- `sharedUnlinkProject {projectId, removeImported}` (Q31, amended in the Task 7 review): the connections linked to that directory's templates are unlinked and made local-only. The user's own (`connections.shared_origin` `exported`: ticked at link time, or shared later with the local-only switch) always stay, with their secrets. The ones the repo brought (`imported`, or a link with no origin, which only an older release's template import stored) are removed with their secrets when `removeImported`, the user's answer to the unlink dialog, and kept like the others otherwise. It answers `{removedConnectionIds, keptConnectionIds, repoRemoved}`, clears the links of its rows, and forgets the repo when no project uses it. Migration `0005_shared_connection_origin.sql` stores the origin: a sync's template import records `imported`, a publish that shares a connection records `exported`, and clearing the link clears it.
- Renaming a project follows Q25: Core rewrites `project.yaml`'s name and never moves the directory.

#### 41. Connection templates (Q23)

Synced by Decision 34 over the template's fields. A connection's user name, secrets, labels, AI settings and local-only flag are never read from or written to a template. A template's path stays in `shared_connection_id` (`<repoId>:<path>`, so older releases still match it); a rename moves the file and updates it.
- **Both sides changed (Q27).** The template wins: host, port, database, SSL mode and SSH host and port take its values. The notice is `Conflict { kind: connection, id, replaced }`, where `replaced` holds the local values the template overwrote (`host`, `port`, `databaseName`, `sslMode`, `sshHost`, `sshPort`, `name`). It never holds the user name or a secret.
- **Name (Q28).** A template's `name:` renames the connection (`R = B, F ≠ B`). If the connection was also renamed here since the last sync, Q27's rule applies.
- **Type (Q28).** A changed `type:` isn't applied. The notice is `TemplateTypeChanged { kind, id, path, template_type, imported }` (`kind` is always `connection`; the fixtures compare it), `imported` naming the connection the same sync made from the template (Q29), and the connection is unlinked (`shared_connection_id`, base and file id cleared) and made local-only, its fields as they were. The template is then a file with no connection, so the same sync imports it as a new connection under its name (`renameIfTaken`), appended to the connection order, and the notice mentions it (Q23, Q29; `connections/template-type-changed`).

#### 42. The right repo, always

Every projection call resolves the repo from the row's project (`git_repo_path`) and the directory from `shared_dir`. Core never reads `activeRepoId`; it stays in `app_state` as last written, for older releases. The GUI drops `state.activeRepoId` as a target for writes (bug 5) and keeps the active project's repo only for the sync button.

#### 43. The repo list

- The `shared` group gets `reposList`, `repoRegister {path, name?, remoteUrl?}` (idempotent by path), `repoUpdate {id, patch}` (`name`, `remoteUrl`, `branch`) and `repoRemove {id}`.
- **`lastSyncAt` is Core's (Task 2 re-review, M1).** A successful `git.pull` or `git.push` sets it, under the repo lock, spliced into the stored JSON like any patch. It is projection state (when this repo last met the remote), so the GUI never sends it. A "repo call", for the fixtures' byte-for-byte rule, is `shared.linkProject`, `unlinkProject`, `importProjects`, `repoRegister`, `repoUpdate`, `repoRemove`, `git.pull` or `git.push`. Core makes `repo-<uuid>` ids.
- Rows keep today's JSON; Core rewrites only the fields a patch names, the rest byte for byte (as 5d-2 does with `aiSettings`), so older releases read them.
- `syncStatus` is never written by a status refresh (bug 20); the GUI derives it from `status` in memory, and older releases refresh it themselves at startup.
- `sharedReposLoadAll` and `sharedReposSaveAll` leave the storage group; their functions stay for the frozen `shared-repos.json` fixture.

#### 44. Events

- A new kind, `sharedRepo`: a repo list write (ids: the repo id), and any file a sync or publish wrote (ids: the repo id, so each window refreshes that repo's git status once).
- A sync's row writes emit their own kinds (`savedQuery`, `dashboard`, `connection`, `project`) with the caller's origin, one event per kind and scope per sync, under 5d's id bounds.
- No event for a scan or a sync that changed nothing.

#### 45. The formats

Ports of today's readers and writers, with four fixes (all in `changes.json`):
- `\r\n` and a leading BOM are accepted (bug 13); files are written with `\n`, as today.
- A value that needs quotes and holds `"` or `\` but no `'` and no newline is written in single quotes, which older readers strip exactly (Decision 52). Any other value that needs quotes is double-quoted. Inside, `"` is written `\"` and a newline `\n`, and a `\` is written `\\` only where the reader would misread it: before `"`, `\`, `n` or a newline, or as the last character (Task 4b review). Every value is written this way, `database:`, a parameter's `type:`, a template's `type:` and `sslMode:` included, and a port is a whole number (Task 4b review, I3). The reader undoes `\"`, `\\` and `\n` inside double quotes only in a file Core wrote, which it knows by the file's `id:` line; it keeps any other backslash pair as written, and undoes `''` inside single quotes (bug 14; Task 2 review, I4).
- A query or template file without an `id:` line (written by 2026.9.2 or earlier, or by hand) reads with the legacy rule: the double quotes are stripped and what's inside is kept literally, exactly as 2026.9.2's reader does, so `"C:\new: path"` stays `C:\new: path` (probe fix 2). The earlier text here claimed 2026.9.2's writer never produced such a value; it did (a value with `: ` and a backslash needed quotes), so the escape rule only applies to files that carry Core's `id`. `content_hash` and `file_hash` agree for both kinds of file, since the three-way hashes are `write(parse(text))` in Core's form. `project.yaml` and `labels.yaml` keep the escape rule: they carry no `id`, and Core's own escaped values there must round-trip; a hand-written backslash inside double quotes in them reads as an escape (known limit).
- A tag or label holding `,` is quoted in an inline list, and the reader splits only outside quotes.
- A description holding a newline is written as `\n` inside double quotes; older readers show `\n` literally, which beats losing the rest.

Unknown keys stay ignored, `default` and `defaultValue` both read, and a missing `project.yaml` still names the project after its directory.

#### 46. Labels

`labels.yaml` and a template's `labels` stay unread for now: the reader is ported so a later slice can use it, and nothing is applied to connections. A follow-up (what they mean is a product question).

### Imports

#### 47. Readers and the import in Core

- `importsCandidates {source, projectId, path?}` reads the default location (today's paths, per platform; TablePlus on macOS only) or the given file, and answers `{found: false}`, `{found: true, unreadable: message}` or the candidates: each with a key, the fields today's mapper produces, `duplicateOf` (the project connection with the same type, host, port, database and user) and a `problem` when it can't be imported (an unsupported driver stays out of the list, as today).
  - **Problems (Task 2 review, I7 and M8).** `invalidPort`, with `port: 0`, for a port that isn't a number or is outside 0–65535 (bug 22). For TablePlus, `noId` for an entry without an `ID` and `duplicateId` for each entry whose `ID` another entry has; both come before `invalidPort`. After the Task 4a review (coordinator's decision, for the owner to overrule): `invalidSshPort`, with the tunnel's `port` 0, for a TablePlus SSH port outside 0–65535, and `noName` for a DBeaver connection with a missing or blank name, since `connectionCreate` refuses both. They come after `invalidPort`. No recorded case has either; `imports_plan.rs` pins them.
  - **Keys (re-review M4).** A TablePlus key is `id:<ID>` (the entry's `ID` as text), or `pos:<n>` (its 0-based position in the plist's list) for a `noId` or `duplicateId` entry. The prefixes keep the two kinds apart, so no `ID` can collide with a position. DBeaver keys are the connection's key in `connections`, as it is (unique by construction).
  - **`importsCreate` refuses a key whose candidate has a problem** (`INVALID_ARGUMENT`, naming the key), whatever the GUI sent. The GUI then says "No TablePlus connections found" instead of nothing.
- `importsCreate {source, projectId, keys, path?}` reads the file again, imports the selected candidates in one `WriteTx`: the duplicate check inside the transaction (bug 23), `connectionCreate`'s rules with `renameIfTaken` and local-only, and the new ids appended to the project's connection order in the same transaction. It answers each key's outcome and emits one `connection` and one `project` event.
- The mapping is today's (driver and provider tables, default ports, TablePlus TLS modes, SSH settings), with port parsing that matches `parseInt` (leading digits). DBeaver's SSH handlers, URL-only configurations and other projects stay unread (a follow-up).
- Paths are resolved from an injected home (`ImportPaths { home }`), so tests never read the real one.

#### 48. No TypeScript twin

The demo and web have neither feature; `NoShared` and `NoImports` answer `NOT_SUPPORTED`, and the GUI keeps hiding the entry points as today. Nothing is added to `TsLibrary`.

### Everything else

#### 49. The main window's file permissions

`fs:allow-read-dir`, `fs:allow-exists`, `fs:allow-mkdir` and `fs:allow-rename` leave `capabilities/default.json` (an `rg` in Task 7 shows no other caller). The rest stay for the exports, the theme import and the ERD viewer; narrowing them is a follow-up.

#### 50. Logs and errors

Repo ids, project ids, kinds, counts and codes. Never a path (it holds the user's home and project names), a name, a host or a file's content. Errors name a path relative to `.seaquel/` only in the GUI-facing message, never in a log. Every new params and plan type has a hand-written `Debug`, and a `capture_logs` test seeds canaries in each.

#### 51. Damage already done stays

"<name> (2)" connections from earlier links, `untitled.sql` files that hold one of several queries, and rows unshared by bugs 3, 4 and 10 aren't repaired: none can be told apart from an intended state. The first sync's notice names files it couldn't pair, which covers the visible part.

#### 52. Older releases and older teammates

- 2026.9.x opens the file after `0004` and ignores the new columns. Its reconcile and edits still behave as today; their effects reach 5e's rows as file changes or row changes, which the three-way rule handles (an older release editing a row changes `R` but not `B`).
- Teammates on older releases read every file 5e writes: new keys (`id`) are ignored, single-quoted values read correctly, and Unicode file names (Q21) are scanned like any other.
- Known limits (probe item 3, no code): a value holding both `'` and `"` is double-quoted with `\"`, which 2026.9.2 reads with the backslash kept; a tag or label holding `,` is written quoted in an inline list, which 2026.9.2 splits at the comma and reads with the quote characters kept. A value holding both `'` and `\` is double-quoted with the backslash escaped where Core's reader needs it, which 2026.9.2 reads literally, so `It's C:\new: x` comes back as `It's C:\\new: x` (review A3). None of these survives a round trip through a 2026.9.2 teammate. A newline in a description is another (Decision 45).

#### 53. Only explicitly shared connections reach the repo

Added in the Task 2 re-review (2026-09-30, the coordinator's decision, M5). A connection is exported to the repo only when it is explicitly shared, which means it has a `shared_connection_id` (a link to its template). Sharing it means ticking it in the first link's dialog (Q30) or using the local-only toggle in a linked project. Either one stores the link and writes the template. The link is stored even when the template write fails: the base stays unset, so the next sync writes the file. A connection in a linked project that isn't `is_local_only` but was never shared is treated as local. That includes every row from before the local-only default, where `is_local_only` 0 is the old default and not a choice (bug 5). Nothing publishes it, a link exports it only if it was ticked (Q30), and a sync never writes a template for it, so its host never leaks into a team repo. Its row is left as it is. Pinned by `connections/never-shared-in-a-linked-project` and `link/unticked-connection-stays-local`.

---

## The split

**Recommendation: one slice, 5e**, with Task 1 shippable on its own as a patch release.

- **The two parts share their new machinery**: the `LocalFiles` policy, the desktop-only groups, the injected paths, and the CLI-shaped test. Imports alone are about 3–4 h; their own probe and checkpoint would add about 1.5 h for little risk.
- **The risk is in one place**, the three-way sync, and one probe at production size can aim at it.
- **Task 1 stops the worst damage now** (bugs 1, 2, 3 and 5) without waiting for Core; the owner can release it before the rest.
- **If time runs short**, the imports tasks finish first and the projection can become a 5e-2 without redoing them. The owner's answers are scope, not cuts; the one cut left is Decision 49's capability trim.

---

## The wire and the API

Wire rules as 5a–5d: `method` before `params`, bodies parsed from raw bytes, `deny_unknown_fields` on every params type, `Clearable` with `ts(optional)`, hand-written `Debug`.

### Pure

```rust
// crates/seaquel-workspace/src/shared.rs
pub mod format {
    pub fn parse_query(text: &str, rel_path: &str, queries_dir: &str) -> QueryFile;
    pub fn write_query(q: &QueryFile) -> String;
    pub fn parse_dashboard(text: &str, rel_path: &str) -> Option<DashboardFile>;
    pub fn write_dashboard(d: &DashboardFile) -> String;
    pub fn parse_template(text: &str) -> Option<TemplateFile>;        // credentials dropped
    pub fn write_template(t: &TemplateFile) -> String;
    pub fn parse_project(text: &str, dir: &str) -> ProjectFile;
    pub fn write_project(p: &ProjectFile) -> String;
    pub fn parse_labels(text: &str) -> Vec<LabelFile>;
}
pub fn file_stem(name: &str) -> String;                                // Q21
pub fn free_path(dir: &str, stem: &str, ext: &str, taken: &dyn Fn(&str) -> bool) -> String;
pub fn content_hash(canonical: &str) -> String;                        // SHA-256, hex

pub struct ProjectLink { pub repo_id: String, pub dir: String }
pub struct RawFile { pub rel_path: String, pub text: String }
pub struct DirScan { pub files: Vec<RawFile>, pub skipped: Vec<Skipped>, pub conflicted: bool }
pub struct SharedRows { pub queries: Vec<LinkedQuery>, pub dashboards: Vec<LinkedDashboard>, pub connections: Vec<LinkedConnection> }

pub fn plan_sync(link: &ProjectLink, scan: &DirScan, rows: &SharedRows, rule: ConflictRule) -> SyncPlan;
pub fn plan_publish(link: &ProjectLink, change: RowChange<'_>, taken: &dyn Fn(&str) -> bool) -> Vec<FileOp>;

pub struct SyncPlan { pub rows: Vec<RowOp>, pub files: Vec<FileOp>, pub bases: Vec<BaseUpdate>, pub notices: Vec<SyncNotice> }
pub enum FileOp { Write { rel_path: String, text: String }, Delete { rel_path: String, expect_hash: String } }
pub enum RowOp { CreateQuery(SavedQueryDraft, LinkFields), UpdateQuery(String, SavedQueryPatch, LinkFields), Unshare(Kind, String), /* dashboards, connections alike */ }
pub enum SyncNotice {
    Conflict { kind, id, replaced: Option<TemplateValues> },   // `replaced`: connections only (Q27)
    RemovedInRepo { kind, id },
    NameTaken { path, taken_by },                              // a file claiming no row, its name taken
    Unpaired { path, claims },                                 // a file claiming a row another file has (M1)
    Skipped { path, why },                                     // why: symlink | unreadable | doesNotParse | …
    TemplateTypeChanged { kind, id, path, template_type, imported }, // Q28; kind = connection; `imported`: the new connection's id (Q29)
}

// crates/seaquel-workspace/src/imports.rs
pub enum ImportSource { Tableplus, Dbeaver }
pub struct ImportCandidate {
    pub key: String, pub name: String, #[serde(rename = "type")] pub ty: String,
    pub host: String, pub port: f64, pub database_name: String, pub username: String,
    pub ssl_mode: Option<String>, pub ssh_tunnel: Option<SshTunnelConfig>,
    pub duplicate_of: Option<String>, pub problem: Option<String>,
}
pub fn tableplus_candidates(entries: &serde_json::Value) -> Vec<ImportCandidate>;
pub fn dbeaver_candidates(json: &[u8]) -> Result<Vec<ImportCandidate>, ImportError>;
pub fn mark_duplicates(candidates: &mut [ImportCandidate], existing: &[ConnectionIdentity]);
```

### Storage and git

```rust
// crates/seaquel-storage/src/queries
shared_repos::{list, get, get_by_path, insert, update_json, delete}      // WriteTx; save_all/load_all frozen
projects::set_shared_dir; saved_queries::{set_link, by_shared_path}; dashboards::{set_link, by_shared_path};
connections::set_link;                                                   // shared_base, shared_file_id
// migrations/0004_shared_links.sql (Decision 33)

// crates/seaquel-git/src/tree.rs
pub async fn scan(root: &Path, dir: &str, bounds: ScanBounds) -> Result<DirScan, GitError>;   // no symlinks
pub async fn apply(root: &Path, ops: &[FileOp]) -> Result<Applied, GitError>;                  // atomic writes
```

### Core

```rust
pub enum LocalFiles { Allowed }                    // CoreBuilder::local_files; absent: denied
impl Workspace {
    // shared.rs (git + storage)
    pub async fn shared_repos(&self) -> Result<Seqd<Vec<Box<RawValue>>>, CoreError>;
    pub async fn shared_repo_register(&self, core: &Core, origin: &WriteOrigin, path: &str, name: Option<String>, remote_url: Option<String>) -> Result<Seqd<Box<RawValue>>, CoreError>;
    pub async fn shared_repo_update(/* id, RepoPatch */) -> …;  pub async fn shared_repo_remove(/* id */) -> …;
    pub async fn shared_link_project(&self, core: &Core, origin: &WriteOrigin, project_id: &str, path: &str, share: &[String]) -> Result<Seqd<SyncReport>, CoreError>;
    pub async fn shared_unlink_project(/* project_id */) -> Result<Seqd<UnlinkReport>, CoreError>;
    pub async fn shared_scan(&self, core: &Core, path: &str) -> Result<RepoPreview, CoreError>;       // read-only
    pub async fn shared_import_projects(/* path, dirs */) -> Result<Seqd<Vec<String>>, CoreError>;
    pub async fn shared_sync(&self, core: &Core, origin: &WriteOrigin, target: SyncTarget /* project | repo */) -> Result<Seqd<SyncReport>, CoreError>;
    // imports.rs (imports + storage)
    pub async fn import_candidates(&self, core: &Core, source: ImportSource, project_id: &str, path: Option<&str>) -> Result<ImportCandidates, CoreError>;
    pub async fn import_create(&self, core: &Core, origin: &WriteOrigin, source: ImportSource, project_id: &str, keys: &[String], path: Option<&str>) -> Result<Seqd<ImportOutcome>, CoreError>;
}
pub struct SyncReport { pub conflicted: bool, pub rows_changed: u32, pub files_written: u32, pub notices: Vec<SyncNotice> }
pub struct Seqd<T> { pub value: T, pub seq: ChangeSeq, #[serde(skip_serializing_if = "Option::is_none")] pub projection: Option<ProjectionOutcome> }
```

### RPC

```rust
// crates/seaquel-rpc/src/shared.rs: desktop only (LocalFiles); dispatch_workspace refuses it otherwise
pub enum SharedRequest {
    ReposList, RepoRegister { path, name?, remote_url? }, RepoUpdate { id, patch: RepoPatch }, RepoRemove { id },
    LinkProject { project_id, path, share: Vec<String> }, UnlinkProject { project_id },   // share: the dialog's ticked connection ids (Q30)
    Scan { path },                                 // the import dialog's preview: projects, templates, file counts
    ImportProjects { path, dirs: Vec<String> },
    Sync { project_id } | SyncRepo { repo_id },
}
// crates/seaquel-rpc/src/imports.rs
pub enum ImportsRequest { Candidates { source, project_id, path? }, Create { source, project_id, keys, path? } }
// StorageRequest loses sharedReposLoadAll and sharedReposSaveAll; StoredKind gains sharedRepo.
```

- New codes: `REPO_NOT_FOUND`, `PROJECT_NOT_LINKED`, `REPO_CONFLICTED` (a sync of a conflicted repo answers `conflicted: true` instead; the code is for link and import), `FILE_ERROR` (with the path relative to `.seaquel/`), `IMPORT_SOURCE_UNREADABLE`. Desktop only, so no web statuses; the server maps `NOT_SUPPORTED` as it does.
- `src-tauri` serves both groups through its storage-backed arm with the webview label; `read_dbeaver_config` and `read_tableplus_config` go.

### TypeScript

```ts
// src/lib/hooks/database/shared/types.ts
export interface SharedService {
  listRepos(): Promise<Seqd<PersistedSharedQueryRepo[]>>;
  registerRepo(path: string, init?: { name?: string; remoteUrl?: string }): Promise<Seqd<PersistedSharedQueryRepo>>;
  linkProject(projectId: string, path: string, share: string[]): Promise<Seqd<SyncReport>>;
  unlinkProject(projectId: string): Promise<Seqd<UnlinkReport>>;
  scan(path: string): Promise<RepoPreview>;
  importProjects(path: string, dirs: string[]): Promise<Seqd<string[]>>;
  sync(target: { projectId: string } | { repoId: string }): Promise<Seqd<SyncReport>>;
}
export interface ImportsService {
  candidates(source: ImportSource, projectId: string): Promise<ImportCandidates>;
  create(source: ImportSource, projectId: string, keys: string[]): Promise<Seqd<ImportOutcome>>;
}
```

---

## Parity fixtures

Recorded from today's TypeScript before it moves, after Task 1, frozen, each set with a `README.md` and a `changes.json`, as 5c and 5d did.

### What gets recorded

**`crates/seaquel-workspace/tests/fixtures/shared/formats.json`**: the parsers and writers alone. For each format, a corpus of inputs and what `parse*` returns, and of objects and what `serialize*` writes, plus `parse(serialize(x))`:
- queries: with and without frontmatter, every field, `default` and `defaultValue`, empty values, a body holding `---`, CRLF, a BOM, tabs, conflict markers in the frontmatter and in the body, names and descriptions with `:`, `#`, `"`, `'`, `[`, `,`, a newline, edge spaces, Unicode;
- dashboards: valid, without a name, not JSON, with run state on widgets, without a viewport;
- templates: every field, credentials at both levels, `.yml`, a missing name, an unknown type, a port that isn't a number, `sshTunnel` with `enabled: false`;
- projects and labels: present, missing fields, comments;
- `nameToFilename` and `dashboardNameToFilename` on about 40 names (Latin, Cyrillic, CJK, emoji, punctuation only, case pairs, reserved words).

**`crates/seaquel-workspace/tests/fixtures/shared/projection.json`**: the managers end to end. Each case has a seed (rows, a file tree, the repo list), steps (manager calls), and after each step the file tree, the rows, the toasts and the view.
- Link and unlink: a project with old connections (`is_local_only` 0) and new ones; link, then the settings save's template import (bug 6); a repo with two shared projects (bug 7); unlink with imported connections and secrets.
- Import projects: a taken name ("Main (2)", bug 8), a `project.yaml` name that isn't its directory's slug, a missing `project.yaml`.
- Queries: share, edit from the tab, rename (also case-only and across folders), unshare, delete; a non-Latin name and two of them (bug 10); "Sales" and "Sales!"; a name with `"` (bug 14); a hand-written `my_query.sql`; a reconcile with a new file, a changed one, a removed one, two files with one name, a CRLF file, a file named "STRASSE" by a row "Straße" (bug 15).
- Dashboards: share, a widget edit then an activation (bug 1 as fixed by Task 1, and the pre-fix recording in `README.md`), a pan, a rename (bug 11), unshare, delete, a file that doesn't parse.
- Connections: edit and remove of a shared connection, the local-only toggle, an edit in an unlinked project after visiting a linked one (bug 5 as fixed by Task 1), a template changed, renamed and removed in the repo (bug 16).
- Repo states: conflicted (bug 3 as fixed), a directory that can't be read (bug 4), a symlinked `.sql` file and a symlinked `queries/` directory (bug 17), a pull without a following activation (bug 12).
- A project rename (bug 9).

**`crates/seaquel-workspace/tests/fixtures/imports/`**: `tableplus.json` and `dbeaver.json`, each a list of inputs (the TablePlus entries as the JSON `read_tableplus_config` produces today, and the same entries as an XML plist; DBeaver's `data-sources.json` bytes) with an `existing` list, and what `discover*` returns. Cases: every driver and provider, default and custom ports, a port that isn't a number, each TLS mode, SSH with password and key, SSH off, SQLite paths, an unsupported driver, duplicates by each of the five fields, an empty file, a file that isn't JSON, no `connections` key. The create side is 5d-1's `library/imports.json`, already frozen.

### How

- **The recorder** is `docs/plans/artifacts/2026-10-05-record-shared-fixtures.test.ts.txt`, copied to `src/lib/hooks/database/record-shared.test.ts` and run with `FREEZE_SHARED=1`, then deleted.
- **The managers** run over the demo's seams on an in-memory sql.js file (`TsLibrary`, `TsState`, `TsSettings`, `TsUi`), wired as `state-replay.svelte.test.ts` wires them, with the 5d recorders' fake clock, uuid counter and toast capture.
- **The file system** is an in-memory tree behind mocks of `@tauri-apps/plugin-fs` (`readDir`, `readTextFile`, `exists`, `writeTextFile`, `mkdir`, `remove`, `rename`, `stat`) and `@tauri-apps/api/path` (`join`, `dirname`). Entries can be files, directories, symlinks (to a path inside or outside the tree) and unreadable directories; `stat` throws, as the missing permission makes it do (bug 19).
- **Git** is a stub whose `status` and `pull` a case sets (clean, ahead, conflicted with markers written into the tree).
- **Import inputs** go through the real `toTablePlusConnection`, `mapToImportable` and `parseDbeaverConnections` with `$lib/api/tauri` mocked to return the case's text. Task 2 also writes each TablePlus case as an XML plist, and the Rust replay first checks that `plist` decodes it to the recorded JSON, which pins the step that moves out of `src-tauri`.
- Ids are `<id:n>` keeping their prefix, times `<now>`; a file tree is `{path: text}` with symlinks as `{"symlink": target}`. Recorded twice, byte-identical.

### Replays

- **Rust, pure** (`seaquel-workspace/tests/{shared_formats,shared_plan,imports_plan}.rs`): every format case and every import case, exactly, after `changes.json`. For the projection, each step's scan and rows go through `plan_sync`/`plan_publish`, and the planned file and row changes must equal the recorded ones.
- **Rust, Core** (`seaquel-core/tests/shared.rs`): each projection case on a temp directory and a temp `Storage` with a `MemoryStore`, its steps as Core calls (`changes.json` gives a `core` call where a step's GUI call maps to one), comparing the file tree and the rows after each step.
- **No TypeScript replay**: there is no TypeScript twin (Decision 48). The GUI's own tests use a recording `SharedService`.

### `changes.json`

Each intended difference, with its Decision. Expected entries:
- Core ids and times (shape only), and file letters kept, per Q21 (Decision 39);
- links stored, pairing by id, path, then name (`name_key`), each file to one row (Decisions 33, 34);
- the three-way rule: local edits written, teammates' changes applied with a version, conflicts per Q20, removed files unsharing with a notice (Decision 34);
- a sync after a pull, commit and resolve, and of every linked project (Decision 35);
- a conflicted repo, an unreadable directory and a skipped file changing nothing (Decisions 32, 35);
- symlinks skipped and refused (Decision 32);
- link without duplicates, one directory's templates, the stored directory, a rename that keeps the directory (Decisions 40, Q25);
- templates synced (Q23, Decision 41);
- the four format fixes (Decision 45);
- a viewport-only change writing nothing (Q24);
- imports: candidates with `duplicateOf` and `problem`, `found: false` and `unreadable` instead of an empty list (Decision 47).

A difference the Core task finds that isn't listed is a finding to report, not an entry to add.

---

## Ground rules

5d's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules; parallel-agent file ownership with small re-read edits to shared files;
- tests never touch the real keychain, data dir, `~/.ssh` or home;
- no secrets, names, hosts, strings, text or values in `Debug`, errors, logs or events;
- the full check list; npm through mise; one shared `CARGO_TARGET_DIR` (`/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`); the MSSQL `tls_server_name` live tests fail on this machine's certificate store (5d-2 checkpoint);
- effort log: `docs/plans/2026-10-05-phase-5e-effort.md`.

### Constraints the executors must obey

- **`seaquel-workspace::{shared, imports}` do no I/O** and never panic on input (proptest over the parsers and the planner). They build for wasm32.
- **File I/O only in `seaquel-git::tree`**, under Decision 32's rules, and only through Core with `LocalFiles::Allowed`.
- **No file I/O inside a `WriteTx`.** A sync reads files, then writes rows in one transaction, then writes files under the repo lock; a publish commits, then writes the file. The repo lock is never held while waiting for the write lock's turn in reverse order (always repo lock first, then `write()`).
- **Storage:** one migration, `0004_shared_links.sql`, under the migrations README's rules, with a case in `tests/baseline.rs`; reads inside a write go through `&mut tx`; the frozen fixtures stay.
- **Tests use temp directories** for repos (with real `git init` where git is involved) and an injected home for imports.
- **The UI does no projection work**: no file paths built, no file read or written, no pairing, in the managers.
- **The demo stays as it is.**

### Things a task could quietly skip

Reviews check each by name:
- a file read or written through a symlink, or a path not checked by Decision 32;
- a scan error, a skipped file or a conflicted repo treated as "file missing";
- a publish that writes the call's copy instead of the row read again under the lock;
- a projection write into a repo other than the row's project's, or for an unlinked project;
- `activeRepoId` read by Core;
- file I/O inside a `WriteTx`, or the repo lock taken after `write()`;
- a pull, commit or resolve that doesn't take the repo lock, or a fast-forward still forced;
- a base not updated after a sync or a publish, or updated after a failed write;
- the CRLF, quoting and comma fixes in the reader but not the writer, or the reverse;
- a rename that leaves the old file, or deletes the new one on a case-insensitive disk;
- a sync that emits an event when nothing changed, or more than one per kind and scope;
- the `projection` outcome dropped by `CoreLibrary` so a failed file write isn't shown;
- `importsCreate` trusting keys without reading the file again, or appending to the order outside its transaction;
- the injected home not used somewhere, so a test reads the real `~/Library`;
- `dispatch_workspace` serving `shared` or `imports` on a Core without `LocalFiles`;
- a path, name, host or file content in a log line;
- (GUI) a manager still importing `@tauri-apps/plugin-fs` for the projection, or calling `nameToFilename`.

---

## Order and estimates

Sized from 5d's logged times (effort log; design doc "Phase 5d cost"). First passes are compared with the nearest 5d task, review fixes are budgeted at about two-thirds of first passes (5d: 64–70%), and probe fixes at 2–3.5 h (5d: 3.9 h and 3.4 h, both above budget, both from scale).

| # | Task | First pass | Nearest 5d task (logged first pass) | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | Fixes now: dashboard edits to the file, the right repo, the conflict guard, a safe fast-forward | 0.35–0.5 h | 5d-1 T1 (0.27 h), 5d-2 T1 (0.25 h); one Rust fix | — | 3 |
| 2 | Fixtures: formats, projection over an in-memory file system, imports | 0.6–0.9 h | 5d-2 T2 (0.6 h, +0.3 h review) plus the file system mock | 1 | 3, 4a |
| 3 | Storage: `0004`, targeted repo writes, link columns | 0.4–0.6 h | 5d-1 T3 (0.47 h) | — | 1, 2 |
| 4a | Domain, imports: mapping, duplicates, replays | 0.25–0.35 h | part of 5d-1 T4 | 2 | 3 |
| 4b | Domain, shared: formats, names, hashing, pairing, `plan_sync`, `plan_publish`, replays | 0.8–1.1 h | 5d-1 T4's domain half (~0.6 h) plus the new planner | 2 | 3 |
| 5 | Core and `seaquel-git::tree`: `LocalFiles`, repo lock, scan and apply, sync, publish from library calls, link/unlink/import, imports reading | 1.3–1.7 h | 5d-2 T4 (1.5 h) | 3, 4a, 4b | — |
| 6 | RPC `shared` and `imports`, the desktop wiring, the git group under the lock, `src-tauri` commands out, `types:gen` | 1.0–1.6 h wall (~0.5–0.7 h work) | 5d-2 T5 (2.5 h wall, ~1 h work) | 5 | — |
| 7 | GUI: the seams, `SharedRepoManager` as a view model, managers rewired, TS projection and parsers deleted, dialogs (the link dialog's connection checkboxes, Q30, ~0.5 h), deep links, capabilities | 1.8–2.4 h | 5d-1 T6 (1.9 h), 5d-2 6b (~1 h wall) | 6 | — |
| 8 | Probe | 0.5–0.8 h | 5d-1 T7 (0.65 h), 5d-2 T7 (0.35 h) | 7 | — |
| 9 | Docs, measurement, checkpoint, one full live run | 1.5–2.3 h wall | 5d-2 T8 (2.25 h wall, ~0.6 h work) | all | — |
| | **First passes** | **8.5–12.25 h** | 5d-2: 11.3 h | | |
| | Review fixes (~60–65% of first passes, Task 9 excluded) | 4.2–6.4 h | 5d-2: 5.75 h | | |
| | Probe fixes | 2–3.5 h | 5d-2: 3.4 h | | |
| | Owner answers and old repos (files found in the probe that no case had) | 0.3–0.7 h | 5d-2: 0.3–0.7 h budgeted | | |
| | **Total** | **~15–22.8 h** | 5d-2: 20.5 h | | |

**Expect about 18 h.** Re-checked after the owner's answers (2026-09-30): every answer is the option the first passes were sized for (Q22's ids and Q23's template sync were already in Tasks 3, 4b and 5; Q26 A adds no CLI work), so the estimate stands; only the cut list shrank to the capability trim. Q30 (2026-10-01) adds the link dialog's checkboxes to Task 7, about 0.5 h, plus about 0.3 h of review fixes: **expect about 18.8 h**. Imports (Tasks 4a and their share of 3, 5, 6 and 7, plus fixes) are about 3.5 h of it.

The riskiest tasks:
- **Task 4b and 5:** the three-way rule against real repos, the ordering of file writes, the repo lock with the write lock, and old rows with no base. Expect the review to find ordering bugs (a publish racing a sync, a rename on a case-insensitive disk).
- **Task 7:** `ProjectManager`, `ConnectionManager`, `SavedQueryManager` and `DashboardManager` lose their file paths in one change, and the import dialogs change shape.
- **The probe:** the first time the projection meets a large repo, a hostile one and a teammate editing at once.

Cut if time runs short: Decision 49's capability trim (a follow-up then). Q20–Q26 are answered and in scope. Task 1 ships regardless.

---

## Task 1: Fixes now

TypeScript and one Rust change, each small enough to ship as a patch before 5e. The recorder records the fixed behaviour.

**Files:**
- `dashboard-manager.svelte.ts`: a stored change to a shared dashboard's name, description, widgets or date filter writes its file (Q24: not a viewport-only change); a rename writes the new file and deletes the old path unless it's the same ignoring case (5d-1's query rule). A failed write is said with `errorToast` and doesn't undo the stored change (bug 1).
- `project-manager.svelte.ts` (`setActive`, `:602-610`): activating a project without a git path clears `activeRepoId`. `shared-repo-manager.svelte.ts` (`shareConnection`, `updateSharedConnection`, `unshareConnection`), `shared-query-manager.svelte.ts` and `shared-dashboard-manager.svelte.ts`: the repo and directory come from the row's project, not the active ones, and nothing is written for a project without a git path (bug 5).
- `project-manager.svelte.ts` (`reconcileGitState`) and `hooks/database.svelte.ts` (`initializeSharedRepos`): no reconcile while the repo's status has conflicted files; a toast says the shared files will be read once the conflicts are resolved (bug 3).
- `crates/seaquel-git/src/ops.rs`: the fast-forward checks out with `CheckoutBuilder::safe()` and refuses with `PULL_ERROR` naming at most ten paths when local changes would be overwritten (bug 2). The sync button's message for it.
- Check bug 19 (`stat`) in a dev build and record the answer in the plan; add nothing.

**Tests first** (they fail before the fix):
- `a widget edit on a shared dashboard writes its file`, `a pan alone writes nothing`, `renaming a shared dashboard leaves one file`, `the next activation keeps the edit`;
- `saving a connection in a project without a git path writes no file, after visiting a linked project`, `a shared query is written to its own project's repo`;
- `a conflicted repo isn't reconciled, and its dashboards stay shared`;
- Rust: `a_fast_forward_keeps_uncommitted_changes` (a temp repo, an edited tracked file, a remote ahead: the pull refuses and the file is unchanged), `a_fast_forward_without_local_changes_still_works`.

**Run:** `mise exec -- npx vitest run src/lib/hooks/database src/lib/components`; `cargo test -p seaquel-git`; `npm run check` 0/0; the autofixer on changed `.svelte` files.

**Review:** no projection write resolves its repo from `activeRepoId`; the dashboard file is written from the stored row, not the in-memory copy.

**Things this task could quietly skip:** the delete of the old file on a dashboard rename; the startup reconcile path as well as activation; the conflicted check reading the repo's status, not the in-memory sync state, which can be stale.

### Notes from Task 1 (as built)

2026-09-30, ~0.55 h (21:40–22:12, including a cold `seaquel-git` build of ~8 min), then ~0.25 h of review fixes (22:18–22:33).

- **Bug 1.** `DashboardManager.save` writes the file after Core answers, from the stored row (`dashboardFromWire(updated.dashboard)`), when the row is shared and the patch names the name, description, widgets or date filter, or sets `shared: true` (`changesFile`). A viewport-only patch writes nothing (Q24). Writes are chained per dashboard, so they land in the order the saves answered. A rename writes the new file, then deletes the file under the old name unless `nameToFilename` of both is the same (case-insensitively). A failed write is `errorToast` with the new `dashboard_shared_file_failed`, and the stored edit stays (see the review fixes for the wording). `shareDashboardById` no longer writes the in-memory copy before the save: sharing stores the flag, then the same path writes the file (Decision 37's order). Restore and the date filter go through `save` and write too.
- **Q24 on the read side.** Writing a pan alone would be pointless if the next activation still put the file's viewport back. So the TS dashboard reconcile's `sameContent` no longer compares the viewport, and a file that differs only there changes nothing. A file that differs in something else still brings its viewport along. This is Decision 34's content rule ahead of Core, and Task 2 records it as fixed behaviour.
- **Bug 5.** `SharedRepoManager.repoForProject(projectId)` resolves the repo from the project's `gitRepoPath` and the directory from its name (today's slug rule; bug 8 is unchanged). Every file write and the reconcile go through it: `shareConnection`, `updateSharedConnection` and `unshareConnection` use `connection.projectId`; the query and dashboard writes and deletes use the row's `projectId`; the two reconciles use the `projectId` they are given. The dead `createFolder` and `createDashboard` use the active project's link. A project without a git path gets nothing written. Nothing reads `activeRepoId` to pick a write target any more. `ProjectManager.activateLinkedRepo(id)` sets `activeRepoId` to the project's repo, or `null` when it has none, and reconciles a linked project. `setActive` (`null` included) and startup both call it. Startup no longer restores the stored `activeRepoId`, so what gets saved back is the derived one. `initRepo`, `cloneRepo` and `removeRepo` used to make the first or next repo active; they now set the active project's repo.
- **Bug 3.** `reconcileGitState` (the one path for activation, startup and the shared-project import) first reads `getRepoStatus(repo.path)` itself, ignoring the page's sync state. If it has conflicted files, nothing is reconciled and a warning toast `shared_reconcile_conflicted` names the project. If the status can't be read, the reconcile is skipped and only the error code is logged, since that isn't proof the files are whole.
- **Bug 2.** `pull_repo`'s fast-forward first runs `checkout_tree` on the fetched commit with `CheckoutBuilder::safe()` and conflict notifications, while HEAD still names the old commit. Only after that succeeds does it move the branch and HEAD. If local changes would be overwritten (an edit, or an untracked file in the way), libgit2 refuses before writing anything, and the pull fails with `PULL_ERROR` "Commit or discard your changes to a, b, … first". The message names at most ten paths, sorted, plus "and N more". The constant `PULL_REFUSED_LOCAL_CHANGES` is exported. The warn log carries only the count. The sync button's Pull and Sync all use `pullFailureText` (`components/shared-queries/pull-error.ts`), which shows `shared_pull_local_changes` ("Nothing was pulled, so your uncommitted changes are kept. …") for that refusal. "Sync all commits first" (Decision 38) is left for Task 7.
- **Bug 19: confirmed refused, by reading.** I couldn't run the desktop app, so this comes from the config, not a dev build. The main window's only capability is `src-tauri/capabilities/default.json` (`"windows": ["main"]`). It grants `fs:allow-write-text-file`, `-write-file`, `-temp-write`, `-remove`, `-exists`, `-read-dir`, `-read-text-file`, `-mkdir` and `-rename`, and no `fs:default`. `theme-editor.json` grants no `fs` at all. In `tauri-plugin-fs` 2.5.2 (the version in `Cargo.lock`), the `stat` command is allowed only by `allow-stat` (`permissions/autogenerated/commands/stat.toml`) or by the sets that include it (`read-meta` in `permissions/read-meta.toml`, and `read-all`), and the main window has none of them. Each `allow-<command>` permission allows only its own command (`build.rs`'s command list), so `allow-read-dir` doesn't cover `stat`. Tauri's ACL therefore rejects `plugin:fs|stat` ("fs.stat not allowed"), and the scan's `try { stat } catch {}` swallows it. Every scanned file's `updatedAt` is unknown, as the survey read it. Nothing added (Core's `tree` scan replaces this in Task 5).
- **Tests seen failing first:** all 16 in `shared-projection-fixes.svelte.test.ts` (an in-memory `plugin-fs`, the managers wired as `UseDatabase` wires them over `TsLibrary`), each for its bug: no file write or the old file left (bug 1, including the pan's viewport being reset at reactivation), writes landing in `/repos/b` or ENOENT in a stale repo, `activeRepoId` kept, rows unshared by an unlinked or conflicted reconcile (bugs 3 and 5), and `activateLinkedRepo is not a function`. In Rust, `a_fast_forward_keeps_uncommitted_changes`, `a_refused_fast_forward_names_at_most_ten_paths` and `a_fast_forward_keeps_an_untracked_file_it_would_overwrite` failed (the pull succeeded over the edits). `a_fast_forward_without_local_changes_still_works` passed before and after, as a guard. `pull-error.test.ts` failed on the missing module. The existing reconcile fixtures (`dashboard-reconcile`, `dashboard-review`, `library-persistence`, `library-replay`) set `activeRepoId` with no linked project. They now link the project (a git path plus a registered repo, or a `repoForProject` stub) and mock a clean `getRepoStatus`. Their assertions are unchanged.
- **Review fixes (22:18–22:33).** These were seen failing first: the version test, both "waits for a write under way" tests, both "an edit answered after … started" tests (once their spy stored the edit at once and only answered late), the two-project import warning (`[2, 1]`), the two new Rust quoting tests and the four `pull-error.test.ts` cases. The M1 and case-insensitive tests already passed against the fixed code. I checked that each can fail by mutating the code: publishing the page's copy fails M1, and comparing raw names deletes the file on the case-insensitive disk.
  - **I1.** When the file changes widgets, description or date filter, `reconcilePatch` sends `captureVersion: true`, and `storeReconciled` splices the answer's version into the page. A failed write followed by an activation that takes the file therefore keeps the edit as a version (Q20's rule, ahead of Core). `dashboard_shared_file_failed` now says so.
  - **I2.** Publishes, unshare's delete and delete's file removal all run on one per-dashboard chain (`onFileChain`; `removeFile` for the deletes). Each unshare or delete bumps a per-dashboard `withdrawals` count when it starts, and each save reads that count when it's sent. A publish writes nothing if the count went up in between, and a delete waits for a write already under way. A first version used a `withdrawn` set that only `shareDashboardById` cleared. In the re-review (N1) it outlived a re-share from another window, so every later edit here skipped its file. The count fixes that, and the test `after another window shares it again, an edit here writes the file` covers it.
  - **M1.** The stored-row test now changes the description and viewport through the library behind the page's back.
  - **M2.** `publish` compares `dashboardNameToFilename` (lower case). To avoid an import cycle, `dashboard-file-parser.ts` now imports `stripWidgetRuntimeState` from `dashboard-serialize`. The test fs has a case-insensitive mode.
  - **M3.** `importFromGitRepo` reconciles the first project only once: through `setActive`, or directly when it's already active.
  - **M6.** Rust names each path as a JSON string (`"a", "b" and 2 more`). The sentence keeps its English prefix and ` first` suffix, so the existing tests pass. The TS `refusedPaths` parses the list, and the toast is the i18n `shared_pull_local_changes` / `_more` with `{files}` (and `{count}`). A list it can't read falls back to the message as sent. `serde_json` moved from seaquel-git's dev-dependencies to its dependencies.
  - **M7.** A comment at the fast-forward in `ops.rs` documents the window where the checkout succeeds but `set_target`/`set_head` fails. The tree and index then hold the fetched commit while the branch names the old one, so `git status` shows the pull as local edits. Nothing local is lost, and pulling again finishes. Behaviour is unchanged.
  - **Logs.** A failed file write or delete logs the error code, or the error's kind, never its message.
- **For Task 7 (M4, M5):** a repo whose status can't be read skips the reconcile silently, with only a log line and no toast. Deep links (`services/deep-link.ts`) still call `setActiveRepo`, so `activeRepoId` can name a repo other than the active project's until the next activation. Writes no longer read it, so this only affects the sync button's target.
- **Left as found:** pre-existing log lines in `shared-repo-manager.svelte.ts` and `dashboard-file-parser.ts` that name paths (Task 7 deletes both files). The orphaned old file after a refused rename followed by another rename in flight. Unshare restoring the file when the row write fails (Decision 37, Task 5).

## Task 2: Fixtures

**Files:** `crates/seaquel-workspace/tests/fixtures/shared/{README.md,changes.json,formats.json,projection.json}`, `…/imports/{README.md,changes.json,tableplus.json,dbeaver.json,plist/*.plist}`, and the recorder artifact.

**Run:** record twice, byte-identical; `git diff --stat` shows only the fixtures and the artifact; `mise exec -- npx vitest run src/lib/hooks/database src/lib/services` passes once the copy is deleted.

**Review:** every bug in "What the code shows" with a stored or file effect has a case; every format quirk in Decision 45 has a case; the README names the recorder's commit, the in-memory file system's rules (symlinks, unreadable directories, `stat`), the git stub, the normalising rules, and what can't be recorded (events, `seq`, the repo lock, real symlinks).

**Things this task could quietly skip:** cases on a beta-era file (dashboards without a project foreign key); the XML plist inputs; a step that pulls without activating (bug 12 is "nothing happens", which still has to be recorded); the toasts.

### Notes from Task 2 (as built)

2026-09-30, ~0.5 h (about 22:40–23:10).

- **What's there.** `shared/formats.json` has 147 cases: 31 query parses, 20 query writes, 12 dashboard parses, 4 dashboard writes, 18 template parses, 5 template writes, 6 project parses, 2 project writes, 4 label parses, 1 label write and 44 file names. `shared/projection.json` has 38 cases and 158 steps. `imports/tableplus.json` has 13 cases with 12 XML plists under `imports/plist/` (`unreadable.plist` is cut short on purpose), and `imports/dbeaver.json` has 12. `shared/changes.json` has 76 entries plus `*` (50 format, 26 projection), and `imports/changes.json` has 25 plus `*`. Each README lists every case.
- **The recorder** is `docs/plans/artifacts/2026-10-05-record-shared-fixtures.test.ts.txt`. It ran on `cfc7294` plus the working tree after Tasks 1 and 3. Run as the plan's "How" says, with `FREEZE_SHARED=1` (or `FREEZE_SHARED_OUT=<dir>`). The managers are wired as Task 1's `shared-projection-fixes` test wires them, over `TsLibrary`/`TsUi`. `page.open` is a copy of `initializeApp`'s projects, connections and `initializeSharedRepos`. The disk is in memory: `readDir` reports symlinks as tauri-plugin-fs 2.5.2 does (via `file_type`, neither file nor directory), every other call follows them, `stat` throws, and there are unreadable directories, failing writes and a case-insensitive mode. Git is a stub over a committed snapshot per repo, so `getRepoStatus`'s `pendingChanges` is real and the background refresh rescans exactly when today's would. Two runs to scratch directories were byte-identical (`diff -r`), and so was the frozen copy. The formatted, lint-clean artifact reproduces the frozen files.
- **Steps carry `core`**, the calls a GUI on Core would send. They are authored for the projection's own steps (`shared.sync`, `syncRepo`, `linkProject`, `unlinkProject`, `scan`, `importProjects`, `git.pull`/`git.commit` followed by `syncRepo`). For edits they are taken from the `TsLibrary` writes the page made, as `library` RPCs with `binds`. Each step also records `library` (every library call), `toasts` and a `view` that no replay compares.
- **changes.json gives concrete values.** Projection entries give whole `tree`/`rows` per step, plus `links` (0004 columns by row), `notices` (the plan's `SyncNotice` shape, paths relative to `.seaquel/`), `projection` and `outcome` where they matter. None has a `view`. The `*` rule covers Core ids (`binds`, then pattern ids; `<core:n>` for rows only Core makes) and Q22's id line or key, which must be present, a v4 uuid and kept across renames, and is removed before comparing. Decision 43 limits `shared_repos` to `id`/`path`/`name`, and `activeRepoId` keeps its seeded value (Decision 42). The import entries spell out Decision 47's answer for every case.
- **Chosen while writing the expected values.** Task 4b and 5 follow these; reopen them only with the owner. Q21: a reserved stem gets `-file` (`con-file`); marks (`\p{M}`) are kept along with letters and digits; NFC, lower case, NFC. Decision 45: `\r\n` becomes `\n` in the whole file, body included (the quoting rule was changed in the review: see below). Q22: parse results add the id as `fileId`, since today's `id` is the scan's `<repoId>:<path>`. Imports: `problem: "invalidPort"` with `port: 0`; an empty DBeaver file or a TablePlus plist that isn't a list is `unreadable`; `key` is TablePlus's `ID` as text or DBeaver's connection key.
- **Review fixes (2026-09-30, ~0.6 h).** I re-recorded the whole set: every existing case in all four files is byte-identical to the frozen copy and keeps its place, the new cases come after them, and two runs matched. The plists were decoded again with `plist` 1.10.1 (13 good, `unreadable` refused).
  - **Counts now:** `formats.json` 153; `projection.json` 50 cases, 214 steps; `tableplus.json` 15 (14 plists); `dbeaver.json` 13. `shared/changes.json` has 96 entries plus `*` (56 format, 40 projection); `imports/changes.json` has 28 plus `*`.
  - **C1 (Q22 ids):** `queries/renamed-in-the-repo-keeps-its-id` pairs by id across a rename, then falls back to the stored path once an older release drops the id (`shared_file_id` cleared), then to the name once the file moves. `links` checks `shared_file_id`. `queries/stored-path-beats-a-second-claim`: the stored path beats a slug-path file with the same name. The git stub now takes a queue of pulls.
  - **I1:** no expected `rows` lists `app_state` any more. `*` checks `activeRepoId` against the seed, so the six steps named, and every other expected step, follow it.
  - **I2:** `*` now has notices default to `[]` on every step with a `shared` call, the base/file-id/`shared_path` invariants (connections included, with the exemptions spelled out) and the byte-for-byte rule for untouched repo rows (M5). Links were added to `unlink/imported-connections` (`shared_dir` cleared) and `import/missing-project-yaml` (`ops`).
  - **I3 (Q23, Q27, Q28):** five template cases (changed here and in the repo; renamed in the repo; renamed after a local rename; type changed; a new template by pull), and the `R = B, F ≠ B` wording in `connections/templates-changed-in-the-repo`. Q27 and Q28 are in "Answered questions" and Decision 41. After a type change the same sync imports the template as a new connection, "Warehouse (2)" (Q29, owner's answer).
  - **I4:** a quoted value with `"` or `\` and no `'` or newline goes in single quotes; `\\` appears only inside double quotes (Decision 45). `write-query/backslash-bare` and `backslash-needing-quotes` changed; the recorder's reader model checks that every value round-trips.
  - **I5:** Decision 34 now defines both hashes (`write(parse(text))` against `write(parse(write(row)))`, also in `*`), with `write-query/crlf-body` and `queries/trailing-whitespace-and-crlf`. Today's reconcile stores the trimmed text over such a row, with a version.
  - **I6:** `repo/two-projects-one-repo-pull`. **I8:** a 250-byte stem cut and a `free_path` re-cut (`file-name/45`, `dashboards/long-cjk-names-collide`). **M2:** `PRN`, `COM0`, `LPT0` and `COM¹` (the reserved set gained `0` and the superscripts), and `queries/nfd-name-from-a-macos-teammate`. **M6:** `unlink/repo-still-used`, where today's unlink forgets a repo another project still uses.
  - **I7, M8:** `invalidPort` now also covers ports outside 0–65535. `noId` and `duplicateId` take position keys (now `pos:<n>`, with `id:<ID>` for the rest; see the re-review). New cases: `out-of-range-ports` (both readers) and `duplicate-ids`. `tableplus/entries-without-id`, `tableplus/ports` and `dbeaver/ports` were corrected, and Decision 47 says `importsCreate` refuses a key with a problem.
  - **M1:** a losing second claim is `unpaired` (with `claims`), not `nameTaken`, and `SyncNotice` changed to match. M3 (Q22 wording), M4 (`projectUpdate` publishes), M7 (README: no folder-move case) and M9 (Task 7 review) are done.
  - **Recorder change:** `core` for `page.open`/`setActive` now asks whether the project has a git path, not whether today's repo list holds its repo. This changed no existing recording, but it makes `unlink/repo-still-used` send the `sync` a GUI on Core would.
- **Re-review fixes (2026-09-30, ~0.4 h).** The set was recorded again. Every existing case is byte-identical and in place, except `repo/two-projects-one-repo-pull`, which was meant to change (I2). Two runs matched. The plists didn't change. Counts now: `projection.json` 51 cases, 221 steps; `shared/changes.json` 97 entries plus `*` (56 format, 41 projection); the import files are unchanged in count.
  - **I1.** The sweep found 6 page-only writes: a `projectSidebarSet` from a project's first activation, in a step whose `core` doesn't send it. Two were already handled (`templates-changed-in-the-repo`, and `template-type-changed`, where Core's own import makes the row). Four were fixed (the three named, and the new M5 case): `withoutPageOrder` drops `project_state` from those expected rows. The other `setProjectSidebar` writes in authored steps belong to the projection (link, unlink, imports), so Core makes them.
  - **I2.** `repo/two-projects-one-repo-pull` activates "Ops" first, so the pull is `R = B, F ≠ B` for both projects with no notice.
  - **I3.** `*` (4b) now says positively that every shared row, and every explicitly shared connection, in a linked project has a non-NULL `shared_path`, except the step's `pathless` list. `pathless` is set in `dashboards/file-does-not-parse`, `repo/unreadable-directory` and `repo/symlinked-queries-directory`.
  - **M1.** Core's pull and push write `lastSyncAt` (Decision 43; `repoUpdate` no longer takes it). `*` (5) defines a repo call, and `repo/two-projects-one-repo-pull` expects the spliced `"lastSyncAt":"<now>"`.
  - **M2.** The README says failing writes need Task 5's per-path hook.
  - **M3.** `TemplateTypeChanged` carries `kind` in the plan, matching the fixtures.
  - **M4.** TablePlus keys are `id:<ID>` and `pos:<n>` (Decision 47).
  - **M5.** Decision 53, and the new case `connections/never-shared-in-a-linked-project`: today an edit writes `billing.yaml`; Core writes nothing. `link/first-link-exports-templates` now expects the link to export nothing, since `c1` was never explicitly shared.
- **Q30 (2026-10-01, ~0.2 h).** The link call now carries `share`, the dialog's ticked ids. Only the `core` of the two `link/` cases changed (`share: ["c1"]` and `share: []`); their other recorded fields and every other case are byte-identical, and two runs matched. `link/first-link-exports-templates` again expects `c1` exported and linked, since it was ticked. New case `link/unticked-connection-stays-local`: "Billing" left unticked gets no template, stays unlinked, isn't re-imported, and a later edit publishes nothing. Today the link writes both templates and the settings save imports both again. Counts now: `projection.json` 52 cases, 225 steps; `shared/changes.json` 98 entries plus `*`. `connections/never-shared-in-a-linked-project` is unchanged.
- **Bugs recorded:** 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16 and 17, each with a case (the README maps them). Bug 2 needs libgit2 (pinned by Task 1's `seaquel-git` tests), and 18, 23, 24 and 25 have no stored effect one page can show. Every Decision 45 quirk has format cases (CRLF and BOM, both quote styles, `''`, backslashes, commas in lists, newlines). A beta-era case, the XML plists, a pull with no activation and the toasts are all there.
- **Found while recording:**
  - The scan cache is read only at startup, after a pull and by the five-minute background refresh. So a query shared and then left before the next refresh gets unshared by the activation's reconcile (`queries/share-then-switch-before-refresh`; its file stays). Core has no cache, so the fix comes with it.
  - A reconciled query's `NAME_TAKEN` toast says "a project called" for a saved query (`queries/strasse`).
  - Two files naming one query trigger one update, not two: the reconcile keeps the last file listed.
- **For Tasks 4a, 4b and 5:**
  - The Core replay must lay out real symlinks, `git init` each repo with the seed committed, and play a case's `pull` through a clone. Compare case-insensitive renames by path, not by the disk: a case-only rename keeps the stored path (`queries/case-only-rename-on-a-case-insensitive-disk`).
  - `import/missing-project-yaml` pins that importing shared projects writes no file. The link writes `project.yaml` only when it's missing (`link/first-link-exports-templates`, where today's export writes it too).
  - A sync after open records bases. So in the pull cases (`dashboards/teammate-change-then-pull`, `repo/pull-without-activation`, `connections/templates-changed-in-the-repo`) the teammate's change is `R = B, F ≠ B` and gives no conflict notice. Only `queries/reconcile-new-changed-removed` and `queries/two-files-one-name` (no base, `R ≠ F`) and `queries/changed-here-and-in-the-repo` (Q20) list `conflict`.
  - Secrets: the recording has no keychain. After each step the Core replay's store holds the case's `secrets` minus the entries of connections no longer in the rows.

## Task 3: Storage

**Files:**
- `crates/seaquel-storage/migrations/0004_shared_links.sql` (Decision 33) and the migrations README.
- `src/queries/shared_repos.rs`: `list`, `get`, `get_by_path`, `insert`, `update_json` (the named fields only, the rest byte for byte), `delete`, each write on `&mut WriteTx`; `save_all`/`load_all` stay for the frozen fixture.
- `projects::set_shared_dir`; `saved_queries::{set_link, by_shared_path}`; `dashboards::{set_link, by_shared_path}`; `connections::set_link`. The `Persisted*` reads carry `shared_path`.
- `seaquel-types/src/storage.rs`: `sharedPath` on `PersistedSavedQuery` and `PersistedDashboard` (optional).
- Tests: `tests/shared.rs` (new), `tests/baseline.rs` (`0004` on every frozen release, beta-era included; `MIGRATION_COLUMNS` gains the new columns).

**Tests first:**
- `migration_0004_applies_on_every_release_schema`, `a_read_only_open_refuses_a_file_with_0004_pending`;
- `update_json_keeps_unknown_fields_byte_for_byte`, `register_is_idempotent_by_path`;
- `set_link_and_by_shared_path_use_the_index` (`EXPLAIN QUERY PLAN`);
- `the_5d_replays_still_pass` with the new columns dropped after checking.

**Run:** `cargo test -p seaquel-storage`; CI clippy.

**Review:** expand-only; nothing written outside a `WriteTx`; the frozen fixtures untouched.

**Things this task could quietly skip:** the beta-era baseline; the read-only refusal; the 5d library and state replays, which dump every column.

### Notes from Task 3 (as built)

- **Migration.** `crates/seaquel-storage/migrations/0004_shared_links.sql` is Decision 33 exactly: nine nullable columns with no default and the two `(project_id, shared_path)` indexes, no data step. The migrations README has its entry. It applies on every frozen release schema, the beta-era one included (`tests/shared.rs`), and `tests/common/mod.rs`' `MIGRATION_COLUMNS` lists the nine columns for the beta.1 data test. The two `[1, 2, 3]` asserts in `tests/state.rs` now expect `[1, 2, 3, 4]`.
- **One link type.** `seaquel_storage::SharedLink { path, base, file_id }` (all `Option<String>`, `None` is NULL) and `RowLink { id, link }` (`src/queries/mod.rs`). `SharedLink`'s `Debug` says only which parts are set. Every table uses it; for a connection `path` **is `shared_connection_id`**, stored as given (`<repoId>:<path>`), so `connections::set_link` writes `shared_connection_id`, `shared_base` and `shared_file_id` together and Core can unlink a removed template in one call. `set_link` always writes all three columns (`None` clears one); Core reads the link first when it changes only the base.
- **Writes** (all `&mut WriteTx`, `false` for a missing row): `saved_queries::set_link`, `dashboards::set_link`, `connections::set_link`, `projects::set_shared_dir(tx, id, Option<&str>)`, and `shared_repos::{insert, update_json, delete}`.
- **Reads** (`impl Into<Reader>`, so inside a write through `&mut tx`): `{saved_queries, dashboards, connections}::links(r, project_id)` (every row of the project, linked or not, rowid order), `{saved_queries, dashboards}::by_shared_path(r, project_id, path)` (ids, exact match, rowid order; a `Vec` because nothing stops a hand-edited file from holding two), `projects::shared_dir(r, id)` (`None` for no dir or no project), `projects::ids_with_repo_path(r, path)` (for "forget the repo when no project uses it"), and `shared_repos::{list, get, get_by_path}`. Each statement is a `pub const` (`SET_LINK`, `LINKS`, `BY_SHARED_PATH`, `SET_SHARED_DIR`), checked with `EXPLAIN QUERY PLAN` for no scan and no sort.
- **`sharedPath` on the rows.** `PersistedSavedQuery` and `PersistedDashboard` gained `shared_path: Option<String>` (`sharedPath?: string` in TS, absent when NULL). It is **read only**: `insert`, `update` and the frozen `save_all`/`save` never write it (the read column lists are separate from the write ones), so a library patch can't move a link. Drafts build it as `None` (`saved_query_from_draft`, `dashboard_from_draft`).
- **The repo list.** `shared_repos::insert` requires an object with a non-empty string `id` and fails on an id that exists. `get_by_path` compares the JSON `path` exactly and reads the whole list (a handful of rows; no expression index, since `json_extract` on an older release's malformed row would fail its insert). No unique path constraint either, for the same reason: registration is idempotent because Core does `get_by_path` and `insert` in one `WriteTx` and writers queue (`register_is_idempotent_by_path` races two). `update_json(tx, id, &[(field, &RawValue)])` splices: it reads the stored values as borrowed `RawValue`s (slices of the stored text), replaces only those bytes, and appends missing fields before the closing brace, so whitespace, key order, number spellings, escapes and unknown fields stay byte for byte; a repeated key has its last occurrence replaced (the one `JSON.parse` reads); a stored value that isn't an object is a decode error and nothing is written. There is no "remove a field": every repo field is cleared by setting `null`. None of the targeted calls touches `activeRepoId`. `load_all` now reads through `list` (rows that aren't UTF-8 are skipped instead of failing); `save_all` is unchanged and stays for the frozen fixture.
- **Older releases.** They never read the new columns; their whole-row inserts and `ON CONFLICT DO UPDATE` upserts name their own columns, so a link they don't know stays (`an_older_releases_writes_still_work_after_0004`). An older release renaming a linked query leaves `shared_path` pointing at the old file, which is Decision 52's "row changed, base didn't" case.
- **The 5d replays.** `crates/seaquel-core/tests/common/mod.rs` has `drop_link_columns`, called from the library and state replays' snapshots: it asserts each new column is present and NULL, then removes it. Seen failing first (`replays_every_fixture`) before it was added.
- **Review fixes.** The older-release test now runs the real frozen writers (`saved_queries::save_all`, `dashboards::save`, `connections::save`, `projects::save_all`) over linked rows with `sharedPath` dropped: all nine columns survive, and a connection's `shared_connection_id` is whatever the older copy held. It passed on first run, so no frozen writer clears a link and none changed. `get_by_path` reads a repeated `"path"` key as `JSON.parse` does (last wins). `update_json` refuses `"id"` (encode error, nothing written). Its doc now says that a row stored as `null`, a key with a lone surrogate escape, and nesting past 128 levels are decode errors, though `get`/`list` treat a `null` row as absent.
- **For Task 5.** Paths are compared exactly by storage; normalising a repo path (trailing `/`, symlinks, case) before `get_by_path`/`ids_with_repo_path` and checking a row's path (Decision 32) are Core's. A sync that creates a row inserts it and then calls `set_link` in the same `WriteTx`. The `sharedRepo` event kind and the storage group's `sharedReposLoadAll`/`sharedReposSaveAll` removal are Task 5/6's; the storage functions are still there for the frozen replay. `seaquel-cli mcp` now refuses a file until the app has opened it once (Task 9 records it).

## Task 4a: Domain, imports

**Files:** `crates/seaquel-workspace/src/imports.rs`, `tests/imports_plan.rs`.

**Tests first:** `replays_every_import_case` with `changes.json` exactly; `parse_int_matches_javascripts` (leading digits, signs, whitespace, `1e3`, empty); `a_port_that_isnt_a_number_is_a_problem`; `duplicates_compare_the_five_fields`; `never_panics` (proptest over JSON values and bytes); `debug_shows_no_host_name_or_user`.

**Run:** `cargo test -p seaquel-workspace`; wasm32 clippy of the pure crates.

**Things this task could quietly skip:** TablePlus values that are numbers or booleans where the TS took strings (`toTablePlusConnection`'s `str`); DBeaver's provider in another case.

### Notes from Task 4a (as built)

2026-10-01, ~0.5 h of work (about 00:05–00:40), plus about 40 min of waiting on builds of the shared target and on Task 4b's in-progress `shared` module, which kept the lib from compiling for a while.

- **What's there.** `crates/seaquel-workspace/src/imports.rs`: `ImportSource`, `ImportProblem` (`as_str`), `ImportCandidate`, `ImportCandidates` (`not_found`, `unreadable`, `from_result`; the `importsCandidates` answer), `ImportError` (`NotJson`, `NotAList`, `NotAnObject`, messages naming no path and quoting nothing), `ConnectionIdentity` (`of` a `PersistedConnection`, `of_candidate`), `tableplus_candidates`, `dbeaver_candidates`, `mark_duplicates`, `refused_key`, `parse_int` and `default_path(source, os)` (today's relative paths per OS). Wire types derive `ts` and are exported. `ryu-js` joined the dependencies (JavaScript's `String(n)`), and `plist` the dev-dependencies.
- **Signatures that differ from the plan's sketch.** `tableplus_candidates` returns `Result`, since a plist that isn't a list is `unreadable` (`tableplus/not-a-list`). `port` is a `u16`: after the range check it always fits, and it serialises as the integer the fixtures hold.
- **Tests** (`tests/imports_plan.rs`, 11 at first and 16 after the review fixes, plus 3 unit tests in the module): the six the plan lists, and `plists_decode_to_the_recorded_json` (the 13 decodable plists give exactly the recorded JSON, both through text as `src-tauri` did and through `serde_json::to_value` as Core will; `unreadable.plist` fails), `refused_keys_are_the_ones_with_problems`, `tableplus_values_read_like_the_typescripts`, `dbeaver_reads_like_the_typescript` and `default_paths_are_todays`. There is no proptest in the tree, so `never_panics` uses a seeded xorshift generator (20,000 rounds of random JSON values, entry lists and DBeaver documents, each also cut short and with a byte flipped), plus 1,000-deep arrays, 100,000-deep brackets and a million-digit port.
- **JavaScript's coercions, ported where the TypeScript leaned on them.** TablePlus: `str()` (a number's `String()`, `1e21` as `1e+21`, `-0` as `0`; a boolean's text; `""` for anything else), `Number(tLSMode)` (whitespace trimmed, `""` and `[]` are 0, `true` is 1, `0x`/`0o`/`0b`, a one-item array read through its text), `isOverSSH`/`isUsePrivateKey` only when exactly `true`, and `parseInt(port) || 22` for the tunnel. DBeaver: truthiness for each `|| default`, `String()` of non-text values (an array joined with `,`, an object `[object Object]`), the provider in lower case (Unicode, so a Kelvin sign reads as `k`), and **`Object.entries` order**: array-index keys first, ascending, then the rest as written, and a repeated key keeps its first place and last value. `serde_json` without `preserve_order` sorts keys, so the top level and `connections` are read with an order-keeping visitor over `RawValue`s.
- **Choices the fixtures don't pin:**
  - An `ID` whose text is empty (`""`, or an array or dict) is `noId`. `duplicateId` counts every dict entry, unsupported drivers included, so a Redis entry sharing a Postgres entry's `ID` makes the Postgres one `duplicateId`.
  - A candidate whose port was invalid matches no saved connection, even when an id problem is the one it reports (today's `NaN` matched none).
  - The TablePlus name fallback keeps today's text, `db:NaN` included.
  - DBeaver: JSON that isn't an object (`null`, `[]`, a string) is `unreadable` (`NotAnObject`); today `null` threw and the others gave an empty list. `connections` that is missing or a scalar gives no candidates; an array is read by index, as `Object.entries` does. A `connections` object or array that doesn't decode (a key with a lone surrogate escape) is unreadable (`NotJson`), never an empty list (review M1). Only `provider`, `name` and `configuration`'s `host`, `port`, `database` and `user` are decoded, each on its own (review M2): a number past f64's range or deep nesting anywhere else keeps the connection, as in JavaScript; such a number in a read field is `Infinity` (so `1e400` as a port is `invalidPort` and as a name is `Infinity`); a read field that can't be decoded at all is absent. A connection whose own keys don't decode, or whose `provider` isn't text (today it threw and the whole discovery failed), is left out.
- **Review fixes (2026-10-01, ~0.4 h).** Test-first: `an_ssh_port_out_of_range_is_a_problem`, `a_dbeaver_connection_without_a_name_is_a_problem`, `undecodable_connections_are_unreadable` and `fields_that_arent_read_never_drop_a_connection` failed before the change. `prototype_names_are_unsupported` passed at once (the behaviour was already there; it pins M3's first item). Two older expectations changed on purpose: the `-5` tunnel port in `tableplus_values_read_like_the_typescripts` is now `invalidSshPort` with port 0, and in `never_panics` a connection whose `configuration` is too deep to decode is now kept (no fields, `noName`) instead of dropped. `never_panics` also splices key escapes, lone surrogates and out-of-range numbers into keys, names, providers and configuration fields. The imports README gained "Differences no case records" (prototype-named drivers and providers, `String()` of non-text DBeaver fields, a non-object top level, undecodable `connections`, the fields decoded), and `changes.json`'s `*` describes the two new problems; no case's `expected` changed.
- **For Task 5:**
  - `importsCreate` must check a candidate against the connections imported earlier in the same call too: `ConnectionIdentity::of_candidate(new_id, &c)` (the TypeScript re-read the page's connections before each create).
  - The two shapes that used to pass as candidates but fail `connectionCreate` are now problems (coordinator's decision after the review, in Decision 47): `invalidSshPort` for a TablePlus tunnel port outside 0–65535 (tunnel `port` 0), and `noName` for a DBeaver connection with a missing or blank name. Both come after `invalidPort`, and `refused_key` refuses them like the others. No recorded case has either; `imports_plan.rs` pins them.
  - **Cap the size of the file Core reads** before decoding it: the readers are linear, but a huge `data-sources.json` or plist is read whole into memory.
  - **Watch the nesting depth in the plist decode and the `to_value` step.** `plist` has no depth limit that I know of, and a deep `serde_json::Value` recurses on drop (and in `to_value`), so a hostile plist could overflow the stack before `tableplus_candidates` sees it. Bound the depth (or refuse past one) before converting. The DBeaver side is safe: serde's raw values skip deep nesting without recursion and only the read fields are decoded.
  - `ImportCandidates::unreadable` takes Core's own message for a read or plist decode failure; it must not carry the `plist` or `io` error text, which can name the path.
  - `refused_key` ignores keys no candidate has; the create reports those itself.

## Task 4b: Domain, shared

**Files:** `crates/seaquel-workspace/src/shared.rs` (or a `shared/` module with `format`, `names`, `plan`), `tests/{shared_formats,shared_plan}.rs`.

**Tests first:**
- `replays_every_format_case`, `replays_every_projection_plan` (with `changes.json` exactly);
- `parse_write_round_trips_every_value` (proptest: names, descriptions, tags, parameter defaults with quotes, commas, newlines, Unicode);
- `older_readers_read_what_we_write`: a TypeScript-equivalent of today's reader, ported into the test, reads every value Core writes back to the same string except `\n` in descriptions;
- `file_stem_avoids_reserved_names`, `free_path_is_case_insensitive`;
- `the_rule_table` (one test per row of Decision 34), `each_file_pairs_with_one_row`, `a_skipped_file_is_never_missing`;
- `canonical_hash_ignores_formatting` (an older release's file and Core's hash the same);
- `plans_never_panic` (proptest over scans and rows), `debug_shows_no_names_paths_or_text`.

**Run:** `cargo test -p seaquel-workspace`; wasm32 clippy.

**Review:** the rule table matches Decision 34 and the owner's answers to Q20–Q25; no I/O; the format writer and reader changed together.

**Things this task could quietly skip:** a file whose name field is empty; a query folder from a file path deeper than the stored folder; the template's credential stripping on read.

### Notes from Task 4b (as built)

2026-10-01, ~2.1 h (about 23:45–01:55), about half of it builds waiting on the shared target.

- **Where it is.** `crates/seaquel-workspace/src/shared/`: `mod.rs` (`content_hash`, the byte caps), `yaml.rs` (Decision 45's line YAML), `json.rs` (JavaScript's `JSON.parse`/`stringify` with key order kept, `ryu-js` numbers, the widget run-state strip), `format.rs` (the five formats), `names.rs` (`file_stem`, `legacy_stem`, `free_path`, `path_key`, `TakenPaths`, `check_rel_path`, `check_folder`), `plan.rs` (`plan_sync`, `plan_publish`, `pick_project_dir`, `row_hash`, `NoticeMemory`). New dependencies: `sha2` 0.10, `unicode-normalization`, `unicode-properties` (general categories only), all already in the lock and pure; `parse_int` is 4a's (`imports::parse_int`). No ts-rs exports yet: `SyncNotice`, `Kind`, `SkipReason`, `ReplacedValues` and `PublishOutcome` serialize as the fixtures write them, and Task 6 decides what crosses the wire.
- **The API differs from the sketch** in "The wire and the API":
  - `plan_sync(link, scan, rows, limits: &Limits, ids: &mut dyn IdSource)` (`Limits { library, state }`, the limits Core was built with; review C1). There's no `ConflictRule`, since Q20 left one rule. The `IdSource` hands out row ids (`saved-`/`dashboard-`/`conn-` plus a uuid) and file ids, so the planner never generates them itself.
  - `SyncPlan { conflicted, rows, links, files, notices }`. `links` replaces `bases`: each `LinkUpdate` is the row's whole new link (path, base, file id), and `pending_on` names the write it waits for.
  - `RowOp` variants: `CreateQuery`/`UpdateQuery`, `CreateDashboard`/`UpdateDashboard` (with `capture_version`), `CreateConnection` (`rename_if_taken`, local-only off, linked; Core appends it to the order) / `UpdateConnection`, and `Unshare { kind, id }` (a connection becomes local-only).
  - `plan_publish(link, &RowChange, &PublishContext { taken, existing }, ids) -> Result<PublishPlan, LibraryError>`. `RowChange` is `Query`/`Dashboard`/`Connection { row: Option<&row>, link, renamed, shared_now? }` or `Project { name }`. `existing` is the text at the stored path (or `project.yaml`), so a rewrite keeps the file's id and a template's labels, and a project rename keeps `project.yaml`'s description. `PublishPlan { files, on_success, on_failure }`.
  - `SharedRows` holds every row of the project, shared or not; an unshared row only takes a name (`NameTaken`).
- **Order for Core** (doc on `plan.rs`):
  - Sync: write `rows` and the links without `pending_on` in one transaction. Then write `files`, each independent of the others. Then store each pending link whose write succeeded. A sync never deletes a file and never renames one.
  - Publish: `files` is a sequence (a rename is write-new, then delete-old). Stop at the first failure. A failed write stores `on_failure`; a refusal (a symlink on the path) stores nothing. Otherwise store `on_success`.
  - `on_failure` is set only for a first share. It stores the path with no base, so the next sync writes the file (`*` (4a)'s "the share toggle stores the link even when the template write fails"). The replay applies it to queries and dashboards too, and nothing contradicts that. `repo/symlinked-queries-directory` expects `pathless`, which only works because a symlink is a refusal, not a failed write. **Task 5 must keep that distinction.**
- **Rules chosen where the plan was open** (each pinned by a test):
  - A query file whose `name:` is empty or blank takes its file name, as one without frontmatter does. A dashboard's blank name does the same. A template without a name doesn't parse. A dashboard whose `name` is a non-empty non-string doesn't parse.
  - `id:` / `"id"` is read into `file_id` when non-empty. The id pairs only when no other file in the scan has it: a copied file pairs by path or name instead, and the new row doesn't take the duplicated id.
  - Pairing goes id → stored path → name (`name_key`, and for queries the folder) → slug path. "Slug path" means both Q21's stem and today's `nameToFilename`, since older rows have today's. A file at a slug path wins a tie among name claims. A free file that names a row already paired is `Unpaired`, and is never imported.
  - Removal: a shared row with no file is `RemovedInRepo` and unshared when it has a base, or when it has neither path nor base (an older release's row; `queries/reconcile-new-changed-removed`). With a path and no base it was never written, so the sync writes it (at its path, or at a free one if a file took it).
  - "A skipped file is never missing": a row whose stored or slug path is a skipped path, or lies under one, is left alone. So is any row without a stored path while anything of its kind was skipped (it could be that file).
  - A query's folder follows its file. When a pair's folder differs, even with equal content, `UpdateQuery { folder }` goes out (if the library can hold the name in that folder). On a rename, `plan_publish` keeps the stored file's own directory, even one deeper than the row's folder, unless the row's folder differs from the stored file's: then the file moves to `queries/<row.folder>` (review I1).
  - Values in double quotes escape `"` and a newline, and a `\` only where Core's reader would misread it: before `"`, `\`, `n` or a newline, or at the end (review flag 3; the newline is my addition, since a newline is written `\n`). Decision 45 is reworded to match.
  - **A name another row holds is withheld** (review flag 4, replacing the first pass's "(2)" special case): an update from a file keeps the row's name and takes the rest, the base becomes the row's own hash after that partial patch, and `NameTaken { path, takenBy }` names the file (once per session). Names are checked against the plan's final state (renames, folder moves and creates earlier in the same plan), so a two-row swap is refused for both. A template imported under a taken name gets a free one ("Warehouse (2)") with the same withheld base; once the name is free, the next sync gives it, and nothing ever writes the "(2)" into the template.
  - Notices: `NoticeMemory` (one per Core workspace) drops a notice whose path was already named this session. `Conflict` and `RemovedInRepo` name rows, not files, and always pass.
- **Replay scope** (`tests/shared_plan/replay.rs`, top doc): each case runs on an in-memory model of Core's rows and links, and of the disk (symlinks, unreadable directories, failing paths, a case-insensitive mode, scripted pulls and conflicts). Each `core` call is played as Task 5's Core will play it, with every decision taken by `shared`:
  - `sync`/`syncRepo`: scan the model's disk, then `plan_sync`;
  - library writes: the library's own patch functions, then `plan_publish`;
  - `linkProject`, `unlinkProject` and `importProjects`: `pick_project_dir`, `plan_publish` and `plan_sync`.

  After every step it compares, exactly and with `changes.json` applied:
  - the rows of every table but `app_state`, versions included (`shared_repos` by `id`/`path`/`name`; ids bound to uuids consistently per case);
  - the whole tree (Core's id line or key checked as a v4 uuid that stays with its row, then removed; files Core didn't write byte for byte);
  - the step's `links`, its notices (exactly, as a set, on every step with a shared call, after the once-per-session rule), and `projection`/`outcome` where listed;
  - `*` (4a), each linked row's base and file id against its file, and (4b), each shared row's path.

  It doesn't model events, `seq`, the lock, real symlinks, git or the keychain. 52 cases, 225 steps, all pass.
- **Tests seen failing first:**
  - The format tests failed to compile before `shared` existed. The first run against the implementation then failed `replays_every_format_case` (2 dashboard writes: the harness built its arguments through `serde_json::Value`, which sorts keys, so the test now reads them as raw JSON), `older_readers_read_what_we_write` (the test's model of when Core escapes was too narrow) and `parse_write_round_trips_every_value` (the generator inserted a newline inside a character).
  - The planner tests were written against the placeholder module before the planner, but first run after it. That run failed `publish_writes_moves_and_deletes` (my test expected a link update for a removed row, which has none left) and the projection replay, with 30 differences:
    - the harness's own key ordering, and then the pretty-printed fixture's whitespace in dashboard JSON (fixed by passing the calls' params as raw, minified JSON);
    - the `(2)` template (a planner fix, above);
    - the (4b) scope (above).
  - To show the planner tests can fail, a mutation run stubbed `plan_sync`, `plan_publish` and `pick_project_dir` to do nothing. 18 of the 22 tests then failed. The 4 that passed are guards: nothing changed, never panics, `Debug`, the notice memory.
- **For Task 5:**
  - Use the order above.
  - `on_failure` is only for a failed write, never for a refusal.
  - Store `shared_dir` at the first sync of a project whose column is NULL (the replay does it). A `projectUpdate` that renames a linked project must store it before the rename, or the slug of the new name wins.
  - `CreateConnection` appends to the order. With no `project_state` row, the order starts as the project's existing connections in id order, which is what `connections/template-type-changed` expects (`["c1", new]`). Unlink writes the order without the removed connections (`[]` included).
  - The link registers the repo under the project's name. The import registers it under the imported project's final name.
  - Run every sync's notices through one `NoticeMemory` per workspace.
  - The planner checks names against every row it is given, so `SharedRows` must hold all of the project's rows (shared or not), and Core must apply the row ops in the plan's order (a rename that frees a name comes before the op that takes it).
  - `template_path` reads `<repoId>:<path>` at the first `:.seaquel/`.

### Review fixes for Task 4b (as built)

2026-10-01, ~1.2 h (about 02:00–03:10, a good part of it builds), with the review's findings and the coordinator's rulings.

- **Flag 1.** `changes.json`: `queries/renamed-in-the-repo-keeps-its-id` steps 1–5 now expect `q1.tags` `"[]"` (one line added to its why), with an entry in the fixtures README's Corrections. `known_difference` and its assert are gone; the replay holds no tolerance.
- **Flag 2.** `*` (4b) covers projects whose directory is stored (`projects.shared_dir`, stored at the first sync, link or import), in `changes.json`, the README and the replay (which keys it on its stored directories).
- **Flag 3, I3.** See the rules above. `database:`, a parameter's `type:`, a template's `type:` and `sslMode:` go through `write_value`, ports are written as whole numbers. That changes one recorded format case, `write-query/database-with-colon` (`database: "pg:15"`), so it has a new `changes.json` entry (99 entries now, 57 formats) and a README correction. The round-trip test now generates those fields, and `older_readers_read_what_we_write` exempts exactly the values with an escaped `"` or a misread-prone `\`.
- **Flag 4, I2.** See the rules above. `*` (4a) now reads "unless a value was withheld" (`changes.json`, README). The replay checks the exception exactly: the base is the row's hash and the row under the file's name hashes as the file.
- **C1.** Each file is checked as the draft it would make (`check_saved_query_draft`, `check_dashboard_draft`, `check_connection_draft`, which covers `check_type` and `check_port`), and each planned update with the patch check, under `Limits`. A refused file is `Skipped { why: invalid }` (new `SkipReason::Invalid`); nothing changes on either side, and its row stays out of pairing and can't be removed. The replay runs the same checks on every op it applies.
- **I4.** A repeated key is found through a `HashMap` in the JSON reader and the template reader.
- **I5.** A row whose stored or slug path was skipped stays out of pairing entirely; a free file naming it is `Unpaired` with `claims`.
- **M1.** `FileOp::Write { expect_hash }`: the scan's hash for a sync's local-change write, the row's base for a publish at its stored path, `None` for a new path. The replay models Core's side: a stale write writes nothing, Core syncs the project instead (the file wins, the row's text is kept as a version), and the publish answers `failed` with code `FILE_CHANGED` (a proposal for Task 5).
- **M2.** `PublishOutcome`'s `Debug` shows whether there is a message, not the message.
- **M4.** Keys, path keys and slug keys are computed once per file and row, pairing uses indexes (id, path, name, slug), skip checks walk a path's ancestors in a `HashSet`, and `TakenPaths` is built once. 20,000 files and rows plan in about a second in a debug build.
- **M5.** Decision 34's table now says it: write for a path-set, base-NULL row; unshare a row with neither; withheld names; `invalid` files.
- **M6.** A connection whose `shared_connection_id` names another repo isn't paired or removed (done here, cheaply, from the id's repo part).
- **M7.** `float_roundtrip` would leak: `cargo tree -e features -i serde_json` shows nothing enables it, and every crate that depends on `seaquel-workspace` (Core, the server, the CLI, `src-tauri`) would get it through feature unification. So the JSON reader parses numbers itself: serde checks the text and splits arrays and objects into raw members, and each number's text goes through Rust's correctly rounded `f64` parser (`1e400` is infinite, as in `JSON.parse`, and `stringify` writes `null`). No serde_json feature changed, so the engine replays weren't rerun.
- **Tests seen failing first** (run before the implementation):
  - `shared_formats`: 6 failed, `numbers_parse_as_javascript_reads_them`, `a_backslash_is_escaped_only_where_the_reader_would_misread_it`, `engine_names_types_and_ports_are_written_as_values`, `parse_write_round_trips_every_value`, `older_readers_read_what_we_write`, `replays_every_format_case`; `a_million_keys_parse_quickly` hung past 60 s (the quadratic lookup) and was stopped.
  - `shared_plan` didn't compile (`Limits`, `SkipReason::Invalid`, `Write { expect_hash }`, the new `plan_sync` argument). Once the code was in, every new test passed but `a_large_project_plans_quickly`, whose expected count of link updates was mine and wrong (rows already linked as they are get none); it ran in about a second.
- **New tests:** `a_file_the_library_would_refuse_is_skipped` (a bogus parameter type, duplicate parameter names, a NUL in a name, an over-limit name under web-like limits, a viewport that isn't an object, `type: oracle`, ports 70000 and -5), `a_row_at_a_skipped_path_is_left_out_of_pairing`, `a_taken_name_is_withheld_from_an_update`, `a_link_to_another_repo_is_ignored`, `a_large_project_plans_quickly`, and, in the replay, `an_imported_template_takes_its_name_once_it_is_free` (continues `connections/template-type-changed`: two more syncs while "c1" holds the name, then "c1" removed) and `a_publish_never_overwrites_a_teammates_change`. `publish_writes_moves_and_deletes` gained the folder move (I1) and the `expect_hash` checks.
- **For Task 5, in addition:**
  - Pass the limits Core was built with to `plan_sync`.
  - Before a `Write` with `expect_hash`, hash the file on disk (`content_hash` of the kind's `*_content` of the parsed text); if it differs, don't write and sync that project instead.
  - The library call checks a query's folder (Decision 32, `check_folder`) before it commits, so a publish never meets a folder it refuses (M3).
  - `LinkedConnection`s of another repo are already ignored by the planner; Core needn't filter them (M6).

### Re-review fixes for Task 4b (as built)

2026-10-01, ~0.5 h (about 03:15–03:45, most of it builds).

- **R1.** The JSON reader stops at 128 levels (`MAX_DEPTH`, serde_json's own default): past it a value doesn't parse, so nothing recurses further and the per-level re-scan stays bounded by 128 × the text. Before the fix, 100,000 levels aborted the test process with a stack overflow on a 2 MiB thread. Every other recursive walk in `shared` is the JSON writer and the widget strip, which only see parsed values, so they are bounded by the same limit; the YAML readers are line loops with no recursion. Tests on 2 MiB threads: arrays and objects 100,000 deep inside `widgets`, through the reader, both writers, `file_hash` and `plan_sync`.
- **R2.** F′ is the file's hash under the row's name. A base equal to F′ means the last sync withheld the file's name. Then `R ≠ B` is a local change: the sync writes the row's content under the file's name (`expect_hash` = F), so the teammate's name stays in the repo, and the base becomes R. `plan_publish` does the same for a write in place, and a delete in that state expects F too. A rename of a withheld row isn't in place: it writes the new name and the next sync applies Q20.
- **Minor:**
  - A connection whose template path isn't in this project's `connections/` (another directory of the same repo, bug 7's damage) is neither paired nor removed.
  - `expect_hash: None` means "no file may be there", for writes and deletes alike. A first link writing `project.yaml` expects none; a project rename expects the file as read.
  - `shared::file_hash(rel_path, text)` is public (queries, dashboards, templates and `project.yaml`), and the replay uses it.
  - The name registry keeps every holder of a name in a sorted set. `NameTaken` names the first by id, whatever the rows' order.
  - The order doc says Core applies `rows` in plan order.
- **Seen failing first:**
  - The deep-nesting test aborted on a stack overflow (run with its one `file_hash` line commented out, since the function didn't exist yet).
  - The planner tests didn't compile, so I ran mutations. With the withheld-state check switched off, `an_edit_while_a_name_is_withheld_stays_local` failed. With the directory filter dropped and the last holder named, `a_link_to_another_directory_is_ignored` and `every_holder_of_a_name_is_kept` failed.

## Task 5: Core and the file tree

**Files:**
- `crates/seaquel-git/src/tree.rs` (new): `scan`, `apply`, Decision 32's rules, atomic writes; `ops.rs`: nothing beyond Task 1.
- `crates/seaquel-core/src/shared.rs` (new): `LocalFiles`, the repo lock (also exposed to `seaquel-rpc` for the git group), repo calls, link/unlink, scan, import projects, sync, and `publish` called from `library.rs` and `state.rs` after their commits; `SyncReport` notices.
- `crates/seaquel-core/src/imports.rs` (new): `ImportPaths`, reading and decoding (plist), candidates, `import_create` in one `WriteTx` with the order append.
- `crates/seaquel-core/src/library.rs`, `state.rs`: the publish hook and `Seqd.projection`.
- `crates/seaquel-core/Cargo.toml`: the `imports` feature.
- Tests: `crates/seaquel-core/tests/{shared,imports,shared_cli}.rs`.

**Tests first:**
- `replays_every_projection_case` (temp dir and storage, `changes.json` exactly);
- `a_symlinked_file_is_skipped_and_a_symlinked_dir_refuses_writes` (real symlinks), `a_path_with_dot_dot_is_refused`, `a_file_past_16_mib_is_skipped_and_named`;
- `a_conflicted_repo_is_not_synced` (a real merge conflict), `an_unreadable_directory_unshares_nothing`;
- `publish_writes_the_row_read_under_the_lock` (two updates racing: the file ends with the later commit), `a_failed_publish_leaves_the_row_and_the_next_sync_writes_it`, `unshare_restores_the_file_when_the_row_write_fails`;
- `a_pull_waits_for_a_publish` (the lock), `no_file_io_inside_a_write_tx` (a tree whose write blocks while another storage write must commit);
- `link_exports_and_links_templates_without_duplicates`, `import_projects_keeps_the_directory_under_a_taken_name`, `rename_keeps_the_directory`;
- `template_changes_reach_the_connection_and_keep_local_fields` (Q23);
- imports: `candidates_read_the_injected_home_only`, `a_missing_file_is_found_false`, `create_checks_duplicates_inside_the_transaction` (two concurrent creates of one candidate: one imported), `create_appends_to_the_order_in_the_same_transaction`;
- `local_files_denied_refuses_every_call_and_publishes_nothing`;
- `shared_cli`: a workspace built as `seaquel-cli` builds it (read-only storage): candidates and scans answer, writes are `STORAGE_READ_ONLY`;
- events: `a_sync_emits_one_event_per_kind_and_scope`, `a_no_op_sync_emits_nothing`, `a_publish_emits_shared_repo`;
- logs: `no_paths_names_hosts_or_contents_in_logs`.

**Run:** `cargo test -p seaquel-git -p seaquel-workspace -p seaquel-storage`; `cargo test -p seaquel-core --features seaquel-runtime/tokio`; `cargo test -p seaquel-mcp -p seaquel-cli`; CI clippy, both wasm32 lines, `npm run crates:check`.

**Review:** the lock order (repo, then write); no file I/O in a `WriteTx`; validation before the first write; `LocalFiles` checked in Core, not only in dispatch; the MCP tests unchanged.

**Things this task could quietly skip:** the publish hook on `connectionRemove` and the local-only patch; restoring a deleted file; the case-insensitive rename on a case-sensitive test file system (test both by comparing paths, not by the disk); `refill_name_keys`-style work after a downgrade (none needed: NULL links mean today's rule).

### Notes from Task 5 (as built)

2026-10-01, about 4.5 h wall, much of it builds and two long mutation runs on the shared target.

- **What's there.**
  - `crates/seaquel-git/src/tree.rs` (new): `scan`, `apply`, `restore`, `conflicted`, `project_dirs`, `read_file`, `paths_under`, `ScanBounds`, `ApplyOptions`, `OpOutcome`, `WriteHook`. Every path below the repo root is checked with `names::check_rel_path`/`check_component`, every component with `symlink_metadata`, and files open with `O_NOFOLLOW | O_NONBLOCK` (a planted FIFO can't hang a read). The root (the user's repo path) is trusted as given. `seaquel-git` now depends on `seaquel-workspace` (for `FileOp`, `DirScan` and `file_hash`) and on `libc` on Unix.
  - `crates/seaquel-core/src/shared.rs` (new): sync, publish, the Decision 37 removals, link, unlink, scan, import projects, the repo list and the git calls under the lock. `crates/seaquel-core/src/git.rs`: `RepoLock` and `Core::repo_lock` (one `futures::lock::Mutex` per canonical repo path).
  - `crates/seaquel-core/src/projection.rs` (new): the hooks `library.rs` and `state.rs` call. They are no-ops without the `git` feature.
  - `crates/seaquel-core/src/imports.rs` (new, `imports` feature): `ImportPaths`, reading and decoding, `import_candidates`, `import_create`.
  - `crates/seaquel-core/src/lib.rs`: `LocalFiles`, `CoreBuilder::{local_files, import_paths, file_write_hook}` (the last is `doc(hidden)`, tests only), and `Core::local_files()`.
  - `library.rs`/`state.rs`: the write bodies were moved into in-transaction helpers so the sync uses the library's own rules: `insert_saved_query_in`, `update_saved_query_in` (keyframe and prune), `insert_connection_in`, `update_connection_in`, `insert_dashboard_in`, `update_dashboard_in`, `insert_linked_project_in` and `connection_order_in`. The publish hooks are on `savedQueryCreate`/`Update`/`Remove`, `dashboardCreate`/`Update`/`Remove`, `connectionUpdate`/`Remove` and `projectUpdate`.
  - `seaquel-workspace`: `Seqd.projection` (`Option<PublishOutcome>`, `ts(optional)` with an inline type, `Seqd::new`), `StoredKind::SharedRepo`, and a new `shared_api.rs` with the wire answers (`SyncReport`, `UnlinkReport`, `RepoPreview`, `PreviewProject`, `PreviewTemplate`, `RepoPatch`, `ImportOutcome`, `ImportKeyOutcome`). It sits outside `shared/` so this task didn't edit 4b's files. None derives `ts` yet, since `SyncNotice` doesn't.
  - `seaquel-storage`: `saved_queries::list` and `connections::list_in_project` (both through `Reader`), and `{saved_queries, dashboards, connections}::link` (one row's link).
- **Lock and transaction order.**
  - The repo lock is always taken first and the storage write lock after. No `WriteTx` is open during file I/O.
  - Sync, under the repo lock:
    1. read the index (a conflicted repo answers `conflicted` and writes nothing);
    2. scan;
    3. in one `WriteTx`, read all of the project's rows, `plan_sync` with `Limits { library, state }` from the Core, apply the row ops in plan order through the helpers, store the links without `pending_on`, the order append and `shared_dir`, then commit;
    4. write the files, each independently (a sync never deletes or renames);
    5. in a second `WriteTx`, store the links whose write succeeded.
  - Events are held until the end: one per kind and scope (the row kinds, scope the project; `project` for an order change; `sharedRepo` when a file was written). A sync that changes nothing emits nothing.
  - A publish runs after the library call's commit. It takes the repo lock, reads the row and its link again, reads the file at the stored path and the taken paths, plans, and applies the files as a sequence that stops at the first failure. Then it stores `on_success`, or `on_failure` after a failed write but never after a refusal, and announces `sharedRepo`. If a `Stale` op (`expect_hash` differs) is found, nothing is written, the project syncs under the same lock (its notices aren't put in the session's memory), and the answer is `FILE_CHANGED`.
  - Removals (unshare, `isLocalOnly: true`, the three removes): under the repo lock the file is deleted first and its bytes kept, then the row is written. A failed row write puts the file back (`tree::restore`). A successful unshare clears the link.
  - `projectUpdate` stores `shared_dir` (the slug of the old name) inside its own transaction before a rename, then publishes `project.yaml`.
  - The library calls' futures stay `Send`: each `PublishContext` (it holds a `&dyn Fn`) lives in a block with no await.
- **Choices the plan left open.**
  - A stored path outside the project's own `<kind>/` directory, or a template link to another repo, is treated as no link, so a publish never writes there. A path that climbs out with `..` is refused by `tree`.
  - A project whose repo has no row (an older release's state) gets one registered under the project's name at its first sync or publish.
  - Scan bounds: past 20,000 files, 4× that many entries or 256 MiB, nothing is read and the whole project is `Skipped { tooMany | tooLarge }` at its directory, so nothing changes on either side. A file over 16 MiB is skipped on its own. Hidden entries are ignored. A name Decision 32 refuses is `Skipped { invalid }`. A missing project directory is an empty scan, but a missing repo folder is `FILE_ERROR`.
  - `dashboardCreate` with `shared: true` publishes. No fixture creates one shared.
  - Clippy (`--workspace --all-targets -D warnings`), both wasm32 lines and `crates:check` (24 crates) pass.
  - The git calls are `Workspace::shared_git_{pull,push,commit,resolve}`, under the lock. A successful pull or push splices `lastSyncAt` into the stored JSON.
  - `ImportPaths` is on the `CoreBuilder`. Without them the default locations are `found: false` (never the real home).
  - The plist is decoded through `plist::stream` first: past 64 levels or 2,000,000 events it is `unreadable`, before `Value::from_reader` builds anything. That stream sits behind plist's unstable feature, so `plist` is pinned `~1.10.1` in Core.
  - `importsCreate` outcomes are `imported`, `duplicate` (with `duplicateOf`) or `notFound` per key. Problem keys refuse the whole call before anything is written.
- **Tests.**
  - `seaquel-git/tests/tree.rs` has 10 tests. All 10 failed against a stub before `tree` was written.
  - The Core tests were written after the implementation, so each was shown to fail by a mutation: the pull without the lock, no restore, no `on_failure`, no stale check, events doubled, no `LocalFiles` check, a path in a log line, a write tx held across the publish's file write, no notice memory (5 replay differences), the plist depth and event bounds off (stack overflow), and the duplicate read outside the transaction. One test, `a_pull_waits_for_a_publish`, hung instead of failing in the first mutation run because the gate stayed held after the assert, so the three gated tests now release before they assert.
  - `tests/shared.rs`: 20 named tests plus the replay. `tests/imports.rs`: 8 tests. `tests/shared_cli.rs`: 1 test.
- **The replay** (`tests/shared/replay.rs`, `tests/shared/world.rs`). It replays all 52 cases and 225 steps.
  - Its setup is real:
    - temp dirs (`/repos`, `/home` and `/outside` rooted in a canonical temp dir);
    - real symlinks, and `unreadable` dirs at mode 000;
    - `git init` with the seed committed and pushed to a bare origin;
    - each pull scripted through a fresh teammate clone, with conflicts made real (both sides change the file, then the conflicted files get the scripted marker text);
    - `failing` paths through the write hook;
    - the beta-era file for `v2026.4.5-beta.1`.
  - It holds Core to more than the pure replay does: seeded repos' `data` byte for byte (all but `lastSyncAt` after a pull), the keychain after each step, `name_key` correctness, and every call succeeding unless the step's outcome is a refusal.
  - Every step passed on the first run that got past the harness's git setup. With the sync's row ops switched off it reports 116 differences in 19 cases. With the sync's file writes switched off it still passes: no recorded case needs a sync-side write (the targeted tests cover it).
  - No fixture expectation was unmet, and no planner bug was found.
- **For Task 6.**
  - Serve `Workspace::shared_*` and `import_*`. `importProjects` answers `ImportedProjects`, not a list of ids, and a repo's sync can report `failures`. `dispatch_git` should call `shared_git_pull`/`push`/`commit`/`resolve` (or take `core.repo_lock` itself) so they run under the lock.
  - The wire types are in `seaquel_workspace::shared_api`, and `SyncNotice`, `Kind`, `SkipReason`, `ReplacedValues` and the `shared_api` types need `ts` derives. `SyncTarget` is Core's.
  - `Seqd.projection` and `StoredKind::SharedRepo` changed the generated TS (`Seqd.ts`, `StoredKind.ts`). `types:gen` ran twice with no diff the second time.
  - Pass `LocalFiles::Allowed` and `ImportPaths::from_env()` on desktop and the CLI.
  - The `git` and `imports` features are now on in Core's dev-dependency for its tests.

### Review fixes for Task 5 (as built)

2026-10-01, about 2.5 h of wall time, much of it builds. Each fix's test was seen failing before the fix unless noted.

- **C1 (stack).** `seaquel-rpc` `every_new_library_method_answers_with_its_own_name` overflowed its 2 MiB stack, because each library call inlined publish → sync. `publish_row`, `publish_locked`, `sync_locked`, `unpublish_begin` and `unpublish_end` are now thin wrappers that `Box::pin` their bodies, which covers every call site.
  - New test: `library_calls_fit_a_2_mib_stack`. It runs every publishing library call through `dispatch_workspace` on an explicit 2 MiB thread, with 768 KiB of that stack reserved first.
  - Measured: unboxed, the calls needed 1.25–1.5 MiB; boxed, 1–1.25 MiB. The test overflows with the boxing removed and passes with it.
  - On a plain 2 MiB thread, with no reservation, it passed even before the fix. Only the existing test failed there.
- **I1 (events).** A sync now announces its first transaction's events right after that commit. Only the late link kinds and `sharedRepo` wait for the files.
  - Test: `a_syncs_row_events_survive_a_failed_file_task`. The published sequence has moved while a file write is held, and the row events arrive even when the file task fails outright (a test hook that panics).
- **I2 (decision).** `unpublish_begin` now returns a `Result`. If the row has a link and its file can't be deleted (failed or refused), or the project's link can't be read, the removal or unshare is refused with `FILE_ERROR` before the row write. A stale file (a teammate's change) still goes on, as before.
  - `tree::apply` now calls the hook for deletes too, and `unpublish_begin` passes `core.file_hook()`.
  - Test: `a_removal_whose_file_cant_be_deleted_is_refused` (a failing delete, then an unreadable project link).
- **I3 (decision).** `repoRemove` answers `REPO_IN_USE` while any project links to the repo's path. `ensure_repo` registers a missing repo under the id the project's template links already carry (from this project's `connections/` directory), when no row has that id.
  - Test: `a_repo_in_use_stays_and_a_lost_one_keeps_its_id`. The first assertion stopped the pre-fix run, so the id-reuse half was shown by mutation: passing no carried id makes the template import a second time.
- **I4 (decision).** `linkProject` answers `PROJECT_ALREADY_LINKED` when the project is linked to another path (checked again inside the transaction). The same path again leaves the project row, directory and repo as they are: it only exports the ticked connections, then syncs.
  - Test: `relinking_is_refused_elsewhere_and_shares_only_on_the_same_repo`.
- **M1.** `seaquel_git::tree::lock_key` canonicalizes on a blocking thread, before the map's mutex is taken.
  - On macOS, `fs::canonicalize` returns the on-disk case of existing components (checked: `myrepo/sub` gives `MyRepo/Sub`). A path that doesn't exist fails and is keyed as given, which is safe because nothing can be written there. So the canonical path is the key; no case folding is needed.
  - `two_spellings_of_a_repo_share_its_lock` (a symlink, and another case on macOS) passed before the change too: keying by the canonical path already worked. The fix only moves the blocking call.
- **M2.** `tree::is_dir` runs on a blocking thread, used for the three `is_dir` checks. The import's file read and its decode, the plist included, run in `spawn_blocking`; the `imports` feature now pulls in tokio's `rt` (native only). No test: these are only thread moves.
- **M3.** When a rename's new file is written but the old file's delete is stale, the new file is taken back (deleted, expecting the bytes just written) before the sync. Only the teammate's file keeps the id; the file wins and the rename stays in the row's history.
  - Test: `a_rename_over_a_teammates_change_leaves_one_file`.
- **M4.** `write_atomic` keeps a rewritten file's mode, and fsyncs the folder after the rename (Unix).
  - A scan with `ScanBounds::clear_temp_files` removes leftover `.…seaquel-tmp` files. Only the sync sets it, since it holds the repo lock; the preview scan runs without the lock and leaves them.
  - Test: `a_rewrite_keeps_the_mode_and_a_scan_clears_stale_temp_files`. The folder fsync isn't observable in a test.
- **M5.** `SyncTarget::Repo` goes on past a failing project, and `SyncReport.failures` (a `ProjectFailure` each) names them.
  - `shared_import_projects` now answers `ImportedProjects { projectIds, failures }`. Each directory is imported whole or not at all: if a project's sync fails, the project is removed again, its rows cascading, along with a repo row the import made that nothing else uses.
  - Test: `multi_project_calls_go_on_past_a_failure`, using `max_saved_queries` to make the second project fail inside its transaction. The undo was also shown by mutation.
  - The replay now treats a reported failure as a failed call.
- **M6.** The `state.rs` replay failure didn't come back. The full Core runs in this round are captured to files, and it passed in all of them.
- **Replay gap.**
  - After every `sync`, `syncRepo`, `linkProject` and `importProjects` step, every row with a stored path in a linked, non-conflicted project must hash to its base, and so must its file (unless a name was withheld).
  - No recorded step leaves a local change for a sync to write, so each case now ends with an epilogue: every linked shared query with a readable file is edited straight in SQL (R ≠ B, F = B), its project is synced, and the file must hold the edit and match its base. At least 20 cases reach it; the test asserts that.
  - A fixture-free test does the same for one query: `a_sync_writes_a_local_change`.
  - With the sync's file writes switched off, the replay reports 56 differences in 22 cases and the test fails. Before this round, the same mutation passed the replay.
- New codes: `PROJECT_ALREADY_LINKED`, `REPO_IN_USE`. New wire types in `shared_api`: `ProjectFailure`, `ImportedProjects`, and `SyncReport.failures`.

### Re-review follow-ups for Task 5 (as built)

2026-10-01, about 1.3 h. Each test was seen failing before its fix unless noted.

- **R1.** `library_calls_fit_a_2_mib_stack` now runs on a Core with `LocalFiles`, with the project linked to a temp `git init` repo, and still sends every call through `dispatch_workspace`.
  - Paths covered: a written template and query, a stale update (publish → `Stale` → `sync_locked` → `apply_row_op`), a stale unshare, a stale dashboard remove (both `unpublish_end` → `Stale` → `sync_locked`), a rename, the project rename and the connection remove. The test asserts each stale answer's `FILE_CHANGED`, which proves those paths ran.
  - It fit on the 2 MiB thread with the 768 KiB reserve, but only just: the calls needed 1152–1280 KiB.
  - So `apply_row_op` and the rpc library dispatch (`dispatch_workspace`'s `Request::Library` arm) are boxed too. The calls now need 1024–1152 KiB: at least 896 KiB is free on a bare 2 MiB worker, and the test keeps the 768 KiB reserve.
- **R2.** A link the sync stores after its file write is announced for its row's kind and id even when the first transaction already announced that kind. Test: `a_late_link_is_announced_for_its_row`.
- **R3.** An imported project, and a repo row it registers, are announced through the after-commit path only once the project's sync succeeds. A project the undo removes is never announced.
  - The race is narrowed, not closed: the project row is committed before its sync, so a window that reloads its projects for another reason during that sync could see it. Its rows' events (scoped to a project no window knows yet) do go out as the sync writes them.
  - Test: `an_import_announces_its_project_only_after_its_sync`.
- **R5.** `shared_link_project` reads the project again after the repo lock is taken, and decides then whether this is a first link, a relink to the same path, or `PROJECT_ALREADY_LINKED`. Test: `a_link_rechecks_the_project_under_the_lock` (the test holds the lock and moves the project elsewhere while the link waits).
- **R6.** The `FILE_CHANGED` wording on a remove is in Task 7.
- **R8.** Repo paths are compared in the canonical form (`same_repo`: `lock_key` of the path without trailing separators) wherever a path names a repo:
  - `linked`'s repo id, the repo's sync, unlink's "still used", the scan's linked projects;
  - `REPO_IN_USE`, `record_last_sync`, and registration (link, import, `repoRegister`, which reuse the matching row's id);
  - the import's undo.
  - The canonical form is computed before each write transaction, since it reads the disk. The exact `get_by_path` and `ids_with_repo_path` inside the transactions stay as a second check.
  - Test: `repo_paths_compare_in_canonical_form` (a project row with a trailing slash, another project through a symlink).
- **R4, R7.** No action; listed in the follow-ups.

## Task 6: RPC and the desktop

**Files:**
- `crates/seaquel-rpc/src/{shared,imports}.rs` (new), `workspace.rs` (`Request::Shared`, `Request::Imports`, the two storage variants out, `storage_change`, `dispatch_workspace`'s `LocalFiles` check), `git.rs` (`dispatch_git` takes the repo lock for pull, commit and resolve), `library.rs` (the `projection` field on answers).
- `src-tauri/src/lib.rs`: `CoreBuilder::local_files(Allowed)`, the two groups in the storage-backed arm with the webview label, `read_dbeaver_config`/`read_tableplus_config` and the `plist` dependency out; `capabilities/default.json` per Decision 49 (if Task 7's `rg` agrees; otherwise in Task 7).
- `crates/seaquel-cli`: `local_files(Allowed)` and the `imports` feature, no command (Q26).
- `crates/seaquel-server`: nothing but a test.
- `npm run types:gen`.

**Tests first:**
- rpc: wire snapshots of every new method, `unknown_request_fields_are_refused` for both groups, `a_retired_storage_method_is_unknown` for the two, `debug_redacts_every_new_params_type`;
- server: `shared_and_imports_are_not_supported_on_web` (with every feature compiled in), `a_web_library_write_never_publishes`;
- src-tauri: `core_call_serves_shared_and_imports_with_the_webview_origin`, `pull_takes_the_repo_lock`.

**Run:** `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio`; `mise exec -- npm run cli:build && cargo test -p seaquel --lib`; `types:gen` twice with no diff the second time; `npm run check` 0/0 (stub the retired TS methods with `NOT_SUPPORTED` until Task 7, as 5d's Task 5 did; don't release in between).

**Review:** no new route or header on web; `dispatch_workspace` refuses both groups without `LocalFiles`; the web server's dependencies still pass the CI step (no `plist` needed there, and `git2` still absent).

**Things this task could quietly skip:** the CLI's `local_files`; the git group's lock (it's in `dispatch_git`, which doesn't see the workspace today).

### Notes from Task 6 (as built)

2026-10-01, about 2.5 h wall, most of it builds and the shared target's lock. Filtered `seaquel` lib runs of single storage tests took 100–130 s (an existing test the same), while the whole lib suite took 40 s; nothing in this task explains it.

- **The groups.**
  - `crates/seaquel-rpc/src/shared.rs` (new): `SharedRequest` (`reposList`, `repoRegister`, `repoUpdate`, `repoRemove`, `linkProject`, `unlinkProject`, `scan`, `importProjects`, `sync`, `syncRepo`) and `SharedResponse`, served by `Workspace::shared_*` behind `git` + `storage`. Writes and lists answer `Seqd`; `scan` answers `RepoPreview` alone; the repo rows cross as their stored text (`ts(as = "Seqd<PersistedSharedQueryRepo…>")`).
  - `crates/seaquel-rpc/src/imports.rs` (new): `ImportsRequest` (`candidates`, `create`, both with an optional `path`) and `ImportsResponse`, behind a new rpc feature `imports` (`storage` + Core's `imports`).
  - Both params enums are `deny_unknown_fields`. `SharedRequest`'s `Debug` is the method only; `ImportsRequest`'s is the method, the source, whether a path was given and the key count. The responses' `Debug` shows the method and `seq` (or the domain types' own redacted `Debug`).
  - `workspace.rs`: `Request::Shared`/`Imports` (and the responses), each refused by `require_local_files` with `NOT_SUPPORTED` "… isn't supported here" before Core is reached when `core.local_files()` is `None`, then boxed. `sharedReposLoadAll`/`SaveAll` are out of the storage group (and of `storage_change`); naming one is `INVALID_ARGUMENT` "unknown variant". Their storage functions stay for the frozen fixture.
  - The `ts` derives Task 5 left: `shared_api`'s answers and `RepoPatch` (now also `Serialize`, so a request round-trips), `SyncNotice`, `Kind` (exported as `SharedKind`, since `Kind` alone is too generic in the TS namespace), `SkipReason` and `ReplacedValues`. ts-rs warns that it ignores `ReplacedValues`' `serialize_with`; the fields are numbers either way.
- **Git under the lock.** `dispatch_git(core, ws: Option<&Workspace>, git, req, origin)`. Pull, push, commit and conflict resolution (`GitRequest::takes_repo_lock`) go through `Workspace::shared_git_*` when there's a workspace and the Core has `LocalFiles` (so a pull or push records `lastSyncAt`); otherwise they take `core.repo_lock` themselves and record nothing. The desktop passes its storage workspace for those four (opening it if needed) and `None` when storage can't open, so a pull still works, under the lock, with broken storage. The other git calls need no storage, as before.
- **The desktop** (`src-tauri/src/lib.rs`): `desktop_core(ImportPaths::from_env())` builds the app's Core with `LocalFiles::Allowed` and the import paths; both groups go through the storage-backed arm with the webview label, like the library. `read_dbeaver_config`, `read_tableplus_config` and the `plist` dependency are gone; `seaquel-core` and `seaquel-rpc` gained `imports`.
- **The CLI** (`crates/seaquel-cli/src/mcp.rs`): `core_builder` adds `LocalFiles::Allowed` and `ImportPaths::from_env()`; Core's `imports` feature is on. No command (Q26). The CLI has no `git`, so its Core has no shared projection; its storage is read-only anyway.
- **The server**: nothing in `src`. Its dev-dependencies turn on rpc's `git` and `imports`, so `tests/rpc_shared.rs` runs on `web_core()` with both compiled in. `cargo tree -p seaquel-server -e normal` still has no `git2`, `plist` or any other banned crate.
- **Stubbed until Task 7.** `RustStorageClient.sharedRepos.loadAll`/`saveAll` reject with `NOT_SUPPORTED` without a request (`retiredStorageMethod`), and `client.test.ts` expects that for the `sharedReposRepo` steps (the demo's sql.js replay still runs them). Meanwhile, on the desktop, `SharedRepoManager` loads no repos at startup and saves none, so shared projects look unlinked to the GUI; the TablePlus and DBeaver import dialogs fail, since `api/tauri.ts` still invokes the two removed commands. Web is unaffected (it had neither). Don't release in between.
- **Decision 49's capability trim is left for Task 7**: `rg` still finds `exists`, `mkdir` and `rename` from `@tauri-apps/plugin-fs` in `project-manager`, `shared-query-manager`, `shared-dashboard-manager` and `shared-repo-manager` (and `readDir` in the latter), so the permissions are still needed until those go.
- **Tests.**
  - `crates/seaquel-rpc/tests/shared.rs` (13): every method answering under its name, the codes (`PROJECT_NOT_LINKED`, `REPO_NOT_FOUND`, `FILE_ERROR`, `PROJECT_ALREADY_LINKED`, `REPO_IN_USE`, `IMPORT_SOURCE_UNREADABLE`), the `LocalFiles` refusal, byte-for-byte round trips, unknown fields, the two retired methods, request and response `Debug`, `reposList` keeping stored bytes, the `sharedRepo` event with its origin, the git calls waiting for the lock (with and without a workspace), `lastSyncAt` after a push, and a 2 MiB-stack test of the deepest calls.
  - `crates/seaquel-server/tests/rpc_shared.rs` (2): `shared_and_imports_are_not_supported_on_web`, `a_web_library_write_never_publishes`.
  - `src-tauri`: `core_call_serves_shared_and_imports_with_the_webview_origin`, `pull_takes_the_repo_lock`. `seaquel-cli`: `the_cli_core_may_read_local_files`.
  - Seen failing first: 11 of the 12 first rpc tests against a stub (the `dispatch_git` signature only); `unknown_request_fields_are_refused` passed vacuously then, as an unknown group is refused too. The desktop's shared test against a stub `desktop_core` without `LocalFiles`; the CLI test against a stub `core_builder`. Written after the code and shown by mutation instead: the server's two tests (both fail with `LocalFiles` on `web_core`), `pull_takes_the_repo_lock` (fails with the lock and the workspace route removed), the dispatcher's own `LocalFiles` check (fails with it removed, Core's refusal having another message), and the stack test (passes even unboxed: the calls fit, and the boxing stays as with the library).
- **For Task 7.**
  - The wire is `shared.*` and `imports.*` as above; `importsCandidates`/`importsCreate` in the plan's prose are `imports.candidates`/`imports.create`. The generated types: `SharedRequest`, `SharedResponse`, `ImportsRequest`, `ImportsResponse`, `SyncReport`, `SyncNotice`, `SharedKind`, `SkipReason`, `ReplacedValues`, `UnlinkReport`, `RepoPreview`, `PreviewProject`, `PreviewTemplate`, `ProjectFailure`, `ImportedProjects`, `ImportOutcome`, `ImportKeyOutcome`, `RepoPatch`.
  - `SyncReport.failures` and `ImportedProjects.failures` are absent when empty. `repoRemove` answers `REPO_IN_USE` while a project links to the repo; `linkProject` answers `PROJECT_ALREADY_LINKED`.
  - Remove `StorageClient.sharedRepos` (and its stubs, the sql.js binding stays for the fixture) and `client.test.ts`'s `SHARED_REPOS_STUBBED` branch, and delete `api/tauri.ts`'s two readers.
  - The git calls already run under the lock and record `lastSyncAt`; the GUI only needs to call `shared.syncRepo` after a pull, a commit or a resolution.

### Review fixes for Task 6 (as built)

2026-10-01, about 0.6 h, mostly builds. Each test was seen failing before its fix unless noted.

- **I1.** `lastSyncAt` is best effort. A pull or push that worked used to fail with the storage error after the tree or the remote had changed (`record_last_sync(...)?`). `shared_git_pull`/`push` now call `record_last_sync_best_effort`, which logs `activity=shared.lastSync` and the code (no path) and still answers the result. Test: `a_push_or_pull_succeeds_when_last_sync_cant_be_written` (Core `tests/shared.rs`, on the CLI's read-only storage: `STORAGE_READ_ONLY` failed the push before the fix).
- **M1.** `repo_path_key` moved to `seaquel_core::git` (public, behind `git` only), and `dispatch_git`'s no-workspace route keys the lock with it, as `shared_git_*` do. A trailing `/` never mattered (a `PathBuf` key ignores it); a trailing `\` on Unix did. Test: `the_plain_route_keys_the_lock_like_the_workspace_route` (rpc, a `\`-suffixed path to a missing folder).
- **M2, M3.** The desktop's comments: the catch-all arm names `shared` and `imports`; the git arm says it may open or retry storage, drops the error, and never blocks git on it.
- **M4 (no change).** `RepoPatch.remoteUrl` stays a plain optional string. The GUI clears a remote by saving `""` (`project-settings-tab-view.svelte` → `setRemoteUrl(id, "")`), never by removing the field, and the stored row's `remoteUrl` is a required string that older releases read; `null` there would break them. Pinned by `an_empty_remote_url_clears_it` (passed first: it only pins existing behaviour).

## Task 7: The GUI

**Files:**
- New `src/lib/hooks/database/shared/{types,core-shared,no-shared,index}.ts` and `imports` beside it.
- `shared-repo-manager.svelte.ts`: the repo list through `shared.*` (no debounce, no replace-all), git calls and sync state as before, `sync` after pull, commit and resolve and on the background refresh; the scan caches go (`sharedQueriesByRepo`, `sharedDashboardsByRepo`, `sharedLabelsByRepo`; `sharedProjectsByRepo`/`sharedConnectionsByProject` come from `shared.scan`).
- `project-manager.svelte.ts`: `setGitRepoPath` becomes `linkProject`/`unlinkProject`; `importFromGitRepo` becomes `importProjects`; `reconcileGitState` becomes `sync`; the rename's directory code and `storeReconciledQueries` go. `project-settings-tab-view.svelte`: no template import after save (the link does it).
- `connection-manager.svelte.ts`, `saved-queries.svelte.ts`, `dashboard-manager.svelte.ts`: no file calls; share and unshare are patches; a `projection.status: "failed"` answer is said with `errorToast` (new i18n key). `storeReconciled` and the 6b reconcile notices move to the sync report's notices.
- Deleted: `shared-query-manager.svelte.ts`, `shared-dashboard-manager.svelte.ts`, `services/{config-file-parser,query-file-parser,dashboard-file-parser,yaml-utils,tableplus-import,dbeaver-import,connection-import}.ts` (whatever still has a caller moves or stays; `rg` decides), `api/tauri.ts`'s two readers, Task 1's TS paths, and their tests.
- Import dialogs and stores: candidates from Core, "nothing found" and "couldn't read" said, `importsCreate` with the selected keys; failures named with their reason.
- `services/deep-link.ts`: queries and dashboards by `sharedPath` after a sync; a link to a file not yet stored says so instead of opening a tab on a scan id (bug 24). A connection link opens the project's connection whose template path is the link's; in a directory no local project links, the import dialog opens with that directory ticked, and the connection opens after the import (Q32); cancelling does nothing.
- `library/sync.ts`: `sharedRepo` events refresh that repo's status.
- `capabilities/default.json` (Decision 49), new i18n keys through `i18n-translator`.
- **The link dialog (Q30, ~0.5 h).** On a project's first link, list its connections with checkboxes: every one ticked except those marked `is_local_only`. Send the ticked ids as `linkProject`'s `share`. Say that unticked connections stay local and that only ticked ones are written to the repo. A test: `linking sends only the ticked connections`.

**Tests first (vitest):**
- `sharing a query calls Core once and touches no file`, `a failed projection is said and the query stays saved`;
- `a pull syncs every project linked to the repo`, `a conflicted sync opens the conflict dialog`;
- `linking a project sends one call`, `unlinking removes the imported connections from the page`;
- `the TablePlus dialog says when nothing is found`, `an import names each failure with its reason`;
- `a deep link to a stored shared query opens it by path`;
- `another window's repo write refreshes the status here`;
- `rg "@tauri-apps/plugin-fs" src/lib/hooks` finds nothing (a test over the file list);
- the dashboard, project, connection and saved-query suites pass, rewritten where they mocked the file projection.

**Run:** `npm run check` 0/0; `CI=1 mise exec -- npx vitest run`; `npx oxlint --type-aware --type-check --deny-warnings`; the autofixer; `build`, `build:web`, `build:demo`; live on desktop: the manual checks' items.

**Review:** the skip list; `rg "nameToFilename|parseQueryFile|serializeDashboardFile|readDir|activeRepoId" src -g '!*.test.ts'` shows only the sync button's use of the active project's repo; the sync's `nameTaken` notice names the right kind of row (today's reconcile toast calls a saved query "a project", Task 2's finding).

**Notice text (from Task 4b):** the `nameTaken` notice for a withheld name (a sync update whose new name another row holds) says that the repo keeps the teammate's name and the row keeps its own until the name is free.

**Wording (Task 5 re-review, R6).** On a remove or an unshare, `projection.code: "FILE_CHANGED"` means a teammate changed the file: the GUI says "A teammate changed it; the repo's version is kept and will reappear", not that the removal failed. On an edit it says the repo's version is shown and the user's is in the history.

**Things this task could quietly skip:** the onboarding's DBeaver card and the getting-started buttons (both open the import); the app header's "Import from repo" path; `project-settings-tab-view.svelte`'s use of `sharedProjectsByRepo`; the connections sidebar's template lookup (`components/sidebar/manage/connections.svelte:303`).

### Notes from Task 7 (as built)

2026-10-01, about 1.1 h wall (about 14:50–15:55), including a full wasm build for the first `npm run check`.

- **The seams.** `src/lib/hooks/database/shared/`: `types.ts` (`SharedService`, `ImportsService`, the codes), `core-shared.ts` (`CoreShared`, `CoreImports` over `RustStorageClient.shared()`/`.imports()`, which share the write queue; `SHARED_METHOD_KIND`/`IMPORTS_METHOD_KIND` classify every method), `no-shared.ts` (`NoShared`, `NoImports`: `NOT_SUPPORTED`, for web and the demo), `index.ts` (`getShared`/`getImports`, picked by `isTauri()`; `setShared`/`setImports` for tests), `projection.ts` (`reportProjection`), `notices.ts` (the sync notices, failures and `shared` errors, worded). The plan's `listRepos`/`registerRepo`/… sketch gained `updateRepo` and `removeRepo`; `sync` takes `{projectId} | {repoId}` and maps to `sync`/`syncRepo`.
- **`SharedRepoManager`** is now a view model: the repo list from `shared.reposList` (no debounce, no replace-all, no stored `syncStatus`: bug 20), git calls and status in memory, `syncProject`/`syncRepo` and `showReport`, and the conflict dialog's state (`state.sharedConflict`). A pull, a commit and a resolution (once no conflicted file is left) call `shared.syncRepo`; the background refresh syncs a repo whose status changed since its last refresh (not on its first pass: startup syncs the active project itself). The scan caches (`sharedQueriesByRepo`, `sharedDashboardsByRepo`, `sharedLabelsByRepo`, `sharedProjectsByRepo`, `sharedConnectionsByProject`), `activeRepoId`, `projectGitSyncState` and the derived values over them are gone from `DatabaseState`.
- **A sync's rows reach the page through the page itself.** Core's events for a sync this page started carry this page's origin, and the change feed skips them. So after a sync that changed rows or wrote files, `UseDatabase.refreshProjectRows` reads the project's saved queries, dashboards, connections, project row and order again. The same happens after a publish answers `FILE_CHANGED` (Core synced inside the call). Other windows' syncs arrive as usual, and their `sharedRepo` events now refresh the repo list and status (`LibrarySync`; desktop only).
- **Managers.** `ProjectManager`: `linkProject`/`unlinkProject`/`importProjects` replace `setGitRepoPath`, `importFromGitRepo`, `importSharedConnections`, `importSingleSharedConnection`; `setActive` syncs a linked project; the rename's directory code, `reconcileGitState`, `canReadSharedFiles`, `storeReconciledQueries` and the unused `setRemoveConnectionCallback` are gone, and `update` no longer takes `gitRepoPath` (`linkProject` sets it; patching it first would make the link a relink that skips `project.yaml` and `shared_dir`). `ConnectionManager`, `SavedQueryManager`, `DashboardManager` and `patchConnection` lost every file call (and `remove`'s `skipUnshare`); share and unshare are patches, and every write answer goes through `reportProjection`, removals and unshares with `removal: true` (R6's wording). `DashboardManager` lost `storeReconciled`, the file chain and the withdrawal counter.
- **Notices.** One toast per notice, at most 5, then "…and N more". `nameTaken` names the kind of row from `takenBy`'s id prefix (`saved-`, `dashboard-`, `conn-`), so a saved query is no longer called a project (Task 2's finding), and says the repo keeps its name and this side keeps its own until the name is free (4b's notice text). `conflict` on a connection lists the replaced values (never a user name or secret: the type has neither). Failures (`SyncReport.failures`, `ImportedProjects.failures`) are error toasts naming the project or directory.
- **The link dialog (Q30)** is `stores/link-project-dialog.svelte.ts` (`linkDialogSelection`, `linkProjectDialogStore.prompt`) and `components/link-project-dialog.svelte`, mounted in `(app)/+layout.svelte`. Project settings asks it on save when the path changes, listing the project's connections that aren't linked yet (local-only ones unticked); cancelling changes nothing. A changed path is the dialog, then `unlinkProject`, then `linkProject`; a cleared one is `unlinkProject`.
- **Imports.** One `ConnectionImportStore` (`stores/connection-import.svelte.ts`, instances `tablePlusImportStore`/`dbeaverImportStore`) and one `components/connection-import-dialog.svelte` replace the two stores and dialogs. "Nothing found" and "couldn't read" are said; a candidate with a `problem` is listed, can't be ticked, and shows its reason; `duplicateOf` shows as before. `ConnectionManager.importConnections(source, projectId, chosen)` sends the keys once (`imports.create`), reads the new connections and the order back, counts duplicates as skipped and names each failure with its reason (`notFound`, or a refused call's message for every key). The onboarding DBeaver card and both getting-started buttons go through the store with the active project.
- **Import from repo** (the header and the getting-started card) is one helper, `services/shared-project-import.ts` (`shared.scan`, then one project at once or the dialog); the dialog sends directories to `importProjects`.
- **Skip list.** Project settings' use of `sharedProjectsByRepo` is now `shared.scan`'s `linkedProjectIds` (a `$derived` promise, for the project link). The connections sidebar's template lookup reads the path out of `sharedConnectionId` (`sidebar/manage/share-link.ts`); queries and dashboards share their stored `sharedPath` (new on the page's `Query` and `Dashboard`, read only). The share link uses the active project's own repo.
- **Task 1's M4 and M5.** A status read that fails sets `syncState.statusUnreadable` and `lastError` and shows the repo as `error` (the badge already shows `lastError`); it's logged by code. Deep links set no active repo: there is none any more.
- **Deep links** (`services/deep-link.ts`): the link's directory is matched to a local project through `shared.scan`'s `linkedProjectIds`; that project is activated (or synced, if active) and the row whose `sharedPath` is the link's path opens. A file not stored says so (bug 24); a directory no project links says to import it. Deviation: a connection link in a directory no local project links imports that project (Core has no single-template import), so the deep-link project picker (its store and dialog) is deleted.
- **Conflicts.** The conflict dialog existed but nothing opened it. It's now mounted in the app layout whenever `state.sharedConflict` is set (a conflicted sync, a pull that conflicts), and its resolution goes through `SharedRepoManager.resolveConflict`. "Sync all" with uncommitted changes opens the commit dialog first and pulls and pushes after the commit (Decision 38). Deviation: `commitChanges` now throws on failure (it returned `null` and the button said "committed").
- **Deleted:** `shared-query-manager.svelte.ts`, `shared-dashboard-manager.svelte.ts`, `services/{config-file-parser,query-file-parser,dashboard-file-parser,yaml-utils,tableplus-import,dbeaver-import,connection-import}.ts` (and the two mapper tests), `stores/{tableplus-import,dbeaver-import,deep-link-project-picker}.svelte.ts`, `types/{tableplus,dbeaver}.ts`, `components/{tableplus-import-dialog,dbeaver-import-dialog,deep-link-project-picker-dialog}.svelte`, `api/tauri.ts`'s two readers, `StorageClient.sharedRepos` (both clients; `client.test.ts` treats it as a retired repo and the demo replay runs it on the frozen sql.js repo), Task 1's `shared-projection-fixes.svelte.test.ts` and `dashboard-reconcile.svelte.test.ts`, and the TS reconcile sections of `dashboard-review` and `library-persistence`. Six message keys left unused by this change went from every locale.
- **Decision 49 done.** `fs:allow-exists`, `-read-dir`, `-mkdir` and `-rename` left `src-tauri/capabilities/default.json`; nothing in `src` imports them.
- **Generated `ImportSource`** replaces the hand-written one in `library/types.ts`.
- **Replays.** `library-replay` no longer replays the 11 cases built on `setGitRepoPath`, `importFromGitRepo`, `importSharedConnections`, `importSingleSharedConnection` and `importConnections` (Core pins them), and neither it nor `state-replay` compares `files` (Core writes them inside the library calls): 104 cases, 134 steps replayed.
- **Review `rg`.** `rg "nameToFilename|parseQueryFile|serializeDashboardFile|readDir|activeRepoId" src -g '!*.test.ts'` finds only the generated `SharedReposState.ts` and the frozen sql.js `shared-repos-repo.ts`, both kept for the fixture; there is no sync-button use left, since the button takes its repo from the project. `rg "@tauri-apps/plugin-fs" src/lib/hooks` finds nothing, and a test scans for it.
- **Tests seen failing first:** `shared-gui.svelte.test.ts` (16 tests) failed to load (`shared/projection` missing); `deep-link.test.ts` (2) failed in the old handler (`sharedQueriesByRepo` undefined); `sync.test.ts`'s `another window's repo write refreshes the status here` failed (no call). After the implementation all passed on the first run. The rewritten `connection-manager` import tests and the `failed-load` and `library-persistence` replacements were written with the implementation.
- **For Task 8/9:** the desktop app wasn't run. The owner's manual checks below cover the link dialog, the conflict dialog's mount and "Sync all".

### Review fixes for Task 7 (as built)

2026-10-01, about 1.4 h wall (about 16:15–17:40), a good part of it Rust builds on the shared target. Q31 and Q32 are the owner's answers to two questions the review raised (in "Answered questions"; Decision 40 and the deep-link text amended).

- **Q31, the origin.** A new nullable column, `connections.shared_origin` (`exported` | `imported`), in a new migration `0005_shared_connection_origin.sql`. Least invasive: one expand-only column on one table, no data step, nothing older releases read; the other options were overloading `shared_file_id` or `shared_base` (changes what an existing column means) or a side table (a second write per link and a join on every list). `0004` hadn't shipped in a release, but dev builds have applied it since Task 3, and sqlx refuses a file whose applied migration's checksum changed (`MigrationState::Mismatch` in `open.rs`), so amending it would have locked those dev databases out; hence `0005`. Storage: `connections::set_origin`/`origin`, `ORIGIN_EXPORTED`/`ORIGIN_IMPORTED`; `set_link` clears the origin when it clears the link and keeps it otherwise; the reads carry `sharedOrigin` (read only, `READ_COLUMNS`), which the generated `PersistedConnection` gained. Core records `imported` when a sync creates a connection from a template (`apply_row_op`'s `CreateConnection`) and `exported` when a publish shares one (`shared_now`: the link dialog's ticks, the local-only switch), in the same transaction as the link. A link with no origin (stored by an older release, which only ever linked template imports) counts as imported.
- **Q31, unlink.** `shared_unlink_project(…, remove_imported)` / `shared.unlinkProject {projectId, removeImported}` (required on the wire): the user's own connections are unlinked and made local-only with their secrets; the imported ones are removed with their secrets when `removeImported`, kept like the others otherwise. `UnlinkReport` gained `keptConnectionIds`; one `connection` event names both lists. A relink is safe by construction: the unlink keeps the user's own, so a link that fails afterwards loses none of them. The GUI: `ProjectManager.importedConnectionsOf`, `unlinkWithConfirmation(projectId, ask)` (no dialog when nothing was imported; `null` from `ask` changes nothing) and `unlinkProject(projectId, removeImported)`, which drops the removed connections and reads the kept ones back. The dialog is `stores/unlink-project-dialog.svelte.ts` and `components/unlink-project-dialog.svelte` (Cancel, Keep them, Remove them), mounted in the app layout. Project settings asks both questions (the link dialog, then the unlink dialog) before changing anything, and the link dialog now lists the user's own connections, shared ones included, so a relink re-shares them.
- **Fixtures.** `unlink/imported-connections` and `unlink/repo-still-used` record an unlink without `removeImported`; the recording's GUI removed the template connections, so both replays send `removeImported: true` for such a call, and since each seeded connection's link has no origin the expected rows don't change. `changes.json` (both entries' `why`) and the fixtures README's Corrections say so; no recording changed.
- **Q32.** A connection deep link in a directory no local project links opens the shared-project import dialog with that directory ticked (`openWithResults(path, [project], {onImported})`); `importSelectedProjects` (the dialog's Import, now in `services/shared-project-import.ts`) imports, closes the dialog and then runs `onImported`, which opens the connection (`connectionTabs.open`, the reconnect form, since an imported connection has no user name or password). Cancelling resets the store, callback included, so nothing runs.
- **I2.** The sidebar's indicator is `isSharedConnection(c)` (`!!c.sharedConnectionId`, `sidebar/manage/share-link.ts`), and the switch sends `isLocalOnly: !!sharedConnectionId`, so one click shares an unshared connection. The 5d-1 TypeScript library replay skips `local-only/toggle-off-and-on#1` (a second click on an unlinked connection flipped it back to local-only; the switch no longer does that). The Rust replays send the recorded `connectionUpdate` and still cover it. 133 steps replayed.
- **Minor fixes.** (1) The replaced values' field names are `shared_field_*` keys. (2) `import_problem_invalid_port` is "Port isn't valid". (3) `shared_notice_name_taken` says the content synced and only the name was withheld. (4) Project settings derives the repo path first; the scan runs only when that string changes. (5) A connection link opens the connection whose `templatePath(sharedConnectionId)` is the link's path, among the project's connections. (6) `deep-link.ts` has no English left (`deep_link_*` keys). (7) `linkProject` and `importProjects` start the background refresh (`ensureBackgroundRefresh`, `refreshing`). (8) The import dialog marks directories a local project already links ("Already linked here"), unticked and not tickable; Import from repo with every directory linked says so instead of making "Team (2)". (9) `pullRepo` answers `"updated" | "conflicted" | "unchanged"`; the sync button says "updated" only for the first, and "Sync all" stops at a conflict. (10) The connection import dialog shows a failed import with `errorToast`.
- **Follow-up added:** a sync's row changes announced by Core with no origin, so the feed applies them (in "Follow-ups", Left).
- **Tests seen failing first:** Core `unlink_keeps_own_connections_and_secrets_and_removes_imported_only_when_confirmed`, `unlink_without_confirming_keeps_every_connection_unlinked` and `a_failed_relink_after_an_unlink_keeps_the_users_connections` didn't compile (3 arguments, no `kept_connection_ids`); storage `a_connections_origin_follows_its_link` didn't compile (no `set_origin`, `origin`, `shared_origin`). In `shared-gui.svelte.test.ts` 8 failed: the three unlink tests, the switch, the pull, the background refresh, the import dialog's linked directories, and the existing unlink test (new arguments). In `deep-link.test.ts` 3 failed (the template-path match, the Q32 import, the cancel). Written after the code: the replaced-labels test (minor 1); the rpc wire expectation (`keptConnectionIds`) and the storage version asserts (`[1, 2, 3, 4, 5]`) were updated when they failed against the change.

### Re-review fixes for Task 7 (as built)

2026-10-01, about 0.6 h, much of it builds.

- **I1 (Important, predates the review round).** A ticked connection that was local-only was never shared: `plan_publish` skips a local-only row, and the link's share loop never cleared the flag. So a wizard connection (local-only by default) ticked in the link dialog was silently left out, and after a Q31 unlink a relink shared none of the kept connections. `shared_link_project` now sets `is_local_only = 0` (its own transaction and `connection` event) on each ticked connection that isn't linked yet, before `publish_locked`: the tick is the explicit share (Q30).
- **Minor 1.** Core answers the unlink dialog's list: `Workspace::shared_unlink_preview` / `shared.unlinkPreview {projectId}` (read only, `deny_unknown_fields`, method-only `Debug`, `UnlinkPreview {importedConnectionIds}` generated). It and the unlink share one rule, `unlink_class` (linked to a template of this project's repo and directory; `exported` is the user's own, anything else imported), so they can't drift. `ProjectManager.importedConnectionsOf` is now async and reads it.
- **Minor 2.** The unlink dialog says that linking the same repo again later brings the teammates' connections back as new copies (`shared_unlink_relink_note`).
- **Minor 3.** `a_dev_file_at_0004_is_refused_read_only_and_upgraded_by_a_writable_open`: a file at `0004` only (a dev database from Tasks 3–6) is refused read-only with `STORAGE_NEEDS_UPGRADE` and unchanged, and a writable open adds `shared_origin`, NULL. Such databases' exported links have a NULL origin, so an unlink counts them as imported (and asks) until they're shared again; the note is in the migrations README rather than in `0005`'s comment, since editing the file would change its checksum.
- **Minor 4.** `import-shared-project-dialog.svelte` is mounted once, in the app layout (the header and the getting-started tab each mounted one before; the deep link opens it too).
- **Tests seen failing first:** Core `linking_shares_a_ticked_local_only_connection` and `a_relink_to_another_repo_exports_a_kept_connection` (no template written); Core `unlink_preview_lists_what_unlink_would_remove` and rpc `new_requests_round_trip_byte_for_byte` and `every_shared_and_imports_method_answers_with_its_own_name` (no method); GUI `the dialog lists the imported connections; cancel does nothing` and `lists only what Core says it would remove` (no `unlinkPreview` call). The storage dev-file test passed on its first run: it pins behaviour the migrator already had.

## Task 8: Probe

A separate agent, on a desktop dev build against real repos (no web: both groups are refused there, which it checks once), records evidence for each:

- **Scale.** A repo with 5,000 query files in nested folders, 500 dashboards of 1 MiB and 200 templates across 5 projects: time a sync with nothing changed, with 100 files changed, and the first sync of an upgraded file whose rows have no base; record memory and the size of the answers.
- **Hostile repo.** Symlinks to `~/.ssh/id_ed25519` as a `.sql` file and as a `queries/` directory; a 1 GB file; a file that isn't UTF-8; names with `..`, a backslash, `CON`, a trailing dot; 50,000 empty files; frontmatter of 10 MB. Nothing outside the repo is read or written, and each is named in the notice.
- **Two writers.** A second clone of the same remote edits, renames and deletes files and pushes while the app edits the same queries and dashboards and pulls: after each round, rows and files agree, Q20's rule held, nothing was lost (each side's content is in a version or in git).
- **Older releases.** 2026.9.x on the same repo (a second data dir) edits a query and a dashboard, renames one, and reads everything 5e wrote, single-quoted and Unicode names included; then 5e reads what it wrote.
- **Windows line endings.** A clone with `core.autocrlf=true`: every file pairs, and the app's writes don't flip line endings in `git status` more than today.
- **Pull safety.** Uncommitted shared edits, then a fast-forward pull: refused with the paths; after a commit, the pull merges.
- **Imports.** Real TablePlus and DBeaver files (the owner's, copied) and synthetic ones of 5,000 entries; two windows importing at once.
- **Leaks.** No path, name, host or file content in the app's log.

Probe fixes are budgeted separately.

### Probe fixes (as built)

- **1. A relink adopts the user's own templates.** An unlink that keeps a connection keeps its `shared_file_id`. Before publishing, the first link's share loop looks at the templates under `connections/` that no connection claims, and pairs a ticked connection with one: by the kept file id, else by `name_key` and type when exactly one template matches (Decision 34's rules). It sets the link and base from the file and marks it `exported`, and writes nothing. In the probe, a relink with the same ticks writes no file and gives no `unpaired`, and a second install imports each template once (before: `own1-2.yaml` and `later-2.yaml`, so teammates got "Own1 (2)"). A teammate's connection the user didn't tick stays local (Decision 53); its template is imported beside it with a `nameTaken` notice, as before. Test: `a_relink_adopts_the_users_own_templates`.
- **2. Legacy double quotes.** `shared/format.rs` picks the quoting per file: a query or template with Core's `id:` line undoes `\"`, `\\` and `\n` inside double quotes, while one without an id reads as 2026.9.2 does (double quotes stripped, the inside literal). `project.yaml` and `labels.yaml` keep the escape rule, since they carry no id and Core's own escaped values there must round-trip. The three-way hashes use Core's form for both kinds of file, so they agree. Decision 45 is corrected. The three `parse-query` entries in `changes.json` now expect the recorded output (Corrections in the fixtures README). In the probe, 9.2's edit of `C:\new: path` now reads exactly; before, it made a duplicate row with a newline in its name. 9.2 also left Core's file there, so the edit is `unpaired` (one file per row) and isn't applied. Tests: `a_920_double_quoted_value_reads_literally`, `a_core_written_value_with_an_id_round_trips`, `hashes_agree_for_core_and_legacy_files`, and the format replay re-reading what Core writes with an id.
- **3.** Decision 52 names the known limits: a value with both `'` and `"`, and a tag or label with `,`, don't survive a 2026.9.2 teammate. No code.
- **4. A sync plans outside the write lock.** The sync reads the project's rows and link columns through the pool and plans. Then, in a short `WriteTx`, it checks a `PlanStamp` before writing the plan. The stamp is every link column of the three kinds, plus the full row of each row the plan touches. Per-row stamps rather than the change sequence: a write to another project or kind advances the sequence and would force a re-plan for nothing, and the stamp compares only what the plan read. If the stamp differs, or a planned write is refused, the sync plans again; after two optimistic tries (`OPTIMISTIC_PLANS`) it plans inside the transaction, as before. A `doc(hidden)` `CoreBuilder::sync_plan_hook` lets the race test edit a row between planning and the check. In the probe's scale run, during `syncRepo`, an unrelated library write waited 1,290–1,301 ms before and 1–2 ms after (release), 15,369 ms before and 2 ms after (debug). "Before" is the same build with `OPTIMISTIC_PLANS = 0`. Sync times are unchanged (7.5 s `syncRepo` in release, 78 s in debug). An edit of a *shared* query during the sync still answers after about 1.3 s (release) or 15.4 s (debug): its publish waits for the repo lock, which the sync holds for one project at a time (Decision 36). Tests: `an_unrelated_write_isnt_held_by_a_large_sync` (6,000 queries, 50 changed by a teammate: worst unrelated write 22 ms after, 418 ms with optimistic planning off) and `a_racing_edit_is_never_overwritten_by_a_stale_plan` (fails with the stamp check disabled).
- **5. Keeping a deletion deletes the file.** `conflictContent` adds `oursDeleted`/`theirsDeleted`. `resolveConflict` takes `delete: true`, which `seaquel-git`'s `resolve_conflict_deleted` serves (the file and its conflict entries go, and the deletion is staged); Core's `shared_git_resolve` takes `Option<&str>`. The dialog sends `null` for a side that deleted the file (`conflict-choice.ts`) and shows "(No remote version)". Conflicted paths now include files with no `our` entry. Tests: `keeping_their_deletion_deletes_the_file` (git), `keeping_a_deletion_over_the_rpc_deletes_the_file`, `resolving_by_deletion_deletes_the_file`, `conflict-choice.test.ts`, and the git service's wire case.
- **6. A refused merge names the paths.** When the merge path's `repo.merge` refuses over local changes, the pull answers `PULL_ERROR` with the fast-forward path's message and JSON-quoted paths. The paths are the working tree's changed files that the incoming side changes since the merge base. Test: `a_refused_merge_names_the_paths`.
- **7. A project skipped whole is said on every sync.** `SyncReport.skippedProjects` (`{projectId, why}`) names a project the scan skipped whole (`tooMany`, `tooLarge`), and that sync's notices bypass `NoticeMemory`. The GUI keeps `state.sharedSyncSkipped` (set from each sync of a project, cleared when a sync reads it), and project settings show a "Skipped" badge with a tooltip. Tests: `a_project_past_the_bounds_is_named_on_every_sync` and the GUI's skipped-project case. The probe's 270 MiB project now carries the notice on its second sync too.
- **8.** `importProjects` reports a directory a project here already links as a per-directory failure with `PROJECT_ALREADY_LINKED`, matching the GUI's per-folder failures (`failureText` words it). The other directories asked for are still imported. The no-`git` warnings are gone: `connection_order_in` and `insert_linked_project_in` are `cfg(feature = "git")`, and `Publish`'s unread fields are allowed. `cargo build -p seaquel-server` is clean. `tree::project_dirs_listing` names a symlinked entry under `.seaquel/projects`, `RepoPreview.skippedDirs` carries it, and the import says it. Tests: `importing_an_already_linked_directory_is_refused`, `a_symlinked_project_dir_is_named_as_skipped`, the `tree` listing case, and two GUI cases. Two existing tests changed on purpose: the rpc round trip no longer imports the linked directory twice, and the `conflictContent` wire shape gained the two flags.
- **Review fixes (A1–A4).**
  - A1: an adopted template becomes the base only when the row's own template hash already equals the file's. Otherwise the link has no base, and the link's sync applies Q27: the template wins, the conflict notice lists the replaced values, and nothing is pushed. This holds for adoption by name and by kept file id, so a teammate's edit made after the unlink survives the relink. Tests: `adopting_a_teammates_template_by_name_pushes_nothing` and `a_relink_keeps_a_teammates_edit_made_after_the_unlink`. Both failed first, with the user's values written over the teammate's file.
  - A2: `resolve_conflict` and `resolve_conflict_deleted` accept only a path the index holds as conflicted. It must be relative, with no `..`, backslash or NUL, and no symlink at any component (`tree::check_resolvable` over `check_no_symlink`). A text resolution is written through `write_atomic`'s no-follow temp file. Anything else is `CONFLICT_ERROR` and nothing is touched. Test: `resolving_refuses_paths_outside_the_conflict`. It failed first, and the old code had already written `../outside/q.sql` before staging refused it.
  - A3: Decision 52 names the `'` plus `\` case.
  - A4: the `PlanStamp` doc says what it compares, and that a new row column must be added to it.
- **Also:** the 5d-1 library replay didn't drop `0005`'s `shared_origin` column, so it failed on every connection row; `LINK_COLUMNS` now lists it.

## Task 9: Docs, measurement, checkpoint

- **CLAUDE.md:** the `shared` and `imports` groups, `LocalFiles`, the projection's rules (links, the three-way sync, when it runs, publishing from library calls, the repo lock, the safe pull, file rules), `0004`, the formats' fixes, the retired storage methods, `SharedService`, the main window's capability, and the lines that describe the TS projection (Phase 5d's "Shared projects and git stay in TypeScript" wherever it appears).
- **Design doc:** the status line, "As built in phase 5e", "Phase 5e cost", and phase 5 marked done.
- **Migrations README:** `0004`.
- **This plan:** execution notes, release notes (edits to shared dashboards and connections reach the repo; pulled changes appear at once; conflicts are kept; file names; the project rename; the CLI refusing the file again until the app opened it; the pull refusing to overwrite uncommitted changes), checkpoint, manual checks.
- **Effort log:** rows and totals. **The full check list**, with one full live run.

**Status (Task 9):** done. CLAUDE.md (Core's `LocalFiles`, shared projects and imports, the `shared` and `imports` groups, `0004`/`0005`, `seaquel-git`'s safe pull and `tree`, the CLI, the desktop's capability, the web refusal, and a new "Shared projects and imports in the GUI" section; the TypeScript reconcile and import lines are gone, the storage group is 13 methods), the migrations README (`0004` now says an unlink keeps the template's file id), the library fixtures README (`files` and the 11 cases no longer replayed in TypeScript), the 5d plan's follow-ups (five items marked closed in 5e), two stale comments in `load-guard.ts` and `storage/client.ts`, the design doc's status line, "As built in phase 5e" and "Phase 5e cost", and the sections below are written. The full check list ran with one live run (see "Checkpoint"). It found one build break, fixed here: `seaquel-cli` no longer compiled on its own (below). The owner's manual checks are pending.

---

## Manual checks

For the owner, after Task 9. Desktop only: web and the demo have no shared projects and no imports. **No agent ran the desktop GUI in phase 5e**, so these checks are the only test of the dialogs, the sync button and the deep links in a real window. Each says what to do and what you should see.

**Setup.**
- Data dir: `D="$HOME/Library/Application Support/app.seaquel.desktop.dev"`. **Before the first launch of this build**, back it up: `cp -R "$D" /tmp/sq-5d2`. Read the file with `sqlite3 "$D/seaquel.db" "…"` (fine while the app runs).
- A team remote and two clones, B playing the teammate:
  ```sh
  git init --bare /tmp/team.git
  git clone /tmp/team.git /tmp/b && git -C /tmp/b commit --allow-empty -m init && git -C /tmp/b push origin HEAD
  git clone /tmp/team.git /tmp/a
  ```
  To make a teammate's change: edit files under `/tmp/b/.seaquel/…` by hand, then `git -C /tmp/b add -A && git -C /tmp/b commit -m change && git -C /tmp/b push`. Run `git -C /tmp/b pull` before each change, since the app pushes too.
- The sidecar: `npm run cli:build`; the app: `npm run tauri dev`.
- Useful reads: `sqlite3 "$D/seaquel.db" "select id, name, git_repo_path, shared_dir from projects"`, `"select name, shared, shared_path, shared_base is not null from saved_queries where project_id = '<id>'"`, `"select name, is_local_only, shared_connection_id, shared_origin from connections where project_id = '<id>'"`.

**CLI and MCP** (first: they need a file this build hasn't opened)
- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5d2 src-tauri/binaries/seaquel-cli-aarch64-apple-darwin mcp </dev/null` fails with "… Open the Seaquel app once …" (`STORAGE_NEEDS_UPGRADE`).
- [ ] `SEAQUEL_DATA_DIR=/tmp/sq-5d2 npm run tauri dev`, wait for the app to load, quit. `sqlite3 /tmp/sq-5d2/seaquel.db "select version from _sqlx_migrations"` prints 1 to 5, and the same CLI command now starts and exits without an error.

**Linking (Q30)**
- [ ] Make a project with three connections: "Own1" (made in the wizard, so local-only), "Own2", and "Old" set as a pre-5e row: `sqlite3 "$D/seaquel.db" "update connections set is_local_only = 0, shared_connection_id = null where name in ('Own2', 'Old')"`. In project settings → Team Sharing, set the Git Directory to `/tmp/a` and save. The link dialog opens and lists all three: Own2 and Old ticked, Own1 unticked and marked "Local only", with the note that unticked connections stay on this computer. Untick Old, tick Own1, and press "Link and share 2".
- [ ] `ls /tmp/a/.seaquel/projects/*/connections` shows two files (Own1, Own2) and no Old; each holds `id:`, host, port and database but no `username` or password. `project.yaml` names the project. The connection list has no "(2)" copies. In SQLite, Own1 and Own2 have `shared_origin` `exported` and a `shared_connection_id`; Old has neither.
- [ ] Edit Old's host and save: `git -C /tmp/a status` shows nothing new (Decision 53). Flip Old's local-only switch in the sidebar once: its template appears and its row gets `exported`.
- [ ] Cancel the link dialog on another project: nothing changes (no `git_repo_path`, no files).
- [ ] Link a second project to `/tmp/a`: it gets its own directory under `.seaquel/projects/`. Linking the first project to another folder, without clearing the path first, answers "This project is linked to another repository. Remove that link first."

**Queries (Q21, Q22)**
- [ ] Share a query named "Отчёт", one named `Sales "Q3"`, and two named "Sales" and "Sales!" (in one folder). `ls /tmp/a/.seaquel/projects/<dir>/queries` shows `отчёт.sql`, `sales-q3.sql`, `sales.sql` and `sales-2.sql`; `head` of each shows `id:` and the name as typed (`Sales "Q3"` in single quotes).
- [ ] Edit a shared query's text and save: the file changes at once. Rename it: one file, under the new name, with the same `id:` line. Rename it in case only ("sales" → "SALES"): the file keeps its path.
- [ ] Unshare one: its file is gone. Delete another: its file is gone.
- [ ] In B, change a query's text and push; in the app press Pull on the sync button: the text changes without switching projects, and the version history has the previous text.
- [ ] Change the same query in both (save in the app, then B pushes another text): after Pull the app shows B's text, yours is in the version history, and a toast says it "changed here and in the repository" (Q20).
- [ ] In B, rename a query's file and its `name:`, keeping the `id:` line, and push. After Pull the same row has the new name (no second query, nothing unshared).
- [ ] In B, delete a query's file and push. After Pull a toast says it "was removed from the repository by a teammate"; the query stays, unshared.
- [ ] In B, add `queries/teammate.sql` (frontmatter `name: From B`) and push. After Pull "From B" appears, shared. In B, add a file whose `name:` is the name of a local query that isn't shared: a toast names the file and says the name is already used here; it says so once per session.

**Dashboards (Q24)**
- [ ] Share a dashboard: `dashboards/<name>.json` appears with an `"id"`. Add a widget, move it, resize it: the file changes each time. Pan and zoom only: `git -C /tmp/a status` shows no change. Switch projects and back: the widget is where you left it.
- [ ] Rename it: one file under the new name. Unshare it: the file goes. Share it again, then delete it: the file goes.
- [ ] In B, change a shared dashboard's widgets and push; Pull: the dashboard shows B's widgets, and its version history has your previous state.

**Connection templates (Q23, Q27–Q29)**
- [ ] In B, change a template's `host:` and push; Pull: the linked connection's host follows, and its user name and saved password stay.
- [ ] Change the same connection's port in the app and its port in B; Pull: the connection takes B's port, and a toast lists the value it replaced ("It replaced port …"), with no user name or password in it.
- [ ] In B, change a template's `type:` (postgres → mysql) and push; Pull: a toast says the template now names a mysql database, the old connection stays as a local connection, and a new connection named like it with "(2)" appears from the template.
- [ ] Switch to a project with no Git Directory and save one of its connections: `git -C /tmp/a status` shows nothing new.

**Unlinking (Q31)**
- [ ] Let B add a template the app imports (a teammate's connection). In project settings, press "Remove Git Directory" and save. The unlink dialog lists only the teammate's connection and says your own stay with their passwords, and that linking again later brings teammates' connections back as new copies. Press Cancel: nothing changes. Do it again and press "Keep them": every connection stays, local-only, with no `shared_connection_id`; Own1's saved password still connects. Link again and unlink with "Remove them": only the teammate's connection goes.
- [ ] Relink the project to `/tmp/a` with Own1 and Own2 ticked: no new file appears under `connections/` (the templates are adopted), and `git -C /tmp/a status` is clean.

**Pull safety, conflicts, "Sync all" (Decisions 35 and 38)**
- [ ] Edit a shared query in the app (don't commit), then let B change a different file and push. Press Pull: it's refused with "Nothing was pulled, so your uncommitted changes are kept. Commit or discard your changes to … first", naming the file; the edit is still in the file. Commit, then Pull: it merges.
- [ ] With uncommitted shared edits, press "Sync All": the commit dialog opens first; after you commit, it pulls and pushes.
- [ ] Change the same query on both sides and commit both; Pull: the conflict dialog opens with the file. The query row doesn't take the conflict markers, and every shared dashboard stays shared. Keep your version, press "Finish & Commit": the app syncs and the query shows your text.
- [ ] In B, delete a file the app also changed, and push; Pull and conflict: the remote side shows "(No remote version)". Keep their version: the file is gone after the commit, and no empty query appears.

**Hostile repo files (Decision 32)**
- [ ] `ln -s ~/.ssh/known_hosts /tmp/a/.seaquel/projects/<dir>/queries/x.sql`, then switch to another project and back: no query "x" appears, and a toast says `…/x.sql` was skipped because it's a symbolic link. Remove the link.
- [ ] `ln -s /tmp /tmp/a/.seaquel/projects/evil` and open "Import from Git repo" on `/tmp/a`: the dialog says the folder `evil` can't be imported. Remove the link.
- [ ] Put 20,001 empty `.sql` files in a project's `queries/` (`for i in $(seq 20001); do : > /tmp/a/.seaquel/projects/<dir>/queries/f$i.sql; done`) and switch to the project: nothing changes, project settings show a "Skipped" badge with its tooltip, and each later sync says so again. Remove the files: the badge goes at the next sync.

**Import from Git repo (Q32)**
- [ ] On a second data dir (`SEAQUEL_DATA_DIR=/tmp/sq-other npm run tauri dev`), clone the remote and use the header's "Import from Git repo" on the clone: the dialog lists the projects with their counts; import one: its queries, dashboards and templates appear (connections without user names). Opening the dialog again marks that directory "Already linked here" and it can't be ticked; with every directory linked it says "Every project in this repository is already linked here."
- [ ] In that second app, copy a connection's share link from a project you haven't imported and open it as a deep link: the import dialog opens with that directory ticked. Cancel: nothing changes. Open it again and import: the connection's form opens after the import.
- [ ] Open a query's share link whose file isn't stored here yet: a toast says it isn't in Seaquel yet and to pull first.

**TablePlus and DBeaver (Decision 47)**
- [ ] Getting started → "Import from TablePlus" (on a Mac with TablePlus) and "Import from DBeaver": the dialog lists the connections; one already saved shows as a duplicate, one with an unreadable port is listed with "Port isn't valid" and can't be ticked. Import two: they appear, local-only, at the end of the connection list. Import again: nothing new.
- [ ] Move the config away (`mv ~/Library/Application\ Support/DBeaverData/workspace6/General/.dbeaver/data-sources.json /tmp/`): the button says "No DBeaver connections found". Put a file that isn't JSON there: it says it couldn't read DBeaver's connections. Move the original back.

**The log and older releases**
- [ ] `grep -rn "/tmp/a\|Отчёт\|known_hosts" ~/Library/Logs/app.seaquel.desktop.dev/` finds nothing.
- [ ] Quit the dev app. Back up the release data dir (`R="$HOME/Library/Application Support/app.seaquel.desktop"`, `cp -R "$R" /tmp/sq-release`), copy `$D/seaquel.db` over `$R/seaquel.db` (remove `$R/seaquel.db-wal` and `-shm` first) and open the installed 2026.9.x app on the linked project: the queries and dashboards show. Edit a shared query's text there and quit. Put the dev data back (`cp $R/seaquel.db $D/seaquel.db`) and start this build: at the project's next sync the edit reaches the file. Put `/tmp/sq-release` back.

---

## Execution notes

Executed task by task with subagents, Tasks 1–3 and 4a/4b overlapping, a review after each task (two rounds for Tasks 1, 2, 4b, 5 and 7), a probe on a desktop-like Core harness, one round of probe fixes with a review, and this checkpoint. Where the result departs from the text above, the repo is authoritative. Per-task times are in `2026-10-05-phase-5e-effort.md`; the measured cost is in the design doc ("Phase 5e cost").

**What went differently from the plan**

- **The slice logged ~28.2 h against "expect about 18.8 h"** (range ~15–22.8 h): first passes ~14.35 h, review fixes ~10.1 h (about 72% of first passes), probe fixes ~3.5 h, the Q30 re-recording ~0.2 h.
- **The pure planner and Core ran far over their first passes.** Task 4b took ~2.1 h (estimate 0.8–1.1 h) and Task 5 ~4.5 h (1.3–1.7 h). The planner gained the library's own checks, the withheld-name rule, `expect_hash` and indexed pairing; Core's replay needed real repos, clones for each pull, real symlinks and a beta-era file. About half of each was builds on the shared target.
- **Owner answers arrived mid-way and changed expectations.** Q27–Q29 came in Task 2's review, Q30 after its re-review (the first link's export reversed twice), Q31 and Q32 in Task 7's review. Each meant re-recording or a new migration, not a rewrite.
- **`0005`, not an edit of `0004`.** Q31 needed each link's origin. `0004` hadn't shipped, but dev databases had applied it since Task 3 and sqlx refuses a changed checksum, so the origin went into a second migration. Dev databases from Tasks 3–6 read their links' origin as unknown (migrations README).
- **The wire grew beyond the sketch:** `unlinkPreview`, `removeImported` on unlink, `ImportedProjects {projectIds, failures}` instead of a list of ids, `SyncReport.failures` and `skippedProjects`, `RepoPreview.skippedDirs`, `REPO_IN_USE`, `PROJECT_ALREADY_LINKED` and `FILE_CHANGED`. `git.resolveConflict` takes `delete`, and `conflictContent` says which side deleted the file.
- **Stack depth.** A library call that publishes, finds a teammate's change and syncs inlined deep futures; on a 2 MiB thread it overflowed. The publish, sync and row-op helpers and the rpc library and shared arms are boxed (Task 5 review and re-review), and a test runs every such call on a 2 MiB thread with 768 KiB reserved.
- **Quoting is per file** (probe fix 2). Decision 45 assumed 2026.9.2 never wrote a backslash inside double quotes; it did. A file without Core's `id:` line is now read as 2026.9.2 reads it.
- **The sync plans outside the write lock** (probe fix 4). A large project's sync held storage's write lock for about 1.3 s in a release build (15 s in debug), so every library write waited. It now plans first and writes under a `PlanStamp` check.
- **A relink adopts the user's own templates** (probe fix 1), so an unlink followed by a link writes no second copy of a template.
- **A connection deep link to an unlinked directory imports the project** (Q32), since Core has no single-template import; the deep-link project picker was deleted.
- **No proptest in the tree.** The parsers, readers and planner are fuzzed with seeded xorshift loops (20,000–50,000 rounds).
- **The checkpoint found a build break.** Probe fix 8 put `connection_order_in` behind `cfg(feature = "git")` to silence a no-`git` warning, but `imports` uses it too, and the CLI builds Core with `imports` and no `git`. `npm run cli:build` failed; every other build and test passed, because they unify features. Fixed here with `cfg(any(feature = "git", feature = "imports"))`; `seaquel-core` was then checked with clippy under `storage`, `storage,imports`, `storage,git`, `storage,git,imports` and the CLI's set, and `seaquel-cli`, `seaquel-mcp` and `seaquel-server` on their own. CI doesn't build the CLI alone (a follow-up).
- **The desktop app was never run by an agent.** The probe drove Core through `dispatch_workspace` and `dispatch_git` as the desktop builds it; the dialogs, the sync button and the deep links have only vitest coverage until the owner's manual checks.

**Decisions made during execution**

- **Owner:** Q27–Q32 ("Answered questions").
- **Coordinator, for the owner to overrule:** Decision 53 (only explicitly shared connections reach the repo); the withheld name and the name-only merge (Task 4b and its follow-up); `invalidSshPort` and `noName` as import problems (Task 4a review); `FILE_CHANGED` (Task 4b review, M1); a removal refused when its file can't be deleted, `REPO_IN_USE` and `PROJECT_ALREADY_LINKED` (Task 5 review I2–I4); no action on Task 5's re-review R4 and R7.

---

## Release notes

For the release after 5d-2's. Earlier notes still apply as written. Desktop only unless noted: shared projects and imports don't exist on web or in the demo.

Changes you may notice:

- **Edits to shared items reach the repository at once.** Saving, renaming, sharing, unsharing or deleting a shared query, dashboard or connection writes, moves or removes its file in the project's own repository right away. Dashboards were the worst case: their edits never reached the file before, and the next project switch put the file's version back. A dashboard's pan and zoom alone don't change the file.
- **Teammates' changes appear when you pull.** A pull, a commit and a conflict resolution sync every project linked to that repository, and so does the background refresh when the repository changes. You no longer need to switch projects or restart.
- **Both sides changed?** Seaquel tells your changes from a teammate's by what both sides had at the last sync. When both changed the same item, the repository's version is shown, yours is kept in the item's history, and a message says so. On the first sync after upgrading, an item that differs from its file is treated the same way, so a dashboard whose edits never reached its file shows the file's version and keeps yours in its history.
- **Connection templates stay in step.** A linked connection's host, port, database, SSL mode and SSH host and port follow its template; your user name, passwords, labels and AI settings stay yours. A template changed on both sides wins, and the message lists what it replaced. A template that now names another database type leaves your connection as a local one and comes in as a new connection.
- **Linking asks which connections to share.** The first time you link a project to a repository, a dialog lists its connections with checkboxes; local-only ones start unticked. Only ticked connections are written to the repository, as templates without user names or passwords. Connections saved before the local-only switch existed are no longer shared just because they weren't marked local-only.
- **Unlinking keeps your connections.** Removing a project's Git Directory keeps the connections you made, with their passwords, as local connections. If the repository brought you connections, a dialog asks whether to remove them or keep them. Linking the same repository again reuses your templates instead of writing copies.
- **Renaming a linked project** renames it in `project.yaml` only; its folder in the repository stays, so teammates' links keep working.
- **New file names keep their letters.** "Отчёт" is saved as `отчёт.sql`, not `untitled.sql`. Two items whose names would give one file get `-2`, `-3`. Existing files keep their names. Each file Seaquel writes carries an `id`, so a teammate's rename arrives as a rename, not as a new item next to an unshared one.
- **Pull no longer overwrites uncommitted changes.** A pull that would overwrite local changes stops and names the files: "Nothing was pulled, so your uncommitted changes are kept. Commit or discard your changes to … first." "Sync All" offers to commit first, then pulls and pushes, and stops at a conflict.
- **Conflicts open the conflict dialog.** It existed but nothing opened it. A conflicted pull or sync now opens it, nothing is synced while files are conflicted, and keeping the side that deleted a file deletes it.
- **Files that can't be trusted are skipped and named:** symbolic links, files that aren't UTF-8 text, files over 16 MiB, names Windows can't hold, and files that don't parse or hold values Seaquel would refuse. A project folder over 20,000 files or 256 MiB isn't synced at all, and its project settings show "Skipped" until it shrinks.
- **The import dialogs say what they found.** TablePlus and DBeaver imports now say when nothing was found or the file couldn't be read, list connections that can't be imported with the reason (a port or SSH port that isn't valid, no name, a missing or repeated ID), and name each failure. Two windows importing at once no longer import twice.
- **Import from Git repo** marks folders a project here already links and won't import them twice. A connection's share link to a project you haven't imported opens the import with that project ticked, then the connection.
- **New messages** say what happened to shared files: "Saved. The shared file couldn't be written: … The next sync writes it."; "A teammate changed this shared file, so it wasn't overwritten / deleted …"; that an item changed here and in the repository; that a teammate removed an item; that a file's name is already used here (naming whether a saved query, dashboard or connection has it); that a file was skipped, and why; that a project has merge conflicts; and for links and imports, that a project is already linked elsewhere, that a repository is still used, or that a link names an item not pulled yet.
- **The command line tool (`seaquel-cli mcp`) again needs the app to open your data once** after upgrading (two new migrations). Until then it stops with "Open the Seaquel app once…".
- **The first sync of a large repository takes a few seconds.** Measured on a test repository of 5,000 queries, 500 dashboards of 1 MiB and 200 templates in five projects (release build): importing it took 5.5 s, a sync of one ~100 MiB project with nothing changed 1.5 s, with 100 changed files 1.9 s, and syncing the whole repository after a pull 7.7 s, using up to about 650 MB of memory. The first sync after upgrading, with no recorded state, changed nothing there. While one project syncs, saving a shared item of that repository waits for it.

Working with teammates on 2026.9.x:

- **They read everything 5e writes**, Unicode file names and the new `id` included.
- **Some values don't survive their edits** (Decision 52): a value holding both `'` and `"`, or `'` and `\`, comes back with an extra backslash; a tag holding a comma is split; a description's line breaks show as `\n`. An item they edit is rewritten under their file name, beside Seaquel's file, and isn't applied until one of the two files goes (one file per item).

Known issues:

- `labels.yaml` and a template's `labels` are read but not applied to connections.
- Damage done by earlier releases stays: "<name> (2)" connections from earlier links, `untitled.sql` files holding one of several queries, and items unshared by earlier pulls or conflicts. The first sync names files it can't pair.
- In `project.yaml` and `labels.yaml`, a backslash typed by hand inside double quotes reads as an escape.
- The remaining file permissions of the main window still cover every path (for exports, the theme import and the ERD viewer).
- DBeaver's SSH settings, URL-only connections and projects other than `General` aren't read; TablePlus is read on macOS only.

---

## Checkpoint

The full check list, run on 2026-10-01 one step at a time on the shared `scratchpad/p5a/target`, npm through `mise exec`:

| Check | Result |
|---|---|
| `cargo test --workspace --exclude seaquel --features seaquel-runtime/tokio --no-fail-fast`, live (the `ci.yml` env, `SEAQUEL_TEST_REQUIRE_ENGINES=1`, `SEAQUEL_TEST_SSH`, the compose databases already up and seeded with `npm run e2e:db:seed -- postgresql mysql mariadb sqlserver duckdb`) | pass: 1,983 passed, 3 ignored, 0 failed, in 186 test targets (doc-tests included). About 5 minutes (18:53–18:58), 37 s of it compiling. The four MSSQL `tls_server_name` tests passed this time |
| `cargo test -p seaquel-core --features storage,workspace --test state` | pass: 38 |
| `npm run crates:check` | pass: 24 crates |
| `cargo fmt --all --check` | pass |
| CI clippy (`--workspace --exclude seaquel --all-targets --features seaquel-runtime/tokio -- -D warnings`) | pass |
| wasm32 clippy, pure crates (`seaquel-types`, `-runtime`, `-engine`, `-sql`, `-wasm`) | pass |
| wasm32 clippy, Core and `seaquel-rpc` with `seaquel-core/browser` | pass |
| Web server dependencies (the `ci.yml` step) | pass: none of the banned crates among 253 |
| `npm run types:gen` twice | pass: the 232 generated files didn't change on either run |
| `npm run check` | pass: 0 errors, 0 warnings (5,963 files) |
| `npx oxlint --type-aware --type-check --deny-warnings` | pass: 0 warnings, 0 errors in 645 files |
| `CI=1 npx vitest run` | pass: 1,998 tests in 115 files |
| `npm run build` | pass |
| `npm run build:web` | pass, with `NODE_OPTIONS=--max-old-space-size=12288` |
| `npm run build:demo` | pass |
| `npm run cli:build` | **failed**, then pass after the fix below |
| `cargo check -p seaquel`, `cargo clippy -p seaquel --all-targets -- -D warnings` | pass |
| `cargo test -p seaquel --lib` | pass: 46 |

**The one failure.** `npm run cli:build` stopped with `E0432: unresolved import crate::library::connection_order_in` in `seaquel-core/src/imports.rs`: the CLI builds Core with `imports` and without `git`, and probe fix 8 had gated the function on `git` alone. Fixed by gating it on either feature (`crates/seaquel-core/src/library.rs`). After the fix: `cli:build`, `cargo fmt --check`, the CI clippy, `cargo clippy -p seaquel-core -- -D warnings` under five feature sets (`storage`; `storage,imports`; `storage,git`; `storage,git,imports`; the CLI's `storage,secrets,ssh,workspace,imports`), clippy of `seaquel-cli`, `seaquel-mcp` and `seaquel-server` on their own, `cargo test -p seaquel-cli` (2 and 3), Core's `imports` (8) and `shared_cli` (1) tests, `cargo check -p seaquel` and `cargo test -p seaquel --lib` (46) all pass. The live run came before the fix; the change only affects builds without `git`, which the workspace run never makes. The earlier `cargo check -p seaquel` had passed against a sidecar binary left from before probe fix 8.

**Manual checks:** passed (the owner).

**Not run:** the release workflow and a signed build.

---

## Follow-ups

Picked up in 5e, and why:
- **The shared-repo projection in Core** (Q7 B) and **the import readers in Core** (Q8 B): the scope.
- **Shared dashboards and their files** (5d-2's execution) and **re-survey bug 25**: the same code; fixing them in TypeScript first would be thrown away.
- **Share, update and unshare write into the active project's repo**: the same code, and it leaks connection details (bug 5); Task 1 fixes it at once.
- **`nameToFilename`**: file names are the projection's.
- **The repo list saved whole** (bug 20): it is the storage group's last replace-all save, and the projection needs its repos addressable by id.
- **A safe fast-forward and no reconcile on a conflicted repo** (bugs 2, 3): not on the list, but the projection's correctness depends on them.
- **The `fs` permissions only the projection used** (bug 18).
- **Dead code** in the shared managers (bug 25 of the survey), since the files go.

Left, and why:
- **A sync's row changes announced without an origin** (Task 7 review). Core's events for a sync this page started carry the page's origin, so the change feed skips them and the page reads the project's rows itself (`UseDatabase.refreshProjectRows`, also after a `FILE_CHANGED` publish). If Core announced a sync's row writes with no origin, the feed would apply them like any other window's and the page's own re-read could go.
- **Task 5 re-review R4 and R7:** no action in 5e, at the coordinator's decision; recorded here so they aren't lost.
  - R4: Core's repo-lock map (`Core::repo_lock`, `git.rs`) never shrinks; one entry per repo path a session touched. Harmless on the desktop.
  - R7: the scan's sweep of stale `.seaquel-tmp` files guards only against this process. A second Seaquel process writing into the same repo could lose its temp file mid-write; that write fails (`Failed`), it never corrupts a file.
- **CI doesn't build `seaquel-cli` on its own** (checkpoint). Every CI build unifies features with `src-tauri`'s, so a Core item gated on `git` but used by `imports` broke only `cli:build` and the release job. A `cargo check -p seaquel-cli` step (and `-p seaquel-mcp`, `-p seaquel-server`) would catch the next one.
- **A shared edit waits for a large sync of its repo** (probe item 4): the publish takes the repo lock, which a sync holds for one project at a time (about 1.3 s for a 100 MiB project in a release build). Unrelated library writes no longer wait.
- **A 2026.9.2 teammate's edit of a file Core wrote** is written under 9.2's own file name next to Core's, and is `unpaired` until one of the two goes (one file per row, probe item 2). And Decision 52's quoting limits.
- **An imported project is visible before its sync finishes** to a window that reloads its projects for another reason (Task 5 re-review R3, narrowed, not closed).
- **The desktop GUI was never driven by an agent** in 5e. A scripted desktop run (or WebDriver over the Tauri window) would let a probe cover the dialogs.
- **Shared labels** (`labels.yaml`, a template's `labels`): what they should do is a product decision; the reader is ported (Decision 46).
- **DBeaver's SSH handlers, URL-only configurations and other projects; TablePlus on Windows**: new import features, not ports.
- **Narrowing the remaining `fs` permissions** (`"path": "**"` for writes, reads and removes): needs the exports and theme import moved to scoped paths or Core.
- **CLI commands** (Q26), **`data_version` polling**, and **the MCP server seeing new connections without a restart**: phase 7.
- **From 5d, unrelated to this scope:** a connection's labels as one whole value; the `db` group ignoring unknown fields; damaged and old-diff version history; `max_version_bytes` per query, not per file; the upgrade's events on web; non-UTF-8 strings keeping secrets; a failed web vault write after a save; the desktop's "Not receiving updates" badge without a test hook; a `unicase` update needing a `name_key` step; a widget snapping back mid-drag; `dashboardVersionsList` bounded by count; the late `pagehide` save; twelve `view: null` steps; two tabs setting up the vault; the license record and nudge as whole values; the dead code outside the shared managers; more desktop windows; field-level merging of whole values; the 5b and 5c leftovers.
- **Phase 8** deletes the demo's TypeScript twins; 5e adds none.
