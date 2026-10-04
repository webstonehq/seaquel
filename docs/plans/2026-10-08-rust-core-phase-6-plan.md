# Phase 6 Implementation Plan: `seaquel-ai`

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (or superpowers:executing-plans) to implement this plan task by task.

**Status:** done: built, Checkpoint 6b passed, owner's manual checks passed. Executed 2026-10-02 (Tasks 1–10, Checkpoint 6a, the probe and its fixes, F4 included); the measured cost is in the design doc ("Phase 6 cost"), the release notes and manual checks below. The owner answered Q1–Q9 on 2026-10-08, each with the recommended option ("Answered questions"). Surveyed on 2026-10-02 at HEAD `ef5014a` (Clean up), which holds the cleanup pass (`docs/plans/2026-10-07-cleanup-pass.md`) committed, including its migration `0006_history_params.sql`; the tree was clean. Line numbers are as of that commit. Spikes ran the same day (see "Spikes"); their code is in the session scratchpad (`…/scratchpad/p6-spikes/`) and isn't kept. No real model provider was called and no real API key was used: every spike ran against a local mock.

**Goal:** Model calls leave the webview. The in-app assistant's tool loop, its provider clients, its prompts and its tools run in Rust, and the assistant and the MCP server share one tool registry. On desktop the key never reaches the page. On web `seaquel-server` calls the model with the key the page sends per call (Q1). The inline SQL prompt, the model list and the provider test go through Core too, and the inline prompt only inserts what it generates (Q9). The demo gets the assistant back on Core in the page, with a key the visitor enters for the session (Q2).

**After this phase, nothing the assistant decides is decided in TypeScript.** What the model is sent, which tools it may call, how a result is cut and what is stored in the chat come from one crate, on every interface that has the assistant, and the MCP server's tools are the same code.

**Architecture:**
- **`seaquel-ai`** (new domain crate, wasm-clean): the provider wire for Anthropic's Messages API and OpenAI-compatible Chat Completions (request bodies, SSE framing, turn decoding), the tool registry (names, descriptions, JSON schemas, argument parsing, result rendering for the assistant and for MCP), the system prompt, the schema context, `@mention` expansion and `AiLimits`. Pure apart from an `HttpClient` trait that someone else implements.
- **`seaquel-http`** (new infra crate, native only): `seaquel-license`'s `http.rs` moved out (the OS and webpki roots, the fallback, `NODE_EXTRA_CA_CERTS`, proxies), plus a streaming POST and the web's egress guard (Decision 9). The license crate keeps using it.
- **`seaquel-core`'s `ai` feature:** the loop that drives a turn (`Workspace::ai_chat`), tool execution through the read-only paths Core already has, keys (keychain, or supplied with the call), sharing flags, approvals and client tools, and writing the turn into the chat. `ai-native` adds the reqwest client. Two new builder policies with no default, as with `ConnectPolicy` and the executor: an `HttpClient` and an `AiEgress`.
- **`seaquel-rpc`:** an `ai` group: `chat` (stream only), `respond`, `generate`, `models`, `test`. A third stream kind, `CoreEvent::Ai`.
- **`seaquel-mcp`:** its eight tools call the shared registry. Names, arguments, results and `INSTRUCTIONS` don't change; its test suite is the check.
- **Storage:** migration `0007_ai_message_parts.sql` (a nullable `parts` column for stored tool calls; Q8).
- **TypeScript:** an `AiService` seam (`CoreAi`) the assistant, the inline prompt and the AI settings use. `services/ai/providers.ts`, the tool definitions, the tool loop, `runAndFormat` and `resolveMentions` go. The approval card and the dashboard tools stay in the page, as answers to Core's requests.

**Tech stack:** Rust (`seaquel-ai`, `seaquel-http`, `seaquel-core`, `seaquel-rpc`, `seaquel-mcp`, `seaquel-server`, `src-tauri`, `seaquel-browser`), reqwest 0.12.28 (the workspace's), rmcp 3.4.1 and schemars 1 (already used), TypeScript/Svelte 5, vitest, and local mock providers (Rust and Node) for every test that would otherwise reach a model.

**Inputs:**
- The design doc: the `seaquel-ai` row of the crate table, the `LlmProvider` plugin kind, "Secrets" (the per-request web secrets), "MCP tool set (first cut)", "CLI (first cut)", phase 6 in the migration plan, "As built in phase 8" (no AI in the demo until phase 6), "Risks" ("AI on web"), and every phase cost section for the estimate.
- The AI safety plan and its cost section: the read-only paths every AI query takes, and its Follow-ups (tool errors marked as errors, "Allow all" carrying over between connections).
- The phase 4 plan (the MCP server) and the 5d plan's Decisions 20 and 24 and Q17/Q19 (AI settings, keys, chats and their web budget).
- Phase 8's Q7 (the demo's assistant is off until this phase) and Decision 13 (the module).
- The effort logs of 5d, 5e and phase 8. Review fixes ran at 64–72% of first passes in 5d and 5e and 28% in phase 8; the AI safety phase ran at about 40% where code decided what SQL runs. Probes found real bugs every time.
- A read-only survey of the TypeScript AI stack, `seaquel-mcp`, Core's read-only query and EXPLAIN paths, AI settings and chats in Core, the web vault, the web server and the transports, below.

**Naming.** "The assistant" is the chat panel. "The inline prompt" is the editor's generate-SQL box. "A turn" is one user message and everything the model does until it stops. "A round" is one request to the provider inside a turn. "A tool call" is one call the model asks for. "A client tool" is one that only the page can run (the dashboard tools). "The registry" is `seaquel-ai::tools`. "The mock" is a local HTTP server that speaks a provider's wire.

---

## What the code shows

### Providers and how they're called

1. **Two provider types, both called with `fetch` from the page.** `AIProviderType` is `"anthropic" | "openai-compatible"` (`src/lib/types/ai.ts:1`). `services/ai/providers.ts` has four call sites:
   - Anthropic streaming (`:95`) and non-streaming (`:183`), to `https://api.anthropic.com/v1/messages` with `x-api-key` and `anthropic-version: 2023-06-01`;
   - OpenAI-compatible streaming (`:284`) and non-streaming (`:376`), to `${baseUrl}/chat/completions` with an optional `Bearer` key; the base URL defaults to `https://api.openai.com/v1` (`:217`).

   Anthropic requests send `max_tokens`: 4,096 streaming (`:5`), 2,048 non-streaming (`:6`); OpenAI-compatible requests send none. The model list and the provider test call `/v1/models` or `${baseUrl}/models` from the page too (`stores/ai-settings.svelte.ts:219-263`).
2. **The SSE handling is line-based on decoded text** (`providers.ts:118-159`, `:303-347`): `data: ` lines are parsed as JSON; `event:` names, comments and multi-line data are ignored. Anthropic reads `content_block_start` (tool_use only), `content_block_delta` (text and `input_json_delta`) and `message_delta`'s `stop_reason`. OpenAI reads `delta.content`, `delta.tool_calls[]` by index and `finish_reason`.
3. **The webview's network rules allow any URL.** The desktop CSP's `connect-src` is `'self' ipc: http://ipc.localhost https: http: ws: wss:` (`src-tauri/tauri.conf.json:24`). Besides the AI calls, the only external `fetch` in the page is the DuckDB community extension list (`hooks/database/extensions-duckdb-tabs.svelte.ts:171`). The web build sends no CSP. There is no Tauri HTTP plugin; the page's `fetch` is the webview's.

### The tool loop and the tools

4. **The loop** is `sendAIMessage` (`services/ai/index.ts:212-281`): stream a round; with a tool call, run it, append the assistant's `tool_use` and the `tool_result`, and go again; at most 20 tool calls per message (`:21`, `:261-265`). Each turn starts from the stored messages' text only (`:236`): tool calls and their results are never kept, so a follow-up question doesn't know what the queries returned.
5. **Six tools.** `run_query { query }` (`tool-definitions.ts:7-21`) is offered only when the connection shares data (`index.ts:231`). The five dashboard tools (`create_dashboard`, `add_widget`, `get_dashboard`, `update_widget`, `remove_widget`; `:23-269`) are offered whenever the page passes their callbacks, which it always does (`hooks/database/ui-state.svelte.ts:436`), whatever the sharing flags.
6. **`run_query`** (`index.ts:198-210`): the read-only check for the chat's connection type (`context.ts:13-16`), then either straight to `runAndFormat` (session-wide "Allow all", `ui-state.svelte.ts:21`, snapshotted per turn at `:396`) or after the approval card (`index.ts:142-163`). `runAndFormat` (`context.ts:43-59`) runs `executeReadOnly` with `maxRows = RUN_QUERY_MAX_ROWS = 1000` (`:27`) and gives the model:
   - `Query returned no rows.` for an empty result;
   - otherwise `buildDataContext`: a Markdown table of **the first 5 rows** (`:98`), cells through `String(v)` (`:93`), bytes as `<n bytes>`;
   - the truncation note (`:62-68`) before it when the result was cut;
   - `Query error: <message>` on failure, `Query cancelled` on Stop.
7. **The dashboard tools** run in the page (`dashboard-tools.ts:91-199`). `add_widget` and `update_widget` refuse unless the active connection is the chat's (`index.ts:126-133`), since a widget runs against the active connection; their query gets the same read-only check but no approval; `create_dashboard` opens a tab (`ui-state.svelte.ts:233-240`), and a widget runs once when added (`:248-251`). `get_dashboard` returns the in-memory dashboard (`:254-262`). The last dashboard id is injected into the last user message as `[Context: The active dashboard ID is "…"…]` (`:182-209`).
8. **The prompt** (`context.ts:114-139`): one line naming the engine, the schema context when the connection shares its schema (`buildSchemaContext`, `:70-86`: `Table: <name>` without its schema, columns with `NOT NULL`, indexes), a line about Markdown code blocks, and the dashboard guidelines when dashboard tools are on.
9. **`@mentions`** (`services/ai-mentions.ts:148-189`) append a "Referenced context" block for each mentioned table (its columns, keys and foreign keys), saved query (its SQL) or dashboard (its widgets' queries). `resolveMentions` is called with the schema, saved queries and dashboards whatever the sharing flags (`ui-state.svelte.ts:115-120`).
10. **The inline prompt** (`components/query-editor/ai-inline-prompt.svelte.ts:68-80`) calls `generateSQL` (`index.ts:295-322`: non-streaming, the same system prompt, the first fenced block of the reply), inserts the SQL at the cursor and calls `onExecute`, which is the editor's Run (`query-editor.svelte:91` → `execution.svelte.ts:75`): the whole tab runs through `db.run`, not through `executeReadOnly`.

### Keys

11. **Desktop:** Core writes `ai-api-key:<id>` in the settings call (5d-2, Decision 20; `crates/seaquel-core/src/state.rs:1157-1300`). The page then reads it back on every message, model list and test with `secret.get` (`services/keyring.ts:145-147`, `index.ts:221`, `:301`), and sends it from the webview. Any script in the webview can read every provider's key.
12. **Web:** Core refuses a key in the settings call (`state.rs:1142-1153`, `NOT_SUPPORTED`). The vault keeps it encrypted in `user_credentials` under `ai-api-key-provider` (`services/vault/vault-keyring.ts:115-122`); the page decrypts it after the vault is unlocked and calls the provider from the browser. The server never sees the key or the prompt. There is no AI route on the server: `src/routes/api` has `account`, `airgap`, `auth`, `rpc`, `signup` and `team`.
13. **Demo:** `NoopKeyringService` (`keyring.ts:171-204`) returns no key, and `features.aiAssistant` is `!demo` (`features/index.ts:76`; phase 8's Q7).

### Chats and settings in Core (5d-2)

14. **AI settings** are one `app_state` record (`aiSettings`), rewritten field by field by `aiSettingsPatch` and `aiProvider{Create,Update,Remove}` (`crates/seaquel-rpc/src/settings.rs:70-100`; `state.rs:1090-1300`). The global sharing defaults are schema on, data off. Each connection row has `aiShareSchema`, `aiShareData`, `activeAIProviderId` and `activeAIModel`.
15. **Chats** are `chatCreate`/`chatUpdate`/`chatRemove`/`chatMessagesPut`/`chatMessagesRemove` and the two lists (`crates/seaquel-rpc/src/library.rs:183-205`). Messages are `{id, role, content, timestamp, query?, dashboardId?}` with GUI-made ids (`crates/seaquel-workspace/src/state.rs:1479-1491`). The page puts a turn's messages at its end, on an error, on Stop and when a pending approval is aborted (`hooks/database/ai-chat-manager.svelte.ts:223-260`). The web caps a chat at 5,000 messages, 1 MiB each and 64 MiB in all, and 50 providers (`crates/seaquel-server/src/lib.rs:147-153`).

### The read-only query path

16. **Every AI query, from the page and from MCP, goes through Core's read-only stream.** The page: `QueryCrud.executeReadOnly` (`hooks/database/query-crud.svelte.ts:390-428`; re-reads the connection after its last await and runs the TypeScript read-only check first) → `CoreProvider.selectReadOnly` (`providers/core-provider.ts:93-103`) → `db.queryStream` with `readOnly` and `maxRows`. Core: `query_stream_as` (`crates/seaquel-core/src/lib.rs:1173-1260`) runs `check_read_only` (`seaquel_sql::read_only_error`, `crates/seaquel-sql/src/read_only.rs:274`, with `BLOCKED_FUNCTIONS_DUCKDB` at `:154`) before the driver's `query_read_only_with`. `QueryOptions` takes `max_rows`, `max_bytes` and `timeout`, only with `read_only` (`lib.rs:600-680`). Only `max_rows` crosses the transports, so the page's AI queries have no byte budget and no timeout; Stop is their only end.

### The MCP server

17. **Eight tools** (`crates/seaquel-mcp/src/server.rs:357-479`), each resolving `connection` against the set exposed on the command line, checking sharing (re-read per call, `server.rs:237-249`), connecting on first use with `restricted` and known SSH hosts only (`:255-276`), under a 60 s call timeout that leaves out keychain prompts (`:290-336`):
    - `list_connections`; `list_schemas`, `list_tables`, `describe_table` (schema sharing; `tools/schema.rs`);
    - `run_query { connection, sql, max_rows? }` and `explain_query { connection, sql }` (data sharing; `tools/query.rs`): default 100 rows, at most 1,000, an 8 MB fetch budget, a 4 MB result, a 60 s statement timeout, rows as JSON arrays with notes when cut;
    - `list_saved_queries`, `run_saved_query` (`tools/saved.rs`: parameters filled as the editor's dialog does, through `seaquel_sql::params::substitute`).

    Cells render like `$lib/values`' `cellText` (`format.rs`), cut at 64 KB. Errors are tool results, `CODE: message`. About 3,000 production lines and a 1,915-line test file (`tests/tools.rs`); the CLI's `tests/stdio.rs` runs the binary.
18. **The sharing rule exists twice:** `exposed::sharing` and `global_sharing_from` (`crates/seaquel-mcp/src/exposed.rs:192-260`) and `_resolveAISettings` (`ui-state.svelte.ts:160-171`). MCP ignores `aiSettings.enabled`.
19. **What the two tool sets don't share:** the assistant has dashboard tools and approvals; MCP has introspection, EXPLAIN and saved queries, a timeout, byte budgets and JSON results. The assistant's `run_query` argument is `query`; MCP's is `sql`.

### Transports

20. **A stream is one of a fixed list on each transport.** `dispatch_stream` (`crates/seaquel-rpc/src/db.rs:660-676`) serves `db.queryStream`, `run`, `page` and `tablePage`, yielding `CoreEvent::Stream` or `CoreEvent::Run` (`:326-359`; `DbRequest::is_run_method`, `:189`). The desktop's `core_stream` finds the stream id per request (`src-tauri/src/lib.rs:475`, `:506`); the web's socket checks the method (`crates/seaquel-server/src/routes/rpc_stream.rs:240`, `:524`); the demo's module calls `dispatch_stream` (`crates/seaquel-browser/src/module.rs:162-173`). Each stream holds one of the web socket's 16 slots.
21. **HTTP from Rust exists only in `seaquel-license`** (`src/http.rs`: reqwest 0.12 with rustls, the OS and webpki roots, a fallback when the OS store is broken, extra roots from `NODE_EXTRA_CA_CERTS`, `HTTP(S)_PROXY`/`NO_PROXY`; the desktop adds `system-proxy`). `LazyClient` is `pub(crate)`.

### What the CLI and MCP would gain

22. **MCP:** nothing visible unless Q5 adds tools. Its formatting and tool bodies move into the registry, so the assistant's fixes and MCP's limits are one piece of code.
23. **The CLI:** the ability to run a turn (`seaquel-cli ask`) with the same loop, keys from the keychain and the same tools, once phase 7 gives it commands (Q6). It opens storage read-only today, so it couldn't store a chat.

### Bugs and gaps the survey found

"Seen" means reproduced in a spike against a mock; "by reading" means not reproduced.

1. **Anthropic from a browser needs a header nobody sends** (by reading). Anthropic's API refuses CORS requests unless they carry `anthropic-dangerous-direct-browser-access: true`; `providers.ts:95`, `:183` and `ai-settings.svelte.ts:221` don't send it. The web page is a browser, and so, to CORS, is the desktop webview (`tauri://localhost` on macOS, `http://tauri.localhost` on Windows). If that holds, Anthropic works on neither today, and only OpenAI-compatible servers that allow CORS do. Not checked against the real API (ground rule); one try with a real key settles it. It stops mattering once calls leave the page; the demo's browser client sends the header (Decision 10).
2. **The desktop key crosses into the webview on every message** (item 11). Fixed by Decision 7.
3. **Only one tool call per round** (by reading; S1 decodes the documented shape with two). Anthropic: each `tool_use` block resets the id, name and input (`providers.ts:141-144`), so only the last one runs. OpenAI-compatible: only index 0 (`:349-359`). The model's other calls vanish without a result.
4. **A provider error mid-stream ends the turn as if it were done** (by reading; S1 decodes the documented `event: error`): Anthropic's `error` event (`overloaded_error`, …) isn't read, so the half answer is stored as the reply.
5. **A reply cut by `max_tokens` says nothing** (by reading): `stop_reason: "max_tokens"` is treated as the end.
6. **The model sees 5 of up to 1,000 fetched rows** (`context.ts:98`), a JSON object cell reads `[object Object]` (`:93`), and `|` or a newline in a cell breaks the table (by reading).
7. **The schema context has no schema names and no size cap** (`context.ts:74`; by reading). Tables with one name in two schemas can't be told apart, and a 5,000-table catalog goes whole into every round of every turn.
8. **`@mentions` ignore the sharing flags** (item 9; by reading). With schema sharing off, `@public.users` still sends the table's columns, and a mentioned saved query or dashboard sends its SQL.
9. **The inline prompt runs what it generates through the editor** (item 10; by reading). The whole tab runs read-write; only the destructive-statement prompt and pending changes (on by default) stand between the model's SQL and the database. CLAUDE.md says AI SQL runs only through `executeReadOnly`. See Q9.
10. **"Allow all" covers every connection for the session** (`ui-state.svelte.ts:21`), and ticking it while a turn runs approves only that one query, since the flag is read at the turn's start (`:396`). The first half is an AI safety Follow-up.
11. **No timeouts.** A provider that stops sending bytes holds the turn until Stop; the non-streaming calls (inline prompt, model list, test) take no signal at all (`providers.ts:183`, `:376`; `ai-settings.svelte.ts:221`, `:232`).
12. **Provider error bodies go to the log** (`providers.ts:198`, `:203`, `:385`, `:392`) and into the chat as `Error: <body>` (`ui-state.svelte.ts:220`).
13. **Tool refusals reach the model as plain text** (`Query error: …`), not as errors (AI safety Follow-up).
14. **The page's AI queries have no byte budget and no timeout** (item 16), where MCP's have 8 MB and 60 s.

---

## Spikes (2026-10-02)

Each ran in `…/scratchpad/p6-spikes/` with `CARGO_TARGET_DIR=…/scratchpad/p5a/target`, against local mocks only. The keys were the string `test-key-not-real`.

**S1. Streaming a provider's SSE in Rust, with the tool loop and cancel.** A crate with reqwest 0.12.28 (the workspace's features) and a hand-written mock server on `127.0.0.1` that answers chunked SSE in 5-byte pieces with 1 ms gaps, so lines, JSON and multi-byte characters (`é`, `☕`) straddle reads.
- A byte-level parser (split on `\n`, drop a trailing `\r`, join `data:` lines, honour `event:`) decoded every event exactly; whole lines are always whole UTF-8, so decoding per line is safe.
- **Anthropic, two rounds:** round 1 streamed text, then two `tool_use` blocks whose inputs arrived as 7-character `input_json_delta` pieces; both parsed (`{"query": "SELECT count(*) FROM café WHERE note = '☕'"}` and `{"table":"café"}`). The second request carried the assistant's text and both `tool_use` blocks, then one user message with both `tool_result`s; the mock answered `end_turn`. Headers arrived as sent.
- **A mid-stream `event: error`** (`overloaded_error`) ended the turn as an error, after the half answer had streamed.
- **OpenAI-compatible:** two parallel tool calls with interleaved `arguments` pieces, `finish_reason: "tool_calls"`, `[DONE]`; both parsed.
- **Cancel:** dropping the request future after the first delta closed the TCP connection; the mock's next write, 52 ms later (it writes every 25 ms), failed. A dropped turn stops the provider's billing at the next write.
- The timings (first delta at 158–192 ms, two rounds in 1.4 s) are the mock's 1 ms gaps between 5-byte pieces, not reqwest.

**S2. Can Core in the page stream from a provider?** A wasm32 crate in two variants, built with `opt-level = "z"`, LTO and `panic = "abort"`, run under Node 24 against a Node mock:

| Variant | Raw | brotli |
|---|---|---|
| The SSE parser alone (baseline) | 17,314 | 7,008 |
| reqwest 0.12.28's wasm backend (`stream` feature, the browser's `fetch`) | 327,972 | 106,104 |
| A JavaScript bridge (`start`, `read`, `abort`), as phase 8's DuckDB bridge | 52,472 | 20,823 |

- Both streamed 20 deltas as the mock sent them (one every 20 ms; first at 31–44 ms), with `☕` split across two writes arriving intact.
- Both stopped the request on drop: after 3 deltas, the mock saw the connection close within 1 ms (reqwest's wasm `Response` holds an `AbortController` guard; the bridge calls `abort`).
- reqwest costs about 99 KB brotli more than the parser; most of the bridge's 14 KB is `wasm-bindgen-futures` and glue the module already has. The module is at 1,481 KB of its 2,000,000-byte budget (`scripts/build-wasm.mjs:80`).
- Node's `fetch` has no CORS, so this doesn't show what a browser allows (bug 1). Anthropic needs the direct-access header from a page, and an OpenAI-compatible server needs to allow the demo's origin.

**S3. Is a DNS filter enough to keep the web server's calls off private addresses?** A reqwest client with a custom `dns_resolver` that drops loopback, private, link-local and unspecified addresses (IPv6 and IPv4-mapped included), against a server on `127.0.0.1`:
- `http://localhost:<port>`: resolver called, refused.
- `http://127.0.0.1:<port>`, `http://[::ffff:127.0.0.1]:<port>` and `http://2130706433:<port>`: **resolver not called, request reached the server.** An IP literal, in any spelling the URL parser normalises, never goes through DNS.
- A 302 to `http://127.0.0.1:<port>/secret` was **followed** under reqwest's default redirect policy; `Policy::none()` returned the 302.
- With an HTTP proxy configured, the resolver was **never asked**; the proxy got `GET http://internal.example.invalid/v1/models`.

So the guard needs the host check on the parsed URL, the resolver, no redirects, and a rule for proxies (Decision 9).

**S4. The transports (read, not run).** A turn fits the stream model as it is: `dispatch_stream` returns a `BoxStream<CoreEvent>`, so `ai.chat` is a third stream kind next to `Stream` and `Run`, and the three transports and the module each need one more entry in their lists (item 20). An approval is a separate unary call that reaches the running turn by its stream id, as `cancel` does. Nothing needed a spike; the cost is in Task 5.

---

## Answered questions (2026-10-08)

The owner answered all nine on 2026-10-08, each with the recommended option. Each keeps the options that were weighed, so later changes start from them. The decisions below follow the answers.

### Q1. Where a web user's model calls run, and who holds the key

Today the browser calls the provider with the key it decrypted from the vault (item 12), and Anthropic probably doesn't work there at all (bug 1).
- **A:** `seaquel-server` calls the provider. The page decrypts the key from the vault as it does for database passwords and sends it with each `ai.chat`, `ai.generate`, `ai.models` and `ai.test`; Core holds it for that call only, never stores or logs it. The operator sets egress with `SEAQUEL_AI_EGRESS`: `public` (default: public addresses only, no redirects, Decision 9), `any` (private networks too, for an Ollama next to the server) or `off` (the assistant is refused with `AI_EGRESS_BLOCKED`, for air-gapped installs). Cost: in the plan (Decision 9's guard is about 1 h of it). The vault's promise changes from "the server never sees the key" to "the server sees it in memory during the call", as for database passwords today; an OpenAI-compatible server on the user's own laptop stops being reachable from web.
- **B:** the loop runs in the page: `seaquel-ai` in the editor module with the browser's `fetch`, and the tools calling Core over `/rpc`. Keys never leave the browser and a laptop Ollama keeps working, but CORS stays (Anthropic needs the direct-access header), the tool loop has two hosts (Core natively, an RPC client in the page), and approvals and limits are enforced in the page. About +4–6 h, and a second copy of the loop's rules.
- **C:** web keeps today's TypeScript path; phase 6 is desktop and demo only. No new cost now, but two implementations until someone finishes it, and bug 1 stays on web.

**Answer (owner): A.** Decisions 7, 8 and 9.

### Q2. The demo's assistant (phase 8's Q7)

- **A:** stay off. No cost.
- **B:** bring your own key, for the session only. The settings form keeps the key in page memory, never in storage or IndexedDB, and sends it with each call; the module calls the provider through a `fetch` bridge (S2: +14 KB) with Anthropic's direct-access header. OpenAI-compatible servers work if they allow the demo's origin; the form says so. About 1–1.5 h (Task 8).
- **C:** a hosted proxy with Webstone's key and a quota. Billing, abuse limits and a service to run; days of work outside this repo.

**Answer (owner): B.** Decisions 7, 8 and 10; Task 8.

### Q3. Which providers

- **A:** the two there are: Anthropic Messages and OpenAI-compatible Chat Completions, which covers OpenAI, Azure OpenAI's compatible endpoint, OpenRouter, Ollama, LM Studio, vLLM and others. The provider trait is open for more.
- **B:** add one or more native APIs now (OpenAI's Responses API, Google Gemini, AWS Bedrock). About 1–2 h each with recorded fixtures and a mock; Bedrock also needs SigV4 signing.

**Answer (owner): A.** Decision 1.

### Q4. What the assistant's tools are, and what `run_query` returns

- **A:** the assistant gets the registry's read tools: `run_query`, `explain_query`, `list_schemas`, `list_tables`, `describe_table`, `list_saved_queries` and `run_saved_query`, bound to the chat's connection (no `connection` argument), plus the dashboard tools. `run_query` and `run_saved_query` return MCP's JSON (columns once, rows as arrays, notes when cut) with an assistant budget: 100 rows by default, at most 1,000, and 256 KB of JSON, where MCP keeps 4 MB. With 256 KB a large result costs at most about 64k tokens per call. The schema in the prompt is capped (Decision 13) and the model is told to use `list_tables`/`describe_table` past the cap.
- **B:** parity: `run_query` only, and the 5-row Markdown sample. Cheapest; bugs 6 and 7 stay.
- **C:** A with MCP's 4 MB budget. Up to about a million tokens per call on the user's key.

**Answer (owner): A.** Decisions 3, 4 and 13.

### Q5. New MCP tools, or write tools

- **A:** no new tools and no writes in phase 6. The eight move onto the registry unchanged. The dashboard tools can't come to MCP yet: they write storage, which the CLI opens read-only until phase 7, and they run in the page (Decision 6).
- **B:** add read-only `list_dashboards` and `get_dashboard`. About +0.5 h.
- **C:** write tools behind a per-connection opt-in stored in the workspace (the design doc's rule). Its own review and probe; several hours, and phase 7's writable storage first.

**Answer (owner): A.** Decisions 1 and 20.

### Q6. A CLI `ask` command

- **A:** not in phase 6. Phase 7 adds the CLI's commands and writable storage; `ask` then stores its chat like the GUI. The Core API in this plan is what it will call.
- **B:** `seaquel-cli ask -c <connection> "<question>"` now: read-only storage, nothing stored, text to stdout, approvals refused unless `--allow-queries`. About +1.5–2 h with its tests.

**Answer (owner): A.** Decision 1.

### Q7. What a turn looks like while it streams, and what is kept

- **A:** text streams as today. Each tool call shows as one line in the reply (the tool, its SQL, the row count or the error), and the approval card is unchanged. Tool calls and a capped copy of their results (16 KB each) are stored with the reply and sent back on later turns within the history budget (Decision 13), so a follow-up knows what the model saw.
- **B:** parity: tool calls invisible, nothing kept (item 4).
- **C:** A plus expandable cards with the full result grid. About +1.5 h of GUI.

**Answer (owner): A.** Decisions 11, 12 and 17; Task 7.

### Q8. Chats across the upgrade, and a turn in flight

- **A:** migration `0007` adds a nullable `parts` JSON column to `ai_messages` for Q7's tool calls; stored messages don't change, and older releases ignore the column and show the text. Core stores the user's message before the first round and the reply at the end, on an error, or on Stop with what had streamed, so a crash or an update restart mid-turn loses at most the unfinished reply. Like every migration since 5d-1, `seaquel-cli mcp` refuses the file (`STORAGE_NEEDS_UPGRADE`) until the app has opened it once; the release notes say so.
- **B:** no migration: tool calls aren't stored (Q7 B), and MCP users aren't asked to open the app.
- **C:** a separate `ai_tool_calls` table. Same effect as A, more code.

**Answer (owner): A.** Decisions 12 and 21.

### Q9. The inline prompt runs the SQL it generates

Today it inserts the SQL and presses Run for the whole tab (bug 9).
- **A:** insert only. The user reads it and runs it; the box says "Inserted. Run it with ⌘↵."
- **B:** insert, then run only the inserted statement, and only when it passes the read-only check, through `executeReadOnly`; otherwise insert only.
- **C:** keep today's behaviour.

**Answer (owner): A.** Decision 18.

---

## Decisions (2026-10-08)

Settled with the answers above. Numbered from 1: phase 6 is its own phase. Each names the answers it follows.

### Scope and placement

#### 1. Scope

- In: the assistant's loop, providers, prompts, tools and chat writes; the inline prompt; the model list and provider test; the MCP server on the registry; keys out of the page on desktop; web model calls in `seaquel-server` (Q1); the demo's assistant (Q2).
- Not in: new providers (Q3), new MCP tools or write tools (Q5), the CLI's `ask` (Q6), prompt caching, configurable `max_tokens`, and a read-only database login for AI queries (the AI safety Follow-up). Each is a follow-up.

#### 2. Crates

- **`seaquel-ai`** is a domain crate (`DOMAIN_AND_INFRA` in `check-crate-deps.mjs`), reached as `seaquel_core::ai`, and builds for wasm32 (CI's wasm line gets it). Its modules: `wire` (provider requests and turn decoding), `sse`, `tools` (registry, arguments, renderers), `prompt` (system prompt, schema context, mentions, history), `limits`, `sharing` (MCP's rule, moved), and `http` (the trait). No I/O of its own, no tokio, no clock.
- **`seaquel-http`** is infra, native only: `seaquel-license::http` moved, a streaming POST, and the egress guard. `seaquel-license` and Core's `ai-native` use it.
- **Core** gets `ai.rs` and `ai/` behind the `ai` feature, which needs `workspace` and `storage`. `ai-native` adds `seaquel-http`'s client and is refused with `browser` (the `compile_error!` list). `src-tauri`, `seaquel-server` and `seaquel-cli` enable `ai-native`; `seaquel-mcp` enables `ai` for the registry only; `seaquel-browser` enables `ai` and passes its own client.

#### 3. One registry, two profiles

- `seaquel_ai::tools` defines each tool once: name, description, argument type (`serde` + `schemars`, the schema frozen in a fixture), a parser that refuses unknown or ill-typed arguments with `INVALID_ARGUMENT`, and a renderer per profile.
- **`Profile::Mcp`** is today's MCP surface, byte for byte: the `connection` argument, `sql`, `max_rows` 1–1,000 (default 100), 4 MB results, the same messages.
- **`Profile::Assistant`** is bound to the chat's connection: no `connection` argument; `run_query { sql, max_rows? }` with Q4's budget. The argument was `query`; no stored chat holds a tool call, so renaming it breaks nothing (`changes.json`).
- Core's `ai::tools::call(core, ws, &ToolContext, call)` executes any tool for either profile; MCP's `server.rs` keeps its `#[tool]` methods and calls it.

#### 4. Tools run only on paths Core already has

- Queries: `Workspace::query_stream` with `read_only`, `max_rows`, `max_bytes` (MCP's 8 MB fetch budget for both profiles) and `timeout` (60 s for both). The assistant's queries gain the byte budget and the timeout they lack (bug 14).
- EXPLAIN: `ConnectionHandle::explain_read_only`. Introspection: `schema_tables` and `table_metadata`. Saved queries: storage plus `seaquel_sql::params::substitute`, as `tools/saved.rs` does.
- No new SQL path, and no new read-only rule: Core's token check and each driver's read-only mode stay the gate. The page's AI-side `readOnlyError` pre-check goes with the loop; `executeReadOnly` stays for dashboard widgets and workflow nodes.
- A tool's refusal or failure goes to the model as an error result (`is_error: true` on Anthropic; on OpenAI-compatible, the content starts `Error:`), closing the AI safety Follow-up (bug 13).

#### 5. Sharing is decided in Core, per call

- One function, `seaquel_ai::sharing(row, global)`, moved from `exposed.rs:192-260`, decides schema and data sharing for both profiles. Core re-reads the connection row and `aiSettings` at the start of each turn and before each tool call, so a change in another window applies to the next call.
- Without schema sharing: no schema context, no introspection tools, and mentions resolve to the bare name (bug 8). Without data sharing: no `run_query`, `run_saved_query` or `explain_query`. `get_dashboard` returns widget queries only with schema sharing.
- `aiSettings.enabled = false` refuses `ai.chat` and `ai.generate` with `AI_DISABLED`. MCP keeps ignoring it: starting the server is its own opt-in.

#### 6. Approvals and client tools

- **Approval.** Before a query tool runs, the turn emits `approvalRequired { callId, sql }` and waits, unless the turn was started with `approval: "allowAll"`. `ai.respond { streamId, callId, decision }` answers `allow`, `deny` or `allowAll` (the rest of the turn runs without asking). The page keeps "Allow all" per connection for the session and sends it with each turn (bug 10).
- **Client tools.** The dashboard tools touch open tabs and the active connection, which only the page knows. The turn checks a widget's query with Core's read-only check, then emits `clientTool { callId, name, input }` and waits for `ai.respond { …, decision: { result, isError } }`. The page runs today's `handleDashboardToolCall` and answers. Offered only when the request says `clientTools: true`. MCP and a future CLI have none.
- **Waiting** is bounded only by Stop: a cancel drops the turn and its waiters. A `respond` for a stream or call the workspace doesn't have is `NOT_FOUND`; one that arrives twice is ignored. Any window of the user may answer (same workspace); the card shows only in the window that started the turn.
- **The connection.** `ai.chat` carries the chat id and the open Core connection id the tools run on. Core doesn't know which saved connection an open one came from, so `db.connect` gains an optional `savedConnectionId` that Core records on the connection, and a turn refuses a connection whose recorded id isn't the chat's (`CONNECTION_MISMATCH`). A reconnect during a turn makes the next tool fail with `CONNECTION_NOT_FOUND`, which the model sees as a tool error.

#### 7. Keys

- **Desktop:** Core reads `ai-api-key:<providerId>` from the keychain itself. `dispatch_secret` refuses `get` on `ai-api-key:*` from the webview, and `getAIApiKeyForProvider` goes, so no key is in the page after the settings form sends it.
- **Web (Q1) and demo (Q2):** `apiKey` travels in the request as a `SuppliedSecret` (redacted `Debug`, never serialized back, dropped at the end of the call). Without one, a provider that needs a key fails with `NO_API_KEY` before any request.
- Keys never reach a log line, an error, an event or a stored row. Tests use `test-key-not-real` and assert it appears in no captured log.

#### 8. The HTTP client

- **Native:** `seaquel-http`'s reqwest client: rustls, the OS and webpki roots with the existing fallback, extra roots and proxies as `seaquel-license` has them, the desktop's system proxy. Timeouts: 10 s to connect, 120 s without a byte (idle), 10 minutes per round. Past one: `TIMEOUT`.
- **Browser:** a `FetchBridge` the page passes to `open` (S2), with `start`, `read` and `abort`; dropping the stream calls `abort`.
- `HttpClient` is a builder policy with no default: without one, `ai.chat`, `ai.generate`, `ai.models` and `ai.test` answer `NOT_SUPPORTED`.

#### 9. Egress on web (Q1)

- `AiEgress` on `CoreBuilder`, with no default: `Off`, `Public` or `Any`. The desktop and CLI pass `Any`; the server reads `SEAQUEL_AI_EGRESS` (default `public`), which joins the `shared/rust-env.js` allow-list.
- `Public` (S3):
  - the URL's host, after parsing, may not be an IP literal in a loopback, private, link-local, unique-local, carrier-grade NAT, multicast or unspecified range (IPv4-mapped IPv6 included);
  - the resolver drops such addresses and fails when none are left;
  - redirects are never followed (`Policy::none()`; a 3xx is `PROVIDER_ERROR`);
  - with `HTTP(S)_PROXY` set, the host check still runs and DNS is the proxy's: the operator's proxy decides. The operator docs say so.
- `http:` is allowed only under `Any`. `Off` refuses every model call with `AI_EGRESS_BLOCKED` (503 on web).

#### 10. Provider wire: parity, then the fixes

- Request bodies are today's, field for field (Parity): `model`, `max_tokens` (Anthropic 4,096 streaming, 2,048 for `generate`), `system`, `messages`, `tools`, `stream`; `anthropic-version: 2023-06-01`; OpenAI's system message first and tools as `function`s.
- Fixed, each in `changes.json`: every tool call in a round runs and gets its result, in order (bug 3); an `error` event or a malformed event ends the turn with `PROVIDER_ERROR` (bug 4); `max_tokens` ends it with a `truncated` note (bug 5); a 429 is `RATE_LIMITED`, other non-2xx `PROVIDER_ERROR` with the status and the provider's error type, never its message, in logs (bug 12). The message is shown in the chat, cut at 1 KB.
- The browser client adds `anthropic-dangerous-direct-browser-access: true`; native clients don't.
- At most 20 tool calls per turn, counted across rounds, as today; the 21st ends the turn with `TOOL_LIMIT`.

#### 11. Events

`CoreEvent::Ai { streamId, event: AiEvent }`, with `AiEvent`:
- `started { providerKind, model }`;
- `text { delta }`, coalesced: at most one every 50 ms or per 4 KB, so a fast model doesn't send a frame per token over the socket;
- `toolCall { callId, name, sql? }` and `toolDone { callId, ok, rows?, truncated?, code? }` (Q7);
- `approvalRequired { callId, sql }` and `clientTool { callId, name, input }` (Decision 6);
- one `done { messages, seq, stop }` (`messages`: the rows Core stored for the turn; `stop`: `end` or `maxTokens`) or one `error { code, message, messages?, seq? }`.

Nothing follows a cancel. `CoreEvent::error` gains the `ai` kind, so a transport that refuses an `ai.chat` frame answers with an `ai` error.

#### 12. Core writes the turn (Q7, Q8)

- `ai.chat` carries the user message (`id`, `content`) and the assistant message's id, both made by the page as today (5d-2, Decision 24), so the page's optimistic rows keep their ids.
- Core stores the user message before the first round, and the reply (text, `query`, `dashboardId`, `parts`) at the end, on an error, or on Stop with what streamed. Each write is one `chatMessagesPut`-shaped write in Core, with its `StorageChanged` (origin: the requesting page); `done` and `error` carry the rows and their `seq`, which the page applies as it applies a run's `done.history`. The page's own `persistMessages` call for a turn goes.
- The web's chat budget is checked before the first round, so a full chat costs no model call (`CHAT_FULL`).
- Migration `0007_ai_message_parts.sql` (`0006` is the cleanup pass's `0006_history_params.sql`): `ALTER TABLE ai_messages ADD COLUMN parts TEXT` (nullable, expand-only). `parts` holds Q7's tool calls: name, arguments, `ok`, and the rendered result cut at 16 KB.

#### 13. Prompt, history and schema

- The system prompt is today's text (Parity) apart from changes listed in `changes.json`: schema-qualified names in the schema context (bug 7), and the tool guidance Q4's tools need.
- **The schema context** is built by Core from `schema_tables` at the turn's start, when the connection shares its schema. It is cut at 128 KB (whole tables only), followed by a line naming how many tables were left out and pointing at `list_tables`/`describe_table`.
- **History** is read from storage, not sent by the page: the chat's messages, newest first, until 512 KB, with `parts` rendered back into tool calls and results (Q7). The `[Context: The active dashboard ID …]` line is added as today.
- **Mentions** are resolved by Core against the schema it read and the project's saved queries and dashboards, under Decision 5's flags.

#### 14. Limits

`AiLimits` on `CoreBuilder` (none by default):
- `max_turns_in_flight` per workspace (web: 4, `TOO_MANY_REQUESTS`, 429);
- `max_message_bytes` for the user's message (web: 1 MiB, the chat budget's);
- the profile budgets of Q4 apply everywhere.

The 20-tool-call cap and the timeouts of Decision 8 apply on every interface.

#### 15. Errors

New codes: `NO_PROVIDER`, `NO_MODEL`, `NO_API_KEY`, `AI_DISABLED`, `AI_EGRESS_BLOCKED` (503), `PROVIDER_ERROR` (502), `RATE_LIMITED` (429), `TOOL_LIMIT`, `CHAT_FULL` (409), `CONNECTION_MISMATCH` (400). `TIMEOUT` and `CANCELLED` keep their meaning. The page words each one (`ai/messages.ts`, i18n); no raw provider body is shown beyond Decision 10's 1 KB.

#### 16. Logs

The AI code logs activity, ids, provider kind, model id, status, codes, round and tool-call counts, durations and token counts from `usage`. Never a prompt, message, schema, SQL, tool input or result, provider error message, URL path or query string, or key. `AiRequest`, `ChatParams`, `SuppliedSecret` and `AiEvent` have hand-written `Debug`.

### The GUI

#### 17. An `AiService` seam with one implementation

`getAi()` returns `CoreAi` on desktop, web and the demo (Q2): `chat` (an async iterator of `AiEvent`s), `respond`, `generate`, `models`, `test`. No TypeScript twin: the demo runs Core. The assistant's view model (`UIStateManager`'s AI half and `AIChatManager`) applies events; the approval card answers with `respond`; the dashboard tools answer `clientTool`. An event handler never calls back into the demo's module synchronously (phase 8, Decision 15): `respond` goes after a microtask.

#### 18. The inline prompt (Q9)

`ai.generate { connectionId (saved), request, existingQuery }` answers `{ sql }` (Core extracts the fenced block, as `index.ts:320` does). The page inserts it and doesn't run it.

*Changed in phase 7a (its Decision 24):* `ai.generate` now resolves the request's `@mentions` as a turn does; with schema sharing off the request goes as typed. The GUI's inline prompt gets it too.

#### 19. The desktop CSP

With no model calls in the page, `connect-src` becomes `'self' ipc: http://ipc.localhost https://duckdb.org`. Task 7 checks every other path that could need more: the updater and licensing are Rust, and `tauri dev`'s HMR socket (`ws://localhost:1420`) must still connect.

### Everything else

#### 20. MCP's surface doesn't change

Tool names, arguments, schemas, results, messages, codes, `INSTRUCTIONS` and limits stay as they are. `format.rs` moves into `seaquel-ai::tools::render`; `exposed.rs`'s resolution and the connect-on-first-use stay in `seaquel-mcp`. `tests/tools.rs` and `seaquel-cli`'s `tests/stdio.rs` pass unchanged.

#### 21. Older releases

`0007` is expand-only: 2026.9.x and the phase 5–8 builds open the file and ignore `parts`. A chat written by phase 6 reads in an older release as text only. An older release writing a message with `chatMessagesPut` leaves `parts` NULL, which reads as "no tool calls".

### Added in Task 1's review (2026-10-02)

The coordinator settled (a)–(g) in Task 1's review; 29–32 are the rules the fixtures needed to pin the rest. `crates/seaquel-ai/tests/fixtures/ts-baseline/changes.json` holds each as concrete values.

#### 22. Sharing turned off at call time (a)

A tool whose sharing is off when the model calls it, whether it was off from the turn's start or switched off between calls, gets an error result with MCP's code and message: `DATA_SHARING_OFF` for `run_query`, `explain_query` and `run_saved_query`, `SCHEMA_SHARING_OFF` for `list_schemas`, `list_tables`, `describe_table` and `list_saved_queries`. The check runs before the arguments are parsed and before any approval. The prompt and the tool list stay as they were at the turn's start.

#### 23. `parts` (b, Q7)

A stored reply's `parts` is an ordered list of `{round, type: "text", text}` and `{round, type: "tool", callId, name, input, ok, result, resultBytes?}`. `round` is 0-based, so one round with two calls and two rounds with one call each are told apart. `result` is the rendered result (an error as `CODE: message` with `ok: false`), cut at 16,384 bytes on a character boundary; when cut, `resultBytes` is the full length and history sends the cut copy followed by `\n(cut at 16 KB of N bytes)`. `parts` is stored only when the reply made a tool call that got a result; a call left unanswered by Stop isn't stored. History renders each round as the assistant's text and calls followed by their results (Anthropic: one user message of `tool_result`s, `is_error` on a failure; OpenAI: one `tool` message each, `Error: ` before a failure), and a round without calls as a plain assistant message.

#### 24. Client tools' input goes through the registry's parser (c)

The dashboard tools' arguments are typed like every other tool's (Decision 3): `x`, `y`, `width` and `height` are numbers, `widget_type` and the configs' `type` and `format` are enums, `text_config.content` is a string, and fields the TypeScript definitions mark required are required. An ill-typed argument is an `INVALID_ARGUMENT` error result before the page sees the call, and the model can retry. Messages are serde's, with the path from `serde_path_to_error`: keys are visited in sorted order (serde_json's map; this depends on `preserve_order` staying off), missing fields are checked afterwards in declaration order, and a problem inside a value is prefixed with its path as `serde_path_to_error` prints it (``kpi_config.format: unknown variant `currency`, expected `number` or `percentage` ``); a missing or unknown field of the root object has no prefix. The rule is the assistant profile's only: MCP's argument errors stay as rmcp words them today (Decision 20).

#### 25. Denial (d)

A denied query is the error result `DENIED: User denied query execution`.

#### 26. `get_dashboard` without schema sharing (e)

Core strips each widget's `query` from the page's answer before the model sees it (re-serialized, so keys are sorted). Core enforces sharing, never the page.

#### 27. No provider, no model (f)

`NO_PROVIDER` says "No AI provider is configured." when none is configured at all, and "The chat's AI provider no longer exists." when the connection names a deleted one. A connection without a chosen model, when providers exist, is `NO_MODEL`, "No model is chosen for this connection."

#### 28. The model list and the provider test (g)

`ai.models` answers the model ids; `ai.test` answers `null`. Either fails with an `RpcError` carrying Decision 15's code: `NO_API_KEY` before any request, `PROVIDER_ERROR` or `RATE_LIMITED` with the provider's message, `PROVIDER_ERROR` "Could not reach the provider." when unreachable. `test` checks reachability and auth only, so any 2xx passes, a body that isn't JSON included; `models` on such a body is `PROVIDER_ERROR` "The provider's answer isn't valid JSON."

#### 29. The history budget drops whole rounds

History counts the UTF-8 bytes of each stored row: a plain row's content; for a row with `parts`, each text item plus each call's input JSON and its result as sent back. Turns (a user message and its reply) are taken newest first while they fit in 512 KiB. In the first turn that doesn't fit, its user message is kept with the reply's latest whole rounds that fit, and nothing older; if no round fits, that turn goes too. A round is never split, so a `tool_use` is never sent without its `tool_result`.

#### 30. The assistant profile's results are MCP's

For the chat's connection, each assistant tool answers what MCP's tool answers with `connection` set to it: the same JSON, codes and messages (`list_saved_queries` lists the chat's project, with `connections` naming the chat's connection). Core asks for approval (Decision 6) only for `run_query`, `explain_query` and `run_saved_query`, and only after sharing, arguments, `max_rows` and the read-only check have passed. Every assistant result shares the 256 KB budget, and assistant size notes under 1 MB are in KB, never "0 MB" (the 8 MB fetch note keeps "8 MB"; MCP's own wording and 4 MB stay as they are): `run_query` and `run_saved_query` keep the rows that fit; `explain_query` cuts the plan text at 262,144 bytes on a character boundary with `"truncated": true` and `"message": "The plan is cut at 256 KB."` (pinned by `tool-results/tool/assistant/explain-query-large`); `list_tables`, `describe_table` and `list_saved_queries` keep their leading whole items (tables; columns, then indexes, then foreign keys; saved queries) that fit, with `"truncated": true` and a message naming how many are shown and the 256 KB limit.

#### 31. What a turn stores, and its errors

Core writes the user message as typed before round 1 and the reply at the end, on an error or on Stop: two writes per turn. The reply keeps the streamed text (empty when nothing streamed), `parts` (Decision 23) and Decision 12's `dashboardId` and `query` fields; the error travels with the turn (`error {code, message}`, the provider's message cut at 1 KiB), and the page words it (Task 7). Core's own refusals, `NO_PROVIDER`, `NO_API_KEY`, `NO_MODEL`, `CHAT_FULL`, `AI_DISABLED` and `CONNECTION_MISMATCH`, write nothing: Core runs those checks before it stores the user message, so `CHAT_FULL` still means no write was made (`page/no-api-key`, `storeCalls: 0`). A send the page stops itself (no model, the chat's connection removed) writes nothing either. Each connection's chat is its own: a turn's history and stored rows are that chat's only.

#### 32. Per-case log checks (I3)

Every case in `turns.json`, `page.json`, `generate.json` and `models.json` lists the strings no Rust log line may contain: the test key, the user's message, the streamed reply, each tool call's SQL and rendered result, the provider's message and the request's URL path, plus markers planted in a prompt, a schema name, a cell and a reply (`turns/anthropic/markers`). No listed string is shorter than 8 characters, so a match means the string itself and not ordinary log text (the recorder refuses a shorter one). Decision 16's lines (activity, ids, provider kind, model id, status, codes, round and tool-call counts, durations, token counts) stay allowed.

### Added in Task 3's review (2026-10-02)

#### 33. Mentions per message

A message resolves at most 100 distinct `@mentions` (`prompt::mentions::MAX_MENTIONS`); tokens past them stay as typed. Names are looked up in maps built once per message, so a 1 MiB message over a 3,000-table schema takes milliseconds.

### Added in Task 9's probe fixes (2026-10-02)

#### 34. A reply too long to store is cut, and the turn ends `tooLong` (F2)

- A reply's text may hold at most the lower of the chat's message limit (`StateLimits::max_message_bytes`, the web's 1 MiB, which the reply's write is checked against) and Core's own ceiling, `AiLimits::max_reply_bytes`. Its default is `limits::MAX_REPLY_BYTES` (1 MiB), so the desktop and the demo, which set no message limit, never store a multi-MB reply. `AiLimits` now has a hand-written `Default` (`AiLimits::DEFAULT`); `WEB_AI_LIMITS` spreads it.
- Once the streamed text reaches the cap minus the note, Core keeps the longest prefix that ends on a character boundary, drops the provider's body (the request is cancelled; the round's unrun tool calls are dropped) and ends the turn with `done { stop: "tooLong" }`. A new `AiStop` variant, not `maxTokens`: the model didn't stop, Core stopped reading, and the page words the two differently. `ts-rs` adds `"tooLong"` to `AiStop`; older pages never see it (the page and Core ship together).
- The stored reply ends with `seaquel_workspace::ai::REPLY_CUT_NOTE` (`\n\n[Seaquel cut this reply here: it was longer than a stored reply may be.]`), so later turns' history tells the model, and a reload still knows. With a limit under twice the note's length the text is cut at the limit itself and stored without it.
- The page takes the note off a stored assistant row (`messageFromWire`, `ai/reply.ts`'s `splitCutNote`; `messageDraft` puts it back), marks the message `cut` and shows `ai_reply_too_long` under it, live and after a reload. `reply.test.ts` pins the TS copy of the note against the Rust source.

---

## The split

**Two slices in one plan, 6a and 6b, each with a checkpoint.**

- **6a: Core talks to models (Tasks 1–6).** The baseline fixtures, both new crates, Core's loop and tools, the RPC group and transports, and MCP on the registry. Nothing in the GUI changes. Checkpoint 6a: MCP's suite and the CLI's stdio test pass unchanged, the mock-provider suites pass on all three transports, and one full live run with every engine.
- **6b: the GUIs on Core (Tasks 7–10).** The page switches to `AiService`, the TypeScript loop goes, the demo's assistant comes back (Q2), then the probe and the final checkpoint.
- **Why not one slice:** 6a changes MCP under a shipped binary and adds a migration; it should pass its live run before the page depends on it. **Why not two plans:** 6b has no design of its own.

---

## The wire and the API

### `seaquel-ai`

```rust
pub mod http {
    #[seaquel_runtime::async_trait]
    pub trait HttpClient: MaybeSend + MaybeSync {
        async fn post(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
    }
    pub struct HttpRequest { pub url: String, pub headers: Vec<(String, Redacted<String>)>, pub body: Vec<u8> }
    pub struct HttpResponse { pub status: u16, pub body: BoxStream<'static, Result<Vec<u8>, HttpError>> }
}
pub enum ProviderKind { Anthropic, OpenAiCompatible }
pub struct Provider { pub kind: ProviderKind, pub base_url: Option<String>, pub model: String }
pub mod wire {
    pub fn round_request(p: &Provider, key: Option<&str>, round: &Round, browser: bool) -> HttpRequest;
    pub struct Decoder;                                   // per provider; feed bytes, take RoundEvents
    pub enum RoundEvent { Text(String), ToolCall(ToolCall), Stop(StopReason), Usage(Usage) }
    pub fn generate_request(..) -> HttpRequest;  pub fn models_request(..) -> HttpRequest;
}
pub mod tools {
    pub enum Profile { Assistant, Mcp }
    pub fn definitions(profile: Profile, sharing: Sharing, client_tools: bool) -> Vec<ToolDefinition>;
    pub fn parse(profile: Profile, name: &str, input: &serde_json::Value) -> Result<Call, ToolError>;
    pub mod render { /* run_query rows, describe_table, explain, saved queries; MCP's format.rs */ }
}
pub mod prompt {
    pub fn system(engine: SqlEngine, schema: Option<&SchemaContext>, dashboards: bool) -> String;
    pub fn schema_context(tables: &[SchemaTable], max_bytes: usize) -> SchemaContext;
    pub fn mentions(text: &str, sharing: Sharing, tables: &[SchemaTable], queries: &[..], dashboards: &[..]) -> String;
    pub fn history(messages: &[PersistedAIMessage], max_bytes: usize) -> Vec<Message>;
}
pub fn sharing(row: &PersistedConnection, global: Sharing) -> Sharing;
pub struct AiLimits { pub max_turns_in_flight: Option<usize>, pub max_message_bytes: Option<usize> }
```

### Core

```rust
// crates/seaquel-core, feature `ai` (`ai-native` adds seaquel_http::NativeHttp)
impl CoreBuilder {
    pub fn ai_http(self, client: Arc<dyn HttpClient>) -> Self;     // no default: NOT_SUPPORTED
    pub fn ai_egress(self, egress: AiEgress) -> Self;               // no default: NOT_SUPPORTED
    pub fn ai_limits(self, limits: AiLimits) -> Self;
}
impl Workspace {
    pub fn ai_chat<'a>(&'a self, core: &'a Core, req: ChatRequest, origin: WriteOrigin) -> BoxStream<'a, AiEvent>;
    pub fn ai_respond(&self, stream_id: &str, call_id: &str, decision: Decision) -> Result<(), CoreError>;
    pub async fn ai_generate(&self, core: &Core, req: GenerateRequest) -> Result<String, CoreError>;
    pub async fn ai_models(&self, core: &Core, provider_id: &str, key: SuppliedSecret) -> Result<Vec<String>, CoreError>;
    pub async fn ai_test(&self, core: &Core, provider_id: &str, key: SuppliedSecret) -> Result<(), CoreError>;
}
pub mod ai::tools { pub async fn call(core: &Core, ws: &Workspace, ctx: &ToolContext, call: Call) -> ToolOutput; }
pub enum AiEgress { Off, Public, Any }
```

### RPC

```ts
// the `ai` group (unary but `chat`)
{ method: "ai", params: { method: "chat", params: {
    streamId, chatId, connectionId, userMessage: { id, content }, assistantMessageId,
    approval: "ask" | "allowAll", clientTools: boolean, apiKey?: string } } }   // stream only
{ method: "respond",  params: { streamId, callId, decision: "allow" | "deny" | "allowAll" | { result: string, isError: boolean } } }
{ method: "generate", params: { connectionId, request, existingQuery, apiKey? } }   // → { sql }
{ method: "models",   params: { providerId, apiKey? } }                            // → string[]
{ method: "test",     params: { providerId, apiKey? } }                            // → null
// events
{ type: "ai", streamId, event: AiEvent }
// db.connect and db.test gain savedConnectionId?: string (Decision 6)
```

`ai.chat` is served by `dispatch_stream` only; `dispatch_workspace` refuses it with `INVALID_ARGUMENT`, as it refuses `db.run`. The stream lists on the desktop (`src-tauri/src/lib.rs:506`), the web socket (`rpc_stream.rs:240`, `:524`) and `DbRequest::is_run_method`'s callers grow one entry each, through one `Request::stream_kind()` so the next stream kind is one line.

### TypeScript

```ts
// src/lib/hooks/database/ai/ (new)
export interface AiService {
  chat(params: ChatParams, signal?: AbortSignal): AsyncIterable<AiEvent>;
  respond(streamId: string, callId: string, decision: AiDecision): Promise<void>;
  generate(params: GenerateParams): Promise<string>;
  models(providerId: string): Promise<string[]>;
  test(providerId: string): Promise<void>;
}
export function getAi(): AiService;   // CoreAi everywhere; web and demo add the key from the vault or the session
```

Deleted: `services/ai/providers.ts`, `tool-definitions.ts`, `index.ts`'s loop and `generateSQL`, most of `context.ts`, `resolveMentions` (the popover's `buildMentionItems` stays), `fetchProviderModels` and its callers' `fetch`, and `getAIApiKeyForProvider` on desktop.

---

## Parity

The rule, as in phases 2–8: **record today's TypeScript before it moves, replay the records against Rust, and list every intended difference.** The records are frozen; a difference not listed is a bug.

### Recorded first (Task 1)

A vitest recorder (kept in `docs/plans/artifacts/`) runs today's `services/ai` with `fetch` replaced by a recorder that answers scripted SSE, and writes `crates/seaquel-ai/tests/fixtures/ts-baseline/`:

- **Request bodies and headers** (the key replaced by `<key>`), for both providers: a plain question; schema sharing on and off; data sharing on and off; dashboard tools on; every round of a turn with one tool call; a turn that hits 20 tool calls; `generateSQL` with and without an existing query; the models request.
- **The system prompt** for each engine, with and without schema and dashboard tools, and `buildSchemaContext` over a schema with indexes, nullable columns, views and two same-named tables.
- **Tool results as the model gets them:** `runAndFormat` for empty, small, exactly-5, truncated, bytes, `bigint`, `SqlDecimal`, JSON object, NaN, a `|` and a newline in a cell, an error and a cancel; the read-only refusal; every dashboard tool's JSON for success and each refusal.
- **Mentions:** each kind, quoted and unquoted, case, duplicates, unknown names, and the dashboard-id injection.
- **Turn outcomes** from scripted streams: text only, one tool call, two tool calls in one round (today: the last runs), a mid-stream `error` event (today: a silent end), `max_tokens`, a 429, a 500 with a body, malformed JSON in an event, `[DONE]` early.
- **The error wording** the page shows (`_formatAIError`, `ui-state.svelte.ts:214-221`).

`README.md` names the commit, how to run the recorder and what can't be recorded (timing, a real provider's behaviour).

### Replayed

- `seaquel-ai`'s `tests/ts_baseline.rs` builds the same requests, prompts, contexts, results and outcomes and compares them to the records, skipping exactly what `changes.json` lists.
- MCP's `tests/tools.rs` (1,915 lines) and the CLI's `tests/stdio.rs` run unchanged over the registry (Decision 20).
- The page's tests that drive the loop (`services/ai/index.test.ts`, `hooks/database/ai-chat-core.svelte.test.ts`, `ai-chat-stream.svelte.test.ts`) move to a fake `AiService` that plays recorded `AiEvent` sequences; what they checked about the loop moves to Core's tests with the mock.

### New checks

- **Mock providers** in `seaquel-ai`'s test support (Rust, S1's server grown up) and a Node one for the module (S2's). Every Core, RPC and transport test that runs a turn points at one. They script rounds, split bytes anywhere, stall, close early, send errors and 3xx, and record request bodies.
- **Egress:** S3's cases as tests, plus IPv6 literals, `0.0.0.0`, `169.254.169.254`, `100.64.0.0/10`, a name that resolves to both a public and a private address, and a 302.
- **Keys:** no test log, event, error or stored row contains the test key (a capture over every AI test).

### `changes.json`

`crates/seaquel-ai/tests/fixtures/ts-baseline/changes.json` lists each intended difference with its decision: bugs 3–8, 10, 12 and 13; Q4's results and tools; Q7's stored tool calls; schema-qualified names; the `sql` argument; the schema cap. A difference the probe finds that isn't listed is a finding, not an entry to add.

---

## Ground rules

Phase 8's, unchanged:
- no git writes;
- conventions: `errorToast`, svelte-autofixer, oxfmt, `i18n-translator` for new keys, never edit `src/lib/components/ui/*`;
- the Core crate rules (`npm run crates:check`; `seaquel-ai` and `seaquel-http` classified);
- parallel agents own their files and make small, re-read edits to shared ones;
- tests never touch the real keychain, data dir, `~/.ssh` or home;
- no secrets, names, hosts, strings, SQL or values in `Debug`, errors, logs or events;
- the full check list, with npm and cargo through `mise exec --`;
- one shared `CARGO_TARGET_DIR` (`/private/tmp/claude-501/-Users-m-projects-github-webstonehq-seaquel/6fe8e76e-3471-4592-8d83-40e0c17c607e/scratchpad/p5a/target`);
- a wasm-capable clang for anything that builds the module;
- effort log: `docs/plans/2026-10-08-phase-6-effort.md`.

Added for phase 6:
- **Never call a real model provider, and never use a real API key**, in tests, spikes, probes or checkpoints. Every turn goes to a local mock. `seaquel-ai`'s test support installs an `HttpClient` that panics on any host but `127.0.0.1`/`localhost`, and every Core, RPC, server and transport test that builds an AI-capable Core uses it. Manual checks with a real key are the owner's.
- **No key, prompt, message, schema, SQL, tool input or result in a log**, and no provider error message (Decision 16). The probe greps the desktop log, the server's stderr and the test captures for the test key and for a marker string planted in a prompt, a schema name and a cell.

### Constraints the executors must obey

- `seaquel-ai` builds for wasm32 with no tokio, threads, `Instant`, `SystemTime` or file system; time comes from the `Executor` (CI's wasm line).
- MCP's surface doesn't change (Decision 20).
- No new path runs SQL (Decision 4).
- No key reaches the page on desktop (Decision 7).
- A turn always ends with exactly one `done` or `error`, unless cancelled.

### Things a task could quietly skip

Reviews check each by name:
- a tool call in a round that runs but whose result isn't sent back, or results out of order;
- the assistant's queries without `max_bytes` or `timeout`;
- sharing read once per turn instead of before each tool call;
- `respond` reaching a turn of another workspace, or a second `respond` running a query twice;
- a cancel during an approval or a client tool that leaves the waiter registered;
- the HTTP response not dropped on cancel (the provider keeps generating);
- the egress check done on the resolver only, or redirects followed;
- the key in a `Debug`, an error's message or a URL;
- `text` events not coalesced, or coalesced so the last delta is lost at `done`;
- the reply not stored on Stop or on an error;
- `done.messages` applied without its `seq`, so another window's older write wins;
- `ai.chat` missing from one transport's stream list (desktop, web socket, module);
- `parts` written by an older release's `chatMessagesPut` read as anything but "none";
- (GUI) `getAIApiKeyForProvider` still called on desktop, or a provider `fetch` left in the page;
- (GUI) an event handler calling the demo's module synchronously.

---

## Order and estimates

Sized from logged first passes of the nearest tasks (effort logs; design doc cost sections). Review fixes are budgeted at about 55% of first passes: below 5d's and 5e's 64–72%, since the spikes settled the mechanics as phase 8's did (28%), and above phase 8 because this phase decides what SQL runs and where keys go, where the AI safety phase ran near 40% and its reviews found every way around the new mechanism. Probe fixes at 2–3.5 h (5d, 5e: 3.4–3.9 h; phase 8: 1.65 h). Builds are inside each row.

| # | Task | First pass | Nearest logged task (first pass) | Needs | Alongside |
|---|---|---|---|---|---|
| 1 | The recorder and the TypeScript baseline | 0.6–0.9 h | phase 8 T1 (0.65 h), 5e T2 (0.5 h) | — | 2 |
| 2 | `seaquel-http`; `seaquel-ai`'s wire, SSE, decoders, mock; the egress guard | 1.5–2.2 h | S1 is about a third of it; 5e T4a (0.5 h), phase 8 T4 (0.85 h) | — | 1 |
| 3 | `seaquel-ai`'s registry, renderers (MCP's `format.rs` moved), prompt, mentions, history, sharing | 1.2–1.8 h | 5e T4b (2.1 h, larger), 5b's planner | 1, 2 | — |
| 4 | Core: the loop, keys, tools, approvals and client tools, chat writes and `0007`, `generate`/`models`/`test`, `savedConnectionId` | 2.5–3.5 h | 5d-2 T4 (1.5 h), 5e T5 (4.5 h, ran over) | 3 | — |
| 5 | RPC `ai` group and `CoreEvent::Ai`; desktop, web socket and module stream lists; server env, limits, egress; the desktop secret refusal | 1.2–1.8 h | 5a T4 (1.25 h), 5c T6 | 4 | 6 |
| 6 | MCP onto the registry | 0.5–0.9 h | phase 4 T5 | 3, 4 | 5 |
| | **Checkpoint 6a** (in Task 10's row) | | | | |
| 7 | GUI: `AiService`, the assistant on events, approvals and dashboard tools via `respond`, the inline prompt, settings, the vault key on web, the CSP, TypeScript out | 2–3 h | 5d-1 T6 (1.9 h), 5e T7 (1.1 h) | 5 | 8 |
| 8 | The demo's assistant (Q2): the fetch bridge, the session key, the flag | 0.8–1.2 h | phase 8 T5's bridge (part of 0.85 h) | 5 | 7 |
| 9 | Probe | 0.8–1.2 h | 5e T8 (0.65 h), phase 8 T7 (1.6 h) | 7, 8 | — |
| 10 | Docs, measurement, both checkpoints | 0.8–1.2 h wall | phase 8 T8 (0.75 h + 0.25 h) | all | — |
| | **First passes** | **11.9–17.7 h** | phase 8: 8.5 h, 5e: 14.35 h | | |
| | Review fixes (~55% of Tasks 1–8) | 5.7–8.4 h | phase 8: 2.05 h, 5e: 10.1 h | | |
| | Probe fixes | 2–3.5 h | phase 8: 1.65 h, 5e: 3.5 h | | |
| | Owner answers (all in, each as recommended) | 0 h | 5e: 0.2 h | | |
| | **Total** | **~19.6–29.6 h** | | | |

**Expect about 24.5 h:** 6a about 14.5 h, 6b about 10 h. Re-checked after the answers (2026-10-08): every answer is the option the rows were sized for, so no row changes; the owner-answers row drops to 0. Moving the chat migration to `0007` costs nothing beyond one more frozen fixture case in Task 4 (a file the cleanup pass's `0006` already reached).

Phase 8 came in at 44% of its expectation because its spikes had settled every open question with running code; the AI safety phase came in under its range for the same reason. Here the spikes settled the mechanics (streaming, cancel, wasm, egress) and the answers settled the design, but 5e ran 50% over where a design question reopened in review. So about 15 h is possible if the reviews stay local, and past 30 h if Task 4's approvals or chat writes reopen.

The riskiest parts:
- **Task 4:** the loop's waiting states (approval, client tool, cancel, a reconnect, eviction on web) and writing the turn without losing the reply. Expect the review to find a waiter that outlives its turn and a reply lost on one ending.
- **Task 5's egress guard**, the first outbound request a web user controls.
- **Task 7:** the assistant's view model changes from owning the turn to following it.
- **The probe:** real browsers, a slow and misbehaving mock, a large schema, and the web server's egress.

Cut if time runs short:
- Q7's tool-call lines in the GUI (store `parts`, show them later);
- coalescing `text` events (send each delta; measure first);
- the desktop CSP change (Decision 19) becomes a follow-up.

The answers to Q1–Q9 are scope, not cuts.

---

## Task 1: The recorder and the TypeScript baseline

**Files:** `docs/plans/artifacts/2026-10-08-record-ai-baseline.test.ts.txt` (the recorder, run from a scratch copy), `crates/seaquel-ai/tests/fixtures/ts-baseline/{README.md,*.json,changes.json}`. The directory is only fixtures until Task 2 creates the crate.

**Tests first:** the recorder runs twice and gives byte-identical output; a case whose request contains the test key fails the recorder.

**Run:** the recorder under `mise exec -- npx vitest run` in a scratch copy; `npm run check` 0/0 in the tree (nothing in the tree changes but the fixtures).

**Review:**
- Each item of "Recorded first" has cases, both providers where it applies.
- Today's bugs are recorded as they are (two tool calls: the last runs; `error`: a silent end), each with its `changes.json` entry.
- No key, no real host other than the provider URLs as constants.

**Things this task could quietly skip:**
- the OpenAI request's system message and `tools` shape;
- `runAndFormat`'s truncation note's exact text;
- the dashboard-id injection on the last user message;
- headers (`anthropic-version`, `content-type`, the absent `authorization` for a keyless OpenAI-compatible server).

### Notes from Task 1 (as built)

- **Files.** `crates/seaquel-ai/tests/fixtures/ts-baseline/`: `turns.json` (73), `page.json` (21), `prompts.json` (28), `mentions.json` (19), `tool-results.json` (66), `errors.json` (8), `generate.json` (21), `models.json` (18) and `history.json` (3, Rust only), 257 cases; `inputs.json` (the shared `SCHEMA`, `SAVED`, `DASHBOARDS` and the 3,000-table generator); `changes.json` (225 entries: `*` and 224 cases); `README.md`. About 4.2 MB, mostly `changes.json`, `turns.json`, `page.json` and `tool-results.json` (the 21-round turns, and the 256 KB and 64 KB inputs). The recorder is `docs/plans/artifacts/2026-10-08-record-ai-baseline.test.ts.txt` (~3,830 lines), copied to `src/lib/services/ai/record-ai-baseline.test.ts` to run (`FREEZE_AI=1`, `FREEZE_AI_OUT`) and deleted after.
- **How it records.** `fetch` is stubbed: each request is recorded (URL, method, the headers the code set, lowercased, the key as `<key>`, the body as JSON) and forwarded to a `node:http` mock on `127.0.0.1` that writes the scripted SSE or JSON in 7-byte pieces; the provider URLs are only recorded. Everything else is the real code (`sendAIMessage`, `generateSQL`, `UIStateManager` over a real `DatabaseState`/`AIChatManager`, the inline prompt, `aiSettingsStore`'s model list, `collectReadOnly`). The keychain answers `test-key-not-real`; the recorder throws if that string is anywhere in its output (the log checks name it `$TEST_KEY`), and its first test checks the guard. Ids are a counter and no timestamps are recorded, so nothing is normalised.
- **Runs.** The first recording: three runs (two to scratch directories, one from the artifact), byte-identical. The review's re-recording: two runs, byte-identical; every first-recording case is unchanged byte for byte except `page/allow-all-other-connection` (its second step now scripts an `allow`, I2). The re-review's re-recording: two runs, byte-identical; every case is unchanged byte for byte except the 11 whose inputs changed (R2's distinctive messages, requests and replies in `page/{no-model,connection-removed,provider-500,dashboard-id-line}` and five `generate` cases, R4's seed in `page/history-skips-pending`, the reply marker in `turns/anthropic/markers`), plus two new cases (`page/no-api-key`, `tool-results/tool/assistant/explain-query-large`). The README says the same.
- **Bugs recorded as they are**, each with an entry: two calls in a round (Anthropic runs the last, OpenAI index 0); an `error` event, a cut stream and `max_tokens` end as done; a malformed event is skipped; 5 of 1,000 rows, `[object Object]`, `|` and newlines; unqualified table names and no cap; mentions with schema sharing off; "Allow all" across connections and read once per turn; provider bodies in the chat and the log; every tool refusal as plain text; the inline prompt runs the tab; `run_query` runs with data sharing off and after sharing is turned off mid-turn; the page stores `Error: <body>` as the reply.
- **`changes.json` is generated** by a model of the Rust side in the recorder: `simulateTurn` (one turn as Core runs it, from the scripted responses), `decodeScript` (Task 2's decoding rules), `rustToolResult` (the registry, sharing, the read-only check, approvals, MCP's renderers), `checkArgs` (serde's messages, Decision 24), `renderHistory` and `historyBudget` (Decisions 23 and 29). Expected requests are built from scratch, not edited from today's. An entry keeps only fields that differ from the recording, apart from `logs` and `storeCalls` on page cases and `cancel-before`'s `fetches: []`. The review added Decisions 22–32 to this plan and folded them in.
- **Concrete values Tasks 2–7 must match, or change with a reason in `changes.json`:**
  - the tool guidance paragraph (`GUIDE_SCHEMA`, `GUIDE_DATA`), after the schema context and before "Provide clear…"; none for `ai.generate`;
  - the schema cap note `(N more tables not shown; use list_tables and describe_table to see them.)`, the cap measured before the note (1,056 of 3,000 tables kept);
  - the assistant tool order: `run_query`, `explain_query`, `list_schemas`, `list_tables`, `describe_table`, `list_saved_queries`, `run_saved_query`, then the dashboard tools;
  - `run_query` results: MCP's JSON, keys sorted, notes worded in KB (`Only the first 87 rows are shown: the next would take the result past the 256 KB limit. …`), `truncatedCells` for cells over 64 KB;
  - every query `{maxRows, maxBytes: 8388608, timeoutMs: 60000}`;
  - error results `CODE: message`: `DENIED: User denied query execution`; `INVALID_ARGUMENT: Unknown tool: <name>`; serde's messages with a dotted path (``sql: invalid type: integer `42`, expected a string``, ``widget_type: unknown variant `map`, …``, ``kpi_config: missing field `label` ``); `max_rows must be between 1 and 1000, got 1001`; MCP's `DATA_SHARING_OFF`/`SCHEMA_SHARING_OFF`, `TABLE_NOT_FOUND`, `AMBIGUOUS_TABLE` messages;
  - Core-worded turn messages: `The provider sent an event that isn't valid JSON.`, `The provider sent a tool call whose input isn't a JSON object.`, `The provider's stream ended before the reply did.`, `Could not reach the provider.`, `The model asked for more than 20 tool calls in this turn.`, `No API key is set for this provider.`, `No AI provider is configured.`, `The chat's AI provider no longer exists.`, `No model is chosen for this connection.`, `The provider's answer isn't valid JSON.`; provider-derived ones are the provider's message cut at 1 KiB;
  - `parts` and history per Decision 23 (`round`, the 16 KiB cut and its `\n(cut at 16 KB of N bytes)` note), the budget per Decision 29;
  - OpenAI tool-call `arguments` as serde_json writes them (sorted keys);
  - Task 7: the inline prompt's "Inserted. Run it with ⌘↵."; the page words every code; the per-connection "Allow all".
- **Re-review fixes (R1–R4):** the page model keeps one chat per connection, so `page/allow-all-other-connection`'s second chat has only its own turn and rows; a forbidden-log string is never shorter than 8 characters, and every case also forbids its streamed reply; Core's own refusals write nothing (`page/no-api-key`); a seeded history holds only what Core would have stored (a page-stopped send and its pending row aren't). The recorder throws on a short forbidden string.
- **Aligned with Task 2 as built:** tool input that isn't a JSON object ends the turn with `PROVIDER_ERROR`; the OpenAI-compatible model list without a base URL asks `https://api.openai.com/v1/models`.
- **Rust only, no recording:** `history.json`'s budget cases, and the Rust results of the six tools today's TypeScript lacks (recorded as "Unknown tool", their inputs in `rustInputs`).
- **Not recorded:** timing (stalls, timeouts; bug 11), real providers (bug 1, CORS), the keychain and vault, the GUI's rendering. The `malformed-event` cases' log line holds Node's `SyntaxError` text, which varies by Node version; logs aren't compared line by line.

## Task 2: `seaquel-http`, the wire, the mock and the egress guard

**Files:**
- `crates/seaquel-http/` (new): `seaquel-license/src/http.rs` moved (the license crate re-uses it; its tests move too), `post_stream`, `NativeHttp` (implements `seaquel_ai::http::HttpClient`), `egress.rs` (Decision 9);
- `crates/seaquel-ai/` (new): `http.rs` (trait), `sse.rs`, `wire/{anthropic,openai}.rs`, `testing/` (the mock server and the loopback-only client, behind a `testing` feature);
- `scripts/check-crate-deps.mjs` (classify both), `ci.yml` (the wasm32 line gets `seaquel-ai`), the CI "Web server dependencies" step (still no OpenSSL).

**Tests first:**
- `sse`: every split position of S1's streams decodes the same events; `\r\n`, `\n`, comments, multi-line `data`, a final event without a blank line.
- `wire`: request bodies equal Task 1's records, both providers; two tool calls in a round decode in order; `error`, `max_tokens`, 429, 500, malformed event, early `[DONE]`.
- `egress`: S3's cases and "New checks"' list; a 302 is not followed; `Any` allows them.
- `license`: its suite unchanged.

**Run:** `cargo test -p seaquel-ai -p seaquel-http -p seaquel-license`; clippy native and `--target wasm32-unknown-unknown -p seaquel-ai`; `npm run crates:check`.

**Review:**
- `seaquel-ai` has no tokio, no reqwest, no clock.
- The license client's behaviour is unchanged (roots, fallback, proxies, timeouts).
- The guard checks the parsed host before DNS, filters DNS answers, follows no redirect.

**Things this task could quietly skip:** an IPv6 zone id or bracketed literal; a decimal or octal IPv4 host (`url` normalises; test it); a name resolving to several addresses where only some are private; the proxy case documented but untested.

### Notes from Task 2 (as built)

- **Crates.** `seaquel-ai` (`http.rs`, `sse.rs`, `wire/{mod,anthropic,openai}.rs`, `testing/{mod,mock,scripts}.rs`) and `seaquel-http` (`client.rs`, the license crate's `http.rs` moved; `egress.rs`; `native.rs`). Both are in `DOMAIN_AND_INFRA`; `check-crate-deps.mjs` also has a `WASM_DOMAIN` rule: `seaquel-ai` may depend only on pure crates and `seaquel-workspace`. CI has a "seaquel-ai builds for wasm32" step. `seaquel-license` depends on `seaquel-http` and calls `seaquel_http::client::{LazyClient, ClientOptions, load_extra_roots}`; its only code change is `..Default::default()` in `ControlClient::new`, and its log lines keep `activity = "license.http"`. The six `http.rs` unit tests moved with the file. The web server's tree gains `seaquel-ai`, `seaquel-http` and `url` (already there through reqwest) and still passes the banned-crates check.
- **The trait is `send`, not `post`.** `HttpRequest` has a `method` (`Get`/`Post`): the model list and the provider test are `GET …/models`. Header values are `Redacted<String>`; `HttpRequest`'s `Debug` shows the method, the host and the header names only. `HttpError { kind, detail }` maps to codes with `code()`: `InvalidUrl` → `INVALID_ARGUMENT`, `EgressBlocked` → `AI_EGRESS_BLOCKED`, `Timeout` → `TIMEOUT`, the rest `PROVIDER_ERROR`. `read_body(stream, max)` reads an error body or a non-streaming answer (the wire's cap is `MAX_ERROR_BODY_BYTES`, 64 KiB).
- **Wire, for Task 3/4.** `round_request(p, key, &Round, browser)`, `generate_request(p, key, system, &[(Role, String)], browser)`, `models_request(p, key, browser)`; `check_status(status, body)`, `decode_generate`, `decode_models`; `Decoder::{new, feed, finish}` yielding `RoundEvent::{Text, ToolCall, Usage, Stop}`. A `Round` is `{system, messages: Vec<Message>, tools: Vec<ToolSpec>}`; `Message` is `User(text)`, `Assistant {text, tool_calls}` (a plain string content when there are no calls, as stored history was) or `ToolResults(Vec<ToolResult>)` (one Anthropic user message of `tool_result` blocks; one OpenAI `tool` message each). **The wire marks tool errors itself**: `ToolResult.is_error` becomes `"is_error": true` on Anthropic and the `Error: ` prefix (`TOOL_ERROR_PREFIX`) on OpenAI-compatible, so Task 3's renderers must not add the prefix. Bodies are built as `serde_json::Value`, so keys go out sorted; compare Task 1's records as JSON values, not bytes.
- **Decoding rules.** Anthropic tool calls come out at their `content_block_stop` (any still open at `message_stop`, in index order); OpenAI's at `[DONE]` or at the body's end after a `finish_reason`, in index order, a missing id becoming `call_<index>`. Every `ToolCall` precedes the one `Stop`. Tool arguments are `{}` when none came, and anything but a JSON object is `Malformed` (the turn ends with `PROVIDER_ERROR`; the TypeScript silently used `{}`). An `error` event (Anthropic) or an `{"error": …}` chunk (OpenAI) is `WireError::Stream`; bad JSON, a tool delta for no tool block and a nameless call are `Malformed`; a body that ends with no stop reason is `Incomplete`. Lenient on purpose: Anthropic's `message_stop` may be missing once `message_delta` gave a stop reason; OpenAI's `[DONE]` without a `finish_reason` ends the round (`ToolUse` if calls came, else `End`) and anything after `[DONE]` is ignored. Stop mapping: `tool_use`/`tool_calls`/`function_call` → `ToolUse`; `max_tokens`/`length`/`model_context_window_exceeded` → `MaxTokens`; anything else → `End`. Task 4 should run the calls it got whatever the stop says.
- **Errors carry the provider's text only for the user.** `WireError::provider_message()` is the message cut at 1 KB on a char boundary (a non-JSON body's trimmed text too); `error_type()` keeps only a short identifier (`[A-Za-z0-9_.-]{1,64}`). `Debug` and `Display` print the status and the type, never the message. 429 is `RATE_LIMITED`; every other non-2xx, a 3xx included, `PROVIDER_ERROR`.
- **Differences Task 3 must list in `changes.json`** (beyond Decision 10's): a provider's `base_url` is honoured for Anthropic too (the TypeScript always used `api.anthropic.com`; Core should pass it only for OpenAI-compatible providers, and tests use it to reach the mock); an OpenAI-compatible model list with no base URL goes to `https://api.openai.com/v1/models` (the TypeScript returned no models); `decode_generate` takes Anthropic's first `text` block rather than `content[0]`; a key that is `None` or empty sends no `x-api-key`/`authorization`.
- **Egress.** `seaquel_http::Egress::{Off, Public, Any}` (`Egress::parse` reads `public`/`any`/`off`, ASCII case and spaces ignored, else `None` so the server can refuse a typo); Core's `AiEgress` maps onto it. `NativeHttp::new(NativeHttpOptions::new(egress))` has Decision 8's timeouts (connect 10 s, `read_timeout` 120 s idle, which also covers waiting for the head, 600 s per request) and never follows a redirect in any mode. `NativeHttpOptions` also takes `extra_ca_file` (Task 5: `NODE_EXTRA_CA_CERTS`), `proxy` and `resolver` (tests). `seaquel-http`'s `system-proxy` feature is for the desktop's Core.
- **Test support.** `seaquel-ai`'s `testing` feature (native only, pulls tokio): `MockProvider` (`reply(Reply::{Sse{events, piece, gap, end}, Raw, Hang})`, `SseEnd::{Finish, Stall, Close, Repeat}`, `requests()`, `connections()`, `client_gone(within)`), `scripts` (S1's streams for both providers), `LoopbackOnly<C>` (panics on any host but `127.0.0.1`/`localhost`/`::1`) and `TEST_KEY`. Core's tests should build `LoopbackOnly(NativeHttp::new(NativeHttpOptions::new(Egress::Any)))` against a `MockProvider` and give the Anthropic `Provider` the mock's URL as `base_url`.
- Task 1's fixtures didn't exist yet when this task finished; `tests/wire.rs` checks the bodies against `providers.ts` as read, and Task 3's replay compares them with the records.
- **Review fixes (security review of Task 2).**
  - *Decoder budget:* a round may have at most 64 tool calls or content blocks open (`MAX_OPEN_CALLS`; a closed Anthropic block frees its slot), 16 MiB of tool arguments (`MAX_TOOL_ARGUMENT_BYTES`) and 16 MiB of text (`MAX_ROUND_TEXT_BYTES`) in all; past any, `Malformed` (`PROVIDER_ERROR`). Task 4 inherits the text cap: a reply is at most 16 MiB per round.
  - *SSE:* an event's data is one `String` joined as it comes, so the 16 MiB cap counts the bytes held (8 Mi empty `data` lines held ~192 MiB before, ~8 MiB now; `SseParser::held_bytes`).
  - *Proxies under `Public`:* the configured proxies' hosts (the option, else `HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY` in either case) resolve unfiltered, so `http://localhost:3128` works; and with any proxy configured, the target name is resolved here first, best effort (`PublicResolver::check_name`): every local answer blocked is `AI_EGRESS_BLOCKED`, a name that doesn't resolve here goes to the proxy. The check runs whenever a proxy is configured, `NO_PROXY` notwithstanding (reqwest exposes no matcher; for an exempted URL it is a redundant lookup before rule 2). The desktop's macOS/Windows system proxy isn't seen, but the desktop runs `Any`.
  - *Ranges:* SIIT `::ffff:0:0/96` (embedded IPv4 checked), `2001::/23` whole (Teredo, benchmarking `2001:2::/48`, ORCHID v1/v2) and `3fff::/20`.
  - `host_of` ends the authority at a backslash too, as the URL parser does (`https://evil.example\@127.0.0.1/` is `evil.example`). `NativeHttpOptions`'s `Debug` shows only whether a proxy, CA file or resolver is set. `load_extra_roots(path, activity)` tags its log lines with the caller's activity (`ai.http`, `license.http`). `MAX_MODELS_BODY_BYTES` (8 MiB) is the `/models` read cap for Task 4 (an OpenRouter-sized list is ~1 MB). A seeded xorshift fuzz loop feeds random bytes, mangled and spliced streams in random pieces to both decoders (3,000 cases each), never panicking.

- **Re-review fixes.** A tool call's name and id are at most 256 bytes (`MAX_TOOL_NAME_BYTES`) and a round has at most 64 tool calls in all (`MAX_ROUND_CALLS`, counted at each call's start, so Anthropic's closed blocks don't reset it); past either, `Malformed`. The local name check through a proxy is bounded by the connect timeout, and a timeout counts as "doesn't resolve here" (the proxy decides). Under `Public` a target whose host is one of the proxies' (case and a trailing dot ignored) is refused (`Refusal::ProxyHost`, `AI_EGRESS_BLOCKED`), closing the fail-open check plus `NO_PROXY` path to the proxy's unfiltered name. `proxy_hosts_from(option, env)` is tested over every variable in both cases, a value without a scheme, IPv6 and the option winning over the environment. `sse.rs` documents the parser's bound: about 32 MiB (the `event` name and the data with the current line, 16 MiB each).

## Task 3: The registry, the prompt and the renderers

**Files:** `crates/seaquel-ai/src/{tools/,prompt/,sharing.rs,limits.rs}`; `crates/seaquel-mcp/src/format.rs` and `exposed.rs`'s sharing moved here (MCP re-points in Task 6); `tests/ts_baseline.rs`; `tests/fixtures/tool-schemas.json` (every tool's schema for each profile, frozen).

**Tests first:**
- `ts_baseline`: prompts, schema contexts, mentions and dashboard results equal the records except `changes.json`.
- Every tool's argument parser refuses unknown fields, wrong types and out-of-range `max_rows`.
- `definitions` offers no data tool without data sharing, no schema tool without schema sharing, no client tool without `clientTools`.
- Renderers: MCP's existing `format.rs` tests move unchanged; the assistant's budget cuts at 256 KB with the note.
- `schema_context` cuts at whole tables and names the remainder; mentions under each sharing combination.

**Run:** `cargo test -p seaquel-ai`; clippy native and wasm32.

**Review:** schemas match MCP's current ones byte for byte for `Profile::Mcp`; the assistant profile has no `connection`; mentions never return columns without schema sharing.

**Things this task could quietly skip:** `get_dashboard` without schema sharing; history's budget dropping a tool result but keeping its call (a dangling `tool_use` the provider refuses); the `[Context: …dashboard…]` line when the last message isn't the user's.

**From Task 1's review:** a test that pins the argument errors' order (two bad fields in one call, the sorted-first one reported), so enabling serde_json's `preserve_order` anywhere in the workspace fails it (Decision 24); the assistant profile's 256 KB cut for `explain_query` and the schema tools (Decision 30).

### Notes from Task 3 (as built)

- **Files.** `seaquel-ai/src/`: `tools/{mod.rs, args.rs, format.rs, render.rs, saved.rs}`, `prompt/{mod.rs, mentions.rs, history.rs}`, `sharing.rs`, `limits.rs`. New deps: `seaquel-types`, `seaquel-sql`, `schemars`, `serde_path_to_error`, `ryu-js` (all wasm-clean; the wasm32 clippy line passes); dev: `sha2`. Tests: `tests/ts_baseline.rs` (7), `tests/registry.rs` (18), `tests/prompt.rs` (9), `tests/fixtures/tool-schemas.json`; `seaquel-mcp/tests/tool_schemas.rs` (1).
- **Core's `ai` feature exists already**, as `ai = ["workspace", "storage", "dep:seaquel-ai"]` and `pub use seaquel_ai as ai;` in `lib.rs`, so the MCP server could reach the moved code (an interface may not name `seaquel-ai`). Task 4 turns the re-export into Core's own `pub mod ai` (re-exporting `seaquel_ai`'s modules beside the turn) and adds `ai-native`.
- **The MCP server moved, not re-pointed.** `seaquel-mcp/src/format.rs` is a one-line shim over `seaquel_core::ai::tools::format`; `exposed.rs` re-exports the sharing rule (`Sharing`, `sharing`, `global_sharing_from`, `DEFAULT_SHARING`, `AI_SETTINGS_KEY`) and keeps the async `global_sharing`; `tools/saved.rs` takes `definitions`, `describe` and `parameter_values` from `seaquel_ai::tools::saved` (`From<ai::tools::ToolError>` in `error.rs`); `ryu-js` left its `Cargo.toml`. Their unit tests moved with them. Still MCP's own, for Task 6 to replace with `render::*`: `tools/query.rs`'s `Rows` and plan cut and `max_rows`, `tools/schema.rs`'s describe and table lookup, `saved.rs`'s listing and lookup, and the argument structs in `tools/mod.rs`, which `seaquel_ai::tools::mcp` duplicates field for field (doc comments are the schema descriptions; both must use the same schemars, 1.2.2 today). `McpServer::tool_list()` is new: `tests/tool_schemas.rs` compares it with the fixture's `mcp` profile, and `seaquel-ai`'s `registry.rs` compares the registry's MCP profile with the same file, so the two are byte-identical.
- **The API, where it differs from the sketch above.**
  - `tools::prepare(name, input, &Gate { sharing, client_tools, connection_name })` is the assistant's whole pre-check, in Decision 22/24/30's order: known tool (a client tool only when offered), sharing (`DATA_SHARING_OFF`/`SCHEMA_SHARING_OFF`, MCP's messages, before the arguments are read), the strict parse, `max_rows`. Then `tools::read_only_check(&call, engine)` (`seaquel_sql::read_only_error`, its message; `run_query`'s and `explain_query`'s `sql` and a widget's non-blank `query`; a saved query's SQL is checked when Core runs it). `parse(profile, …)` is the parse alone; MCP's is rmcp's (unknown fields ignored, serde's message without a path).
  - `Call { tool, connection, args: Args }`; `Args::Client { input, query }` carries the model's input as sent, for `clientTool`. `Call::max_rows()` is the checked value (100 by default). `Tool::asks_approval()` is the three data tools. `ToolError`'s `Debug` shows the code only; `ToolOutput { text, is_error }` is what the wire gets (no `Error: ` prefix anywhere in `seaquel-ai` but the wire's).
  - Core's order for an assistant call (what `ts_baseline.rs`'s stand-in does): `prepare` → `read_only_check` → for `run_saved_query`, `render::find_saved_query` (the chat's project), `saved::parameter_values`, `substitute`, then `tools::read_only_sql` on the substituted SQL, so a saved query that writes is refused before the approval card ever shows it → approval for `asks_approval` tools (`run_saved_query` shows the saved SQL) → run with `limits::QuerySpec::new(call.max_rows())` (8 MiB, 60 s) → `render::Rows::new(Profile::Assistant, max_rows)` (`set_columns`, `mark_truncated` on a truncated batch, `push(&[Value])` until `false`, `into_json`); `render::explain`, `schemas`, `tables`, `find_table` + `describe` (columns and indexes from `table_metadata`), `saved_queries(profile, entries, hidden)` with `saved::describe(q, project, &[chat's connection name])`. A client tool's answer goes through `render::client_result(tool, answer, sharing.schema)`: Decision 26's strip, `isError` when the answer has `error`, every result cut at 256 KB on a character boundary with `\n(cut at 256 KB of N bytes)`; a `get_dashboard` answer that doesn't parse (or nests past serde_json's 128 levels) without schema sharing is `INVALID_ARGUMENT: The page's answer to get_dashboard isn't valid JSON.`, never the raw text; other tools' unparsed answers pass as (cut) text.
  - `prompt::system(engine, schema: Option<&SchemaContext>, sharing, tools: bool, dashboards)`: the context goes in only when `sharing.schema` (gated inside, review item 8); `tools: false` for `ai.generate` (no guidance), `true` for a turn, with the sharing the turn started with. `prompt::schema_context(tables, limits::SCHEMA_CONTEXT_BYTES)` returns `{text, kept, left}`.
  - `prompt::mentions(content, share_schema, tables, &[MentionQuery], &[MentionDashboard])`: Core maps saved queries and dashboards (widgets' `widgetType` and `query`) into those; data sharing plays no part.
  - `prompt::history::history(&[HistoryRow], limits::HISTORY_BYTES)` → `History { kept, bytes, messages }`: Task 4 maps stored rows (with Task 4's `parts` column) to `HistoryRow { id, role, content, parts: Option<Vec<Part>>, dashboard_id }`. `Part` serializes as Decision 23's JSON (`{round, type: "text", text}`, `{round, type: "tool", callId, name, input, ok, result, resultBytes?}`); `Part::tool(round, call_id, name, input, &ToolOutput)` cuts at 16 KiB. `last_dashboard_id(rows)` (from all stored rows, as the page did) and `with_dashboard_context(&mut messages, id)` put the line on the last `Message::User`, never on a round's `ToolResults`, so it stays on the typed message in round 2 and later; calling it again with the same id adds nothing. A reply stored empty (no text, no calls: Stop or an error before anything streamed) is left out of history, since Anthropic answers 400 to an assistant message with empty content, and the user messages around it are joined with `\n\n` (the budget already counted both rows, and the model sees what was asked). Task 4 must append the turn's own message with `history::push_user`, which joins it the same way to a history that ends with a question.
- **Schemas.** MCP's are rmcp's (`schema_for_input`: draft 2020-12, root `title`/`description` dropped). The assistant's are generated the same way with subschemas inlined (no `$ref`), `$schema` dropped and `additionalProperties: false` everywhere (`deny_unknown_fields`); optional fields read `["T", "null"]` with `default: null`, enums keep their values. The dashboard tools keep the TypeScript descriptions; `run_query` etc. have new ones naming "the connected database". `tool-schemas.json` is `{"mcp": [tools/list entries], "assistant": [{name, description, input_schema}]}` in `Tool::ASSISTANT` order, which is what `changes.json`'s `$toolSchemas` expands from (Task 4: take the named entries and put them in the provider's shape, or use `definitions(...)[i].spec()`).
- **Argument errors.** serde's message after `serde_path_to_error`'s path; a missing or unknown field of the root has no prefix (serde_path_to_error puts an unknown field's own key on the path, which is dropped, so a nested one reads `text_config: unknown field \`font\`, …`). A sequence index prints as `[1]` (`chart_config.yAxis[1]: …`) where the recorder's model joined it with `.`; no recorded case reaches it, and Decision 24 says serde_path_to_error's print. `argument_errors_name_the_sorted_first_field` parses `{"y", "x", "widget_type", "height", …}` from text and expects `height`, so `preserve_order` fails it.
- **Budgets.** `Rows` and the plan cut count as MCP did (columns plus each row and a comma; the notes and keys come on top, as MCP's 4 MB always did); the fixture's 87-row case pins the count. So an assistant query result can pass 256 KiB by its keys and note (a few hundred bytes), and `explain_query`'s plan is cut at 262,144 bytes of text before JSON escaping, so a plan full of quotes, backslashes or control characters serializes past it (`explain-query-large` pins both as they are). Once a row doesn't fit, `Rows::push` refuses every later one, so the rows shown are always a prefix. The assistant's listings (`tables`, `describe`, `saved_queries`) keep 512 bytes for their note, so the whole text stays within 256 KiB; MCP's listings stay whole. Messages: `Only the first N of M tables are shown: … past the 256 KB limit. Pass \`schema\` to list one schema's tables.`, `Only the first N of the table's M columns, indexes and foreign keys are shown: …`, `Only the first N of M saved queries are shown: …`. The 8 MB fetch note keeps "8 MB" in both profiles (it is MCP's text and 8 MiB).
- **Replayed here:** `prompts.json` 28/28, `mentions.json` 19/19, `tool-results.json` 66/66 (the two cancel cases as `$absent`: the stand-in sends no result; the page-side `calls` of a forwarded client tool are the recording's), `history.json` 3/3. **Left to Task 4:** `turns.json` (73), `page.json` (21), `generate.json` (21), `models.json` (18); `errors.json` (8) is Task 7's wording. `the_files_left_to_task_4_are_not_replayed_here` pins those counts. Two harness choices: `tool/dashboard/not-available` and `tool/dashboard/create-failed` both record `variant: ["onCreateDashboard"]`, so the stand-in turns client tools off by the case's name; `tool/dashboard/unknown-direct` has no Rust counterpart, so it checks `$absent` and that `prepare` refuses `pin_widget`.
- No `changes.json` entry needed changing.

## Task 4: Core runs a turn

**Files:** `crates/seaquel-core/src/ai.rs`, `ai/{turn.rs,tools.rs,keys.rs,chats.rs,waiters.rs}`, `lib.rs` (features `ai`, `ai-native`, the builder policies, the `compile_error!` list), `workspace.rs` (`savedConnectionId` on connect); `crates/seaquel-storage/migrations/0007_ai_message_parts.sql` and the `ai_chats` queries (`parts`; `append_turn_in` on `WriteTx`); `crates/seaquel-workspace/src/state.rs` (`parts` on the persisted message and draft).

**Tests first** (Core with the mock, SQLite and DuckDB in memory):
- a turn with no tools; with one and two tool calls in a round; across three rounds; past 20 calls (`TOOL_LIMIT`);
- sharing changed between two tool calls (the second refused);
- approval: allow, deny, allow-all mid-turn, a respond for another workspace (`NOT_FOUND`), a double respond, a cancel while waiting (no waiter left);
- a client tool answered, answered with an error, and never answered then cancelled;
- the reply stored at `done`, on `error`, on cancel with what streamed; the user message stored before the first round; `CHAT_FULL` before any request; `done.seq` and one `StorageChanged` per write;
- a stalled provider ends with `TIMEOUT` (executor time); cancel drops the response (the mock sees the close);
- desktop keys from a `MemoryStore`; a supplied key wins; no key: `NO_API_KEY` before any request; the test key in no captured log;
- `CONNECTION_MISMATCH`; a reconnect mid-turn;
- `generate` extracts the fenced block as the record says; `models` and `test`;
- `0007` on every frozen release fixture and on a file the cleanup pass's `0006` already reached, and an older-release write (no `parts`) read back.

**Run:** `cargo test -p seaquel-core --features ai,ai-native`; `cargo test -p seaquel-storage`; clippy native and the `browser` line with `ai`.

**Review:** every ending of a turn writes or deliberately doesn't; waiters are removed on every path; the loop holds no storage write lock across a model round; time only from the executor.

**Things this task could quietly skip:** eviction (`close_all`) during a turn on web; a write failing at the end of a turn (the `error` still carries what streamed); `stop: maxTokens` stored and shown; usage numbers logged but not tokens of text.

### Notes for Task 4 (from Task 2's re-review)

- The decoder refuses a round past 64 tool calls (`MAX_ROUND_CALLS`), above the turn's cap of 20. Core must count calls as the `ToolCall` events stream and stop the turn with `TOOL_LIMIT` at the 21st, not after the round ends, so the 21st call never runs and the decoder's own limit is never what a user sees.

### Notes from Task 4 (as built)

- **Files.** Core: `src/ai.rs` (re-exports, `AiEgress`, the codes, provider/key/model resolution, sending on the executor's clock, error wording, `ai_generate`/`ai_models`/`ai_test`, `extract_sql`), `src/ai/{turn,tools,keys,chats,waiters}.rs`; `lib.rs` (features, the three builder policies, the `browser` + `ai-native` `compile_error!`, `Connection::saved_connection_id`, a turn's stream entry with no connection); `workspace.rs` (`ConnectRequest::saved_connection_id`/`with_saved_connection_id`, `Workspace::ai`). Storage: `migrations/0007_ai_message_parts.sql`, `ai_chats` (`parts` read and written by every message write; `append_turn_in(tx, chat, messages, touched_at)`), the README entry, `tests/fixtures/wasm-made/meta.db` regenerated. `seaquel-types`: `PersistedAIMessage::parts` (`Option<serde_json::Value>`, a JSON list). `seaquel-workspace`: `ChatMessageDraft::parts` with `check_parts` (a list, within `max_message_bytes` as JSON), `AiSettings::enabled()`/`provider(id)` (`ProviderInfo`), and **`src/ai.rs` (new, beyond the file list)**: the wire types `ChatParams`, `ChatUserMessage`, `Approval`, `AiDecision` (`ApprovalDecision` or `ClientResult`, untagged), `AiEvent`, `AiStop`, `GenerateParams`, all with `ts` derives and hand-written `Debug`, beside `run.rs`'s `RunEvent` because Core has no serde of its own. `seaquel-ai`: `tools::saved::is_js_space` is `pub` (the inline prompt's trim). `npm run types:gen` writes the nine new types and `parts` on `PersistedAIMessage` and `ChatMessageDraft` (`npm run check` 0/0). CI: the workspace clippy and test lines add `seaquel-core/ai-native` (drop it once Task 5's interfaces enable it), and the wasm32 browser step gains a line with `seaquel-core/ai`.
- **The API.** `Workspace::ai_chat(core, ChatParams, WriteOrigin) -> BoxStream<AiEvent>`, `ai_respond(stream_id, call_id, AiDecision)`, `ai_generate(core, GenerateParams) -> String`, `ai_models(core, provider_id, api_key) -> Vec<String>`, `ai_test(...)`, `ai_turn_count()`, `ai_waiter_count()` (hidden); `seaquel_core::ai::tools::call(core, ws, &ToolContext, &Call) -> Result<Json, ToolError>` for either profile (Task 6), with `ToolContext {profile, connection_id, connection_name, project_id, project_name}`; `ai::native::{NativeHttp, NativeHttpOptions, Egress}` behind `ai-native`, with `Egress::from(AiEgress)`. Codes as consts in `seaquel_core::ai` (`NO_PROVIDER`, …, `TOOL_LIMIT`, `CHAT_FULL`, `NOT_FOUND`, `TOO_MANY_REQUESTS`). `seaquel_core::ai` is now Core's module: `http`, `limits`, `prompt`, `sharing`, `sse`, `wire` are `seaquel-ai`'s, and `ai::tools` re-exports the registry whole (MCP's paths are unchanged).
- **Order** (`turn.rs::resolve`, `ai.rs::resolve`): no executor, client or egress policy `NOT_SUPPORTED`; `Off` `AI_EGRESS_BLOCKED`; the turns-in-flight cap `TOO_MANY_REQUESTS` (a second turn under a running stream id is `INVALID_ARGUMENT`); `max_message_bytes`, then both message ids `INVALID_ARGUMENT`; `CHAT_NOT_FOUND`; the chat's saved connection (`CONNECTION_NOT_FOUND`); `NO_PROVIDER` ("No AI provider is configured." when none is and the row names none; "The chat's AI provider no longer exists." when the row names one that's gone, also with none configured, as `turns/none/no-provider` records); a row naming none while providers exist is `NO_MODEL`; `NO_API_KEY` (Anthropic only; an OpenAI-compatible provider may be keyless; a keychain read the store refuses is `SECRET_UNREADABLE`); `NO_MODEL`; an `http:` base URL under `Public` is `AI_EGRESS_BLOCKED`; `CHAT_FULL`; `AI_DISABLED`; then the open connection (`CONNECTION_NOT_FOUND`) and `CONNECTION_MISMATCH`. Each is one `error` event with no `messages`, nothing stored, no request. `CHAT_FULL` means the chat can't take the user's message plus a reply of `max_message_bytes` (or two more messages), checked by a read and again inside the user's write. The keychain is read before `AI_DISABLED` because that is the order asked for; a disabled assistant on desktop can therefore show a keychain prompt.
- **Writes.** Two per turn, each `chats::write`: `check_messages` on the row, a `WriteTx`, `append_turn_in`, commit, one `chatMessages` event (scope the chat, ids the row, the caller's origin). The reply's write also sets the chat's `updated_at` but emits only that one `chatMessages` event, not a `chat` one (Task 7: refresh the chat list on it). `done {messages: [user, reply], seq: the reply's}`; an `error` after the user's write carries both rows and the reply's `seq`, or, when the reply's write is what failed (past `max_message_bytes` on web, say), the reply as it streamed (not stored) and the user's `seq`. A reply's `parts` is stored only when a call got a result; text items with no text are left out; `dashboardId` is the `dashboard_id` a successful `create_dashboard` answer carried; `query` is never set.
- **Cancel.** A turn registers its stream id in Core's stream map with no connection (so `disconnect` and a reconnect fail the next tool call, `CONNECTION_NOT_FOUND`, and don't end the turn). `Workspace::cancel` and `close_all` cancel its token: the provider's response, a query in flight and every waiter are dropped, the reply is stored with what streamed, and no event follows. **Task 5: a transport must stop a turn with `cancel` and keep polling the stream to its end; dropping the stream drops the turn where it is, without the reply's write** (no `'static` spawn is possible: the turn borrows the workspace). The waiters live in `Workspace::ai` (one entry per running turn, removed by the turn's guard on every ending); `respond` is `NOT_FOUND` for another workspace's turn, an unknown call or a finished turn, ignores a second answer to a call, and refuses an answer of the wrong kind (`INVALID_ARGUMENT`, still waiting).
- **Time.** All from the executor: 120 s with no byte from the provider (`IDLE_TIMEOUT`, also for the head), 600 s per round or non-streaming call (`ROUND_TIMEOUT`), 60 s per tool call (MCP's "The call took longer than 60 s and was cancelled"), text coalesced to one event per 50 ms or 4 KiB with a timer while the provider is quiet, and flushed before any other event. `TIMEOUT` is "The provider didn't answer in time."
- **Wording.** `HttpError` `Connect`/`Other` "Could not reach the provider.", `Body` and `WireError::Incomplete` "The provider's stream ended before the reply did.", the decoder's `Malformed` reasons to Task 1's sentences (an answer that isn't JSON: "The provider's answer isn't valid JSON."; a list without `data`: "The provider's answer isn't a model list."), a status or stream error to the provider's message (cut at 1 KiB) or "The provider answered HTTP n." / "The provider sent an error.". Logs carry the kind, status, error type and code only.
- **Tests.** `seaquel-core/tests/ai_turn.rs` (23) and `ai_replay.rs` (5) share `tests/ai_support/` (a scripted engine for every engine id that records each read-only query, a `Redirect` client that records the request Core built, real URL and all, then sends it to the mock through `LoopbackOnly(NativeHttp::new(NativeHttpOptions::new(Egress::Any)))`, and a workspace seeded with plain SQL). Anthropic requests reach the mock this way, since Core ignores a stored base URL for Anthropic (Task 2's note). `*` rule 1 is applied by decoding a scripted round with Task 2's decoder and sending it again with the `run_query` argument renamed (a round that doesn't decode goes as recorded). The replays compare requests (`$toolSchemas` expanded from `tool-schemas.json`), queries, approvals, outcome and text, stored rows and `storeCalls` (the `chatMessages` events), errors, `inserted`/`code`/`message`, models/test results and unused responses, and each case's `$forbidden` against every log line the test binary made. Not compared: the page's view (`messages`, `allowAllAfter`, `dashboardCalls`) and the inline prompt's `executed`, `notice`, `error` and `toasts` (Task 7), `chunks` (`*` rule 2). The page harness answers a client tool with the result the next expected request carries for that call, keeps "Allow all" per connection, and seeds only what a completed send stored. `seaquel-storage/tests/ai_parts.rs` (4): `0007` on every frozen release file, a file at `0006` refused read-only (`migration 7`) and upgraded by a writable open, `parts` round trips and unreadable values, `append_turn_in`. `tests/common`'s `LINK_COLUMNS` gains `ai_messages.parts`, so the 5d replays check it stays NULL. The CLI's `a_data_dir_the_app_must_upgrade_is_refused` and MCP's suite pass unchanged.
- **For Task 5.** `CoreEvent::Ai` wraps `AiEvent` as is (`seaquel_workspace::ai`, serde camelCase, `type`-tagged); `ChatParams` and `GenerateParams` refuse unknown fields; `db.connect`/`test`'s `savedConnectionId` goes to `ConnectRequest::with_saved_connection_id` (a saved target records its own id when none is given). Status codes to add: `NOT_FOUND` 404, `CHAT_FULL` 409, `CONNECTION_MISMATCH` 400, `TOO_MANY_REQUESTS`/`RATE_LIMITED` 429, `AI_EGRESS_BLOCKED` 503, `PROVIDER_ERROR` 502, `NO_PROVIDER`/`NO_MODEL`/`NO_API_KEY`/`AI_DISABLED` 400. The interfaces enable `ai-native` and build `NativeHttp` with `Egress::from(egress)` for the same `AiEgress` they pass Core.
- **For Task 7.** A failed turn's `error` has the stored rows only (apply them by `seq` like `done`; keep the streamed text when the reply isn't among them); `PersistedAIMessage.parts` is in the generated types as `Array<unknown>`; a `chatMessagesPut` of a message without `parts` keeps them (review I3); the reply's write also emits a `chat` event (refresh the chat list on it).
- **Review fixes (Task 4 review; they supersede the bullets above where they differ).**
  - *I1, stream ids.* `Core::register_stream` checks and registers under one lock: a key a turn holds (no connection) refuses any other registration, and a turn refuses any taken key, with `INVALID_ARGUMENT` "This stream id is already in use." and the map untouched (a query stream reusing a query stream's id still replaces it, as before). `db.queryStream`, `run`, `page` and `tablePage` answer that as their one error event. A turn also stops on `Workspace::closing` (`ai::Stop` = its token or `close_all`'s), so an eviction reaches it whatever its stream entry.
  - *M4.* One turn per chat at a time: a second is `TURN_IN_PROGRESS` (`seaquel_core::ai::TURN_IN_PROGRESS`), before anything is read or stored. *M5.* A user message and reply under one id are `INVALID_ARGUMENT`.
  - *Flag 2, order.* `NO_PROVIDER`, then `AI_DISABLED` and the `http:`-under-`Public` check, then the keychain (`NO_API_KEY`), `NO_MODEL`, `CHAT_FULL`, `CONNECTION_MISMATCH`; `ai.generate` the same after its size check. A turned-off assistant reads no key (a counting store pins it).
  - *I3, parts and the budget.* `CONTENT_BYTES` and `content_bytes_of` count `octet_length(parts)` with the content, so `max_chat_bytes`, `storedBytes` and `full` include tool calls; `parts_bytes_of` is new. A `chatMessagesPut` without `parts` keeps the stored ones (`COALESCE(excluded.parts, ai_messages.parts)`, as an older release's put does) and the budget counts what it keeps; one with `parts` replaces them, checked (a list, within `max_message_bytes`) and counted. The reply's write checks `max_chat_bytes` too (`CHAT_FULL`).
  - *I4, the reply's write.* Refused for size (`CHAT_FULL`, or `INVALID_ARGUMENT` past `max_message_bytes`) with parts, the reply is written once more without them, keeping its text; stored, the turn ends as it would have. An ending's `messages` holds only stored rows: when the reply can't be stored at all, `error` carries the user's row and its `seq`, and the page keeps the text it has from the `text` events.
  - *Flag 3.* The reply's write announces `chatMessages` (its `seq` is `done.seq`) and then a `chat` event (scope the saved connection, ids the chat), taken after the commit.
  - *M1–M3.* Text the coalescer holds when the turn is cancelled is dropped: nothing follows a cancel. `race` and the tool run check the stop first. A waiter lost without an answer fails the turn (`INTERNAL`, "The turn stopped waiting for an answer.") instead of ending it silently.
  - *M6.* `Workspace::connect` refuses a `savedConnectionId` that isn't a saved target's own id (`INVALID_ARGUMENT`); a form connect's stays trusted.
  - *M8.* `resolve` lost its unused flag; `ai.generate` applies `max_message_bytes` to `request` and `existingQuery`; keys travel as `seaquel_workspace::ai::SuppliedSecret` (`ChatParams.apiKey`, `GenerateParams.apiKey`, `ai_models`/`ai_test`), redacted in `Debug`, never serialized, `string` in TypeScript.
  - *Flag 5.* The provider seeding of `turns/none/no-provider` and `generate/none/no-providers` is in the baseline README ("The chat's provider when none is configured"); `changes.json` is unchanged.
- **Re-review cleanups.** The `ChatMessageDraft::parts` doc and the `0007` README entry say a put without `parts` keeps them. A turn asked for after `close_all` answers `WORKSPACE_CLOSED` (one `error`, nothing stored) instead of ending silently. New tests pin the reply's retry after the reply write's `max_chat_bytes` check (`CHAT_FULL`) and a chat taking a new turn after one ended by `done`, by cancel and by `error` (the `TURN_IN_PROGRESS` release).
- **Contract for Task 5 (review I2): a transport ends an `ai.chat` by cancelling it, never by aborting it.** On the web socket's close, a desktop webview reload or window close, and the demo module's teardown, the transport calls `Workspace::cancel(stream_id)` and keeps polling the turn's stream until it ends, so the reply is stored with what streamed; it never drops or aborts the stream's task while it runs. `seaquel-server/src/routes/rpc_stream.rs`'s close path (`running.task.abort()`, line 361 at Task 4) aborts today and must change for turns. One test per transport (web socket, desktop `core_stream`, the module) closes mid-turn and finds the reply stored. Add `TURN_IN_PROGRESS` 409 to `status_for`, with the codes listed above.
- **For Task 6.** `ai::tools::call` doesn't run `read_only_check`: the MCP path must run it itself (or keep Core's `READ_ONLY` refusal from `query_stream`) so MCP's wording stays as it is.

## Task 5: The RPC group and the transports

**Files:** `crates/seaquel-rpc/src/ai.rs` (new), `db.rs` (`CoreEvent::Ai`, `Request::stream_kind`), `workspace.rs`; `src-tauri/src/lib.rs` (`core_stream`, `ai-native`, the secret refusal); `crates/seaquel-server/src/{lib.rs,routes/rpc.rs,routes/rpc_stream.rs,error.rs,startup.rs}` (egress from env, `AiLimits`, status codes, the turns-in-flight cap); `shared/rust-env.js` (`SEAQUEL_AI_EGRESS`); `crates/seaquel-browser/src/module.rs` (the bridge in `open`); `npm run types:gen` output.

**Tests first:**
- RPC: `ai.chat` refused by `dispatch_workspace`; served by `dispatch_stream`; `respond`, `generate`, `models`, `test` round trips; unknown fields refused.
- Desktop: `core_stream` serves `ai.chat` and counts its events; `secret.get` of `ai-api-key:x` refused, of `db:x` allowed.
- Web: the socket accepts `ai.chat`, refuses a bad frame with an `ai` error, cancels on close; a fifth turn is 429; `SEAQUEL_AI_EGRESS=off` gives `AI_EGRESS_BLOCKED`; status codes per Decision 15; the env allow-list test finds the new variable.
- Module: a turn through `stream` with a JS bridge over the Node mock.

**Run:** `cargo test -p seaquel-rpc -p seaquel-server -p seaquel`; the module's Node harness; `npm run types:gen`; `npm run check`.

**Review:** every transport's stream list includes `ai.chat` through the one function; the web socket's 16-stream cap counts a turn as one; the key isn't in the request log line.

**Things this task could quietly skip:** a `respond` for a turn whose socket already closed (`NOT_FOUND`, not a hang); the desktop webview reload cancelling its turn; a turn's events after the socket's event queue lags (`EVENTS_LAGGED` ends it with `CANCELLED`).

### Notes for Task 5 (from Task 2's review)

- The operator doc for `SEAQUEL_AI_EGRESS` must state the proxy rule: behind a proxy (`HTTPS_PROXY`/`HTTP_PROXY`/`ALL_PROXY`), `public` still refuses private IP literals and refuses a name whose answers from the server's own DNS are all private or local; a name the server can't resolve goes to the proxy, which then decides what it may reach. The proxy's own host is trusted as configured.

### Notes from Task 5 (as built)

- **Files.** `seaquel-rpc`: `src/ai.rs` (new: `AiRequest` `chat`/`respond`/`generate`/`models`/`test`, `ProviderParams`, `AiResponse`, `Generated`, `TURN_STOP_WAIT`, the dispatch), `db.rs` (`StreamKind`, `CoreEvent::Ai`, `CoreEvent::error(stream_id, StreamKind, …)`, `ConnectParams.savedConnectionId`), `workspace.rs` (`Request::Ai`, `Request::stream_kind()`/`stream_id()`, the `ai-api-key:*` refusal), the `ai` feature (`seaquel-core/ai`), `tests/ai.rs` (9). `src-tauri`: `desktop_core` passes `NativeHttp` (`Egress::Any`) and `AiEgress::Any`, `core_stream` serves `ai.chat` through `stream_kind`, `stop_turn`, features `ai-native` and `ai-system-proxy` (new Core feature: `seaquel-http/system-proxy`; an interface can't name seaquel-http), the `ai_tests` module (4). `seaquel-server`: `WEB_AI_LIMITS` (4 turns, 1 MiB), `web_core(egress, extra_ca_file)` and `web_core_from_env()` (`main` exits on a bad `SEAQUEL_AI_EGRESS`), `startup::{AI_EGRESS_ENV, ai_egress_from, EXTRA_CA_CERTS_ENV}`, `status_for` (exported) with every phase 6 code, `rpc_stream.rs` on `StreamKind`, `tests/rpc_ai.rs` (9), `tests/rpc_ai_logs.rs` (1), `tests/common` (`Env::with_ai`, `ai_core`). `seaquel-browser`: `src/fetch.rs` (new: `FetchBridge`, `BridgeHttp`), `open(bridge, image?, onTrap?, fetch?)`, features `seaquel-core/ai` and `seaquel-rpc/ai`. Core: `From<Egress> for AiEgress`, `FROM_BROWSER` (`cfg!(feature = "browser")`) passed as the wire's `browser` flag in all three request builders, `ai-system-proxy`. TypeScript: `transport.ts` (`FetchBridge` type, `open`'s fourth parameter, `BROWSER_MODULE_EXPORTS.open: 4`), `testing/node.ts` (`seaquel-ai` in the stamp's sources), `src/lib/core/browser/ai-turn.test.ts` (3), `shared/rust-env.{js,test.ts}`, 13 generated types (`AiRequest`, `AiResponse`, `ProviderParams`, `Generated`, and Task 4's wire types now reachable from `CoreRequest`/`CoreEvent`; `ConnectParams`, `CoreEvent`, `CoreRequest`, `CoreResponse`, `SecretRequest` changed). Docs: README's configuration table and proxy paragraph, `deploy/docker/.env.example`, `main.rs`'s environment list. CI: the workspace clippy and test lines lost `seaquel-core/ai-native` (seaquel-server's normal dependency unifies it), and the wasm32 `ai` line checks `seaquel-rpc/ai` too.
- **One stream list.** `StreamKind::of(group, method)` is the list (`db.queryStream` → `Stream`, `run`/`page`/`tablePage` → `Run`, `ai.chat` → `Ai`); `Request::stream_kind()` and the web's lenient `named_kind` (a frame that doesn't parse) both use it, so a refused `ai.chat` frame is an `ai` error event. `DbRequest::is_run`/`is_run_method` stay for their tests and `StreamKind::of`.
- **Stopping a turn (Task 4's contract).** Web: closing (or losing, or lagging out) the socket cancels every stream through Core and aborts every task except a turn's; `stop` lets the turn's task run on, aborting it only after `TURN_STOP_WAIT` (45 s since the review, M1). In `run_stream` a turn whose outbox is gone keeps polling (events dropped) and cancels itself through Core if nobody had. Desktop: a reload or window close already cancelled through Core while `run_core_stream` kept polling; a channel that refuses an event now cancels a turn and drains it (`stop_turn`, bounded by the same wait) instead of breaking. Module: `stream` always polls to the end. Each pinned with the reply stored: web close, web lag (`EVENTS_LAGGED`), desktop reload, desktop gone channel, module `db.cancel` (which also aborts the bridge's request). Temporarily aborting turns on web fails three tests.
- **`respond`** goes over the unary transports (`core_call`, `/rpc`, `call`). After the socket closed, or after a reload, it answers `NOT_FOUND` at once (web 404); another user's is `NOT_FOUND` too.
- **Limits.** The 16-stream cap counts a turn as one (pinned with 15 hanging queries); a fifth turn in flight is `TOO_MANY_REQUESTS` (`WEB_AI_LIMITS`, Core's check). `max_message_bytes` is 1 MiB, the chat budget's message cap.
- **Egress.** `SEAQUEL_AI_EGRESS` unset or empty is `public`; `Egress::parse` reads the rest; anything else stops the server (`"SEAQUEL_AI_EGRESS must be public, any or off"`, the value not echoed). The model client trusts `NODE_EXTRA_CA_CERTS`. `off` gives `AI_EGRESS_BLOCKED` (503) on `/rpc` and as the turn's `ai` error. The vitest that scans for env reads now covers `crates/seaquel-http/src` too and requires `SEAQUEL_AI_EGRESS`.
- **Status codes.** 400: `NO_PROVIDER`, `NO_MODEL`, `NO_API_KEY`, `AI_DISABLED`, `CONNECTION_MISMATCH`, `TOOL_LIMIT` (a turn's ending, never an HTTP answer; mapped for completeness). 404: `NOT_FOUND`. 409: `CHAT_FULL`, `TURN_IN_PROGRESS`. 429: `RATE_LIMITED`, `TOO_MANY_REQUESTS`. 502: `PROVIDER_ERROR`. 503: `AI_EGRESS_BLOCKED`. 504: `TIMEOUT` (as before).
- **Keys.** The `secret` group refuses `get`, `set` and `delete` of `ai-api-key:*` with `INVALID_ARGUMENT` in `seaquel-rpc` itself, so every transport refuses it (the web has no store anyway). `db:*` and the rest are unchanged. The server's log test captures what stderr writes plus every Seaquel line at any level, over a turn with a query, a provider refusal, `models` and `generate`: no key, message, reply, SQL, provider message or URL path. (tungstenite's TRACE line quotes whole frames, key included; the server never writes dependency lines below WARN, and the desktop has no tungstenite.)
- **The module's fetch bridge** (the Rust half of Decision 8's): `start(id, method, url, headers, body) → Promise<status>`, `read(id) → Promise<Uint8Array | null>`, `abort(id)`. Rust picks the ids, so a send dropped before the head aborts too; the body stream aborts on drop unless it ended. Errors carry fixed text, never the URL or the page's message. Without a bridge every `ai` call is `NOT_SUPPORTED`; `index.ts` still passes three arguments, so the demo has no assistant until Task 8. The module grew from 1,506.5 KB to 1,605.8 KB brotli (budget 2,000,000).
- **Found on the way.** Task 4's `0007` made the TypeScript 5d-2 state replay (`state-replay.svelte.test.ts`) fail on the new `ai_messages.parts` column (the browser-test module hadn't been rebuilt since). Its dump now drops `parts` only when NULL (`NULL_ONLY_COLUMNS`), as Rust's `LINK_COLUMNS` check does.
- **For Task 6.** Nothing in the RPC changed for MCP; `cargo test -p seaquel-mcp -p seaquel-cli` passes.
- **For Task 7.** The desktop page's `getAIApiKeyForProvider` (`services/keyring.ts`) resolves `null` (review I2), so the TypeScript assistant on desktop can't send an Anthropic key between this task and Task 7, which deletes it; see "Notes for Task 7". `db.connect`/`test` take `savedConnectionId`. Turns ride `core_stream` and `/rpc/stream` as `{type: "ai", streamId, event}`; a refused frame is an `ai` error; the TS clients' stream code must treat `ai` like `run` (one terminal event, nothing after a cancel). `ai.respond` goes through the unary call.
- **For Task 8.** Pass the page's fetch bridge as `open`'s fourth argument (the type is `FetchBridge` in `transport.ts`); on a trap restart the transport must pass it again. Anthropic's direct-access header is already sent by the module (`FROM_BROWSER`, pinned in `ai-turn.test.ts`). The module's `ai.chat` runs under `AiEgress::Any`.

- **Review fixes (Task 5 review).**
  - *I1:* `ai.generate`, `ai.models` and `ai.test` count toward `MAX_EDIT_CALLS_PER_USER` with the edit calls (`is_edit_call` became `is_slow_call`): with four stalled at the provider, a fifth is 429 before it sends anything, and another user isn't held up. `ai.respond` doesn't count; a turn is under Core's `WEB_AI_LIMITS`.
  - *I2:* `TauriKeyring.getAIApiKeyForProvider` resolves `null` without asking the keychain, so Settings → AI opens and Test/Models answer false or empty on the desktop until Task 7 (see "Notes for Task 7"). No `.svelte` file changed.
  - *M1:* `TURN_STOP_WAIT` is `seaquel_core::storage::WRITE_WAIT + 15 s` (45 s), derived from the constant, so it exists with `seaquel-rpc`'s `ai` feature; a test pins the relation.
  - *M2:* under `Public`, `seaquel-http` decides the proxies itself (`proxy_plan`: a given proxy, else the environment's with `NO_PROXY`, else none) and turns reqwest's automatic proxies off, so the OS proxy can't be switched on by feature unification. Unit-tested on the plan (no OS proxy touched); the license client and `Any` keep reqwest's behaviour. Documented for Task 10.
  - *M3:* the `secret` group names its own key forms (`db:`, `ssh:`, `ssh-key:`, `license-key`) before the store's check, and refuses `ai-api-key:*` as "managed by Core". `seaquel-secrets`' message is the store's (Core writes AI keys there) and is unchanged.

## Task 6: MCP onto the registry

**Files:** `crates/seaquel-mcp/src/{server.rs,tools/*,exposed.rs}` (call `seaquel_core::ai::tools::call`; resolution and connect-on-first-use stay), `Cargo.toml` (`seaquel-core` with `ai`), `format.rs` removed.

**Tests first:** none new: `tests/tools.rs` and `seaquel-cli`'s `tests/stdio.rs` pass unchanged, and the tool schemas MCP lists equal `tool-schemas.json`'s MCP profile.

**Run:** `cargo test -p seaquel-mcp -p seaquel-cli`.

**Review:** no change in any tool's output; the call timeout and the keychain-wait accounting unchanged; the CLI still refuses a file `0007` hasn't reached (`STORAGE_NEEDS_UPGRADE`).

**Things this task could quietly skip:** `INSTRUCTIONS`; the `truncatedCells` field; saved-query sharing's "how many left out" count.

### Notes from Task 6 (as built)

- **What moved.** MCP's six connection tools go through one function, `tools::on_connection` (`seaquel-mcp/src/tools/mod.rs`), which ends in `seaquel_core::ai::tools::call` with `Profile::Mcp` under the server's `timed` (connect on first use inside it, as before). Deleted from `seaquel-mcp`: `tools/query.rs` (270 lines: `Rows`, the plan cut, `max_rows`, `run_rows`), `tools/schema.rs` (153: the listings, describe and the table lookup), `format.rs` (the 5-line shim), the seven argument structs and `require_schema`/`require_data`/`schema_sharing_off` from `tools/mod.rs` (167 → 124), the lookup and run from `tools/saved.rs` (174 → 152), the query id counter from `server.rs` (507 → 498); `schemars` and `futures` left its `Cargo.toml`. The crate's `src/` went from 2,008 to 1,506 lines. `list_connections` and `list_saved_queries`' gathering across the exposed projects stay MCP's (nothing else has an exposed set); the latter renders with `render::saved_queries(Profile::Mcp, …)`, hidden count and all. The sharing refusals are `ToolError::schema_sharing_off`/`data_sharing_off`; `error.rs` re-exports the registry's `INVALID_ARGUMENT`, `SCHEMA_SHARING_OFF` and `DATA_SHARING_OFF`.
- **One set of arguments.** The `#[tool]` methods take `seaquel_core::ai::tools::mcp::*` as their `Parameters`, so rmcp still parses them (its wording, unknown fields ignored) and lists their schemas; `tool_schemas.rs` passes unchanged. That makes `seaquel_ai::tools::mcp` the only copy, so it stays (removing it would leave none). Each struct becomes a `Call` through `From` (`seaquel-ai/src/tools/mod.rs`, `mcp_calls`), which `parse_mcp` now uses too.
- **`ToolContext::timeout`** (new): the database's statement timeout for the call's query or EXPLAIN. `call` had used `CALL_TIMEOUT` for it; MCP passes its `call_timeout` (60 s unless set), the assistant `CALL_TIMEOUT`, so the database's timeout still follows a configured call timeout. `timed_out(limit)` words the turn's tool timeout. (Superseded by the review fix below: `call` has no deadline of its own.)
- **Order kept.** `on_connection` checks, as MCP did: the connection, `max_rows`, a saved query's lookup (before sharing), the tool's sharing flag (re-read per call), a saved query's engine and parameter values (before connecting), then connects and calls the registry. The lookup and the parameter check run again inside `call`'s `plan`; they cost a storage read and are what keeps `SAVED_QUERY_NOT_FOUND` ahead of `DATA_SHARING_OFF` and `INVALID_PARAMETERS` ahead of a connect error. One difference: a substitution error (`substitute`'s own, past the parameter check) now comes after the connect, not before; no test reaches it. `READ_ONLY` for `run_query` is still `query_stream`'s; for a saved query it is `plan`'s `read_only_sql`, the same function and text on the same connection's rules (Task 4's note).
- **Stream ids** are the registry's `ai-<uuid>` instead of `mcp-<n>`; nothing outside Core sees them.
- **Features.** `seaquel-mcp` already had `seaquel-core/ai`; `seaquel-cli`'s Core features are unchanged (`ai` through `seaquel-mcp`, no `ai-native`, no `seaquel-http` or `reqwest` in its tree). `crates:check` passes with no rule change.
- **Tests.** `seaquel-mcp`: `tools.rs` 35 (with `SEAQUEL_TEST_POSTGRES` and `SEAQUEL_TEST_REQUIRE_ENGINES`, so the Postgres cases ran), `tool_schemas.rs` 1; `seaquel-cli`: 2 unit, `cli.rs` 3, `stdio.rs` 8; `seaquel-ai` 130 across its binaries; Core `ai_turn.rs` 34 and `ai_replay.rs` 5. No pinned test changed.
- **For Task 10.** CLAUDE.md's `seaquel-mcp` section still names `tools/query.rs` and `format::MAX_CELL_BYTES`; they are now the registry's (`seaquel_ai::limits`, `tools::format`).
- **Review fixes (Task 6 review).**
  - *Important: one call deadline.* `ai::tools::call` had raced its work against `ctx.timeout` on the executor's clock. That deadline couldn't see MCP's keychain-wait extension (the `SecretWait` counts a pending read in any call, so call A's `timed` deadline moves while call B waits on a prompt, and Core's didn't), and it cut `list_tables` and `describe_table`, which had no timeout of their own. The earlier note that "MCP's always fires first" was wrong. `call` now sets no deadline: MCP's `timed` is the only one for its calls, the assistant's turn races `run` against its own timer as before, and `ctx.timeout` is only the database's statement timeout. Pinned by `seaquel-core/tests/ai_tools.rs` (1, new): a Core with an executor, a 20 ms `ToolContext::timeout` and a query that takes 200 ms ends `Ok`, and the database got the 20 ms; it failed first with `TIMEOUT: The call took longer than 0.02 s`.
  - *Minor.* `limits::CALL_TIMEOUT` and `QuerySpec::new` say Core replaces the timeout with `ToolContext::timeout` (MCP: the server's call timeout); `QuerySpec::new` keeps its signature, so `ts_baseline.rs` is untouched. `engine_of` uses `ToolError::read_only()`. `tools/saved.rs`'s `check_parameters` says why there are two engine sources: the row's `type` for the "Unknown engine" refusal before connecting, the connection's `sql_engine` for substitution and the read-only check. `parse` and `definitions` say their `Profile::Mcp` arm is there to pin the registry to `tool-schemas.json` (the server goes through rmcp and `From`).

### Checkpoint 6a

The full check list, the live run with every engine, MCP against a real database through `seaquel-cli mcp`, and the mock-provider suites on desktop, web and the module. The GUI still uses the TypeScript path.

## Task 7: The GUIs on Core

**Files:** `src/lib/hooks/database/ai/` (new: `index.ts`, `core-ai.ts`, `events.ts`, `messages.ts`); `hooks/database/ui-state.svelte.ts` and `ai-chat-manager.svelte.ts` (the turn follows events; per-connection "Allow all"; `done.messages` applied by `seq`); `components/ai-assistant.svelte` (approval through `respond`, Q7's tool lines); `components/query-editor/ai-inline-prompt.svelte.ts` (Q9); `stores/ai-settings.svelte.ts` (`models`, `test` through Core); `services/keyring.ts`; `hooks/database/connection-manager.svelte.ts` (`savedConnectionId`); `src-tauri/tauri.conf.json` (Decision 19); deletions listed under "TypeScript"; messages (i18n).

**Tests first** (vitest with a fake `AiService` that plays event sequences):
- a turn streams into its placeholder; `done` replaces it with the stored rows by `seq`; another window's `chatMessages` event during a turn waits for the turn;
- the approval card calls `respond` with `allow`, `deny`, `allowAll`; "Allow all" sticks to the connection, not the session;
- a dashboard `clientTool` runs `handleDashboardToolCall` and answers; Stop answers nothing and cancels;
- each new code's wording; the inline prompt inserts and doesn't run;
- web sends the vault key; desktop sends none and never calls `secret.get` for `ai-api-key:*`.

**Run:** `mise exec -- npx vitest run src/lib`; `npm run check` 0/0; svelte-autofixer on changed `.svelte` files; one desktop run in `tauri dev` with the mock entered as an OpenAI-compatible provider's base URL (no test-only switch in the app).

**Review:** no provider URL in `src/`; the page's AI state machine has no decisions left (what to send, which tools, when to store); the CSP still lets the extension list load.

**Things this task could quietly skip:** a pending-model-selection message (no model) still retried after the model is chosen; a deleted chat during a turn; the "chat full" state from `CHAT_FULL`; the mention popover still offered with schema sharing off.

### Notes for Task 7 (from Task 5's review, I2)

- The page can no longer read a provider's key on the desktop: `TauriKeyring.getAIApiKeyForProvider` resolves `null` without a call (the `secret` group refuses `ai-api-key:*`). The settings form needs a "key saved" signal from Core instead: `hasKey` per provider in `aiSettingsGet` (from the keychain on desktop; on web, whether the vault holds one is the page's own knowledge).
- Three callers must move to Core: `ai-provider-section.svelte:65` (`providerFormHasExistingKey`, to `hasKey`), `ai-settings.svelte.ts:238` (`testConnection`, to `ai.test`) and `:252` (`fetchModels`, to `ai.models`). Until then, on desktop, Test answers false and Models nothing for an Anthropic provider, and both run keyless for an OpenAI-compatible one.

### Notes from Task 7 (as built)

- **Files.** New: `src/lib/hooks/database/ai/` (`index.ts` the `AiService` seam with `getAi`/`setAi`, `core-ai.ts` `CoreAi`, `events.ts` the reply's segments, `messages.ts` the wording, `testing.ts` a `FakeAi` for the page's tests). Rewritten: `ui-state.svelte.ts` (the AI half follows Core's events), `ai-chat-manager.svelte.ts` (`applyTurnRows`, `markFull`, `refetchAfterTurnFor`; no `persistMessages`), `ai-inline-prompt.svelte.ts` (+ the overlay's `notice`), the AI half of `stores/ai-settings.svelte.ts` (`hasKey`, `models`/`test` through Core). Changed: `ai-assistant.svelte` (tool lines, the error under a reply, the cut note, per-connection "Allow all"), `ai-provider-section.svelte` (`hasKey`), `services/keyring.ts` (`aiKeyVault()`; no AI key read on the desktop or in the demo) and `vault-keyring.ts` (`hasAIApiKeyForProvider`, a row check that never unlocks), `connection-manager.svelte.ts`, `providers/{types,core-provider}.ts` (`bindSaved`), `core/{client,tauri,http}.ts` and `core/browser/client.ts` (`AiChatRequest`, `isStreamFrame`: the `{type:"ai"}` frames on all three transports), `state-restoration.svelte.ts` (the merge by `seq`), `state.svelte.ts` (`aiMessagesStored` replaces `aiMessagesSent`), `library/{convert,types}.ts` (`parts` → segments, the `aiMessage` row key), `types/query.ts` (`AiSegment`, `AiToolLine`, `error`, `truncated`, `pendingApproval.allowAll`), `database.svelte.ts`, `src-tauri/tauri.conf.json`, `messages/en.json` (39 keys). Deleted: `services/ai/providers.ts`, `tool-definitions.ts`, `index.ts` (the loop, `handleToolCall`, `generateSQL`) and `index.test.ts`; `context.ts` keeps only `readOnlyError` (`executeReadOnly` uses it); `ai-mentions.ts` keeps `buildMentionItems` (+ `mentionItemsFor`, the popover's sharing gate) and loses `resolveMentions`; `ui-state.svelte.test.ts` and `ai-chat-stream.svelte.test.ts` (their checks moved to `ai/turn.svelte.test.ts`); the old `getAIApiKeyForProvider` shim. `dashboard-tools.ts` stays: `handleDashboardToolCall` answers `clientTool`.
- **Rust, beyond the file list.** (1) **`hasKey`** (superseded by review I2, below: `aiProviderHasKey`). (2) **`db.bindSaved {connectionId, savedConnectionId}`** (`Workspace::bind_saved_connection`, `Core::bind_saved_as`): `add` connects the form before Core has made the row, so the new connection had no recorded saved id and every turn on it was `CONNECTION_MISMATCH` until a reconnect. The page binds it right after `connectionCreate`. Once per connection: the same id again is a no-op, another (or rebinding a saved target's connection) `INVALID_ARGUMENT`, another workspace's `CONNECTION_NOT_FOUND`. Trusted exactly as a form connect's `savedConnectionId` is. Reconnect sends `savedConnectionId: <the row>`; `autoReconnect` (a saved target) records its own; `test` names none.
- **The turn in the page.** `sendAIMessage` sends what was typed (Core resolves mentions), `approval: "allowAll"` when the session's per-connection flag says so, `clientTools: true`, the chat's open Core connection and the provider id (for the web's key; `CoreAi` strips it). The page stops a send itself only where it can't name what Core needs: the chat's connection is gone (pinned text), not open ("Connect {name} to ask about it."), or has no model (the pending message, retried under its own user-message id once a model is chosen). A new send stops the turn running anywhere. `done.messages`/`error.messages` go through `applyTurnRows` with the event's `seq` on per-row keys (`rowKey("aiMessage", id)`); `restoreAIChatMessages` keeps a row whose applied `seq` is newer than the list's, and drops a stored row a newer list lacks. A message the page shows that Core hasn't stored (in flight, pending, refused) survives a refetch. Another window's `chatMessages` event during a turn waits for it (as before). An `error` without rows from a transport ending (`WS_CLOSED`, …) re-reads the chat after the turn, since Core may have stored it. `CHAT_FULL` marks the chat full (banner and toast). Stop cancels and answers nothing; deleting the chat cancels too. `respond` always goes after a microtask (Decision 17). Q7's lines come from `toolCall`/`toolDone`/`approvalRequired` live and from `parts` after `done` or a reload (rows and the cut merged from the live view).
- **Keys.** Desktop: no AI key read exists in the page any more (`TauriKeyringService` lost `getAIApiKeyForProvider`, `aiKeyVault()` is `null`); pinned by `keyring.test.ts`, `ai/core-ai.test.ts` (no `secret` call) and `ai/turn.svelte.test.ts`. Web: `CoreAi` asks the vault for a provider's row first and decrypts only when there is one, so a keyless provider never prompts an unlock; a cancelled unlock ends the turn with `VAULT_LOCKED`.
- **CSP (Decision 19).** `connect-src 'self' ipc: http://ipc.localhost ws://localhost:1420 https://duckdb.org`. `ws://localhost:1420` is the dev server's HMR socket: `'self'` covers it in CSP3, but older WebKit didn't match `ws:` to `'self'`, and the one socket costs nothing. Every other `fetch` in the desktop page is relative (`'self'`); the updater and licensing are Rust. `ai/no-model-calls.test.ts` pins the directive, that the extension list's URL is in it, and that no provider URL or header is in the page's code (tests may name what Core sends).
- **Tests.** New: `ai/turn.svelte.test.ts` (23), `ai/page-replay.svelte.test.ts` (21 `page.json` cases + 6 `turns.json` dashboard cases), `ai/messages.test.ts` (32), `ai/events.test.ts` (7), `ai/core-ai.test.ts` (7), `ai/no-model-calls.test.ts` (4), `query-editor/ai-inline-prompt.svelte.test.ts` (22), `stores/ai-settings-core.svelte.test.ts` (4), `core/browser/client.test.ts` (2), `services/ai-mentions.test.ts` (2), 3 Tauri and 3 web transport tests, 3 connection-manager tests, `keyring.test.ts` rewritten (3); `ai-chat-core.svelte.test.ts` and `library/sync-state.test.ts` moved to `FakeAi`. Rust: `seaquel-rpc/tests/ai.rs` `bind_saved_records_a_connection_opened_before_its_row`, `tests/state.rs` `ai_settings_get_says_which_providers_have_a_key`.
- **Pinned page views.** `page.json`: all 21 cases' `messages` (the three `$absent` ones as worded in the baseline README's "Corrections"), `approvals` (the cards shown) and `allowAllAfter` (read as "some connection has Allow all"); the fake Core's stored rows are checked against `stored` so it can't drift from Core. `turns.json`: the six `dashboards` cases' `dashboardCalls` and `create_dashboard`'s answer (= the `tool_result` Core sent). `generate.json`: all 21 cases' `inserted`, `executed` (0), `notice`, `error` and `toasts`. `errors.json`: the 7 coded cases' `shown`.
- **The state replay** (`state-replay.svelte.test.ts`) skips ten 5d-2 chat cases whose turn the page used to store (`CORE_STORES_THE_TURN`: first/second turn, error, throw, Stop, Stop at a card, flush, two tabs, retitle, delete while streaming); Core's `ai_replay.rs` pins those writes and the page tests pin the view. 101 cases and 338 steps run.
- **i18n.** 39 keys (`ai_error_*`, `ai_tool_*`, `ai_inline_*`, `ai_reply_truncated`, `ai_allow_all_connection`), translated by the `i18n-translator` agent. The inline prompt's English is today's (pinned by `generate.json`), now in i18n.
- **Review fixes (Task 7 review; they supersede the bullets above where they differ).**
  - *I1:* a supplied key names its provider: `ChatParams.providerId` and `GenerateParams.providerId` go with every `apiKey` (`CoreAi` sends both or neither). Core checks it right after resolving the connection's provider, before `AI_DISABLED`, the keychain or any request: a key without `providerId` is `INVALID_ARGUMENT`, a key for another provider `AI_PROVIDER_CHANGED` (409; worded "The connection's AI provider changed. Send your message again."). Pinned by `seaquel-rpc/tests/ai.rs` (`a_supplied_key_for_another_provider_is_refused_before_any_request`: no request reaches the mock, nothing stored), `ai/core-ai.test.ts` and `ai/messages.test.ts`; the existing key-supplying tests (rpc, server, Core, the module's `ai-turn.test.ts`) name their provider now.
  - *I2:* `aiSettingsGet` reads no secret again (`get_ai_settings_for_page` is gone). `settings.aiProviderHasKey {id}` (`Workspace::ai_provider_has_key`, `Seqd<bool>`, a read in `SETTINGS_METHOD_KIND`) reads that one provider's keychain entry; `NOT_SUPPORTED` without a secret store (web: the page asks its vault), `AI_PROVIDER_NOT_FOUND` for an unknown id. The settings form asks only when it opens a provider (`aiSettingsStore.hasKey`). Pinned with a counting store: `aiSettingsGet` 0 reads, `aiProviderHasKey` 1.
  - *I3:* the "Allow all" box is the card's own: `pendingApproval.allowAllTicked`/`setAllowAllTicked`, every card starts unticked, and `approve()` answers `allowAll` when its own box is ticked. Card ids are `<streamId>:<callId>`, so a later turn's `call_1` is another card.
  - *M1:* the error under a reply is plain text (no `{@html}`); the three keys lost their Markdown (`Settings → AI`).
  - *M2–M3:* `turn.svelte.test.ts` pins the first message's `chatUpdate {title, touched}`; `chats/retitle-stored-chat`'s skip reason says what moved where; `page/allow-all-other-connection` asserts conn-1 has Allow all and conn-2 doesn't.
  - *M4:* Allow all is stored with what the connection pointed at (engine, host, port, database): removed, or pointing elsewhere, it's forgotten (a rename keeps it). `resetAISessionState` (dead) is gone.
  - *M5:* Stop re-reads the chat once the turn ends (Core stored what streamed). A list read no longer sorts the page's own unstored rows by time (Core's and the page's clocks differ): they keep their place after the row shown before them, so a half-stored turn can't put the reply above its question.
  - *M7:* the card's, the pending-model line's and the SQL block's English moved to i18n (`ai_approval_query`, `ai_approval_allow`, `ai_approval_deny`, `ai_choose_model_to_send`, `ai_new_messages`, `ai_open_in_editor`, `ai_sql_tab_title`), translated with `ai_error_provider_changed` by the `i18n-translator` agent.
- **Re-review fixes.** *Stop and Core's late store:* the transports end a stopped turn's iterator at once, so the re-read after the turn usually runs before Core stores the reply, and Core's `chatMessages` event for it carries this page's origin, which the feed skips. Stop now marks the chat (`AIChatManager.awaitOwnStore`), the feed lets an own-origin event through when `acceptOwn` says so (`UseDatabase`: a `chatMessages` event for a marked chat), and the mark clears once that refresh is applied; pinned in `library/sync-state.test.ts` (the list answers first without the reply, then the own-origin event arrives and is read; a later own event is skipped again). *`isAllowAll` is pure:* reads never write `$state`; `forgetStaleAllowAll` drops entries whose connection is gone or points elsewhere, called by `ConnectionManager`'s new `onConnectionsChanged` (after a list is applied and when a connection is forgotten) and before each turn.
- **For Task 8.** The demo has `aiKeyVault() === null`; its session key needs its own source in `CoreAi` (`vault` is a constructor argument, so a session-key provider plugs in there). The assistant is still hidden in the demo (`features.aiAssistant`). `FakeAi` (`ai/testing.ts`) is there for its page tests.
- **For Task 9.** Things to probe: the `ws://localhost:1420` entry under `tauri dev` on macOS; a reload mid-turn on web (the socket closes, Core stores the reply; the page re-reads the chat on the next load only); `WS_CLOSED` mid-turn re-reads the chat after the turn; a slow vault unlock before a turn starts.

## Task 8: The demo's assistant (Q2)

**Files:** `src/lib/core/browser/fetch-bridge.ts` (new), `src/lib/core/browser/index.ts` (pass it to `open`), `crates/seaquel-browser/src/module.rs` (the bridge type), `features/index.ts` (`aiAssistant` on), the AI settings form's demo branch (session key, CORS note), `services/keyring.ts` (the demo keeps keys in memory for the session).

**Tests first:** the module's Node harness runs a turn through the bridge against the Node mock with the direct-access header present; reload forgets the key; the snapshot in IndexedDB never contains it.

**Run:** the Node harness; `npm run build:demo` and its size check (budget 2,000,000 bytes brotli).

**Review:** the key is in no storage, snapshot, `localStorage`, URL or log; abort on drop.

**Things this task could quietly skip:** the trap-restart path (a restarted module has no key; the page re-sends it); the tutorial (unchanged, no AI).

### Notes from Task 8 (as built)

- **Files.** New: `src/lib/core/browser/fetch-bridge.ts` (`makeFetchBridge(fetch?)`), `src/lib/services/session-keys.ts` (`SessionKeys`, `sessionKeys()`), `src/lib/core/browser/testing/mock-provider.ts` (the mock provider and `localFetch`, moved out of `ai-turn.test.ts` and shared). Changed: `transport.ts` (`BrowserCoreOptions.fetch`, passed as `open`'s fourth argument by `openInstance`, so the first open and every restart's get it), `index.ts` (`OpenBrowserCoreOptions.fetch`: `makeFetchBridge()` when left out, `null` for none; re-exports `makeFetchBridge`), `services/keyring.ts` (`aiKeyVault()` is the session keys in the demo), `stores/ai-settings.svelte.ts` (the demo branch of add, update and remove), `features/index.ts` (`aiAssistant: true`), `components/settings/ai/ai-provider-section.svelte` (the notes), `hooks/database/ai/core-ai.ts` and `demo/core.ts` (docs), `messages/en.json` (3 keys). No Rust changed: Task 5's `open(bridge, image?, onTrap?, fetch?)` and `fetch.rs` were enough, so the module is the same size.
- **The bridge.** `start` sends what Rust built with `credentials: "omit"`, `referrerPolicy: "no-referrer"`, `cache: "no-store"` and `redirect: "error"` (a redirect fails the call, as Decision 9 has natively); a GET or an empty body sends none. `read` gives chunks, then `null` (also for a finished, aborted or unknown id). `abort` aborts the signal before the head and cancels the body after it. A failure rejects with fixed text, never the browser's message (it can name the URL). Nothing logs.
- **The key.** `SessionKeys` is a `Map` in the page's memory, one per page load; nothing writes it anywhere. The settings store, in the demo only, creates and updates the provider in Core without a key (as on web) and keeps the key in `sessionKeys()`; `""` (clear) and a removed provider forget it. `hasKey` reads it through `aiKeyVault()`, and `CoreAi` already sent a vault's key with its `providerId` (Task 7, I1), so `CoreAi` itself didn't change. The module never holds a key between calls, so a trap restart needs nothing from the page but the next call, which sends the key again.
- **The form** (demo only, `isDemo()`): under the key field "In the demo, your API key stays in this page's memory for this session only. It is never stored, and it is forgotten when you reload or close the page."; under an OpenAI-compatible base URL, the CORS note naming `window.location.origin`; a set key reads "API key set for this session".
- **Build constants.** `aiKeyVault()` and the store's three branches test `import.meta.env.VITE_BUILD_TARGET === "demo"` itself, so `session-keys.ts` is dropped from `build` and `build-web` (checked: its `keys.has(…)`/`keys.set(…)` code is only in `build-demo`); the fetch bridge comes only through `$lib/core/browser`, which only the demo's start loads. The form's two notes are plain i18n strings and show up in every build's chunks, as all messages do; their `{#if}` is the runtime `isDemo()`.
- **Tests.** New: `fetch-bridge.test.ts` (7), `stores/ai-settings-demo.svelte.test.ts` (4), `demo/assistant.svelte.test.ts` (4: a turn through the page's bridge with the key, then the key searched for in every snapshot's bytes, `localStorage` with the view-state journal written at `pagehide`, `sessionStorage`, the mock's request paths, the page's log calls, `console` and toasts; a reload forgets the key and sends none; a trap restart, then a turn that sends the key again; Stop aborts the request and the mock sees the client go), `demo/tutorial-no-ai.test.ts` (2). `ai-turn.test.ts` now runs on `makeFetchBridge` (over `localFetch`) and gained a drop test (`__test_stream_dropped_after`: the bridge's `abort` is called and the mock sees the client go). `no-model-calls.test.ts` skips `src/lib/core/browser/testing/` (test-only; the harness sends Anthropic's fixed URL to the mock).
- **Found on the way.** After a trap restart the demo connection's active state comes back only once `autoReconnect` finishes (a few hundred ms after the new Core id appears); a message sent in between has no active connection and isn't sent. Not changed here (phase 8's behaviour); the page test waits for it. Worth a look in Task 9's probe.
- **For Task 10.** CLAUDE.md's "The demo" product choices still say the demo has no AI assistant; it has it now (the visitor's key in page memory for the session, the fetch bridge, the CORS note).
- **Review fixes (Task 8 review).**
  - *Important: a trap left the dead instance's model request running.* `makeFetchBridge` has `abortAll()` (every running controller and reader; `FetchBridge.abortAll?` is optional for other bridges), and `BrowserCore.reinstantiate` calls it next to DuckDB's `closeAll()`. Pinned by `demo/assistant.svelte.test.ts` (a trap during a stalled streaming turn: the mock sees the client go) and `fetch-bridge.test.ts` (before and after the head); both failed first.
  - *The scan's skip* is now only `testing/mock-provider.ts`, and `no-model-calls.test.ts` checks that no app file imports `core/browser/testing` (the one test-support file that does, `library/fixture-support.ts`, is checked to be imported only by tests).
  - *Docs:* `transport.ts`'s orphaned "The module's exports" line sits on `BrowserModule` again.
  - *`settings_ai_demo_cors_note`* also says the key goes in the key field, never in the URL (the URL is saved). Needs translating again.
  - *The supplied key in provider messages, on every interface.* `seaquel-ai`'s `cut_message` replaces every occurrence of the request's key with `<redacted>` (`wire::REDACTED`) before the 1 KiB cut, so a key straddling the cut leaves none of its bytes. The key reaches it through `check_status_with(status, body, secret)` (`check_status` is `None`) and `Decoder::with_secret(kind, secret)` (stream `error` events, both providers; `Decoder` stays without `Debug`). Core passes the key it used (supplied or the keychain's) in the turn's rounds, `generate`, `models` and `test` (`call_once` and `models_call` carry it). An empty key redacts nothing. Pinned by `seaquel-ai/tests/wire.rs` (2) and `seaquel-core/tests/ai_turn.rs` (3: a 401 JSON echo, a stream error and `ai.test`'s plain-text 401 with the key twice, the key across the cut); all failed first. The module is 1,635,683 bytes brotli (+1.1 KB).

## Task 9: Probe

Real browsers (Playwright's Chromium, Firefox, WebKit) for the demo and web, the desktop app in dev, and a mock provider that misbehaves: stalls, sends 1 MB deltas, 10,000 tiny deltas, two tool calls per round for 20 rounds, a 302 to `127.0.0.1`, an `error` after 30 s. Plus a 5,000-table schema (Postgres), a chat at the web's message cap, Stop at each waiting state, a reload mid-turn, eviction mid-turn (`SEAQUEL_WORKSPACE_CAP=1`), and the log grep for the test key and planted markers. Each finding is fixed and reviewed in "Probe fixes".

### Probe findings

2026-10-02, ~1.65 h wall. Scratch harness in `scratchpad/p6-probe/` (`mock.mjs`, `lib.mjs`, `weblib.mjs`, `t-*.mjs` for the demo, `wt-*.mjs` for web; outputs in `out/`, screenshots in `shots/`).

**Setup.**
- *Demo:* a `build:demo` from a scratch copy of the tree, with the `test-hooks` module in `browser-pkg/` and one probe line exposing the opened Core (for `__test_trap` and holding `ai.respond`), served under `/demo`.
- *Web:* `dev:web:full`'s two processes (`with-internal-secret.mjs` + `concurrently` running `rust:dev` and `dev:web` on port 18970), `DATA_DIR` in the scratchpad, an air-gap bundle for the licence, and four users signed up as `probe*@example.invalid`. Each user has a Postgres connection to a scratch database `p6_probe`, dropped afterwards.
- *The mock:* an OpenAI-compatible server on `127.0.0.1:8899` with CORS for any origin. Its mode is switched per run. Provider key `test-key-not-real`, session key in the demo, vault key on web.

**Probes.**

| # | Probe | What happened | Result |
|---|---|---|---|
| 1 | Browsers | Turns streamed, were stored and came back after a reload in Chromium, Firefox and WebKit, on both the demo and web. The key reached the mock as `Bearer` on both. Under `SEAQUEL_AI_EGRESS=public` the `http:` mock gives `AI_EGRESS_BLOCKED` (503 on `ai.test`, an `ai` error on the socket, "This server doesn't allow calls to that AI provider…", 0 requests at the mock). `https://` to `127.0.0.1`, `localhost`, `[::1]`, `2130706433`, `0x7f.1`, `10.0.0.1`, `169.254.169.254`, `[::ffff:127.0.0.1]` and `127.0.0.1.nip.io` are all refused the same way. | PASS |
| 2a | Stall before the head / mid-stream | In both builds and all three browsers, `TIMEOUT` came after 120.1–121.0 s ("The provider didn't answer in time"). The mock saw the client go, and the partial text was kept after a reload. | PASS |
| 2b | 1 MB deltas (3 × 1 MiB) | Demo: shown and stored (3,145,716 bytes); the longest frame gap was 233–256 ms. Web: 3 text frames (largest 1,048,671 bytes), then the reply's write was refused. See F2. | FAIL (F2) |
| 2c | 10,000 tiny deltas | Shown and stored in every browser. Web sent 6 frames per turn (`started`, 4 `text`, `done`): the text is coalesced. Longest frame gap during the stream was 34–50 ms. | PASS |
| 2d | Two tool calls per round for 20 rounds | `TOOL_LIMIT` after 20 tool lines (`toolCall` 20, `toolDone` 20, the 21st never ran; logged as `rounds=11 tool_calls=21`). The worded notice was shown and `parts` were stored. | PASS |
| 2e | 302 to `127.0.0.1` | Web: "AI provider error: The provider answered HTTP 302." with nothing followed. Demo: the bridge's `redirect: "error"` gives "Could not reach the provider." WebKit's own console line names the URL path; that line is the browser's, not ours. | PASS |
| 2f | `error` after 30 s | After 30.2–31.3 s: "AI provider error: MOCK_ERROR_MSG_9Z8 …". The text that had streamed was stored. | PASS |
| 3 | 5,000 tables (Postgres) | The first request reached the mock 858 ms after send, including the vault unlock (~490 ms of argon2). The system prompt was 100,408 bytes with all 5,002 tables (names only, no cap note); see F3. `list_tables` returned 244,022 bytes. The whole turn took 986 ms and the longest frame gap was 591 ms (the unlock). An earlier 12.8 s freeze was the harness's own `getByRole` polling, confirmed with a CPU profile. | PASS (note F3) |
| 4 | Chat at the web's cap | At 4,998 messages, one turn ran. At 5,000 the send was refused with `CHAT_FULL` and no request reached the mock; the banner and toast showed and the input was disabled, still disabled after a reload. At 66.2 MB, past `max_chat_bytes` less 1 MiB, the list says `full` and the chat opens with sending off. Rendering a 66 MB chat froze Chromium for 1.3 s and Firefox for 7.9 s. | PASS |
| 5 | Stop at each waiting state | Tested in the demo and on web, in all three browsers, at four points:<br>- *Streaming:* the mock saw the client go and the text so far was stored.<br>- *At approval:* nothing ran and the card went away.<br>- *During a tool:* web's Postgres `generate_series` was cancelled on the server (active 1 → 0); the demo's DuckDB `range(2e10)` stopped and the next query answered at once.<br>- *During a client tool* (`ai.respond` held 8 s): the late `respond` got 404 `NOT_FOUND` with no error shown.<br>After each, the next turn ran in 380–450 ms. Pressing Stop in the first seconds of a web turn hits a toast instead of the button; see F1. | PASS (F1) |
| 6 | Reload mid-turn | Web, all three browsers: Core cancelled within 4–23 ms, and after the reload the reply held exactly what had streamed (word23 / 21 / 22). A socket dropped mid-turn showed "The connection to the server closed…"; the turn stopped and the next send worked. A slow vault unlock (45 s) still ran the turn; a cancelled unlock showed "Unlock your vault…", sent nothing, and the next send asked again. Demo: the reply is lost and only the user's message stays, since Core lives in the page (Q8's crash case). | PASS (web); demo as Q8 allows |
| 7 | Eviction mid-turn (`CAP=1`, second user) | Chromium and WebKit: `WORKSPACE_EVICTED` arrived, the turn ended with an `ai` `CANCELLED`, the mock saw the client go 21 ms later, and the reply was stored up to the last word streamed. | PASS |
| 8 | Logs | Markers: the key, a prompt, a schema (`marker_schema_tbl_4r7`, `marker_col_2j6`), a cell, a reply, a provider message and the URL path. All of them reached the mock except the key (sent as a header) and the provider message and URL path (never in a request body). None of them, nor `Bearer` or the vault passphrase, is in the server's stderr or Node's output (19 logs, 1,602 lines) or in any browser console (demo and web, all three browsers). AI lines carry only ids, provider kind, model, codes, counts and durations. | PASS |
| 9 | Demo trap mid-turn | In all three browsers, `__test_trap("async")` during a stream ended the turn with "This chat's connection was closed or reconnected…", and the mock saw the client go 5–10 ms later (`abortAll`). Sending 0, 60 and 200 ms after the trap reached the mock and answered, so Task 8's flag didn't reproduce. In WebKit at 60 ms the panel was briefly empty, but the send still went through. | PASS |
| 10 | Desktop in `tauri dev` | Not run: it needs a display and can't run headless. It stays the owner's manual check 13. | Not run |

**Findings.**

- **F1, Minor: the "Vault unlocked" toast covers the assistant's Stop button.**
  - Evidence: on web, the first turn after a page load decrypts the key. `vault-unlock-dialog.svelte:39` toasts "Vault unlocked", and the toaster sits `bottom-right` (`src/routes/+layout.svelte:25`), right over the Send/Stop button at (1392, 804). `elementFromPoint` at the button's centre is the toast. A real mouse click there didn't stop the turn. Playwright waited 1.9 s (Chromium) and 2.4 s (Firefox) for the toast to clear; 19–24 more words streamed meanwhile and were stored.
  - Fix: no toast when the unlock was started by a send (the dialog closing says enough). Or move the toaster away from the assistant's footer while the panel is open.
- **F2, Minor: oversized replies.** Only a misbehaving provider can send these (`max_tokens` is 4,096).
  - Web: a reply over `max_message_bytes` (1 MiB) streams in full, then `store_reply` (`crates/seaquel-core/src/ai/turn.rs:334–341, 371`) is refused (`Storing the reply failed … code=INVALID_ARGUMENT`) and nothing is kept. After a reload the user's message stands alone. The page shows "Something went wrong: The message is longer than allowed here (max_message_bytes: 1048576 bytes)" (`ai/messages.ts:81`), which leaks an internal name.
  - Demo: a reply of 9 MB or more made of 1 MB lines is stored (16,777,158 bytes at the 16 MiB round cap, which ends the turn "The provider sent an answer Seaquel can't read.") but never shown. `marked` throws "Maximum call stack size exceeded" in `textBlock` (`ai-assistant.svelte:171`), live and after every reload.
  - Fix: Core stores the reply cut at the limit on a character boundary, marked `truncated`, and words the refusal. The page renders replies over a threshold (e.g. 256 KB) as plain text, or catches `marked`'s throw and falls back to plain text.
- **F3, Minor (parity, a note for Task 10): the schema context has table names only.**
  - Evidence: every engine's `schema_tables` returns `columns: vec![]` (e.g. `seaquel-engine-postgres/src/introspect.rs:168`, `seaquel-engine-duckdb/src/introspect.rs:282`). On a real connection the prompt is `Table: demo.customers` lines with no columns or indexes, in the demo and on Postgres. The TypeScript had columns only for tables whose schema tab had been opened, so this isn't a regression. Decision 13's 128 KB cap and the recorded prompts (scripted engine, with columns) assume columns, which real runs never send.
  - Fix: either say so in Task 10's docs, or fill columns in one catalog query per turn, which fits the "schema cache" follow-up.

**As designed, noted:**
- A turn's error notice (`TOOL_LIMIT`, `PROVIDER_ERROR`, `TIMEOUT`) isn't stored, so after a reload only the streamed text shows (Decision 31).
- A call stopped at approval or while running leaves no tool line after the turn's re-read (Decision 23).

**Found on the way, not phase 6:**
- **F4, Important: web keeps every closed tab's database connections.**
  - Evidence: each page load connects a new Core connection, and the old ones stay in the workspace until eviction. Measured 6 Postgres backends per load (0 → 6 → 12 → … → 30 after 5 loads of one user).
  - Impact: under the 16-per-user cap, one user's ~16 reloads can hold 96 backends, enough to exhaust a default Postgres (`max_connections` 100). The probe hit "too many clients" twice. After that every connect, for any user, sits at "Connecting…" instead of failing.
  - Fix: close a page's connections when its socket closes, or at `pagehide` with a keepalive `db.disconnect`. Give the pools a short idle timeout and the connect an acquire timeout.
- **F5, Minor: web auto-reconnect fails for a saved connection with no stored password.**
  - Evidence: the connection was saved with `savePassword` on (the wizard's default) and an empty password (trust auth). Its auto-reconnect (a saved target) answers `NO_SECRET_STORE` as HTTP 500 and opens the reconnect form every time.
  - Fix: on a Core with no secret store, a saved row's flags shouldn't require a stored secret (the vault supplies what it has). Map `NO_SECRET_STORE` to a 4xx in `status_for`.

**Not run:**
- The desktop app in `tauri dev` (item 10, above).
- The demo has no chat cap or eviction, since it runs without limits and has one workspace.

**Clean-up:** every server, mock and browser the probe started is stopped. The `p6_probe` database and its 5,000-table `wide` schema are dropped. The containers were left running.

### Probe fixes

2026-10-02, ~0.9 h wall. Test first: every new or changed test below failed before its fix.

- **F1 (the toast over Stop).** `Vault.ensureUnlocked({ quiet })` marks a waiter, and `unlock` answers `{ announce }`: false only when every waiter was quiet. `VaultKeyringService.getAIApiKeyForProvider(id, { quiet })` and the `AiKeyVault` interface pass it through; `CoreAi` asks quietly for `chat` only (`generate`, `models` and `test` keep the toast). The unlock dialog toasts "Vault unlocked" only when `announce`. The toaster stays where it is. Tests: `vault-state.svelte.test.ts` (a quiet wait isn't announced; a quiet and a plain one are) and `core-ai.test.ts` (a send asks quietly, the other calls don't); all three failed first.
- **F2 (oversized replies).** Decision 34. Core: `reply_cap`, `prefix_within`, `Turn::text_cap` in `ai/turn.rs`. Page: `ai/reply.ts` (`REPLY_CUT_NOTE`, `splitCutNote`, `isPlainReply`, `replyHtml`); a reply over 64 KiB (`PLAIN_REPLY_BYTES`, UTF-8 bytes) is shown whole as plain text without `marked` or the SQL-block split, and a segment `marked` throws on falls back to plain text. `aiErrorText`'s fallback no longer echoes Core's message: an unknown code reads `ai_error_generic` ("Something went wrong ({code}). Try again."), so `max_message_bytes` can't reach the user. Tests, all failing first: `ai_turn.rs` `a_reply_past_the_message_limit_is_cut_stored_and_ends_too_long` (a 4,000-byte limit, an endless stream of 3- and 2-byte characters: `done tooLong`, the stored row within the limit with the note, the streamed text equal to what was kept, the mock sees the client go) and `without_a_limit_a_reply_is_cut_at_one_mib` (the default ceiling: it ended `PROVIDER_ERROR` before); `reply.test.ts` (the note against Rust's, the stored row read and written back, Markdown up to 64 KiB, plain past it with `marked` never called, plain when `marked` throws); `turn.svelte.test.ts` (a `tooLong` reply shows `cut` without the note); `messages.test.ts` (the generic fallback, no limit name).
  - **Changed on purpose:** `a_reply_too_large_to_store_is_left_out_of_the_error` pinned the old behaviour (a 40-byte limit: the reply streamed whole and failed to store). It is now `a_reply_past_a_tiny_limit_is_cut_without_its_note` (cut at 40 bytes, `done tooLong`, stored). `turn.svelte.test.ts`'s thrown-stream case now expects "Something went wrong (UNKNOWN). Try again.". No JSON fixture changed: every replay (`ai_replay.rs`, `page-replay`, `ts_baseline.rs`) passed unchanged.
- **F5 (`NO_SECRET_STORE` on web).** `Workspace::plan` no longer treats a `NO_SECRET_STORE` read as unreadable: on a workspace with no store, a saved row whose save flag is on connects with what was supplied (here nothing), as a form does; the builder still words a missing SSH password itself, and web refuses SSH earlier anyway. `unreadable_error`'s `NO_SECRET_STORE` branch went (dead). `status_for` maps `NO_SECRET_STORE` to 400 for any read that still answers it. Tests, failing first: Core `connect.rs` `no_secret_store_connects_a_saved_row_with_no_password` (replaces `no_secret_store_fails_only_for_rows_that_need_a_secret`: the saved row and the same form fail alike at the driver), server `rpc_db_policy.rs` `a_saved_row_with_no_stored_password_connects_without_one` (`connect` and `test` reach the driver, `CONNECTION_ERROR` 502, was 500), and the `status_for` unit test. **Changed on purpose:** MCP's `a_workspace_without_a_secret_store_is_a_tool_error` is now `…_connects_with_no_password` and expects `CONNECTION_ERROR`; the CLI always has the keychain, so MCP's real surface (Decision 20) doesn't change.
- **F3** is a note for Task 10 and a Follow-up, no code.
- **i18n:** `ai_error_generic` replaces `ai_error_other`; `ai_reply_too_long` is new. Both written in all six locales by hand (no agent).
- **Runs:** `seaquel-ai`, Core (`ai`, `ai-native`, `storage`, `secrets`, `ssh`, `workspace`), `seaquel-rpc`, `seaquel-server`, `seaquel-mcp`, `seaquel-workspace`, `seaquel` (desktop): all pass. Clippy: the workspace line, `seaquel`/CLI/server/MCP, wasm32 `seaquel-ai`, wasm32 Core + rpc with `browser,storage,workspace,ai`, wasm32 `seaquel-browser` with `test-hooks`. fmt, `crates:check`, `wasm:build:browser-test` (1,608.5 KB brotli), vitest 2,049 in 136 files, `npm run check` 0/0, oxlint clean. The Svelte autofixer on the two changed components reports only the existing, sanitized `{@html}`.

- **F4 (closed tabs' connections; the owner chose to fix it here).** 2026-10-02, ~1.3 h wall. Test first: every new test below failed before its fix, except the two noted.
  - **Ownership by window.** Core records the write origin of the call that opened a connection (`ConnectRequest::origin`, `with_origin`; `seaquel-rpc`'s `db.connect` passes `dispatch_workspace`'s origin) on the connection, with a Core-wide opening order. A connection opened with no origin (the CLI, MCP, the engine tests) belongs to no window, and nothing below touches it.
  - **Replace on reconnect** (every interface; it only matters across a reload, since the GUI's own reconnect disconnects first). When a window connects a saved connection (a saved target, a form with `savedConnectionId`, or `db.bindSaved`, now async) and the connect succeeds, Core closes that window's connections for the same saved id that opened *before* the new one, cancelling their streams as `disconnect` does (`CONNECTION_CLOSED`), and announces each as `connectionClosed` `CONNECTION_REPLACED`. A failed connect leaves them; another window's are never touched. Only older ones: of two overlapping connects, the later replace can't close the newer connection, so one survives instead of none. Under `per_workspace` the connections a connect will replace don't count against the cap, so a window at 16 can reload (netted across connects in flight since the review, M2).
  - **Reaping closed windows (web only).** `Workspaces::hold_window(core, user, origin)`, held by each `/rpc/stream` socket with an origin for its life. When a window's last socket closes, a timer of `WINDOW_GRACE` (2 minutes then, 10 since the review; `Workspaces::with_window_grace` for tests) starts; a socket of that window opening meanwhile, or another close, makes it stale (a Core-wide generation number). At the end it calls `Workspace::close_owned_by(core, origin)` on the user's workspace if it is open (it never opens one, and an evicted one closed everything already): every connection of that window is closed, streams cancelled, each announced as `WINDOW_CLOSED`, which only the user's other sockets hear. A socket without an origin keeps nothing; one user's window can't reap another's.
  - **Connect timeout.** `CONNECT_TIMEOUT` (30 s; `CoreBuilder::connect_timeout` for tests) bounds `engine.open_with` in `Core::connect_as` and `Core::test` and the SSH tunnel's open in `Workspace::connect`/`test`, raced against the executor's `sleep` (no `tokio::time` in Core; a Core without an executor has no limit, and every interface has one). Past it the connect fails with `TIMEOUT` (504 on web). Pools: the sqlx engines set `POOL_IDLE_TIMEOUT` (10 min) and `POOL_ACQUIRE_TIMEOUT` (30 s) explicitly (`seaquel-engine`); both are sqlx's defaults, so idle pooled backends were already released after 10 minutes. MSSQL keeps its own 30 s TCP and login timeouts inside Core's.
  - **GUI.** `handleConnectionClosed` marks a connection closed with `WINDOW_CLOSED` or `CONNECTION_REPLACED` disconnected without a toast (`QUIET_CLOSE_CODES`); one for a connection the page doesn't hold was already ignored. No i18n keys.
  - **Tests.** Core `window_connections.rs` (9): same-window reconnect closes the old (its stream ends `CONNECTION_CLOSED`, `CONNECTION_REPLACED` announced), five reloads of two saved rows leave two, a form with `savedConnectionId` replaces, `bindSaved` replaces, a replace never closes a newer connection, a replaced connection doesn't count against `per_workspace`, `close_owned_by` closes only that window's (2 of 4; `WINDOW_CLOSED`), and Postgres, MySQL and SQL Server against a local listener that accepts and never answers fail `connect` and `test` with `TIMEOUT` at a 300 ms limit (they hung past the test's 10 s guard before). Two passed at once, pinning what was already right: another window's and origin-less connections stay, and a failed reconnect keeps the old one. Server `rpc_windows.rs` (5): a closed window's connections close after a 300 ms grace and the other tab hears `WINDOW_CLOSED` (failed first: no frame), a window back within the grace keeps them (also with a second socket of the window), an origin-less socket or another user's same-named window reaps nothing, five reloads leave one connection per saved id, and live Postgres: `pg_stat_activity` backends under a unique `application_name` stay at one load's count (6) after six loads and drop to 0 once the tab is gone past the grace (failed first with 6 left; the reload half already passed, since Core's replace was in by then, as was the fake-engine reload test when it first ran). GUI `connection-manager.svelte.test.ts` 2 (both codes quiet; failed first with a toast). Engines: the Postgres and MySQL pool tests pin the idle and acquire timeouts (passed at once: sqlx's defaults).
  - **Changed on purpose:** `seaquel-rpc` `ai.rs` `bind_saved_records_a_connection_opened_before_its_row` connected the saved row from the same window and then ran a turn on the bound form connection; that connection is now replaced, so the turn runs first and the test then checks the bound connection is gone (`CONNECTION_NOT_FOUND`). `close_all` now closes through the same helper (its event and message unchanged; a failed driver close logs `activity = "workspace.close"`).
  - **Also fixed:** `fetch-bridge.test.ts`'s `abortAll` test left its `head` promise unhandled until after the abort rejected it (vitest's "1 unhandled error", every run); it now attaches the expectation first.
  - **Runs:** the workspace with the CI features and live Postgres, MySQL, MariaDB, SQL Server and SSH: 2,239 passed, 3 ignored in 210 targets; the only failures were the changed `bind_saved` test (now passing) and `seaquel-engine-mssql`'s `tls_server_name` (3), which can't read the platform trust store inside the sandbox and pass outside it. Desktop `seaquel` 50. Clippy: the workspace line, `seaquel`/CLI/server/MCP, and all nine wasm32 lines; fmt, `crates:check`, `wasm:build:browser-test` (1,610.4 KB brotli), vitest 2,051 in 136 files with no errors, `npm run check` 0/0, oxlint clean.

- **F4 review fixes, and three items left from the F1/F2/F5 review.** 2026-10-02, ~0.75 h wall. Test first as before.
  - **I1 (a), `db.alive {connectionIds}`.** Answers which of the ids this workspace still holds, in the order asked (`Workspace::alive`); another workspace's, closed and unknown ids are left out; more than `MAX_ALIVE_IDS` (1,000) is `INVALID_ARGUMENT`. `ConnectionManager.listenForCoreEvents` registers `onResubscribed` before `events`; on every restart that isn't the first (`checkAlive`) it asks Core about the provider ids the page shows (nothing, when it shows none) and hands each missing one to `handleConnectionLost`: marked disconnected, then one quiet `autoReconnect`, with `connection_closed_lost` as a toast only if that fails.
  - **I1 (b), the central hook.** `$lib/core/connection-watch.ts`: `getCoreClient()` now returns `watchClient(client)` (one wrapper per client), which reports a `db` or `ai` call that rejects, or a stream that ends, with `CONNECTION_NOT_FOUND` to `onConnectionNotFound` handlers with the request's `connectionId` (`disconnect` and `alive` aren't reported). It never retries the call: the user runs it again. `ConnectionManager` listens and goes through `handleConnectionLost`; an id the page doesn't show is ignored.
  - **I1 (c).** `WINDOW_GRACE` is 10 minutes.
  - **I2.** `dispatch_workspace` boxes its `Db` and `Ai` arms (`Box::pin`), as it does the library. `-p seaquel-rpc --test state` overflowed its stack before (SIGABRT, no report); it now reports 18 passed, and so does the CI workspace run, `library_calls_fit_a_2_mib_stack` included.
  - **M1.** `autoReconnect` and `reconnect` are single-flight per connection id: a second call while one runs gets its promise.
  - **M2.** The cap's discount is netted under the `connecting` lock: each slot keeps the ids it will replace in a counted map, and an open connection any slot claims is discounted once.
  - **M5, noted (no change):** a page can claim another of the same user's tabs' window ids as its origin (`X-Seaquel-Origin` is checked for shape only). Then its connects replace that tab's connections for the same saved ids, and its socket keeps that tab's connections from being reaped. Both stay inside one user's own session, and the other tab gets its connection back through I1, so the impact is low.
  - **P1.** A user message (or the inline prompt's request or editor text) past `max_message_bytes` is `MESSAGE_TOO_LONG` (413 in `status_for`), worded by `aiErrorText` as `ai_error_message_too_long` ("Your message is too long to send."). The invalid provider URL keeps `INVALID_ARGUMENT`: nothing but the message tells it apart, so no case was added.
  - **P2.** On web, when the row saves its password and the vault read fails or its unlock is cancelled, `heldSecrets` throws and `autoReconnect` answers false without dialling. A vault that holds no password for the row (trust auth, F5) still connects.
  - **P3.** Core test only. It passed at once, pinning existing behaviour.
  - **Tests**, all failing first except as noted:
    - Core `window_connections.rs`:
      - `a_tab_at_the_cap_reconnects_several_saved_ids_in_parallel` (refused before);
      - `one_replaced_connection_is_discounted_once` (passed at once: the old per-connect discount also refused the second);
      - `alive_answers_only_this_workspaces_open_connections`.
    - Core `ai_turn.rs`: `a_tool_call_before_text_over_the_cap_runs_nothing` (P3: no tool events, `done tooLong`, no `parts`, one request; passed at once). The two message-cap assertions now expect `MESSAGE_TOO_LONG` (changed on purpose).
    - Server:
      - `rpc_windows.rs` `a_reaped_tabs_connections_arent_alive_and_reconnect` (the reaped id was answered as alive by the stub);
      - the `WINDOW_GRACE` unit test;
      - the `status_for` case.
    - GUI `connection-watch.test.ts` (4; 2 failed against a no-op stub, and the two that pass a call through or check that a client is wrapped once passed at once).
    - GUI `connection-manager.svelte.test.ts`:
      - a restart reconnects what Core lost, quietly;
      - a failed reconnect toasts;
      - nothing is asked or reconnected when the page holds nothing;
      - a `CONNECTION_NOT_FOUND` reconnects once and ignores unknown ids;
      - single flight for `autoReconnect` and for `reconnect`;
      - no dial when the vault read fails;
      - a vault with no password still connects (passed at once).
    - GUI `messages.test.ts`: `MESSAGE_TOO_LONG`.
  - **i18n:** `ai_error_message_too_long` (en only; to be translated).
  - **Runs:**
    - The workspace with the CI features and live engines: 2,263 passed, 3 ignored in 211 targets, all three `state` binaries reporting. The 4 failures were `tls_server_name` (3) and `connect.rs` `row_6_mssql_over_ssh…`, which can't read the platform trust store inside the sandbox; they pass outside it.
    - Desktop: 50 passed.
    - Clippy: every native line and all nine wasm32 lines are clean (the bare `browser` line needed `allow(dead_code)` on the `Connecting` struct without `workspace`).
    - fmt and `crates:check` pass. `wasm:build:browser-test` built (1,615.0 KB brotli).
    - vitest: 2,065 in 137 files, no errors. `npm run check` 0/0, oxlint clean.

- **F4 re-review fixes.** 2026-10-02, ~0.4 h wall, TypeScript only (no Rust touched). Test first.
  - **R1, a quiet reconnect never moves the active connection.** `handleConnectionLost`, `checkAlive` and the `CORE_RESTARTED` branch go through `reconnectQuietly`. It calls `markDisconnected(connection, {background: true})`, which only clears the Core id: the active connection doesn't move and the schema tabs stay. It then calls `autoReconnect(id, {background: true})`, whose `connectExisting(…, background)` neither calls `setActiveForProject` nor opens an initial tab. A lost active connection stays active, shown disconnected until it's back.
  - **M-b, schema tabs.** Kept during a background reconnect (`closeSchemaTabs` split out of `markDisconnected`). If the reconnect fails, they close then and the failure toast shows; the active connection still doesn't move.
  - **M-c, a disconnect during a quiet reconnect.** `toggle` on a connection whose auto-reconnect is in flight sets `userDisconnected`. A background `connectExisting` checks it after the connect lands and after the schema load, then closes the new connection and gives up without a toast.
  - **M-a, an undecryptable vault entry.** `VaultKeyringService.getDbPasswordStrict` throws `VaultEntryUnreadableError` for a stored password it can't decrypt and answers null only when none is stored (`getDbPassword` keeps answering null for both). `heldSecrets` uses the strict getter where the keyring has it, so an unreadable entry returns false without dialling, while trust auth (no row) still connects.
  - **Tests**, all failing first:
    - `connection-manager.svelte.test.ts` "background reconnects":
      - an inactive connection reported lost reconnects without becoming active (it became active);
      - after a sleep, reconnecting both keeps the active one, and it waits disconnected meanwhile (active moved, then went null);
      - schema tabs stay while reconnecting and close only on failure (they closed at once, and the failure toasted twice);
      - a disconnect during the reconnect wins (the late connection stayed open);
      - an undecryptable password isn't dialled (it connected with none).
    - `vault-keyring.test.ts` (new, 3; the strict getter didn't exist).
    - Changed on purpose: the three earlier assertions now expect `autoReconnect` called with `{background: true}`.
  - **i18n:** no new key this round. `connection_closed_lost` was already in `en.json` at HEAD and is only reused. Across F4 and its reviews the one key added is `ai_error_message_too_long`.
  - **Runs:** vitest 2,073 in 138 files, no errors; `npm run check` 0/0; oxlint clean.

## Task 10: Docs, measurement, checkpoints

- **CLAUDE.md:** the AI section (Core runs the assistant; keys; egress; the registry; the `ai` group; `CoreEvent::Ai`; new codes; `0007`), the MCP section (the registry), the desktop CSP line, the demo's assistant.
- **The design doc:** "As built in phase 6" under the MCP tool set and "Secrets", the phase 6 status line, the "AI on web" risk closed or restated, and a "Phase 6 cost" section from the effort log.
- **Measurement:** lines added and removed (Rust production and tests, TypeScript production and tests, fixtures), the module's size, the event count and first-delta time through each transport against the mock.
- **Release notes:** web operators' `SEAQUEL_AI_EGRESS`; MCP users open the app once after upgrading (`0007`); the inline prompt no longer runs what it generates (Q9); the demo's assistant (Q2).
- Checkpoint 6b: the full check list and the live run.

**Status (Task 10):** done but for Checkpoint 6b, which runs separately. CLAUDE.md: a Core section for the assistant (policies and egress, a turn's checks and events, the tools, approvals and client tools, keys, what is stored, prompt and history, stopping, the unary calls, logs), `seaquel-ai` and `seaquel-http` entries with their crate rules, the `ai` group and `StreamKind` in `seaquel-rpc`, `db.alive`/`db.bindSaved`, the `secret` group's refusal, `aiProviderHasKey`, `0007`, F4's window ownership, connect timeout and reaping, F5, the web statuses, `is_slow_call`, the proxy plan, the MCP section on the registry, the CLI's `0007`, `src-tauri` and the CSP, "Model-written SQL runs only read-only" in place of the `executeReadOnly` rule, new GUI sections ("The assistant in the GUI", "Connections lost and reconnected") and the demo's assistant. The design doc: the status line, "As built in phase 6" under "Secrets" and "MCP tool set", the migration plan's phase 6 entry, the phase 8 "No AI in the demo" note, the WASM size and "AI on web" risks, and "Phase 6 cost". This plan: this status, the release notes and the manual checks below. The effort log: Task 10's row and the totals.

### Notes for Task 10 (from Task 9's probe)

- **CLAUDE.md and the design doc, the schema context (F3):** it lists tables and views by schema-qualified name only, with no columns or indexes, because every engine's `schema_tables` returns `columns: vec![]`. That is parity with the TypeScript, which had columns only for tables whose schema tab had been opened. The model gets columns with `describe_table` (the prompt points at it). Decision 13's 128 KB cap rarely binds on names alone (5,002 tables made 100 KB).
- **CLAUDE.md, the assistant:** a reply is cut at the lower of the message limit and 1 MiB and ends `tooLong` with Core's note (Decision 34); web auto-reconnect of a saved row without a vault password connects with none (F5; `NO_SECRET_STORE` is 400).

### Notes for Task 10 (from Task 9's probe fix F4)

- **CLAUDE.md, Ownership (Core):** each connection records the window (write origin) whose call opened it (`ConnectRequest::origin`); none for the CLI, MCP and engine tests. A window's successful connect for a saved connection (saved target, form with `savedConnectionId`, `bindSaved`) closes that window's older connections for the same saved id (streams end `CONNECTION_CLOSED`; `connectionClosed` `CONNECTION_REPLACED`), never newer ones or another window's; those don't count against `per_workspace`. `Workspace::close_owned_by(core, origin)` closes a window's connections (`WINDOW_CLOSED`). The `WorkspaceEvent::ConnectionClosed` codes are now `WORKSPACE_EVICTED`, `WINDOW_CLOSED` and `CONNECTION_REPLACED` (`CONNECTION_CLOSED`/`TUNNEL_CLOSED` still reserved).
- **CLAUDE.md, `ConnectionLimits` / connect:** `CONNECT_TIMEOUT` (`CoreBuilder::connect_timeout`) bounds the opening of a connection through the executor's clock: 30 s each for the SSH open and the engine open, failing `TIMEOUT`; sqlx pools set `POOL_IDLE_TIMEOUT` (10 min) and `POOL_ACQUIRE_TIMEOUT` (30 s), sqlx's defaults, explicitly.
- **CLAUDE.md, web (`/rpc/stream` and eviction):** each socket with an origin holds its window (`Workspaces::hold_window`); when a window's last socket closes and none returns within `WINDOW_GRACE` (10 minutes), its connections in the open workspace are closed and the user's other sockets get `connectionClosed` `WINDOW_CLOSED`. A reload comes back within the grace and its reconnects replace the old connections. A tab whose socket was down past the grace (a sleeping laptop) misses that event, so on every event-channel restart the page asks `db.alive {connectionIds}` (only the workspace's own open ids come back) and quietly reconnects the missing ones. `getCoreClient()` wraps every client in `watchClient` (`$lib/core/connection-watch.ts`): any `db`/`ai` call or stream that fails with `CONNECTION_NOT_FOUND` reports its connection id to `onConnectionNotFound`, and `ConnectionManager` marks the connection disconnected and reconnects it once, quietly, never retrying the failed call. `autoReconnect` and `reconnect` are single-flight per connection. Same-user origin spoofing (M5) can only replace or keep that user's own tabs' connections.
- **CLAUDE.md, the GUI:** `handleConnectionClosed` shows `WINDOW_CLOSED` and `CONNECTION_REPLACED` as disconnected without a toast. Quiet reconnects (`CORE_RESTARTED`, `db.alive`, a `CONNECTION_NOT_FOUND`) run in the background: the active connection never moves, schema tabs stay unless the reconnect fails, and a user disconnect meanwhile closes the late connection. Web auto-reconnect reads the vault strictly (`getDbPasswordStrict`): an undecryptable entry isn't dialled. On web an auto-reconnect of a row that saves its password doesn't dial when the vault read fails or is cancelled (P2).
- **CLAUDE.md, the assistant:** a user message past `max_message_bytes` is `MESSAGE_TOO_LONG` (413), worded "Your message is too long to send." (P1).
- **CLAUDE.md, `seaquel-rpc`:** `dispatch_workspace` boxes the `db` and `ai` arms too (I2), and `db.alive` is in the `db` method list.

### Notes for Task 10 (from Task 5's review)

- **CLAUDE.md, the web body limits:** `MAX_EDIT_CALLS_PER_USER` (4) now counts `ai.generate`, `ai.models` and `ai.test` with the edit calls (`is_slow_call` in `routes/rpc.rs`; I1). The sentence "at most 4 of them are `db.applyChanges`, `db.planEdits` or `db.duckdbExtension`" needs the three AI calls.
- **CLAUDE.md, "Features aren't a security boundary":** add that under `AiEgress::Public` the model client makes every proxy decision itself (`ClientOptions::env_proxies_only`, `proxy_plan` in `seaquel-http/src/client.rs`): the environment's proxies with `NO_PROXY`, or none, and reqwest's automatic proxies off, so `system-proxy` unified into a server build (reqwest's OS proxy, which the egress guard can't see) can never take effect (M2). Under `Any` (desktop) reqwest's automatic proxies, the OS's included, stay.

---

## Manual checks

For the owner, with real keys, after Checkpoint 6b. Items 13–15 need no key.

**Setup.** Desktop: `npm run tauri dev`. Web: `npm run dev:web:full`, once as is (`SEAQUEL_AI_EGRESS` unset means `public`) and once with `SEAQUEL_AI_EGRESS=off` exported before the command (the Rust service reads it at startup; restart to change it). Demo: `npm run dev:demo`. MCP: `npm run cli:build`, then point Claude Desktop at the binary as Settings → MCP shows it. For item 12, the desktop log is the app's log viewer, the server's is the `dev:web:full` terminal.

1. **Desktop, Anthropic:** add a provider with a key, pick a model, ask a question that needs a query on a Postgres connection with data sharing on; approve; the answer uses the rows; the tool line shows the SQL and row count. Settle bug 1 by trying the same on the last release first.
2. **Desktop, OpenAI-compatible:** the same against OpenAI or a local Ollama at `http://localhost:11434/v1`.
3. **Approvals:** deny one; "Allow all" on one connection, then a chat on another still asks.
4. **Stop** during streaming, during an approval and during a long query; the reply keeps what streamed.
5. **Sharing:** schema sharing off: no schema in replies, `@table` gives only the name; data sharing off: the model says it can't query.
6. **Dashboards:** "make a dashboard of orders by month" builds one; switching the active connection mid-turn refuses widget changes as today.
7. **Inline prompt:** inserts, doesn't run (Q9).
8. **A follow-up** ("what was the second row?") answered from the stored tool result (Q7).
9. **Web:** the same with the vault unlocked; `SEAQUEL_AI_EGRESS=off` shows the operator message; a base URL of `http://127.0.0.1:…` is refused under `public`.
10. **MCP:** Claude Desktop with `seaquel-cli mcp` lists and queries as before (after opening the app once).
11. **Demo** (Q2): a session key works for one session and is gone after a reload.
12. **Logs:** the desktop log and the server's stderr hold no prompt, SQL or key after the checks above.
13. **Desktop with a local mock (Task 7's one `tauri dev` run, no real key).** Start an OpenAI-compatible mock on `127.0.0.1:8787` (it lists one model and answers every chat with one streamed sentence):

    ```bash
    node -e 'require("http").createServer((q,r)=>{if(q.url.endsWith("/models")){r.writeHead(200,{"content-type":"application/json"});return r.end(JSON.stringify({data:[{id:"mock-1"}]}))}let b="";q.on("data",c=>b+=c);q.on("end",()=>{r.writeHead(200,{"content-type":"text/event-stream"});const s=t=>r.write("data: "+JSON.stringify({choices:[{index:0,delta:{content:t}}]})+"\n\n");s("Hello ");s("from the mock.");r.write("data: "+JSON.stringify({choices:[{index:0,delta:{},finish_reason:"stop"}]})+"\n\ndata: [DONE]\n\n");r.end()})}).listen(8787,"127.0.0.1",()=>console.log("mock on http://127.0.0.1:8787/v1"))'
    ```

    Then `npm run tauri dev`; Settings → AI → Add provider: type OpenAI-compatible, base URL `http://127.0.0.1:8787/v1`, any or no key (`test-key-not-real`). Test answers "Connection successful", the model switcher lists `mock-1`; pick it on a connection, open the assistant and send a message: "Hello from the mock." streams and survives a reload. The inline prompt (⌘K in the editor) inserts the reply and says "Inserted. Run it with ⌘↵.", running nothing. The DuckDB extensions tab's community list still loads, and the dev server's HMR still reloads the page after a `.svelte` edit (the CSP).
14. **Web, closed tabs (F4).** With a Postgres connection open in one tab, run `SELECT count(*) FROM pg_stat_activity WHERE datname = '<db>'` from `psql`. Reload the tab five times: the count stays at one load's (up to 6), no toast. Open a second tab, close the first, wait 10 minutes: the first tab's backends are gone, and the second tab shows nothing. Put the laptop to sleep with a tab open for more than 10 minutes, wake it and run a query: the connection comes back by itself (no toast, the active connection unchanged), and the query runs on the second try at most.
15. **Connect timeout.** Add a Postgres connection to a host that drops packets (e.g. `10.255.255.1`, port 5432) and connect: it fails with a timeout after about 30 seconds, on desktop and web, instead of hanging at "Connecting…".

---

## Release notes

For the release that ships phase 6. Earlier notes still apply as written.

**The AI assistant**

- **The assistant now runs inside Seaquel's engine** on the desktop app, the web app and the demo, instead of in the page. You'll see the same answers, with some differences:
  - When the model runs a query, the reply shows a line for it (the tool, its SQL, and how many rows came back or why it failed). Follow-up questions can use what earlier queries returned, because those results are now kept with the chat.
  - The model can also look up the database itself: list schemas and tables, describe a table, explain a query, and list or run saved queries. Anything that reads data still asks for your approval first.
  - When the model asks for several queries at once, all of them run; before, only one did. Provider errors, cut-off replies and rate limits are now shown as such instead of ending the reply silently.
  - It sees up to 100 rows of a query result by default (at most 1,000, and about 256 KB), where it used to see only the first 5.
  - The assistant's queries now stop after 60 seconds and fetch at most 8 MB, as the MCP server's do.
  - A reply longer than 1 MB is cut and says so.
- **"Allow all" now applies to one connection.** Ticking it on the approval card lets the assistant run queries without asking on that connection for the rest of the session; a chat on another connection still asks. It's forgotten if the connection is removed or pointed at another database.
- **The inline SQL prompt (⌘K) no longer runs what it generates.** It inserts the SQL at the cursor and says "Inserted. Run it with ⌘↵." Read it, then run it yourself.
- **With schema sharing off, `@table` mentions send only the name** to the model, not the table's columns, and the mention list follows the setting.
- **Desktop: your API keys stay in the keychain.** The page no longer reads them; Seaquel's engine reads a key only when it calls the provider.
- **The demo has the assistant again.** Enter your own API key in Settings → AI; it stays in the page's memory for that visit only, and is gone when you reload or close the tab. It is never saved, not even in the demo's browser storage. An OpenAI-compatible server has to allow requests from the demo's address (CORS); the settings form says so.

**For web operators**

- **Model calls now leave from the server, not the browser.** The page sends the provider key, decrypted from the user's vault, with each request; the server holds it in memory for that request only, as it does database passwords. A model server running on a user's own laptop is no longer reachable from the web app.
- **`SEAQUEL_AI_EGRESS`** controls where those calls may go:
  - `public` (the default): `https://` to public addresses only. Private, loopback and link-local addresses are refused whether written as an IP or reached through DNS, and redirects are never followed.
  - `any`: anywhere, plain `http://` included, for a model server on your own network (an Ollama next to the server).
  - `off`: no model calls; the assistant tells users the server doesn't allow them. Use this for air-gapped installs.
  
  Any other value stops the server at startup. Behind an HTTP proxy (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`), `public` still refuses private IP literals and names that resolve only to private addresses from the server, but a name the server can't resolve is sent to the proxy, which then decides what it can reach. The model client trusts the certificates in `NODE_EXTRA_CA_CERTS`.
- Each user can have 4 assistant replies in progress at once, and a message to the assistant may be up to 1 MB.

**Web connections**

- **Closed tabs release their database connections.** Before, every page load opened a new set of connections and kept the old ones until the server unloaded the user's workspace, so frequent reloads could run a database out of connections. A reload now replaces the tab's previous connections, and a tab that's been closed for 10 minutes has its connections closed. A tab that comes back after that (a laptop waking from sleep) reconnects on its own.
- **Connecting gives up after 30 seconds** with a timeout, instead of hanging at "Connecting…" when a database host doesn't answer. This applies on the desktop too.
- **A saved connection with no password** (trust authentication) reconnects automatically again; it used to fail and open the reconnect form every time.

**MCP server**

- **Open the Seaquel app once after upgrading** before using `seaquel-cli mcp` again. This release adds a column to the app's database (for the assistant's tool calls), and the MCP server, which only reads that file, refuses it with `STORAGE_NEEDS_UPGRADE` until the app has updated it. The tools themselves haven't changed.

Known issues:

- The model sees table names only in its starting context, not their columns; it looks columns up with `describe_table` when it needs them.
- If a reply fails partway (a provider error, a timeout, too many tool calls), the error shows under the reply, but after a reload only the text that streamed is there.
- In the demo, reloading during a reply loses that reply (your message stays).
- `seaquel-cli mcp --log-level debug` writes each tool call's SQL to stderr (a library's own debug line).

---

## Follow-ups (not in phase 6)

- Arabic plurals for `ai_tool_rows` and `ai_tool_rows_cut` have only `one`/`other`; add `two`, `few` and `many` (Task 7 re-review).
- `connectionCreate` should take the open connection the page just connected and bind it itself, so `db.bindSaved` (Task 7) and its trusted claim go (Task 7 review M6).
- More providers (Q3's option B), configurable `max_tokens` per provider, and Anthropic prompt caching on the system prompt and schema context.
- MCP's dashboard tools and write tools (Q5's options B and C), once phase 7 makes the CLI's storage writable.
- `seaquel-cli ask` (Q6's option B), in phase 7.
- A read-only database login for AI queries (the AI safety Follow-up; the only full fix on SQL Server).
- A schema cache in Core per connection, so a turn doesn't introspect each time.
- Fill the schema context's columns from that cache, in one catalog query per connection rather than one `table_metadata` per table (probe F3: today the context lists table names only, and Decision 13's cap assumes columns).
- Token usage shown per chat, from the `usage` numbers Core already logs.
- `seaquel-cli`'s log filter should hold `rmcp` at WARN whatever `--log-level` says: at DEBUG rmcp's "received request" line writes each tool call's arguments, SQL included, to stderr (found at Checkpoint 6a; older than phase 6). **Closed in phase 7a (Task 2 review, Decision 17):** `seaquel-cli` builds its filter with `seaquel_terminal::log_filter_holding(level, &["rmcp"])` (`mcp.rs`, test `rmcp_is_held_at_warn`).
- A graceful shutdown for `seaquel-server` (`main.rs`'s `axum::serve` has none): on SIGTERM, cancel every turn, wait briefly for their reply writes, then exit. Today a SIGTERM loses the unfinished reply of each turn in flight (the user's message is already stored), which Q8 accepts.
