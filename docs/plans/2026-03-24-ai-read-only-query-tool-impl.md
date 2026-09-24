# AI Read-Only Query Tool — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the dead "Share query results with AI" toggle with a working setting that lets the AI call a `run_query` tool to execute read-only `SELECT` queries, with per-query user approval.

**Architecture:** The AI service becomes a multi-turn loop: stream one turn, detect a `tool_use` block, pause and call `onApprovalRequired`, execute the query via `executeRaw()`, send a `tool_result` message, and loop. Session-level "allow all" is a plain boolean on the `UIStateManager`. The approval widget is rendered inline in the chat by checking `message.pendingApproval`.

**Tech Stack:** SvelteKit 5 (runes), TypeScript, Anthropic Messages API (streaming + tool_use), OpenAI-compatible chat API (streaming + tool_calls), `executeRaw()` from `query-execution.svelte.ts`

---

### Task 1: Extend the `AIMessage` type with `pendingApproval`

**Files:**
- Modify: `src/lib/types/query.ts:203-214`

**Step 1: Add the field**

In `src/lib/types/query.ts`, extend `AIMessage`:

```ts
export interface AIMessage {
  id: string;
  role: "user" | "assistant";
  content: string;
  timestamp: Date;
  query?: string;
  /** Set while waiting for the user to approve an AI-requested query. Cleared once resolved. */
  pendingApproval?: {
    query: string;
    connectionName: string;
    approve: () => void;
    deny: () => void;
  } | null;
}
```

**Step 2: Type-check**

```bash
npm run check
```
Expected: no new errors (the field is optional so existing usages are unaffected).

---

### Task 2: Add session state and `executeRaw` reference to `UIStateManager`

**Files:**
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`
- Modify: `src/lib/hooks/database.svelte.ts` (to pass `executeRaw` into `UIStateManager`)

**Context:** `UIStateManager` is constructed in `database.svelte.ts`. It needs access to `executeRaw` (which lives on `QueryExecutionManager`) and needs to own the session-level `aiAllowAllQueries` flag.

**Step 1: Find the UIStateManager constructor call**

Search `src/lib/hooks/database.svelte.ts` for `new UIStateManager`. Note how it's constructed — what args it receives currently.

**Step 2: Add `aiAllowAllQueries` to `UIStateManager`**

In `ui-state.svelte.ts`, add inside the class body (before the constructor):

```ts
aiAllowAllQueries = $state(false);
```

And update the constructor signature to accept an `executeRaw` function:

```ts
constructor(
  private state: DatabaseState,
  private schedulePersistence: (projectId: string | null) => void,
  private executeRawQuery: (query: string) => Promise<Record<string, unknown>[]>,
) {}
```

**Step 3: Pass `executeRaw` when constructing `UIStateManager` in `database.svelte.ts`**

Find the `new UIStateManager(...)` call and add the third argument:

```ts
this.ui = new UIStateManager(
  this.state,
  this.schedulePersistence.bind(this),
  (query) => this.queryExecution.executeRaw(query),
);
```

**Step 4: Type-check**

```bash
npm run check
```
Expected: no errors.

---

### Task 3: Rewrite `sendAIMessage` in `ai.ts` — Anthropic multi-turn loop

**Files:**
- Modify: `src/lib/services/ai.ts`

**Context:** Currently `sendAIMessage` does a single streaming call. We need a loop that handles `tool_use` blocks. Start with the Anthropic provider only (OpenAI-compat in Task 4).

**Step 1: Add the `run_query` tool definition constant**

Near the top of the file (after imports), add:

```ts
const RUN_QUERY_TOOL = {
  name: "run_query",
  description:
    "Run a read-only SQL SELECT query against the connected database. Use this to fetch data that helps answer the user's question. Only SELECT and WITH queries are allowed.",
  input_schema: {
    type: "object" as const,
    properties: {
      query: {
        type: "string",
        description: "A read-only SELECT or WITH query",
      },
    },
    required: ["query"],
  },
};
```

**Step 2: Add a query validation helper**

```ts
function validateReadOnlyQuery(query: string): string | null {
  const trimmed = query.trim();
  const upper = trimmed.toUpperCase();
  if (!/^(SELECT|WITH)\b/.test(upper)) {
    return "Only read-only SELECT queries are permitted";
  }
  const dml = /\b(INSERT|UPDATE|DELETE|DROP|ALTER|TRUNCATE|CREATE)\b/;
  if (dml.test(upper)) {
    return "Only read-only SELECT queries are permitted";
  }
  return null; // valid
}
```

**Step 3: Update `SendAIMessageParams`**

Replace the current interface definition:

```ts
export interface SendAIMessageParams {
  providerId: string;
  model: string;
  messages: AIMessage[];
  schema: SchemaTable[];
  shareSchema: boolean;
  shareData: boolean;
  connectionName: string;
  executeQuery: (query: string) => Promise<Record<string, unknown>[]>;
  aiAllowAllQueries: boolean;
  onAllowAll: () => void;
  onApprovalRequired: (
    query: string,
    connectionName: string,
    approve: () => void,
    deny: () => void,
  ) => void;
  onChunk: (delta: string) => void;
  onDone: () => void;
  onError: (msg: string) => void;
}
```

Remove the old `sampleRows` and `sampleColumns` fields — they were never used.

**Step 4: Rewrite the Anthropic streaming function as a single-turn helper**

Replace the existing `streamAnthropic` function with a helper that returns the tool call info if the model requested one, or null if the turn is complete:

```ts
interface TurnResult {
  toolCall: { id: string; name: string; input: Record<string, unknown> } | null;
}

async function streamAnthropicTurn(params: {
  apiKey: string;
  model: string;
  systemPrompt: string;
  messages: { role: string; content: unknown }[];
  tools: typeof RUN_QUERY_TOOL[];
  onChunk: (delta: string) => void;
  onError: (msg: string) => void;
}): Promise<TurnResult | null> {
  const { apiKey, model, systemPrompt, messages, tools, onChunk, onError } = params;

  const body: Record<string, unknown> = {
    model,
    max_tokens: 4096,
    system: systemPrompt,
    messages,
    stream: true,
  };
  if (tools.length > 0) body.tools = tools;

  const res = await fetch("https://api.anthropic.com/v1/messages", {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "x-api-key": apiKey,
      "anthropic-version": "2023-06-01",
    },
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const errText = await res.text().catch(() => res.statusText);
    onError(res.status === 429 ? "rate_limit" : errText);
    return null;
  }

  const reader = res.body?.getReader();
  if (!reader) {
    onError("no_stream");
    return null;
  }

  const decoder = new TextDecoder();
  let buffer = "";

  // Track tool use block being accumulated
  let toolUseId: string | null = null;
  let toolUseName: string | null = null;
  let toolInputJson = "";
  let stopReason: string | null = null;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";

    for (const line of lines) {
      if (!line.startsWith("data: ")) continue;
      const data = line.slice(6).trim();
      if (data === "[DONE]") continue;
      try {
        const event = JSON.parse(data);

        if (event.type === "content_block_start" && event.content_block?.type === "tool_use") {
          toolUseId = event.content_block.id;
          toolUseName = event.content_block.name;
          toolInputJson = "";
        } else if (event.type === "content_block_delta") {
          if (event.delta?.type === "text_delta") {
            onChunk(event.delta.text);
          } else if (event.delta?.type === "input_json_delta") {
            toolInputJson += event.delta.partial_json;
          }
        } else if (event.type === "message_delta") {
          stopReason = event.delta?.stop_reason ?? null;
        }
      } catch { /* skip malformed */ }
    }
  }

  if (stopReason === "tool_use" && toolUseId && toolUseName) {
    let input: Record<string, unknown> = {};
    try { input = JSON.parse(toolInputJson); } catch { /* malformed */ }
    return { toolCall: { id: toolUseId, name: toolUseName, input } };
  }

  return { toolCall: null };
}
```

**Step 5: Rewrite the `sendAIMessage` orchestration function**

Replace the existing `sendAIMessage` body with a multi-turn loop for Anthropic. OpenAI-compat will be added in Task 4.

```ts
export async function sendAIMessage(params: SendAIMessageParams): Promise<void> {
  const {
    messages, schema, shareSchema, shareData,
    connectionName, executeQuery, aiAllowAllQueries, onAllowAll,
    onApprovalRequired, onChunk, onDone, onError,
  } = params;

  const { providerId, model } = params;
  const activeConfig = aiSettingsStore.getProvider(providerId);
  if (!activeConfig) { onError("no_provider"); return; }
  const apiKey = await getKeyringService().getAIApiKeyForProvider(activeConfig.id) ?? "";
  const { type: provider, baseUrl } = activeConfig;
  if (provider === "anthropic" && !apiKey) { onError("no_api_key"); return; }

  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, "");
  const tools = shareData ? [RUN_QUERY_TOOL] : [];

  // Build the mutable API message history
  // Anthropic uses structured content arrays; OpenAI-compat uses strings
  type ApiMessage = { role: string; content: unknown };
  let apiMessages: ApiMessage[] = messages.map((m) => ({ role: m.role, content: m.content }));

  if (provider === "anthropic") {
    while (true) {
      const turnResult = await streamAnthropicTurn({
        apiKey, model, systemPrompt,
        messages: apiMessages,
        tools,
        onChunk,
        onError,
      });

      if (!turnResult) return; // error was reported via onError

      if (!turnResult.toolCall) {
        onDone();
        return;
      }

      const { id: toolUseId, name: toolName, input } = turnResult.toolCall;
      if (toolName !== "run_query") {
        // Unknown tool — send back an error result and continue
        apiMessages = [
          ...apiMessages,
          { role: "assistant", content: [{ type: "tool_use", id: toolUseId, name: toolName, input }] },
          { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: "Unknown tool" }] },
        ];
        continue;
      }

      const query = typeof input.query === "string" ? input.query : "";

      // Validate
      const validationError = validateReadOnlyQuery(query);
      if (validationError) {
        apiMessages = [
          ...apiMessages,
          { role: "assistant", content: [{ type: "tool_use", id: toolUseId, name: toolName, input }] },
          { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: validationError }] },
        ];
        continue;
      }

      // Request approval (or auto-approve if session trust is set)
      let toolResultContent: string;
      if (aiAllowAllQueries) {
        toolResultContent = await runAndFormat(query, executeQuery);
      } else {
        toolResultContent = await new Promise<string>((resolve) => {
          onApprovalRequired(
            query,
            connectionName,
            // approve
            async () => {
              const result = await runAndFormat(query, executeQuery);
              resolve(result);
            },
            // deny
            () => resolve("User denied query execution"),
          );
        });
      }

      apiMessages = [
        ...apiMessages,
        { role: "assistant", content: [{ type: "tool_use", id: toolUseId, name: toolName, input }] },
        { role: "user", content: [{ type: "tool_result", tool_use_id: toolUseId, content: toolResultContent }] },
      ];
    }
  } else {
    // OpenAI-compatible — added in Task 4; for now fall through to onDone
    onDone();
  }
}
```

**Step 6: Add `runAndFormat` helper**

```ts
async function runAndFormat(
  query: string,
  executeQuery: (q: string) => Promise<Record<string, unknown>[]>,
): Promise<string> {
  try {
    const rows = await executeQuery(query);
    if (rows.length === 0) return "Query returned no rows.";
    const columns = Object.keys(rows[0]);
    return buildDataContext(rows, columns);
  } catch (err) {
    return `Query error: ${err instanceof Error ? err.message : String(err)}`;
  }
}
```

**Step 7: Type-check**

```bash
npm run check
```
Expected: errors about the missing new params in the `sendAIMessage` call site — that's expected and will be fixed in Task 5.

---

### Task 4: Add tool-use support to the OpenAI-compatible streaming path

**Files:**
- Modify: `src/lib/services/ai.ts`

**Context:** OpenAI-compatible APIs use a different tool_calls format. We need a similar single-turn helper.

**Step 1: Add `streamOpenAICompatTurn` helper**

```ts
async function streamOpenAICompatTurn(params: {
  apiKey: string;
  model: string;
  baseUrl: string;
  systemPrompt: string;
  messages: { role: string; content: unknown }[];
  tools: typeof RUN_QUERY_TOOL[];
  onChunk: (delta: string) => void;
  onError: (msg: string) => void;
}): Promise<TurnResult | null> {
  const { apiKey, model, baseUrl, systemPrompt, messages, tools, onChunk, onError } = params;

  const openaiTools = tools.map((t) => ({
    type: "function" as const,
    function: {
      name: t.name,
      description: t.description,
      parameters: t.input_schema,
    },
  }));

  const body: Record<string, unknown> = {
    model,
    messages: [{ role: "system", content: systemPrompt }, ...messages],
    stream: true,
  };
  if (openaiTools.length > 0) body.tools = openaiTools;

  const headers: Record<string, string> = { "content-type": "application/json" };
  if (apiKey) headers["authorization"] = `Bearer ${apiKey}`;

  const res = await fetch(`${baseUrl}/chat/completions`, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });

  if (!res.ok) {
    const errText = await res.text().catch(() => res.statusText);
    onError(res.status === 429 ? "rate_limit" : errText);
    return null;
  }

  const reader = res.body?.getReader();
  if (!reader) { onError("no_stream"); return null; }

  const decoder = new TextDecoder();
  let buffer = "";

  // Accumulate tool call
  let toolCallId: string | null = null;
  let toolCallName: string | null = null;
  let toolCallArgs = "";
  let finishReason: string | null = null;

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";

    for (const line of lines) {
      if (!line.startsWith("data: ")) continue;
      const data = line.slice(6).trim();
      if (data === "[DONE]") continue;
      try {
        const event = JSON.parse(data);
        const delta = event.choices?.[0]?.delta;
        const reason = event.choices?.[0]?.finish_reason;
        if (reason) finishReason = reason;

        if (delta?.content) {
          onChunk(delta.content);
        }
        if (delta?.tool_calls?.length) {
          const tc = delta.tool_calls[0];
          if (tc.id) toolCallId = tc.id;
          if (tc.function?.name) toolCallName = tc.function.name;
          if (tc.function?.arguments) toolCallArgs += tc.function.arguments;
        }
      } catch { /* skip */ }
    }
  }

  if (finishReason === "tool_calls" && toolCallId && toolCallName) {
    let input: Record<string, unknown> = {};
    try { input = JSON.parse(toolCallArgs); } catch { /* malformed */ }
    return { toolCall: { id: toolCallId, name: toolCallName, input } };
  }

  return { toolCall: null };
}
```

**Step 2: Replace the OpenAI-compat `else` branch in `sendAIMessage`**

In the `sendAIMessage` function, replace the `// OpenAI-compatible — added in Task 4` placeholder with:

```ts
  } else {
    // OpenAI-compatible
    const base = (baseUrl ?? "").replace(/\/$/, "") || "https://api.openai.com/v1";
    while (true) {
      const turnResult = await streamOpenAICompatTurn({
        apiKey, model, baseUrl: base, systemPrompt,
        messages: apiMessages,
        tools,
        onChunk,
        onError,
      });

      if (!turnResult) return;

      if (!turnResult.toolCall) {
        onDone();
        return;
      }

      const { id: toolCallId, name: toolName, input } = turnResult.toolCall;
      if (toolName !== "run_query") {
        apiMessages = [
          ...apiMessages,
          { role: "assistant", content: null, tool_calls: [{ id: toolCallId, type: "function", function: { name: toolName, arguments: JSON.stringify(input) } }] },
          { role: "tool", tool_call_id: toolCallId, content: "Unknown tool" },
        ];
        continue;
      }

      const query = typeof input.query === "string" ? input.query : "";
      const validationError = validateReadOnlyQuery(query);
      if (validationError) {
        apiMessages = [
          ...apiMessages,
          { role: "assistant", content: null, tool_calls: [{ id: toolCallId, type: "function", function: { name: toolName, arguments: JSON.stringify(input) } }] },
          { role: "tool", tool_call_id: toolCallId, content: validationError },
        ];
        continue;
      }

      let toolResultContent: string;
      if (aiAllowAllQueries) {
        toolResultContent = await runAndFormat(query, executeQuery);
      } else {
        toolResultContent = await new Promise<string>((resolve) => {
          onApprovalRequired(
            query,
            connectionName,
            async () => { resolve(await runAndFormat(query, executeQuery)); },
            () => resolve("User denied query execution"),
          );
        });
      }

      apiMessages = [
        ...apiMessages,
        { role: "assistant", content: null, tool_calls: [{ id: toolCallId, type: "function", function: { name: toolName, arguments: JSON.stringify(input) } }] },
        { role: "tool", tool_call_id: toolCallId, content: toolResultContent },
      ];
    }
  }
```

**Step 3: Remove the now-unused `streamAnthropic` and `streamOpenAICompat` old functions**

Delete the old `streamAnthropic` and `streamOpenAICompat` function bodies (they were the single-turn versions replaced by the new `streamAnthropicTurn` and `streamOpenAICompatTurn` helpers).

**Step 4: Type-check**

```bash
npm run check
```

---

### Task 5: Update `UIStateManager.sendAIMessage` to pass new params

**Files:**
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`

**Context:** `ui-state.svelte.ts` calls `sendAIMessageService(...)`. We need to pass the new required params: `connectionName`, `executeQuery`, `aiAllowAllQueries`, `onAllowAll`, and `onApprovalRequired`.

**Step 1: Update the `sendAIMessageService` call**

In `sendAIMessage()` inside `UIStateManager`, update the call to `sendAIMessageService`. Add these new params alongside the existing ones:

```ts
connectionName: activeConn?.name ?? "Unknown",
executeQuery: this.executeRawQuery,
aiAllowAllQueries: this.aiAllowAllQueries,
onAllowAll: () => {
  this.aiAllowAllQueries = true;
},
onApprovalRequired: (query, connectionName, approve, deny) => {
  // Set pendingApproval on the current assistant message placeholder
  this.state.aiMessages = this.state.aiMessages.map((m) =>
    m.id === assistantMessageId
      ? { ...m, pendingApproval: { query, connectionName, approve, deny } }
      : m,
  );
},
```

Also update `onDone` and `onError` to clear `pendingApproval` when done:

```ts
onDone: () => {
  this.state.isAIStreaming = false;
  this.state.aiMessages = this.state.aiMessages.map((m) =>
    m.id === assistantMessageId ? { ...m, pendingApproval: null } : m,
  );
},
```

**Step 2: Remove `sampleRows` and `sampleColumns` from the call** — they no longer exist in `SendAIMessageParams`.

**Step 3: Type-check**

```bash
npm run check
```
Expected: no errors.

---

### Task 6: Render the approval widget in `ai-assistant.svelte`

**Files:**
- Modify: `src/lib/components/ai-assistant.svelte`

**Context:** Currently message content is rendered as `<p class="whitespace-pre-wrap">{message.content}</p>`. We need to also check `message.pendingApproval` and render the widget instead of (or below) the content.

**Step 1: Add imports**

At the top of the `<script>` block, add:
```ts
import { Checkbox } from "$lib/components/ui/checkbox";
import { Label } from "$lib/components/ui/label";
import DatabaseIcon from "@lucide/svelte/icons/database";
import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
import XCircleIcon from "@lucide/svelte/icons/x-circle";
```

**Step 2: Add local state for "allow all" checkbox**

```ts
let allowAllChecked = $state(false);
```

**Step 3: Replace the message content rendering**

Find this block in the template (around line 87-89):
```svelte
<div class={["text-sm rounded-lg p-3", ...]}>
  <p class="whitespace-pre-wrap">{message.content}</p>
</div>
```

Replace with:
```svelte
<div class={["text-sm rounded-lg p-3", message.role === "user" ? "bg-primary text-primary-foreground ms-8" : "bg-muted me-8"]}>
  {#if message.content}
    <p class="whitespace-pre-wrap">{message.content}</p>
  {/if}
  {#if message.pendingApproval}
    <div class="mt-2 space-y-3">
      <div class="rounded border bg-background p-2 space-y-1">
        <p class="text-xs font-medium text-muted-foreground">Query to execute:</p>
        <pre class="text-xs font-mono whitespace-pre-wrap break-all">{message.pendingApproval.query}</pre>
      </div>
      <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
        <DatabaseIcon class="size-3" />
        <span>{message.pendingApproval.connectionName}</span>
      </div>
      <div class="flex items-center gap-1.5">
        <Checkbox id="allow-all-{message.id}" bind:checked={allowAllChecked} />
        <Label for="allow-all-{message.id}" class="text-xs font-normal cursor-pointer">
          Allow all queries this session
        </Label>
      </div>
      <div class="flex gap-2">
        <Button
          size="sm"
          class="flex-1 gap-1.5"
          onclick={() => {
            if (allowAllChecked) db.ui.setAIAllowAll();
            message.pendingApproval?.approve();
          }}
        >
          <CheckCircleIcon class="size-3.5" />
          Allow
        </Button>
        <Button
          size="sm"
          variant="outline"
          class="flex-1 gap-1.5"
          onclick={() => message.pendingApproval?.deny()}
        >
          <XCircleIcon class="size-3.5" />
          Deny
        </Button>
      </div>
    </div>
  {/if}
</div>
```

**Step 4: Expose `setAIAllowAll` on `UIStateManager`**

In `ui-state.svelte.ts`, add:
```ts
setAIAllowAll() {
  this.aiAllowAllQueries = true;
}
```

And make it accessible via `db.ui.setAIAllowAll()` (the `UIStateManager` is already exposed as `db.ui`).

**Step 5: Check with svelte-autofixer**

Run the Svelte MCP autofixer on the updated `ai-assistant.svelte` before finalizing.

**Step 6: Type-check**

```bash
npm run check
```

---

### Task 7: Update i18n strings in all locale files

**Files:**
- Modify: `messages/en.json`
- Modify: `messages/ar.json`
- Modify: `messages/de.json`
- Modify: `messages/es.json`
- Modify: `messages/fr.json`
- Modify: `messages/ko.json`

**Context:** Four keys need updating. Current values in `en.json`:
- `"settings_ai_share_data_globally"`: `"Share query results with AI"`
- `"settings_ai_share_data_globally_description"`: `"Send sample query results to the AI to enable data-aware responses"`
- `"wizard_ai_share_data"`: `"Share data with AI"`
- `"wizard_ai_share_data_description"`: `"Controls whether query results and row data are sent to the AI."`

New values for `en.json`:
```json
"settings_ai_share_data_globally": "Allow AI to run read-only queries",
"settings_ai_share_data_globally_description": "Lets the AI execute SELECT queries against your database to answer data-specific questions. You will be asked to approve each query.",
"wizard_ai_share_data": "Allow AI to run read-only queries",
"wizard_ai_share_data_description": "When enabled, the AI can run SELECT queries against this connection. You will be asked to approve each query.",
```

For the non-English locale files (`ar.json`, `de.json`, `es.json`, `fr.json`, `ko.json`), update the same four keys with equivalent translations. If you are not confident in a translation, use the English text as a placeholder — the product owner can refine later.

**Step 1: Update `messages/en.json`** — change only the four keys above.

**Step 2: Update each of the other five locale files** — same four keys.

**Step 3: Type-check**

```bash
npm run check
```

---

### Task 8: Manual smoke test

No automated tests exist for the AI flow (it hits external APIs). Instead, verify manually:

1. Open the app connected to a database
2. Enable "Allow AI to run read-only queries" in Settings → AI
3. Open the AI chat panel
4. Ask: "How many rows are in [any table]?"
5. Verify: approval widget appears with the query, connection name label, Allow/Deny buttons, and "Allow all" checkbox
6. Click Allow → verify query executes and the AI incorporates the result in its response
7. Ask another data question → verify approval widget appears again (session trust not set)
8. Click Allow with "Allow all" checked → verify subsequent queries auto-approve without showing the widget
9. Start fresh session → verify "allow all" is reset
10. Deny a query → verify the AI acknowledges the denial gracefully
11. Ask the AI to `DELETE FROM` something → verify it cannot (tool validation rejects it)
12. Disable the setting → verify the AI no longer attempts to run queries
