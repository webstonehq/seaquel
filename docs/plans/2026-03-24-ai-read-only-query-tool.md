# AI Read-Only Query Tool

## Overview

Replace the unused "Share query results with AI" privacy toggle with a functional setting that allows the AI to run read-only `SELECT` queries against the active database connection. The AI uses a `run_query` tool via the LLM tool-use API; each query requires user approval before execution, with a session-level "allow all" option.

## Architecture

### Tool-Use Loop (`src/lib/services/ai.ts`)

The `sendAIMessage` function becomes a multi-turn loop:

1. Send user message to LLM with a `run_query` tool definition (only included when `shareData` is `true`)
2. Stream the response as normal
3. If the model emits a `tool_use` block, pause and call `onApprovalRequired(query, connectionName, approve, deny)`
4. Caller renders the approval widget; user decides
5. On approval: execute the query via the DB adapter, send back a `tool_result` message, resume the loop
6. On denial: send back `tool_result` with `"User denied query execution"`, resume the loop
7. Loop terminates when the model returns a response with no tool calls

Tool definition sent to the LLM:
```json
{
  "name": "run_query",
  "description": "Run a read-only SQL SELECT query against the connected database. Use this to fetch data that helps answer the user's question.",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": { "type": "string", "description": "A read-only SELECT or WITH query" }
    },
    "required": ["query"]
  }
}
```

### Query Validation & Execution

Before executing any tool-called query:
- Strip leading whitespace and check the query starts with `SELECT` or `WITH` (case-insensitive)
- Reject if it contains DML keywords: `INSERT`, `UPDATE`, `DELETE`, `DROP`, `ALTER`, `TRUNCATE`, `CREATE`
- Execute via the active connection's DB adapter (same path as the query editor)
- Format results with the existing `buildDataContext()` function (up to 5 rows)
- Return formatted result as the `tool_result` content

### Approval State (`src/lib/hooks/database/ui-state.svelte.ts`)

Add a session-level flag: `aiAllowAllQueries: boolean = false` (plain `$state`, not persisted).

When `onApprovalRequired` is called and `aiAllowAllQueries` is `true`, auto-approve without showing the widget.

### AIMessage Type (`src/lib/types/ai.ts` or `src/lib/types/query.ts`)

Add optional field:
```ts
pendingApproval?: { query: string; connectionName: string } | null;
```

The Svelte chat component renders the approval widget for any message with `pendingApproval` set.

## UI: Approval Widget

Shown inline in the chat for the assistant message slot awaiting approval:

- The AI's preceding text (if any) rendered above
- Read-only SQL code block showing the query
- Read-only label: "Connection: <connection name>"
- Two buttons: **Allow** and **Deny**
- Checkbox: "Allow all queries this session"

On Allow: calls `approve()`, collapses widget, streaming resumes.
On Deny: calls `deny()`, collapses widget, model receives denial message.
If "Allow all" is checked on Allow: sets `aiAllowAllQueries = true` before calling `approve()`.

## Privacy Setting

The existing `shareData` / `aiShareData` boolean is repurposed — no storage migration needed.

| Before | After |
|--------|-------|
| "Share query results with AI" | "Allow AI to run read-only queries" |

- `false` (default): `run_query` tool omitted from LLM request; AI cannot execute queries
- `true`: `run_query` tool included; approval flow active

Update all locale files (`en.json`, `ar.json`, `de.json`, `es.json`, `fr.json`, `ko.json`).

## Error Handling

| Case | Behavior |
|------|----------|
| Query fails validation (not SELECT/WITH, or contains DML) | `tool_result`: `"Error: Only read-only SELECT queries are permitted"` |
| Query execution error | `tool_result`: the DB error message |
| User denies | `tool_result`: `"User denied query execution"` |
| No active connection | `tool_result`: `"Error: No active database connection"` |
| `shareData` is false | Tool definition omitted; model never calls it |

## Files Changed

- `src/lib/services/ai.ts` — multi-turn tool-use loop, query validation, new callbacks
- `src/lib/types/ai.ts` — add `pendingApproval` to `AIMessage`
- `src/lib/hooks/database/ui-state.svelte.ts` — `aiAllowAllQueries` session state, updated `sendAIMessage` call
- `src/lib/hooks/database/state.svelte.ts` — add `aiAllowAllQueries` field if needed
- `src/lib/components/ai-assistant.svelte` — render approval widget
- `src/lib/components/settings-dialog.svelte` — rename setting label
- `src/lib/components/connection-wizard/wizard-step-details.svelte` — rename setting label
- `messages/en.json` (and ar, de, es, fr, ko) — update i18n strings
