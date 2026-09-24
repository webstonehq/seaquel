# AI @-Mentions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add @-mention autocomplete in the AI chat input for tables, saved queries, and dashboards — resolving mentions into structured context appended to the LLM message.

**Architecture:** When the user types `@` in the AI textarea, a popover appears with filtered mentionable items (tables, saved queries, dashboards). Selecting an item inserts plain text like `@public.users`. On send, mentions are resolved: matched items have their details (columns, SQL, widget info) appended as a `Referenced context:` block to the message sent to the LLM. The displayed user message keeps the raw `@mention` text. @-mentions are disabled when schema sharing is off.

**Tech Stack:** Svelte 5, TypeScript, Tailwind CSS, bits-ui Popover

---

### Task 1: Create the mention resolution utility

**Files:**
- Create: `src/lib/services/ai-mentions.ts`

This module handles two concerns:
1. Building the list of mentionable items from state
2. Resolving `@mentions` in a message string into appended context

**Step 1: Create `src/lib/services/ai-mentions.ts`**

```ts
import type { SchemaTable, SavedQuery, Dashboard } from "$lib/types";

export type MentionKind = "table" | "query" | "dashboard";

export interface MentionItem {
  kind: MentionKind;
  label: string;
  searchText: string;
  /** Identifier used in the message text, e.g. "public.users" */
  token: string;
}

/**
 * Build the list of mentionable items from current state.
 */
export function buildMentionItems(
  tables: SchemaTable[],
  savedQueries: SavedQuery[],
  dashboards: Dashboard[],
): MentionItem[] {
  const items: MentionItem[] = [];

  for (const t of tables) {
    const token = t.schema ? `${t.schema}.${t.name}` : t.name;
    items.push({
      kind: "table",
      label: token,
      searchText: `${t.schema} ${t.name}`.toLowerCase(),
      token,
    });
  }

  for (const q of savedQueries) {
    items.push({
      kind: "query",
      label: q.name,
      searchText: q.name.toLowerCase(),
      token: q.name,
    });
  }

  for (const d of dashboards) {
    items.push({
      kind: "dashboard",
      label: d.name,
      searchText: d.name.toLowerCase(),
      token: d.name,
    });
  }

  return items;
}

/**
 * Parse @-mentions from message text and return the enriched content
 * to send to the LLM plus the clean display content.
 *
 * Mention syntax: `@token` (no spaces) or `@"token with spaces"`
 */
export function resolveMentions(
  content: string,
  tables: SchemaTable[],
  savedQueries: SavedQuery[],
  dashboards: Dashboard[],
): string {
  // Extract all @-mention tokens from the message
  const mentionRegex = /@"([^"]+)"|@([\w.]+)/g;
  const mentionedTokens = new Set<string>();
  let match: RegExpExecArray | null;
  while ((match = mentionRegex.exec(content)) !== null) {
    mentionedTokens.add(match[1] ?? match[2]);
  }

  if (mentionedTokens.size === 0) return content;

  const contextBlocks: string[] = [];

  for (const token of mentionedTokens) {
    const lower = token.toLowerCase();

    // Try tables
    const table = tables.find((t) => {
      const full = t.schema ? `${t.schema}.${t.name}` : t.name;
      return full.toLowerCase() === lower || t.name.toLowerCase() === lower;
    });
    if (table) {
      const cols = table.columns
        .map((c) => {
          const flags: string[] = [];
          if (c.isPrimaryKey) flags.push("PK");
          if (c.isForeignKey && c.foreignKeyRef)
            flags.push(`FK→${c.foreignKeyRef.referencedTable}.${c.foreignKeyRef.referencedColumn}`);
          if (!c.nullable) flags.push("NOT NULL");
          const flagStr = flags.length ? ` (${flags.join(", ")})` : "";
          return `  ${c.name} ${c.type}${flagStr}`;
        })
        .join("\n");
      const schemaPrefix = table.schema ? `${table.schema}.` : "";
      contextBlocks.push(`Table: ${schemaPrefix}${table.name} (${table.type})\n${cols}`);
      continue;
    }

    // Try saved queries
    const query = savedQueries.find((q) => q.name.toLowerCase() === lower);
    if (query) {
      contextBlocks.push(`Saved Query: ${query.name}\n\`\`\`sql\n${query.query}\n\`\`\``);
      continue;
    }

    // Try dashboards
    const dashboard = dashboards.find((d) => d.name.toLowerCase() === lower);
    if (dashboard) {
      const widgets = dashboard.widgets
        .map((w) => `  - ${w.title} (${w.widgetType})${w.query ? `: ${w.query.slice(0, 100)}` : ""}`)
        .join("\n");
      contextBlocks.push(`Dashboard: ${dashboard.name}\nWidgets:\n${widgets}`);
      continue;
    }
  }

  if (contextBlocks.length === 0) return content;

  return `${content}\n\n---\nReferenced context:\n${contextBlocks.join("\n\n")}`;
}
```

**Step 2: Verify it compiles**

Run: `npm run check`

**Step 3: Commit**

```
feat: add AI mention resolution utility
```

---

### Task 2: Create the mention popover component

**Files:**
- Create: `src/lib/components/ai-mention-popover.svelte`

A floating popover that appears when the user types `@` inside the AI chat textarea. It filters mentionable items as the user continues typing after `@`, and inserts the selected token back into the textarea.

**Step 1: Create `src/lib/components/ai-mention-popover.svelte`**

```svelte
<script lang="ts">
  import { type MentionItem } from "$lib/services/ai-mentions";
  import TableIcon from "@lucide/svelte/icons/table";
  import FileTextIcon from "@lucide/svelte/icons/file-text";
  import LayoutDashboardIcon from "@lucide/svelte/icons/layout-dashboard";
  import EyeIcon from "@lucide/svelte/icons/eye";

  interface Props {
    items: MentionItem[];
    filter: string;
    position: { x: number; y: number };
    onSelect: (item: MentionItem) => void;
    onClose: () => void;
  }

  let { items, filter, position, onSelect, onClose }: Props = $props();
  let selectedIndex = $state(0);

  const filtered = $derived(
    filter
      ? items.filter((item) => item.searchText.includes(filter.toLowerCase())).slice(0, 8)
      : items.slice(0, 8),
  );

  // Reset selection when filter changes
  $effect(() => {
    filter;
    selectedIndex = 0;
  });

  export function handleKeydown(e: KeyboardEvent): boolean {
    if (filtered.length === 0) return false;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedIndex = (selectedIndex + 1) % filtered.length;
      return true;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedIndex = (selectedIndex - 1 + filtered.length) % filtered.length;
      return true;
    }
    if (e.key === "Enter" || e.key === "Tab") {
      e.preventDefault();
      onSelect(filtered[selectedIndex]);
      return true;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
      return true;
    }
    return false;
  }

  const iconMap = {
    table: TableIcon,
    query: FileTextIcon,
    dashboard: LayoutDashboardIcon,
  };

  const kindLabel = {
    table: "Table",
    query: "Saved Query",
    dashboard: "Dashboard",
  };
</script>

{#if filtered.length > 0}
  <div
    class="absolute z-50 w-64 max-h-48 overflow-y-auto rounded-md border bg-popover text-popover-foreground shadow-md"
    style="bottom: {position.y}px; left: {position.x}px;"
  >
    {#each filtered as item, i (item.kind + item.token)}
      <button
        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-accent transition-colors {i === selectedIndex ? 'bg-accent' : ''}"
        onmousedown|preventDefault={() => onSelect(item)}
        onmouseenter={() => (selectedIndex = i)}
      >
        <svelte:component this={iconMap[item.kind]} class="size-3.5 shrink-0 text-muted-foreground" />
        <span class="truncate flex-1">{item.label}</span>
        <span class="text-[10px] text-muted-foreground shrink-0">
          {#if item.kind === "table"}
            {#if item.label.includes(".")}
              <!-- It's a view or table, we can show the icon for view -->
            {/if}
          {/if}
          {kindLabel[item.kind]}
        </span>
      </button>
    {/each}
  </div>
{/if}
```

**Step 2: Verify it compiles**

Run: `npm run check`

**Step 3: Commit**

```
feat: add AI mention popover component
```

---

### Task 3: Integrate mention popover into AI assistant

**Files:**
- Modify: `src/lib/components/ai-assistant.svelte`

Replace the `<Textarea>` with a wrapper that tracks `@` triggers and shows the mention popover. Insert selected mention tokens into the textarea value.

Key changes:
1. Import `buildMentionItems` and `AiMentionPopover`
2. Track mention state: `mentionActive`, `mentionFilter`, `mentionPosition`
3. On input, detect `@` trigger and activate popover
4. On selection, insert the token replacing the `@filter` text
5. Derive `mentionItems` from `db.state.activeSchema`, `db.state.projectSavedQueries`, `db.state.projectDashboards`
6. Compute `schemaSharing` derived to conditionally enable mentions
7. Bind textarea ref for cursor position calculation

**Step 1: Add mention state and derived values to the script block**

After the existing state declarations, add:

```ts
import AiMentionPopover from "$lib/components/ai-mention-popover.svelte";
import { buildMentionItems, type MentionItem } from "$lib/services/ai-mentions";

let textareaRef = $state<HTMLTextAreaElement | null>(null);
let mentionActive = $state(false);
let mentionFilter = $state("");
let mentionStartIndex = $state(0);
let mentionPopoverRef = $state<ReturnType<typeof AiMentionPopover> | null>(null);

const shareSchema = $derived(() => {
  const conn = db.state.activeConnection;
  const settings = aiSettingsStore.settings;
  return conn?.aiShareSchema !== undefined ? conn.aiShareSchema : settings.shareSchemaGlobally;
});

const mentionItems = $derived(
  shareSchema()
    ? buildMentionItems(
        db.state.activeSchema,
        db.state.projectSavedQueries,
        db.state.projectDashboards,
      )
    : [],
);
```

**Step 2: Add mention trigger logic**

Replace `handleKeydown` with:

```ts
const handleKeydown = (e: KeyboardEvent) => {
  if (mentionActive && mentionPopoverRef) {
    if (mentionPopoverRef.handleKeydown(e)) return;
  }
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    handleSend();
  }
};

function handleInput() {
  if (!textareaRef || mentionItems.length === 0) {
    mentionActive = false;
    return;
  }
  const val = textareaRef.value;
  const cursor = textareaRef.selectionStart;

  // Find the last unmatched @ before cursor
  const before = val.slice(0, cursor);
  const atIndex = before.lastIndexOf("@");
  if (atIndex === -1 || (atIndex > 0 && before[atIndex - 1] !== " " && before[atIndex - 1] !== "\n")) {
    mentionActive = false;
    return;
  }

  const fragment = before.slice(atIndex + 1);
  // Close if there's a space and we're past a quoted mention
  if (fragment.includes(" ") && !fragment.startsWith('"')) {
    mentionActive = false;
    return;
  }

  mentionActive = true;
  mentionFilter = fragment;
  mentionStartIndex = atIndex;
}

function selectMention(item: MentionItem) {
  const needsQuotes = item.token.includes(" ");
  const insertText = needsQuotes ? `@"${item.token}" ` : `@${item.token} `;
  const before = messageInput.slice(0, mentionStartIndex);
  const after = messageInput.slice(
    mentionStartIndex + 1 + mentionFilter.length,
  );
  messageInput = before + insertText + after;
  mentionActive = false;

  // Restore focus and cursor position
  requestAnimationFrame(() => {
    if (textareaRef) {
      textareaRef.focus();
      const pos = before.length + insertText.length;
      textareaRef.selectionStart = pos;
      textareaRef.selectionEnd = pos;
    }
  });
}

function closeMention() {
  mentionActive = false;
}
```

**Step 3: Update the textarea in the template**

Replace the existing `<Textarea>` with:

```svelte
<div class="relative flex-1">
  {#if mentionActive}
    <AiMentionPopover
      bind:this={mentionPopoverRef}
      items={mentionItems}
      filter={mentionFilter}
      position={{ x: 0, y: 4 }}
      onSelect={selectMention}
      onClose={closeMention}
    />
  {/if}
  <Textarea
    bind:ref={textareaRef}
    bind:value={messageInput}
    placeholder={m.ai_placeholder()}
    class="min-h-[60px] max-h-[120px] resize-none text-sm"
    onkeydown={handleKeydown}
    oninput={handleInput}
  />
</div>
```

**Step 4: Import `aiSettingsStore`**

Add to existing imports:

```ts
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
```

**Step 5: Verify it compiles**

Run: `npm run check`

**Step 6: Commit**

```
feat: integrate @-mention popover into AI assistant
```

---

### Task 4: Resolve mentions on send

**Files:**
- Modify: `src/lib/hooks/database/ui-state.svelte.ts`

When `sendAIMessage` is called, resolve any `@mentions` in the content before dispatching to the AI service. The user-visible message keeps the raw text; the LLM receives the enriched version.

**Step 1: Import `resolveMentions`**

```ts
import { resolveMentions } from "$lib/services/ai-mentions";
```

**Step 2: Modify `sendAIMessage` method**

In `sendAIMessage(content: string)`, after creating the user message (which stores raw `content`), compute the resolved content and pass it to `_dispatchToAI`:

```ts
sendAIMessage(content: string) {
  const chatId = this.aiChatManager.ensureActiveChat();
  if (!chatId) return;

  const messages = this.state.aiMessagesByChat[chatId] ?? [];
  const isFirstMessage = messages.filter((m) => m.role === "user").length === 0;

  const userMessage: AIMessage = {
    id: crypto.randomUUID(),
    chatId,
    role: "user",
    content,  // raw text with @mentions for display
    timestamp: new Date(),
  };
  this._setMessages(chatId, [...messages, userMessage]);

  if (isFirstMessage) {
    this.aiChatManager.updateChatTitle(chatId, content);
  }

  // Resolve @-mentions into enriched content for the LLM
  const enrichedContent = resolveMentions(
    content,
    this.state.activeSchema,
    this.state.savedQueriesByProject[this.state.activeProjectId ?? ""] ?? [],
    this.state.dashboardsByProject[this.state.activeProjectId ?? ""] ?? [],
  );

  this._dispatchToAI(enrichedContent, chatId);
}
```

**Step 3: Update `_dispatchToAI` signature** (no change needed — it already receives `content` as a string and passes it through)

Actually, looking at `_dispatchToAI`, it currently re-reads the last user message from state. We need to make sure the enriched content is what the LLM sees. Looking at the implementation:

```ts
private _dispatchToAI(content: string, chatId: string) {
```

The `content` param is not actually used as the message — instead, `sendAIMessageService` receives all messages from state. So we need a different approach: **override the last user message content** in the messages array sent to the service.

Update `_dispatchToAI` to accept optional `enrichedContent`:

```ts
private _dispatchToAI(content: string, chatId: string, enrichedContent?: string) {
```

Then in the `sendAIMessageService` call, map messages to replace the last user message content:

```ts
const rawMessages = this._getMessages(chatId).filter(
  (m) => m.id !== assistantMessageId && !m.pendingModelSelection,
);

// If enrichedContent is provided, replace the last user message content with it
const messagesForApi = enrichedContent
  ? rawMessages.map((m, i) => {
      if (i === rawMessages.length - 1 && m.role === "user") {
        return { ...m, content: enrichedContent };
      }
      return m;
    })
  : rawMessages;
```

And pass `messagesForApi` to `sendAIMessageService` instead of the filtered raw messages.

Update the call in `sendAIMessage`:

```ts
this._dispatchToAI(enrichedContent, chatId, enrichedContent);
```

And in `retryPendingMessage`, leave the existing call as-is (no enrichment needed since it re-dispatches).

**Step 4: Verify it compiles**

Run: `npm run check`

**Step 5: Commit**

```
feat: resolve @-mentions into LLM context on send
```

---

### Task 5: Manual testing checklist

1. Open AI chat with a connected database
2. Type `@` — popover should appear with tables, saved queries, dashboards
3. Continue typing to filter — e.g. `@us` should narrow to tables containing "us"
4. Arrow keys navigate, Enter/Tab selects, Escape closes
5. Selected item inserts as `@schema.table ` or `@"My Query Name" `
6. Send a message with a mention — check the LLM response references the correct table/query context
7. Toggle schema sharing OFF — type `@` — no popover should appear
8. Verify the displayed user message shows raw `@mention` text, not the resolved context
