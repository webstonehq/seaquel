# Phase 2b spike: sqlparser-rs and seaquel-wasm

**Date:** 2026-09-27
**Status:** Done. Prototypes and harness in `spike/` (see `spike/README.md`).
Nothing in `src/`, `crates/` or `src-tauri/` changed. `spike/` was deleted in
phase 2b's Task 12: its Rust moved into `crates/seaquel-sql`, its corpus into
the fixture recorder (kept as `artifacts/2026-09-27-sql-recorder-*.txt`), and
the two Playwright load checks are kept as
`artifacts/2026-09-27-spike-browser-check.mjs.txt` and
`artifacts/2026-09-27-spike-demo-editor-check.mjs.txt`. The paths below are
as they were.

This spike answers the questions the design doc's "Phase 2b estimate" left
open: can sqlparser-rs stand in for node-sql-parser without rewriting lesson
criteria, and does a WASM module load cleanly in all three builds?

Both answers are yes. On a corpus of 172 queries, the Rust port of the
tutorial parser produced the same `ParsedQuery` as the TS code for all 91
tutorial queries, and every lesson criterion gave the same verdict on both
sides. The criteria never read the AST. They read the query builder snapshot
and the SQL text, so the risk the design doc was worried about isn't there.
The stub module is 1.56 MB (561 KB gzipped). It loads in the desktop, web and
demo builds and in the dev server with no Vite plugin. Init takes 3–7 ms in
Chromium and 16–27 ms in WebKit. One thing does have to change: Tauri's CSP
blocks WebAssembly today.

Recommendation: switch everything to sqlparser-rs, tutorial included, and plan
2b at about 18–28 h rather than 27–45 h. The details follow.

## Corpus

| Set | Entries | Where it came from |
|---|---|---|
| Tutorial solutions | 55 | One passing query per challenge, written for the spike (the lessons store criteria only) |
| Tutorial hints | 8 | The SQL after "Write:" in a hint, verbatim, where it differs from the solution. Users paste these |
| Tutorial variants | 28 | Other ways to type an answer: lower case, extra parentheses, comments, quoted names, `50.0`, `<>`, `ORDER BY 2`, `USING`, `EXISTS`, a trailing second statement |
| Query builder round trips | 57 | 37 builder states run through `buildSql` (query-builder-sql.ts) with bare table names, plus 4 of them with each engine's qualified names (`public.x`, `` `Sales`.`customers` ``, `[Sales].[customers]`, `main.x`, `"Sales".customers`) |
| Editor statements | 24 | Visual AST inputs the others don't cover: DML, CASE, CAST, `::`, `$1`, UNION, `$$…$$`, MySQL `LIMIT 5, 10`, `TOP`, `OFFSET … FETCH`, SQLite `GLOB`, DuckDB `GROUP BY ALL` and `QUALIFY` |
| **Harness total** | **172** | `spike/harness/corpus.ts` |
| Parse acceptance | 140 | Statements already in the repo: sample queries (18), the six e2e `schema.sql` files split with the app's splitter (118), demo dashboard queries (4) |

Each harness entry runs through `parseSql`, `parseQueryForVisualization` and
`resolveColumnSources` on the TS side and through their ports on the Rust side.
The harness then evaluates the lesson criteria and regenerates the SQL from
each side's `ParsedQuery`, using the real `applyParsedSqlToState` and
`buildSql`.

## AST mapping

### Tutorial `ParsedQuery` and lesson criteria

| | Result |
|---|---|
| `ParsedQuery` identical, tutorial entries | 91/91 |
| Criteria verdict identical | 91/91 (81 pass on both sides, 10 fail on both) |
| Criteria that need rewriting | 0 |

The ten that fail on both sides are variants the current parser doesn't
understand either: `ORDER BY 2`, `GROUP BY 1`, `HAVING n > 3` on an alias,
`JOIN … USING`, a compound `ON`, `country = "USA"` (a column name in
PostgreSQL, so the filter is dropped) and an `EXISTS` answer to a challenge
that asks for `IN`.

The first run gave two differences, both mapper bugs I fixed in minutes:

- node-sql-parser keeps a decimal literal's text (`50.0` stays `"50.0"`), and
  my port had normalised it to `"50"`. The normalised version passed three
  criteria that the TS code fails. That might be worth doing on purpose later,
  but it's a behaviour change, so the port now keeps the text.
- Aggregate names in the visual AST: node-sql-parser upper-cases `count(*)`.

`criteria.ts` works on `QueryBuilderSnapshot` (tables, joins, filters, …) and
on the raw SQL string, never on node-sql-parser nodes. The AST only touches
`tutorial/sql-parser.ts`. The query builder's two-way sync goes through the
same `parseSql`, so porting one function covers both.

### Query builder round trips

| | TS today | Rust, PostgreSQL mode | Rust, engine dialect |
|---|---|---|---|
| SQL regenerated exactly | 44/57 | 43/57 | 45/57 |
| `ParsedQuery` identical to TS | — | 56/57 | — |

Twelve round trips fail identically on both sides. These are limitations of
today's pipeline, and the port reproduces them on purpose:

- `IS NULL`, `IS NOT NULL`, `IS NOT FALSE`, `IN (1, 2, 3)`, `NOT IN (…)` and
  `BETWEEN` filters are dropped. The parser only keeps comparisons with a
  literal on the right.
- In `a AND b OR c`, the builder writes each filter's connector before the
  next filter, and the parser reads it back one filter later, so `OR` becomes
  `AND`.
- HAVING loses the table (`AVG(products.price)` comes back as `AVG(price)`).
- SELECT subqueries are ignored, FROM subqueries lose their alias, and
  `products.*` comes back as the full column list.
- `{{min_price}}` doesn't parse in either parser.

The one difference is `` FROM `Sales`.`customers` ``. node-sql-parser's
PostgreSQL mode accepts backticks, but sqlparser-rs's PostgreSQL dialect
doesn't. Today the builder always parses as PostgreSQL, whatever the
connection. With the connection's own dialect the MySQL case round-trips, and
so does `[Sales].[customers]` on SQL Server, which fails on both sides today.
So in 2b the builder has to pass the engine dialect. That's one argument, but
it's required, not optional.

### Visual AST

| | Entries |
|---|---|
| Identical as returned | 15/172 |
| Identical once known TS bugs are set aside | 161/172 |

The TS visual AST is broken in ways node-sql-parser 5.x caused and nobody
noticed:

| TS bug | Diff lines | Entries |
|---|---|---|
| Column names print as `[object Object]` (5.x wraps them in `{ expr: { value } }`) | 365 | 144 |
| No LIMIT gives `{ count: NaN }` (`limit` is `{ value: [] }`) | 135 | 134 |
| Window and other non-aggregate calls print as `""` | 6 | 6 |
| DISTINCT never detected (`distinct` is an object now) | 1 | 1 |

It shows in the product. In the current demo, the Visual tab for `SELECT
p.name, p.price FROM demo.products p WHERE p.price > 10 ORDER BY p.price`
shows `p.[object Object] > 10` in the WHERE node, `p.[object Object]` twice
under SELECT, and a LIMIT node reading `NaN`. The port does what the TS code
meant to do, so these get fixed as a side effect.

The 11 remaining differences:

| Kind | Count | Example |
|---|---|---|
| Mapper formatting, trivial | 6 | `IN ((subquery))` vs `IN (subquery)`; `schema: null` vs absent on INSERT/UPDATE sources |
| TS bug | 2 | MySQL `LIMIT 5, 10` read as count 5, offset 10 (it's offset 5, count 10); `$$it's$$` prints as `""` |
| node-sql-parser can't parse it, sqlparser-rs can | 2 | DuckDB `GROUP BY ALL`, `QUALIFY` (TS parses DuckDB in PostgreSQL mode) |
| The port does more | 1 | SQL Server `TOP 10` becomes the limit |

### Column sources

170/172 identical. The two differences are the DuckDB queries that
node-sql-parser can't parse.

### Parse acceptance on SQL from the repo

| Source | Statements | Both parse | Only node-sql-parser | Only sqlparser-rs | Neither |
|---|---|---|---|---|---|
| Sample queries | 18 | 16 | 0 | 1 | 1 |
| e2e schema files | 118 | 101 | 0 | 16 | 1 |
| Demo dashboard | 4 | 4 | 0 | 0 | 0 |
| **Total** | **140** | **121** | **0** | **17** | **2** |

sqlparser-rs parses everything node-sql-parser parses here. It also parses
four of SQL Server's six `CREATE TABLE`s, two MariaDB `CREATE TABLE`s and a
`CREATE INDEX`, DuckDB's `DROP SEQUENCE` and `DESCRIBE`, and the all-types
tables of Postgres and DuckDB. Neither parses
SQLite's `PRAGMA table_info(your_table)` or a column typed `UNSIGNED BIG INT`.
MariaDB goes through sqlparser's MySQL dialect, since sqlparser has no MariaDB
one, and all 18 MariaDB statements parsed.

### Genuine AST gaps

None found. Nothing in the corpus needed a node that sqlparser-rs doesn't
have. Its tree is more explicit than node-sql-parser's (`InList`, `Between`,
`IsNull` and `Like` are their own variants instead of `binary_expr` with an
operator string), which made the port shorter, not longer.

### How much was ported

| TS | Lines | Rust (spike) | State |
|---|---|---|---|
| `tutorial/sql-parser.ts` (`parseSql`: CTEs, subqueries in WHERE/FROM/HAVING, joins, aggregates, GROUP BY, HAVING, ORDER BY, LIMIT) | 1,213 | `tutorial.rs` 712 | Complete, at parity |
| `db/sql-ast-parser.ts` (SELECT, INSERT, UPDATE, DELETE) | 532 | `visual.rs` 528 | Complete; TS bugs fixed |
| `db/column-sources.ts` | 181 | `column_sources.rs` 119 | Complete, at parity |
| Shared helpers | — | `ast_util.rs` ~140 | |

Not ported:

- `hooks/query-builder-sql.ts` (338 lines). String building, no parser. It
  can stay in TS or move with little risk.
- `hooks/query-builder-parsed-sql.ts` and the other builder hooks. They turn
  a `ParsedQuery` into canvas state (`SvelteSet`, UUIDs, positions). That's
  GUI state, and by decision 8 it stays in the interface.
- `criteria.ts` (327 lines). It reads the snapshot, so it can stay in TS.
- The scanners (`sql-parser.ts`, `sql-scan.ts`, `query-params.ts`,
  `query-utils.ts`) and `parse-create-table.ts`. See "Scanners" below.

For production, the ported code still needs types generated for `ParsedQuery`
and `ParsedQueryVisual` (specta, as for the rest of `seaquel-types`), the
harness corpus frozen as fixtures, and the six formatting differences settled.

## WASM in the build

### Size

| Build of the stub | After `wasm-opt -Oz` | gzip -9 | brotli 11 |
|---|---|---|---|
| `opt-level = "z"`, LTO, `panic = "abort"` | 1.56 MB | 561 KB | 428 KB |
| Same, split only (no `parse` exports) | 1.47 MB | 515 KB | — |
| `opt-level = "s"` | 1.86 MB | 668 KB | — |
| `opt-level = 3` | 2.61 MB | 874 KB | — |
| For comparison: node-sql-parser `index.js` (all dialects; what the app imports) | 2.51 MB | 420 KB | 213 KB |

Dropping the parser exports saves only 6%. The dialects are trait objects, so
the tokenizer drags most of the parser in with it. A "small scanner module"
only pays off if it's a hand scanner without sqlparser. sqlparser's
`recursive-protection` and `visitor` features made no measurable difference.

Once node-sql-parser is gone, each build grows by about 140 KB gzipped
(561 − 420), or 215 KB with brotli. The whole app today is 27.8 MB of JS
(6.2 MB gzipped), so that's small. The builds with the module grew by exactly
the `.wasm` plus 6 KB of JS glue, and build times didn't change (desktop 39→40
s, web 51→52 s, demo 39→43 s).

### Init time

Node 24 (a proxy, fresh process per run, five runs):

| Step | Time |
|---|---|
| `WebAssembly.compile` (V8 compiles lazily) | 1.4 ms |
| Instantiate | 0.13 ms |
| First call (`split_statements`, compiles on first use) | 1.2 ms |
| For comparison: `require("node-sql-parser")` | 31 ms |

Headless browsers, loaded from a local server, awaited in the root
`+layout.ts`. The time runs from the start of `init` to ready and includes the
fetch:

| Target | Chromium | WebKit |
|---|---|---|
| Desktop static build | 4–7 ms | 16–27 ms |
| Web (adapter-node server) | 7 ms | 25 ms |
| Demo (`/demo` base path) | 3.4 ms | 23 ms |
| Vite dev server | 86 ms | 112 ms |

Per call, warm, in Node: splitting a three-statement editor buffer takes
20 µs, `statement_at` 17 µs, and the visual AST of a join/group/having query
(JSON out, `JSON.parse` in) 33 µs. `astify` alone on the same query takes
97 µs in node-sql-parser. An `opt-level = 3` build halves the split time but
is two thirds bigger. At keystroke rate `z` is fast enough.

### Bundler setup

What worked on all three targets, with no config change and no plugin:

```ts
// src/lib/wasm/seaquel-sql.ts
import init, * as wasm from "./pkg/seaquel_wasm_stub.js"; // wasm-bindgen --target web
import wasmUrl from "./pkg/seaquel_wasm_stub_bg.wasm?url";
export const initSeaquelSql = () => init({ module_or_path: wasmUrl });

// src/routes/+layout.ts (ssr = false already)
export const load = async () => { await initSeaquelSql(); return {}; };
```

- **Desktop (`npm run build`, adapter-static):** loads. The static server
  sent `application/wasm`. Tauri 2.11.6 serves embedded assets with
  `MimeType::parse`, which sniffs content with the `infer` crate, and `infer`
  recognises the WASM magic number. I read that in the source; I didn't run it
  in a Tauri window.
- **Web (`npm run build:web`, adapter-node):** loads. sirv sends
  `application/wasm`.
- **Demo (`npm run build:demo`, base `/demo`):** loads. `?url` picks up the
  base path.
- **Dev server:** loads from `/src/lib/wasm/pkg/…` as `application/wasm`.
  SvelteKit warns that `load` used `window.fetch`. Passing `load`'s `fetch`
  through to `init` avoids that.
- **Wrong MIME type:** wasm-bindgen's glue falls back to non-streaming
  instantiate with a console warning, and it still loads.
- **vitest:** `initSync({ module: readFileSync(…) })` works in Node. The
  scanner test does exactly that. 2b needs a vitest setup file so tests that
  import the call sites don't have to init by hand.

I didn't need `vite-plugin-wasm`, top-level await or `?init`. `?init` hands
back an instance, and wasm-bindgen's glue wants to instantiate the module
itself, so it's the wrong fit anyway.

The WASM has to exist before `vite build`. So 2b has to add a build step to
the npm scripts, CI, the Dockerfile and the website repo's `demo:update`, and
wasm-bindgen-cli has to be pinned to the crate's exact wasm-bindgen version.
That's the part of the build work with unknowns.

### Tauri CSP

`src-tauri/tauri.conf.json` sets `script-src 'self'`. I served the desktop
build with that policy, plus the inline-script hashes Tauri adds. Both engines
refused the module:

- WebKit: `CompileError: Refused to create a WebAssembly object because
  'unsafe-eval' or 'wasm-unsafe-eval' is not an allowed source of script`
- Chromium: `Compiling or instantiating WebAssembly module violates the
  following Content Security policy directive`

With `'wasm-unsafe-eval'` added to `script-src`, both loaded. 2b has to
change the desktop CSP. The web build sends no CSP, and the demo already runs
DuckDB-WASM and sql.js.

### Sync call sites

In the scratch copy, `splitSqlStatements` called the module synchronously and
fell back to the TS splitter when the tokenizer rejected the input. In the
demo build, typing 43 characters into the editor made 42 synchronous calls,
from the statement count in `view-state.svelte.ts`, which is a `$derived`.
Cmd+Enter then ran the second of two statements, the one after `'東京😀'`,
through `getStatementAtOffset`. It worked in both Chromium and WebKit, with no
"used before init" error. Because init is awaited in the root layout's `load`,
nothing mounts before it's ready. The demo's existing page errors (a
`parentNode` TypeError and a monaco-sql-languages worker message) happen on
the baseline build too.

## Offsets

There are three units, not two. Monaco and JS strings use UTF-16 code units.
Rust indexes `&str` by UTF-8 byte. sqlparser's spans are (line, column) with
columns counted in chars. For `東京` the char column matches UTF-16 but the
byte offset doesn't. For an emoji, all three differ.

The probe splits `SELECT '東京' AS city;\nSELECT '😀' AS face;\nSELECT 3`:

| Cursor (as Monaco reports it) | Used as a byte offset | Converted at the boundary |
|---|---|---|
| Start of statement 2, UTF-16 21 (byte 25) | statement 1 | statement 2 |
| Start of statement 3, UTF-16 42 (byte 48) | statement 2 | statement 3 |

The fix is small. `offsets.rs` has `utf16_to_byte`, `byte_to_utf16` and
`location_to_utf16`, about 30 lines with tests. The WASM exports take and
return UTF-16 offsets only, and the conversion happens inside `seaquel-wasm`.
It's a linear scan, which at editor sizes is already inside the 20 µs split.
The rule for 2b: no byte offset or sqlparser `Location` leaves `seaquel-wasm`.

## Scanners

The same 15 inputs through three splitters:

| | Right |
|---|---|
| `db/sql-parser.ts` `splitSqlStatements` (what the editor uses today) | 5/15 |
| sqlparser-rs tokenizer, engine dialect | 12/15 |
| `engine/sql-scan.ts` `sqlTokens` (the paging checks' per-engine tokenizer) | 15/15 |

`splitSqlStatements` ignores its `dbType` argument. On MySQL it splits inside
`#` comments and backtick names, and it gets backslash escapes wrong. It also
splits inside SQL Server bracket names, Postgres `E'…'` strings and nested
comments. A backtick or bracket name containing a quote swallows the rest of
the script into one statement. Those are live bugs.

sqlparser's tokenizer handles all of those. It fails three cases:

- **Unterminated strings and comments are errors.** That's the normal state of
  the editor while someone types, and splitting has to be a total function.
- **DuckDB block comments don't nest** in sqlparser's DuckDB dialect.
- **Speed:** 60 ms for 1 MB natively. That's fine for the editor and slow for
  a large pasted script.

`{{param}}` tokenises as four braces around a word. That's harmless, but no
help for substitution, which needs each engine's string-literal boundaries
anyway (`query-params.ts`).

Recommendation: keep a hand scanner in `seaquel-sql`, a port of
`engine/sql-scan.ts`, and build statement splitting, statement at cursor,
`{{param}}` detection and substitution, and the read-only check on it. Use
sqlparser only where an AST is needed. Porting fixes the splitter bugs above,
which changes behaviour for MySQL and SQL Server users, so it deserves a line
in the release notes. Neither scanner handles SQL Server's `GO` batch
separator. That's out of scope.

## Recommendation

Replace node-sql-parser everywhere, the tutorial included. Keeping it for the
tutorial would buy nothing: the lesson criteria don't read the AST, and the
parser port reached parity in about an hour of work. It would also keep 420 KB
gzipped of JS in the bundle next to the WASM.

For 2b:

1. Port `parseSql`, the visual AST and column sources as in the spike, at
   parity. Keep the tutorial parser's quirks (dropped IN/IS NULL/BETWEEN
   filters, the connector shift, lost aliases) and fix them in a follow-up.
   The criteria and the builder depend on them, so they deserve their own
   change and their own review.
2. Take the visual AST fixes now. They're bugs users can see.
3. Pass the connection's dialect to the query builder parse. Only the tutorial
   stays in PostgreSQL mode.
4. Port `sql-scan.ts` as the one scanner, drop `splitSqlStatements`' state
   machine, and put `query-params.ts` on top of it.
5. Ship one module, initialised in the root `+layout.ts`. At 3–27 ms it
   doesn't need lazy loading. Add `'wasm-unsafe-eval'` to the Tauri CSP.

## Revised 2b estimate

| Part | Design doc | Revised | Why |
|---|---|---|---|
| TS baseline | 3–5 h | 1–2 h | The harness exists; freeze its 172 entries and 140 acceptance statements as fixtures |
| Scanners and `parse_create_table` | 4–6 h | 4–6 h | Unchanged. `sql-scan.ts` is the model, but `query-params.ts` is 715 lines of per-engine substitution, and `parse_create_table` wasn't measured |
| AST helpers (visual AST, column sources, tutorial, builder parse) | 10–16 h | 4–6 h | Ported to parity in the spike; left: API shape, generated types, fixtures, engine dialect for the builder, the six formatting differences |
| `seaquel-wasm` in three builds | 3–5 h | 3–5 h | The Vite side is solved. Build steps for npm, CI, Docker and the website's `demo:update`, the pinned wasm-bindgen-cli, the CSP and a vitest setup file are the rest |
| Switch call sites, delete TS | 2–4 h | 2–4 h | Unchanged |
| Review fixes (about a quarter) | 5–9 h | 4–6 h | |
| **Total** | **27–45 h** | **~18–29 h** | |

Phase 2's lesson was that parts sitting on a library with its own behaviour
doubled. The spike measured that library for the AST work, and it behaved. The
risk that's left is in the build plumbing, and that's why its row didn't
shrink.

## Not measured

- A real Tauri window. WKWebView was stood in for by Playwright's WebKit with
  Tauri's CSP applied, and the MIME type comes from reading Tauri's source.
  WebView2 (Windows) and WebKitGTK (Linux) weren't tried at all.
- Download time over a real network. Every load came from localhost.
  adapter-node runs with `precompress: false`, so the web build sends the
  1.56 MB uncompressed unless a proxy compresses it.
- Repeat-visit code caching, memory use of the instance, and slow machines.
- `parse-create-table.ts` against sqlparser's `CreateTable`, the
  `query-params.ts` substitution port, and Monaco completion ranking (listed
  among the keystroke-rate paths in the design doc).
- Lone surrogates mid-edit. wasm-bindgen turns them into U+FFFD, which is
  also one UTF-16 unit, so offsets should stay aligned, but I didn't test it.

## Files

- `spike/README.md`: how to run everything below
- `spike/sql-spike/`: the sqlparser-rs port (`tutorial.rs`, `visual.rs`,
  `column_sources.rs`, `scan.rs`, `offsets.rs`) and the `spike-cli`,
  `accept-cli` and `scan-probe` binaries
- `spike/wasm-stub/`: stub `seaquel-wasm`, `build.sh`, `bench-node.mjs`,
  `browser-check.mjs`, `demo-editor-check.mjs`
- `spike/harness/`: `corpus.ts`, `harness.test.ts`, `acceptance.test.ts`,
  `scanners.test.ts`; output in `spike/harness/out/` (gitignored)
