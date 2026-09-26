import type { SchemaTable } from "$lib/types";
import type { AIMessage } from "$lib/types/query";
import type { DatabaseType } from "$lib/types/database";
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
import { getKeyringService } from "$lib/services/keyring";
import {
  QUERY_CANCELLED,
  buildSchemaContext,
  buildSystemPrompt,
  readOnlyError,
  runAndFormat,
} from "./context.js";
import { RUN_QUERY_TOOL, DASHBOARD_TOOLS, DASHBOARD_TOOL_NAMES } from "./tool-definitions.js";
import { handleDashboardToolCall } from "./dashboard-tools.js";
import type { DashboardCallbacks, DashboardGetResult } from "./dashboard-tools.js";
import { createAnthropicProvider, createOpenAICompatProvider } from "./providers.js";
import type { AIProvider } from "./providers.js";
import { log } from "$lib/utils/logger";

const MAX_TOOL_CALLS_PER_MESSAGE = 20;

export type { DashboardGetResult };
export { buildSchemaContext, buildDataContext } from "./context.js";

// --- Public types ---

/** The connection a chat's tool calls run on. */
export interface AIConnection {
  id: string;
  type: DatabaseType;
  name: string;
}

export interface SendAIMessageParams {
  providerId: string;
  model: string;
  messages: AIMessage[];
  schema: SchemaTable[];
  shareSchema: boolean;
  shareData: boolean;
  /**
   * The connection the chat belongs to (`AIChat.connectionId`). Tool calls
   * run on it whatever is active by then; the system prompt and the
   * `run_query` tool's read-only check use its type.
   */
  connection: AIConnection;
  /**
   * Runs one read-only query on `connection` (`executeReadOnly` bound to its
   * id), which refuses when it was removed or is disconnected. The only way
   * the model's SQL runs.
   */
  runQuery: (sql: string, signal?: AbortSignal) => Promise<Record<string, unknown>[]>;
  /**
   * The active connection, read when a dashboard tool changes a widget: a
   * widget renders against the active connection, so `add_widget` and
   * `update_widget` refuse unless it's the chat's.
   */
  activeConnection: () => { id: string; name: string } | null;
  aiAllowAllQueries: boolean;
  onApprovalRequired: (
    query: string,
    connection: AIConnection,
    approve: () => void,
    deny: () => void,
  ) => void;
  /**
   * The pending approval was settled (allowed, denied or cancelled by Stop):
   * clear its card, so a second approval in the same reply gets a fresh one.
   */
  onApprovalSettled?: () => void;
  /** Stop: ends the tool loop, cancels a running query and resolves a pending approval. */
  signal?: AbortSignal;
  onChunk: (delta: string) => void;
  onDone: () => void;
  onError: (msg: string) => void;
  onDashboardCreated?: (dashboardId: string) => void;
  onCreateDashboard?: (name: string) => Promise<{ dashboardId: string } | null>;
  onAddWidget?: DashboardCallbacks["onAddWidget"];
  onGetDashboard?: (dashboardId: string) => DashboardGetResult | null;
  onUpdateWidget?: DashboardCallbacks["onUpdateWidget"];
  onRemoveWidget?: (dashboardId: string, widgetId: string) => Promise<void>;
}

// --- Provider resolution ---

function resolveProvider(
  providerType: string,
  apiKey: string,
  model: string,
  baseUrl?: string,
): AIProvider {
  if (providerType === "anthropic") {
    return createAnthropicProvider(apiKey, model);
  }
  return createOpenAICompatProvider(apiKey, model, baseUrl ?? "");
}

// --- Unified tool loop ---

function getDashboardCallbacks(params: SendAIMessageParams): DashboardCallbacks | null {
  if (
    !params.onCreateDashboard ||
    !params.onAddWidget ||
    !params.onGetDashboard ||
    !params.onUpdateWidget ||
    !params.onRemoveWidget
  ) {
    return null;
  }
  return {
    onCreateDashboard: params.onCreateDashboard,
    onAddWidget: params.onAddWidget,
    onGetDashboard: params.onGetDashboard,
    onUpdateWidget: params.onUpdateWidget,
    onRemoveWidget: params.onRemoveWidget,
  };
}

/**
 * `add_widget` and `update_widget` store a query checked for the chat's
 * connection, and the widget runs against the active one: the refusal when
 * those differ, or `null`.
 */
function dashboardConnectionError(params: SendAIMessageParams): string | null {
  const active = params.activeConnection();
  if (active?.id === params.connection.id) return null;
  const change = `switch back to "${params.connection.name}" to change this dashboard`;
  return active
    ? `The active connection is "${active.name}"; ${change}`
    : `No connection is active; ${change}`;
}

const WIDGET_WRITE_TOOLS = new Set(["add_widget", "update_widget"]);

/**
 * Waits for the user to approve `query`. Resolves with the tool result:
 * the query's, "User denied query execution", or `QUERY_CANCELLED` when the
 * signal aborts first (Stop), after which a late approval runs nothing.
 */
function runAfterApproval(query: string, params: SendAIMessageParams): Promise<string> {
  const { signal } = params;
  if (signal?.aborted) return Promise.resolve(QUERY_CANCELLED);
  return new Promise<string>((resolve) => {
    let decided = false;
    const decide = (result: () => Promise<string> | string) => {
      if (decided) return;
      decided = true;
      signal?.removeEventListener("abort", onAbort);
      params.onApprovalSettled?.();
      void Promise.resolve(result()).then(resolve);
    };
    const onAbort = () => decide(() => QUERY_CANCELLED);
    signal?.addEventListener("abort", onAbort, { once: true });
    params.onApprovalRequired(
      query,
      params.connection,
      () => decide(() => runAndFormat(query, params.runQuery, signal)),
      () => decide(() => "User denied query execution"),
    );
  });
}

/** One tool call from the model. Exported for tests. */
export async function handleToolCall(
  toolName: string,
  input: Record<string, unknown>,
  params: SendAIMessageParams,
): Promise<string> {
  if (DASHBOARD_TOOL_NAMES.has(toolName)) {
    const callbacks = getDashboardCallbacks(params);
    if (!callbacks) return JSON.stringify({ error: "Dashboard tools not available" });
    if (WIDGET_WRITE_TOOLS.has(toolName)) {
      const refusal = dashboardConnectionError(params);
      if (refusal) return JSON.stringify({ error: refusal });
    }
    const result = await handleDashboardToolCall(
      toolName,
      input,
      callbacks,
      (query) => readOnlyError(query, params.connection.type),
      params.signal,
    );
    if (toolName === "create_dashboard" && params.onDashboardCreated) {
      try {
        const parsed = JSON.parse(result);
        if (parsed.dashboard_id) {
          params.onDashboardCreated(parsed.dashboard_id);
        }
      } catch {
        // parse failed — skip callback
      }
    }
    return result;
  }

  if (toolName !== "run_query") {
    return "Unknown tool";
  }

  const query = typeof input.query === "string" ? input.query : "";
  const validationError = readOnlyError(query, params.connection.type);
  if (validationError) return validationError;

  if (params.aiAllowAllQueries) {
    return runAndFormat(query, params.runQuery, params.signal);
  }
  return runAfterApproval(query, params);
}

export async function sendAIMessage(params: SendAIMessageParams): Promise<void> {
  const { messages, schema, shareSchema, connection, signal, onChunk, onDone, onError } = params;

  const { providerId, model } = params;
  const activeConfig = aiSettingsStore.getProvider(providerId);
  if (!activeConfig) {
    onError("no_provider");
    return;
  }
  const apiKey = (await getKeyringService().getAIApiKeyForProvider(activeConfig.id)) ?? "";
  const { type: providerType, baseUrl } = activeConfig;
  if (providerType === "anthropic" && !apiKey) {
    onError("no_api_key");
    return;
  }

  const provider = resolveProvider(providerType, apiKey, model, baseUrl);
  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, connection.type, !!params.onCreateDashboard);
  const dataTools = params.shareData ? [RUN_QUERY_TOOL] : [];
  const dashboardTools = params.onCreateDashboard ? DASHBOARD_TOOLS : [];
  const tools = [...dataTools, ...dashboardTools];

  type ApiMessage = Record<string, unknown>;
  let apiMessages: ApiMessage[] = messages.map((m) => ({ role: m.role, content: m.content }));

  void log.debug(`[AI] sendAIMessage: sending via ${providerType} ${model}`);

  try {
    let toolCallCount = 0;
    while (true) {
      if (signal?.aborted) return;

      const turnResult = await provider.streamTurn({
        systemPrompt,
        messages: apiMessages,
        tools,
        onChunk,
        onError,
        signal,
      });

      if (!turnResult) return;

      if (!turnResult.toolCall) {
        onDone();
        return;
      }

      toolCallCount++;
      if (toolCallCount > MAX_TOOL_CALLS_PER_MESSAGE) {
        onError("Tool call limit exceeded (max 20 per message)");
        return;
      }

      const { id: toolCallId, name: toolName, input } = turnResult.toolCall;
      const toolResultContent = await handleToolCall(toolName, input, params);
      if (signal?.aborted) return;

      apiMessages = [
        ...apiMessages,
        provider.formatAssistantToolUse(turnResult.assistantText, turnResult.toolCall),
        provider.formatToolResult(toolCallId, toolResultContent),
      ];
    }
  } catch (err) {
    if (err instanceof DOMException && err.name === "AbortError") return;
    throw err;
  }
}

// --- SQL generation (non-streaming, for inline editor prompt) ---

export interface GenerateSQLParams {
  providerId: string;
  model: string;
  request: string;
  existingQuery: string;
  schema: SchemaTable[];
  shareSchema: boolean;
  databaseType?: DatabaseType;
}

export async function generateSQL(params: GenerateSQLParams): Promise<string> {
  const { request, existingQuery, schema, shareSchema, databaseType } = params;

  const { providerId, model } = params;
  const activeConfig = aiSettingsStore.getProvider(providerId);
  if (!activeConfig) throw new Error("no_provider");
  const apiKey = (await getKeyringService().getAIApiKeyForProvider(activeConfig.id)) ?? "";
  const { type: providerType, baseUrl } = activeConfig;
  if (providerType === "anthropic" && !apiKey) throw new Error("no_api_key");

  const provider = resolveProvider(providerType, apiKey, model, baseUrl);
  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, databaseType);

  const userMessage = existingQuery.trim()
    ? `${request}\n\nExisting query for context:\n\`\`\`sql\n${existingQuery}\n\`\`\``
    : request;
  void log.debug(`[AI] generateSQL: generating via ${providerType} ${model}`);

  const content = await provider.fetchNonStreaming({
    systemPrompt,
    messages: [{ role: "user", content: userMessage }],
  });

  // Extract SQL from markdown code block if present
  const match = content.match(/```(?:sql)?\n([\s\S]*?)```/);
  return match ? match[1].trim() : content.trim();
}
