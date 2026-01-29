# Unified Query Editor Design

## Summary

Consolidate query execution logic by having the query builder workspace embed the main query-editor component instead of maintaining its own execution code. This eliminates duplicate logic and gives the Learn section automatic parameter prompt support.

## Goals

- Single source of truth for query execution and parameter handling
- Visual query builder uses the same editor as the Manage section
- Parameter prompts (`{{variable}}`) work in both Learn and Manage contexts

## Non-Goals

- EXPLAIN/EXPLAIN ANALYZE in Learn section (stays Manage-only)
- Row editing in Learn section (stays Manage-only)
- Full feature parity between contexts

## Architecture

### QueryExecutor Interface

```typescript
// src/lib/types/query.ts
interface QueryExecutor {
  execute(sql: string): Promise<Record<string, unknown>[]>;
  dbType?: DatabaseType; // for parameter substitution style
}
```

### Query Editor Changes

The `query-editor.svelte` component accepts an optional `executor` prop:

```typescript
interface Props {
  // ... existing props
  executor?: QueryExecutor;
}
```

Behavior:
- **With executor**: Use it for execution, disable Manage-only features
- **Without executor**: Current behavior using `useDatabase()` hooks

Derived flag controls feature visibility:
```typescript
const isStandaloneMode = $derived(!!executor);
```

Features hidden in standalone mode:
- EXPLAIN / EXPLAIN ANALYZE buttons
- Row editing (edit mode, delete row)
- Chart visualization (optional)
- Export menu (optional)

### Query Builder Workspace Integration

The workspace embeds query-editor instead of its own execution logic:

```svelte
<script>
  import { executeQuery } from '$lib/tutorial/database';
  import QueryEditor from '$lib/components/query-editor.svelte';

  const tutorialExecutor: QueryExecutor = {
    execute: executeQuery,
    dbType: 'duckdb'
  };
</script>

<QueryEditor
  executor={tutorialExecutor}
  value={qb.customSql ?? qb.generatedSql}
  onSqlChange={(sql) => qb.customSql = sql}
/>
```

### SQL Synchronization

- Query-editor accepts `value` prop for SQL content
- Query-editor calls `onSqlChange` callback when user edits
- Workspace passes `qb.generatedSql` and writes edits to `qb.customSql`
- Existing two-way sync behavior preserved

## Files to Modify

| File | Change |
|------|--------|
| `src/lib/types/query.ts` | Add `QueryExecutor` interface |
| `src/lib/components/query-editor.svelte` | Add `executor` prop, conditional feature hiding, standalone execution path |
| `src/lib/components/query-builder/workspace.svelte` | Remove own execution logic, embed query-editor with tutorial executor |
| `src/lib/components/query-editor/query-toolbar.svelte` | Accept prop to hide EXPLAIN/edit features |

## Parameter Handling Flow

1. User types `{{customer_id}}` in filter value (visual builder) or SQL editor
2. Generated SQL contains `WHERE column = {{customer_id}}`
3. User clicks Run
4. Query-editor detects parameters via `hasParameters(sql)`
5. Parameter dialog opens asking for values
6. `substituteParameters()` replaces placeholders with inline values
7. Substituted SQL passed to `executor.execute()`
8. Results displayed

## Migration Notes

- `sql-editor.svelte` in query-builder folder may be removed if query-editor fully replaces it
- Tutorial database unchanged - just wrapped in QueryExecutor interface
- Existing parameter utilities (`query-params.ts`) work as-is
