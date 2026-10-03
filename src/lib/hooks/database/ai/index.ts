/**
 * The assistant on Core (phase 6, Decision 17): one `AiService` seam the
 * chat panel, the inline prompt and Settings → AI use. `CoreAi` sends each
 * call to Core's `ai` group on the desktop, the web and the demo; there is
 * no TypeScript twin. The page decides nothing about a turn: Core picks
 * what the model is sent and which tools it gets, runs them and stores the
 * turn; the page shows the events and answers approvals and dashboard
 * tools with `respond`.
 */
import type { AiDecision } from "$lib/types/generated/AiDecision";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { ChatParams } from "$lib/types/generated/ChatParams";
import type { GenerateParams } from "$lib/types/generated/GenerateParams";
import { CoreAi } from "./core-ai";

export type { AiDecision, AiEvent };
export { CoreAi, type AiKeyVault } from "./core-ai";

/**
 * One turn as the page asks for it: `ai.chat`'s params without a key, and
 * the provider whose key the web page sends (from the vault).
 */
export interface AiChatRequest extends Omit<ChatParams, "apiKey" | "providerId"> {
  providerId: string | null;
}

/** `ai.generate`'s params without a key, and the provider for the web's key. */
export interface AiGenerateRequest extends Omit<GenerateParams, "apiKey" | "providerId"> {
  providerId: string | null;
}

export interface AiService {
  /**
   * One turn's events, ending with exactly one `done` or `error`. Aborting
   * `signal` (Stop) cancels the turn in Core, which stores what streamed;
   * the iterator then ends with `CANCELLED`.
   */
  chat(request: AiChatRequest, signal?: AbortSignal): AsyncIterable<AiEvent>;
  /** Answer a turn waiting on an approval or a client tool. */
  respond(streamId: string, callId: string, decision: AiDecision): Promise<void>;
  /** The inline prompt's SQL (Decision 18). Rejects with Core's code. */
  generate(request: AiGenerateRequest): Promise<string>;
  /** The provider's model ids. Rejects with Core's code. */
  models(providerId: string): Promise<string[]>;
  /** Whether the provider answers with its key. Rejects with Core's code. */
  test(providerId: string): Promise<void>;
}

let override: AiService | null = null;
let core: AiService | null = null;

/** The page's `AiService`: `CoreAi`, unless a test set another. */
export function getAi(): AiService {
  return override ?? (core ??= new CoreAi());
}

/** Replace the page's `AiService` (tests); `null` goes back to `CoreAi`. */
export function setAi(next: AiService | null): void {
  override = next;
}
