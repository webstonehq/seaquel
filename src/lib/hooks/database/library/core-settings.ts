/**
 * `SettingsService` over Seaquel Core (desktop and web): the `settings` RPC
 * group, through the page's `RustStorageClient`, so its writes join the
 * write queue and land in the order the page issued them. Core checks each
 * value, rewrites records from its stored copy (so another window's change
 * isn't lost), writes an AI provider's API key to the keychain inside the
 * call on the desktop, and emits `storageChanged`.
 */
import type {
  RustStorageClient,
  SettingsMethod,
  SettingsParams,
  SettingsResult,
} from "$lib/storage/rust-client";
import type {
  AiProviderDraft,
  AiProviderPatch,
  AiSettingsPatch,
  ImportSource,
  SettingKey,
  SettingsService,
} from "./types";

/** What `CoreSettings` needs of the storage client: its queued `settings` call. */
export type SettingsCaller = Pick<RustStorageClient, "settings">;

export class CoreSettings implements SettingsService {
  /** `getCaller` is read per call, so the page's client can be swapped (tests). */
  constructor(private readonly getCaller: () => SettingsCaller) {}

  private call<M extends SettingsMethod>(
    method: M,
    params: SettingsParams<M>,
  ): Promise<SettingsResult<M>> {
    return this.getCaller().settings(method, params);
  }

  getSetting(key: SettingKey) {
    return this.call("settingGet", { key });
  }
  setSetting(key: SettingKey, value: string | null) {
    return this.call("settingSet", { key, value });
  }

  getAiSettings() {
    return this.call("aiSettingsGet", undefined);
  }
  patchAiSettings(patch: AiSettingsPatch) {
    return this.call("aiSettingsPatch", { patch });
  }
  createAiProvider(provider: AiProviderDraft, apiKey?: string) {
    return this.call(
      "aiProviderCreate",
      apiKey === undefined ? { provider } : { provider, apiKey },
    );
  }
  updateAiProvider(id: string, patch: AiProviderPatch, apiKey?: string | null) {
    return this.call(
      "aiProviderUpdate",
      apiKey === undefined ? { id, patch } : { id, patch, apiKey },
    );
  }
  removeAiProvider(id: string) {
    return this.call("aiProviderRemove", { id });
  }
  aiProviderHasKey(id: string) {
    return this.call("aiProviderHasKey", { id });
  }

  getThemes() {
    return this.call("themesGet", undefined);
  }
  setThemePreferences(lightThemeId: string, darkThemeId: string) {
    return this.call("themePreferencesSet", { lightThemeId, darkThemeId });
  }
  createUserTheme(theme: unknown) {
    return this.call("userThemeCreate", { theme });
  }
  updateUserTheme(id: string, theme: unknown) {
    return this.call("userThemeUpdate", { id, theme });
  }
  removeUserTheme(id: string) {
    return this.call("userThemeRemove", { id });
  }

  getOnboarding() {
    return this.call("onboardingGet", undefined);
  }
  patchOnboarding(patch: Record<string, unknown>) {
    return this.call("onboardingPatch", { patch });
  }

  listTutorial() {
    return this.call("tutorialList", undefined);
  }
  saveTutorial(lessonId: string, challengeId: string, state: string | null) {
    return this.call("tutorialSave", { lessonId, challengeId, state });
  }
  removeTutorialLesson(lessonId: string) {
    return this.call("tutorialRemoveLesson", { lessonId });
  }
  resetTutorial() {
    return this.call("tutorialReset", undefined);
  }

  getImportState(source: ImportSource) {
    return this.call("importStateGet", { source });
  }
  saveImportState(
    source: ImportSource,
    hasOfferedImport: boolean,
    lastCheckTimestamp: string | null,
  ) {
    return this.call("importStateSave", { source, hasOfferedImport, lastCheckTimestamp });
  }
}
