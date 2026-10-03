/**
 * The phase 6 ts-baseline's page-view values, replayed through the page on
 * Core's events (Task 7). Task 4's replay (`seaquel-core/tests/ai_replay.rs`)
 * pins what Core sends, stores and refuses for `page.json`; it left the
 * page's own view to this task: `messages` (what the chat shows),
 * `allowAllAfter`, the approval cards (`approvals`), and `turns.json`'s
 * `dashboardCalls` (what the dashboard tools did in the page).
 *
 * A fake `AiService` plays Core: each turn emits the events Core would for
 * the case's expected outcome (its stored rows and their `parts`, the
 * approvals it asks for, its error), stores the rows through the recording
 * library and waits for the page's `respond`. The steps (`send`, the
 * approval decisions, Stop, `activate`) drive the real `UIStateManager`
 * and `AIChatManager`. Where `changes.json` says a page view is `$absent`
 * until Task 7 (a failed turn), `CORRECTIONS` is the view now; the
 * ts-baseline README's "Corrections" records the same.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardManager } from "../dashboard-manager.svelte.js";
import type { DashboardTabManager } from "../dashboard-tabs.svelte.js";
import type { ChatMessageDraft } from "../library/types";
import type { AIMessage } from "$lib/types";
import type { PersistedAIMessage } from "$lib/types/generated/PersistedAIMessage";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => true,
  isWeb: () => false,
  isDemo: () => false,
}));
vi.mock("$lib/stores/ai-settings.svelte", () => ({
  aiSettingsStore: { settings: { shareSchemaGlobally: true, shareDataGlobally: false } },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({
  toast: { success: vi.fn(), info: vi.fn(), warning: vi.fn() },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("../state.svelte.js");
const { AIChatManager } = await import("../ai-chat-manager.svelte.js");
const { UIStateManager } = await import("../ui-state.svelte.js");
const { StateRestorationManager } = await import("../state-restoration.svelte.js");
const { RecordingLibrary } = await import("../library/recording-library");
const { setLibrary } = await import("../library/index");
const { setAi } = await import("./index");
const { FakeAi } = await import("./testing");
type Turn = import("./testing").FakeTurn;

const FIXTURES = join(process.cwd(), "crates/seaquel-ai/tests/fixtures/ts-baseline");
const read = <T>(f: string) => JSON.parse(readFileSync(join(FIXTURES, f), "utf8")) as T;

type Json = Record<string, unknown>;
interface Row {
  role: "user" | "assistant";
  content: string;
  dashboardId?: string;
  pendingModelSelection?: string;
  parts?: Json[];
}
interface Step {
  send?: string;
  approvals?: string[];
  activate?: string;
  stopAtFirstChunk?: boolean;
}
interface PageCase {
  name: string;
  input: {
    connections: Array<Json & { id: string; name: string }>;
    active: string;
    global: { shareSchemaGlobally: boolean; shareDataGlobally: boolean };
    steps: Step[];
    history?: Row[];
    removeConnection?: boolean;
    noKey?: boolean;
    dataOffAfterFirstQuery?: boolean;
  };
  messages: Row[];
  stored: Row[] | null;
  approvals: Array<{ query: string; connectionName: string }>;
  allowAllAfter: boolean;
}
type Changes = Record<string, { expected: Record<string, unknown> }>;

const pageCases = read<PageCase[]>("page.json");
const changes = read<Changes>("changes.json");

/** The page views `changes.json` left `$absent` until Task 7, as they are now. */
const CORRECTIONS: Record<string, Row[]> = {
  "page/provider-500": [
    { role: "user", content: "Hello there, assistant" },
    {
      role: "assistant",
      content: "",
      error: "AI provider error: Internal server error",
    } as Row,
  ],
  "page/error-after-partial": [
    { role: "user", content: "Explain the orders table" },
    { role: "assistant", content: "Half an ans", error: "AI provider error: Overloaded" } as Row,
  ],
  "page/no-api-key": [
    { role: "user", content: "Which tables are the largest?" },
    {
      role: "assistant",
      content: "",
      error: "No API key is set for this provider. Add your key in Settings → AI.",
    } as Row,
  ],
};

const isAbsent = (v: unknown) => typeof v === "object" && v !== null && "$absent" in v;
const QUERY_TOOLS = new Set(["run_query", "explain_query", "run_saved_query"]);
const CLIENT_TOOLS = new Set([
  "create_dashboard",
  "add_widget",
  "get_dashboard",
  "update_widget",
  "remove_widget",
]);

/** A row as the page view compares it. */
function viewOf(m: AIMessage): Row & { error?: string } {
  const row: Row & { error?: string } = { role: m.role, content: m.content };
  if (m.dashboardId !== undefined) row.dashboardId = m.dashboardId;
  if (m.pendingModelSelection !== undefined) row.pendingModelSelection = m.pendingModelSelection;
  if (m.error) row.error = m.error;
  return row;
}
const storedView = (r: Row) => ({
  role: r.role,
  content: r.content,
  ...(r.dashboardId !== undefined ? { dashboardId: r.dashboardId } : {}),
});

const settle = async () => {
  for (let i = 0; i < 4; i++) await new Promise((r) => setTimeout(r, 0));
};

let library: InstanceType<typeof RecordingLibrary>;

/** The dashboard tools' page side, recording what they did. */
function dashboardsStub() {
  const calls: Json[] = [];
  const manager = {
    createDashboard: vi.fn(async (name: string) => {
      calls.push({ call: "create", name });
      return { id: "dash-new", name };
    }),
    addWidget: vi.fn(async (dashboardId: string, widget: Json) => {
      const { id: _id, ...rest } = widget;
      calls.push({ call: "addWidget", dashboardId, widget: rest });
    }),
    executeWidget: vi.fn(async () => {}),
    getDashboard: vi.fn(() => null),
    updateWidget: vi.fn(async () => {}),
    removeWidget: vi.fn(async () => {}),
  };
  return { manager, calls };
}

function page(c: { connections: Json[]; active: string }) {
  const state = new DatabaseState();
  state.activeProjectId = "p";
  state.connections = c.connections.map((conn) => ({
    providerConnectionId: `pc-${String(conn.id)}`,
    ...conn,
  })) as never;
  state.activeConnectionIdByProject = { p: c.active };
  const restoration = new StateRestorationManager(state);
  let ui!: InstanceType<typeof UIStateManager>;
  const chats = new AIChatManager(
    state,
    (chatId) => restoration.loadAIChatMessages(chatId),
    (chatId) => ui.abortStreamFor(chatId),
  );
  const dashboards = dashboardsStub();
  ui = new UIStateManager(
    state,
    () => {},
    chats,
    dashboards.manager as unknown as DashboardManager,
    { add: vi.fn() } as unknown as DashboardTabManager,
  );
  return { state, ui, chats, dashboards };
}

/** Core's two writes and the rows `done` carries. */
async function store(turn: Turn, reply: Omit<ChatMessageDraft, "id" | "role" | "timestamp">) {
  const { chatId, userMessage, assistantMessageId } = turn.request;
  const user: ChatMessageDraft = {
    id: userMessage.id,
    role: "user",
    content: userMessage.content,
    timestamp: new Date(Date.now()).toISOString(),
  };
  await library.putChatMessages(chatId, [user]);
  const row: ChatMessageDraft = {
    id: assistantMessageId,
    role: "assistant",
    timestamp: new Date(Date.now() + 1).toISOString(),
    ...reply,
  };
  const { seq } = await library.putChatMessages(chatId, [row]);
  return { messages: [user, row].map((m) => ({ ...m, chatId })) as PersistedAIMessage[], seq };
}

interface TurnPlan {
  /** The stored reply, when the case pins it. */
  reply: Row | null;
  /** The approvals Core asks for in this turn, in order. */
  asks: Array<{ query: string }>;
  /** Core's error ending: after storing the rows (`stored`) or before (a refusal). */
  error: { code: string; message: string; stored: boolean } | null;
  /** Stop once the first text shows. */
  stopAtFirstChunk: boolean;
  /** The page's answers to client tools, by call id. */
  answers: Map<string, string>;
}

/** Plays Core's side of one turn from its plan. */
function coreTurn(plan: TurnPlan) {
  return async (turn: Turn) => {
    const reply = plan.reply;
    const asks = [...plan.asks];
    let text = "";
    const say = (t: string) => {
      if (!t) return;
      text += t;
      turn.emit({ type: "text", delta: t });
    };
    const ask = async (callId: string, sql: string) => {
      turn.emit({ type: "approvalRequired", callId, sql });
      return Promise.race([turn.answer(callId), turn.stopped().then(() => null)]);
    };
    /** Stop: Core stores the reply with what streamed, and sends nothing more (Q8). */
    const stopped = async () => {
      await store(turn, { content: text });
    };
    turn.emit({ type: "started", providerKind: "anthropic", model: "model-1" });
    const parts = (reply?.parts ?? []) as Json[];
    if (reply && parts.length === 0) say(reply.content);
    for (const part of parts) {
      if (turn.cancelled) return stopped();
      if (part.type === "text") {
        say(String(part.text));
        continue;
      }
      const callId = String(part.callId);
      const name = String(part.name);
      const input = part.input as Json;
      const sql = typeof input?.sql === "string" ? input.sql : undefined;
      turn.emit({ type: "toolCall", callId, name, ...(sql !== undefined ? { sql } : {}) });
      if (CLIENT_TOOLS.has(name)) {
        turn.emit({ type: "clientTool", callId, name, input });
        const d = await Promise.race([turn.answer(callId), turn.stopped().then(() => null)]);
        if (d === null) return stopped();
        plan.answers.set(callId, (d as { result: string }).result);
      } else if (QUERY_TOOLS.has(name) && asks[0]?.query === sql) {
        asks.shift();
        const d = await ask(callId, sql!);
        if (d === null) return stopped();
      }
      turn.emit({
        type: "toolDone",
        callId,
        ok: part.ok === true,
        ...(part.ok === true ? {} : { code: String(part.result).split(":")[0] }),
      });
    }
    // Approvals the stored reply doesn't hold (Stop at the card, or a turn
    // whose rows another chat holds).
    let n = 0;
    for (const a of asks) {
      const callId = `call_x${++n}`;
      turn.emit({ type: "toolCall", callId, name: "run_query", sql: a.query });
      const d = await ask(callId, a.query);
      if (d === null) return stopped();
      turn.emit({ type: "toolDone", callId, ok: d !== "deny" });
    }
    if (plan.stopAtFirstChunk) {
      await turn.stopped();
      return stopped();
    }
    if (plan.error) {
      if (!plan.error.stored) {
        turn.emit({ type: "error", code: plan.error.code, message: plan.error.message });
        return;
      }
      const { messages, seq } = await store(turn, { content: text });
      turn.emit({
        type: "error",
        code: plan.error.code,
        message: plan.error.message,
        messages,
        seq,
      });
      return;
    }
    const { messages, seq } = await store(turn, {
      content: reply?.content ?? text,
      ...(reply?.dashboardId !== undefined ? { dashboardId: reply.dashboardId } : {}),
      ...(reply?.parts ? { parts: reply.parts } : {}),
    } as never);
    turn.emit({ type: "done", messages, seq, stop: "end" });
  };
}

/** Core's history: no pending-model row, and not the message it waited on. */
function coreHistory(history: Row[]): Row[] {
  const out: Row[] = [];
  for (const r of history) {
    if (r.pendingModelSelection !== undefined) {
      if (out.at(-1)?.role === "user") out.pop();
      continue;
    }
    out.push(r);
  }
  return out;
}

beforeEach(() => {
  library = new RecordingLibrary();
});

describe("page.json's page view, on Core's events", () => {
  for (const c of pageCases) {
    const name = `page/${c.name}`;
    const expected = changes[name]?.expected ?? {};
    it(c.name, async () => {
      for (const conn of c.input.connections) library.seedConnection(conn.id);
      setLibrary(library);
      const ai = new FakeAi();
      setAi(ai);
      const { state, ui, chats } = page(c.input);
      const stored = (expected.stored as Row[] | undefined) ?? c.stored ?? [];
      const approvals = (expected.approvals as PageCase["approvals"] | undefined) ?? c.approvals;
      const error = expected.error as { code: string; message: string } | undefined;
      const storeCalls = expected.storeCalls as number;

      // Seeded history: what the page shows, and what Core has stored.
      if (c.input.history) {
        const chatId = (await chats.createChat())!;
        const shown: AIMessage[] = c.input.history.map((r, i) => ({
          id: `h${i}`,
          chatId,
          role: r.role,
          content: r.content,
          timestamp: new Date(Date.UTC(2020, 0, 1, 0, 0, i)),
          ...(r.dashboardId ? { dashboardId: r.dashboardId } : {}),
          ...(r.pendingModelSelection !== undefined
            ? { pendingModelSelection: r.pendingModelSelection }
            : {}),
        }));
        const keep = new Set(coreHistory(c.input.history));
        const kept = shown.filter((_, i) => keep.has(c.input.history![i]));
        await library.putChatMessages(
          chatId,
          kept.map((m) => ({
            id: m.id,
            role: m.role,
            content: m.content,
            timestamp: m.timestamp.toISOString(),
            ...(m.dashboardId ? { dashboardId: m.dashboardId } : {}),
          })),
        );
        state.aiMessagesByChat = { ...state.aiMessagesByChat, [chatId]: shown };
        state.aiMessagesStored.set(chatId, new Set(kept.map((m) => m.id)));
      }
      if (c.input.removeConnection) {
        await chats.createChat();
        state.connections = [];
      }

      // Each send's plan: the stored turn rows of the final chat, in order.
      const turnRows = stored.slice(coreHistory(c.input.history ?? []).length);
      const sends = c.input.steps.filter((s) => s.send !== undefined);
      const lastActivate = c.input.steps.map((s) => s.activate !== undefined).lastIndexOf(true);
      const askQueue = [...approvals];
      const plans: TurnPlan[] = [];
      let rowAt = 0;
      c.input.steps.forEach((step, i) => {
        if (step.send === undefined) return;
        const inFinalChat = i > lastActivate;
        const reply = inFinalChat && turnRows[rowAt + 1] ? turnRows[rowAt + 1] : null;
        if (inFinalChat) rowAt += 2;
        // The cards this turn shows: those its calls (or, without stored
        // calls, its approval steps) take from the expected list.
        const callSqls = ((reply?.parts ?? []) as Json[])
          .filter((p) => p.type === "tool")
          .map((p) => (p.input as Json)?.sql);
        const asks: Array<{ query: string }> = [];
        while (askQueue.length > 0) {
          const head = askQueue[0];
          const mine = reply?.parts
            ? callSqls.includes(head.query)
            : asks.length < (step.approvals?.length ?? 0);
          if (!mine) break;
          asks.push(askQueue.shift()!);
        }
        plans.push({
          reply,
          asks,
          error: error ? { ...error, stored: storeCalls > 0 } : null,
          stopAtFirstChunk: !!step.stopAtFirstChunk,
          answers: new Map(),
        });
      });
      expect(plans).toHaveLength(sends.length);
      ai.scripts = plans.map(coreTurn);

      // Drive the steps.
      const shownCards: Array<{ query: string; connectionName: string }> = [];
      const seen = new Set<string>();
      for (const step of c.input.steps) {
        if (step.activate) {
          state.activeConnectionIdByProject = { p: step.activate };
          continue;
        }
        const decisions = [...(step.approvals ?? [])];
        const before = ai.chats.length;
        await ui.sendAIMessage(step.send!);
        await settle();
        if (ai.chats.length === before) continue; // The page stopped it.
        for (let guard = 0; guard < 200 && state.isAIStreaming; guard++) {
          const active = state.aiStreamingChatId!;
          const last = state.aiMessagesByChat[active]?.at(-1);
          const card = last?.pendingApproval;
          if (card && !seen.has(card.id)) {
            seen.add(card.id);
            shownCards.push({ query: card.query, connectionName: card.connectionName });
            const decision = decisions.shift() ?? "allow";
            if (decision === "stop") ui.cancelAIStream();
            else if (decision === "allowAll") card.allowAll();
            else if (decision === "deny") card.deny();
            else card.approve();
          } else if (step.stopAtFirstChunk && last?.content) {
            ui.cancelAIStream();
          }
          await settle();
        }
        expect(state.isAIStreaming).toBe(false);
      }

      // The page view.
      const finalChat = state.activeAIChatId!;
      const want =
        expected.messages === undefined
          ? c.messages
          : isAbsent(expected.messages)
            ? CORRECTIONS[name]
            : (expected.messages as Row[]);
      expect(want, `${name} has a page view`).toBeDefined();
      expect((state.aiMessagesByChat[finalChat] ?? []).map(viewOf)).toEqual(want);
      expect(shownCards).toEqual(approvals);
      const allowAll = (expected.allowAllAfter as boolean | undefined) ?? c.allowAllAfter;
      expect(ui.hasAnyAllowAll()).toBe(allowAll);
      if (c.name === "allow-all-other-connection") {
        // Per connection (bug 10): conn-1's Allow all doesn't cover conn-2.
        expect(ui.isAllowAll("conn-1") && !ui.isAllowAll("conn-2")).toBe(true);
      }

      // The fake played Core as the case pins it: the final chat's stored
      // rows are the expected ones (Core's replay pins `parts` and writes).
      if (storeCalls > 0 || stored.length > 0) {
        const rows = (await library.listChatMessages(finalChat)).value.messages;
        expect(rows.map((r) => storedView(r as unknown as Row))).toEqual(stored.map(storedView));
      }
    });
  }
});

/** A turn's expected request shape: the calls each round made and what they got back. */
interface CallRecord {
  id: string;
  name: string;
  input: Json;
  result: string;
  isError: boolean;
}
function callsOf(lastBody: Json): CallRecord[] {
  const messages = lastBody.messages as Json[];
  const results = new Map<string, { content: string; isError: boolean }>();
  for (const m of messages) {
    if (m.role === "tool") {
      const content = String(m.content);
      results.set(String(m.tool_call_id), {
        content: content.replace(/^Error: /, ""),
        isError: content.startsWith("Error: "),
      });
    } else if (m.role === "user" && Array.isArray(m.content)) {
      for (const b of m.content as Json[]) {
        if (b.type === "tool_result") {
          results.set(String(b.tool_use_id), {
            content: String(b.content),
            isError: b.is_error === true,
          });
        }
      }
    }
  }
  const out: CallRecord[] = [];
  for (const m of messages) {
    if (m.role !== "assistant") continue;
    const blocks = Array.isArray(m.content)
      ? (m.content as Json[]).filter((b) => b.type === "tool_use")
      : [];
    const calls: Array<{ id: string; name: string; input: Json }> = blocks.map((b) => ({
      id: String(b.id),
      name: String(b.name),
      input: b.input as Json,
    }));
    for (const t of (m.tool_calls as Json[] | undefined) ?? []) {
      const fn = t.function as Json;
      calls.push({
        id: String(t.id),
        name: String(fn.name),
        input: JSON.parse(String(fn.arguments)) as Json,
      });
    }
    for (const call of calls) {
      const r = results.get(call.id)!;
      out.push({ ...call, result: r.content, isError: r.isError });
    }
  }
  return out;
}

interface TurnCase {
  name: string;
  input: { engine: string; shareSchema: boolean; shareData: boolean; dashboards?: boolean };
  dashboardCalls?: Json[];
}

describe("turns.json's dashboardCalls, through the page's client tools", () => {
  const cases = read<TurnCase[]>("turns.json").filter((c) => c.input.dashboards);
  for (const c of cases) {
    const name = `turns/${c.name}`;
    it(c.name, async () => {
      library.seedConnection("conn-1");
      setLibrary(library);
      const ai = new FakeAi();
      setAi(ai);
      const conn = {
        id: "conn-1",
        name: "Local",
        type: c.input.engine,
        activeAIProviderId: "prov-1",
        activeAIModel: "model-1",
        aiShareSchema: c.input.shareSchema,
        aiShareData: c.input.shareData,
      };
      const { ui, state, dashboards } = page({ connections: [conn], active: "conn-1" });
      const requests = changes[name].expected.requests as Array<{ body: Json }>;
      const calls = callsOf(requests.at(-1)!.body);
      const answered = new Map<string, string>();
      ai.scripts = [
        async (turn) => {
          for (const call of calls) {
            turn.emit({ type: "toolCall", callId: call.id, name: call.name });
            // Core's own refusal (`CODE: message`) never reaches the page.
            const core = /^[A-Z_]+: /.test(call.result);
            if (CLIENT_TOOLS.has(call.name) && !core) {
              turn.emit({
                type: "clientTool",
                callId: call.id,
                name: call.name,
                input: call.input,
              });
              const d = (await turn.answer(call.id)) as { result: string };
              answered.set(call.id, d.result);
            }
            turn.emit({ type: "toolDone", callId: call.id, ok: !call.isError });
          }
          const { messages, seq } = await store(turn, { content: "" });
          turn.emit({ type: "done", messages, seq, stop: "end" });
        },
      ];
      await ui.sendAIMessage("How many users signed up this month?");
      for (let i = 0; i < 20 && state.isAIStreaming; i++) await settle();
      expect(state.isAIStreaming).toBe(false);

      const want = (c.dashboardCalls ?? []).filter((d) => d.call !== "created");
      expect(dashboards.calls).toEqual(want);
      // `create_dashboard`'s answer is what Core sent the model.
      for (const call of calls.filter((x) => x.name === "create_dashboard")) {
        expect(answered.get(call.id)).toBe(call.result);
      }
      const created = (c.dashboardCalls ?? []).find((d) => d.call === "created");
      if (created) {
        const answer = [...answered.values()][0];
        expect(JSON.parse(answer)).toEqual({ dashboard_id: created.dashboardId });
      }
    });
  }
});
