# Cleanup pass after phase 8

**Status:** done (2026-10-07): A, B and C built and reviewed; C's Monaco patch dropped at the owner's decision (Q2), B stores values with no size cap (Q1). Three items from the follow-up lists of 5d, 5e and phase 8, done before phase 6. Each runs as its own task with a review.

## A. CI builds `seaquel-cli` and runs storage's wasm tests

The 5e checkpoint found `npm run cli:build` broken by a cfg gate that every other build hid through feature unification (the CLI builds Core with `imports` and without `git`). Phase 8 added storage's wasm32 tests under `wasm-bindgen-test-runner`, which CI doesn't run.

- A CI step that builds `seaquel-cli` on its own (its own feature set, no unification with `src-tauri`), and the same for `seaquel-server` and `seaquel-mcp`.
- A CI step that runs `cargo test --target wasm32-unknown-unknown -p seaquel-storage --all-targets` under the runner.

## B. Applied grid edits keep their values in query history

`Workspace::apply_changes` records `r.planned.sql` (the SQL as queued, with `?`/`$n`/`@Pn` placeholders) through `query_history::append_many`, without the values. The history list shows placeholders, and re-running such a row fails ("Values were not provided"). Same on desktop, web and the demo.

## C. The editor slows down with many `{{param}}` uses

Measured by the phase 8 probe in Chromium: 500 uses 14.7 s, 1,000 40.4 s, 2,000 102.8 s, 4,000 crashed the tab. `extractParameters` is linear; the cost is the editor's own per-change work. Older than phase 8.

## Release notes

- **B.** Applied pending changes now keep their values in query history, on desktop, web and the demo. The history list shows them under the SQL ("Values: 1: 'Jonson'  2: 1"), and clicking such a row puts it back in Pending Changes for its connection, with its values, ready to review and execute (clicking it again doesn't queue it twice). Rows recorded before this release have no values and still open in an editor tab, where they can't run.
- **B, for developers and MCP users.** Storage migration `0006` (`query_history.params`) makes `seaquel-cli mcp` refuse the metadata file (`STORAGE_NEEDS_UPGRADE`) until the app has opened it once. The bundled CLI ships with the app, so in practice the app already has.

## Notes (as built)

(Each task adds its notes here.)

### A (as built)

Only `.github/workflows/ci.yml` changed, two steps in the `rust` job:

- **Interfaces build on their own features**, after Tests: `cargo clippy -p seaquel-cli -- -D warnings`, then the same for `seaquel-server` and `seaquel-mcp`, one command each. The resolver unifies features across every package one command names, so three `-p` in one command (or `--workspace`) would hide the break again. Lib and bins only (no `--all-targets`), so no dev-dependency adds a feature; `npm run cli:build` builds exactly that. Clippy rather than `check`: the same cost, and dead code under one interface's set fails as well. Not `build`: the break was a compile error, which a check finds without codegen and linking. No separate target dir: a different feature set already gets its own artifact hashes, and sharing the dir (and rust-cache) reuses every dependency whose features match.
- **Storage tests in wasm32**, after Storage builds for wasm32: `wasm-bindgen-test-runner` from `taiki-e/install-action` (`wasm-bindgen@<Cargo.lock's version>`, whose package carries the runner; the frontend job's install, copied), then `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner cargo test --target wasm32-unknown-unknown -p seaquel-storage --all-targets` under the job's Node 24 and the workflow's `clang-18`/`llvm-ar-18`. 6 wrapper tests and 7 in `tests/wasm.rs`; the other test files are `cfg(not(wasm32))` and run nothing.
- **Mutation.** With `connection_order_in`'s cfg put back to `#[cfg(feature = "git")]`, the CLI line fails (`E0432: unresolved import crate::library::connection_order_in` in `imports.rs`, exit 101); restored, it passes, and the file's SHA-256 matches the one taken before.
- **Cost.** On this Mac, after the job's earlier steps: 7 s, 11 s and 7 s for the three interface lines, 9 s for the wasm tests. Expect about a minute or two in all on a GitHub runner, plus a few seconds for the install.

### B (as built)

**Design.** A new expand-only migration, `0006_history_params.sql`: a nullable `query_history.params TEXT` holding the JSON array of `Value`s in the cell wire format. The row's `query` stays the SQL Core ran, placeholders and all. The display-form alternative (values inlined as dialect literals) was not taken: there's no literal writer for every engine (MSSQL and DuckDB inline `{{param}}`s, but Postgres, MySQL/MariaDB and SQLite only bind), and a display string isn't something you can safely run again. Re-running a row goes through the pending-changes queue as a typed change (`{type: "sql", sql, params}`), the way a statement the editor deferred does. That is the edits path (`db.applyChanges`), so the sheet's review, its destructive-statement dialog and Core's `confirmRequired` stay exactly as they were, and the run is recorded in history again with its values. `db.run` only knows `{{param}}`s, and `db.execute` would skip the confirmation and the history. Rows with values only ever come from the sheet's apply (immediate grid edits send no history context), so sending them back to the sheet fits. This happens whatever the pending-changes setting is: the sheet opens and the user presses Execute All.

**Changes**
- `crates/seaquel-storage/migrations/0006_history_params.sql` (new); `migrations/README.md` entry.
- `crates/seaquel-types/src/storage.rs:634`: `PersistedQueryHistoryItem.params: Option<Vec<Value>>` (serde default, left out when `None`; TS `params?: Array<unknown>`). `:908`: a hand-written `Debug` (id, `query_len`, timestamp, connection id, favourite, value count). The derived one printed the SQL and, now, the values.
- `crates/seaquel-storage/src/queries/query_history.rs`: `params` in `COLUMNS` (`:22`), `read_params` (`:71`; anything that isn't a value array reads as `None`, so one bad row can't fail the list), `params_text` (`:76`; `None` and `[]` store NULL, and encoding happens before `BEGIN`). Used by `append`, `append_many` and the frozen `replace_all`.
- `crates/seaquel-core/src/edits.rs:220`: `Ran.params`, filled in both apply branches. `:493`: `append_edit_history` stores the planned binds, `None` when there are none. `crates/seaquel-workspace/src/run.rs:972`: run history has `params: None`, since it records its `{{param}}` text.
- `src/lib/types/query.ts`, `persisted.ts`: `params?: unknown[]` (wire format). `query-history.svelte.ts:32`: `fromPersisted` keeps non-empty `params`.
- `src/lib/hooks/database/pending-changes.svelte.ts:245`: `addFromHistory(item)` queues `addSql(…, item.params, detectQueryType(…), "history")` and opens the sheet. `src/lib/types/pending-changes.ts:79`: origin `"history"`. The sheet labels it "History" in hard-coded English (`pending-changes-sheet.svelte:84`), like the other origin labels; it isn't an i18n key.
- `src/lib/hooks/database/query-tabs.svelte.ts:29,212`: `setHistoryRerun`; `loadFromHistory` hands a row with values to it instead of opening a tab (no view switch). Wired in `database.svelte.ts:306`. Both the sidebar's history list and the command palette go through it.
- `src/lib/utils/bind-values.ts` (new): `formatBindValues` (moved out of the sheet) and `formatWireBindValues`. `src/lib/components/sidebar/manage/queries-tab.svelte:259`: a "Values: …" line under a history row's SQL (the sheet's `pending_changes_values` message, cut at 200 characters). No new `en.json` keys, so no translation run. The unused `components/query-history.svelte` was left alone.
- `crates/seaquel-storage/tests/fixtures/wasm-made/meta.db` regenerated with `SEAQUEL_RECORD_WASM_FIXTURE=1` (the fixture README's procedure; it isn't one of the frozen fixtures). `tests/wasm.rs` lists `0006`, and `src/migrations.rs`' check expects 6. `tests/shared.rs` and `tests/state.rs` now expect `[1, …, 6]` applied. No frozen fixture changed. No replay needed the column dropped: the edits replay compares `query`/`rowCount`, the repo replays serialize `None` as absent, and the library replays compare `query_history` by id.

**Logs and Debug.** `Ran` has no `Debug`; `PersistedQueryHistoryItem`'s `Debug` shows only a count. Storage's encode error carries serde's message, which never quotes a value. `no_sql_keys_or_values_in_logs` already applies a change with canary values into history and still passes.

**Web limits.** Values stored are values that already passed `WEB_EDIT_LIMITS` (at most 16 MiB of values and 10,000 changes per apply), so one apply adds at most that much to history. History has a row cap (500 per connection, favourites kept) but no byte cap. A user who keeps applying near-limit batches can grow `meta.db` by up to about 16 MiB per apply until the oldest rows age out, and that can reach `SEAQUEL_USER_DB_MAX_BYTES` (2 GiB by default) sooner than SQL text alone would. When it does, the backstop works as designed: the history append fails with `STORAGE_FULL`, Core logs the code, and the apply still succeeds with no history row. Other writes for that user then fail too, until history is cleared or rows age out. That isn't new (2 MiB of typed SQL per apply could do the same, more slowly), but storing values makes it more likely. A per-row cap on stored values would close it, at the cost of rows that can't be re-run. **Owner's decision: no size cap on stored history values.**

**Tests first**, each seen failing before its implementation:
- `seaquel-types` `history_debug_shows_no_values_or_sql`: failed on the derived `Debug` (`CANARY` in the output).
- `seaquel-storage` `query_history.rs` `params_are_stored_and_read_back_in_the_wire_format`, `empty_params_are_stored_as_null`, `unreadable_params_load_as_none_and_keep_the_row`: first didn't compile (no field), then failed at runtime with no column. `src/migrations.rs` `migrations_match_sqlx` failed on the count (5) once `0006` existed.
- `seaquel-core` `edits.rs` `history_rows_keep_their_values` (atomic with an edit, typed bigint/bytes/NULL values and a change without values; then a single apply): `params` was `null`.
- vitest: `query-history.svelte.test.ts` (2), `pending-changes.svelte.test.ts` `addFromHistory` (3), new `query-tabs-history.svelte.test.ts` (2), 7 of 7 failed; new `utils/bind-values.test.ts` (3) failed on the missing module.
- Written after the code and passed on their first run: `migration_0006_applies_on_every_release_schema` (every frozen release schema, an older release's insert reads `None`); the extended `debug_shows_no_sql_keys_or_values` in `edits.rs` (the history `Debug` was already fixed); `tests/wasm.rs` `history_keeps_an_applied_changes_values` (now 8 wasm tests, where A counted 7); and the demo test in `src/lib/demo/duckdb-on-core.test.ts`, "records the values, shows them, and runs the row again with them" (an apply through the browser Core stores `[7]`, `addFromHistory` and apply insert it again, and the second row keeps `[7]`).

**Review fixes** (two Important, three minor):
- **I1, the queued change could land out of view.** The sheet followed `pendingConnectionId` (a focused data tab's or result's connection), not the history row's. `DatabaseState.pendingFocusConnectionId` is now a re-run's focus. `pendingConnectionId` prefers it while the sheet is open, and `activeRightPanel` became a getter/setter that clears it whenever the sheet closes or the AI panel replaces it. `addFromHistory` sets the focus, opens the sheet and shows `toast.info`: `history_rerun_queued` ("Queued for {connection}. Press Execute All in Pending Changes to run it."), translated into every locale by the i18n-translator agent.
- **I2, `decodeCell` mutated nested arrays in place.** A history row's `params` live in deep `$state`, so rendering them threw `state_unsafe_mutation`. Separately, a queued change's `params` and its decoded `bindValues` shared the inner arrays, so apply could send a `BigInt` (which `JSON.stringify` refuses) or a `Uint8Array`. `decodeCell` is now pure: it returns an array as a decoded copy and never writes its input. Every caller uses its return value: `decodeRows` assigns `row[i]` itself, `$lib/sql`'s `substituteParameters` maps fresh wasm output, `pending-changes.svelte.ts` maps `params`, and `bind-values.ts` does too. So no caller relied on the mutation, and no separate pure decode was needed.
- **M3, formatting stops at 200 characters.** `formatBindValues(values, maxLength)` cuts each value before `cellText`: a string's start, the first `budget/2` bytes, an array's first items, recursively. It stops at the limit and ends with "…". `formatWireBindValues` also cuts a wire value before decoding it, so a long `bytes` tag is never decoded whole. Used in the history line (`queries-tab.svelte:261`) and in both of the sheet's value lines (`pending-changes-sheet.svelte:272,352`).
- **M4, no double queueing.** A second click on a row whose SQL and values are already queued with origin `history` on that connection queues nothing. It still focuses and opens the sheet, and shows `history_rerun_already_queued`.
- **M5.** The "History" label wording above.
- **Tests first.** These failed first, 11 in all:
  - `values.test.ts` "leaves its input untouched, nested arrays included" (it saw the input mutated);
  - `bind-values.test.ts`: the two limit cases (lengths 1,000,011 and 4,000,009 against 201) and "doesn't change the values it's given";
  - `pending-connection.svelte.test.ts`: the three focus cases (a data tab on another connection, the sheet closing, the AI panel replacing it);
  - `pending-changes.svelte.test.ts`: "focuses the row's connection … and says it was queued", "a second click … doesn't queue it twice", and "nested values stay in the wire format, shown decoded, and re-run as sent" (`[[{"$sq":"bigint","v":"9007199254740993"}]]`; the JSON sent to Core holds the wire tag).
  - A `$state`/`$effect.root` rendering test also failed first, but on the test itself: Vitest resolves Svelte's server build, where `$state` isn't a proxy and effects don't run. It was replaced by a deeply frozen input to `formatWireBindValues` (any write throws). Added later: "cuts long wire values before decoding them".
- **Reruns after the fixes:**
  - `CI=1 npx vitest run`: 1,909 tests in 122 files, all passing.
  - `npm run check`: 0 errors, 0 warnings (4,725 files).
  - oxlint: clean (exit 0).
  - oxfmt: clean on the changed files.
  - The Svelte autofixer found no issues in the changed lines of `pending-changes-sheet.svelte` and `queries-tab.svelte`.

**Runs** (shared `scratchpad/p5a/target`, through `mise exec`, logs in `scratchpad/cleanup-b/`):
- `cargo test -p seaquel-storage -p seaquel-core --features seaquel-runtime/tokio` with `CI=1`, so `wasm_made` is strict: 593 passed, 0 failed.
- `cargo test -p seaquel-rpc -p seaquel-server --features seaquel-runtime/tokio`: 276 passed.
- `cargo test -p seaquel-types -p seaquel-workspace -p seaquel-mcp -p seaquel-cli`: 324 passed.
- `cargo test --target wasm32-unknown-unknown -p seaquel-storage --test wasm` (Homebrew llvm, the runner): 8 passed.
- `cargo fmt --all --check` clean. All 11 clippy lines in `ci.yml` pass: the workspace line, the three interface lines, and the seven wasm32 lines.
- `npm run types:gen` twice: only `PersistedQueryHistoryItem.ts` changed, on the first run (232 files); both reruns changed nothing.
- `npm run check`: 0 errors, 0 warnings.
- `npm run wasm:build:browser-test`, then `CI=1 npx vitest run`: 1,897 tests in 122 files, all passing (later, `duckdb-on-core.test.ts` alone with the new demo test: 26).
- `npx oxlint --type-aware --type-check --deny-warnings`: nothing in this task's files. Two `unbound-method` warnings in `src/lib/monaco/multi-character-typing.test.ts`, item C's untracked work in progress.
- oxfmt clean on the changed TS files. The Svelte autofixer found nothing in the changed parts of `queries-tab.svelte` and `pending-changes-sheet.svelte`.
- `npm run build` (299 files), `build:web` (574), `build:demo` (309) pass.

### C (as built)

**Finding.** The phase 8 probe's numbers came from how it typed, not from parameter handling. It filled the editor with Playwright's `keyboard.insertText`, which reaches the editor as one input event carrying the whole text as typing: an EditContext `textupdate` in Chromium, a textarea `input` in WebKit, a one-shot IME composition in Firefox. Monaco's `CursorsController.type` types such a text one character at a time inside one edit, with auto-closing and overtype. Two costs grow faster than the text:

- Each auto-closed `{` adds an `AutoClosedAction`, and Monaco drops the ones the cursor has left only after the whole edit ends. Before every character it reads and concatenates all of them in `AutoClosedAction.getAllAutoClosedCharacters`. A counter in a scratch build found 602 live actions at 300 uses. Chromium CPU profile at 300 uses: 5.4 s in total, 3.9 s under `getAutoClosedCharacters`.
- Each pair typed inside its closer splits a piece, and the piece tree finds a column within a line by walking that line's pieces (`nodeAt2`). Later checks, and the `{{name}}` decorations, walk ever more of them. A query filled this way stays slow to type in afterwards (9 keys: 1.4 s, longest frame 233 ms).

`(p)` repeated is slow too (1.1 s for 300 uses); `p` repeated is not (92 ms). The parameter dialog opened in under 140 ms at every size, even with 1,000 distinct names, and `extractParameters` is linear. Normal input was always fast: pasting a 1,000-use query took 15 to 60 ms, and typing 9 keys one by one into it had a longest frame of 33 ms (Chromium, before any change).

**Kept: decoration batching.** In Firefox the composition makes Monaco emit one content change per character, and `monaco-editor.svelte` re-ran `updateVariableDecorations` (whole text, regex, every decoration set again) on each one: 6,027 calls for the 1,000-use text. `src/lib/monaco/once-per-task.ts`, used at `monaco-editor.svelte:143,181,290`, queues the update to a microtask, so one input event gets one pass. It still runs before the next frame, so highlighting is unchanged, and the value binding and `onChange` stay synchronous. No SQL scanning moved; the existing regex is untouched. Its test, `src/lib/monaco/once-per-task.test.ts`, was written first: on a no-op stub, 6,000 calls in one task ran the work 6,000 times instead of once.

**Dropped: the Monaco patch.** A prototype patch that typed any multi-character keyboard text as one edit made every browser flat (1,000 uses: 43 ms in Chromium, 88 in Firefox, 61 in WebKit; 4,000 under 170 ms). It also turned off auto-close, overtype and auto-indent inside such input. The owner decided against patching Monaco internals, so it was removed, and dictation and IME input keep Monaco's normal behaviour.

**Measured with only the batching** (demo rebuilt in `scratchpad/cleanup-c/`, scripts in `cleanup-c/probe/`; ms from `insertText` to the next frame; run-to-dialog 36 to 86 ms throughout):

| uses | Chromium before | Chromium after | Firefox before | Firefox after |
|---|---|---|---|---|
| 100 | 308 | 257 | 152 | 93 |
| 1,000 | 42,946 | 41,358 | 3,601 | 905 |
| 4,000 | tab crashed | 252,375 (no crash this run) | 21,547 | 10,076 |

Earlier "before" runs: Firefox 500 uses 1,497 ms and 2,000 uses 8,651 ms; WebKit 1,000 uses 6,954 ms and 4,000 uses 59,793 ms. WebKit's path is the Chromium one (per-character typing with auto-close), which batching doesn't reach.

In a 1,000-use query after batching: paste 54 ms, then 9 keys one by one with a 50 ms longest frame (Chromium). Filled by `insertText`, the 9 keys took 1,403 ms with a 233 ms longest frame (Chromium) and 471 ms with a 55 ms longest frame (Firefox).

**Follow-ups.** Monaco's auto-closed actions are quadratic in a long text typed as one input, and the split pieces make the line slow afterwards. It's upstream (`vs/editor/common/cursor/cursor.js`, `monaco-editor` 0.55): validating the actions per character in `type`'s loop would remove the first part. Worth an upstream report if anyone cares; real users meet it only through dictation, text expanders or similar input.

**Runs.** `CI=1 npx vitest run`: 1,909 tests in 122 files pass. `npm run check`: 0 errors, 0 warnings. `npx oxlint --type-aware --type-check --deny-warnings`: clean. The Svelte autofixer reports no issues in `monaco-editor.svelte`, only generic suggestions about code that was already there. `npm run build:demo` passes in the scratch copy (`SEAQUEL_WASM_PREBUILT=1`; no wasm crate changed); `npm run build` passed there earlier with the same Svelte change.
