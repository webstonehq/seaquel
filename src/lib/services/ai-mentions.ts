/**
 * The AI assistant's `@mention` autocomplete. Core expands the mentions a
 * message holds (phase 6, Decision 13); the page only offers them.
 * @module services/ai-mentions
 */

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
 * Builds a flat list of mentionable items for the autocomplete popover.
 */
export function buildMentionItems(
  tables: SchemaTable[],
  savedQueries: SavedQuery[],
  dashboards: Dashboard[],
): MentionItem[] {
  const items: MentionItem[] = [];

  for (const t of tables) {
    const token = `${t.schema}.${t.name}`;
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
 * What the chat's `@` popover offers: nothing when the chat's connection
 * doesn't share its schema with AI (phase 6, Decision 5). Core resolves
 * the mentions a message holds, under the same flag, so a mention typed
 * anyway reaches the model as a bare name.
 */
export function mentionItemsFor(
  shareSchema: boolean,
  tables: SchemaTable[],
  savedQueries: SavedQuery[],
  dashboards: Dashboard[],
): MentionItem[] {
  return shareSchema ? buildMentionItems(tables, savedQueries, dashboards) : [];
}
