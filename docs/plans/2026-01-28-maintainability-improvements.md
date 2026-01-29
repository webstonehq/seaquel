# Maintainability Improvements Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Improve code maintainability through three focused refactors: extract a shared provider registry, adopt the existing `Result<T>` error handling pattern, and decompose the 3065-line query builder into smaller modules.

**Architecture:** Each improvement is self-contained and builds on the previous. The provider registry eliminates duplication across 4 manager classes. Error handling standardization replaces ad-hoc `formatError` methods with the existing centralized infrastructure. Query builder decomposition extracts SQL generation, serialization, and `applyFromParsedSql` into standalone pure-function modules.

**Tech Stack:** SvelteKit 5 + TypeScript, Svelte 5 runes (`$state`, `$derived`)

---

### Task 1: Create the ProviderRegistry class

**Files:**
- Create: `src/lib/providers/provider-registry.ts`
- Reference: `src/lib/providers/index.ts` (existing factory functions)
- Reference: `src/lib/providers/types.ts` (DatabaseProvider interface)

**Step 1: Create `src/lib/providers/provider-registry.ts`**

```typescript
/**
 * Centralized provider lifecycle manager.
 * Replaces duplicated getOrCreate/getProviderFor patterns across managers.
 */

import type { DatabaseProvider } from './types';
import { getProvider, getDuckDBProvider } from './index';

export class ProviderRegistry {
  private provider: DatabaseProvider | null = null;
  private duckdbProvider: DatabaseProvider | null = null;

  /**
   * Get the appropriate provider for a given database type.
   * Lazily initializes and caches provider instances.
   */
  async getForType(dbType: string): Promise<DatabaseProvider> {
    if (dbType === 'duckdb') {
      return this.getOrCreateDuckDB();
    }
    return this.getOrCreateDefault();
  }

  /**
   * Get or create the default database provider (PostgreSQL/SQLite).
   */
  async getOrCreateDefault(): Promise<DatabaseProvider> {
    if (!this.provider) {
      this.provider = await getProvider();
    }
    return this.provider;
  }

  /**
   * Get or create the DuckDB provider.
   */
  async getOrCreateDuckDB(): Promise<DatabaseProvider> {
    if (!this.duckdbProvider) {
      this.duckdbProvider = await getDuckDBProvider();
    }
    return this.duckdbProvider;
  }

  /**
   * Reset cached provider instances.
   * Call on disconnect or cleanup.
   */
  reset(): void {
    this.provider = null;
    this.duckdbProvider = null;
  }
}
```

**Step 2: Export from `src/lib/providers/index.ts`**

Add at the end of the file:
```typescript
export { ProviderRegistry } from './provider-registry';
```

**Step 3: Verify the project type-checks**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/providers/provider-registry.ts src/lib/providers/index.ts
git commit -m "refactor: add ProviderRegistry to centralize provider lifecycle"
```

---

### Task 2: Wire ProviderRegistry into ConnectionManager

**Files:**
- Modify: `src/lib/hooks/database/connection-manager.svelte.ts`
- Modify: `src/lib/hooks/database.svelte.ts` (pass registry via constructor)

**Step 1: Update ConnectionManager to accept ProviderRegistry**

In `connection-manager.svelte.ts`:

1. Replace the import:
   ```typescript
   // Remove:
   import { getProvider, getDuckDBProvider, type DatabaseProvider } from "$lib/providers";
   // Add:
   import type { DatabaseProvider } from "$lib/providers";
   import type { ProviderRegistry } from "$lib/providers";
   ```

2. Replace the provider fields and methods:
   ```typescript
   // Remove these fields (lines 34-35):
   private provider: DatabaseProvider | null = null;
   private duckdbProvider: DatabaseProvider | null = null;

   // Remove these methods (lines 49-74):
   private async getOrCreateProvider()
   private async getOrCreateDuckDBProvider()
   private async getProviderForType(dbType: string)
   ```

3. Add `providers: ProviderRegistry` to the constructor parameters.

4. Replace all `this.getOrCreateProvider()` calls with `this.providers.getOrCreateDefault()`.

5. Replace all `this.getOrCreateDuckDBProvider()` calls with `this.providers.getOrCreateDuckDB()`.

6. Replace all `this.getProviderForType(...)` calls with `this.providers.getForType(...)`.

**Step 2: Update UseDatabase constructor in `database.svelte.ts`**

1. Import ProviderRegistry:
   ```typescript
   import { ProviderRegistry } from "$lib/providers";
   ```

2. Create the shared instance in the constructor before any managers:
   ```typescript
   const providers = new ProviderRegistry();
   ```

3. Pass it to ConnectionManager:
   ```typescript
   this.connections = new ConnectionManager(
     this.state,
     this.persistence,
     this._stateRestoration,
     this.tabs,
     providers,
     (connectionId, schemas, adapter, providerConnectionId, mssqlConnectionId) => { ... },
     () => this.queryTabs.add()
   );
   ```

**Step 3: Verify the project type-checks**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/hooks/database/connection-manager.svelte.ts src/lib/hooks/database.svelte.ts
git commit -m "refactor: wire ProviderRegistry into ConnectionManager"
```

---

### Task 3: Wire ProviderRegistry into QueryExecutionManager

**Files:**
- Modify: `src/lib/hooks/database/query-execution.svelte.ts`
- Modify: `src/lib/hooks/database.svelte.ts`

**Step 1: Update QueryExecutionManager**

1. Replace the import:
   ```typescript
   // Remove:
   import { getProvider, getDuckDBProvider, type DatabaseProvider } from "$lib/providers";
   // Add:
   import type { DatabaseProvider } from "$lib/providers";
   import type { ProviderRegistry } from "$lib/providers";
   ```

2. Remove the three duplicated fields/methods:
   - `private provider: DatabaseProvider | null = null;`
   - `private duckdbProvider: DatabaseProvider | null = null;`
   - `private async getOrCreateProvider()`
   - `private async getOrCreateDuckDBProvider()`
   - `private async getProviderForConnection(connection)`

3. Add `private providers: ProviderRegistry` to the constructor:
   ```typescript
   constructor(
     private state: DatabaseState,
     private queryHistory: QueryHistoryManager,
     private providers: ProviderRegistry
   ) {}
   ```

4. Replace all `this.getProviderForConnection(connection)` with `this.providers.getForType(connection.type)`.

5. Replace any standalone `this.getOrCreateProvider()` with `this.providers.getOrCreateDefault()`.

**Step 2: Update UseDatabase constructor**

Pass `providers` to QueryExecutionManager:
```typescript
this.queries = new QueryExecutionManager(this.state, this.history, providers);
```

**Step 3: Verify**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/hooks/database/query-execution.svelte.ts src/lib/hooks/database.svelte.ts
git commit -m "refactor: wire ProviderRegistry into QueryExecutionManager"
```

---

### Task 4: Wire ProviderRegistry into SchemaTabManager and ExplainTabManager

**Files:**
- Modify: `src/lib/hooks/database/schema-tabs.svelte.ts`
- Modify: `src/lib/hooks/database/explain-tabs.svelte.ts`
- Modify: `src/lib/hooks/database.svelte.ts`

**Step 1: Update SchemaTabManager**

1. Replace the import:
   ```typescript
   // Remove:
   import { getProvider, getDuckDBProvider, type DatabaseProvider } from '$lib/providers';
   // Add:
   import type { ProviderRegistry } from '$lib/providers';
   ```

2. Remove the `getProviderForConnection` method (lines 35-41).

3. Add `private providers: ProviderRegistry` to the constructor:
   ```typescript
   constructor(
     state: DatabaseState,
     tabOrdering: TabOrderingManager,
     schedulePersistence: (projectId: string | null) => void,
     private providers: ProviderRegistry
   ) {
     super(state, tabOrdering, schedulePersistence);
   }
   ```

4. Replace all `this.getProviderForConnection(...)` calls with `this.providers.getForType(...)` (passing the dbType string).

**Step 2: Update ExplainTabManager**

1. Replace the import:
   ```typescript
   // Remove:
   import { getProvider, getDuckDBProvider, type DatabaseProvider } from '$lib/providers';
   // Add:
   import type { ProviderRegistry } from '$lib/providers';
   ```

2. Remove the `getProviderForConnection` method (lines 72-78).

3. Add `private providers: ProviderRegistry` to the constructor:
   ```typescript
   constructor(
     state: DatabaseState,
     tabOrdering: TabOrderingManager,
     schedulePersistence: (projectId: string | null) => void,
     setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void,
     private providers: ProviderRegistry
   ) {
     super(state, tabOrdering, schedulePersistence);
     this.setActiveView = setActiveView;
   }
   ```

4. Replace all `this.getProviderForConnection()` calls with `this.providers.getForType(this.state.activeConnection?.type ?? '')`.

**Step 3: Update UseDatabase constructor**

Pass `providers` to both managers:
```typescript
this.schemaTabs = new SchemaTabManager(this.state, this.tabs, scheduleProjectPersistence, providers);
this.explainTabs = new ExplainTabManager(this.state, this.tabs, scheduleProjectPersistence, setActiveView, providers);
```

**Step 4: Check for any other managers that import from `$lib/providers`**

Search for `from '$lib/providers'` or `from "$lib/providers"` across the hooks directory. If any other managers also have the pattern, update them similarly.

**Step 5: Verify**

Run: `npm run check`
Expected: No type errors

**Step 6: Commit**

```bash
git add src/lib/hooks/database/schema-tabs.svelte.ts src/lib/hooks/database/explain-tabs.svelte.ts src/lib/hooks/database.svelte.ts
git commit -m "refactor: wire ProviderRegistry into SchemaTabManager and ExplainTabManager"
```

---

### Task 5: Replace `formatError` with `extractErrorMessage` in QueryExecutionManager

**Files:**
- Modify: `src/lib/hooks/database/query-execution.svelte.ts`

**Step 1: Add import for `extractErrorMessage`**

```typescript
import { extractErrorMessage } from '$lib/errors';
```

**Step 2: Remove the private `formatError` method**

Remove lines 57-64:
```typescript
// DELETE:
private formatError(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}
```

**Step 3: Replace all `this.formatError(...)` calls with `extractErrorMessage(...)`**

There are ~10 call sites in the file. Replace each `this.formatError(error)` with `extractErrorMessage(error)`.

**Step 4: Verify**

Run: `npm run check`
Expected: No type errors

**Step 5: Commit**

```bash
git add src/lib/hooks/database/query-execution.svelte.ts
git commit -m "refactor: replace formatError with centralized extractErrorMessage"
```

---

### Task 6: Adopt `withErrorHandling` in ConnectionManager

**Files:**
- Modify: `src/lib/hooks/database/connection-manager.svelte.ts`

**Step 1: Add import**

```typescript
import { withErrorHandling, handleError, createError, extractErrorMessage } from '$lib/errors';
```

**Step 2: Refactor the `test()` method to use `withErrorHandling`**

The `test()` method is a good candidate because it's self-contained. Find the method and wrap its core logic:

Before (pattern):
```typescript
async test(connection: ConnectionInput): Promise<boolean> {
  try {
    // ... test logic ...
    toast.success("Connection successful!");
    return true;
  } catch (error) {
    errorToast(error instanceof Error ? error.message : String(error));
    return false;
  }
}
```

After:
```typescript
async test(connection: ConnectionInput): Promise<boolean> {
  const result = await withErrorHandling(
    async () => {
      // ... existing test logic (without the try-catch) ...
    },
    'CONNECTION_FAILED',
    `Failed to connect to ${connection.name || 'database'}`
  );

  if (result.ok) {
    toast.success("Connection successful!");
    return true;
  }
  return false;
}
```

**Step 3: Refactor the `add()` method's error handling similarly**

Replace the outer try-catch in `add()` with `withErrorHandling`, using error code `'CONNECTION_FAILED'`.

**Step 4: Verify**

Run: `npm run check`
Expected: No type errors

**Step 5: Commit**

```bash
git add src/lib/hooks/database/connection-manager.svelte.ts
git commit -m "refactor: adopt withErrorHandling in ConnectionManager"
```

---

### Task 7: Extract SQL generation from QueryBuilderState

**Files:**
- Create: `src/lib/hooks/query-builder-sql.ts`
- Modify: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Create `src/lib/hooks/query-builder-sql.ts`**

Extract these methods as pure functions (they only read state passed as params, no `this` needed):

```typescript
/**
 * Pure SQL generation functions for the query builder.
 * Converts builder state into SQL strings.
 */

import type {
  CanvasTable,
  CanvasJoin,
  FilterCondition,
  GroupByCondition,
  HavingCondition,
  SortCondition,
  SelectAggregate,
  CanvasSubquery,
  SubqueryInnerState,
  CanvasCTE
} from '$lib/types';

/**
 * Check if a value is a template variable like {{my_var}}.
 */
function isTemplateVariable(value: string): boolean {
  return /^\{\{.+\}\}$/.test(value.trim());
}

/**
 * Build the complete SQL statement from top-level query builder state.
 */
export function buildSql(
  tables: CanvasTable[],
  joins: CanvasJoin[],
  filters: FilterCondition[],
  groupBy: GroupByCondition[],
  having: HavingCondition[],
  orderBy: SortCondition[],
  limit: string | number | null,
  selectAggregates: SelectAggregate[],
  subqueries: CanvasSubquery[],
  ctes: CanvasCTE[]
): string {
  let sql = '';

  // Build WITH clause if CTEs exist
  if (ctes.length > 0) {
    const cteParts = ctes
      .filter((cte) => cte.name && cte.innerQuery.tables.length > 0)
      .map((cte) => {
        const innerSql = buildSubquerySql(cte.innerQuery);
        const indentedInnerSql = innerSql
          .split('\n')
          .map((line) => '  ' + line)
          .join('\n');
        return `${cte.name} AS (\n${indentedInnerSql}\n)`;
      });

    if (cteParts.length > 0) {
      sql = `WITH ${cteParts.join(',\n')}\n`;
    }
  }

  sql += buildQuerySql(tables, joins, filters, groupBy, having, orderBy, limit, selectAggregates, subqueries);
  return sql;
}

/**
 * Build SQL from query state (used for main query and recursive subqueries).
 */
export function buildQuerySql(
  tables: CanvasTable[],
  joins: CanvasJoin[],
  filters: FilterCondition[],
  groupBy: GroupByCondition[],
  having: HavingCondition[],
  orderBy: SortCondition[],
  limit: string | number | null,
  selectAggregates: SelectAggregate[],
  subqueries: CanvasSubquery[]
): string {
  // (Move the entire body of the existing private buildQuerySql method here,
  //  replacing this.buildSubquerySql with buildSubquerySql,
  //  this.buildFilterCondition with buildFilterCondition,
  //  this.buildHavingCondition with buildHavingCondition)
  // ... exact existing logic from lines 1987-2117 ...
}

/**
 * Build SQL for a subquery's inner state (recursive).
 */
export function buildSubquerySql(inner: SubqueryInnerState): string {
  return buildQuerySql(
    inner.tables,
    inner.joins,
    inner.filters,
    inner.groupBy,
    inner.having,
    inner.orderBy,
    inner.limit,
    inner.selectAggregates,
    inner.subqueries
  );
}

/**
 * Build a single filter condition string.
 */
export function buildFilterCondition(
  filter: FilterCondition,
  subqueries: CanvasSubquery[] = []
): string {
  // (Move body from existing private buildFilterCondition, lines 2141-2203)
}

/**
 * Build a single HAVING condition string.
 */
export function buildHavingCondition(having: HavingCondition): string {
  // (Move body from existing private buildHavingCondition, lines 2208-2213)
}
```

**Step 2: Update QueryBuilderState to delegate to the extracted functions**

In `query-builder.svelte.ts`:

1. Add import:
   ```typescript
   import { buildSql as generateSqlFromState } from './query-builder-sql';
   ```

2. Replace the `private buildSql()` method body:
   ```typescript
   private buildSql(): string {
     return generateSqlFromState(
       this.tables, this.joins, this.filters,
       this.groupBy, this.having, this.orderBy,
       this.limit, this.selectAggregates, this.subqueries, this.ctes
     );
   }
   ```

3. Remove the now-unused private methods:
   - `private buildQuerySql(...)` (lines 1976-2117)
   - `private buildSubquerySql(...)` (lines 2123-2135)
   - `private buildFilterCondition(...)` (lines 2141-2203)
   - `private buildHavingCondition(...)` (lines 2208-2213)

4. Remove the `isTemplateVariable` function from the file (it moved to `query-builder-sql.ts`).

**Step 3: Verify**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/hooks/query-builder-sql.ts src/lib/hooks/query-builder.svelte.ts
git commit -m "refactor: extract SQL generation into query-builder-sql module"
```

---

### Task 8: Extract serialization from QueryBuilderState

**Files:**
- Create: `src/lib/hooks/query-builder-serialization.ts`
- Modify: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Create `src/lib/hooks/query-builder-serialization.ts`**

Move the serialization interfaces and functions:

```typescript
/**
 * Serialization/deserialization for query builder state.
 * Converts between runtime types (with Set/Map) and plain JSON-safe types.
 */

import { SvelteSet } from 'svelte/reactivity';
import type {
  CanvasTable,
  CanvasJoin,
  FilterCondition,
  GroupByCondition,
  HavingCondition,
  SortCondition,
  SelectAggregate,
  CanvasSubquery,
  SubqueryInnerState,
  CanvasCTE,
  ColumnAggregate
} from '$lib/types';

// === Serializable interfaces ===

export interface SerializableTable {
  id: string;
  tableName: string;
  position: { x: number; y: number };
  selectedColumns: string[];
  columnAggregates?: Record<string, ColumnAggregate>;
  cteId?: string;
}

export interface SerializableInnerQuery {
  tables: SerializableTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  groupBy: GroupByCondition[];
  having: HavingCondition[];
  orderBy: SortCondition[];
  limit: string | number | null;
  selectAggregates: SelectAggregate[];
  subqueries?: SerializableSubquery[];
}

export interface SerializableSubquery {
  id: string;
  position: { x: number; y: number };
  size: { width: number; height: number };
  role: 'where' | 'from' | 'select';
  linkedFilterId?: string;
  alias?: string;
  innerQuery: SerializableInnerQuery;
}

export interface SerializableCTE {
  id: string;
  name: string;
  position: { x: number; y: number };
  size: { width: number; height: number };
  innerQuery: SerializableInnerQuery;
}

export interface SerializableQueryBuilderState {
  tables: SerializableTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  groupBy?: GroupByCondition[];
  having?: HavingCondition[];
  orderBy: SortCondition[];
  limit: string | number | null;
  customSql?: string | null;
  selectAggregates?: SelectAggregate[];
  subqueries?: SerializableSubquery[];
  ctes?: SerializableCTE[];
}

// === Serialize functions ===

export function serializeQueryBuilderState(state: {
  tables: CanvasTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  groupBy: GroupByCondition[];
  having: HavingCondition[];
  orderBy: SortCondition[];
  limit: string | number | null;
  customSql: string | null;
  selectAggregates: SelectAggregate[];
  subqueries: CanvasSubquery[];
  ctes: CanvasCTE[];
}): SerializableQueryBuilderState {
  return {
    tables: state.tables.map((t) => ({
      id: t.id,
      tableName: t.tableName,
      position: t.position,
      selectedColumns: Array.from(t.selectedColumns),
      columnAggregates: Object.fromEntries(t.columnAggregates),
      cteId: t.cteId
    })),
    joins: [...state.joins],
    filters: [...state.filters],
    groupBy: [...state.groupBy],
    having: [...state.having],
    orderBy: [...state.orderBy],
    limit: state.limit,
    customSql: state.customSql,
    selectAggregates: [...state.selectAggregates],
    subqueries: serializeSubqueries(state.subqueries),
    ctes: serializeCtes(state.ctes)
  };
}

function serializeSubqueries(subqueries: CanvasSubquery[]): SerializableSubquery[] {
  return subqueries.map((sq) => ({
    id: sq.id,
    position: sq.position,
    size: sq.size,
    role: sq.role,
    linkedFilterId: sq.linkedFilterId,
    alias: sq.alias,
    innerQuery: serializeInnerQuery(sq.innerQuery)
  }));
}

function serializeCtes(ctes: CanvasCTE[]): SerializableCTE[] {
  return ctes.map((cte) => ({
    id: cte.id,
    name: cte.name,
    position: cte.position,
    size: cte.size,
    innerQuery: serializeInnerQuery(cte.innerQuery)
  }));
}

function serializeInnerQuery(inner: SubqueryInnerState): SerializableInnerQuery {
  return {
    tables: inner.tables.map((t) => ({
      id: t.id,
      tableName: t.tableName,
      position: t.position,
      selectedColumns: Array.from(t.selectedColumns),
      columnAggregates: Object.fromEntries(t.columnAggregates)
    })),
    joins: [...inner.joins],
    filters: [...inner.filters],
    groupBy: [...inner.groupBy],
    having: [...inner.having],
    orderBy: [...inner.orderBy],
    limit: inner.limit,
    selectAggregates: [...inner.selectAggregates],
    subqueries: serializeSubqueries(inner.subqueries)
  };
}

// === Deserialize functions ===

export function deserializeQueryBuilderState(state: SerializableQueryBuilderState): {
  tables: CanvasTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  groupBy: GroupByCondition[];
  having: HavingCondition[];
  orderBy: SortCondition[];
  limit: string | number | null;
  customSql: string | null;
  selectAggregates: SelectAggregate[];
  subqueries: CanvasSubquery[];
  ctes: CanvasCTE[];
} {
  return {
    tables: state.tables.map((t) => ({
      id: t.id,
      tableName: t.tableName,
      position: t.position,
      selectedColumns: new SvelteSet(t.selectedColumns),
      columnAggregates: new Map(
        Object.entries(t.columnAggregates ?? {}) as [string, ColumnAggregate][]
      ),
      cteId: t.cteId
    })),
    joins: state.joins.map((j) => ({ ...j })),
    filters: state.filters.map((f) => ({ ...f })),
    groupBy: (state.groupBy ?? []).map((g) => ({ ...g })),
    having: (state.having ?? []).map((h) => ({ ...h })),
    orderBy: state.orderBy.map((o) => ({ ...o })),
    limit: state.limit,
    customSql: state.customSql ?? null,
    selectAggregates: (state.selectAggregates ?? []).map((a) => ({ ...a })),
    subqueries: deserializeSubqueries(state.subqueries ?? []),
    ctes: deserializeCtes(state.ctes ?? [])
  };
}

function deserializeSubqueries(subqueries: SerializableSubquery[]): CanvasSubquery[] {
  return subqueries.map((sq) => ({
    id: sq.id,
    position: sq.position,
    size: sq.size,
    role: sq.role,
    linkedFilterId: sq.linkedFilterId,
    alias: sq.alias,
    innerQuery: deserializeInnerQuery(sq.innerQuery)
  }));
}

function deserializeCtes(ctes: SerializableCTE[]): CanvasCTE[] {
  return ctes.map((cte) => ({
    id: cte.id,
    name: cte.name,
    position: cte.position,
    size: cte.size,
    innerQuery: deserializeInnerQuery(cte.innerQuery)
  }));
}

function deserializeInnerQuery(inner: SerializableInnerQuery): SubqueryInnerState {
  return {
    tables: inner.tables.map((t) => ({
      id: t.id,
      tableName: t.tableName,
      position: t.position,
      selectedColumns: new SvelteSet(t.selectedColumns),
      columnAggregates: new Map(
        Object.entries(t.columnAggregates ?? {}) as [string, ColumnAggregate][]
      )
    })),
    joins: inner.joins.map((j) => ({ ...j })),
    filters: inner.filters.map((f) => ({ ...f })),
    groupBy: inner.groupBy.map((g) => ({ ...g })),
    having: inner.having.map((h) => ({ ...h })),
    orderBy: inner.orderBy.map((o) => ({ ...o })),
    limit: inner.limit,
    selectAggregates: inner.selectAggregates.map((a) => ({ ...a })),
    subqueries: deserializeSubqueries(inner.subqueries ?? [])
  };
}
```

**Step 2: Update QueryBuilderState to use extracted serialization**

1. Add import:
   ```typescript
   import {
     serializeQueryBuilderState,
     deserializeQueryBuilderState,
     type SerializableQueryBuilderState
   } from './query-builder-serialization';
   ```

2. Simplify `toSerializable()`:
   ```typescript
   toSerializable(): SerializableQueryBuilderState {
     return serializeQueryBuilderState(this);
   }
   ```

3. Simplify `fromSerializable()`:
   ```typescript
   fromSerializable(state: SerializableQueryBuilderState): void {
     const deserialized = deserializeQueryBuilderState(state);
     this.tables = deserialized.tables;
     this.joins = deserialized.joins;
     this.filters = deserialized.filters;
     this.groupBy = deserialized.groupBy;
     this.having = deserialized.having;
     this.orderBy = deserialized.orderBy;
     this.limit = deserialized.limit;
     this.customSql = deserialized.customSql;
     this.selectAggregates = deserialized.selectAggregates;
     this.subqueries = deserialized.subqueries;
     this.ctes = deserialized.ctes;
   }
   ```

4. Remove the old private serialization methods and interfaces from `query-builder.svelte.ts`:
   - `serializeSubqueries`, `serializeCtes`, `serializeInnerQuery`
   - `deserializeSubqueries`, `deserializeCtes`, `deserializeInnerQuery`
   - `SerializableTable`, `SerializableInnerQuery`, `SerializableSubquery`, `SerializableCTE`, `SerializableQueryBuilderState`

5. Re-export the serializable types from `query-builder.svelte.ts` for backward compatibility (if other files import them):
   ```typescript
   export type {
     SerializableTable,
     SerializableInnerQuery,
     SerializableSubquery,
     SerializableCTE,
     SerializableQueryBuilderState
   } from './query-builder-serialization';
   ```

**Step 3: Check for external consumers of the old exports**

Search for imports of `SerializableQueryBuilderState`, `SerializableTable`, etc. from `query-builder.svelte.ts` across the codebase and update them to import from `query-builder-serialization.ts` or from the re-export.

**Step 4: Verify**

Run: `npm run check`
Expected: No type errors

**Step 5: Commit**

```bash
git add src/lib/hooks/query-builder-serialization.ts src/lib/hooks/query-builder.svelte.ts
git commit -m "refactor: extract serialization from query builder into standalone module"
```

---

### Task 9: Final verification and cleanup

**Files:**
- All modified files from previous tasks

**Step 1: Full type check**

Run: `npm run check`
Expected: Clean pass

**Step 2: Review the line count reduction**

Run: `wc -l src/lib/hooks/query-builder.svelte.ts`
Expected: Roughly ~2200 lines (down from 3065 — about 860 lines extracted)

**Step 3: Verify no unused imports remain**

Run: `npm run check` (TypeScript will catch unused imports with strict settings)

**Step 4: Final commit if any cleanup was needed**

```bash
git add -A
git commit -m "refactor: cleanup after maintainability improvements"
```
