/**
 * `TsSettings`: the demo's `SettingsService` (phase 5d-2, Decisions 20 and
 * 26), Core's `settings` rules in TypeScript over the demo's sql.js file,
 * until phase 8.
 *
 * It follows `crates/seaquel-core/src/state.rs` and
 * `seaquel_workspace::state`:
 * - settings are a closed set of keys, each value checked, and `null`
 *   deletes the row; `lastActiveProjectId` is written by `windowActivate`
 *   only, and `connectionStringSecretsNotice` can only be cleared;
 * - the AI settings record is read with the legacy provider cleanup and
 *   today's fallback (a record that doesn't read is the defaults), and each
 *   call rewrites it from the stored copy, keeping fields it doesn't know;
 * - user themes are one row each, their `id`, `createdAt` and `updatedAt`
 *   set here; removing a theme in use resets that preference;
 * - onboarding is the six defaults with the stored fields over them, and a
 *   patch merges its top-level fields;
 * - tutorial progress and import state are one row each.
 *
 * The demo has no keychain: an API key is refused (`NOT_SUPPORTED`), as the
 * web workspace refuses it. No limits. Calls run one at a time; the change
 * sequence has one epoch per instance.
 */
import type { SqliteDatabase } from "$lib/storage/sqlite-types";
import {
  AI_PROVIDER_NOT_FOUND,
  INVALID_ARGUMENT,
  LibraryCallError,
  THEME_NOT_FOUND,
  type AiProviderCreated,
  type AiProviderDraft,
  type AiProviderPatch,
  type AiSettingsPatch,
  type ChangeSeq,
  type ImportSource,
  type ImportState,
  type Seqd,
  type SettingKey,
  type SettingsService,
  type Themes,
  type ThemeCreated,
  type TutorialProgress,
} from "./types";

type Row = Record<string, unknown>;
type Obj = Record<string, unknown>;
type Statement = { sql: string; params?: unknown[] };

const SETTING_KEYS: readonly SettingKey[] = [
  "editorKeybindingMode",
  "pending_changes_enabled",
  "skippedUpdateVersion",
  "query_version_limit",
  "dashboard_version_limit",
  "license_nudge",
  "lastActiveProjectId",
  "connectionStringSecretsNotice",
];
const AI_SETTINGS_KEY = "aiSettings";
const PROVIDER_TYPES = ["anthropic", "openai-compatible"];
const IMPORT_SOURCES: readonly string[] = ["tableplus", "dbeaver"];
const DEFAULT_LIGHT_THEME = "default-light";
const DEFAULT_DARK_THEME = "default-dark";
const ONBOARDING_DEFAULTS: Obj = {
  isFirstRun: true,
  userBackground: "none",
  hasCompletedWizard: false,
  showWizardHints: true,
  dismissedHints: [],
  learnEnabled: true,
};

function invalid(message: string): LibraryCallError {
  return new LibraryCallError(INVALID_ARGUMENT, message);
}

function isObject(v: unknown): v is Obj {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** A JSON object from `text`, or `null`. */
function parseObject(text: unknown): Obj | null {
  if (typeof text !== "string") return null;
  try {
    const value: unknown = JSON.parse(text);
    return isObject(value) ? value : null;
  } catch {
    return null;
  }
}

function hasNul(v: string): boolean {
  return v.includes("\u0000");
}

/** As Core's `key_for_message`. */
function keyText(key: string): string {
  return key.length <= 64 && !/\p{Cc}/u.test(key)
    ? `The key ${JSON.stringify(key)}`
    : `A key of ${new TextEncoder().encode(key).length} bytes`;
}

function parseKey(key: string): SettingKey {
  if (!(SETTING_KEYS as readonly string[]).includes(key)) {
    throw invalid(`${keyText(key)} isn't a setting that can be read or written here.`);
  }
  return key as SettingKey;
}

/** `settingSet`'s checks (`check_setting_set`). */
function checkSettingSet(key: string, value: string | null): SettingKey {
  const k = parseKey(key);
  const refuse = (why: string) => invalid(`${keyText(k)} ${why}`);
  if (k === "lastActiveProjectId") {
    throw refuse("is written when a window activates a project, not set directly.");
  }
  if (value === null) return k;
  let ok: boolean;
  switch (k) {
    case "editorKeybindingMode":
      ok = ["default", "vim", "emacs"].includes(value);
      break;
    case "pending_changes_enabled":
      ok = value === "true" || value === "false";
      break;
    case "skippedUpdateVersion":
      ok = !hasNul(value);
      break;
    case "query_version_limit":
    case "dashboard_version_limit":
      ok = /^\d{1,6}$/.test(value) && Number(value) <= 100_000;
      break;
    case "license_nudge":
      ok = !hasNul(value) && parseObject(value) !== null;
      break;
    case "connectionStringSecretsNotice":
      throw refuse("can only be cleared (set to null).");
  }
  if (!ok) throw refuse("can't hold this value.");
  return k;
}

// ── AI settings ──

/** `{ ...rest, type: rest.type ?? provider ?? "anthropic" }` without `model` and `provider`. */
function cleanProvider(p: unknown): Obj {
  if (!isObject(p)) return { type: "anthropic" };
  const { model: _model, provider, ...rest } = p;
  if (rest.type === undefined || rest.type === null) {
    rest.type = provider === undefined || provider === null ? "anthropic" : provider;
  }
  return rest;
}

/** Core's `read_ai_settings`: the defaults, the stored fields over them, providers cleaned. */
export function readAiSettings(raw: string | null): Obj {
  const defaults: Obj = {
    enabled: true,
    providers: [],
    shareSchemaGlobally: true,
    shareDataGlobally: false,
  };
  if (!raw) return defaults;
  const parsed = parseObject(raw);
  if (!parsed) return defaults;
  const stored = parsed.providers;
  let providers: Obj[];
  if (stored === undefined || stored === null) providers = [];
  else if (Array.isArray(stored) && !stored.some((p) => p === null)) {
    providers = stored.map(cleanProvider);
  } else return defaults;
  return { ...defaults, ...parsed, providers };
}

function checkName(name: unknown, what: string): void {
  if (typeof name !== "string" || name.trim() === "") {
    throw invalid(`A ${what} needs a name.`);
  }
  if (hasNul(name)) throw invalid("A value can't contain a NUL character.");
}

function checkProviderType(ty: unknown): void {
  if (typeof ty !== "string" || !PROVIDER_TYPES.includes(ty)) {
    throw invalid("An AI provider's type must be anthropic or openai-compatible.");
  }
}

function providerNotFound(): LibraryCallError {
  return new LibraryCallError(AI_PROVIDER_NOT_FOUND, "AI provider not found.");
}

// ── Themes ──

/** A user theme's stored JSON (`user_theme_json`). */
function userThemeJson(theme: Obj, id: string, createdAt: string, updatedAt: string): string {
  const { id: _id, createdAt: _c, updatedAt: _u, ...rest } = theme;
  const out: Obj = { ...rest, id };
  if (!("isBuiltIn" in out)) out.isBuiltIn = false;
  out.createdAt = createdAt;
  out.updatedAt = updatedAt;
  return JSON.stringify(out);
}

function checkUserTheme(theme: unknown): Obj {
  if (!isObject(theme)) throw invalid("A theme is a JSON object.");
  if (typeof theme.name !== "string") throw invalid("A theme needs a name.");
  if (hasNul(theme.name)) throw invalid("A value can't contain a NUL character.");
  if (theme.isBuiltIn === true) throw invalid("Built-in themes aren't stored.");
  return theme;
}

function checkThemeId(id: string): void {
  if (id === "") throw invalid("A theme id can't be empty.");
  if (hasNul(id)) throw invalid("A value can't contain a NUL character.");
}

// ── Onboarding ──

function readOnboarding(stored: unknown): Obj {
  return { ...ONBOARDING_DEFAULTS, ...parseObject(stored) };
}

function checkOnboardingPatch(patch: unknown): Obj {
  if (!isObject(patch)) throw invalid("An onboarding change is a JSON object.");
  for (const [k, v] of Object.entries(patch)) {
    let ok: boolean;
    switch (k) {
      case "isFirstRun":
      case "hasCompletedWizard":
      case "showWizardHints":
      case "learnEnabled":
        ok = typeof v === "boolean";
        break;
      case "userBackground":
        ok = typeof v === "string" && !hasNul(v);
        break;
      case "dismissedHints":
        ok = Array.isArray(v) && v.every((h) => typeof h === "string" && !hasNul(h));
        break;
      default:
        throw invalid("An onboarding change names a field onboarding doesn't have.");
    }
    if (!ok) throw invalid("An onboarding field has a value of the wrong type.");
  }
  return patch;
}

export interface TsSettingsOptions {
  now?: () => Date;
  epoch?: string;
}

export class TsSettings implements SettingsService {
  private readonly now: () => Date;
  private readonly epoch: string;
  private n = 0;
  private queue: Promise<unknown> = Promise.resolve();

  constructor(
    private readonly db: SqliteDatabase,
    options: TsSettingsOptions = {},
  ) {
    this.now = options.now ?? (() => new Date());
    this.epoch = options.epoch ?? crypto.randomUUID();
  }

  // -------- Plumbing --------

  private run<T>(fn: () => Promise<T>): Promise<T> {
    const next = this.queue.then(fn);
    this.queue = next.catch(() => {});
    return next;
  }

  private seq(): ChangeSeq {
    return { epoch: this.epoch, n: this.n };
  }

  private async write(statements: Statement[]): Promise<ChangeSeq> {
    if (statements.length > 0) await this.db.transaction(statements);
    this.n += 1;
    return this.seq();
  }

  private iso(): string {
    return this.now().toISOString();
  }

  private async appState(key: string): Promise<string | null> {
    const [row] = await this.db.query<Row>("SELECT value FROM app_state WHERE key = ?", [key]);
    return typeof row?.value === "string" ? row.value : null;
  }

  private setAppState(key: string, value: string): Statement {
    return {
      sql: `INSERT INTO app_state (key, value) VALUES (?, ?)
            ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
      params: [key, value],
    };
  }

  // -------- Settings --------

  getSetting(key: SettingKey): Promise<Seqd<string | null>> {
    return this.run(async () => {
      const k = parseKey(key);
      return { value: await this.appState(k), seq: this.seq() };
    });
  }

  setSetting(key: SettingKey, value: string | null): Promise<Seqd<string | null>> {
    return this.run(async () => {
      const k = checkSettingSet(key, value);
      const seq = await this.write([
        value === null
          ? { sql: "DELETE FROM app_state WHERE key = ?", params: [k] }
          : this.setAppState(k, value),
      ]);
      return { value, seq };
    });
  }

  // -------- AI settings --------

  private async readAi(): Promise<Obj> {
    return readAiSettings(await this.appState(AI_SETTINGS_KEY));
  }

  /** Rewrites the stored record with `change` applied, from the stored copy. */
  private async rewriteAi(change: (s: Obj) => void): Promise<Seqd<Obj>> {
    const settings = await this.readAi();
    change(settings);
    const seq = await this.write([this.setAppState(AI_SETTINGS_KEY, JSON.stringify(settings))]);
    return { value: settings, seq };
  }

  private refuseKey(apiKey: string | null | undefined): void {
    if (apiKey !== undefined) {
      throw new LibraryCallError(
        "NOT_SUPPORTED",
        "Saving an API key with the provider isn't available here.",
      );
    }
  }

  getAiSettings(): Promise<Seqd<unknown>> {
    return this.run(async () => ({ value: await this.readAi(), seq: this.seq() }));
  }

  patchAiSettings(patch: AiSettingsPatch): Promise<Seqd<unknown>> {
    return this.run(() =>
      this.rewriteAi((s) => {
        for (const key of ["enabled", "shareSchemaGlobally", "shareDataGlobally"] as const) {
          if (patch[key] !== undefined) s[key] = patch[key];
        }
      }),
    );
  }

  createAiProvider(draft: AiProviderDraft, apiKey?: string): Promise<Seqd<AiProviderCreated>> {
    return this.run(async () => {
      checkName(draft.name, "AI provider");
      checkProviderType(draft.type);
      if (draft.baseUrl !== undefined && hasNul(draft.baseUrl)) {
        throw invalid("A value can't contain a NUL character.");
      }
      this.refuseKey(apiKey);
      const id = crypto.randomUUID();
      const { value, seq } = await this.rewriteAi((s) => {
        const provider: Obj = { id, name: draft.name, type: draft.type };
        if (draft.baseUrl !== undefined) provider.baseUrl = draft.baseUrl;
        s.providers = [...(s.providers as Obj[]), provider];
      });
      return { value: { id, settings: value }, seq };
    });
  }

  updateAiProvider(
    id: string,
    patch: AiProviderPatch,
    apiKey?: string | null,
  ): Promise<Seqd<unknown>> {
    return this.run(async () => {
      if (patch.name !== undefined) checkName(patch.name, "AI provider");
      if (patch.type !== undefined) checkProviderType(patch.type);
      if (typeof patch.baseUrl === "string" && hasNul(patch.baseUrl)) {
        throw invalid("A value can't contain a NUL character.");
      }
      this.refuseKey(apiKey);
      return this.rewriteAi((s) => {
        const providers = s.providers as Obj[];
        const i = providers.findIndex((p) => p.id === id);
        if (i === -1) throw providerNotFound();
        const p = { ...providers[i] };
        if (patch.name !== undefined) p.name = patch.name;
        if (patch.type !== undefined) p.type = patch.type;
        if (patch.baseUrl === null) delete p.baseUrl;
        else if (patch.baseUrl !== undefined) p.baseUrl = patch.baseUrl;
        s.providers = providers.map((q, j) => (j === i ? p : q));
      });
    });
  }

  removeAiProvider(id: string): Promise<Seqd<unknown>> {
    return this.run(() =>
      this.rewriteAi((s) => {
        const providers = s.providers as Obj[];
        if (!providers.some((p) => p.id === id)) throw providerNotFound();
        s.providers = providers.filter((p) => p.id !== id);
      }),
    );
  }

  // -------- Themes --------

  private async readThemes(): Promise<Themes> {
    const [prefs] = await this.db.query<Row>(
      "SELECT light_theme_id, dark_theme_id FROM theme_preferences WHERE id = 1",
    );
    const rows = await this.db.query<Row>("SELECT data FROM user_themes ORDER BY rowid");
    const userThemes: unknown[] = [];
    for (const row of rows) {
      try {
        userThemes.push(JSON.parse(String(row.data)));
      } catch {
        // As Core: a theme that doesn't read is skipped (and kept).
      }
    }
    return {
      preferences: {
        lightThemeId:
          typeof prefs?.light_theme_id === "string" ? prefs.light_theme_id : DEFAULT_LIGHT_THEME,
        darkThemeId:
          typeof prefs?.dark_theme_id === "string" ? prefs.dark_theme_id : DEFAULT_DARK_THEME,
      },
      userThemes,
    };
  }

  private setPreferences(light: string, dark: string): Statement {
    return {
      sql: `INSERT INTO theme_preferences (id, light_theme_id, dark_theme_id) VALUES (1, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
              light_theme_id = excluded.light_theme_id, dark_theme_id = excluded.dark_theme_id`,
      params: [light, dark],
    };
  }

  getThemes(): Promise<Seqd<Themes>> {
    return this.run(async () => ({ value: await this.readThemes(), seq: this.seq() }));
  }

  setThemePreferences(lightThemeId: string, darkThemeId: string): Promise<Seqd<Themes>> {
    return this.run(async () => {
      checkThemeId(lightThemeId);
      checkThemeId(darkThemeId);
      const seq = await this.write([this.setPreferences(lightThemeId, darkThemeId)]);
      return { value: await this.readThemes(), seq };
    });
  }

  createUserTheme(theme: unknown): Promise<Seqd<ThemeCreated>> {
    return this.run(async () => {
      const body = checkUserTheme(theme);
      const now = this.iso();
      const id = `theme-${crypto.randomUUID()}`;
      const seq = await this.write([
        {
          sql: "INSERT INTO user_themes (id, data) VALUES (?, ?)",
          params: [id, userThemeJson(body, id, now, now)],
        },
      ]);
      return { value: { id, themes: await this.readThemes() }, seq };
    });
  }

  updateUserTheme(id: string, theme: unknown): Promise<Seqd<Themes>> {
    return this.run(async () => {
      checkThemeId(id);
      const body = checkUserTheme(theme);
      const [row] = await this.db.query<Row>("SELECT data FROM user_themes WHERE id = ?", [id]);
      if (!row) throw new LibraryCallError(THEME_NOT_FOUND, "Theme not found.");
      const now = this.iso();
      const created = parseObject(row.data)?.createdAt;
      const seq = await this.write([
        {
          sql: "UPDATE user_themes SET data = ? WHERE id = ?",
          params: [userThemeJson(body, id, typeof created === "string" ? created : now, now), id],
        },
      ]);
      return { value: await this.readThemes(), seq };
    });
  }

  removeUserTheme(id: string): Promise<Seqd<Themes>> {
    return this.run(async () => {
      checkThemeId(id);
      const [row] = await this.db.query<Row>("SELECT id FROM user_themes WHERE id = ?", [id]);
      if (!row) throw new LibraryCallError(THEME_NOT_FOUND, "Theme not found.");
      const statements: Statement[] = [
        { sql: "DELETE FROM user_themes WHERE id = ?", params: [id] },
      ];
      const [prefs] = await this.db.query<Row>(
        "SELECT light_theme_id, dark_theme_id FROM theme_preferences WHERE id = 1",
      );
      if (prefs && (prefs.light_theme_id === id || prefs.dark_theme_id === id)) {
        statements.push(
          this.setPreferences(
            prefs.light_theme_id === id ? DEFAULT_LIGHT_THEME : String(prefs.light_theme_id),
            prefs.dark_theme_id === id ? DEFAULT_DARK_THEME : String(prefs.dark_theme_id),
          ),
        );
      }
      const seq = await this.write(statements);
      return { value: await this.readThemes(), seq };
    });
  }

  // -------- Onboarding --------

  private async storedOnboarding(): Promise<unknown> {
    const [row] = await this.db.query<Row>("SELECT data FROM onboarding_state WHERE id = 1");
    return row?.data;
  }

  getOnboarding(): Promise<Seqd<unknown>> {
    return this.run(async () => ({
      value: readOnboarding(await this.storedOnboarding()),
      seq: this.seq(),
    }));
  }

  patchOnboarding(patch: Record<string, unknown>): Promise<Seqd<unknown>> {
    return this.run(async () => {
      const checked = checkOnboardingPatch(patch);
      const merged = { ...readOnboarding(await this.storedOnboarding()), ...checked };
      const seq = await this.write([
        {
          sql: `INSERT INTO onboarding_state (id, data) VALUES (1, ?)
                ON CONFLICT(id) DO UPDATE SET data = excluded.data`,
          params: [JSON.stringify(merged)],
        },
      ]);
      return { value: merged, seq };
    });
  }

  // -------- Tutorial progress --------

  private async readTutorial(): Promise<TutorialProgress[]> {
    const rows = await this.db.query<Row>(
      "SELECT lesson_id, challenge_id, state FROM tutorial_progress ORDER BY rowid",
    );
    return rows.map((r) => ({
      lessonId: String(r.lesson_id),
      challengeId: String(r.challenge_id),
      state: typeof r.state === "string" ? r.state : null,
    }));
  }

  private checkTutorialIds(lessonId: string, challengeId?: string): void {
    for (const id of challengeId === undefined ? [lessonId] : [lessonId, challengeId]) {
      if (typeof id !== "string" || id === "" || hasNul(id)) {
        throw invalid("A tutorial lesson or challenge id is 1 or more characters, no NUL.");
      }
    }
  }

  listTutorial(): Promise<Seqd<TutorialProgress[]>> {
    return this.run(async () => ({ value: await this.readTutorial(), seq: this.seq() }));
  }

  saveTutorial(
    lessonId: string,
    challengeId: string,
    state: string | null,
  ): Promise<Seqd<TutorialProgress[]>> {
    return this.run(async () => {
      this.checkTutorialIds(lessonId, challengeId);
      if (state !== null && hasNul(state)) throw invalid("A value can't contain a NUL character.");
      const seq = await this.write([
        {
          sql: `INSERT INTO tutorial_progress (lesson_id, challenge_id, state) VALUES (?, ?, ?)
                ON CONFLICT(lesson_id, challenge_id) DO UPDATE SET state = excluded.state`,
          params: [lessonId, challengeId, state],
        },
      ]);
      return { value: await this.readTutorial(), seq };
    });
  }

  removeTutorialLesson(lessonId: string): Promise<Seqd<TutorialProgress[]>> {
    return this.run(async () => {
      this.checkTutorialIds(lessonId);
      const seq = await this.write([
        { sql: "DELETE FROM tutorial_progress WHERE lesson_id = ?", params: [lessonId] },
      ]);
      return { value: await this.readTutorial(), seq };
    });
  }

  resetTutorial(): Promise<Seqd<TutorialProgress[]>> {
    return this.run(async () => {
      const seq = await this.write([{ sql: "DELETE FROM tutorial_progress" }]);
      return { value: [], seq };
    });
  }

  // -------- Import state --------

  private checkSource(source: string): void {
    if (!IMPORT_SOURCES.includes(source)) {
      throw invalid("An import source is tableplus or dbeaver.");
    }
  }

  getImportState(source: ImportSource): Promise<Seqd<ImportState | null>> {
    return this.run(async () => {
      this.checkSource(source);
      const [row] = await this.db.query<Row>(
        "SELECT has_offered_import, last_check_timestamp FROM import_state WHERE source = ?",
        [source],
      );
      const value: ImportState | null = row
        ? {
            hasOfferedImport: row.has_offered_import === 1,
            lastCheckTimestamp:
              typeof row.last_check_timestamp === "string" ? row.last_check_timestamp : null,
          }
        : null;
      return { value, seq: this.seq() };
    });
  }

  saveImportState(
    source: ImportSource,
    hasOfferedImport: boolean,
    lastCheckTimestamp: string | null,
  ): Promise<Seqd<ImportState>> {
    return this.run(async () => {
      this.checkSource(source);
      if (lastCheckTimestamp !== null && hasNul(lastCheckTimestamp)) {
        throw invalid("A value can't contain a NUL character.");
      }
      const seq = await this.write([
        {
          sql: `INSERT INTO import_state (source, has_offered_import, last_check_timestamp)
                VALUES (?, ?, ?)
                ON CONFLICT(source) DO UPDATE SET
                  has_offered_import = excluded.has_offered_import,
                  last_check_timestamp = excluded.last_check_timestamp`,
          params: [source, hasOfferedImport ? 1 : 0, lastCheckTimestamp],
        },
      ]);
      return { value: { hasOfferedImport, lastCheckTimestamp }, seq };
    });
  }
}
