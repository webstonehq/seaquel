# Multi-Provider AI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Allow multiple AI provider configurations to coexist, with a model switcher in the chat panel footer and inline query prompt.

**Architecture:** Replace the single `AISettings` provider config with a list of `AIProviderConfig` objects each with their own keyring entry (`ai-api-key:{id}`). A reactive `activeProviderId` pointer determines which provider the AI service uses. Two UI switchers (chat footer, inline prompt) update `activeProviderId` in place.

**Tech Stack:** Svelte 5 runes, TypeScript, bits-ui Select, Tauri keyring plugin, paraglide i18n

---

### Task 1: Update AI types

**Files:**
- Modify: `src/lib/types/ai.ts`

**Step 1: Replace the file contents**

```typescript
export type AIProvider = "anthropic" | "openai-compatible";

export interface AIProviderConfig {
  id: string;          // UUID, stable across saves
  name: string;        // user-chosen label e.g. "Claude Opus"
  provider: AIProvider;
  model: string;
  baseUrl: string;     // only used for openai-compatible
}

export interface AISettings {
  providers: AIProviderConfig[];
  activeProviderId: string | null;
  shareSchemaGlobally: boolean;
  shareDataGlobally: boolean;
}

export const DEFAULT_AI_SETTINGS: AISettings = {
  providers: [],
  activeProviderId: null,
  shareSchemaGlobally: true,
  shareDataGlobally: false,
};
```

**Step 2: Verify types compile**

Run: `npm run check`
Expected: no errors in `src/lib/types/ai.ts` (other files will error until later tasks are done — that's fine)

---

### Task 2: Add per-provider keyring methods

**Files:**
- Modify: `src/lib/services/keyring.ts`

**Step 1: Add three methods to the `KeyringService` interface** (after the existing `deleteAIApiKey` line)

```typescript
  setAIApiKeyForProvider(id: string, key: string): Promise<void>;
  getAIApiKeyForProvider(id: string): Promise<string | null>;
  deleteAIApiKeyForProvider(id: string): Promise<void>;
```

**Step 2: Implement in `TauriKeyringService`** (after the existing `deleteAIApiKey` method)

```typescript
  async setAIApiKeyForProvider(id: string, key: string): Promise<void> {
    await this.init();
    await this.keyringApi!.setPassword(SERVICE, `ai-api-key:${id}`, key);
  }

  async getAIApiKeyForProvider(id: string): Promise<string | null> {
    await this.init();
    try {
      return await this.keyringApi!.getPassword(SERVICE, `ai-api-key:${id}`);
    } catch {
      return null;
    }
  }

  async deleteAIApiKeyForProvider(id: string): Promise<void> {
    await this.init();
    try {
      await this.keyringApi!.deletePassword(SERVICE, `ai-api-key:${id}`);
    } catch {
      // Ignore
    }
  }
```

**Step 3: Add no-op implementations to `NoopKeyringService`** (after `deleteAIApiKey`)

```typescript
  async setAIApiKeyForProvider(): Promise<void> {}
  async getAIApiKeyForProvider(): Promise<string | null> { return null; }
  async deleteAIApiKeyForProvider(): Promise<void> {}
```

**Step 4: Verify**

Run: `npm run check`
Expected: no new errors

---

### Task 3: Rewrite AI settings store

**Files:**
- Modify: `src/lib/stores/ai-settings.svelte.ts`

**Step 1: Replace the entire file**

```typescript
import type { SqliteDatabase } from "$lib/storage";
import { appStateRepo } from "$lib/storage";
import { getKeyringService } from "$lib/services/keyring";
import { DEFAULT_AI_SETTINGS, type AISettings, type AIProviderConfig } from "$lib/types/ai";

const AI_SETTINGS_KEY = "aiSettings";

class AISettingsStore {
  settings = $state<AISettings>({ ...DEFAULT_AI_SETTINGS });

  get activeProvider(): AIProviderConfig | null {
    if (!this.settings.activeProviderId) return null;
    return this.settings.providers.find(p => p.id === this.settings.activeProviderId) ?? null;
  }

  async initialize(db: SqliteDatabase): Promise<void> {
    const raw = await appStateRepo.get(db, AI_SETTINGS_KEY);
    if (raw) {
      try {
        this.settings = { ...DEFAULT_AI_SETTINGS, ...JSON.parse(raw) };
      } catch {
        // ignore parse error, use defaults
      }
    }
  }

  private async persistSettings(db: SqliteDatabase, settings: AISettings): Promise<void> {
    this.settings = settings;
    await appStateRepo.set(db, AI_SETTINGS_KEY, JSON.stringify(settings));
  }

  async addProvider(db: SqliteDatabase, config: AIProviderConfig, apiKey?: string): Promise<void> {
    const providers = [...this.settings.providers, config];
    const activeProviderId = this.settings.activeProviderId ?? config.id;
    await this.persistSettings(db, { ...this.settings, providers, activeProviderId });
    if (apiKey) {
      await getKeyringService().setAIApiKeyForProvider(config.id, apiKey);
    }
  }

  async updateProvider(db: SqliteDatabase, config: AIProviderConfig, apiKey?: string): Promise<void> {
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
    const activeProviderId = this.settings.activeProviderId === id
      ? (providers[0]?.id ?? null)
      : this.settings.activeProviderId;
    await this.persistSettings(db, { ...this.settings, providers, activeProviderId });
    await getKeyringService().deleteAIApiKeyForProvider(id);
  }

  async setActiveProvider(db: SqliteDatabase, id: string): Promise<void> {
    await this.persistSettings(db, { ...this.settings, activeProviderId: id });
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
      if (config.provider === "anthropic") {
        const res = await fetch("https://api.anthropic.com/v1/models", {
          headers: {
            "x-api-key": apiKey,
            "anthropic-version": "2023-06-01",
          },
        });
        return res.ok;
      } else {
        const baseUrl = config.baseUrl.replace(/\/$/, "");
        const res = await fetch(`${baseUrl}/models`, {
          headers: { Authorization: `Bearer ${apiKey}` },
        });
        return res.ok;
      }
    } catch {
      return false;
    }
  }
}

export const aiSettingsStore = new AISettingsStore();
```

**Step 2: Verify**

Run: `npm run check`
Expected: errors only in files that still reference the old `aiSettingsStore.save()` / `aiSettingsStore.settings.provider` — that's fine for now

---

### Task 4: Update AI service

**Files:**
- Modify: `src/lib/services/ai.ts`

**Step 1: Update `sendAIMessage`** — replace the block that reads provider settings and calls `getKeyringService().getAIApiKey()`:

Old:
```typescript
  const apiKey = await getKeyringService().getAIApiKey();
  if (!apiKey) {
    onError("no_api_key");
    return;
  }

  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const dataCtx = shareData && sampleRows && sampleColumns ? buildDataContext(sampleRows, sampleColumns) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, dataCtx);

  const { provider, model, baseUrl } = aiSettingsStore.settings;
```

New:
```typescript
  const activeConfig = aiSettingsStore.activeProvider;
  if (!activeConfig) {
    onError("no_provider");
    return;
  }
  const apiKey = await getKeyringService().getAIApiKeyForProvider(activeConfig.id);
  if (!apiKey) {
    onError("no_api_key");
    return;
  }

  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const dataCtx = shareData && sampleRows && sampleColumns ? buildDataContext(sampleRows, sampleColumns) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, dataCtx);

  const { provider, model, baseUrl } = activeConfig;
```

**Step 2: Update `generateSQL`** — replace the equivalent block:

Old:
```typescript
  const apiKey = await getKeyringService().getAIApiKey();
  if (!apiKey) throw new Error("no_api_key");

  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, "");

  const userMessage = existingQuery.trim()
    ? `${request}\n\nExisting query for context:\n\`\`\`sql\n${existingQuery}\n\`\`\``
    : request;

  const { provider, model, baseUrl } = aiSettingsStore.settings;
```

New:
```typescript
  const activeConfig = aiSettingsStore.activeProvider;
  if (!activeConfig) throw new Error("no_provider");
  const apiKey = await getKeyringService().getAIApiKeyForProvider(activeConfig.id);
  if (!apiKey) throw new Error("no_api_key");

  const schemaCtx = shareSchema ? buildSchemaContext(schema) : "";
  const systemPrompt = buildSystemPrompt(schemaCtx, "");

  const userMessage = existingQuery.trim()
    ? `${request}\n\nExisting query for context:\n\`\`\`sql\n${existingQuery}\n\`\`\``
    : request;

  const { provider, model, baseUrl } = activeConfig;
```

**Step 3: Verify**

Run: `npm run check`
Expected: `src/lib/services/ai.ts` clean, errors only in component files

---

### Task 5: Add i18n messages

**Files:**
- Modify: `messages/en.json`
- Modify: `messages/de.json`, `messages/es.json`, `messages/fr.json`, `messages/ar.json`, `messages/ko.json`

**Step 1: Add new keys to `messages/en.json`** after the `"settings_ai_saved"` line:

```json
"settings_ai_add_provider": "Add provider",
"settings_ai_provider_name": "Name",
"settings_ai_provider_name_placeholder": "e.g. Claude Opus, Local Ollama",
"settings_ai_edit_provider": "Edit provider",
"settings_ai_delete_provider": "Delete provider",
"settings_ai_set_active": "Set as active",
"settings_ai_active": "Active",
"settings_ai_no_providers": "No AI providers configured.",
"settings_ai_key_saved": "API key saved",
"settings_ai_model_switcher_label": "Model",
"settings_ai_configure_prompt": "Configure an AI provider in",
"settings_ai_configure_link": "Settings",
```

**Step 2: Add the same keys to all other locale files** — use the same English strings as fallback (they will be translated separately):

Copy the same key-value pairs into `messages/de.json`, `messages/es.json`, `messages/fr.json`, `messages/ar.json`, `messages/ko.json`.

**Step 3: Verify**

Run: `npm run check`
Expected: no paraglide errors

---

### Task 6: Rewrite the AI provider settings section

**Files:**
- Modify: `src/lib/components/settings-dialog.svelte`

This is the largest task. Work through it in sub-steps.

**Step 6a: Add missing icon imports** — add after the existing `SparklesIcon` import line:

```typescript
import PencilIcon from "@lucide/svelte/icons/pencil";
import CircleIcon from "@lucide/svelte/icons/circle";
import CircleCheckIcon from "@lucide/svelte/icons/circle-check";
```

**Step 6b: Remove old AI state vars and handlers**

Remove these lines from the script section (lines ~332–378):
- `let apiKeyInput = $state("")`
- `let aiHasExistingKey = $state(false)`
- `let aiTestStatus = $state<...>("idle")`
- `let isTestingAI = $state(false)`
- `let isSavingAI = $state(false)`
- The entire `$effect` block that calls `getKeyringService().getAIApiKey()`
- The entire `handleAISave` async function
- The entire `handleAITestConnection` async function

**Step 6c: Add new AI state vars** — add in their place:

```typescript
// AI provider management state
let isAddingProvider = $state(false);
let editingProviderId = $state<string | null>(null);
let providerFormName = $state("");
let providerFormProvider = $state<"anthropic" | "openai-compatible">("anthropic");
let providerFormModel = $state("");
let providerFormBaseUrl = $state("");
let providerFormApiKey = $state("");
let providerFormHasExistingKey = $state(false);
let isSavingProvider = $state(false);
let providerTestStatus = $state<Record<string, "idle" | "success" | "failed">>({});
let isTestingProvider = $state<Record<string, boolean>>({});

function startAddProvider() {
  editingProviderId = null;
  providerFormName = "";
  providerFormProvider = "anthropic";
  providerFormModel = "";
  providerFormBaseUrl = "";
  providerFormApiKey = "";
  providerFormHasExistingKey = false;
  isAddingProvider = true;
}

async function startEditProvider(config: import("$lib/types/ai").AIProviderConfig) {
  isAddingProvider = false;
  providerFormName = config.name;
  providerFormProvider = config.provider;
  providerFormModel = config.model;
  providerFormBaseUrl = config.baseUrl;
  providerFormApiKey = "";
  providerFormHasExistingKey = !!(await getKeyringService().getAIApiKeyForProvider(config.id));
  editingProviderId = config.id;
}

function cancelProviderForm() {
  isAddingProvider = false;
  editingProviderId = null;
}

async function saveProviderForm() {
  if (!providerFormName.trim() || !providerFormModel.trim()) return;
  isSavingProvider = true;
  try {
    const sqliteDb = await getDatabase();
    if (editingProviderId) {
      const existing = aiSettingsStore.settings.providers.find(p => p.id === editingProviderId);
      if (!existing) return;
      await aiSettingsStore.updateProvider(sqliteDb, {
        ...existing,
        name: providerFormName.trim(),
        provider: providerFormProvider,
        model: providerFormModel.trim(),
        baseUrl: providerFormBaseUrl.trim(),
      }, providerFormApiKey || undefined);
      if (providerFormApiKey.trim()) providerFormHasExistingKey = true;
    } else {
      const id = crypto.randomUUID();
      await aiSettingsStore.addProvider(sqliteDb, {
        id,
        name: providerFormName.trim(),
        provider: providerFormProvider,
        model: providerFormModel.trim(),
        baseUrl: providerFormBaseUrl.trim(),
      }, providerFormApiKey || undefined);
    }
    providerFormApiKey = "";
    cancelProviderForm();
    toast.success(m.settings_ai_saved());
  } finally {
    isSavingProvider = false;
  }
}

async function deleteProvider(id: string) {
  const sqliteDb = await getDatabase();
  await aiSettingsStore.deleteProvider(sqliteDb, id);
}

async function setActiveProvider(id: string) {
  const sqliteDb = await getDatabase();
  await aiSettingsStore.setActiveProvider(sqliteDb, id);
}

async function testProviderConnection(id: string) {
  isTestingProvider = { ...isTestingProvider, [id]: true };
  providerTestStatus = { ...providerTestStatus, [id]: "idle" };
  try {
    const ok = await aiSettingsStore.testConnection(id);
    providerTestStatus = { ...providerTestStatus, [id]: ok ? "success" : "failed" };
  } catch {
    providerTestStatus = { ...providerTestStatus, [id]: "failed" };
  } finally {
    isTestingProvider = { ...isTestingProvider, [id]: false };
  }
}
```

**Step 6d: Replace the `ai-provider` template section**

Find and replace the entire `{#if shouldShowSection("ai-provider")}` block (the one ending with the Model input and Save/Test buttons, ~lines 955–1045). Replace with:

```svelte
{#if shouldShowSection("ai-provider")}
  <div class="space-y-6">
    <div>
      <h2 class="text-lg font-medium">{m.settings_ai_provider()}</h2>
      <p class="text-sm text-muted-foreground mt-1">
        {m.settings_ai_provider_description()}
      </p>
    </div>

    <div class="space-y-3">
      {#if aiSettingsStore.settings.providers.length === 0 && !isAddingProvider}
        <p class="text-sm text-muted-foreground">{m.settings_ai_no_providers()}</p>
      {/if}

      {#each aiSettingsStore.settings.providers as config (config.id)}
        {#if editingProviderId === config.id}
          <!-- Inline edit form -->
          <div class="rounded-lg border p-4 space-y-3">
            <p class="text-sm font-medium">{m.settings_ai_edit_provider()}</p>
            <input
              type="text"
              class="w-full px-3 py-2 border rounded-md bg-background text-sm"
              placeholder={m.settings_ai_provider_name_placeholder()}
              bind:value={providerFormName}
            />
            <select
              class="w-full px-3 py-2 border rounded-md bg-background text-sm"
              bind:value={providerFormProvider}
            >
              <option value="anthropic">{m.settings_ai_provider_anthropic()}</option>
              <option value="openai-compatible">{m.settings_ai_provider_openai_compatible()}</option>
            </select>
            <div class="space-y-1">
              {#if providerFormHasExistingKey}
                <p class="text-xs text-green-600 dark:text-green-400">{m.settings_ai_key_saved()}</p>
              {/if}
              <input
                type="password"
                class="w-full px-3 py-2 border rounded-md bg-background text-sm"
                placeholder={providerFormHasExistingKey ? "***" : m.settings_ai_api_key_placeholder()}
                bind:value={providerFormApiKey}
              />
            </div>
            {#if providerFormProvider === "openai-compatible"}
              <input
                type="text"
                class="w-full px-3 py-2 border rounded-md bg-background text-sm"
                placeholder={m.settings_ai_base_url_placeholder()}
                bind:value={providerFormBaseUrl}
              />
            {/if}
            <input
              type="text"
              class="w-full px-3 py-2 border rounded-md bg-background text-sm"
              placeholder={m.settings_ai_model_placeholder()}
              bind:value={providerFormModel}
            />
            <div class="flex items-center gap-2">
              <Button size="sm" onclick={saveProviderForm} disabled={isSavingProvider || !providerFormName.trim() || !providerFormModel.trim()}>
                {m.settings_ai_save()}
              </Button>
              <Button size="sm" variant="ghost" onclick={cancelProviderForm}>Cancel</Button>
            </div>
          </div>
        {:else}
          <!-- Provider card -->
          <div class="flex items-center justify-between rounded-lg border px-3 py-2.5">
            <div class="flex items-center gap-3">
              <button
                class="shrink-0 text-muted-foreground hover:text-primary transition-colors"
                onclick={() => setActiveProvider(config.id)}
                title={m.settings_ai_set_active()}
              >
                {#if aiSettingsStore.settings.activeProviderId === config.id}
                  <CircleCheckIcon class="size-4 text-primary" />
                {:else}
                  <CircleIcon class="size-4" />
                {/if}
              </button>
              <div>
                <p class="text-sm font-medium">{config.name}</p>
                <p class="text-xs text-muted-foreground">{config.provider === "anthropic" ? m.settings_ai_provider_anthropic() : m.settings_ai_provider_openai_compatible()} · {config.model}</p>
              </div>
            </div>
            <div class="flex items-center gap-1">
              {#if providerTestStatus[config.id] === "success"}
                <span class="text-xs text-green-600 dark:text-green-400 flex items-center gap-1 mr-1">
                  <CheckIcon class="size-3" />{m.settings_ai_test_success()}
                </span>
              {:else if providerTestStatus[config.id] === "failed"}
                <span class="text-xs text-destructive mr-1">{m.settings_ai_test_failed()}</span>
              {/if}
              <Button
                size="icon"
                variant="ghost"
                class="size-7"
                disabled={isTestingProvider[config.id]}
                onclick={() => testProviderConnection(config.id)}
                title={m.settings_ai_test_connection()}
              >
                <SparklesIcon class="size-3.5" />
              </Button>
              <Button
                size="icon"
                variant="ghost"
                class="size-7"
                onclick={() => startEditProvider(config)}
                title={m.settings_ai_edit_provider()}
              >
                <PencilIcon class="size-3.5" />
              </Button>
              <Button
                size="icon"
                variant="ghost"
                class="size-7 text-destructive hover:text-destructive"
                onclick={() => deleteProvider(config.id)}
                title={m.settings_ai_delete_provider()}
              >
                <TrashIcon class="size-3.5" />
              </Button>
            </div>
          </div>
        {/if}
      {/each}

      {#if isAddingProvider}
        <!-- Inline add form -->
        <div class="rounded-lg border p-4 space-y-3">
          <p class="text-sm font-medium">{m.settings_ai_add_provider()}</p>
          <input
            type="text"
            class="w-full px-3 py-2 border rounded-md bg-background text-sm"
            placeholder={m.settings_ai_provider_name_placeholder()}
            bind:value={providerFormName}
          />
          <select
            class="w-full px-3 py-2 border rounded-md bg-background text-sm"
            bind:value={providerFormProvider}
          >
            <option value="anthropic">{m.settings_ai_provider_anthropic()}</option>
            <option value="openai-compatible">{m.settings_ai_provider_openai_compatible()}</option>
          </select>
          <input
            type="password"
            class="w-full px-3 py-2 border rounded-md bg-background text-sm"
            placeholder={m.settings_ai_api_key_placeholder()}
            bind:value={providerFormApiKey}
          />
          {#if providerFormProvider === "openai-compatible"}
            <input
              type="text"
              class="w-full px-3 py-2 border rounded-md bg-background text-sm"
              placeholder={m.settings_ai_base_url_placeholder()}
              bind:value={providerFormBaseUrl}
            />
          {/if}
          <input
            type="text"
            class="w-full px-3 py-2 border rounded-md bg-background text-sm"
            placeholder={m.settings_ai_model_placeholder()}
            bind:value={providerFormModel}
          />
          <div class="flex items-center gap-2">
            <Button size="sm" onclick={saveProviderForm} disabled={isSavingProvider || !providerFormName.trim() || !providerFormModel.trim()}>
              {m.settings_ai_save()}
            </Button>
            <Button size="sm" variant="ghost" onclick={cancelProviderForm}>Cancel</Button>
          </div>
        </div>
      {:else if !editingProviderId}
        <Button variant="outline" size="sm" onclick={startAddProvider}>
          <PlusIcon class="size-3.5 mr-1" />
          {m.settings_ai_add_provider()}
        </Button>
      {/if}
    </div>
  </div>
{/if}
```

**Step 6e: Update the two privacy toggle `onCheckedChange` handlers** — they currently call `aiSettingsStore.save(sqliteDb, { ...aiSettingsStore.settings })`. Replace both with `aiSettingsStore.savePrivacySettings(sqliteDb, { shareSchemaGlobally: checked, shareDataGlobally: aiSettingsStore.settings.shareDataGlobally })` and `aiSettingsStore.savePrivacySettings(sqliteDb, { shareSchemaGlobally: aiSettingsStore.settings.shareSchemaGlobally, shareDataGlobally: checked })` respectively.

**Step 6f: Verify**

Run: `npm run check`
Expected: clean or near-clean — fix any remaining type errors

---

### Task 7: Add model switcher to chat panel

**Files:**
- Modify: `src/lib/components/ai-assistant.svelte`

**Step 1: Add imports** at the top of the script block, add:

```typescript
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
import { getDatabase } from "$lib/storage/db";
```

**Step 2: Add the switcher below the textarea row in `CardFooter`**

The current footer is:
```svelte
<CardFooter class="border-t p-3">
  <div class="flex gap-2 w-full">
    <Textarea ... />
    <Button ... />
  </div>
</CardFooter>
```

Replace with:
```svelte
<CardFooter class="border-t p-3">
  <div class="flex flex-col gap-2 w-full">
    <div class="flex gap-2">
      <Textarea bind:value={messageInput} placeholder={m.ai_placeholder()} class="min-h-[60px] max-h-[120px] resize-none text-sm" onkeydown={handleKeydown} />
      <Button size="icon" class="shrink-0" aria-label={m.ai_send()} onclick={handleSend} disabled={!messageInput.trim() || db.state.isAIStreaming}>
        <SendIcon class="size-4" />
      </Button>
    </div>
    {#if aiSettingsStore.settings.providers.length === 0}
      <p class="text-xs text-muted-foreground">
        {m.settings_ai_configure_prompt()}
        <button class="underline hover:no-underline" onclick={() => { /* open settings */ }}>
          {m.settings_ai_configure_link()}
        </button>
      </p>
    {:else if aiSettingsStore.settings.providers.length > 1}
      <div class="flex items-center gap-2">
        <span class="text-xs text-muted-foreground">{m.settings_ai_model_switcher_label()}:</span>
        <select
          class="text-xs bg-transparent border-none outline-none cursor-pointer text-muted-foreground hover:text-foreground"
          value={aiSettingsStore.settings.activeProviderId}
          onchange={async (e) => {
            const sqliteDb = await getDatabase();
            await aiSettingsStore.setActiveProvider(sqliteDb, e.currentTarget.value);
          }}
        >
          {#each aiSettingsStore.settings.providers as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </div>
    {:else}
      <p class="text-xs text-muted-foreground">{aiSettingsStore.settings.providers[0].name}</p>
    {/if}
  </div>
</CardFooter>
```

**Step 3: Wire "Configure in Settings" button**

The `ai-assistant.svelte` doesn't currently import `settingsDialogStore`. Add it:

```typescript
import { settingsDialogStore } from "$lib/stores/settings-dialog.svelte.js";
```

Then replace the `/* open settings */` comment with:
```typescript
settingsDialogStore.open("ai-provider")
```

**Step 4: Verify**

Run: `npm run check`
Expected: clean

---

### Task 8: Add model switcher to inline query prompt

**Files:**
- Modify: `src/lib/components/query-editor.svelte`

The inline prompt bar already imports `aiSettingsStore`. It needs `getDatabase` added.

**Step 1: Verify `getDatabase` is already imported** — run:

```bash
grep "getDatabase" src/lib/components/query-editor.svelte
```

If not present, add to imports: `import { getDatabase } from "$lib/storage/db";`

**Step 2: Add the model switcher inside the inline prompt bar**

Find the closing section of the inline prompt bar — the `{:else}` block that shows the ESC button when there's no error/loading:

```svelte
{:else}
  <button
    class="shrink-0 text-xs text-muted-foreground hover:text-foreground"
    onclick={closeAIInlinePrompt}
    aria-label="Close"
  >ESC</button>
{/if}
```

Replace with:

```svelte
{:else}
  {#if aiSettingsStore.settings.providers.length > 1}
    <select
      class="text-xs bg-transparent border-none outline-none cursor-pointer text-muted-foreground hover:text-foreground max-w-28 truncate"
      value={aiSettingsStore.settings.activeProviderId}
      onchange={async (e) => {
        const sqliteDb = await getDatabase();
        await aiSettingsStore.setActiveProvider(sqliteDb, e.currentTarget.value);
      }}
    >
      {#each aiSettingsStore.settings.providers as p (p.id)}
        <option value={p.id}>{p.name}</option>
      {/each}
    </select>
  {/if}
  <button
    class="shrink-0 text-xs text-muted-foreground hover:text-foreground"
    onclick={closeAIInlinePrompt}
    aria-label="Close"
  >ESC</button>
{/if}
```

**Step 3: Handle `no_provider` error in `submitAIInlinePrompt`**

In the catch block of `submitAIInlinePrompt` (around line 568), add a case for `"no_provider"`:

```typescript
if (err instanceof Error && err.message === "no_provider") {
  aiInlinePromptError = {
    message: "No AI provider configured.",
    action: {
      label: "Configure",
      fn: () => { settingsDialogStore.open("ai-provider"); closeAIInlinePrompt(); },
    },
  };
} else if (err instanceof Error && err.message === "no_api_key") {
```

**Step 4: Final verify**

Run: `npm run check`
Expected: no errors
