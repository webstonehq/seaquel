import { getStorage } from "$lib/storage";
import { getKeyringService } from "$lib/services/keyring";
import { DEFAULT_AI_SETTINGS, type AISettings, type AIProvider } from "$lib/types/ai";
import { log } from "$lib/utils/logger";
import { NotLoadedError } from "$lib/storage/load-guard";

const ANTHROPIC_API_VERSION = "2023-06-01";

const AI_SETTINGS_KEY = "aiSettings";

class AISettingsStore {
  settings = $state<AISettings>({ ...DEFAULT_AI_SETTINGS });
  /**
   * True once the stored settings were read. Saving writes the whole record,
   * so until then a save would replace the stored providers with defaults.
   */
  private loaded = false;

  getProvider(id: string): AIProvider | null {
    return this.settings.providers.find((p) => p.id === id) ?? null;
  }

  /** Loads the stored settings. A failed read leaves the store unloaded. */
  async initialize(): Promise<void> {
    let raw: string | null;
    try {
      raw = await getStorage().appState.get(AI_SETTINGS_KEY);
    } catch (err) {
      void log.error("[AI] Failed to load AI settings; saving is off until they load:", err);
      return;
    }
    this.loaded = true;
    if (raw) {
      try {
        const parsed = JSON.parse(raw);
        // Migration: strip legacy fields from old AIProviderConfig shape
        const providers = (parsed.providers ?? []).map(
          (p: AIProvider & { model?: string; provider?: string }) => {
            const {
              model: _model,
              provider,
              ...rest
            } = p as AIProvider & { model?: string; provider?: string };
            return { ...rest, type: rest.type ?? provider ?? "anthropic" } as AIProvider;
          },
        );
        this.settings = { ...DEFAULT_AI_SETTINGS, ...parsed, providers };
      } catch (err) {
        void log.error("[AI] Failed to parse saved AI settings, using defaults:", err);
      }
    }
  }

  /**
   * Before a change: the settings must have loaded, or load now. Throws
   * `NotLoadedError` if they can't, so the change isn't saved over them.
   */
  private async ensureLoaded(): Promise<void> {
    if (!this.loaded) await this.initialize();
    if (!this.loaded) throw new NotLoadedError("AI settings");
  }

  private async persistSettings(settings: AISettings): Promise<void> {
    if (!this.loaded) throw new NotLoadedError("AI settings");
    this.settings = settings;
    await getStorage().appState.set(AI_SETTINGS_KEY, JSON.stringify(settings));
  }

  async addProvider(config: AIProvider, apiKey?: string): Promise<void> {
    await this.ensureLoaded();
    const providers = [...this.settings.providers, config];
    await this.persistSettings({ ...this.settings, providers });
    if (apiKey) {
      await getKeyringService().setAIApiKeyForProvider(config.id, apiKey);
    }
  }

  async updateProvider(config: AIProvider, apiKey?: string): Promise<void> {
    await this.ensureLoaded();
    const providers = this.settings.providers.map((p) => (p.id === config.id ? config : p));
    await this.persistSettings({ ...this.settings, providers });
    if (apiKey !== undefined) {
      const keyring = getKeyringService();
      if (apiKey === "") {
        await keyring.deleteAIApiKeyForProvider(config.id);
      } else {
        await keyring.setAIApiKeyForProvider(config.id, apiKey);
      }
    }
  }

  async deleteProvider(id: string): Promise<void> {
    await this.ensureLoaded();
    const providers = this.settings.providers.filter((p) => p.id !== id);
    await this.persistSettings({ ...this.settings, providers });
    await getKeyringService().deleteAIApiKeyForProvider(id);
  }

  async setEnabled(enabled: boolean): Promise<void> {
    await this.ensureLoaded();
    await this.persistSettings({ ...this.settings, enabled });
  }

  async savePrivacySettings(
    patch: Pick<AISettings, "shareSchemaGlobally" | "shareDataGlobally">,
  ): Promise<void> {
    await this.ensureLoaded();
    await this.persistSettings({ ...this.settings, ...patch });
  }

  /**
   * Fetch the /models endpoint for a provider, handling auth headers for both
   * Anthropic and OpenAI-compatible providers.
   */
  private async fetchProviderModels(config: AIProvider, apiKey: string): Promise<Response | null> {
    if (config.type === "anthropic") {
      return fetch("https://api.anthropic.com/v1/models", {
        headers: {
          "x-api-key": apiKey,
          "anthropic-version": ANTHROPIC_API_VERSION,
        },
      });
    }
    const baseUrl = (config.baseUrl ?? "").replace(/\/$/, "");
    if (!baseUrl) return null;
    const headers: Record<string, string> = {};
    if (apiKey) headers["Authorization"] = `Bearer ${apiKey}`;
    return fetch(`${baseUrl}/models`, { headers });
  }

  async testConnection(providerId: string): Promise<boolean> {
    const config = this.settings.providers.find((p) => p.id === providerId);
    if (!config) return false;
    const apiKey = (await getKeyringService().getAIApiKeyForProvider(config.id)) ?? "";
    if (config.type === "anthropic" && !apiKey) return false;
    try {
      const res = await this.fetchProviderModels(config, apiKey);
      return res?.ok ?? false;
    } catch (err) {
      void log.error("[AI] testConnection error:", err);
      return false;
    }
  }

  async fetchModels(providerId: string): Promise<string[]> {
    const config = this.settings.providers.find((p) => p.id === providerId);
    if (!config) return [];
    const apiKey = (await getKeyringService().getAIApiKeyForProvider(config.id)) ?? "";
    if (config.type === "anthropic" && !apiKey) return [];
    try {
      const res = await this.fetchProviderModels(config, apiKey);
      if (!res?.ok) return [];
      const data = await res.json();
      return (data.data as Array<{ id: string }>).map((e) => e.id);
    } catch (err) {
      void log.error("[AI] fetchModels error:", err);
      return [];
    }
  }
}

export const aiSettingsStore = new AISettingsStore();
