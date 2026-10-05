# Render snapshots

`TestBackend` buffers as plain text, one file per case, written by the unit tests in `src/view/mod.rs`, `src/runtime/event_loop.rs` and `src/runtime/app_tests.rs` (`keychain_wait_100x30`, drawn from a real Core waiting on a blocking secret store) through `src/testing/snapshot.rs`. `UPDATE_SNAPSHOTS=1 cargo test -p seaquel-tui` rewrites them; review the diff before keeping it. The app's version is written as `<version>`.

The reference is the TUI's design (`seaquel-tui-text.txt`, removed with the other planning files and kept in git history; screen 1a at 148×42). The snapshots at 80×24, 100×30 and 79×24 have no design counterpart and are reviewed for fit.

- `empty_*`, `help_*`, `confirm_quit_80x24`, `keybars`: the frame with nothing connected, regenerated for the new keys (`Connect: enter`, `Reload: r`, the dialogs' bars).
- `connected_148x42`: screen 1a's panels 1–3 with the design's data (`testing::fixtures::connected`); `connected_saved_80x24` and `connected_history_100x30` the same at other sizes, with panel 3 focused.
- `dialog_*_80x24`: the picker (both steps), the password prompt, the trust dialog, a failed connect with a retry, a notice and the keychain wait box; `dialog_problem_reconnect_80x24` (below) joined them with the DuckDB helper.
- `browse_148x42` and `browse_80x24`: screen 1a (`testing::fixtures::browsing` plus `view::tests::screen_1a`): `public.invoices` opened, its fifteen rows, the staged edit of `48109.total` and the delete of `48106` (staged through the keys, their plans as Core words them), the cursor on the edit. `browse_editing_148x42`: a cell being edited. `browse_{structure,indexes,constraints,ddl}_148x42`: the other four tabs over the fixture's metadata.
- `pending_148x42` and `pending_80x24`: screen 1c (`testing::fixtures::staged` plus `view::tests::screen_1c`): panel 4 focused on the delete of `48106` (`2 of 4`) beside the edit of `48109.total`, an insert and an edit of `48108.customer`, its diff in the main view. `pending_sql_148x42`: the SQL tab over the edit. `commit_148x42`: the commit dialog on the `prod` connection with `pro` typed; `commit_80x24` on a connection without the label; `commit_long_80x24` 30 changes with 15 destructive statements, the list (first 10) and the `prod` field kept in view under the counts. `dialog_discard_80x24` and `dialog_switch_80x24`: `D`'s question and the staged changes' switch question.
- `query_148x42` and `query_80x24`: screen 1b (`testing::fixtures::screen_1b`): `revenue_by_month.sql` opened from panel 3 and edited (`INSERT · modified`), an untitled tab beside it, the cursor after `i.iss` on line 7 with the completion popup open, and the Explain tab with the design's ANALYZE plan. `query_results_148x42`: a run of four statements (two SELECTs, an UPDATE, one that failed) with the first SELECT shown (`statement 1 of 2`) and the results box focused; `query_messages_80x24` its Messages tab. `dialog_{params,run_confirm,save_as,cell}_80x24`: the `{{param}}` form, the destructive question on a `prod` connection with `pr` typed, the name prompt and a JSON cell full size. The bars and help (`keybars`, `empty_*`, `help_*`, `connected_*`) were regenerated for the query keys (`Query: Q`, `Open in a tab: o`, the query contexts).
- `ask_148x42` and `ask_80x24`: screen 1d (`testing::fixtures::screen_1d`): Ask AI over an untitled tab on `prod-analytics`, the design's request with its two mentions, the design's SQL answered in 1.8 s by `claude-sonnet`. `ask_{mention,waiting,error,not_run}_80x24`: the `@` list after `count @inv`, the wait, a `NO_PROVIDER` refusal worded, and Ctrl+R on an `UPDATE` (inserted, not run, the note saying why, the bar `Refine | Save as | Close`). `keybars` and the 1b bar assertion gained Ask AI's keys (`Ask AI: ctrl+k` on the Insert bar, which pushes `Open in $EDITOR` off at 148 columns; the five Ask contexts).
- `install_{ask,progress,failed}_148x42` and `_80x24`, `install_checking_80x24`: the DuckDB helper's install dialog (`testing::fixtures::dialog`): the size lookup, the question with the size (`11.7 MB`) and the binary's version on a line of its own (so the version's length never moves a line break), the download at 36% (`4.2 MB of 11.7 MB`), and an offline failure worded (`NETWORK_ERROR`) with `Retry: r | Close: esc`. `install_ask_repair_80x24`: the question for a helper already there in a folder that isn't private (`Unsafe`), with the line saying an install makes it private. `dialog_problem_reconnect_80x24`: a helper that didn't start (`ENGINE_UNAVAILABLE`) with `Connect again: r`. The design has none of these; they were reviewed for fit at both sizes. `keybars` gained the six contexts (`ProblemReconnect`, `InstallChecking`, `InstallAsk`, `InstallDownloading`, `InstallFailed`, and `InstallFailedFinal`: `Close: esc` only, for `NOT_SUPPORTED`).
- `dialog_recommit_80x24`: `c` on a queue a commit cut off by its connection may have applied, with panel 4's hint `! may be partly applied: check the data`. `keybars` gained `ConfirmRecommit` (`Commit again: y | Cancel: esc`).

## Where they differ from the design, and why

| Difference | Cause |
|---|---|
| Panel 2's title is `Tables - Views`, without `Functions - Enums` | Deferred |
| Panel 1 has no `pg 16.2` | The server version is deferred |
| Panel 3 has no `⎇ main ↑1` | Git status in Saved is deferred |
| Placeholders (`not connected · enter to pick a connection`, `no connection`, `nothing selected`) in the `empty_*` snapshots | Nothing connected; `connected_*` show panels 1–3 filled |
| Panel 1 says `✓ prod-analytics → pg · db.internal` (with `· ssh <host>` through a tunnel), not `pg 16.2 · ssh bastion` | No server version; the place is the host (and port when not the default) or the file |
| Panel 2's counts are Core's approximate row counts (`12.4k`), its schemas `▾`/`▸` folders with their table count; the Views tab shows `view` or `matview` in the count's place | The design's counts were mock data |
| Panel 3's marker is `⇄` for a query shared with a project's repo, never `M`/`A` | No git status; the shared marker comes from `sharedPath` |
| Panel 3's History rows are `HH:MM:SS  SQL` (local time; `MM-DD HH:MM` when not today), newest first | The TUI shows what Core reads |
| The main view over a table that isn't opened lists its columns and types (`schema.table · ≈rows`) and `enter to open it` | A page is read only on Enter, never while `j`/`k` move through panel 2 (no query per keystroke) |
| The grid's filter line says `/filter…`, and on its right the server filter and sort as `F total > 3000 · sort issued_at ↓`, not `ORDER BY issued_at DESC` | The TUI writes no SQL (ground rules); Core builds the SELECT from the typed `TableQuery` |
| The grid's footer counts are Core's: `rows 1–15 of 48,112 · 18 ms` (`≈` before an estimated count), with `n matches on this page ·` while `/` filters | The counts are Core's |
| `›` at the end of the column names when columns go on past the right edge, `‹` at their start when some scrolled off the left | Review M6 |
| Control, bidi and zero-width characters in cells, names, types, defaults, index names, DDL and the command log show as `�` (a newline as `↵`, a tab as `⇥`) | Review M5: nothing from the database reaches the terminal as a control sequence |
| `paid_at` is off the right edge at 148 columns, the columns as wide as their widest cell (at most 40) | Columns that don't fit scroll so the cursor's stays in view; the design's mock cut the timestamps |
| Numbers line up on the right by column (the first non-NULL cell decides), so a staged `3150.00` lines up with the loaded ones | The design right-aligned `total` only |
| The command log shows Core's SQL with its placeholders (`staged UPDATE "public"."invoices" SET "total" = $1 WHERE "id" = $2`), not the values | Core plans the edit (`plan_edits`); the TUI builds no SQL |
| Panel 4 (1c) lists one table, `public.invoices`, with `M`/`D`/`A` rows; the design spread them over `items` and `invoices` | The fixture has one table's metadata; grouping by table is tested on the queue (`panel_four_groups_entries_by_table`) |
| Panel 4's delete row shows the row's first two values that aren't its key (`Hooli · overdue`), an insert its first value (`customer New Co`); the design showed `Hooli · 540.00` and `id 87 "Priority support"` | One rule for every table: Core gives no "display columns" |
| Panel 4's hint is `space unstage   u undo   c commit` (not `c commit all`); on another connection it says where the changes are staged | The hint's words pair with keys the keymap binds (`dialog_footers_name_bound_keys`) |
| The diff's header is `@@ public.invoices · id 48106 (delete) @@`, not `WHERE id = 12`, and values are shown as the grid shows them (`Hooli`, not `'Hooli'`) | The TUI writes no SQL (ground rules) |
| The SQL tab is Core's plan, `-- planned by Core`, with its placeholders and the values below (`$1 = 3150.00`), not `-- generated` with literals | Core plans the edit (`plan_edits`); the TUI builds no SQL |
| The commit dialog's counts are `+1  INSERT  public.invoices` with two spaces; it says `⚠ This connection is tagged production.` when nothing is deleted; on a `prod` connection its preview key is Tab, not `p` | The TUI asks on every commit on a `prod` connection; `p` is text in the `prod` field |
| The commit dialog has no footer; `Execute: enter | Preview SQL: p | Cancel: esc` is on the bar | The bar is generated from the keymap |
| The Constraints tab names the kinds (`PRIMARY KEY`, `FOREIGN KEY`, `UNIQUE`, `UNIQUE INDEX`) rather than constraint names, and says CHECKs aren't listed | `table_metadata` has no constraint names or CHECKs |
| The DDL tab is headed `-- approximate: …` and is Core's `create_table` over the metadata | The TUI shows what Core reads |
| A view's main view has Data and Columns | Columns as for a table; Data browses it, and Core refuses its edits (`NOT_EDITABLE`) |
| The main view over a saved query or history row shows its SQL, read-only | Editing it is a query tab's job |
| Panel 1 is cut with `…` at 80 columns | Panel 1 is 28 columns wide below 100 × 30 |
| The dialogs have no footer; their keys are on the key bar | The bar is generated from the keymap |
| `Commit: c` is on the Tables, main view and grid bars (the prototype's); panel 4's bar is 1c's `Unstage: space | Undo: u | Edit value: e | Commit: c | Discard all: D | Keybindings: ?` | The bar is generated from the keymap |
| The grid's bar is `Move: h j k l`, `Edit: e`, `Stage delete: d`, `Insert: a`, `Filter: /`, `Tabs: [ ]`, `Undo: u`, `Commit: c`, `Back: esc`, `Keybindings: ?`; editing shows `Save: enter` and `Cancel: esc` with `EDIT` on the right (no `Set NULL: type NULL`, which isn't a key; the help says it) | The bar is generated from the keymap |
| `Main: 0` and `Focus panel: 1-4` are on panel 1's bar only | The prototype's default bar (`renderVals`), used for panel 1 |
| The help lists `ctrl+c`, `ctrl+z`, `q` and `h l` | The help is generated from the keymap; the prototype's help listed only its mock's keys |
| The help is grouped Global, Panels, Saved and History, Pending changes, Connection, Main view | Generated from the keymap table in table order |
| At 80×24, panel 4's `+0 ~0 -0` is dropped | A right-hand title is drawn only where it fits beside the left one; panel 1's `○ 0` still shows the staged count |
| The main view's title has no `[0]` | As the design: the main view isn't numbered; `Main: 0` is on the bar |
| At 80 columns the bar drops `Next panel: tab` but keeps `Keybindings: ?` | A narrow bar loses its middle entries; the last always stays |
| `Quit?` dialog | New (`q` and Ctrl+C ask when something would be lost) |
| The query box is `[Q]-revenue_by_month.sql - untitled-1 - +`, not `[q]-… untitled-2` | `q` quits (the prototype's), so `Q` shows the editor; `+` (in Normal mode, or outside the editor) opens a tab; untitled tabs count from 1 |
| The 1b bar is `Run: ctrl+r`, not `Run: ctrl+enter`, and has no `Format: ctrl+f` or `Next tab: ]` | Ctrl+R works in every terminal (Ctrl+Enter and F5 are aliases); no formatter in 7a; `]` is text in Insert mode, so tabs switch in Normal mode |
| The 1b bar's `Run statement` is `ctrl+e` | Probe F8: macOS terminals type `®` for Option+R unless Option sends Meta; Alt+R stays as an alias |
| With the popup open the bar is the popup's (`Insert: tab | Choose: arrows | Close: esc`), and the right side is `INSERT · Ln 7, Col 13` | The bar follows the context and carries the mode with `Ln, Col` |
| The popup lists `issued_at date` and `issuer_id int8`, not `is_subscription` | Completion matches the typed prefix (`iss`) case-insensitively from the start of the name |
| The highlighting follows `seaquel_core::sql`'s scanner per engine: keywords red, function names purple, strings blue, numbers cyan, quoted names yellow, comments dim | The design coloured a hand-picked sample |
| No Chart tab and no `⚠ hint` with `i stage index` | Deferred |
| The Explain tree's first column shows Core's labels (`Sort (sum(revenue)) DESC`, `HashAggregate`, `Hash Join li.invoice_id = i.id`), its numbers right-aligned; shares of 50% and more are in the warning colour | Labels come from the plan's own fields (sort key, hash condition, relation, index); the design's mock wrote `GROUP BY 1, 2` |
| At 80×24 the Explain labels are cut with `…`, and `INSERT · modified` is dropped from the editor's title | Narrow layout; the bar still shows the mode |
| Results show `rows 1–3 of 3 · 4 ms` and the column under the cursor in the footer; Messages list each statement with its outcome | As the grid's footer |
| The dialogs (parameters, run confirmation, name, cell) have no footer | The bar says their keys |
| Ask AI's status line is `✓ generated in 1.8s · claude-sonnet`, without `3 tables in context` | `ai_generate` answers the SQL only; the model is the connection's, from the library |
| No explanation line under the SQL (`Joins through invoice_line_items because …`) | `generate` returns SQL only |
| The popup has no footer; `Insert: enter | Run: ctrl+r | Refine: tab | Save as: ctrl+s | Close: esc` is on the bar (and `Mention: @` while typing) | The bar is generated from the keymap |
| The popup's border is the added colour with `Ask AI` on the left and the sharing line on the right in the muted colour, without per-word colours | One role per span keeps `NO_COLOR` readable |
| The request wraps after a space; the SQL is numbered and cut at the popup's width, not wrapped | The editor's highlighting (`line_runs`) is reused as is |
