/**
 * `CoreAi`: the `AiService` over Core's `ai` group (phase 6, Task 7).
 *
 * - `chat` is a stream (`ai.chat`, `CoreClient.stream`), `respond`,
 *   `generate`, `models` and `test` are unary calls.
 * - Keys (Decision 7, Q1): on web the page sends the provider's key from
 *   the vault with each call, and the provider it read it for (Core
 *   refuses it for any other, `AI_PROVIDER_CHANGED`), read only when the vault holds one for that
 *   provider (a keyless provider never unlocks it). On the desktop there
 *   is no vault: the page sends none and never reads one, and Core reads
 *   the keychain. The demo's vault is the visitor's session keys
 *   (`$lib/services/session-keys`, Q2 B): in page memory, sent the same way.
 * - A call Core refuses rejects with its `CoreCallError` (`code`).
 */
import { getCoreClient, type CoreClient, cancelledEvent } from "$lib/core";
import { CoreCallError } from "$lib/storage/rust-client";
import { aiKeyVault, type AiKeyVault } from "$lib/services/keyring";
import type { AiDecision } from "$lib/types/generated/AiDecision";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { AiRequest } from "$lib/types/generated/AiRequest";
import type { AiResponse } from "$lib/types/generated/AiResponse";
import type { ChatParams } from "$lib/types/generated/ChatParams";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { AiChatRequest, AiGenerateRequest, AiService } from "./index";

export type { AiKeyVault };

/** A turn whose key the vault couldn't give (a cancelled unlock, say). */
const VAULT_LOCKED = "VAULT_LOCKED";

type Method = AiResponse["method"];
type ResultOf<M extends Method> = Extract<AiResponse, { method: M }>["result"];

export class CoreAi implements AiService {
  constructor(
    private readonly client: () => CoreClient = getCoreClient,
    private readonly vault: () => AiKeyVault | null = aiKeyVault,
  ) {}

  /**
   * The provider's key from the vault (web), or none. Throws when it can't
   * be read. `quiet`: an unlock this starts isn't announced (a send: the
   * toast would cover the Stop button, probe F1).
   */
  private async keyFor(
    providerId: string | null | undefined,
    quiet = false,
  ): Promise<string | undefined> {
    const vault = this.vault();
    if (!vault || !providerId) return undefined;
    if (!(await vault.hasAIApiKeyForProvider(providerId))) return undefined;
    return (
      (await vault.getAIApiKeyForProvider(providerId, quiet ? { quiet } : undefined)) || undefined
    );
  }

  private async call<M extends Method>(request: AiRequest & { method: M }): Promise<ResultOf<M>> {
    const response = await this.client().call({ method: "ai", params: request } as CoreRequest);
    if (response?.method !== "ai" || response.result?.method !== request.method) {
      // Never echo the request: it may carry a key.
      throw new CoreCallError({
        code: "PROTOCOL_ERROR",
        message: `expected an ai ${request.method} response`,
      });
    }
    return response.result.result as ResultOf<M>;
  }

  chat(request: AiChatRequest, signal?: AbortSignal): AsyncIterable<AiEvent> {
    const client = this.client;
    const keyFor = (id: string | null) => this.keyFor(id, true);
    return (async function* () {
      const { providerId, ...turn } = request;
      let apiKey: string | undefined;
      try {
        apiKey = await keyFor(providerId);
      } catch {
        // The vault's message can name the scope; say only that it's locked.
        yield {
          type: "error",
          code: VAULT_LOCKED,
          message: "The vault didn't give this provider's API key.",
        } satisfies AiEvent;
        return;
      }
      if (signal?.aborted) {
        yield cancelledEvent("The turn was cancelled") as AiEvent;
        return;
      }
      // A key always names its provider: Core refuses it for any other
      // (`AI_PROVIDER_CHANGED`, review I1).
      const params: ChatParams = apiKey && providerId ? { ...turn, apiKey, providerId } : turn;
      yield* client().stream(
        { method: "ai", params: { method: "chat", params } },
        signal ? { signal } : {},
      );
    })();
  }

  async respond(streamId: string, callId: string, decision: AiDecision): Promise<void> {
    await this.call({ method: "respond", params: { streamId, callId, decision } });
  }

  async generate(request: AiGenerateRequest): Promise<string> {
    const { providerId, ...params } = request;
    const apiKey = await this.keyFor(providerId);
    const { sql } = await this.call({
      method: "generate",
      params: apiKey && providerId ? { ...params, apiKey, providerId } : params,
    });
    return sql;
  }

  async models(providerId: string): Promise<string[]> {
    const apiKey = await this.keyFor(providerId);
    return this.call({
      method: "models",
      params: apiKey ? { providerId, apiKey } : { providerId },
    });
  }

  async test(providerId: string): Promise<void> {
    const apiKey = await this.keyFor(providerId);
    await this.call({ method: "test", params: apiKey ? { providerId, apiKey } : { providerId } });
  }
}
