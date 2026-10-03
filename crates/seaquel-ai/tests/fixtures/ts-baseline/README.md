# ts-baseline fixtures

These files record what today's TypeScript assistant does before phase 6 moves it into Rust: the requests it sends to Anthropic's Messages API and to OpenAI-compatible Chat Completions, the system prompt and schema context, what each tool returns to the model, `@mention` expansion, how a turn ends for each kind of provider stream, what the page shows, the inline prompt (`generateSQL`), and the model list and provider test. They pin `seaquel-ai`'s wire, prompt and tool renderers (Tasks 2 and 3) and Core's turn (Task 4). See `docs/plans/2026-10-08-rust-core-phase-6-plan.md`, "Parity", "What the code shows" (the numbered bugs) and Decisions 3–32 (22–32 were added in Task 1's review).

**The fixtures are frozen.** Change a case only when Rust is meant to behave differently, say why in `changes.json`, and never re-record to make a failing test pass.

## How they were made

The recorder is `docs/plans/artifacts/2026-10-08-record-ai-baseline.test.ts.txt`, a vitest file. It ran on `ef5014a` (Clean up), the phase 6 plan's survey commit, while Task 2's Rust changes were in progress in the same tree; they touch no TypeScript, so the recorded code is `ef5014a`'s. To rerun it, copy it to `src/lib/services/ai/record-ai-baseline.test.ts`, run it with `FREEZE_AI=1` (and `SEAQUEL_WASM_PREBUILT=1` if `src/lib/wasm/pkg/` is current), and delete the copy. `FREEZE_AI_OUT=<dir>` writes somewhere else. It needs a tree that still has the TypeScript assistant, so before Task 7.

The first recording was checked by three runs (two into scratch directories and one from the artifact), all byte-identical. The review's re-recording was checked by two runs, byte-identical, and every case of the first recording is unchanged byte for byte except `page/allow-all-other-connection`, whose second step now scripts an `allow` (TypeScript never uses it; Rust asks). The re-review's re-recording was checked by two runs, byte-identical; it changed only the 11 cases whose inputs changed (distinctive messages, requests and replies for the log checks, `page/history-skips-pending`'s seed, the reply marker) and added `page/no-api-key` and `tool-results/tool/assistant/explain-query-large`.

It runs the real code: `sendAIMessage`, `handleToolCall`, `generateSQL`, `runAndFormat`, `buildSchemaContext`, `buildSystemPrompt`, `handleDashboardToolCall`, `resolveMentions`, both providers' streaming and non-streaming calls, `UIStateManager` (mentions, the dashboard-id line, the approval card, the error wording) over a real `DatabaseState` and `AIChatManager` (with the `RecordingLibrary`), `createAIInlinePrompt`, `aiSettingsStore.fetchModels`/`testConnection`, and `collectReadOnly` (how a read-only stream's batch reaches the tool, column dedupe included).

Stubbed:

- **`fetch`.** The stub records each request and forwards it to a mock HTTP server on `127.0.0.1`, which answers the case's scripted response in 7-byte pieces, so lines, JSON and multi-byte characters (`é`, `☕`) straddle reads. Nothing leaves the machine: the provider URLs (`https://api.anthropic.com/v1/…`, `https://api.openai.com/v1/…`, `http://localhost:11434/v1/…`) are only recorded. A `networkError` script makes the stub throw `TypeError("fetch failed")` instead.
- **Keys.** The keychain answers `test-key-not-real` for `prov-1` when the case has a key. Headers record it as `<key>`; the recorder fails if the string appears anywhere in its output, and its first test checks that it would. No real key was used and no provider was called.
- **The read-only runner** answers each SQL with the case's batch (`answers`, else `{"columns":["n"],"rows":[[1]]}`) in the wire format, decoded and passed through the real `collectReadOnly`, or with the case's error or a cancel.
- Dashboard callbacks (recorded), the logger (captured), toasts (captured), `crypto.randomUUID` (a counter).

Not recorded: timing (how fast deltas arrive, when a stall times out; the TS has no timeouts, bug 11), real providers' behaviour (CORS and bug 1 among it), the keychain and the vault, and the GUI's rendering.

## Files

| file | cases | covers |
| --- | --- | --- |
| `turns.json` | 73 | `sendAIMessage` with scripted streams, both providers: no tools; schema sharing on, data on; dashboard tools; history; one tool call (every round); approval allowed, denied, Stop during it; the read-only refusal; a query error; an unknown tool; tool input that isn't JSON, and JSON that isn't an object; two tool calls in one round (bug 3: Anthropic runs the last, OpenAI index 0), with and without approvals; three rounds; 21 calls (the limit); an `error` event mid-stream (bug 4: a silent end); `max_tokens`/`length` (bug 5); 429; 500 with a JSON body and with a 1,400-byte message; 502 with text; a malformed event; a stream cut before its end marker; `fetch` failing; Stop mid-stream (the mock sees the close); `[DONE]` early (OpenAI); keyless and custom-base-URL OpenAI; no key; no provider; each engine; a dashboard built with two client tools; a widget whose query writes; `run_query` called with data sharing off; markers in the prompt, a schema name and a cell |
| `page.json` | 21 | `UIStateManager.sendAIMessage`: table, query and dashboard mentions with schema sharing from the row; a mention with schema sharing off (bug 8); the dashboard-id line after the mention block; a pending-model message left out of history; follow-ups after a tool turn (Anthropic and OpenAI, a failed call, a result over 16 KiB, a reply over three rounds); the approval card (deny, then allow); "Allow all" mid-turn and across connections (bug 10); data sharing turned off mid-turn; no model; the chat's connection removed; a 500 shown as `Error: <body>` (bug 12); an `error` event after partial text; Stop mid-stream and during an approval; a dashboard created by a tool; a provider without a key (Core's refusal, nothing stored) |
| `prompts.json` | 28 | `buildSystemPrompt` for each engine, with and without the schema context and the dashboard guidelines; `buildSchemaContext` empty, over `SCHEMA` (indexes, nullable columns, a view, two `users` tables, a table without columns), with `indexes` missing, and over 3,000 generated tables (recorded as its size, SHA-256, first and last 200 characters) |
| `mentions.json` | 19 | `resolveMentions`: qualified, name only, ambiguous, case, a view, quoted and unquoted queries, a table over a query, dashboards with and without widgets, duplicates, unknown, trailing punctuation, an e-mail address, none, several, and three with schema sharing off (bug 8) |
| `tool-results.json` | 66 | `runAndFormat` for empty, small, exactly 5, 6, truncated (1,000 of more), bytes, bigint, decimal, JSON object (bug 6) and array, NaN and infinities, `|` and a newline, NULL/empty/bool/float, duplicate column names, Unicode, a query error, Core's read-only refusal, Stop before and during, 100 rows of 3 KB (the 256 KB budget) and a 70,000-byte cell; `handleToolCall` for `run_query`'s refusals (a write, a second statement, missing, non-string and unknown arguments, denied), `max_rows` 5 and 1,001, data sharing off, an unknown tool, every dashboard tool's success and each refusal, and the tools today's TypeScript doesn't have (`explain_query`, also with a plan past 256 KB, `list_schemas`, `list_tables`, `describe_table`, `list_saved_queries`, `run_saved_query`; recorded as "Unknown tool", their Rust results in `changes.json`) |
| `errors.json` | 8 | `_formatAIError` for each error string the turn can end with |
| `generate.json` | 21 | the inline prompt: with and without an existing query, a reply without a fence, two fences, schema off, 429, 500, `fetch` failing, empty content, no key, keyless, no model, no providers (it inserts and then runs the tab: bug 9) |
| `models.json` | 18 | `fetchModels` and `testConnection`: Anthropic with and without a key and refused; OpenAI-compatible with a key, keyless, without a base URL (no request), a 200 that isn't JSON, `fetch` failing |
| `history.json` | 3 | **Rust only** (no TypeScript counterpart: today's history is text only and has no budget): Decision 29's budget over generated stored rows |
| `inputs.json` | | `SCHEMA`, `SAVED`, `DASHBOARDS` and `LARGE`, which cases name by those strings |

257 cases in all, plus `changes.json` (225 entries: `*` and 224 cases).

### Shared inputs

- `SCHEMA`, `SAVED`, `DASHBOARDS` are in `inputs.json`; a case's `"schema": "SCHEMA"` means that value.
- `LARGE` is generated: tables `t_0001` … `t_3000` in schema `big`, each with columns `c1` … `c6` of type `integer`, `c<j>` nullable when `j` is even (`NOT NULL` otherwise), no indexes, `type` `table`, in that order.
- `history.json`'s `generate`: for each turn `{tag, user, rounds, resultBytes, finalText}`, a user row `{id: "<tag>-user", role: "user", content: user}` and a reply `{id: "<tag>-reply", role: "assistant"}` whose `parts` are, for each round `r` below `rounds`, `{round: r, type: "text", text: "Round <r>. "}` and `{round: r, type: "tool", callId: "<tag>_<r>", name: "run_query", input: {sql: "SELECT <r>"}, ok: true, result: "x" × resultBytes}`, then `{round: rounds, type: "text", text: finalText}`; its `content` is the texts joined. Turns are oldest first.
- A page case's seeded `history` is what the page shows; Core has stored only the rows a completed send wrote, so a pending-model row and the user message before it are not in Core's history or `stored` (`page/history-skips-pending`, Decision 31). Each connection has its own chat: after an `activate` step, history and `stored` are the new chat's.
- Tool cases' `rustInputs` are what the database and storage answer the Rust tools (the schemas list, the EXPLAIN plan, the project's name); their `answers` are the query results by SQL.

## A case

| field | meaning |
| --- | --- |
| `name`, `note` | what the case is |
| `input` | what the code was given: provider (`prov-1`, its `type` and `baseUrl`), whether it has a key, the model (`model-1`), the engine, the messages, the sharing flags, `dashboards` (callbacks passed), `allowAll`, scripted approvals, query answers. `page.json` cases have the connections (with `aiShareSchema`/`aiShareData` when set; the global defaults are schema on, data off), the active connection, seeded history, steps (`approvals` in order, `stop` pressing Stop at a card, `stopAtFirstChunk`), `provider` when not Anthropic, and `dataOffAfterFirstQuery` |
| `responses` | the scripted provider responses, in order: `body` (the exact bytes, SSE or JSON), `status` (200 when absent), `contentType`, `stall` (kept open until the client closes) or `networkError` |
| `requests` | each request the code made: `method`, `url`, `headers` (the ones the code set, lowercased, sorted; the key as `<key>`) and `body` (parsed JSON; `null` for a GET) |
| `chunks`, `text` | the deltas `onChunk` got, and their concatenation |
| `queries` | each `runQuery` call: the SQL and `maxRows` |
| `approvals` | each approval request: the SQL (and, in `page.json`, the connection name the card shows) |
| `outcome` | how `sendAIMessage` ended: `done` (`onDone`), `error` (`onError(error)`), `cancelled` (returned after Stop) or `threw` (rejected; the page then shows `Error: <message>`) |
| `providerClosed` | for a stalled response: whether the client closed it (Stop drops the request) |
| `dashboardCalls`, `calls` | the dashboard callbacks the tool called, with their arguments |
| `result`, `isError` | a tool's result as the model gets it; `isError` is always false today (bug 13) |
| `messages`, `stored`, `storeCalls`, `allowAllAfter` | `page.json`: the chat after the steps (`pendingApproval` is the card's SQL), what the last save held and how many saves there were, and the session's "Allow all" flag |
| `inserted`, `executed`, `error`, `toasts` | `generate.json`: the text inserted at the cursor, how many times the tab ran, the prompt box's error and the error toasts |
| `result`, `unusedResponses` | `models.json`: the list or the test's answer, and scripted responses nobody asked for |
| `logs` | the log lines, `level: text` |

### The chat's provider when none is configured

Two cases record `provider: null` and expect different refusals, because the connection the recording ran with differed in a field the case doesn't hold. Core's replay (`seaquel-core/tests/ai_replay.rs`) seeds them as the recordings ran:

- `turns/none/no-provider`: the connection names `prov-1`, which isn't configured, so Core answers "The chat's AI provider no longer exists."
- `generate/none/no-providers`: the connection names no provider and none is configured, so Core answers "No AI provider is configured."

Every other `turns.json` case's connection names `prov-1`; a `generate.json` case's names it only when its `provider` isn't `null`.

## How the replay compares

`seaquel-ai`'s `tests/ts_baseline.rs` (Tasks 2 and 3) builds the same requests, prompts, contexts, results and mention expansions and compares them with the records; Core's tests (Task 4) run the `turns.json` and `page.json` scripts against the Rust mock provider and `history.json` through the history builder. A case's recorded fields, with `changes.json`'s `expected` fields put in their place, must equal what Rust produces. Request bodies are compared as JSON values (key order doesn't matter), except an OpenAI tool call's `arguments` string, compared as text (serde_json's output, keys sorted). Headers are compared as a set.

## `changes.json`

Each key is `<file>/<case name>`; the value is `{decision, why, expected}`. `decision` lists the plan's Decisions and Qs behind it; `why` says what changes and why, field by field; `expected` maps a field of the case to the value Rust must produce there. A field not in `expected` must equal the recording. Every field in `expected` differs from the recording, except `logs` and `storeCalls`, which every page case lists (their meaning changed), and `tool-results/run-query/cancel-before`'s `fetches: []`, kept explicit. No entry uses `null` to mean "not compared"; `null` in `expected` is a value (`test`'s answer, a GET's body, a column's absent default).

`"*"` lists the rules for every case:

1. In each scripted model response, a `run_query` input's `query` is sent as `sql` (Decision 3). Expected requests already show `sql`.
2. `chunks` is compared as its concatenation, `text`: Rust coalesces text events (Decision 11). No entry lists `chunks`.
3. `logs` in `expected` is `{"$forbidden": [...], "$allowed": "..."}`: no Rust log line may contain a forbidden string (case-sensitive substring; `$TEST_KEY` stands for the test key, which no fixture holds), and only the allowed kinds of lines may name the call (Decisions 16 and 32). Every forbidden string is at least 8 characters, and the list includes the case's streamed reply. The markers case plants `MARKER_PROMPT_q8z7`, `MARKER_SCHEMA_q8z7`, `MARKER_CELL_q8z7` and `MARKER_REPLY_q8z7`.
4. Headers: as the code sets them; `anthropic-version` stays `2023-06-01`; native clients send no browser-access header (Decision 10).
5. Bodies as JSON values; OpenAI `arguments` as sorted-key text.
6. `{"$toolSchemas": {profile, shape, names}}` in an expected body stands for the assistant profile's definitions of those tools, in that order, from Task 3's frozen `tool-schemas.json`, in the provider's shape (Anthropic `{name, description, input_schema}`, OpenAI `{type: "function", function: {name, description, parameters}}`). The assistant profile's tools, in order: `run_query` and `explain_query` (data sharing), `list_schemas`, `list_tables`, `describe_table` and `list_saved_queries` (schema sharing), `run_saved_query` (data sharing), then the five dashboard tools (`clientTools`). No tools: no `tools` key.
7. `{"$absent": why}` means Rust has no such field or value (a cancelled turn sends no tool result; a failed `models`/`test` has no result; a failed turn's page view is Task 7's).
8. `storeCalls` is Core's writes (Decision 31), not today's page saves.
9. `history.json` cases have no recording; their entries are the whole spec.

The expected requests, results and stored rows are produced by a model of the Rust side inside the recorder (`simulateTurn`, `rustToolResult`, `decodeScript`, `renderHistory`, `historyBudget`), so one rule gives every case's value. What the entries say, by group:

- **Prompts (Decision 13, bug 7).** Schema-qualified names (`Table: public.users`); with schema sharing, a paragraph after the schema context naming the schema tools, and with data sharing one naming the data tools; the schema context cut at 128 KiB (131,072 bytes before the note), whole tables only, then `(N more tables not shown; use list_tables and describe_table to see them.)`. The large case keeps 1,056 tables and leaves out 1,944. `ai.generate` offers no tools, so its prompt has no tool paragraph. A `prompts.json` system case is read as schema sharing = its `schema`, data sharing off.
- **Tools and results (Q4, Decisions 3, 4, 6, 22, 24–26, 30).** The model's `run_query` takes `sql`; queries run with `maxRows` (100 unless `max_rows`, 1–1,000), `maxBytes: 8388608` and `timeoutMs: 60000`; results are MCP's JSON (`{"columns","message"?,"rowCount","rows","truncated","truncatedCells"?}`, keys sorted, cells by `format.rs`'s rules, a cell over 64 KB cut), within 256 KB with the notes worded in KB. A refused or failed call is an error result, `CODE: message` (`is_error: true` on Anthropic; `Error: ` before the content on OpenAI): `READ_ONLY`, `QUERY_ERROR`, `DENIED`, `DATA_SHARING_OFF`/`SCHEMA_SHARING_OFF` (MCP's messages, checked before the arguments), `INVALID_ARGUMENT` (an unknown tool; serde's messages with a dotted path, client tools included; `max_rows` out of range). The tools today's TypeScript lacks answer MCP's results for the chat's connection, within the same 256 KB (a plan past it is cut, "The plan is cut at 256 KB."). Dashboard tools that pass Core's checks keep the page's text, marked as an error when the answer holds `error`; `get_dashboard` without schema sharing has its widget queries stripped by Core.
- **Turns (Decisions 10, 15).** Every call in a round runs, in order, and each gets its result (bug 3), approvals asked in order; an `error` event, a malformed event, tool input that isn't a JSON object, or a stream that ends before its end marker is `PROVIDER_ERROR` (bug 4); `max_tokens` is `stop: maxTokens` (bug 5); a 429 is `RATE_LIMITED` and other non-2xx `PROVIDER_ERROR` with the provider's message cut at 1 KiB (bug 12); `[DONE]` ends the round; the 21st call is `TOOL_LIMIT`; `NO_API_KEY`, `NO_PROVIDER` and an unreachable provider (`PROVIDER_ERROR`, "Could not reach the provider.") are named.
- **The page (Decisions 5, 6, 12, 22, 23, 29, 31).** Mentions without schema sharing add nothing (bug 8); "Allow all" covers the rest of the turn and one connection (bug 10); Core stores the user message and the reply (two writes a turn, `storeCalls`), the reply with only its streamed text and `parts` with `round`s; history renders `parts` back in both providers' shapes, a failed call as an error and a cut result with its note; a failed turn carries `error {code, message}` and the page view is Task 7's; Core's own refusals (`NO_API_KEY` here) store nothing.
- **The inline prompt (Q9, Decisions 18, 27).** It inserts and doesn't run (`executed: 0`) and says "Inserted. Run it with ⌘↵." (`notice`, Task 7's wording); errors carry a code and a message (`NO_MODEL`, `NO_PROVIDER`, `NO_API_KEY`, `RATE_LIMITED`, `PROVIDER_ERROR`).
- **Errors (Decision 15).** The code and message each TS error string becomes; `shown` is `$absent` until Task 7 words it.
- **Model list and test (Decisions 10, 28).** `models` answers the list, `test` answers `null`; failures are `RpcError`s (`NO_API_KEY` before any request, `PROVIDER_ERROR` with the provider's message, unreachable, or a body that isn't JSON for `models`); `test` passes on any 2xx. An OpenAI-compatible provider without a base URL lists and tests against `https://api.openai.com/v1/models`.
- **History (Decision 29).** `kept` lists the stored rows the budget keeps, oldest first, with `rounds` for a reply cut to its latest whole rounds; `bytes` is what they cost.

## Corrections

What Task 7 (the GUIs on Core) words where `changes.json` left a page view `$absent` "until Task 7". The recorded files are unchanged; these are the values the page now shows, pinned by vitest.

**`errors.json`'s `shown`** (`src/lib/hooks/database/ai/messages.test.ts`, `aiErrorText(code, message)`, i18n keys `ai_error_*`). Only a provider's own message (`PROVIDER_ERROR`, `RATE_LIMITED`) appears inside the sentence; Core's other messages are never shown raw. The page shows them as plain text (no Markdown, Task 7 review M1).

| case | code | shown |
| --- | --- | --- |
| `assistant/no-provider` | `NO_PROVIDER` | This chat has no AI provider. Add one in Settings → AI, then pick a model below. |
| `assistant/no-api-key` | `NO_API_KEY` | No API key is set for this provider. Add your key in Settings → AI. |
| `assistant/rate-limit` | `RATE_LIMITED` | The provider is limiting requests (Rate limited). Wait a moment and try again. |
| `assistant/tool-limit` | `TOOL_LIMIT` | The model asked for more than 20 tool calls in this turn, so the turn was stopped. |
| `assistant/provider-body` | `PROVIDER_ERROR` | AI provider error: Internal server error |
| `assistant/fetch-failed` | `PROVIDER_ERROR` | AI provider error: Could not reach the provider. |
| `assistant/no-stream` | `PROVIDER_ERROR` | AI provider error: The provider's stream ended before the reply did. |
| `assistant/connection-removed` | (the page's own check) | Error: This chat's connection was removed (unchanged; `page/connection-removed` pins it) |

**`page.json`'s `messages` for a failed turn** (`src/lib/hooks/database/ai/page-replay.svelte.test.ts`, `CORRECTIONS`). The reply keeps the text that streamed as its `content`; the worded error is shown under it (`error`, never stored, Decision 31):

- `page/provider-500`: the user's message, then `{role: "assistant", content: "", error: "AI provider error: Internal server error"}`;
- `page/error-after-partial`: the user's message, then `{role: "assistant", content: "Half an ans", error: "AI provider error: Overloaded"}`;
- `page/no-api-key`: the user's message, then `{role: "assistant", content: "", error: "No API key is set for this provider. Add your key in Settings → AI."}` (nothing stored).

**`page.json`'s `allowAllAfter`** is read as "the session holds an Allow all for some connection" (`UIStateManager.hasAnyAllowAll`), since it is per connection now (bug 10). `page/allow-all-other-connection`'s `true` is conn-1's; conn-2's second card was still shown (`approvals`), and the replay asserts `isAllowAll("conn-1") && !isAllowAll("conn-2")`.

**`generate.json`'s `notice`** on a machine that isn't a Mac reads `Inserted. Run it with Ctrl+Enter.` (the recorded `⌘↵` is the Mac's; `ai_inline_inserted` with the editor's Run shortcut). The other inline-prompt values (`error`, `toasts`, `inserted`, `executed: 0`) are the recorded or `changes.json` values as they stand, pinned by `src/lib/components/query-editor/ai-inline-prompt.svelte.test.ts`.
