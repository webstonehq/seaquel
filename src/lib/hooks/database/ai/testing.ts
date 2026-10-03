/**
 * A fake `AiService` for the page's tests (phase 6 Task 7): each `chat`
 * runs a scripted turn that emits `AiEvent`s as Core would and waits for
 * the page's `respond`. Stop (the signal) ends the turn as the transports
 * do: no terminal event from the turn, and the stream ends `CANCELLED`.
 * No model, no Core, no key.
 */
import { StreamQueue } from "$lib/core/client";
import type { AiDecision } from "$lib/types/generated/AiDecision";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { AiChatRequest, AiGenerateRequest, AiService } from "./index";

/** One running turn as the script sees it. */
export class FakeTurn {
  private readonly waiters = new Map<string, (d: AiDecision) => void>();
  private readonly queue: StreamQueue<AiEvent>;
  /** Set once the page stopped the turn. */
  cancelled = false;
  private readonly cancelListeners: Array<() => void> = [];

  constructor(
    readonly request: AiChatRequest,
    queue: StreamQueue<AiEvent>,
  ) {
    this.queue = queue;
  }

  emit(event: AiEvent): void {
    if (!this.cancelled) this.queue.push(event);
  }

  /** The page's answer to `callId` (an approval or a client tool). */
  answer(callId: string): Promise<AiDecision> {
    return new Promise((resolve) => this.waiters.set(callId, resolve));
  }

  /** Resolves when the page stops the turn. */
  stopped(): Promise<void> {
    if (this.cancelled) return Promise.resolve();
    return new Promise((resolve) => this.cancelListeners.push(resolve));
  }

  /** @internal */
  respond(callId: string, decision: AiDecision): boolean {
    const waiter = this.waiters.get(callId);
    if (!waiter) return false;
    this.waiters.delete(callId);
    waiter(decision);
    return true;
  }

  /** @internal */
  cancel(): void {
    if (this.cancelled) return;
    this.cancelled = true;
    this.waiters.clear();
    for (const l of this.cancelListeners.splice(0)) l();
  }

  /** Callers still waited for. */
  get waiting(): number {
    return this.waiters.size;
  }
}

export type TurnScript = (turn: FakeTurn) => Promise<void> | void;

export class FakeAi implements AiService {
  readonly chats: AiChatRequest[] = [];
  readonly responses: Array<{ streamId: string; callId: string; decision: AiDecision }> = [];
  readonly generates: AiGenerateRequest[] = [];
  readonly turns: FakeTurn[] = [];
  /** What the next turns do, in order; the last one repeats. */
  scripts: TurnScript[] = [];
  generateAnswer: (req: AiGenerateRequest) => Promise<string> = async () => "SELECT 1";
  modelsAnswer: (providerId: string) => Promise<string[]> = async () => [];
  testAnswer: (providerId: string) => Promise<void> = async () => {};

  chat(request: AiChatRequest, signal?: AbortSignal): AsyncIterable<AiEvent> {
    this.chats.push(request);
    let turn: FakeTurn | null = null;
    const queue = new StreamQueue<AiEvent>(() => turn?.cancel());
    turn = new FakeTurn(request, queue);
    this.turns.push(turn);
    const script = this.scripts.length > 1 ? this.scripts.shift()! : this.scripts[0];
    const onAbort = () => {
      turn!.cancel();
      queue.pushError({ type: "error", code: "CANCELLED", message: "The query was cancelled" });
    };
    if (signal?.aborted) onAbort();
    signal?.addEventListener("abort", onAbort, { once: true });
    void (async () => {
      try {
        if (script) await script(turn!);
      } finally {
        // A script that ends without `done`/`error` was cancelled.
        queue.finish();
      }
    })();
    return queue;
  }

  async respond(streamId: string, callId: string, decision: AiDecision): Promise<void> {
    this.responses.push({ streamId, callId, decision });
    const turn = this.turns.find((t) => t.request.streamId === streamId);
    if (!turn?.respond(callId, decision)) {
      throw Object.assign(new Error("NOT_FOUND: No such call"), { code: "NOT_FOUND" });
    }
  }

  generate(request: AiGenerateRequest): Promise<string> {
    this.generates.push(request);
    return this.generateAnswer(request);
  }

  models(providerId: string): Promise<string[]> {
    return this.modelsAnswer(providerId);
  }

  test(providerId: string): Promise<void> {
    return this.testAnswer(providerId);
  }
}

/** Lets queued microtasks and the turn's events run. */
export const settle = () => new Promise((r) => setTimeout(r, 0));
