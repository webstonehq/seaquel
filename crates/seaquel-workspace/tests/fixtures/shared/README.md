# shared fixtures

These files record what today's TypeScript does with shared projects: the `.seaquel` file formats (queries, dashboards, connection templates, `project.yaml`, `labels.yaml`), the file names, and the projection end to end (linking, importing and renaming projects, sharing and editing queries, dashboards and connections, the reconcile that reads the repo back, pulls). Phase 5e moves that work into Core (`seaquel_workspace::shared`, `seaquel-git`'s `tree`, Core's `shared.rs` and the `shared` RPC group), and these cases pin it the way `../library` and `../state` pinned 5d. See `docs/plans/2026-10-05-rust-core-phase-5e-plan.md`, "What the code shows", "Parity fixtures", Q20–Q26 and Decisions 29–52.

**The fixtures are frozen.** After 5e the projection runs in Core, and the TypeScript is deleted (Decision 48: there is no TypeScript twin, not even in the demo). Change a case only when Core is meant to behave differently, say why in `changes.json` and here, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-05-record-shared-fixtures.test.ts.txt`, a vitest file that writes these files and `../imports`. It ran on `cfc7294` plus the phase 5e working tree after Task 1 (with its review fixes and the N1 re-review fix) and Task 3. So it records Task 1's fixes as today's behaviour: shared dashboard edits written to the file, writes going to the row's own project's repo, no reconcile while the repo has conflicted files, and a reconcile that ignores a viewport-only difference. To rerun it, copy it to `src/lib/hooks/database/record-shared.test.ts`, run it with `FREEZE_SHARED=1` (and `SEAQUEL_WASM_PREBUILT=1` if `pkg/` is current), and delete the copy. `FREEZE_SHARED_OUT=<dir>` writes `<dir>/shared` and `<dir>/imports` instead. It needs a tree that still has the TypeScript projection, so before 5e Task 7.

Two runs gave byte-identical files. After the Task 2 re-review the set was recorded again: everything came out byte-identical except `repo/two-projects-one-repo-pull`, whose steps were meant to change, and the new case. After the Task 2 review the whole set was recorded again: every case from the first recording came out byte-identical and in its place, the review's cases follow them, and two runs matched again.

It runs the real code:

- the parsers and writers (`query-file-parser.ts`, `dashboard-file-parser.ts`, `config-file-parser.ts`, `yaml-utils.ts`), `nameToFilename`, `queryNameToFilename` and `dashboardNameToFilename`;
- the managers, wired as `UseDatabase` wires them: `ProjectManager`, `ConnectionManager`, `SavedQueryManager`, `DashboardManager`, `SharedRepoManager`, `SharedQueryManager`, `SharedDashboardManager`, `WindowStateManager` and `StateRestorationManager`, over the demo's `TsLibrary` and `TsUi` and its sql.js storage client on one in-memory sql.js file. `TsLibrary` keeps Core's library rules (5d), so the rows are what Core's library calls store.

Stubbed:

- **The disk.** `@tauri-apps/plugin-fs` and `@tauri-apps/api/path` are an in-memory disk with absolute `/`-separated paths. Its rules:
  - Entries are files, directories and symlinks. A symlink's target is absolute or relative to its directory, and may point outside the repo (`/home/user/.ssh/id_ed25519`, `/outside/queries`; fake content, no real secrets).
  - `readDir` reports each entry as tauri-plugin-fs 2.5.2 does (`DirEntry::file_type`, which doesn't follow a symlink): a symlink is `isSymlink`, neither a file nor a directory. Entries are listed sorted by name (UTF-16 order), which decides which of two files the reconcile sees last.
  - Every other call follows symlinks, as `std::fs` does: `readTextFile`, `exists` and `writeTextFile` read and write through them (bug 17), `readDir` lists a symlinked directory's target.
  - A directory listed in a seed's `unreadable` fails `readDir` with `EACCES` (bug 4); its files can still be read by name.
  - A file listed in a seed's `failing` fails `writeTextFile` with `EACCES` until a `disk.allowWrites` step.
  - `stat` always throws (`fs.stat not allowed`), as the main window's missing `fs:allow-stat` makes it (bug 19, confirmed by reading in Task 1). So no scanned file has an `updatedAt`, and the dashboard reconcile stamps the time of the reconcile (`<now>`).
  - A seed's `git` pull can be a queue: each pull takes the next one.
  - With a seed's `caseInsensitive`, a path matches an existing one that differs only in case and keeps its stored case, as on macOS and Windows.
- **Git.** `$lib/services/git` is a stub. A directory holding `.git` is a repo, and the seed's files under it are its committed snapshot. `getRepoStatus` counts the files that differ from the snapshot (modified, added or removed), and the case's `git` entry sets `aheadBy`, `behindBy`, conflicted files, a status that can't be read, and a pull: the files it changes (`null` deletes), committed by it, and the conflicts it leaves (with the markers in the files). A conflicted pull answers `success: false` and leaves the conflicted files in the status. `commitChanges` snapshots the working tree. `initRepo` makes `.git`. `getRemoteUrl` answers `null`.
- The keychain is unavailable (`TsLibrary` refuses secrets, like the demo). Toasts are captured. The logger and console are silenced. `crypto.randomUUID` is a counter. The clock is fake timers pinned at 2030-01-01T00:00:00Z. After each step the recorder lets pending promises finish and advances the clock past the 500 ms repo-list debounce (eight rounds of 600 ms), so fire-and-forget writes (a tab save's file write) and the repo-list save land in the step that started them.
- **The startup** (`page.open`) is `UseDatabase.initializeApp`'s part that matters here, copied: `projects.initialize`, `connections.initializePersistedConnections`, then `initializeSharedRepos` (load the stored repos, scan each, read its status, `activateLinkedRepo` for the active project). The background refresh isn't started; each tick is a `sharedRepos.refreshAllRepoStatuses` step.

## Normalising

- Ids made during a case are `<id:n>`, numbered in order of first appearance in the case's JSON (then its `changes.json` entry), keeping their prefix: `saved-<id:2>`, `dashboard-<id:3>`, `conn-<id:4>`, `project-<id:1>`, `repo-<id:1>`, `dver-<id:5>`, `ver-<id:6>`. Seeded ids (`p1`, `c1`, `q1`, `d1`, `repo-a`) stay.
- Times from the pinned clock are `<now>`. Seeded times (`2024-01-01T00:00:00.000Z`) stay.
- In `formats.json`, `NaN` would be `null` in JSON, so a value that is `NaN` is the string `"NaN"`; a field the TypeScript left `undefined` is absent.
- A file tree is `{absolute path: text}` for files and `{absolute path: {"symlink": target}}` for symlinks, sorted by path. Directories aren't recorded (git keeps none); a seed's tree may name one as `{"dir": true}`, and parent directories of seeded files exist.
- Rows are the raw SQLite rows (`0`/`1` for booleans, JSON as text) of `projects`, `connections`, `project_state` (only `project_id` and `connection_order`), `saved_queries`, `query_versions`, `dashboards`, `dashboard_versions`, `shared_repos` and `app_state` (only `activeRepoId`), each ordered by its key; a table with no rows is left out.

## formats.json

153 cases, each `{name, note?, fn, args, output, reparsed?}`. `output` is what today's function returns for `args`; a writer's case also has `reparsed`, its text read back by today's reader.

| `fn` | cases | runs | covers |
| --- | --- | --- | --- |
| `parseQuery` | 31 | `parseQueryFile(text, "repo-1", path, ".seaquel/projects/team/queries")` | every field; `default` and `defaultValue`; a parameter without a type or a name; empty values; no frontmatter (name from the file, `-` and `_` as spaces, `.SQL`); empty frontmatter; an empty body; a body holding `---`; CRLF; a BOM; tabs; conflict markers in the frontmatter and in the body; `:`, `#`, `"` (as today's writer escapes it), `'` (bare, and `''` inside single quotes), backslashes, edge spaces, Unicode; tags quoted with a comma and a single tag; a description with `\n` escaped and with a raw newline (as today's writer leaves it); a stable `id`; unknown keys and comments; a deep folder; an indented key that isn't top-level |
| `writeQuery` | 21 | `serializeQueryFile(query)`, then `parseQueryFile` | minimal and full; empty description, tags and parameters; `:`, `#`, `[`, edge spaces, `"`, `'`, both quotes, backslashes with and without quoting; a description with a newline; a tag holding `,` and one holding `"`; a parameter default with `"`; an empty default; a database holding `:` (written raw); Unicode; a body holding `---`; a body with edge whitespace; a body with `\r\n` |
| `parseDashboard` | 12 | `parseDashboardFile(text, "repo-1", path)` | valid; no name and an empty name (from the file); not JSON; conflict markers; `null`; an array; run state on widgets (stripped); no viewport or widgets; unknown keys; a BOM; a stable `id` |
| `writeDashboard` | 4 | `serializeDashboardFile`, then `parseDashboardFile` | full; empty description and no filter; run state; quotes, a newline and Unicode in the name |
| `parseTemplate` | 18 | `parseConnectionFile(text, "repo-1", "repo-1:.seaquel/projects/team", path)` | every field; `.yml`; credentials at both levels (dropped); a missing name; the default type, host and ports per engine; an unknown type; a port that isn't a number and one with trailing letters; `sshTunnel` with `enabled: false`, without `enabled` and with a bad port; comments and unknown keys; an empty SSL mode; CRLF; labels with a quoted comma; a stable `id` |
| `writeTemplate` | 5 | `serializeConnectionFile`, then `parseConnectionFile` | full; minimal; quotes and `:` in host, name and database; a label holding `,`; SSH disabled |
| `parseProject` | 6 | `parseProjectFile(text, "repo-1", dir)` | present; empty (named after the directory); description only; comments and an empty description; escaped quotes; CRLF |
| `writeProject` | 2 | `serializeProjectFile`, then `parseProjectFile` | name and description; a name with `"` |
| `parseLabels` | 4 | `parseLabelsFile(text)` | present; a missing colour and a missing name; comments and the list ending at a top-level key; none |
| `writeLabels` | 1 | `serializeLabelsFile`, then `parseLabelsFile` | two labels, one with `:` |
| `fileName` | 49 | `nameToFilename`, `queryNameToFilename`, `dashboardNameToFilename` | Latin, case pairs, punctuation only, `-`/`_`/spaces, `/` and `\`, dots, the empty name, emoji, `ß`, precomposed and decomposed `Ä`, Cyrillic, CJK, Hangul, Arabic, Devanagari (marks), Greek, a Roman numeral, Windows reserved names (`CON`, `con`, `Aux`, `nul`, `COM1`, `lpt9`, `con.sql`, `PRN`, `COM0`, `LPT0`, `COM¹`), 100 Latin characters, 100 CJK characters (300 bytes, cut to 249) |

For a parse case, `output` is today's object: `id` is the scan's `<repoId>:<path>` and `repoId` the repo's, which Core's `QueryFile`/`DashboardFile`/`TemplateFile` don't hold; the replay compares the rest (`filePath` is the relative path Core is given, `folder` the path below `queries/`). A writer case's `reparsed` leaves out `id`, `repoId` and `filePath` for queries and dashboards.

## projection.json

52 cases, 225 steps. Each case is `{name, note?, file, seed, secrets?, steps}`; each step is `{op, args?, core, outcome, library, rows, tree, toasts, view}`.

| field | meaning |
| --- | --- |
| `file` | `current` (a fresh file) or `v2026.4.5-beta.1` (`dashboards.project_id` without a foreign key) |
| `seed.rows` | rows inserted with plain `INSERT`s before step 0, in the order `projects`, `connections`, `project_state`, `saved_queries`, `query_versions`, `dashboards`, `dashboard_versions`, `shared_repos`, `app_state` |
| `seed.tree`, `unreadable`, `failing`, `caseInsensitive` | the disk before step 0 (see "How they were made") |
| `seed.git` | per repo path: `aheadBy`, `behindBy`, `conflictFiles`, `statusError`, `pull`, `remoteUrl` (see "Git") |
| `secrets` | the keychain before the case, for Core's `MemoryStore` (the recording has none) |
| `steps[].op`, `args` | the manager call and its input; `ref` names a query or dashboard by the label its create step gave it, its id or its name |
| `steps[].core` | the Core calls a GUI on Core sends for the step, in order, or `null` for none (below) |
| `steps[].outcome` | `{ok: true, value}` or `{ok: false, error}` for the manager call |
| `steps[].library` | every `LibraryService` call the page made, with its arguments, a create's new id (`made`) and a refusal's `error` |
| `steps[].rows`, `tree` | the tables and the whole disk after the step |
| `steps[].toasts` | `{kind, message}` |
| `steps[].view` | what the page showed: active project and repo, projects, connections, queries and dashboards per project, repos, the scanned shared projects and the scan cache. **No replay compares it** (there is no TypeScript twin; the GUI's own tests use a recording `SharedService`), and `changes.json` doesn't restate it |

Seeds: most cases use the standard seed: `p1` "Team" (active) linked to `/repos/a` (`repo-a`), whose directory is `.seaquel/projects/team` with `project.yaml` and empty `connections`, `queries` and `dashboards`, and `p2` "Solo" with no git path.

| group | cases | covers (bugs from the plan's list) |
| --- | --- | --- |
| `link/`, `unlink/` | 5 | the first link exporting an old connection (`is_local_only` 0) and not a new one, then the settings save importing it again as "Warehouse (2)" (6); a repo with two shared projects, both imported (7); unlinking with imported connections, their secrets and a local one; unlinking one of two projects that share a repo (today the repo is forgotten for both); a link leaving one connection unticked (Q30) |
| `import/` | 3 | a taken name ("Main (2)" looks at `main-2/`, 8); a `project.yaml` name that isn't its directory's slug (8); a missing `project.yaml` |
| `queries/` | 18 | share, edit from the tab, rename, case-only rename and unshare; delete; a share followed by a project switch before the next refresh (unshared: a new finding, below); two non-Latin names (10); "Sales" and "Sales!" (10); a name with `"` (14); a hand-written `my_query.sql` (10); a reconcile with a new, a changed and a removed file; two files with one name; a CRLF file (13); "STRASSE" next to "Straße" (15); a rename in a folder; a case-only rename on a case-insensitive disk; changed here (a failed write) and in the repo (Q20); a file renamed in the repo keeping its `id`, then rewritten without it by an older release, then moved (Q22); a stored path that isn't the slug path against a second file of the same name; a query typed with `\r\n` and trailing spaces (Decision 34's hash); a name colliding with a teammate's NFD file name |
| `dashboards/` | 9 | share, a widget edit and an activation (1, fixed by Task 1: before it, the edit wasn't written and the activation copied the old file over the row with no version, so it was lost); a pan (Q24); a rename and a case-only rename (11, fixed by Task 1); unshare and delete; a file that doesn't parse; a teammate's change pulled (12); sharing over a teammate's file of the same name (11); a beta-era file with a dashboard in no project; two 100-character CJK names, the second's `-2` past the 255-byte cap (Q21) |
| `connections/` | 10 | edit, rename and remove of a shared connection; the local-only toggle; a connection of an unlinked project after visiting a linked one (5, fixed by Task 1); templates changed, renamed and removed in the repo (16); a template changed here and in the repo (Q27); a template's `name:` changed, alone and after a local rename (Q28); a template's `type:` changed (Q28); a new template by pull (Q23); a connection in a linked project that isn't local-only but was never shared (Decision 53: today an edit writes its template) |
| `repo/` | 6 | a conflicted pull (3, fixed by Task 1); an unreadable directory (4); a symlinked `.sql` file and a symlinked `queries/` directory (17); a pull without an activation, then a background refresh and a commit (12); two projects linked to one repo and a pull changing both (12, Decision 35) |
| `project/` | 1 | a rename (9) |

Bugs with no case, and why: bug 2 (a fast-forward overwriting uncommitted files) lives in libgit2's checkout, which the stub doesn't run; Task 1 fixed it in `seaquel-git` and `crates/seaquel-git/tests/ops.rs` pins it. Bugs 18 (the webview's `fs` permissions), 23 (the import's duplicate check in the page, a race between two windows), 24 (a deep link to an unstored dashboard) and 25 (dead code) have no stored or file effect a single page can show. Bug 19 (`stat`) is the disk's rule above. Bug 20 (the repo list saved whole) shows as a `shared_repos` row rewritten after status refreshes; `changes.json`'s `*` says what Core is held to. Bug 21 (labels unread) has no effect to record; the label parser is in `formats.json`. Bug 22 is `../imports`.

There's no case that moves a query between folders: the GUI has no folder move today (a query's folder comes only from its file's path), so there's nothing to record. A rename inside a folder is `queries/rename-in-folder`.

### The Core calls

`core` is how a GUI on Core would call for the step (the plan's "The wire and the API"; `{group, method, params}`):

- **Authored** for the steps the projection decides: `page.open` and `projects.setActive` send `shared.sync {projectId}` when the project has a git path (Core resolves the repo itself; today's page may have lost it, as in `unlink/repo-still-used`); a background refresh tick sends `shared.syncRepo {repoId}` for each repo whose status showed a change (the ones today's tick rescanned); `projects.setGitRepoPath` is `shared.linkProject {projectId, path, share}` (`share`: the link dialog's ticked connections, Q30) or `shared.unlinkProject {projectId}`; the import dialog is `shared.scan {path}` then `shared.importProjects {path, dirs}`; a pull is `git.pull {path}` then `shared.syncRepo {repoId}`, a commit `git.commit {path, message}` then `shared.syncRepo`. `projects.importSharedConnections` (the settings save) and `disk.allowWrites` send nothing. The library writes these steps make today (the reconcile's creates and patches, the link's project update and template imports) are inside those Core calls, so they aren't listed.
- **The library writes** the page made, for every other step: each `TsLibrary` write as its `library` RPC (`savedQueryCreate {query}`, `savedQueryUpdate {id, patch}`, `dashboardUpdate {id, patch}`, `connectionUpdate {id, patch}`, `projectUpdate {id, patch}`, `projectSidebarSet {projectId, connectionOrder}`, …), in order. A create carries `binds`, the id the TypeScript made, which the replay binds to the id Core answers. In Core these calls publish (Decision 36).

## Replay rules

### Rust, pure (`seaquel-workspace/tests/{shared_formats,shared_plan}.rs`)

- **Formats:** every case, exactly, after `changes.json`: a parse case's `output` (without `id` and `repoId`), a writer case's `output` text byte for byte and `reparsed`, and a file name's `stem` (`file_stem`), with `.sql`/`.json` added.
- **Projection plans:** for each step that Core turns into a sync or a publish, the scan (the step's tree under the project's directory, with the skips Decision 32 makes) and the rows go through `plan_sync`/`plan_publish`, and the planned file and row changes must give the step's expected tree and rows.

### Rust, Core (`seaquel-core/tests/shared.rs`)

- **The file and disk:** `current` is a fresh `Storage`; `v2026.4.5-beta.1` loads `crates/seaquel-storage/tests/fixtures/schemas/v2026.4.5-beta.1.sql` and opens it through `Storage::open`. The seed rows go in with plain SQL. The tree is laid out under a temp directory (paths keep their absolute form below it), with real directories, files and symlinks, and a real `git init` in each repo with the seed committed, so the repo lock and the safe pull run for real. `unreadable` is a directory with mode `000`; `failing` is a path whose write must fail. A read-only file can't stand in, since a path that doesn't exist yet (`main-db.yaml` in `connections/template-renamed-after-a-local-rename`) has no mode to set, so the Core replay needs Task 5's test hook that fails a write by path, and fails every `failing` path through it. A case-insensitive case runs its paths through the comparison rule, not the disk (Task 5: "test both by comparing paths, not by the disk"). `secrets` go into a `MemoryStore`.
- **Steps:** each `core` call goes to its Core method, in order; `null` sends nothing. A pull applies the case's scripted `pull` files and commits them in a clone first, so `git.pull` fetches them (or leaves the scripted conflicts).
- **After each step**, the tree and the rows equal the step's (after `changes.json`), under `changes.json`'s `*` rules; `links`, `notices`, `projection` and `outcome` are compared where a change lists them.
- **The keychain** after each step holds `secrets` minus the entries of connections no longer in `rows.connections` (`connectionRemove` deletes them, as it does today).

## changes.json

`changes.json` lists every intended difference, keyed by case name, each with its Decision and why. Format cases give `expected.output` (and `reparsed` for writers). Projection cases give `expected.steps`, a map from step index to what Core's run must show there: `tree` and `rows` (whole, replacing the recorded ones; `app_state` is never in them, see `*`), `pathless` (shared rows allowed no `shared_path` after the step, see `*` (4)), `links` (stored link columns by table and row id, e.g. `projects.shared_dir`, `saved_queries.shared_path`, `saved_queries.shared_file_id`), `notices` (the sync's notices as a set), `projection` (the write's `Seqd.projection`) and `outcome` (the answer of the step's last Core call; only the fields given are compared). A step not listed is compared as recorded. Expected rows use `<core:n>` for an id only Core makes (any v4 uuid with that prefix, the same everywhere in the case).

The `*` entry holds the rules for every projection case:

1. **Ids and times.**
2. **Files.** A query's, dashboard's or template's stable id (Q22) is present once Core has written the file, a v4 uuid, kept across renames, and removed before comparing. Core writes it at the next real write of a file, never only to add it.
3. **The hashes (Decision 34).** A file hashes as `write(parse(text))` and a row as `write(parse(write(row)))`.
4. **Invariants after every step.**
   - Every row linked to a file that exists, wasn't skipped and isn't in a conflicted repo has `shared_base` = that file's hash and `shared_file_id` = its id, unless a value was withheld. A sync update from a file whose name another row holds keeps the row's name; the base is then the row's own hash, which differs from the file's only by that name.
   - After a step whose `core` calls all succeeded, every shared query and dashboard, and every explicitly shared connection (one that has a `shared_connection_id`; Decision 53), in a linked project whose directory is stored (`projects.shared_dir`, which Core stores at the project's first sync, link or import) has a non-NULL path (`shared_path`, or for a connection `shared_connection_id`). The share toggle stores the link even when the template write fails: the base stays unset, so the next sync writes the file. The only exceptions are the rows the step's `pathless` list names. A NULL path is not a link.
   - A name-only change from a teammate merges with a local content change: when the base is the file under the row's name and the row changed, the file takes the row's content under the teammate's name, and the next sync gives the row that name, with no conflict and no version (coordinator's decision after the Task 4b re-review).
5. **Rows.**
   - The 0004 link columns are dropped after (4) and `links`.
   - `shared_repos` compares `id`, `path` and `name` (Decision 43).
   - A repo call is `shared.linkProject`, `unlinkProject`, `importProjects`, `repoRegister`, `repoUpdate`, `repoRemove`, `git.pull` or `git.push`.
   - A seeded repo that no repo call names keeps its `data` byte for byte. One named only by a pull or push keeps it except `lastSyncAt`, which Core's pull and push write.
   - Expected rows hold only what the step's Core calls write. A `projectSidebarSet` the page made on a project's first activation, in a step whose `core` doesn't send it, isn't in them.
   - `app_state.activeRepoId` keeps its seeded value (Decision 42).
6. **Notices.** A step with a `shared` call gives exactly the notices listed, and none when none are listed. Each file is named at most once per session.

The notices are given in the plan's `SyncNotice` shape, with paths relative to `.seaquel/` (Decision 50):

- `{type: "conflict", kind, id, replaced?}`. `replaced` is for connections only (Q27): the local values the template overwrote, never a user name or secret.
- `{type: "removedInRepo", kind, id}`.
- `{type: "nameTaken", path, takenBy}`: a file that claims no row and whose name an unshared row has, or a file whose new name the sync withheld from its row because another row holds it.
- `{type: "unpaired", path, claims}`: a file that claims a row another file already has, which isn't imported (M1).
- `{type: "skipped", path, why: "symlink" | "unreadable" | "doesNotParse" | "invalid" | …}`. `invalid` is a file that parses but that the library's checks refuse (a parameter type, duplicate parameter names, a NUL, an unknown engine, a port outside 0–65535, a limit).
- `{type: "templateTypeChanged", kind, id, path, templateType, imported}` (Q28; `imported` is the id of the connection the same sync made from the template, Q29).

Task 4b may rename the serde tags. The contract is one notice per listed item.

Decision 45's quoting, as the fixtures pin it:

- A value that needs quotes and holds `"` or `\` but no `'` and no newline is single-quoted, which older readers strip exactly.
- Otherwise a value that needs quotes is double-quoted. Inside, `"` is written `\"` and a newline `\n`, and a `\` is written `\\` only where the reader would misread it: before `"`, `\`, `n` or a newline, or as the last character (Task 4b review). Every value is written this way, `database:`, a parameter's `type:`, a template's `type:` and `sslMode:` included.
- The reader undoes `\"`, `\\` and `\n` inside double quotes, and keeps any other backslash pair as written, only in a query or template file with Core's `id:` line. A file without one reads with 2026.9.2's rule: the double quotes are stripped and the inside is kept literally (probe fix 2, see Corrections).

99 entries plus `*`:

- **Formats (57).**
  - Decision 45 (24): CRLF and a BOM accepted (query, dashboard, template, project); the quoting above; a `,` quoted in a list and split only outside quotes; a description's newline as `\n`; a `\r\n` body written with `\n`.
  - Q21 (29): the file names that change under the new rule, the byte cap and the reserved names included.
  - Q22 (3): a file's stable id read as `fileId`.
  - Decision 45 as amended (1): `write-query/database-with-colon` (see Corrections).
- **Projection (42).**
  - Decision 40 (7): the first link exports and links the ticked connection (Q30); one directory's templates; an imported project keeps its directory (three cases); unlink clears the directory; an unlink keeps a repo another project uses.
  - Decision 34 (7): pairing by stored path (hand-written file, and against a second claim); notices for a changed and a removed file; one file per row (`unpaired` for the other); a name taken by folding; a file that doesn't parse changes nothing; the hash under `\r\n` and trailing spaces.
  - Decision 35 (5): no scan cache; a pull's sync applies teammates' changes at once (three cases, one with two projects); a conflicted repo answers `conflicted: true`.
  - Q21 (5): non-Latin names; "Sales!" as `sales-2.sql`; a share next to a teammate's file of the same name; the byte cap; an NFD name.
  - Decision 32 (3): an unreadable directory, a symlinked file and a symlinked directory are skipped, and the write is refused.
  - Q28 (3), Decision 45 (2), Decision 41 (2), Q23 (2), Q20, Q22, Q25, Q27, Q30 and Decision 53 (1 each).
- `queries/two-files-one-name` keeps the recorded rows: today's last-listed file happens to be the slug path Core pairs. The entry adds the notices.
- `connections/template-type-changed`: the same sync imports the retyped template as a new connection ("Warehouse (2)"), and the notice's `imported` names it (Q29, the owner's answer).

## Corrections

Changes made after the recording froze, each from a review of the task that replays them:

- **`queries/renamed-in-the-repo-keeps-its-id`, steps 1–5 (Task 4b review).** `q1.tags` is `"[]"`, not `null`: the row takes the file's whole content, as every other update from a file does (`repo/pull-without-activation`, `repo/two-projects-one-repo-pull`, `queries/changed-here-and-in-the-repo`, `queries/two-files-one-name`).
- **`*` (4a) and (4b) (Task 4b review).** (4a) allows a withheld name. (4b) covers only projects whose directory is stored, since a linked project no call has synced yet can't have paths (`repo/two-projects-one-repo-pull` #0, `unlink/repo-still-used` #0–#1).
- **`write-query/database-with-colon` (Task 4b review, I3).** `database:` is written with the quoting every value gets, so `pg:15` is `"pg:15"`.
- **`unlink/imported-connections` and `unlink/repo-still-used`, step 1 (Task 7 review, Q31).** `shared.unlinkProject` now takes `removeImported`, the user's answer to the unlink dialog. The recorded call has none: the recording's GUI removed every template connection, so the replays send `removeImported: true` (the user confirmed). Each case's seeded connection was linked by an older release (no `shared_origin`), which counts as imported, so the expected rows don't change. A connection shared from here (`exported`) is kept unlinked and local-only with its secrets; no recorded case has one, and `seaquel-core/tests/shared.rs` pins it.

- **`parse-query/double-quote-as-written-today`, `parse-query/backslashes` and `parse-query/description-escaped-newline` (5e probe, fix 2, the coordinator's decision).** These files have no `id:` line, so they were written by 2026.9.x or by hand, and Core now reads them as 2026.9.2 does: double quotes stripped, the inside literal. Each entry's expected output is now the recorded output (the recordings are unchanged), and its `why` says so. A file Core writes carries its `id` and keeps the escape rule; `tests/shared_formats.rs` pins both, and that both kinds hash alike.

## Found while recording

- **A query shared and then left before the next refresh is unshared** (`queries/share-then-switch-before-refresh`). The scan cache is read only at startup, after a pull and by the five-minute background refresh, so the activation's reconcile doesn't see a file written since and unshares its query (the file stays). Core has no cache (Decision 35), so this goes with the TypeScript.
- **Unlinking one project forgets a repo another project still uses** (`unlink/repo-still-used`): the other project keeps its git path but reads nothing until the repo is registered again.
- **A query typed with `\r\n` or trailing spaces changes at the next activation** (`queries/trailing-whitespace-and-crlf`): the file keeps the body as typed, the reader trims it, and the reconcile stores the trimmed text over the row with a version.
- **"STRASSE"'s refusal names the wrong kind of row:** the toast says `There's already a project called "Straße"` for a saved query (`queries/strasse`). The message comes from `libraryErrorMessage` through `storeReconciledQueries`, which Task 7 deletes; the sync's `nameTaken` notice replaces it.

## What can't be recorded

- **Events and `seq`:** the recording has one page and `TsLibrary`'s sequence; which `StorageChanged` events a sync or publish emits (Decision 44) and their `seq` are Core's own tests'.
- **The repo lock and ordering:** steps run one at a time, so a publish racing a sync or a pull, and the lock order (repo lock, then the write lock), are Task 5's tests.
- **Real symlinks and `O_NOFOLLOW`:** the disk models a symlink by its path, not by the file system; the Core replay lays out real symlinks.
- **Git itself:** the safe checkout (bug 2), merges and conflict resolution (Decision 35's sync after a resolve) aren't run; the stub only moves files.
- **File modification times** (`stat` throws, bug 19), the keychain (the recording has none: see `secrets`), and the base hashes Core records (Decision 34) don't exist in today's code.
