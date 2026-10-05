import { getSettings } from "$lib/hooks/database/library/index";
import { RowSeqs } from "$lib/hooks/database/library/seqs";
import type { ChangeSeq } from "$lib/hooks/database/library/types";
import { aiKeyVault, getKeyringService } from "$lib/services/keyring";
import { sessionKeys } from "$lib/services/session-keys";
import { getAi } from "$lib/hooks/database/ai/index";
import { errorCode } from "$lib/core/client";
import {
  DEFAULT_AI_SETTINGS,
  type AISettings,
  type AIProvider,
  type AIProviderType,
} from "$lib/types/ai";
import { isTauri } from "$lib/utils/environment";
import { isFeatureEnabled } from "$lib/features";
import { log } from "$lib/utils/logger";
import { onStoredChange } from "./settings-sync";
import { m } from "$lib/paraglide/messages.js";

/** The record's key for the `seq` rule. */
const RECORD = "aiSettings";

/**
 * Web: the provider was saved (Core answered with its id) but the vault
 * didn't take its API key. The form edits that provider from here, so a
 * retry stores the key and makes no second provider.
 */
export class ProviderKeyNotSavedError extends Error {
  readonly providerId: string;
  constructor(providerId: string, cause: unknown) {
    super(
      m.settings_ai_key_not_saved({
        message: cause instanceof Error ? cause.message : String(cause),
      }),
      { cause },
    );
    this.name = "ProviderKeyNotSavedError";
    this.providerId = providerId;
  }
}

/** A new provider as the settings form makes it; Core gives it its id. */
export type AIProviderInput = Omit<AIProvider, "id">;

/**
 * The record Core answers (legacy fields already cleaned) as the store holds
 * it: the known fields typed, and any other field (a newer release's) kept
 * as it is, on the record and on each provider.
 */
function toSettings(value: unknown): AISettings {
  const record = (value ?? {}) as Record<string, unknown>;
  const providers: unknown[] = Array.isArray(record.providers) ? record.providers : [];
  const flag = (key: keyof AISettings, fallback: boolean) =>
    typeof record[key] === "boolean" ? (record[key] as boolean) : fallback;
  return {
    ...record,
    enabled: flag("enabled", DEFAULT_AI_SETTINGS.enabled),
    shareSchemaGlobally: flag("shareSchemaGlobally", DEFAULT_AI_SETTINGS.shareSchemaGlobally),
    shareDataGlobally: flag("shareDataGlobally", DEFAULT_AI_SETTINGS.shareDataGlobally),
    providers: providers
      .filter((p): p is Record<string, unknown> => typeof p === "object" && p !== null)
      .map(
        (p) =>
          ({
            ...p,
            id: typeof p.id === "string" ? p.id : "",
            name: typeof p.name === "string" ? p.name : "",
            type: (p.type ?? "anthropic") as AIProviderType,
          }) as AIProvider,
      ),
  };
}

/** A base URL as stored: an empty one is none. */
function baseUrlOf(url: string | undefined): string | undefined {
  return url ? url : undefined;
}

/**
 * The AI settings (phase 5d-2): each change is one targeted
 * `settings` call (a provider added, changed or removed, or the flags), so
 * Core rewrites the record from its stored copy and another window's
 * change isn't lost. Every answer holds the whole record, applied by the
 * `seq` rule; another window's change is read again at once.
 *
 * API keys: on the desktop the key goes with the Core
 * call, which writes the keychain; on the web the vault keeps it in the
 * browser, written here after Core answers (Core deletes a removed
 * provider's vault rows). The demo keeps the visitor's key in this page's
 * memory for the session (`sessionKeys`), never in storage, and
 * forgets it on reload; Core stores the provider without it. The page never reads a
 * key on the desktop (phase 6): whether one is saved is a
 * targeted `aiProviderHasKey` call. The model list and the provider test are
 * Core's (`ai.models`, `ai.test`).
 */
export class AISettingsStore {
  settings = $state<AISettings>({ ...DEFAULT_AI_SETTINGS });
  /** True once the stored settings were read. */
  private loaded = false;
  private loading: Promise<void> | null = null;
  private readonly seqs = new RowSeqs();

  constructor() {
    onStoredChange("aiSettings", () => (this.loaded ? this.read() : undefined));
  }

  /**
   * Whether the assistant is offered: this build has it (`aiAssistant`; the demo has it too since phase 6)
   * and the user hasn't turned it off. The header's
   * toggle, the command palette, the editor's inline prompt, the right
   * panel and Settings' AI group all read this.
   */
  get available(): boolean {
    return isFeatureEnabled("aiAssistant") && this.settings.enabled;
  }

  getProvider(id: string): AIProvider | null {
    return this.settings.providers.find((p) => p.id === id) ?? null;
  }

  /** Loads the stored settings. A failed read leaves the defaults showing. */
  initialize(): Promise<void> {
    this.loading = this.read().catch((err) => {
      void log.error("[AI] Failed to load AI settings:", err);
    });
    return this.loading;
  }

  private async read(): Promise<void> {
    const { value, seq } = await getSettings().getAiSettings();
    this.loaded = true;
    this.apply(value, seq);
  }

  private apply(value: unknown, seq: ChangeSeq): void {
    if (this.seqs.take(RECORD, seq)) this.settings = toSettings(value);
  }

  /**
   * Whether a key is saved for the provider, never the key: on the desktop
   * one `aiProviderHasKey` (one keychain read), asked when the
   * settings form opens a provider; on web whether the vault holds its row
   * (nothing decrypted). A read that fails says none.
   */
  async hasKey(providerId: string): Promise<boolean> {
    const vault = aiKeyVault();
    if (vault) return vault.hasAIApiKeyForProvider(providerId);
    if (!isTauri()) return false;
    try {
      return (await getSettings().aiProviderHasKey(providerId)).value;
    } catch (err) {
      void log.warn(`[AI] Reading whether a key is saved failed (${errorCode(err) ?? "unknown"})`);
      return false;
    }
  }

  /**
   * Before a change: a load still out is waited for, and one that never
   * ran is made, so the settings on screen are the stored ones. The change
   * itself is targeted, so it's sent whether or not the load worked.
   */
  private async ready(): Promise<void> {
    if (!this.loading) void this.initialize();
    await this.loading;
  }

  /** Adds a provider and answers the id Core gave it. */
  async addProvider(input: AIProviderInput, apiKey?: string): Promise<string> {
    await this.ready();
    const desktop = isTauri();
    const draft = {
      name: input.name,
      type: input.type,
      ...(baseUrlOf(input.baseUrl) ? { baseUrl: input.baseUrl } : {}),
    };
    const { value, seq } = await getSettings().createAiProvider(
      draft,
      desktop && apiKey ? apiKey : undefined,
    );
    this.apply(value.settings, seq);
    if (import.meta.env.VITE_BUILD_TARGET === "demo") {
      if (apiKey) sessionKeys().set(value.id, apiKey);
    } else if (!desktop && apiKey) {
      try {
        await getKeyringService().setAIApiKeyForProvider(value.id, apiKey);
      } catch (error) {
        throw new ProviderKeyNotSavedError(value.id, error);
      }
    }
    return value.id;
  }

  /**
   * Changes a provider: only the fields that differ from the one shown.
   * `apiKey`: left out keeps it, `""` deletes it, anything else sets it.
   */
  async updateProvider(config: AIProvider, apiKey?: string): Promise<void> {
    await this.ready();
    const existing = this.getProvider(config.id);
    const patch: { name?: string; type?: AIProviderType; baseUrl?: string | null } = {};
    if (config.name !== existing?.name) patch.name = config.name;
    if (config.type !== existing?.type) patch.type = config.type;
    const url = baseUrlOf(config.baseUrl);
    if (url !== baseUrlOf(existing?.baseUrl)) patch.baseUrl = url ?? null;
    const desktop = isTauri();
    const key = apiKey === undefined ? undefined : apiKey === "" ? null : apiKey;
    const { value, seq } = await getSettings().updateAiProvider(
      config.id,
      patch,
      desktop ? key : undefined,
    );
    this.apply(value, seq);
    if (import.meta.env.VITE_BUILD_TARGET === "demo") {
      if (key === null) sessionKeys().delete(config.id);
      else if (key !== undefined) sessionKeys().set(config.id, key);
    } else if (!desktop && key !== undefined) {
      const keyring = getKeyringService();
      if (key === null) await keyring.deleteAIApiKeyForProvider(config.id);
      else await keyring.setAIApiKeyForProvider(config.id, key);
    }
  }

  /** Removes a provider; Core deletes its API key too (keychain or vault). */
  async deleteProvider(id: string): Promise<void> {
    await this.ready();
    const { value, seq } = await getSettings().removeAiProvider(id);
    this.apply(value, seq);
    if (import.meta.env.VITE_BUILD_TARGET === "demo") sessionKeys().delete(id);
  }

  async setEnabled(enabled: boolean): Promise<void> {
    await this.ready();
    const { value, seq } = await getSettings().patchAiSettings({ enabled });
    this.apply(value, seq);
  }

  async savePrivacySettings(
    patch: Pick<AISettings, "shareSchemaGlobally" | "shareDataGlobally">,
  ): Promise<void> {
    await this.ready();
    const { value, seq } = await getSettings().patchAiSettings(patch);
    this.apply(value, seq);
  }

  /** Whether the provider answers with its key (`ai.test`). Refusals are logged by code. */
  async testConnection(providerId: string): Promise<boolean> {
    try {
      await getAi().test(providerId);
      return true;
    } catch (err) {
      void log.warn(`[AI] The provider test failed (${errorCode(err) ?? "unknown"})`);
      return false;
    }
  }

  /** The provider's model ids (`ai.models`); none when Core refuses. */
  async fetchModels(providerId: string): Promise<string[]> {
    try {
      return await getAi().models(providerId);
    } catch (err) {
      void log.warn(`[AI] Listing models failed (${errorCode(err) ?? "unknown"})`);
      return [];
    }
  }
}

export const aiSettingsStore = new AISettingsStore();
