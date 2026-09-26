import type { SchemaTable } from "$lib/types";
import type { AIMessage } from "$lib/types/query";
import type { DatabaseType } from "$lib/types/database";
import type { DashboardWidget } from "$lib/types/dashboard";
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
import { getKeyringService } from "$lib/services/keyring";
import { buildSchemaContext, buildSystemPrompt, readOnlyError, runAndFormat } from "./context.js";
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

export interface SendAIMessageParams {
  providerId: string;
  model: string;
  messages: AIMessage[];
  schema: SchemaTable[];
  shareSchema: boolean;
  shareData: boolean;
  connectionName: string;
  /**
   * The connection's engine. The `run_query` tool's read-only check reads
   * the query with its quoting, and refuses every query when it's unknown.
   */
  databaseType: DatabaseType | undefined;
  executeQuery: (query: string) => Promise<Record<string, unknown>[]>;
  aiAllowAllQueries: boolean;
  onApprovalRequired: (
    query: string,
    connectionName: string,
    approve: () => void,
    deny: () => void,
  ) => void;
  signal?: AbortSignal;
  onChunk: (delta: string) => void;
  onDone: () => void;
  onError: (msg: string) => void;
  onDashboardCreated?: (dashboardId: string) => void;
  onCreateDashboard?: (name: string) => Promise<{ dashboardId: string } | null>;
  onAddWidget?: (
    dashboardId: string,
    widget: Omit<DashboardWidget, "id" | "result" | "isLoading" | "error" | "lastRefreshed">,
  ) => Promise<{ widgetId: string } | null>;
  onGetDashboard?: (dashboardId: string) => DashboardGetResult | null;
  onUpdateWidget?: (
    dashboardId: string,
    widgetId: string,
    updates: Partial<DashboardWidget>,
  ) => Promise<void>;
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

/** One tool call from the model. Exported for tests. */
export async function handleToolCall(
  toolName: string,
  input: Record<string, unknown>,
  params: SendAIMessageParams,
): Promise<string> {
  if (DASHBOARD_TOOL_NAMES.has(toolName)) {
    const callbacks = getDashboardCallbacks(params);
    if (!callbacks) return JSON.stringify({ error: "Dashboard tools not available" });
    const result = await handleDashboardToolCall(toolName, input, callbacks, (query) =>
      readOnlyError(query, params.databaseType),
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
  const validationError = readOnlyError(query, params.databaseType);
  if (validationError) return validationError;

  if (params.aiAllowAllQueries) {
    return runAndFormat(query, params.executeQuery);
  }

  return new Promise<string>((resolve) => {
    params.onApprovalRequired(
      query,
      params.connectionName,
      async () => resolve(await runAndFormat(query, params.executeQuery)),
      () => resolve("User denied query execution"),
    );
  });
}

export async function sendAIMessage(params: SendAIMessageParams): Promise<void> {
  const { messages, schema, shareSchema, databaseType, signal, onChunk, onDone, onError } = params;

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
  const systemPrompt = buildSystemPrompt(schemaCtx, databaseType, !!params.onCreateDashboard);
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
