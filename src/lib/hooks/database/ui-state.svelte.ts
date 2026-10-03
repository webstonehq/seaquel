import type { AIMessage, DashboardWidget, DatabaseConnection } from "$lib/types";
import type { ActiveViewType } from "$lib/types/persisted";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { DatabaseState } from "./state.svelte.js";
import type { AIChatManager } from "./ai-chat-manager.svelte.js";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";
import { CANCELLED } from "$lib/core/client";
import { handleDashboardToolCall } from "$lib/services/ai/dashboard-tools";
import { readOnlyError } from "$lib/services/ai/context";
import { m } from "$lib/paraglide/messages.js";
import { log } from "$lib/utils/logger";
import { getAi, type AiDecision } from "./ai/index.js";
import { aiErrorText } from "./ai/messages.js";
import {
  appendText,
  finishTool,
  resumeTool,
  settleTools,
  startTool,
  waitTool,
} from "./ai/events.js";
import { stripWidgetRuntimeState } from "./dashboard-serialize.js";

/** The abort reason when a streaming chat is deleted. */
const CHAT_DELETED = "chat-deleted";

/** A widget renders against the active connection: these refuse unless it's the chat's. */
const WIDGET_WRITE_TOOLS = new Set(["add_widget", "update_widget"]);

/**
 * Ends without `messages` that mean the turn may still have been stored
 * (the transport lost it): the chat is read again once the turn ends.
 */
const LOST_TURN_CODES = new Set([
  "WS_CLOSED",
  "ACCESS_LOST",
  "TOO_MANY_TABS",
  "CORE_RESTARTED",
  "NETWORK_ERROR",
  "UNKNOWN",
]);

interface RunningTurn {
  chatId: string;
  streamId: string;
  assistantMessageId: string;
  connection: DatabaseConnection;
  controller: AbortController;
}

/**
 * UI state: the AI panel, view switching, and the assistant's view model.
 *
 * The assistant runs in Core (phase 6): a turn is one `ai.chat`, and this
 * shows its events: the reply's text and tool lines (Q7), the approval
 * card (answered with `respond`), the dashboard tools (run here, answered
 * with `respond`), and the rows Core stored (`done.messages`, by `seq`).
 * It decides nothing about the turn: not what the model is sent, which
 * tools it gets, or when the turn is stored.
 */
/**
 * What an "Allow all" was given for: the connection's engine and where it
 * points. Changing any of them (or removing the connection) forgets it.
 */
function allowAllTarget(c: DatabaseConnection): string {
  return JSON.stringify([c.type, c.host ?? "", c.port ?? null, c.databaseName ?? ""]);
}

export class UIStateManager {
  /**
   * "Allow all" given this session, per saved connection (bug 10), with
   * what the connection pointed at then (`allowAllTarget`).
   */
  aiAllowAllByConnection = $state<Record<string, string>>({});
  private turn: RunningTurn | null = null;

  constructor(
    private state: DatabaseState,
    private schedulePersistence: (projectId: string | null) => void,
    private aiChatManager: AIChatManager,
    private dashboardManager: DashboardManager,
    private dashboardTabs: DashboardTabManager,
  ) {}

  /**
   * Whether "Allow all" was given on `connectionId` this session, and the
   * connection still points where it did then (review M4). Reads only:
   * `forgetStaleAllowAll` drops what no longer holds.
   */
  isAllowAll(connectionId: string): boolean {
    const given = this.aiAllowAllByConnection[connectionId];
    if (given === undefined) return false;
    const connection = this.state.connections.find((c) => c.id === connectionId);
    return connection !== undefined && allowAllTarget(connection) === given;
  }

  /** Whether "Allow all" holds on any connection this session. */
  hasAnyAllowAll(): boolean {
    return Object.keys(this.aiAllowAllByConnection).some((id) => this.isAllowAll(id));
  }

  /**
   * Drop each "Allow all" whose connection was removed or now points
   * elsewhere (engine, host, port, database): called when the page's
   * connections change, so it can't come back with them.
   */
  forgetStaleAllowAll(): void {
    const kept = Object.fromEntries(
      Object.entries(this.aiAllowAllByConnection).filter(([id]) => this.isAllowAll(id)),
    );
    if (Object.keys(kept).length !== Object.keys(this.aiAllowAllByConnection).length) {
      this.aiAllowAllByConnection = kept;
    }
  }

  /**
   * Stop: the turn is cancelled in Core, which stores what streamed (Q8).
   * Nothing is answered: a waiting approval or client tool is dropped with
   * the turn.
   */
  cancelAIStream() {
    const turn = this.turn;
    const chatId = turn?.chatId ?? this.state.aiStreamingChatId ?? this.state.activeAIChatId;
    this.turn = null;
    turn?.controller.abort();
    this.state.isAIStreaming = false;
    this.state.aiStreamingChatId = null;
    if (chatId && turn) {
      this._settleReply(chatId, turn.assistantMessageId);
      // Core stores what streamed, after the stream ended here: read the
      // chat once the turn ends (review M5), and hear Core's own-origin
      // `chatMessages` event for it, which usually comes after that read.
      this.aiChatManager.refetchAfterTurnFor(chatId);
      this.aiChatManager.awaitOwnStore(chatId);
    }
  }

  /** Stops the turn streaming on `chatId`, if one is: the chat is being deleted. */
  abortStreamFor(chatId: string) {
    if (this.turn?.chatId !== chatId) return;
    const turn = this.turn;
    this.turn = null;
    turn.controller.abort(CHAT_DELETED);
    this.state.isAIStreaming = false;
    this.state.aiStreamingChatId = null;
  }

  toggleAI() {
    this.state.isAIOpen = !this.state.isAIOpen;
  }

  /**
   * Send a message in the active chat (created first, through Core, when
   * there's none). A chat the web's budget filled takes no more (Q17).
   * What was typed goes as it is: Core resolves `@mentions` (Decision 13).
   */
  async sendAIMessage(content: string): Promise<boolean> {
    const chatId = await this.aiChatManager.ensureActiveChat();
    // False: nothing was sent (no chat, a refused create, a full chat), so
    // the caller keeps what was typed.
    if (!chatId || this.state.aiChatFull[chatId]) return false;

    const messages = this._getMessages(chatId);
    const isFirstMessage = messages.filter((msg) => msg.role === "user").length === 0;
    const userMessage: AIMessage = {
      id: crypto.randomUUID(),
      chatId,
      role: "user",
      content,
      timestamp: new Date(),
    };
    this._setMessages(chatId, [...messages, userMessage]);
    if (isFirstMessage) this.aiChatManager.updateChatTitle(chatId, content);

    this._dispatch(chatId, { id: userMessage.id, content });
    return true;
  }

  /** A message that waited for a model: sent now, under its own id. */
  retryPendingMessage(messageId: string) {
    const chatId = this.state.activeAIChatId;
    if (!chatId || this.state.aiChatFull[chatId]) return;
    const messages = this._getMessages(chatId);
    const at = messages.findIndex((msg) => msg.id === messageId);
    const pending = messages[at];
    if (!pending?.pendingModelSelection) return;
    const before = messages[at - 1];
    const user =
      before?.role === "user" && before.content === pending.pendingModelSelection
        ? { id: before.id, content: before.content }
        : { id: crypto.randomUUID(), content: pending.pendingModelSelection };
    this._setMessages(
      chatId,
      messages.filter((msg) => msg.id !== messageId),
    );
    this._dispatch(chatId, user);
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

  private _updateMessage(
    chatId: string,
    messageId: string,
    updater: (msg: AIMessage) => AIMessage,
  ) {
    const messages = this.state.aiMessagesByChat[chatId];
    if (!messages) return; // The chat was deleted.
    this._setMessages(
      chatId,
      messages.map((msg) => (msg.id === messageId ? updater(msg) : msg)),
    );
  }

  /** The reply after its turn ended: no card, no call still running. */
  private _settleReply(chatId: string, messageId: string) {
    this._updateMessage(chatId, messageId, (msg) => ({
      ...msg,
      pendingApproval: null,
      ...(msg.segments ? { segments: settleTools(msg.segments) } : {}),
    }));
  }

  /**
   * The connection a chat belongs to (`AIChat.connectionId`), if the chat
   * and the connection still exist. No fallback to the active connection.
   */
  private _chatConnection(chatId: string): DatabaseConnection | undefined {
    const connectionId = Object.values(this.state.aiChatsByConnection)
      .flat()
      .find((c) => c.id === chatId)?.connectionId;
    if (!connectionId) return undefined;
    return this.state.connections.find((c) => c.id === connectionId);
  }

  /**
   * Start a turn. The page stops a send itself only where it can't name
   * what Core needs: the chat's connection is gone or not open (no Core
   * connection id), or no model is chosen yet (the message waits).
   */
  private _dispatch(chatId: string, userMessage: { id: string; content: string }) {
    const connection = this._chatConnection(chatId);
    const assistant = (fields: Partial<AIMessage>): AIMessage => ({
      id: crypto.randomUUID(),
      chatId,
      role: "assistant",
      content: "",
      timestamp: new Date(),
      ...fields,
    });

    if (!connection) {
      this._setMessages(chatId, [
        ...this._getMessages(chatId),
        assistant({ content: m.ai_error_connection_removed() }),
      ]);
      return;
    }
    if (!connection.activeAIProviderId || !connection.activeAIModel) {
      this._setMessages(chatId, [
        ...this._getMessages(chatId),
        assistant({ pendingModelSelection: userMessage.content }),
      ]);
      return;
    }
    if (!connection.providerConnectionId) {
      this._setMessages(chatId, [
        ...this._getMessages(chatId),
        assistant({ error: m.ai_error_not_connected({ name: connection.name }) }),
      ]);
      return;
    }

    // A turn still streaming (here or in another chat) stops first; Core
    // stores what it had.
    if (this.turn) this.cancelAIStream();
    this.forgetStaleAllowAll();

    const reply = assistant({ segments: [] });
    this._setMessages(chatId, [...this._getMessages(chatId), reply]);
    const turn: RunningTurn = {
      chatId,
      streamId: crypto.randomUUID(),
      assistantMessageId: reply.id,
      connection,
      controller: new AbortController(),
    };
    this.turn = turn;
    this.state.isAIStreaming = true;
    this.state.aiStreamingChatId = chatId;
    void this._run(turn, userMessage);
  }

  private async _run(turn: RunningTurn, userMessage: { id: string; content: string }) {
    const { connection, controller } = turn;
    try {
      const events = getAi().chat(
        {
          streamId: turn.streamId,
          chatId: turn.chatId,
          connectionId: connection.providerConnectionId!,
          userMessage,
          assistantMessageId: turn.assistantMessageId,
          approval: this.isAllowAll(connection.id) ? "allowAll" : "ask",
          clientTools: true,
          providerId: connection.activeAIProviderId ?? null,
        },
        controller.signal,
      );
      for await (const event of events) {
        if (controller.signal.aborted) break;
        this._apply(turn, event);
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        void log.error("[AI] The turn failed:", error instanceof Error ? error.name : "error");
        this._fail(turn, "UNKNOWN", error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (this.turn === turn) {
        this.turn = null;
        this.state.isAIStreaming = false;
        this.state.aiStreamingChatId = null;
      }
      if (controller.signal.reason !== CHAT_DELETED) {
        void this.aiChatManager.afterTurn(turn.chatId);
      }
    }
  }

  /** One of Core's events, shown on the turn's reply. */
  private _apply(turn: RunningTurn, event: AiEvent) {
    const { chatId, assistantMessageId: id } = turn;
    const update = (updater: (msg: AIMessage) => AIMessage) =>
      this._updateMessage(chatId, id, updater);
    switch (event.type) {
      case "started":
        return;
      case "text":
        return update((msg) => ({
          ...msg,
          content: msg.content + event.delta,
          segments: appendText(msg.segments ?? [], event.delta),
        }));
      case "toolCall":
        return update((msg) => ({
          ...msg,
          segments: startTool(msg.segments ?? [], event),
        }));
      case "toolDone":
        return update((msg) => ({
          ...msg,
          segments: finishTool(msg.segments ?? [], event),
        }));
      case "approvalRequired":
        return this._askApproval(turn, event.callId, event.sql);
      case "clientTool":
        void this._clientTool(turn, event.callId, event.name, event.input);
        return;
      case "done":
        this.aiChatManager.applyTurnRows(chatId, event.messages, event.seq);
        this._settleReply(chatId, id);
        if (event.stop === "maxTokens") update((msg) => ({ ...msg, truncated: true }));
        // Core cut it for its size (probe F2); the stored row says so too.
        if (event.stop === "tooLong") update((msg) => ({ ...msg, cut: true }));
        this.aiChatManager.updateChatTimestamp(chatId);
        return;
      case "error":
        if (event.messages && event.seq) {
          this.aiChatManager.applyTurnRows(chatId, event.messages, event.seq);
          this.aiChatManager.updateChatTimestamp(chatId);
        } else if (LOST_TURN_CODES.has(event.code)) {
          this.aiChatManager.refetchAfterTurnFor(chatId);
        }
        this._fail(turn, event.code, event.message);
        return;
    }
  }

  /** The turn ended with `code`: said on the reply (a full chat, in its banner). */
  private _fail(turn: RunningTurn, code: string, message: string) {
    const { chatId, assistantMessageId: id } = turn;
    this._settleReply(chatId, id);
    if (code === CANCELLED) return;
    if (code === "CHAT_FULL") {
      this.aiChatManager.markFull(chatId, true);
      return;
    }
    this._updateMessage(chatId, id, (msg) => ({ ...msg, error: aiErrorText(code, message) }));
  }

  /** Answer the turn (after a microtask: never from inside an event handler, Decision 17). */
  private _respond(turn: RunningTurn, callId: string, decision: AiDecision) {
    queueMicrotask(() => {
      if (turn.controller.signal.aborted) return;
      getAi()
        .respond(turn.streamId, callId, decision)
        .catch((error: unknown) => {
          // NOT_FOUND: the turn ended meanwhile. Logged by name only.
          void log.warn(
            "[AI] Answering the turn failed:",
            error instanceof Error ? error.name : "error",
          );
        });
    });
  }

  /** The approval card for one query: answered once, through `respond`. */
  private _askApproval(turn: RunningTurn, callId: string, sql: string) {
    const { chatId, assistantMessageId: id, connection } = turn;
    let answered = false;
    /** The card's own box, as it shows now. */
    const ticked = () => {
      const card = this._getMessages(chatId).find((msg) => msg.id === id)?.pendingApproval;
      return card?.id === `${turn.streamId}:${callId}` && card.allowAllTicked;
    };
    const decide = (decision: "allow" | "deny" | "allowAll") => {
      if (answered || turn.controller.signal.aborted) return;
      answered = true;
      if (decision === "allowAll") {
        const now = this.state.connections.find((c) => c.id === connection.id) ?? connection;
        this.aiAllowAllByConnection = {
          ...this.aiAllowAllByConnection,
          [connection.id]: allowAllTarget(now),
        };
      }
      this._updateMessage(chatId, id, (msg) => ({
        ...msg,
        pendingApproval: null,
        segments:
          decision === "deny" ? (msg.segments ?? []) : resumeTool(msg.segments ?? [], callId),
      }));
      this._respond(turn, callId, decision);
    };
    this._updateMessage(chatId, id, (msg) => ({
      ...msg,
      segments: waitTool(msg.segments ?? [], callId),
      pendingApproval: {
        // A card per call of this turn (a later turn's `call_1` is another card).
        id: `${turn.streamId}:${callId}`,
        query: sql,
        connectionName: connection.name,
        connectionType: connection.type,
        approve: () => decide(ticked() ? "allowAll" : "allow"),
        deny: () => decide("deny"),
        allowAll: () => decide("allowAll"),
        allowAllTicked: false,
        setAllowAllTicked: (value: boolean) =>
          this._updateMessage(chatId, id, (msg) =>
            msg.pendingApproval?.id === `${turn.streamId}:${callId}`
              ? { ...msg, pendingApproval: { ...msg.pendingApproval, allowAllTicked: value } }
              : msg,
          ),
      },
    }));
  }

  /**
   * A dashboard tool: only the page can run it (open tabs, the active
   * connection). Core checked its arguments and its query; the page runs
   * today's handler and answers with its result. Stop answers nothing.
   */
  private async _clientTool(turn: RunningTurn, callId: string, name: string, input: unknown) {
    const { connection, controller } = turn;
    const args =
      typeof input === "object" && input !== null ? (input as Record<string, unknown>) : {};
    let result: string;
    if (WIDGET_WRITE_TOOLS.has(name) && this.state.activeConnectionId !== connection.id) {
      result = JSON.stringify({ error: this._widgetConnectionRefusal(connection) });
    } else {
      result = await handleDashboardToolCall(
        name,
        args,
        this._dashboardCallbacks(connection.id),
        (query) => readOnlyError(query, connection.type),
        controller.signal,
      );
    }
    if (controller.signal.aborted) return;
    this._respond(turn, callId, { result });
  }

  /** Why a widget change is refused while another connection is active. */
  private _widgetConnectionRefusal(connection: DatabaseConnection): string {
    const active = this.state.activeConnection;
    const change = `switch back to "${connection.name}" to change this dashboard`;
    return active
      ? `The active connection is "${active.name}"; ${change}`
      : `No connection is active; ${change}`;
  }

  /**
   * The dashboard tools' callbacks for a chat on `connectionId`. A widget
   * runs against the active connection; saving awaits persistence, so the
   * first run checks again and is skipped after a switch (the widget runs
   * read-only on the next refresh). `signal` is the AI's Stop.
   */
  private _dashboardCallbacks(connectionId: string) {
    const stillActive = () => this.state.activeConnectionId === connectionId;
    return {
      onCreateDashboard: async (name: string) => {
        const dashboard = await this.dashboardManager.createDashboard(name, {
          renameIfTaken: true,
        });
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

  setActiveView(view: ActiveViewType) {
    this.state.activeView = view;
    const projectId = this.state.activeProjectId;
    if (projectId) this.state.activeViewByProject[projectId] = view;
    this.schedulePersistence(this.state.activeProjectId);
  }
}
