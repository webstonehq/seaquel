# Demo baseline (phase 8, Task 1)

What a visitor of the browser demo sees before phase 8 moves it onto Core, recorded so the demo on Core can be compared step by step. The crate itself arrives in Task 5; until then this directory holds only the fixtures.

- `baseline.json`: the recording.
- `changes.json`: each difference phase 8 is meant to make, with its decision. A difference the probe finds that isn't listed there is a finding, not an entry to add.

## How it was recorded

- **Recorder:** `docs/plans/artifacts/2026-10-06-record-demo-baseline.mjs.txt` (copy it as `.mjs` to run it: `node record-demo-baseline.mjs <build-demo dir> <out.json>`).
- **Build:** `BUILD_TARGET=demo vite build --mode demo` (`npm run build:demo` with `SEAQUEL_WASM_PREBUILT=1`) of HEAD `8d73887` (Phase 5e) plus Task 1's two fixes (the sample dashboard and the extra query tab), in a scratch copy of the tree, so `build-demo/` never appeared in the repo.
- **Browser:** Chromium 153.0.8010.12 (Playwright 1.63, headless), 1440×900, `en-US`, time zone UTC. DuckDB-WASM 1.32.0 (`v1.4.3`) from jsDelivr, as the demo loads it.
- **Server:** the recorder serves the build under `/demo` and resolves `/demo/manage` to `manage.html`, as Cloudflare does; a reload of the app's URL needs that.
- **One browser context** for the whole run, so the demo's metadata (the sql.js file in `localStorage`) carries from step to step as a visitor's does. It starts empty: the old file isn't recorded, since phase 8 deletes it unread (Q2 C).

Two runs of the final recorder against the same build gave byte-identical files (`cmp`), as runs of each earlier revision had. Adding the last step (`destructive statement, prompted`) left the twelve steps before it byte for byte as they were. A run against the build without Task 1's fixes differs only in the tabs and toasts of the reload steps (an extra "Query 2", "Query 3", "Query 4"; the dashboard error toast on every reload), so the baseline catches both bugs.

## The steps

Each step is one object in `steps`, in order. `toasts` in every step is the sorted list of toasts shown since the step before.

| Step | Records |
|---|---|
| `cold load` | `view` |
| `reload` | `view` |
| `saved query` | the Save Query dialog as shown (`dialogs`), then `view` |
| `saved query, reload` | `view` |
| `run with {{param}}` | the parameter dialog, the result pane (`pane`), the sidebar's queries panel (`queries`: saved queries and history) |
| `run with an error` | `pane`, `queries` |
| `run with a destructive statement` | `DELETE FROM demo.order_items WHERE id = 1`, which isn't destructive by design (`destructive_reason` flags a DELETE or UPDATE without WHERE, DROP, TRUNCATE, …), so `dialogs` is empty and it is queued like any write. Then `pane` (unchanged: the deferred run shows nothing new), the pending changes sheet (`pending`), the apply confirmation (`applyDialogs`, listing no destructive statement), the sheet after the apply (`pendingAfterApply`, empty: a full apply closes it), a `count(*)` afterwards (`countAfter`), `queries` |
| `grid edit, pending changes on` | `demo.customers` after an edit (`pane`), the header's pending count (`header`), `pending`, `applyDialogs`, the grid after the apply (`paneAfterApply`) |
| `grid edit, pending changes off` | the Settings → Features pane after turning pending changes off (`settings`), the grid after an immediate edit, `header` |
| `data-tab filter` | `demo.customers` filtered on `country = US` |
| `extensions list` | `tabs`, the extensions tab |
| `final reload` | `view`: everything above as the next visit shows it |
| `destructive statement, prompted` | pending changes back on, then `DELETE FROM demo.order_items` (no WHERE): the editor's destructive prompt (`dialogs`), `pane`, `pending`, the apply confirmation listing it as a destructive statement (`applyDialogs`), `countAfter` (0), `queries`. Last, so the steps before it are unchanged |

`view` is:
- `tabs`: the header's tabs in order, the active one marked `* `;
- `connections`: the left sidebar above its panel tabs (Learn, Manage, the connections);
- `projects`: the project menu's items;
- `header`: the app header;
- `schema`, `dashboards`, `queries`: the left sidebar's three panels, each read after selecting it (queries last, as a visitor left it).

Captures are Playwright aria snapshots (`ariaSnapshot()`), one line per array entry, without the bare `- img` lines. `pane` is the `main` landmark without its tab list (`tabs` has it). `pageErrors` is the sorted set of uncaught page errors over the whole run.

## Normalising

The recorder replaces, in every captured string:
- `duckdb-<digits>` (the DuckDB connection id) with `duckdb-<id>`;
- UUIDs with `<uuid>`;
- durations (`12ms`, `0.4 s`, …) with `<duration>`;
- relative times (`just now`, `5 minutes ago`, `3m ago`, …) with `<ago>`;
- clock times (`10:23`, `10:23:45 PM`) with `<time>`;
- ISO date-times with `<datetime>`.

Data values are left alone. The sample data is fixed, so `created_at` shows as epoch milliseconds (`1705314600000`) exactly; that is today's demo (Arrow's `toJSON()`), and Decision 8 changes it.

Every capture waits until two reads 400 ms apart agree. A run's result is read once it differs from the pane before the run (or after 5 s, for a run that shows nothing new).

## What isn't recorded

- **Timings** of DuckDB and of the page (normalised away; load times are in the plan's "Sizes and load times today").
- **The order of toasts** (they're a sorted list per step) and toasts that came and went inside a wait.
- **Pixels:** charts, colours and layout. Only the accessibility tree is captured, so a chart is its axis labels.
- **The editor's text.** Monaco's lines aren't in the aria tree; the steps say what was typed, and history shows it.
- **The metadata file** (sql.js in `localStorage`): phase 8 deletes it unread (Q2 C).
- **Firefox and WebKit:** the baseline is Chromium only. The probe (Task 7) covers the other two.
- **The first click on Pending Changes after a grid edit** doesn't open the sheet in headless Chromium (the second does). The recorder clicks until it opens and records only the open sheet. It is older than phase 8 and not traced here.

## Corrections

- **Task 6's review (2026-10-02).** The first recording on Core (Task 6) differed from three entries as written. The review confirmed each is Core's behaviour, the same as desktop, and `changes.json` was corrected, not the recording: `decision-8-cells` now says the grid formats DuckDB's timestamp text (`Jan 15, 2024, …`) rather than showing it raw; `bug-7-page-sql` adds the `Query failed: ` prefix (Core's error wording); and a new `history-survives-reload` entry covers `final reload` and the last step, whose history now survives a reload (with applied edits stored as their `?` SQL, as on every interface). `baseline.json` is unchanged.
