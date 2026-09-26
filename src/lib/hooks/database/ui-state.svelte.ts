import type { AIMessage, DashboardWidget, DatabaseConnection } from "$lib/types";
import type { ActiveViewType } from "$lib/types/persisted";
import type { DatabaseState } from "./state.svelte.js";
import type { AIChatManager } from "./ai-chat-manager.svelte.js";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";
import { sendAIMessage as sendAIMessageService } from "$lib/services/ai";
import { resolveMentions } from "$lib/services/ai-mentions";
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";

import { stripWidgetRuntimeState } from "./dashboard-serialize.js";

/**
 * Manages UI state: AI panel, view switching.
 */
export class UIStateManager {
  aiAllowAllQueries = $state(false);
  private aiAbortController: AbortController | null = null;

  constructor(
    private state: DatabaseState,
    private schedulePersistence: (projectId: string | null) => void,
    /** `executeReadOnly`: the only way the model's SQL runs. */
    private runReadOnly: (
      connectionId: string,
      sql: string,
      signal?: AbortSignal,
      connectionName?: string,
    ) => Promise<Record<string, unknown>[]>,
    private aiChatManager: AIChatManager,
    private persistAIChatMessages: (chatId: string) => Promise<void>,
    private dashboardManager: DashboardManager,
    private dashboardTabs: DashboardTabManager,
  ) {}

  setAIAllowAll() {
    this.aiAllowAllQueries = true;
  }

  resetAISessionState() {
    this.aiAllowAllQueries = false;
  }

  cancelAIStream() {
    if (this.aiAbortController) {
      this.aiAbortController.abort();
      this.aiAbortController = null;
    }
    this.state.isAIStreaming = false;

    // Clear pendingApproval on any assistant message and persist
    const chatId = this.state.activeAIChatId;
    if (chatId) {
      const messages = this.state.aiMessagesByChat[chatId] ?? [];
      const hasStale = messages.some((m) => m.pendingApproval);
      if (hasStale) {
        this._setMessages(
          chatId,
          messages.map((m) => (m.pendingApproval ? { ...m, pendingApproval: null } : m)),
        );
      }
      void this.persistAIChatMessages(chatId);
    }
  }

  toggleAI() {
    this.state.isAIOpen = !this.state.isAIOpen;
  }

  sendAIMessage(content: string) {
    const chatId = this.aiChatManager.ensureActiveChat();
    if (!chatId) return;

    const messages = this.state.aiMessagesByChat[chatId] ?? [];
    const isFirstMessage = messages.filter((m) => m.role === "user").length === 0;

    const userMessage: AIMessage = {
      id: crypto.randomUUID(),
      chatId,
      role: "user",
      content,
      timestamp: new Date(),
    };
    this._setMessages(chatId, [...messages, userMessage]);

    if (isFirstMessage) {
      this.aiChatManager.updateChatTitle(chatId, content);
    }

    const enrichedContent = resolveMentions(
      content,
      this.state.activeSchema,
      this.state.queriesByProject[this.state.activeProjectId ?? ""] ?? [],
      this.state.dashboardsByProject[this.state.activeProjectId ?? ""] ?? [],
    );

    this._dispatchToAI(content, chatId, enrichedContent);
  }

  retryPendingMessage(messageId: string) {
    const chatId = this.state.activeAIChatId;
    if (!chatId) return;
    const messages = this.state.aiMessagesByChat[chatId] ?? [];
    const msg = messages.find((m) => m.id === messageId);
    if (!msg?.pendingModelSelection) return;
    const content = msg.pendingModelSelection;
    this._setMessages(
      chatId,
      messages.filter((m) => m.id !== messageId),
    );
    this._dispatchToAI(content, chatId);
  }

  private _getMessages(chatId: string): AIMessage[] {
    return this.state.aiMessagesByChat[chatId] ?? [];
  }

  private _setMessages(chatId: string, messages: AIMessage[]) {
    this.state.aiMessagesByChat = {
      ...this.state.aiMessagesByChat,
      [chatId]: messages,
    };
  }

  private _updateMessage(chatId: string, messageId: string, updater: (m: AIMessage) => AIMessage) {
    const messages = this.state.aiMessagesByChat[chatId];
    if (!messages) return; // Chat was deleted mid-stream
    this._setMessages(
      chatId,
      messages.map((m) => (m.id === messageId ? updater(m) : m)),
    );
  }

  private _resolveAISettings(connection: DatabaseConnection | undefined) {
    const settings = aiSettingsStore.settings;
    const shareSchema =
      connection?.aiShareSchema !== undefined
        ? connection.aiShareSchema
        : settings.shareSchemaGlobally;
    const shareData =
      connection?.aiShareData !== undefined ? connection.aiShareData : settings.shareDataGlobally;
    const activeProviderId = connection?.activeAIProviderId ?? null;
    const activeModel = connection?.activeAIModel ?? null;
    return { shareSchema, shareData, activeProviderId, activeModel };
  }

  private _buildMessagesForApi(
    chatId: string,
    assistantMessageId: string,
    enrichedContent?: string,
  ): AIMessage[] {
    const rawMessages = this._getMessages(chatId).filter(
      (m) => m.id !== assistantMessageId && !m.pendingModelSelection,
    );

    // Find the most recent dashboard ID from prior assistant messages
    const lastDashboardId = rawMessages.findLast((m) => m.dashboardId)?.dashboardId;

    let messages = rawMessages;
    if (enrichedContent) {
      messages = messages.map((msg, i) => {
        if (i === messages.length - 1 && msg.role === "user") {
          return { ...msg, content: enrichedContent };
        }
        return msg;
      });
    }

    // Inject dashboard context so the AI knows which dashboard to operate on
    if (lastDashboardId) {
      const lastUserIdx = messages.findLastIndex((m) => m.role === "user");
      if (lastUserIdx !== -1) {
        const msg = messages[lastUserIdx];
        messages = messages.map((m, i) =>
          i === lastUserIdx
            ? {
                ...m,
                content: `${msg.content}\n\n[Context: The active dashboard ID is "${lastDashboardId}". Use this ID for any dashboard tool calls.]`,
              }
            : m,
        );
      }
    }

    return messages;
  }

  private _formatAIError(err: string): string {
    if (err === "no_provider")
      return "No AI provider configured. Please add one in **Settings → AI**.";
    if (err === "no_api_key")
      return "No API key configured. Please add your key in **Settings → AI**.";
    if (err === "rate_limit") return "Rate limit reached. Please wait a moment and try again.";
    return `Error: ${err}`;
  }

  /**
   * The dashboard tools' callbacks for a chat on `connectionId`. A widget
   * runs against the active connection, and `handleToolCall` checks that it's
   * the chat's before a widget changes; saving awaits persistence, so the
   * first run checks again and is skipped after a switch (the widget runs
   * read-only on the next refresh). `signal` is the AI's Stop.
   */
  private _createDashboardCallbacks(connectionId: string) {
    const stillActive = () => this.state.activeConnectionId === connectionId;
    return {
      onCreateDashboard: async (name: string) => {
        const dashboard = await this.dashboardManager.createDashboard(name);
        if (!dashboard) return null;
        this.dashboardTabs.add(dashboard.id, name);
        return { dashboardId: dashboard.id };
      },
      onAddWidget: async (
        dashboardId: string,
        widget: Omit<DashboardWidget, "id" | "result" | "isLoading" | "error" | "lastRefreshed">,
        signal?: AbortSignal,
      ) => {
        const widgetId = `widget-${crypto.randomUUID()}`;
        const fullWidget = { ...widget, id: widgetId } as DashboardWidget;
        await this.dashboardManager.addWidget(dashboardId, fullWidget);
        if (stillActive() && !signal?.aborted) {
          await this.dashboardManager.executeWidget(dashboardId, widgetId, signal);
        }
        return { widgetId };
      },
      onGetDashboard: (dashboardId: string) => {
        const dashboard = this.dashboardManager.getDashboard(dashboardId);
        if (!dashboard) return null;
        return {
          id: dashboard.id,
          name: dashboard.name,
          widgets: dashboard.widgets.map(stripWidgetRuntimeState),
        };
      },
      onUpdateWidget: async (
        dashboardId: string,
        widgetId: string,
        updates: Partial<DashboardWidget>,
        signal?: AbortSignal,
      ) => {
        const queryChanged = updates.query !== undefined;
        await this.dashboardManager.updateWidget(dashboardId, widgetId, updates);
        if (queryChanged && stillActive() && !signal?.aborted) {
          await this.dashboardManager.executeWidget(dashboardId, widgetId, signal);
        }
      },
      onRemoveWidget: async (dashboardId: string, widgetId: string) => {
        await this.dashboardManager.removeWidget(dashboardId, widgetId);
      },
    };
  }

  /**
   * The connection a chat belongs to (`AIChat.connectionId`), if the chat and
   * the connection still exist. No fallback to the active connection: a chat
   * without one fails closed.
   */
  private _chatConnection(chatId: string): DatabaseConnection | undefined {
    const connectionId = Object.values(this.state.aiChatsByConnection)
      .flat()
      .find((c) => c.id === chatId)?.connectionId;
    if (!connectionId) return undefined;
    return this.state.connections.find((c) => c.id === connectionId);
  }

  private _dispatchToAI(content: string, chatId: string, enrichedContent?: string) {
    // Everything below is bound to the chat's connection; nothing reads the
    // active connection after this, except the dashboard tools' check.
    const connection = this._chatConnection(chatId);
    const { shareSchema, shareData, activeProviderId, activeModel } =
      this._resolveAISettings(connection);

    if (!connection) {
      this._setMessages(chatId, [
        ...this._getMessages(chatId),
        {
          id: crypto.randomUUID(),
          chatId,
          role: "assistant",
          content: this._formatAIError("This chat's connection was removed"),
          timestamp: new Date(),
        },
      ]);
      return;
    }

    if (!activeProviderId || !activeModel) {
      const noModelMsg: AIMessage = {
        id: crypto.randomUUID(),
        chatId,
        role: "assistant",
        content: "",
        timestamp: new Date(),
        pendingModelSelection: content,
      };
      this._setMessages(chatId, [...this._getMessages(chatId), noModelMsg]);
      return;
    }

    const assistantMessageId = crypto.randomUUID();
    const assistantMessage: AIMessage = {
      id: assistantMessageId,
      chatId,
      role: "assistant",
      content: "",
      timestamp: new Date(),
    };
    this._setMessages(chatId, [...this._getMessages(chatId), assistantMessage]);
    this.state.isAIStreaming = true;

    if (this.aiAbortController) this.aiAbortController.abort();
    this.aiAbortController = new AbortController();
    const { signal } = this.aiAbortController;
    // Stop (or a newer message) resolves a pending approval as cancelled
    // (`handleToolCall`); clear its card on this chat, which may no longer be
    // the active one.
    signal.addEventListener(
      "abort",
      () => {
        if (!this._getMessages(chatId).find((m) => m.id === assistantMessageId)?.pendingApproval) {
          return;
        }
        this._updateMessage(chatId, assistantMessageId, (m) => ({ ...m, pendingApproval: null }));
        void this.persistAIChatMessages(chatId);
      },
      { once: true },
    );
    const aiConnection = { id: connection.id, type: connection.type, name: connection.name };

    const messagesForApi = this._buildMessagesForApi(chatId, assistantMessageId, enrichedContent);

    void sendAIMessageService({
      messages: messagesForApi,
      schema: this.state.schemas[connection.id] ?? [],
      shareSchema,
      shareData,
      providerId: activeProviderId,
      model: activeModel,
      connection: aiConnection,
      runQuery: (sql, querySignal) =>
        this.runReadOnly(aiConnection.id, sql, querySignal, aiConnection.name),
      activeConnection: () => {
        const active = this.state.activeConnection;
        return active ? { id: active.id, name: active.name } : null;
      },
      aiAllowAllQueries: this.aiAllowAllQueries,
      signal,
      onApprovalRequired: (query, conn, approve, deny) => {
        this._updateMessage(chatId, assistantMessageId, (m) => ({
          ...m,
          pendingApproval: {
            id: crypto.randomUUID(),
            query,
            connectionName: conn.name,
            connectionType: conn.type,
            approve,
            deny,
          },
        }));
      },
      onApprovalSettled: () => {
        this._updateMessage(chatId, assistantMessageId, (m) => ({ ...m, pendingApproval: null }));
      },
      onDashboardCreated: (dashboardId: string) => {
        this._updateMessage(chatId, assistantMessageId, (m) => ({
          ...m,
          dashboardId,
        }));
      },
      onChunk: (delta: string) => {
        this._updateMessage(chatId, assistantMessageId, (m) => ({
          ...m,
          content: m.content + delta,
        }));
      },
      onDone: () => {
        this.state.isAIStreaming = false;
        this._updateMessage(chatId, assistantMessageId, (m) => ({
          ...m,
          pendingApproval: null,
        }));
        this.aiChatManager.updateChatTimestamp(chatId);
        void this.persistAIChatMessages(chatId);
      },
      onError: (err: string) => {
        this.state.isAIStreaming = false;
        this._updateMessage(chatId, assistantMessageId, (m) => ({
          ...m,
          content: this._formatAIError(err),
          pendingApproval: null,
        }));
        void this.persistAIChatMessages(chatId);
      },
      ...this._createDashboardCallbacks(connection.id),
    });
  }

  setActiveView(view: ActiveViewType) {
    this.state.activeView = view;
    this.schedulePersistence(this.state.activeProjectId);
  }
}
