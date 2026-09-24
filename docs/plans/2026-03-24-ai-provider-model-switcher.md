# AI Provider & Model Switcher Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Separate AI providers (credentials) from model selection, with per-connection active model persisted to the DB and a grouped model switcher shown in the AI assistant and query editor.

**Architecture:** `AIProvider` no longer stores a model — it only stores type/credentials/baseURL. `DatabaseConnection` gains `activeAIProviderId` and `activeAIModel`. A new `AiModelSwitcher` component fetches models on demand and renders providers as groups. `sendAIMessage`/`generateSQL` receive provider+model explicitly instead of reading a global active provider.

**Tech Stack:** SvelteKit 5 (Svelte runes), TypeScript, Tailwind CSS v4, bits-ui, paraglide i18n

---

## Task 1: Update `AIProvider` type — remove `model`, rename field

**Files:**
- Modify: `src/lib/types/ai.ts`

### Step 1: Edit the file

Replace the entire file with:

```ts
export type AIProviderType = "anthropic" | "openai-compatible";

export interface AIProvider {
  id: string;
  name: string;
  type: AIProviderType;
  baseUrl?: string; // openai-compatible only
}

export interface AISettings {
  providers: AIProvider[];
  shareSchemaGlobally: boolean;
  shareDataGlobally: boolean;
}

export const DEFAULT_AI_SETTINGS: AISettings = {
  providers: [],
  shareSchemaGlobally: true,
  shareDataGlobally: false,
};
```

Key changes:
- `AIProviderConfig` → `AIProvider`
- `provider` field renamed to `type`
- `model` field removed
- `activeProviderId` removed from `AISettings`

---

## Task 2: Update `AISettingsStore` to match new types

**Files:**
- Modify: `src/lib/stores/ai-settings.svelte.ts`

### Step 1: Rewrite the file

```ts
import type { SqliteDatabase } from "$lib/storage";
import { appStateRepo } from "$lib/storage";
import { getKeyringService } from "$lib/services/keyring";
import { DEFAULT_AI_SETTINGS, type AISettings, type AIProvider } from "$lib/types/ai";

const AI_SETTINGS_KEY = "aiSettings";

class AISettingsStore {
  settings = $state<AISettings>({ ...DEFAULT_AI_SETTINGS });

  getProvider(id: string): AIProvider | null {
    return this.settings.providers.find(p => p.id === id) ?? null;
  }

  async initialize(db: SqliteDatabase): Promise<void> {
    const raw = await appStateRepo.get(db, AI_SETTINGS_KEY);
    if (raw) {
      try {
        const parsed = JSON.parse(raw);
        // Migration: strip legacy fields from old AIProviderConfig shape
        const providers = (parsed.providers ?? []).map((p: AIProvider & { model?: string; provider?: string }) => {
          const { model: _model, provider, ...rest } = p as AIProvider & { model?: string; provider?: string };
          return { ...rest, type: rest.type ?? provider ?? "anthropic" } as AIProvider;
        });
        this.settings = { ...DEFAULT_AI_SETTINGS, ...parsed, providers };
      } catch (err) {
        console.error("[AI] Failed to parse saved AI settings, using defaults:", err);
      }
    }
  }

  private async persistSettings(db: SqliteDatabase, settings: AISettings): Promise<void> {
    this.settings = settings;
    await appStateRepo.set(db, AI_SETTINGS_KEY, JSON.stringify(settings));
  }

  async addProvider(db: SqliteDatabase, config: AIProvider, apiKey?: string): Promise<void> {
    const providers = [...this.settings.providers, config];
    await this.persistSettings(db, { ...this.settings, providers });
    if (apiKey) {
      await getKeyringService().setAIApiKeyForProvider(config.id, apiKey);
    }
  }

  async updateProvider(db: SqliteDatabase, config: AIProvider, apiKey?: string): Promise<void> {
    const providers = this.settings.providers.map(p => p.id === config.id ? config : p);
    await this.persistSettings(db, { ...this.settings, providers });
    if (apiKey !== undefined) {
      const keyring = getKeyringService();
      if (apiKey === "") {
        await keyring.deleteAIApiKeyForProvider(config.id);
      } else {
        await keyring.setAIApiKeyForProvider(config.id, apiKey);
      }
    }
  }

  async deleteProvider(db: SqliteDatabase, id: string): Promise<void> {
    const providers = this.settings.providers.filter(p => p.id !== id);
    await this.persistSettings(db, { ...this.settings, providers });
    await getKeyringService().deleteAIApiKeyForProvider(id);
  }

  async savePrivacySettings(db: SqliteDatabase, patch: Pick<AISettings, "shareSchemaGlobally" | "shareDataGlobally">): Promise<void> {
    await this.persistSettings(db, { ...this.settings, ...patch });
  }

  async testConnection(providerId: string): Promise<boolean> {
    const config = this.settings.providers.find(p => p.id === providerId);
    if (!config) return false;
    const apiKey = await getKeyringService().getAIApiKeyForProvider(config.id);
    if (!apiKey) return false;
    try {
      if (config.type === "anthropic") {
        const res = await fetch("https://api.anthropic.com/v1/models", {
          headers: {
            "x-api-key": apiKey,
            "anthropic-version": "2023-06-01",
          },
        });
        return res.ok;
      } else {
        const baseUrl = (config.baseUrl ?? "").replace(/\/$/, "");
        const res = await fetch(`${baseUrl}/models`, {
          headers: { Authorization: `Bearer ${apiKey}` },
        });
        return res.ok;
      }
    } catch {
      return false;
    }
  }

  async fetchModels(providerId: string): Promise<string[]> {
    const config = this.settings.providers.find(p => p.id === providerId);
    if (!config) return [];
    const apiKey = await getKeyringService().getAIApiKeyForProvider(config.id);
    if (!apiKey) return [];
    try {
      if (config.type === "anthropic") {
        const res = await fetch("https://api.anthropic.com/v1/models", {
          headers: { "x-api-key": apiKey, "anthropic-version": "2023-06-01" },
        });
        if (!res.ok) return [];
        const data = await res.json();
        return (data.data as Array<{ id: string }>).map(e => e.id);
      } else {
        const baseUrl = (config.baseUrl ?? "").replace(/\/$/, "");
        if (!baseUrl) return [];
        const headers: Record<string, string> = { Authorization: `Bearer ${apiKey}` };
        const res = await fetch(`${baseUrl}/models`, { headers });
        if (!res.ok) return [];
        const data = await res.json();
        return (data.data as Array<{ id: string }>).map(e => e.id);
      }
    } catch {
      return [];
    }
  }
}

export const aiSettingsStore = new AISettingsStore();
```

Key changes:
- `AIProviderConfig` → `AIProvider`, `provider` → `type` throughout
- Removed `activeProvider` getter, removed `setActiveProvider`
- Added `fetchModels(providerId)` method (extracted from settings-dialog)
- Added migration in `initialize` to strip old `model`/`provider` fields

---

## Task 3: Add `activeAIProviderId` and `activeAIModel` to connection types

**Files:**
- Modify: `src/lib/types/database.ts` (add fields to `DatabaseConnection`)
- Modify: `src/lib/hooks/database/types.ts` (add fields to `PersistedConnection`)

### Step 1: Add to `DatabaseConnection` in `src/lib/types/database.ts`

After the `aiShareData` field, add:

```ts
  /** Active AI provider ID for this connection */
  activeAIProviderId?: string;
  /** Active AI model for this connection */
  activeAIModel?: string;
```

### Step 2: Add to `PersistedConnection` in `src/lib/hooks/database/types.ts`

After the `aiShareData` field, add:

```ts
  /** Active AI provider ID for this connection */
  activeAIProviderId?: string;
  /** Active AI model for this connection */
  activeAIModel?: string;
```

---

## Task 4: Persist and load `activeAIProviderId`/`activeAIModel` in the persistence manager

**Files:**
- Modify: `src/lib/hooks/database/persistence-manager.svelte.ts`

### Step 1: Find `persistConnection` — add new fields to `persistedConnection`

Search for the block building `persistedConnection`. After `aiShareData: connection.aiShareData,` add:

```ts
activeAIProviderId: connection.activeAIProviderId,
activeAIModel: connection.activeAIModel,
```

### Step 2: Find where loaded connections are mapped back to `DatabaseConnection`

Search for where `PersistedConnection` fields are spread into `DatabaseConnection` (look for `aiShareSchema` and `aiShareData` being mapped). After those two fields add:

```ts
activeAIProviderId: persisted.activeAIProviderId,
activeAIModel: persisted.activeAIModel,
```

---

## Task 5: Add setter for active AI model in database hooks

**Files:**
- Modify: `src/lib/hooks/database.svelte.ts` (the main `UseDatabase` class)

### Step 1: Find the class and add a method

Find where other connection mutation methods live (e.g., near `updateConnection` or privacy settings helpers). Add:

```ts
async setConnectionAIModel(connectionId: string, providerId: string, model: string): Promise<void> {
  const conn = this.state.connections.find(c => c.id === connectionId);
  if (!conn) return;
  const updated = { ...conn, activeAIProviderId: providerId, activeAIModel: model };
  this.state.connections = this.state.connections.map(c => c.id === connectionId ? updated : c);
  await this.persistence.persistConnection(updated);
}
```

---

## Task 6: Update `sendAIMessage` and `generateSQL` to accept explicit provider+model

**Files:**
- Modify: `src/lib/services/ai.ts`

### Step 1: Update `SendAIMessageParams`

Add two fields:

```ts
export interface SendAIMessageParams {
  messages: AIMessage[];
  schema: SchemaTable[];
  sampleRows?: Record<string, unknown>[];
  sampleColumns?: string[];
  shareSchema: boolean;
  shareData: boolean;
  providerId: string;
  model: string;
  onChunk: (delta: string) => void;
  onDone: () => void;
  onError: (msg: string) => void;
}
```

### Step 2: Update `sendAIMessage` function body

Replace the lines that read from `aiSettingsStore.activeProvider` with:

```ts
const { providerId, model } = params;
const activeConfig = aiSettingsStore.getProvider(providerId);
if (!activeConfig) {
  onError("no_provider");
  return;
}
const apiKey = await getKeyringService().getAIApiKeyForProvider(activeConfig.id);
if (!apiKey) {
  onError("no_api_key");
  return;
}
```

Then replace `const { provider, model, baseUrl } = activeConfig;` with `const { type: provider, baseUrl } = activeConfig;`

### Step 3: Update `GenerateSQLParams`

Add two fields:

```ts
export interface GenerateSQLParams {
  request: string;
  existingQuery: string;
  schema: SchemaTable[];
  shareSchema: boolean;
  providerId: string;
  model: string;
}
```

### Step 4: Update `generateSQL` function body

Replace the `aiSettingsStore.activeProvider` block with:

```ts
const { providerId, model } = params;
const activeConfig = aiSettingsStore.getProvider(providerId);
if (!activeConfig) throw new Error("no_provider");
const apiKey = await getKeyringService().getAIApiKeyForProvider(activeConfig.id);
if (!apiKey) throw new Error("no_api_key");
```

Then replace `const { provider, model, baseUrl } = activeConfig;` with `const { type: provider, baseUrl } = activeConfig;`

---

## Task 7: Update `UIStateManager` to pass provider+model from active connection

**Files:**
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`

### Step 1: Update `sendAIMessage` call

After resolving `shareSchema`/`shareData`, resolve the active model from the connection:

```ts
const activeProviderId = activeConn?.activeAIProviderId ?? null;
const activeModel = activeConn?.activeAIModel ?? null;

if (!activeProviderId || !activeModel) {
  // Push error message directly
  const errMsg: AIMessage = {
    id: crypto.randomUUID(),
    role: "assistant",
    content: "No AI model selected. Use the model switcher to choose one.",
    timestamp: new Date(),
  };
  this.state.aiMessages = [...this.state.aiMessages, errMsg];
  return;
}
```

Then pass `providerId: activeProviderId, model: activeModel` to `sendAIMessageService(...)`.

---

## Task 8: Update `query-editor.svelte` inline AI to pass provider+model

**Files:**
- Modify: `src/lib/components/query-editor.svelte`

### Step 1: Find the `generateSQL` call (around line 559)

Replace it with a version that reads from the active connection:

```ts
const activeConn = db.state.activeConnection;
const activeProviderId = activeConn?.activeAIProviderId ?? null;
const activeModel = activeConn?.activeAIModel ?? null;

if (!activeProviderId || !activeModel) {
  aiInlineError = "no_provider";
  aiInlineLoading = false;
  return;
}

const sql = await generateSQL({
  request: aiInlinePromptText,
  existingQuery: db.state.activeQueryTab?.query ?? "",
  schema: db.state.activeSchema,
  shareSchema,
  providerId: activeProviderId,
  model: activeModel,
});
```

---

## Task 9: Create shared `AiModelSwitcher` component

**Files:**
- Create: `src/lib/components/ai-model-switcher.svelte`

This component is used in both the AI assistant footer and query editor toolbar.

```svelte
<script lang="ts">
  import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
  import { settingsDialogStore } from "$lib/stores/settings-dialog.svelte.js";
  import { m } from "$lib/paraglide/messages.js";
  import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
  import LoaderCircleIcon from "@lucide/svelte/icons/loader-circle";

  interface Props {
    providerId: string | null;
    model: string | null;
    onSelect: (providerId: string, model: string) => void;
  }

  let { providerId, model, onSelect }: Props = $props();

  let open = $state(false);
  let modelsByProvider = $state<Record<string, string[]>>({});
  let loadingByProvider = $state<Record<string, boolean>>({});

  async function fetchAll() {
    for (const p of aiSettingsStore.settings.providers) {
      if (modelsByProvider[p.id]) continue; // already fetched
      loadingByProvider = { ...loadingByProvider, [p.id]: true };
      const models = await aiSettingsStore.fetchModels(p.id);
      modelsByProvider = { ...modelsByProvider, [p.id]: models };
      loadingByProvider = { ...loadingByProvider, [p.id]: false };
    }
  }

  function toggle() {
    open = !open;
    if (open) fetchAll();
  }

  const activeProvider = $derived(
    providerId ? aiSettingsStore.getProvider(providerId) : null
  );

  const label = $derived(
    activeProvider && model
      ? `${activeProvider.name} / ${model}`
      : m.ai_model_switcher_no_model()
  );
</script>

<div class="relative">
  <button
    type="button"
    class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
    onclick={toggle}
  >
    <span class="truncate max-w-48">{label}</span>
    <ChevronDownIcon class="size-3 shrink-0" />
  </button>

  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="fixed inset-0 z-40" onclick={() => (open = false)}></div>
    <div class="absolute bottom-full mb-1 left-0 z-50 min-w-56 max-w-72 bg-popover border rounded-md shadow-md overflow-hidden">
      {#if aiSettingsStore.settings.providers.length === 0}
        <div class="p-3 text-xs text-muted-foreground">
          {m.settings_ai_configure_prompt()}
          <button
            type="button"
            class="underline ml-1"
            onclick={() => { open = false; settingsDialogStore.open("ai-provider"); }}
          >{m.settings_ai_configure_link()}</button>
        </div>
      {:else}
        {#each aiSettingsStore.settings.providers as p (p.id)}
          <div>
            <div class="px-3 py-1.5 text-xs font-semibold text-muted-foreground bg-muted/50 border-b">
              {p.name}
            </div>
            {#if loadingByProvider[p.id]}
              <div class="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground">
                <LoaderCircleIcon class="size-3 animate-spin" />
                <span>{m.settings_ai_fetching_models()}</span>
              </div>
            {:else if (modelsByProvider[p.id] ?? []).length === 0}
              <div class="px-3 py-2 text-xs text-muted-foreground italic">
                {m.settings_ai_fetch_models_error()}
              </div>
            {:else}
              {#each modelsByProvider[p.id] as m_ (m_)}
                <button
                  type="button"
                  class={[
                    "w-full text-left px-3 py-1.5 text-xs hover:bg-accent",
                    providerId === p.id && model === m_ ? "font-semibold text-foreground" : "text-muted-foreground"
                  ]}
                  onclick={() => { onSelect(p.id, m_); open = false; }}
                >
                  {m_}
                </button>
              {/each}
            {/if}
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>
```

---

## Task 10: Update `ai-assistant.svelte` to use `AiModelSwitcher`

**Files:**
- Modify: `src/lib/components/ai-assistant.svelte`

### Step 1: Replace the `<select>` model switcher in the footer

Remove the existing `<select>` block (lines ~122–140) and replace with:

```svelte
<AiModelSwitcher
  providerId={db.state.activeConnection?.activeAIProviderId ?? null}
  model={db.state.activeConnection?.activeAIModel ?? null}
  onSelect={async (pid, mod) => {
    const conn = db.state.activeConnection;
    if (!conn) return;
    await db.setConnectionAIModel(conn.id, pid, mod);
  }}
/>
```

### Step 2: Add import

```ts
import AiModelSwitcher from "$lib/components/ai-model-switcher.svelte";
```

Remove imports of `aiSettingsStore`, `getDatabase`, and `settingsDialogStore` if no longer used.

---

## Task 11: Update query-editor model switcher to use `AiModelSwitcher`

**Files:**
- Modify: `src/lib/components/query-editor.svelte`

### Step 1: Find the `<select>` model switcher in the inline AI popup (around line 726)

Replace it with:

```svelte
<AiModelSwitcher
  providerId={db.state.activeConnection?.activeAIProviderId ?? null}
  model={db.state.activeConnection?.activeAIModel ?? null}
  onSelect={async (pid, mod) => {
    const conn = db.state.activeConnection;
    if (!conn) return;
    await db.setConnectionAIModel(conn.id, pid, mod);
  }}
/>
```

### Step 2: Add import

```ts
import AiModelSwitcher from "$lib/components/ai-model-switcher.svelte";
```

---

## Task 12: Update settings-dialog provider form — remove model field

**Files:**
- Modify: `src/lib/components/settings-dialog.svelte`

### Step 1: Remove model-related state variables

Remove these state declarations:
- `providerFormModel`
- `providerFormModels`
- `providerFormModelsFetching`
- `providerFormModelsFetchError`

### Step 2: Remove `fetchAvailableModels` function entirely

The function is now in `aiSettingsStore.fetchModels`. Delete it.

### Step 3: Update `providerFormProvider` → `providerFormType`

Rename the state variable and all usages for clarity (matches new `type` field name).

### Step 4: Update `startAddProvider` / `startEditProvider`

Remove model-related assignments. Remove `fetchAvailableModels()` call from `startEditProvider`.

### Step 5: Update `saveProviderForm`

Remove model from the guard (`!providerFormModel.trim()`) and from the objects passed to `addProvider`/`updateProvider`. The `AIProvider` shape no longer has `model`.

Change the type annotation from `AIProviderConfig` to `AIProvider` and `provider` to `type`:

```ts
await aiSettingsStore.addProvider(sqliteDb, {
  id,
  name: providerFormName.trim(),
  type: providerFormType,
  baseUrl: providerFormBaseUrl.trim() || undefined,
}, providerFormApiKey || undefined);
```

### Step 6: Remove the model UI from the form template

In the `{#if shouldShowSection("ai-provider")}` block, delete:
- The `<div class="space-y-1">` block containing the model label and model input/select
- The `{#if providerFormModelsFetching}` / `{:else if providerFormModels.length > 0}` / `{:else}` block

### Step 7: Remove `settings_ai_set_active` / `setActiveProvider` usage

The provider list previously had a "Set as active" button. Remove it — active model is now per-connection via the switcher.

---

## Task 13: Add i18n keys

**Files:**
- Modify: `messages/en.json`
- Modify: `messages/de.json`, `messages/ar.json`, `messages/es.json`, `messages/fr.json`, `messages/ko.json`

### Step 1: Add to `messages/en.json`

After `"settings_ai_configure_link"` add:

```json
"ai_model_switcher_no_model": "Select model",
```

### Step 2: Add the same key to all other locale files with the same value (will be translated later)

---

## Task 14: Run type check and fix any remaining references

**Files:**
- Various (follow compiler errors)

### Step 1: Run type check

```bash
npm run check
```

### Step 2: Fix any remaining references to old fields

Common things to look for:
- `AIProviderConfig` → rename to `AIProvider`
- `.provider` field on AI provider → `.type`
- `activeProviderId` on `AISettings` → no longer exists
- `setActiveProvider` → no longer exists
- `activeProvider` getter → no longer exists

---

## Completion

After all tasks pass `npm run check` cleanly, the feature is ready for manual testing:

1. Open Settings → AI, add a provider (Anthropic or OpenAI-compatible) — no model field shown
2. Connect to a database
3. Open AI assistant — click the model switcher, see providers grouped with models fetched
4. Select a model — switcher label updates to `{provider} / {model}`
5. Close and reopen the app — selected model persists for that connection
6. Open query editor inline AI (Cmd+K) — same grouped model switcher appears
