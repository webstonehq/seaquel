# Interactive SELECT Tutorial Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build an interactive SQL query builder where users drag tables onto a canvas, select columns, define JOINs by connecting tables, add filters/sorting/limits, and see real-time SQL generation with two-way Monaco editor sync.

**Architecture:** A standalone query builder component using @xyflow/svelte for the canvas, custom table nodes with column checkboxes, and a filter panel below. State managed via a new `QueryBuilderState` class following existing patterns. SQL parsing via `node-sql-parser` for two-way sync.

**Tech Stack:** Svelte 5 runes, @xyflow/svelte, node-sql-parser, sql-formatter, Monaco editor, SQLite (tutorial database)

---

## Task 1: TypeScript Types

**Files:**
- Create: `src/lib/types/query-builder.ts`
- Modify: `src/lib/types/index.ts`

**Step 1: Create type definitions**

```typescript
// src/lib/types/query-builder.ts

/** Column definition from the sample database schema */
export interface TutorialColumn {
  name: string;
  type: string;
  primaryKey?: boolean;
  foreignKey?: { table: string; column: string };
}

/** Table definition from the sample database schema */
export interface TutorialTable {
  name: string;
  columns: TutorialColumn[];
}

/** A table placed on the canvas */
export interface CanvasTable {
  id: string;
  tableName: string;
  position: { x: number; y: number };
  selectedColumns: Set<string>;
}

/** Join type options */
export type JoinType = 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';

/** A join between two tables */
export interface CanvasJoin {
  id: string;
  sourceTable: string;
  sourceColumn: string;
  targetTable: string;
  targetColumn: string;
  joinType: JoinType;
}

/** Comparison operators for WHERE clauses */
export type FilterOperator =
  | '='
  | '!='
  | '>'
  | '<'
  | '>='
  | '<='
  | 'LIKE'
  | 'NOT LIKE'
  | 'IS NULL'
  | 'IS NOT NULL'
  | 'IN'
  | 'BETWEEN';

/** A single WHERE condition */
export interface FilterCondition {
  id: string;
  column: string; // format: "table.column"
  operator: FilterOperator;
  value: string;
  connector: 'AND' | 'OR';
}

/** Sort direction */
export type SortDirection = 'ASC' | 'DESC';

/** An ORDER BY clause */
export interface SortCondition {
  id: string;
  column: string; // format: "table.column"
  direction: SortDirection;
}

/** Complete query builder state */
export interface QueryBuilderSnapshot {
  tables: CanvasTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  orderBy: SortCondition[];
  limit: number | null;
}

/** Validation criterion for challenges */
export interface ChallengeCriterion {
  id: string;
  description: string;
  check: (state: QueryBuilderSnapshot, sql: string) => boolean;
  satisfied: boolean;
}

/** A single challenge within a lesson */
export interface Challenge {
  id: string;
  title: string;
  description: string;
  hint?: string;
  criteria: Omit<ChallengeCriterion, 'satisfied'>[];
}

/** A tutorial lesson */
export interface TutorialLesson {
  id: string;
  title: string;
  introduction: string;
  challenges: Challenge[];
}
```

**Step 2: Export from index**

Add to `src/lib/types/index.ts`:

```typescript
// Query builder types
export type {
  TutorialColumn,
  TutorialTable,
  CanvasTable,
  JoinType,
  CanvasJoin,
  FilterOperator,
  FilterCondition,
  SortDirection,
  SortCondition,
  QueryBuilderSnapshot,
  ChallengeCriterion,
  Challenge,
  TutorialLesson
} from './query-builder';
```

**Step 3: Verify types compile**

Run: `npm run check`

**Step 4: Commit**

```bash
git add src/lib/types/query-builder.ts src/lib/types/index.ts
git commit -m "feat(tutorial): add TypeScript types for query builder"
```

---

## Task 2: Sample Database Schema Definition

**Files:**
- Create: `src/lib/tutorial/schema.ts`

**Step 1: Define the e-commerce schema**

```typescript
// src/lib/tutorial/schema.ts
import type { TutorialTable } from '$lib/types';

export const TUTORIAL_SCHEMA: TutorialTable[] = [
  {
    name: 'categories',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'description', type: 'TEXT' },
    ],
  },
  {
    name: 'products',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'description', type: 'TEXT' },
      { name: 'price', type: 'DECIMAL' },
      { name: 'stock', type: 'INTEGER' },
      { name: 'category_id', type: 'INTEGER', foreignKey: { table: 'categories', column: 'id' } },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'customers',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'name', type: 'TEXT' },
      { name: 'email', type: 'TEXT' },
      { name: 'country', type: 'TEXT' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'orders',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'customer_id', type: 'INTEGER', foreignKey: { table: 'customers', column: 'id' } },
      { name: 'status', type: 'TEXT' },
      { name: 'total', type: 'DECIMAL' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
  {
    name: 'order_items',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'order_id', type: 'INTEGER', foreignKey: { table: 'orders', column: 'id' } },
      { name: 'product_id', type: 'INTEGER', foreignKey: { table: 'products', column: 'id' } },
      { name: 'quantity', type: 'INTEGER' },
      { name: 'unit_price', type: 'DECIMAL' },
    ],
  },
  {
    name: 'reviews',
    columns: [
      { name: 'id', type: 'INTEGER', primaryKey: true },
      { name: 'product_id', type: 'INTEGER', foreignKey: { table: 'products', column: 'id' } },
      { name: 'customer_id', type: 'INTEGER', foreignKey: { table: 'customers', column: 'id' } },
      { name: 'rating', type: 'INTEGER' },
      { name: 'comment', type: 'TEXT' },
      { name: 'created_at', type: 'DATETIME' },
    ],
  },
];

/** Get a table by name */
export function getTable(name: string): TutorialTable | undefined {
  return TUTORIAL_SCHEMA.find((t) => t.name === name);
}

/** Get all table names */
export function getTableNames(): string[] {
  return TUTORIAL_SCHEMA.map((t) => t.name);
}
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/tutorial/schema.ts
git commit -m "feat(tutorial): add e-commerce sample database schema"
```

---

## Task 3: Query Builder State Management

**Files:**
- Create: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Create the state class**

```typescript
// src/lib/hooks/query-builder.svelte.ts
import { setContext, getContext } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
import type {
  CanvasTable,
  CanvasJoin,
  FilterCondition,
  SortCondition,
  JoinType,
  FilterOperator,
  SortDirection,
  QueryBuilderSnapshot,
} from '$lib/types';
import { TUTORIAL_SCHEMA, getTable } from '$lib/tutorial/schema';

const QUERY_BUILDER_KEY = Symbol('query-builder');

export class QueryBuilderState {
  // Canvas state
  tables = $state<CanvasTable[]>([]);
  joins = $state<CanvasJoin[]>([]);

  // Query modifiers
  filters = $state<FilterCondition[]>([]);
  orderBy = $state<SortCondition[]>([]);
  limit = $state<number | null>(100);

  // SQL sync state
  private _sqlOverride = $state<string | null>(null);
  private _isVisualMode = $state(true);

  get isVisualMode() {
    return this._isVisualMode;
  }

  // Derived: generate SQL from visual state
  generatedSql = $derived.by(() => {
    if (this._sqlOverride !== null) {
      return this._sqlOverride;
    }
    return this.buildSql();
  });

  // Snapshot for validation
  get snapshot(): QueryBuilderSnapshot {
    return {
      tables: this.tables.map((t) => ({
        ...t,
        selectedColumns: new Set(t.selectedColumns),
      })),
      joins: [...this.joins],
      filters: [...this.filters],
      orderBy: [...this.orderBy],
      limit: this.limit,
    };
  }

  // === Table Management ===

  addTable(tableName: string, position: { x: number; y: number }) {
    const table = getTable(tableName);
    if (!table) return;

    // Don't add duplicate tables
    if (this.tables.some((t) => t.tableName === tableName)) return;

    const id = crypto.randomUUID();
    this.tables.push({
      id,
      tableName,
      position,
      selectedColumns: new SvelteSet(),
    });
  }

  removeTable(tableId: string) {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    // Remove associated joins
    this.joins = this.joins.filter(
      (j) => j.sourceTable !== table.tableName && j.targetTable !== table.tableName
    );

    // Remove associated filters and order by
    this.filters = this.filters.filter((f) => !f.column.startsWith(`${table.tableName}.`));
    this.orderBy = this.orderBy.filter((o) => !o.column.startsWith(`${table.tableName}.`));

    // Remove the table
    this.tables = this.tables.filter((t) => t.id !== tableId);
  }

  updateTablePosition(tableId: string, position: { x: number; y: number }) {
    const table = this.tables.find((t) => t.id === tableId);
    if (table) {
      table.position = position;
    }
  }

  toggleColumn(tableId: string, columnName: string) {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    if (table.selectedColumns.has(columnName)) {
      table.selectedColumns.delete(columnName);
    } else {
      table.selectedColumns.add(columnName);
    }
  }

  selectAllColumns(tableId: string) {
    const canvasTable = this.tables.find((t) => t.id === tableId);
    if (!canvasTable) return;

    const schema = getTable(canvasTable.tableName);
    if (!schema) return;

    schema.columns.forEach((col) => canvasTable.selectedColumns.add(col.name));
  }

  clearColumns(tableId: string) {
    const table = this.tables.find((t) => t.id === tableId);
    if (table) {
      table.selectedColumns.clear();
    }
  }

  // === Join Management ===

  addJoin(
    sourceTable: string,
    sourceColumn: string,
    targetTable: string,
    targetColumn: string,
    joinType: JoinType = 'INNER'
  ) {
    // Don't add duplicate joins
    const exists = this.joins.some(
      (j) =>
        j.sourceTable === sourceTable &&
        j.sourceColumn === sourceColumn &&
        j.targetTable === targetTable &&
        j.targetColumn === targetColumn
    );
    if (exists) return;

    this.joins.push({
      id: crypto.randomUUID(),
      sourceTable,
      sourceColumn,
      targetTable,
      targetColumn,
      joinType,
    });
  }

  updateJoinType(joinId: string, joinType: JoinType) {
    const join = this.joins.find((j) => j.id === joinId);
    if (join) {
      join.joinType = joinType;
    }
  }

  removeJoin(joinId: string) {
    this.joins = this.joins.filter((j) => j.id !== joinId);
  }

  // === Filter Management ===

  addFilter(
    column: string,
    operator: FilterOperator = '=',
    value: string = '',
    connector: 'AND' | 'OR' = 'AND'
  ) {
    this.filters.push({
      id: crypto.randomUUID(),
      column,
      operator,
      value,
      connector,
    });
  }

  updateFilter(filterId: string, updates: Partial<Omit<FilterCondition, 'id'>>) {
    const filter = this.filters.find((f) => f.id === filterId);
    if (filter) {
      Object.assign(filter, updates);
    }
  }

  removeFilter(filterId: string) {
    this.filters = this.filters.filter((f) => f.id !== filterId);
  }

  // === Order By Management ===

  addOrderBy(column: string, direction: SortDirection = 'ASC') {
    // Don't add duplicate
    if (this.orderBy.some((o) => o.column === column)) return;

    this.orderBy.push({
      id: crypto.randomUUID(),
      column,
      direction,
    });
  }

  updateOrderBy(orderId: string, direction: SortDirection) {
    const order = this.orderBy.find((o) => o.id === orderId);
    if (order) {
      order.direction = direction;
    }
  }

  removeOrderBy(orderId: string) {
    this.orderBy = this.orderBy.filter((o) => o.id !== orderId);
  }

  reorderOrderBy(fromIndex: number, toIndex: number) {
    const item = this.orderBy.splice(fromIndex, 1)[0];
    if (item) {
      this.orderBy.splice(toIndex, 0, item);
    }
  }

  // === Limit Management ===

  setLimit(limit: number | null) {
    this.limit = limit;
  }

  // === SQL Generation ===

  private buildSql(): string {
    if (this.tables.length === 0) {
      return '-- Add tables to start building your query';
    }

    // Collect selected columns
    const selectColumns: string[] = [];
    for (const table of this.tables) {
      for (const col of table.selectedColumns) {
        selectColumns.push(`${table.tableName}.${col}`);
      }
    }

    if (selectColumns.length === 0) {
      return '-- Select columns to include in your query';
    }

    // Build SELECT
    let sql = `SELECT\n  ${selectColumns.join(',\n  ')}`;

    // Build FROM with JOINs
    const [firstTable, ...otherTables] = this.tables;
    sql += `\nFROM ${firstTable.tableName}`;

    // Add JOINs
    for (const join of this.joins) {
      const joinKeyword =
        join.joinType === 'INNER'
          ? 'INNER JOIN'
          : join.joinType === 'LEFT'
            ? 'LEFT JOIN'
            : join.joinType === 'RIGHT'
              ? 'RIGHT JOIN'
              : 'FULL OUTER JOIN';

      sql += `\n${joinKeyword} ${join.targetTable}`;
      sql += `\n  ON ${join.sourceTable}.${join.sourceColumn} = ${join.targetTable}.${join.targetColumn}`;
    }

    // Add tables without joins (CROSS JOIN implicitly)
    for (const table of otherTables) {
      const hasJoin = this.joins.some(
        (j) => j.sourceTable === table.tableName || j.targetTable === table.tableName
      );
      if (!hasJoin) {
        sql += `,\n  ${table.tableName}`;
      }
    }

    // Build WHERE
    if (this.filters.length > 0) {
      sql += '\nWHERE ';
      this.filters.forEach((filter, i) => {
        if (i > 0) {
          sql += `\n  ${filter.connector} `;
        }
        sql += this.buildFilterCondition(filter);
      });
    }

    // Build ORDER BY
    if (this.orderBy.length > 0) {
      sql += '\nORDER BY ';
      sql += this.orderBy.map((o) => `${o.column} ${o.direction}`).join(', ');
    }

    // Build LIMIT
    if (this.limit !== null) {
      sql += `\nLIMIT ${this.limit}`;
    }

    return sql;
  }

  private buildFilterCondition(filter: FilterCondition): string {
    if (filter.operator === 'IS NULL') {
      return `${filter.column} IS NULL`;
    }
    if (filter.operator === 'IS NOT NULL') {
      return `${filter.column} IS NOT NULL`;
    }
    if (filter.operator === 'IN') {
      return `${filter.column} IN (${filter.value})`;
    }
    if (filter.operator === 'BETWEEN') {
      return `${filter.column} BETWEEN ${filter.value}`;
    }
    if (filter.operator === 'LIKE' || filter.operator === 'NOT LIKE') {
      return `${filter.column} ${filter.operator} '${filter.value}'`;
    }

    // Check if value is numeric
    const isNumeric = !isNaN(Number(filter.value)) && filter.value.trim() !== '';
    const quotedValue = isNumeric ? filter.value : `'${filter.value}'`;

    return `${filter.column} ${filter.operator} ${quotedValue}`;
  }

  // === SQL Sync (for two-way binding) ===

  setSqlOverride(sql: string | null) {
    this._sqlOverride = sql;
    this._isVisualMode = sql === null;
  }

  // === Reset ===

  reset() {
    this.tables = [];
    this.joins = [];
    this.filters = [];
    this.orderBy = [];
    this.limit = 100;
    this._sqlOverride = null;
    this._isVisualMode = true;
  }
}

export function setQueryBuilder(state: QueryBuilderState = new QueryBuilderState()) {
  setContext(QUERY_BUILDER_KEY, state);
  return state;
}

export function useQueryBuilder(): QueryBuilderState {
  return getContext<QueryBuilderState>(QUERY_BUILDER_KEY);
}
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(tutorial): add QueryBuilderState for canvas state management"
```

---

## Task 4: Table Node Component

**Files:**
- Create: `src/lib/components/query-builder/table-node.svelte`

**Step 1: Create the custom xyflow node**

```svelte
<!-- src/lib/components/query-builder/table-node.svelte -->
<script lang="ts">
  import { Handle, Position, type NodeProps } from '@xyflow/svelte';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Button } from '$lib/components/ui/button';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { getTable } from '$lib/tutorial/schema';
  import { TableIcon, LinkIcon } from '@lucide/svelte';

  type TableNodeData = {
    tableName: string;
    tableId: string;
    selectedColumns: Set<string>;
  };

  let { id, data }: NodeProps<TableNodeData> = $props();

  const qb = useQueryBuilder();
  const schema = $derived(getTable(data.tableName));
</script>

<div class="bg-card border rounded-lg shadow-md min-w-[200px] max-w-[280px]">
  <!-- Header -->
  <div class="flex items-center gap-2 px-3 py-2 border-b bg-muted/50 rounded-t-lg">
    <TableIcon class="size-4 text-muted-foreground" />
    <span class="font-medium text-sm flex-1 truncate">{data.tableName}</span>
    <div class="flex gap-1">
      <Button
        variant="ghost"
        size="icon-sm"
        class="size-6"
        onclick={() => qb.selectAllColumns(data.tableId)}
      >
        <span class="text-[10px]">All</span>
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        class="size-6"
        onclick={() => qb.clearColumns(data.tableId)}
      >
        <span class="text-[10px]">None</span>
      </Button>
    </div>
  </div>

  <!-- Columns -->
  <ScrollArea class="max-h-[240px]">
    <div class="p-1">
      {#if schema}
        {#each schema.columns as column (column.name)}
          {@const isSelected = data.selectedColumns.has(column.name)}
          {@const hasFk = !!column.foreignKey}
          <div
            class="flex items-center gap-2 px-2 py-1.5 hover:bg-muted/50 rounded relative group"
          >
            <!-- Left handle for receiving connections -->
            <Handle
              type="target"
              position={Position.Left}
              id={`${column.name}-target`}
              class="!size-2 !bg-primary !border-background !-left-1"
            />

            <Checkbox
              checked={isSelected}
              onCheckedChange={() => qb.toggleColumn(data.tableId, column.name)}
            />

            <div class="flex-1 min-w-0 flex items-center gap-1.5">
              <span class="text-sm truncate" class:font-medium={column.primaryKey}>
                {column.name}
              </span>
              {#if hasFk}
                <LinkIcon class="size-3 text-muted-foreground shrink-0" />
              {/if}
            </div>

            <span class="text-xs text-muted-foreground shrink-0">{column.type}</span>

            <!-- Right handle for creating connections -->
            <Handle
              type="source"
              position={Position.Right}
              id={`${column.name}-source`}
              class="!size-2 !bg-primary !border-background !-right-1"
            />
          </div>
        {/each}
      {/if}
    </div>
  </ScrollArea>
</div>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/table-node.svelte
git commit -m "feat(tutorial): add TableNode component for canvas"
```

---

## Task 5: Join Edge Component

**Files:**
- Create: `src/lib/components/query-builder/join-edge.svelte`

**Step 1: Create the custom edge with label**

```svelte
<!-- src/lib/components/query-builder/join-edge.svelte -->
<script lang="ts">
  import {
    BaseEdge,
    EdgeLabelRenderer,
    getBezierPath,
    type EdgeProps,
  } from '@xyflow/svelte';
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
  import { Button } from '$lib/components/ui/button';
  import type { JoinType } from '$lib/types';

  type JoinEdgeData = {
    joinId: string;
    joinType: JoinType;
  };

  let {
    id,
    sourceX,
    sourceY,
    targetX,
    targetY,
    sourcePosition,
    targetPosition,
    data,
    markerEnd,
    style,
  }: EdgeProps<JoinEdgeData> = $props();

  const qb = useQueryBuilder();

  const [edgePath, labelX, labelY] = $derived(
    getBezierPath({
      sourceX,
      sourceY,
      sourcePosition,
      targetX,
      targetY,
      targetPosition,
    })
  );

  const joinTypeLabels: Record<JoinType, string> = {
    INNER: 'INNER',
    LEFT: 'LEFT',
    RIGHT: 'RIGHT',
    FULL: 'FULL',
  };

  const joinTypeDescriptions: Record<JoinType, string> = {
    INNER: 'Only matching rows from both tables',
    LEFT: 'All rows from left, matching from right',
    RIGHT: 'All rows from right, matching from left',
    FULL: 'All rows from both tables',
  };

  function handleJoinTypeChange(type: JoinType) {
    if (data?.joinId) {
      qb.updateJoinType(data.joinId, type);
    }
  }

  const isDashed = $derived(data?.joinType !== 'INNER');
</script>

<BaseEdge
  path={edgePath}
  {markerEnd}
  style={`${style || ''}; stroke-dasharray: ${isDashed ? '5,5' : 'none'};`}
/>

<EdgeLabelRenderer>
  <div
    class="absolute pointer-events-auto nodrag nopan"
    style:transform="translate(-50%, -50%) translate({labelX}px, {labelY}px)"
  >
    <DropdownMenu.Root>
      <DropdownMenu.Trigger>
        {#snippet child({ props })}
          <Button
            variant="outline"
            size="sm"
            class="h-6 px-2 text-xs bg-background shadow-sm"
            {...props}
          >
            {joinTypeLabels[data?.joinType ?? 'INNER']} JOIN
          </Button>
        {/snippet}
      </DropdownMenu.Trigger>
      <DropdownMenu.Content class="w-56">
        <DropdownMenu.Group>
          <DropdownMenu.GroupHeading>Join Type</DropdownMenu.GroupHeading>
          {#each Object.entries(joinTypeLabels) as [type, label]}
            <DropdownMenu.Item onclick={() => handleJoinTypeChange(type as JoinType)}>
              <div class="flex flex-col">
                <span class="font-medium">{label} JOIN</span>
                <span class="text-xs text-muted-foreground">
                  {joinTypeDescriptions[type as JoinType]}
                </span>
              </div>
            </DropdownMenu.Item>
          {/each}
        </DropdownMenu.Group>
      </DropdownMenu.Content>
    </DropdownMenu.Root>
  </div>
</EdgeLabelRenderer>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/join-edge.svelte
git commit -m "feat(tutorial): add JoinEdge component with type selector"
```

---

## Task 6: Table Palette Component

**Files:**
- Create: `src/lib/components/query-builder/table-palette.svelte`

**Step 1: Create the draggable table list**

```svelte
<!-- src/lib/components/query-builder/table-palette.svelte -->
<script lang="ts">
  import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { ScrollArea } from '$lib/components/ui/scroll-area';
  import { TableIcon, GripVerticalIcon, CheckIcon } from '@lucide/svelte';

  const qb = useQueryBuilder();

  // Track which tables are already on the canvas
  const tablesOnCanvas = $derived(new Set(qb.tables.map((t) => t.tableName)));

  function handleDragStart(event: DragEvent, tableName: string) {
    if (!event.dataTransfer) return;
    event.dataTransfer.setData('application/table-name', tableName);
    event.dataTransfer.effectAllowed = 'copy';
  }
</script>

<div class="w-48 border-r bg-muted/30 flex flex-col">
  <div class="p-3 border-b">
    <h3 class="font-medium text-sm">Tables</h3>
    <p class="text-xs text-muted-foreground mt-1">Drag onto canvas</p>
  </div>

  <ScrollArea class="flex-1">
    <div class="p-2 space-y-1">
      {#each TUTORIAL_SCHEMA as table (table.name)}
        {@const isOnCanvas = tablesOnCanvas.has(table.name)}
        <div
          draggable={!isOnCanvas}
          ondragstart={(e) => handleDragStart(e, table.name)}
          class="flex items-center gap-2 px-2 py-1.5 rounded text-sm transition-colors"
          class:cursor-grab={!isOnCanvas}
          class:hover:bg-muted={!isOnCanvas}
          class:opacity-50={isOnCanvas}
          class:cursor-default={isOnCanvas}
        >
          {#if isOnCanvas}
            <CheckIcon class="size-4 text-green-500" />
          {:else}
            <GripVerticalIcon class="size-4 text-muted-foreground" />
          {/if}
          <TableIcon class="size-4 text-muted-foreground" />
          <span class="truncate">{table.name}</span>
          <span class="text-xs text-muted-foreground ml-auto">
            {table.columns.length}
          </span>
        </div>
      {/each}
    </div>
  </ScrollArea>
</div>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/table-palette.svelte
git commit -m "feat(tutorial): add TablePalette component for dragging tables"
```

---

## Task 7: Filter Panel Component

**Files:**
- Create: `src/lib/components/query-builder/filter-panel.svelte`

**Step 1: Create the stacked filter/sort/limit panel**

```svelte
<!-- src/lib/components/query-builder/filter-panel.svelte -->
<script lang="ts">
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Input } from '$lib/components/ui/input';
  import { Checkbox } from '$lib/components/ui/checkbox';
  import { Label } from '$lib/components/ui/label';
  import * as Select from '$lib/components/ui/select';
  import * as Collapsible from '$lib/components/ui/collapsible';
  import {
    PlusIcon,
    XIcon,
    ChevronDownIcon,
    FilterIcon,
    ArrowUpDownIcon,
    HashIcon,
  } from '@lucide/svelte';
  import type { FilterOperator, SortDirection } from '$lib/types';

  const qb = useQueryBuilder();

  // Get all columns from tables on canvas
  const availableColumns = $derived(
    qb.tables.flatMap((t) => {
      const schema = qb.tables.find((table) => table.tableName === t.tableName);
      if (!schema) return [];
      // Import schema to get column info
      return [t.tableName]; // Simplified - will expand
    })
  );

  // Build column options grouped by table
  const columnOptions = $derived(() => {
    const options: { value: string; label: string; group: string }[] = [];
    for (const table of qb.tables) {
      const schema = import.meta.glob('$lib/tutorial/schema.ts', { eager: true });
      // For now, we'll use a simpler approach
      options.push({ value: `${table.tableName}.*`, label: '*', group: table.tableName });
    }
    return options;
  });

  const operators: { value: FilterOperator; label: string }[] = [
    { value: '=', label: 'equals' },
    { value: '!=', label: 'not equals' },
    { value: '>', label: 'greater than' },
    { value: '<', label: 'less than' },
    { value: '>=', label: 'greater or equal' },
    { value: '<=', label: 'less or equal' },
    { value: 'LIKE', label: 'contains' },
    { value: 'IS NULL', label: 'is null' },
    { value: 'IS NOT NULL', label: 'is not null' },
  ];

  let whereExpanded = $state(true);
  let orderByExpanded = $state(true);
  let limitExpanded = $state(true);
  let noLimit = $state(false);

  function getColumnsForSelect(): { value: string; label: string }[] {
    const cols: { value: string; label: string }[] = [];
    for (const table of qb.tables) {
      for (const col of table.selectedColumns) {
        cols.push({
          value: `${table.tableName}.${col}`,
          label: `${table.tableName}.${col}`,
        });
      }
    }
    return cols;
  }

  const selectableColumns = $derived(getColumnsForSelect());

  function getAllColumns(): { value: string; label: string }[] {
    const cols: { value: string; label: string }[] = [];
    // This needs schema import - simplified for now
    for (const table of qb.tables) {
      cols.push({
        value: `${table.tableName}.id`,
        label: `${table.tableName}.id`,
      });
    }
    return cols;
  }

  const allColumns = $derived(getAllColumns());
</script>

<div class="border-t bg-muted/30 max-h-64 overflow-auto">
  <!-- WHERE Section -->
  <Collapsible.Root bind:open={whereExpanded}>
    <Collapsible.Trigger class="flex items-center justify-between w-full px-4 py-2 hover:bg-muted/50">
      <div class="flex items-center gap-2">
        <FilterIcon class="size-4" />
        <span class="font-medium text-sm">WHERE</span>
        {#if qb.filters.length > 0}
          <span class="text-xs text-muted-foreground">({qb.filters.length})</span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <Button
          variant="ghost"
          size="icon-sm"
          onclick={(e) => {
            e.stopPropagation();
            qb.addFilter(allColumns[0]?.value ?? 'table.column');
          }}
          disabled={qb.tables.length === 0}
        >
          <PlusIcon class="size-4" />
        </Button>
        <ChevronDownIcon class="size-4 transition-transform" class:rotate-180={!whereExpanded} />
      </div>
    </Collapsible.Trigger>
    <Collapsible.Content>
      <div class="px-4 pb-3 space-y-2">
        {#each qb.filters as filter, i (filter.id)}
          <div class="flex items-center gap-2">
            {#if i > 0}
              <Select.Root
                type="single"
                value={filter.connector}
                onValueChange={(v) => qb.updateFilter(filter.id, { connector: v as 'AND' | 'OR' })}
              >
                <Select.Trigger class="w-16 h-8 text-xs">
                  {filter.connector}
                </Select.Trigger>
                <Select.Content>
                  <Select.Item value="AND">AND</Select.Item>
                  <Select.Item value="OR">OR</Select.Item>
                </Select.Content>
              </Select.Root>
            {:else}
              <div class="w-16"></div>
            {/if}

            <Input
              value={filter.column}
              onchange={(e) => qb.updateFilter(filter.id, { column: e.currentTarget.value })}
              class="flex-1 h-8 text-sm"
              placeholder="table.column"
            />

            <Select.Root
              type="single"
              value={filter.operator}
              onValueChange={(v) => qb.updateFilter(filter.id, { operator: v as FilterOperator })}
            >
              <Select.Trigger class="w-32 h-8 text-xs">
                {operators.find((o) => o.value === filter.operator)?.label ?? filter.operator}
              </Select.Trigger>
              <Select.Content>
                {#each operators as op}
                  <Select.Item value={op.value}>{op.label}</Select.Item>
                {/each}
              </Select.Content>
            </Select.Root>

            {#if filter.operator !== 'IS NULL' && filter.operator !== 'IS NOT NULL'}
              <Input
                value={filter.value}
                onchange={(e) => qb.updateFilter(filter.id, { value: e.currentTarget.value })}
                class="w-32 h-8 text-sm"
                placeholder="value"
              />
            {/if}

            <Button variant="ghost" size="icon-sm" onclick={() => qb.removeFilter(filter.id)}>
              <XIcon class="size-4" />
            </Button>
          </div>
        {/each}

        {#if qb.filters.length === 0}
          <p class="text-xs text-muted-foreground py-2">No filters added</p>
        {/if}
      </div>
    </Collapsible.Content>
  </Collapsible.Root>

  <!-- ORDER BY Section -->
  <Collapsible.Root bind:open={orderByExpanded}>
    <Collapsible.Trigger class="flex items-center justify-between w-full px-4 py-2 hover:bg-muted/50 border-t">
      <div class="flex items-center gap-2">
        <ArrowUpDownIcon class="size-4" />
        <span class="font-medium text-sm">ORDER BY</span>
        {#if qb.orderBy.length > 0}
          <span class="text-xs text-muted-foreground">({qb.orderBy.length})</span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <Button
          variant="ghost"
          size="icon-sm"
          onclick={(e) => {
            e.stopPropagation();
            qb.addOrderBy(selectableColumns[0]?.value ?? 'table.column');
          }}
          disabled={selectableColumns.length === 0}
        >
          <PlusIcon class="size-4" />
        </Button>
        <ChevronDownIcon class="size-4 transition-transform" class:rotate-180={!orderByExpanded} />
      </div>
    </Collapsible.Trigger>
    <Collapsible.Content>
      <div class="px-4 pb-3 space-y-2">
        {#each qb.orderBy as order (order.id)}
          <div class="flex items-center gap-2">
            <Input
              value={order.column}
              onchange={(e) => {
                const newCol = e.currentTarget.value;
                qb.removeOrderBy(order.id);
                qb.addOrderBy(newCol, order.direction);
              }}
              class="flex-1 h-8 text-sm"
              placeholder="table.column"
            />

            <Select.Root
              type="single"
              value={order.direction}
              onValueChange={(v) => qb.updateOrderBy(order.id, v as SortDirection)}
            >
              <Select.Trigger class="w-24 h-8 text-xs">
                {order.direction}
              </Select.Trigger>
              <Select.Content>
                <Select.Item value="ASC">ASC</Select.Item>
                <Select.Item value="DESC">DESC</Select.Item>
              </Select.Content>
            </Select.Root>

            <Button variant="ghost" size="icon-sm" onclick={() => qb.removeOrderBy(order.id)}>
              <XIcon class="size-4" />
            </Button>
          </div>
        {/each}

        {#if qb.orderBy.length === 0}
          <p class="text-xs text-muted-foreground py-2">No sorting applied</p>
        {/if}
      </div>
    </Collapsible.Content>
  </Collapsible.Root>

  <!-- LIMIT Section -->
  <Collapsible.Root bind:open={limitExpanded}>
    <Collapsible.Trigger class="flex items-center justify-between w-full px-4 py-2 hover:bg-muted/50 border-t">
      <div class="flex items-center gap-2">
        <HashIcon class="size-4" />
        <span class="font-medium text-sm">LIMIT</span>
      </div>
      <ChevronDownIcon class="size-4 transition-transform" class:rotate-180={!limitExpanded} />
    </Collapsible.Trigger>
    <Collapsible.Content>
      <div class="px-4 pb-3">
        <div class="flex items-center gap-4">
          <Label class="flex items-center gap-2">
            <span class="text-sm">Return first</span>
            <Input
              type="number"
              value={qb.limit ?? ''}
              onchange={(e) => {
                const val = e.currentTarget.value;
                qb.setLimit(val ? parseInt(val, 10) : null);
              }}
              disabled={noLimit}
              class="w-24 h-8"
              min={1}
            />
            <span class="text-sm">rows</span>
          </Label>

          <Label class="flex items-center gap-2 text-sm">
            <Checkbox
              checked={noLimit}
              onCheckedChange={(checked) => {
                noLimit = !!checked;
                if (checked) {
                  qb.setLimit(null);
                } else {
                  qb.setLimit(100);
                }
              }}
            />
            No limit
          </Label>
        </div>
      </div>
    </Collapsible.Content>
  </Collapsible.Root>
</div>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/filter-panel.svelte
git commit -m "feat(tutorial): add FilterPanel component for WHERE/ORDER BY/LIMIT"
```

---

## Task 8: SQL Editor Component

**Files:**
- Create: `src/lib/components/query-builder/sql-editor.svelte`

**Step 1: Create the Monaco editor with two-way sync**

```svelte
<!-- src/lib/components/query-builder/sql-editor.svelte -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { Button } from '$lib/components/ui/button';
  import { CopyIcon, RotateCcwIcon, AlertTriangleIcon } from '@lucide/svelte';
  import { toast } from 'svelte-sonner';

  const qb = useQueryBuilder();

  let editorContainer: HTMLDivElement;
  let editor: import('monaco-editor').editor.IStandaloneCodeEditor | null = null;
  let isUpdatingFromExternal = false;

  // Warning for unsupported SQL
  let showUnsupportedWarning = $state(false);

  onMount(async () => {
    const monaco = await import('monaco-editor');

    editor = monaco.editor.create(editorContainer, {
      value: qb.generatedSql,
      language: 'sql',
      theme: 'vs-dark',
      minimap: { enabled: false },
      fontSize: 13,
      lineNumbers: 'on',
      scrollBeyondLastLine: false,
      automaticLayout: true,
      tabSize: 2,
      wordWrap: 'on',
      padding: { top: 12, bottom: 12 },
    });

    // Listen for user edits
    editor.onDidChangeModelContent(() => {
      if (isUpdatingFromExternal) return;

      const sql = editor?.getValue() ?? '';
      // For now, just set the override - two-way parsing comes later
      qb.setSqlOverride(sql);
    });

    return () => {
      editor?.dispose();
    };
  });

  // Update editor when generated SQL changes (from visual edits)
  $effect(() => {
    if (editor && qb.isVisualMode) {
      const currentValue = editor.getValue();
      if (currentValue !== qb.generatedSql) {
        isUpdatingFromExternal = true;
        editor.setValue(qb.generatedSql);
        isUpdatingFromExternal = false;
      }
    }
  });

  function handleCopy() {
    navigator.clipboard.writeText(qb.generatedSql);
    toast.success('SQL copied to clipboard');
  }

  function handleReset() {
    qb.setSqlOverride(null);
    showUnsupportedWarning = false;
    if (editor) {
      isUpdatingFromExternal = true;
      editor.setValue(qb.generatedSql);
      isUpdatingFromExternal = false;
    }
  }
</script>

<div class="flex flex-col h-full border-l">
  <!-- Header -->
  <div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
    <span class="font-medium text-sm">SQL</span>
    <div class="flex items-center gap-1">
      <Button variant="ghost" size="icon-sm" onclick={handleCopy}>
        <CopyIcon class="size-4" />
      </Button>
      {#if !qb.isVisualMode}
        <Button variant="ghost" size="icon-sm" onclick={handleReset}>
          <RotateCcwIcon class="size-4" />
        </Button>
      {/if}
    </div>
  </div>

  <!-- Warning banner -->
  {#if showUnsupportedWarning}
    <div class="flex items-center gap-2 px-3 py-2 bg-yellow-500/10 border-b border-yellow-500/20 text-yellow-600 dark:text-yellow-400">
      <AlertTriangleIcon class="size-4 shrink-0" />
      <span class="text-xs">This query contains features not supported by the visual builder.</span>
      <Button variant="ghost" size="sm" class="ml-auto h-6 text-xs" onclick={handleReset}>
        Reset
      </Button>
    </div>
  {/if}

  <!-- Editor -->
  <div bind:this={editorContainer} class="flex-1 min-h-0"></div>
</div>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/sql-editor.svelte
git commit -m "feat(tutorial): add SqlEditor component with Monaco"
```

---

## Task 9: Main Canvas Component

**Files:**
- Create: `src/lib/components/query-builder/canvas.svelte`

**Step 1: Create the main @xyflow/svelte canvas**

```svelte
<!-- src/lib/components/query-builder/canvas.svelte -->
<script lang="ts">
  import {
    SvelteFlow,
    Controls,
    Background,
    BackgroundVariant,
    type Node,
    type Edge,
    type Connection,
  } from '@xyflow/svelte';
  import '@xyflow/svelte/dist/style.css';

  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { getTable } from '$lib/tutorial/schema';
  import TableNode from './table-node.svelte';
  import JoinEdge from './join-edge.svelte';
  import type { JoinType } from '$lib/types';

  const qb = useQueryBuilder();

  // Custom node types
  const nodeTypes = {
    tableNode: TableNode,
  };

  // Custom edge types
  const edgeTypes = {
    joinEdge: JoinEdge,
  };

  // Convert state to xyflow nodes
  const nodes = $derived<Node[]>(
    qb.tables.map((table) => ({
      id: table.id,
      type: 'tableNode',
      position: table.position,
      data: {
        tableName: table.tableName,
        tableId: table.id,
        selectedColumns: table.selectedColumns,
      },
    }))
  );

  // Convert joins to xyflow edges
  const edges = $derived<Edge[]>(
    qb.joins.map((join) => {
      const sourceNode = qb.tables.find((t) => t.tableName === join.sourceTable);
      const targetNode = qb.tables.find((t) => t.tableName === join.targetTable);

      return {
        id: join.id,
        type: 'joinEdge',
        source: sourceNode?.id ?? '',
        sourceHandle: `${join.sourceColumn}-source`,
        target: targetNode?.id ?? '',
        targetHandle: `${join.targetColumn}-target`,
        data: {
          joinId: join.id,
          joinType: join.joinType,
        },
      };
    })
  );

  function handleNodeDragStop(event: CustomEvent<{ node: Node }>) {
    const { node } = event.detail;
    qb.updateTablePosition(node.id, node.position);
  }

  function handleConnect(event: CustomEvent<Connection>) {
    const { source, sourceHandle, target, targetHandle } = event.detail;
    if (!source || !target || !sourceHandle || !targetHandle) return;

    const sourceNode = qb.tables.find((t) => t.id === source);
    const targetNode = qb.tables.find((t) => t.id === target);
    if (!sourceNode || !targetNode) return;

    // Extract column names from handles
    const sourceColumn = sourceHandle.replace('-source', '');
    const targetColumn = targetHandle.replace('-target', '');

    // Detect join type based on column names
    let joinType: JoinType = 'INNER';

    qb.addJoin(
      sourceNode.tableName,
      sourceColumn,
      targetNode.tableName,
      targetColumn,
      joinType
    );
  }

  function handleDrop(event: DragEvent) {
    event.preventDefault();
    const tableName = event.dataTransfer?.getData('application/table-name');
    if (!tableName) return;

    // Get drop position relative to the canvas
    const canvas = event.currentTarget as HTMLElement;
    const rect = canvas.getBoundingClientRect();
    const position = {
      x: event.clientX - rect.left - 100,
      y: event.clientY - rect.top - 50,
    };

    qb.addTable(tableName, position);
  }

  function handleDragOver(event: DragEvent) {
    event.preventDefault();
    if (event.dataTransfer) {
      event.dataTransfer.dropEffect = 'copy';
    }
  }
</script>

<div
  class="flex-1 h-full"
  ondrop={handleDrop}
  ondragover={handleDragOver}
  role="application"
  aria-label="Query builder canvas"
>
  <SvelteFlow
    {nodes}
    {edges}
    {nodeTypes}
    {edgeTypes}
    fitView
    onnodedragstop={handleNodeDragStop}
    onconnect={handleConnect}
    deleteKeyCode="Delete"
  >
    <Controls />
    <Background variant={BackgroundVariant.Dots} gap={16} />
  </SvelteFlow>
</div>
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/canvas.svelte
git commit -m "feat(tutorial): add main Canvas component with xyflow"
```

---

## Task 10: Component Index Export

**Files:**
- Create: `src/lib/components/query-builder/index.ts`

**Step 1: Create the barrel export file**

```typescript
// src/lib/components/query-builder/index.ts
export { default as QueryBuilderCanvas } from './canvas.svelte';
export { default as TableNode } from './table-node.svelte';
export { default as JoinEdge } from './join-edge.svelte';
export { default as TablePalette } from './table-palette.svelte';
export { default as FilterPanel } from './filter-panel.svelte';
export { default as SqlEditor } from './sql-editor.svelte';
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/index.ts
git commit -m "feat(tutorial): add query-builder component index"
```

---

## Task 11: Learn Sandbox Page

**Files:**
- Create: `src/routes/learn/sandbox/+page.svelte`

**Step 1: Create the sandbox route**

```svelte
<!-- src/routes/learn/sandbox/+page.svelte -->
<script lang="ts">
  import { SidebarInset } from '$lib/components/ui/sidebar';
  import SidebarLeft from '$lib/components/sidebar-left.svelte';
  import {
    QueryBuilderCanvas,
    TablePalette,
    FilterPanel,
    SqlEditor,
  } from '$lib/components/query-builder';
  import {
    QueryBuilderState,
    setQueryBuilder,
  } from '$lib/hooks/query-builder.svelte';
  import { ResizablePaneGroup, ResizablePane, ResizableHandle } from '$lib/components/ui/resizable';
  import { Button } from '$lib/components/ui/button';
  import { RotateCcwIcon, PlayIcon } from '@lucide/svelte';

  // Initialize query builder state
  const qb = setQueryBuilder(new QueryBuilderState());

  function handleReset() {
    qb.reset();
  }
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-2 border-b">
    <div>
      <h1 class="font-semibold">SQL Sandbox</h1>
      <p class="text-sm text-muted-foreground">Build queries visually - drag tables to start</p>
    </div>
    <div class="flex items-center gap-2">
      <Button variant="outline" size="sm" onclick={handleReset}>
        <RotateCcwIcon class="size-4 mr-2" />
        Reset
      </Button>
      <Button size="sm" disabled>
        <PlayIcon class="size-4 mr-2" />
        Run Query
      </Button>
    </div>
  </div>

  <!-- Main content -->
  <div class="flex-1 flex min-h-0">
    <!-- Table palette -->
    <TablePalette />

    <!-- Canvas + SQL editor -->
    <ResizablePaneGroup direction="horizontal" class="flex-1">
      <ResizablePane defaultSize={60} minSize={30}>
        <div class="flex flex-col h-full">
          <!-- Canvas -->
          <div class="flex-1 min-h-0">
            <QueryBuilderCanvas />
          </div>
          <!-- Filter panel -->
          <FilterPanel />
        </div>
      </ResizablePane>

      <ResizableHandle withHandle />

      <ResizablePane defaultSize={40} minSize={20}>
        <SqlEditor />
      </ResizablePane>
    </ResizablePaneGroup>
  </div>
</SidebarInset>
```

**Step 2: Run the app and test**

Run: `npm run tauri dev`

Navigate to `/learn/sandbox` and verify:
- Table palette shows on the left
- Can drag tables onto canvas
- Tables appear as nodes with checkboxes
- Can check columns and see SQL update
- Can create joins by dragging between columns
- Filter panel works
- SQL editor shows generated query

**Step 3: Commit**

```bash
git add src/routes/learn/sandbox/+page.svelte
git commit -m "feat(tutorial): add sandbox page with query builder"
```

---

## Task 12: Update Learn Page with Navigation

**Files:**
- Modify: `src/routes/learn/+page.svelte`

**Step 1: Update to show lesson list with sandbox link**

```svelte
<!-- src/routes/learn/+page.svelte -->
<script lang="ts">
  import { SidebarInset } from '$lib/components/ui/sidebar';
  import SidebarLeft from '$lib/components/sidebar-left.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '$lib/components/ui/card';
  import { Badge } from '$lib/components/ui/badge';
  import {
    GraduationCapIcon,
    PlayIcon,
    BookOpenIcon,
    CheckCircle2Icon,
    LockIcon,
  } from '@lucide/svelte';

  const lessons = [
    {
      id: 'select',
      title: 'SELECT Statements',
      description: 'Learn to retrieve data from tables using SELECT queries',
      status: 'available' as const,
      challenges: 5,
    },
    {
      id: 'where',
      title: 'Filtering with WHERE',
      description: 'Filter query results using conditions',
      status: 'locked' as const,
      challenges: 4,
    },
    {
      id: 'joins',
      title: 'JOINs',
      description: 'Combine data from multiple tables',
      status: 'locked' as const,
      challenges: 6,
    },
    {
      id: 'aggregates',
      title: 'Aggregate Functions',
      description: 'Summarize data with COUNT, SUM, AVG, and more',
      status: 'locked' as const,
      challenges: 5,
    },
  ];
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
  <div class="flex-1 overflow-auto">
    <div class="max-w-4xl mx-auto p-8">
      <!-- Header -->
      <div class="text-center mb-8">
        <GraduationCapIcon class="size-12 mx-auto mb-4 text-primary" />
        <h1 class="text-2xl font-bold mb-2">Learn SQL</h1>
        <p class="text-muted-foreground">
          Master SQL through interactive visual exercises
        </p>
      </div>

      <!-- Sandbox CTA -->
      <Card class="mb-8 border-primary/20 bg-primary/5">
        <CardContent class="flex items-center justify-between p-6">
          <div>
            <h2 class="font-semibold text-lg">SQL Sandbox</h2>
            <p class="text-sm text-muted-foreground">
              Free-form playground to practice building queries
            </p>
          </div>
          <Button href="/learn/sandbox">
            <PlayIcon class="size-4 mr-2" />
            Open Sandbox
          </Button>
        </CardContent>
      </Card>

      <!-- Lessons -->
      <div class="space-y-4">
        <h2 class="font-semibold text-lg">Lessons</h2>
        {#each lessons as lesson (lesson.id)}
          <Card class:opacity-60={lesson.status === 'locked'}>
            <CardHeader class="pb-2">
              <div class="flex items-start justify-between">
                <div>
                  <CardTitle class="flex items-center gap-2">
                    {lesson.title}
                    {#if lesson.status === 'locked'}
                      <LockIcon class="size-4 text-muted-foreground" />
                    {/if}
                  </CardTitle>
                  <CardDescription>{lesson.description}</CardDescription>
                </div>
                <Badge variant="secondary">{lesson.challenges} challenges</Badge>
              </div>
            </CardHeader>
            <CardContent>
              <Button
                variant={lesson.status === 'available' ? 'default' : 'outline'}
                size="sm"
                disabled={lesson.status === 'locked'}
                href={`/learn/${lesson.id}`}
              >
                {#if lesson.status === 'available'}
                  <BookOpenIcon class="size-4 mr-2" />
                  Start Lesson
                {:else}
                  <LockIcon class="size-4 mr-2" />
                  Locked
                {/if}
              </Button>
            </CardContent>
          </Card>
        {/each}
      </div>
    </div>
  </div>
</SidebarInset>
```

**Step 2: Verify the page works**

Run: `npm run tauri dev`

Navigate to `/learn` and verify:
- Shows lesson list
- Sandbox button links to `/learn/sandbox`
- Locked lessons appear disabled

**Step 3: Commit**

```bash
git add src/routes/learn/+page.svelte
git commit -m "feat(tutorial): update learn page with lesson list and sandbox link"
```

---

## Task 13: SQLite Tutorial Database Setup

**Files:**
- Create: `src-tauri/resources/tutorial-seed.sql`
- Create: `src/lib/tutorial/database.ts`

**Step 1: Create the seed SQL file**

```sql
-- src-tauri/resources/tutorial-seed.sql
-- E-commerce sample database for SQL tutorial

CREATE TABLE IF NOT EXISTS categories (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT
);

CREATE TABLE IF NOT EXISTS products (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  price DECIMAL(10,2) NOT NULL,
  stock INTEGER DEFAULT 0,
  category_id INTEGER REFERENCES categories(id),
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS customers (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  email TEXT UNIQUE NOT NULL,
  country TEXT,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER REFERENCES customers(id),
  status TEXT CHECK(status IN ('pending', 'shipped', 'delivered', 'cancelled')) DEFAULT 'pending',
  total DECIMAL(10,2),
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS order_items (
  id INTEGER PRIMARY KEY,
  order_id INTEGER REFERENCES orders(id),
  product_id INTEGER REFERENCES products(id),
  quantity INTEGER NOT NULL,
  unit_price DECIMAL(10,2) NOT NULL
);

CREATE TABLE IF NOT EXISTS reviews (
  id INTEGER PRIMARY KEY,
  product_id INTEGER REFERENCES products(id),
  customer_id INTEGER REFERENCES customers(id),
  rating INTEGER CHECK(rating >= 1 AND rating <= 5),
  comment TEXT,
  created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Seed data: Categories
INSERT INTO categories (id, name, description) VALUES
  (1, 'Electronics', 'Phones, laptops, and gadgets'),
  (2, 'Clothing', 'Apparel and accessories'),
  (3, 'Books', 'Fiction and non-fiction'),
  (4, 'Home', 'Furniture and decor'),
  (5, 'Sports', 'Equipment and gear');

-- Seed data: Products (10 per category = 50 total)
INSERT INTO products (id, name, description, price, stock, category_id) VALUES
  -- Electronics
  (1, 'Smartphone X', 'Latest smartphone with 5G', 799.99, 50, 1),
  (2, 'Laptop Pro', '15-inch professional laptop', 1299.99, 30, 1),
  (3, 'Wireless Earbuds', 'Noise-canceling earbuds', 149.99, 100, 1),
  (4, 'Tablet Mini', '8-inch portable tablet', 399.99, 45, 1),
  (5, 'Smart Watch', 'Fitness and notification watch', 249.99, 75, 1),
  (6, '4K Monitor', '27-inch 4K display', 449.99, 25, 1),
  (7, 'Mechanical Keyboard', 'RGB mechanical keyboard', 129.99, 60, 1),
  (8, 'Wireless Mouse', 'Ergonomic wireless mouse', 59.99, 80, 1),
  (9, 'Portable Charger', '20000mAh power bank', 39.99, 150, 1),
  (10, 'USB-C Hub', '7-in-1 USB-C adapter', 49.99, 90, 1),
  -- Clothing
  (11, 'Cotton T-Shirt', 'Classic fit cotton tee', 24.99, 200, 2),
  (12, 'Denim Jeans', 'Slim fit blue jeans', 59.99, 100, 2),
  (13, 'Wool Sweater', 'Warm winter sweater', 79.99, 50, 2),
  (14, 'Running Shoes', 'Lightweight running shoes', 119.99, 75, 2),
  (15, 'Leather Belt', 'Genuine leather belt', 34.99, 120, 2),
  (16, 'Winter Jacket', 'Insulated winter coat', 149.99, 40, 2),
  (17, 'Casual Shorts', 'Comfortable cotton shorts', 29.99, 90, 2),
  (18, 'Dress Shirt', 'Formal button-down shirt', 49.99, 70, 2),
  (19, 'Sneakers', 'Casual everyday sneakers', 89.99, 85, 2),
  (20, 'Baseball Cap', 'Adjustable baseball cap', 19.99, 150, 2),
  -- Books
  (21, 'The Great Novel', 'Bestselling fiction', 14.99, 200, 3),
  (22, 'Learn SQL', 'Database fundamentals', 39.99, 100, 3),
  (23, 'History of Time', 'Popular science book', 24.99, 80, 3),
  (24, 'Cooking Basics', 'Essential recipes', 29.99, 60, 3),
  (25, 'Business Strategy', 'Management guide', 34.99, 45, 3),
  (26, 'Poetry Collection', 'Modern poetry anthology', 19.99, 70, 3),
  (27, 'Travel Guide', 'World destinations', 22.99, 55, 3),
  (28, 'Art History', 'Visual arts overview', 44.99, 35, 3),
  (29, 'Self Improvement', 'Personal growth guide', 16.99, 120, 3),
  (30, 'Mystery Novel', 'Thrilling detective story', 12.99, 90, 3),
  -- Home
  (31, 'Coffee Table', 'Modern wooden table', 199.99, 20, 4),
  (32, 'Floor Lamp', 'Adjustable LED lamp', 79.99, 40, 4),
  (33, 'Throw Pillow', 'Decorative cushion', 24.99, 100, 4),
  (34, 'Wall Clock', 'Minimalist wall clock', 34.99, 60, 4),
  (35, 'Bookshelf', '5-tier wooden shelf', 149.99, 25, 4),
  (36, 'Area Rug', '5x7 woven area rug', 89.99, 30, 4),
  (37, 'Picture Frame', '8x10 photo frame', 14.99, 150, 4),
  (38, 'Candle Set', 'Scented candle trio', 29.99, 80, 4),
  (39, 'Plant Pot', 'Ceramic planter', 19.99, 90, 4),
  (40, 'Mirror', 'Round wall mirror', 59.99, 35, 4),
  -- Sports
  (41, 'Yoga Mat', 'Non-slip exercise mat', 29.99, 100, 5),
  (42, 'Dumbbells', '10lb dumbbell pair', 49.99, 50, 5),
  (43, 'Tennis Racket', 'Lightweight racket', 89.99, 30, 5),
  (44, 'Basketball', 'Official size basketball', 24.99, 70, 5),
  (45, 'Cycling Helmet', 'Safety helmet', 59.99, 45, 5),
  (46, 'Resistance Bands', 'Set of 5 bands', 19.99, 120, 5),
  (47, 'Jump Rope', 'Speed jump rope', 12.99, 150, 5),
  (48, 'Water Bottle', '32oz sports bottle', 14.99, 200, 5),
  (49, 'Gym Bag', 'Durable sports bag', 39.99, 60, 5),
  (50, 'Soccer Ball', 'Size 5 soccer ball', 29.99, 80, 5);

-- Seed data: Customers (100 total)
INSERT INTO customers (id, name, email, country) VALUES
  (1, 'Alice Johnson', 'alice@email.com', 'USA'),
  (2, 'Bob Smith', 'bob@email.com', 'USA'),
  (3, 'Carol White', 'carol@email.com', 'Canada'),
  (4, 'David Brown', 'david@email.com', 'UK'),
  (5, 'Emma Davis', 'emma@email.com', 'Australia'),
  (6, 'Frank Miller', 'frank@email.com', 'USA'),
  (7, 'Grace Wilson', 'grace@email.com', 'Canada'),
  (8, 'Henry Taylor', 'henry@email.com', 'UK'),
  (9, 'Ivy Anderson', 'ivy@email.com', 'Germany'),
  (10, 'Jack Thomas', 'jack@email.com', 'France'),
  (11, 'Karen Martinez', 'karen@email.com', 'Spain'),
  (12, 'Leo Garcia', 'leo@email.com', 'Mexico'),
  (13, 'Mia Robinson', 'mia@email.com', 'USA'),
  (14, 'Noah Clark', 'noah@email.com', 'USA'),
  (15, 'Olivia Lewis', 'olivia@email.com', 'Canada'),
  (16, 'Paul Walker', 'paul@email.com', 'UK'),
  (17, 'Quinn Hall', 'quinn@email.com', 'Australia'),
  (18, 'Ruby Allen', 'ruby@email.com', 'USA'),
  (19, 'Sam Young', 'sam@email.com', 'Canada'),
  (20, 'Tina King', 'tina@email.com', 'UK');

-- Seed more customers (21-100) with variety of countries
INSERT INTO customers (id, name, email, country)
SELECT
  20 + seq,
  'Customer ' || (20 + seq),
  'customer' || (20 + seq) || '@email.com',
  CASE (seq % 10)
    WHEN 0 THEN 'USA'
    WHEN 1 THEN 'Canada'
    WHEN 2 THEN 'UK'
    WHEN 3 THEN 'Germany'
    WHEN 4 THEN 'France'
    WHEN 5 THEN 'Australia'
    WHEN 6 THEN 'Spain'
    WHEN 7 THEN 'Mexico'
    WHEN 8 THEN 'Japan'
    WHEN 9 THEN 'Brazil'
  END
FROM (
  SELECT 1 as seq UNION SELECT 2 UNION SELECT 3 UNION SELECT 4 UNION SELECT 5
  UNION SELECT 6 UNION SELECT 7 UNION SELECT 8 UNION SELECT 9 UNION SELECT 10
  UNION SELECT 11 UNION SELECT 12 UNION SELECT 13 UNION SELECT 14 UNION SELECT 15
  UNION SELECT 16 UNION SELECT 17 UNION SELECT 18 UNION SELECT 19 UNION SELECT 20
  UNION SELECT 21 UNION SELECT 22 UNION SELECT 23 UNION SELECT 24 UNION SELECT 25
  UNION SELECT 26 UNION SELECT 27 UNION SELECT 28 UNION SELECT 29 UNION SELECT 30
  UNION SELECT 31 UNION SELECT 32 UNION SELECT 33 UNION SELECT 34 UNION SELECT 35
  UNION SELECT 36 UNION SELECT 37 UNION SELECT 38 UNION SELECT 39 UNION SELECT 40
  UNION SELECT 41 UNION SELECT 42 UNION SELECT 43 UNION SELECT 44 UNION SELECT 45
  UNION SELECT 46 UNION SELECT 47 UNION SELECT 48 UNION SELECT 49 UNION SELECT 50
  UNION SELECT 51 UNION SELECT 52 UNION SELECT 53 UNION SELECT 54 UNION SELECT 55
  UNION SELECT 56 UNION SELECT 57 UNION SELECT 58 UNION SELECT 59 UNION SELECT 60
  UNION SELECT 61 UNION SELECT 62 UNION SELECT 63 UNION SELECT 64 UNION SELECT 65
  UNION SELECT 66 UNION SELECT 67 UNION SELECT 68 UNION SELECT 69 UNION SELECT 70
  UNION SELECT 71 UNION SELECT 72 UNION SELECT 73 UNION SELECT 74 UNION SELECT 75
  UNION SELECT 76 UNION SELECT 77 UNION SELECT 78 UNION SELECT 79 UNION SELECT 80
);

-- Seed data: Orders (500 total) - using simpler approach
INSERT INTO orders (id, customer_id, status, total, created_at)
SELECT
  seq,
  1 + (seq % 100),
  CASE (seq % 4)
    WHEN 0 THEN 'pending'
    WHEN 1 THEN 'shipped'
    WHEN 2 THEN 'delivered'
    WHEN 3 THEN 'cancelled'
  END,
  ROUND(50 + (seq % 450) + 0.99, 2),
  datetime('now', '-' || (seq % 730) || ' days')
FROM (
  WITH RECURSIVE cnt(seq) AS (
    SELECT 1
    UNION ALL
    SELECT seq + 1 FROM cnt WHERE seq < 500
  )
  SELECT seq FROM cnt
);

-- Seed data: Order Items (1500 total, ~3 per order)
INSERT INTO order_items (id, order_id, product_id, quantity, unit_price)
SELECT
  seq,
  1 + ((seq - 1) / 3),
  1 + (seq % 50),
  1 + (seq % 5),
  ROUND(10 + (seq % 100) + 0.99, 2)
FROM (
  WITH RECURSIVE cnt(seq) AS (
    SELECT 1
    UNION ALL
    SELECT seq + 1 FROM cnt WHERE seq < 1500
  )
  SELECT seq FROM cnt
);

-- Seed data: Reviews (200 total)
INSERT INTO reviews (id, product_id, customer_id, rating, comment, created_at)
SELECT
  seq,
  1 + (seq % 50),
  1 + (seq % 100),
  1 + (seq % 5),
  CASE (seq % 5)
    WHEN 0 THEN 'Terrible product, do not buy!'
    WHEN 1 THEN 'Not great, had some issues.'
    WHEN 2 THEN 'Average product, nothing special.'
    WHEN 3 THEN 'Good product, would recommend.'
    WHEN 4 THEN 'Excellent! Best purchase ever!'
  END,
  datetime('now', '-' || (seq % 365) || ' days')
FROM (
  WITH RECURSIVE cnt(seq) AS (
    SELECT 1
    UNION ALL
    SELECT seq + 1 FROM cnt WHERE seq < 200
  )
  SELECT seq FROM cnt
);
```

**Step 2: Create database helper module**

```typescript
// src/lib/tutorial/database.ts
import Database from '@tauri-apps/plugin-sql';

let tutorialDb: Database | null = null;

/**
 * Get or create the tutorial SQLite database connection.
 * The database is created in-memory and seeded on first access.
 */
export async function getTutorialDatabase(): Promise<Database> {
  if (tutorialDb) {
    return tutorialDb;
  }

  // Create in-memory SQLite database
  tutorialDb = await Database.load('sqlite::memory:');

  // Seed the database
  await seedDatabase(tutorialDb);

  return tutorialDb;
}

async function seedDatabase(db: Database): Promise<void> {
  // Create tables
  await db.execute(`
    CREATE TABLE IF NOT EXISTS categories (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS products (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      description TEXT,
      price DECIMAL(10,2) NOT NULL,
      stock INTEGER DEFAULT 0,
      category_id INTEGER REFERENCES categories(id),
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS customers (
      id INTEGER PRIMARY KEY,
      name TEXT NOT NULL,
      email TEXT UNIQUE NOT NULL,
      country TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS orders (
      id INTEGER PRIMARY KEY,
      customer_id INTEGER REFERENCES customers(id),
      status TEXT CHECK(status IN ('pending', 'shipped', 'delivered', 'cancelled')) DEFAULT 'pending',
      total DECIMAL(10,2),
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS order_items (
      id INTEGER PRIMARY KEY,
      order_id INTEGER REFERENCES orders(id),
      product_id INTEGER REFERENCES products(id),
      quantity INTEGER NOT NULL,
      unit_price DECIMAL(10,2) NOT NULL
    )
  `);

  await db.execute(`
    CREATE TABLE IF NOT EXISTS reviews (
      id INTEGER PRIMARY KEY,
      product_id INTEGER REFERENCES products(id),
      customer_id INTEGER REFERENCES customers(id),
      rating INTEGER CHECK(rating >= 1 AND rating <= 5),
      comment TEXT,
      created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )
  `);

  // Seed categories
  await db.execute(`
    INSERT INTO categories (id, name, description) VALUES
      (1, 'Electronics', 'Phones, laptops, and gadgets'),
      (2, 'Clothing', 'Apparel and accessories'),
      (3, 'Books', 'Fiction and non-fiction'),
      (4, 'Home', 'Furniture and decor'),
      (5, 'Sports', 'Equipment and gear')
  `);

  // Seed sample products (simplified - 10 products)
  await db.execute(`
    INSERT INTO products (id, name, description, price, stock, category_id) VALUES
      (1, 'Smartphone X', 'Latest smartphone with 5G', 799.99, 50, 1),
      (2, 'Laptop Pro', '15-inch professional laptop', 1299.99, 30, 1),
      (3, 'Wireless Earbuds', 'Noise-canceling earbuds', 149.99, 100, 1),
      (4, 'Cotton T-Shirt', 'Classic fit cotton tee', 24.99, 200, 2),
      (5, 'Denim Jeans', 'Slim fit blue jeans', 59.99, 100, 2),
      (6, 'The Great Novel', 'Bestselling fiction', 14.99, 200, 3),
      (7, 'Learn SQL', 'Database fundamentals', 39.99, 100, 3),
      (8, 'Coffee Table', 'Modern wooden table', 199.99, 20, 4),
      (9, 'Yoga Mat', 'Non-slip exercise mat', 29.99, 100, 5),
      (10, 'Dumbbells', '10lb dumbbell pair', 49.99, 50, 5)
  `);

  // Seed sample customers
  await db.execute(`
    INSERT INTO customers (id, name, email, country) VALUES
      (1, 'Alice Johnson', 'alice@email.com', 'USA'),
      (2, 'Bob Smith', 'bob@email.com', 'USA'),
      (3, 'Carol White', 'carol@email.com', 'Canada'),
      (4, 'David Brown', 'david@email.com', 'UK'),
      (5, 'Emma Davis', 'emma@email.com', 'Australia')
  `);

  // Seed sample orders
  await db.execute(`
    INSERT INTO orders (id, customer_id, status, total) VALUES
      (1, 1, 'delivered', 849.98),
      (2, 1, 'shipped', 24.99),
      (3, 2, 'pending', 1349.98),
      (4, 3, 'delivered', 89.98),
      (5, 4, 'cancelled', 199.99)
  `);

  // Seed sample order items
  await db.execute(`
    INSERT INTO order_items (id, order_id, product_id, quantity, unit_price) VALUES
      (1, 1, 1, 1, 799.99),
      (2, 1, 3, 1, 49.99),
      (3, 2, 4, 1, 24.99),
      (4, 3, 2, 1, 1299.99),
      (5, 3, 10, 1, 49.99),
      (6, 4, 5, 1, 59.99),
      (7, 4, 9, 1, 29.99),
      (8, 5, 8, 1, 199.99)
  `);

  // Seed sample reviews
  await db.execute(`
    INSERT INTO reviews (id, product_id, customer_id, rating, comment) VALUES
      (1, 1, 1, 5, 'Excellent phone!'),
      (2, 2, 2, 4, 'Great laptop, a bit pricey'),
      (3, 4, 3, 5, 'Very comfortable'),
      (4, 6, 4, 3, 'Good read'),
      (5, 9, 5, 5, 'Perfect for yoga')
  `);
}

/**
 * Execute a query on the tutorial database and return results.
 */
export async function executeQuery(sql: string): Promise<Record<string, unknown>[]> {
  const db = await getTutorialDatabase();
  return db.select<Record<string, unknown>[]>(sql);
}

/**
 * Close the tutorial database connection.
 */
export async function closeTutorialDatabase(): Promise<void> {
  if (tutorialDb) {
    await tutorialDb.close();
    tutorialDb = null;
  }
}
```

**Step 3: Verify types compile**

Run: `npm run check`

**Step 4: Commit**

```bash
git add src-tauri/resources/tutorial-seed.sql src/lib/tutorial/database.ts
git commit -m "feat(tutorial): add SQLite database setup and seed data"
```

---

## Task 14: Challenge Criteria System

**Files:**
- Create: `src/lib/tutorial/criteria.ts`

**Step 1: Create validation functions**

```typescript
// src/lib/tutorial/criteria.ts
import type { QueryBuilderSnapshot, ChallengeCriterion } from '$lib/types';

type CriterionCheck = (state: QueryBuilderSnapshot, sql: string) => boolean;

/**
 * Check if a specific table is on the canvas.
 */
export function hasTable(tableName: string): CriterionCheck {
  return (state) => state.tables.some((t) => t.tableName === tableName);
}

/**
 * Check if a specific column is selected in a table.
 */
export function hasColumn(tableName: string, columnName: string): CriterionCheck {
  return (state) => {
    const table = state.tables.find((t) => t.tableName === tableName);
    return table ? table.selectedColumns.has(columnName) : false;
  };
}

/**
 * Check if any columns are selected.
 */
export function hasAnyColumns(): CriterionCheck {
  return (state) => state.tables.some((t) => t.selectedColumns.size > 0);
}

/**
 * Check if at least N columns are selected total.
 */
export function hasAtLeastColumns(count: number): CriterionCheck {
  return (state) => {
    const total = state.tables.reduce((sum, t) => sum + t.selectedColumns.size, 0);
    return total >= count;
  };
}

/**
 * Check if a JOIN exists between two tables.
 */
export function hasJoin(table1: string, table2: string): CriterionCheck {
  return (state) =>
    state.joins.some(
      (j) =>
        (j.sourceTable === table1 && j.targetTable === table2) ||
        (j.sourceTable === table2 && j.targetTable === table1)
    );
}

/**
 * Check if a specific join type is used.
 */
export function hasJoinType(joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL'): CriterionCheck {
  return (state) => state.joins.some((j) => j.joinType === joinType);
}

/**
 * Check if a WHERE filter exists on a column with a specific operator.
 */
export function hasFilter(
  column: string,
  operator?: string,
  value?: string | number
): CriterionCheck {
  return (state) =>
    state.filters.some((f) => {
      if (f.column !== column) return false;
      if (operator && f.operator !== operator) return false;
      if (value !== undefined && f.value !== String(value)) return false;
      return true;
    });
}

/**
 * Check if any WHERE filter exists.
 */
export function hasAnyFilter(): CriterionCheck {
  return (state) => state.filters.length > 0;
}

/**
 * Check if ORDER BY is applied on a specific column.
 */
export function hasOrderBy(column: string, direction?: 'ASC' | 'DESC'): CriterionCheck {
  return (state) =>
    state.orderBy.some((o) => {
      if (o.column !== column) return false;
      if (direction && o.direction !== direction) return false;
      return true;
    });
}

/**
 * Check if any ORDER BY is applied.
 */
export function hasAnyOrderBy(): CriterionCheck {
  return (state) => state.orderBy.length > 0;
}

/**
 * Check if LIMIT is set to a specific value.
 */
export function hasLimit(value?: number): CriterionCheck {
  return (state) => {
    if (value === undefined) {
      return state.limit !== null;
    }
    return state.limit === value;
  };
}

/**
 * Check if the SQL contains a specific keyword (case-insensitive).
 */
export function sqlContains(keyword: string): CriterionCheck {
  return (_, sql) => sql.toUpperCase().includes(keyword.toUpperCase());
}

/**
 * Create a ChallengeCriterion from a description and check function.
 */
export function criterion(
  id: string,
  description: string,
  check: CriterionCheck
): Omit<ChallengeCriterion, 'satisfied'> {
  return { id, description, check };
}
```

**Step 2: Verify types compile**

Run: `npm run check`

**Step 3: Commit**

```bash
git add src/lib/tutorial/criteria.ts
git commit -m "feat(tutorial): add challenge validation criteria functions"
```

---

## Task 15: SELECT Lesson Content

**Files:**
- Create: `src/lib/tutorial/lessons/select.ts`

**Step 1: Create the SELECT lesson with challenges**

```typescript
// src/lib/tutorial/lessons/select.ts
import type { TutorialLesson } from '$lib/types';
import {
  criterion,
  hasTable,
  hasColumn,
  hasAnyColumns,
  hasAtLeastColumns,
  hasAnyFilter,
  hasFilter,
  hasOrderBy,
  hasLimit,
} from '../criteria';

export const selectLesson: TutorialLesson = {
  id: 'select',
  title: 'SELECT Statements',
  introduction: `
The SELECT statement is the most fundamental SQL command. It retrieves data from one or more tables in a database.

Every SELECT query has at least two parts:
- **SELECT** - specifies which columns you want to retrieve
- **FROM** - specifies which table to get the data from

In this lesson, you'll learn to build SELECT queries step by step using the visual query builder. Drag tables onto the canvas, check the columns you want, and watch the SQL update in real-time.
  `.trim(),
  challenges: [
    {
      id: 'select-1',
      title: 'Your First Query',
      description:
        'Drag the "products" table onto the canvas and select the "name" and "price" columns.',
      hint: 'Look for the products table in the left panel. Drag it onto the canvas, then check the boxes next to "name" and "price".',
      criteria: [
        criterion('has-products', 'Add the products table to the canvas', hasTable('products')),
        criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
        criterion('has-price', 'Select the price column', hasColumn('products', 'price')),
      ],
    },
    {
      id: 'select-2',
      title: 'Select All Columns',
      description:
        'Sometimes you want all columns from a table. Add the "customers" table and select all of its columns.',
      hint: 'Use the "All" button in the table header to select all columns at once.',
      criteria: [
        criterion('has-customers', 'Add the customers table', hasTable('customers')),
        criterion('has-id', 'Select the id column', hasColumn('customers', 'id')),
        criterion('has-name', 'Select the name column', hasColumn('customers', 'name')),
        criterion('has-email', 'Select the email column', hasColumn('customers', 'email')),
        criterion('has-country', 'Select the country column', hasColumn('customers', 'country')),
        criterion(
          'has-created',
          'Select the created_at column',
          hasColumn('customers', 'created_at')
        ),
      ],
    },
    {
      id: 'select-3',
      title: 'Filtering Results',
      description:
        'Add a WHERE clause to filter products. Show only products with a price greater than $50.',
      hint: 'Use the WHERE section in the filter panel below the canvas. Add a filter on products.price with the "greater than" operator.',
      criteria: [
        criterion('has-products', 'Add the products table', hasTable('products')),
        criterion('has-columns', 'Select at least one column', hasAnyColumns()),
        criterion(
          'has-filter',
          'Add a WHERE filter on price > 50',
          hasFilter('products.price', '>', '50')
        ),
      ],
    },
    {
      id: 'select-4',
      title: 'Sorting Results',
      description: 'Sort the products by price in descending order (highest first).',
      hint: 'Use the ORDER BY section in the filter panel. Add products.price and set direction to DESC.',
      criteria: [
        criterion('has-products', 'Add the products table', hasTable('products')),
        criterion('has-columns', 'Select at least one column', hasAnyColumns()),
        criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC')),
      ],
    },
    {
      id: 'select-5',
      title: 'Limiting Results',
      description: 'Show only the top 5 most expensive products.',
      hint: 'Combine ORDER BY (price DESC) with LIMIT 5 to get just the top 5.',
      criteria: [
        criterion('has-products', 'Add the products table', hasTable('products')),
        criterion('has-name', 'Select the name column', hasColumn('products', 'name')),
        criterion('has-price', 'Select the price column', hasColumn('products', 'price')),
        criterion('has-order', 'Sort by price descending', hasOrderBy('products.price', 'DESC')),
        criterion('has-limit', 'Limit to 5 results', hasLimit(5)),
      ],
    },
  ],
};
```

**Step 2: Create lessons index**

```typescript
// src/lib/tutorial/lessons/index.ts
export { selectLesson } from './select';

import type { TutorialLesson } from '$lib/types';
import { selectLesson } from './select';

export const LESSONS: Record<string, TutorialLesson> = {
  select: selectLesson,
};

export function getLesson(id: string): TutorialLesson | undefined {
  return LESSONS[id];
}
```

**Step 3: Verify types compile**

Run: `npm run check`

**Step 4: Commit**

```bash
git add src/lib/tutorial/lessons/select.ts src/lib/tutorial/lessons/index.ts
git commit -m "feat(tutorial): add SELECT lesson with 5 challenges"
```

---

## Task 16: Challenge Card Component

**Files:**
- Create: `src/lib/components/query-builder/challenge-card.svelte`

**Step 1: Create the challenge display component**

```svelte
<!-- src/lib/components/query-builder/challenge-card.svelte -->
<script lang="ts">
  import type { Challenge, ChallengeCriterion, QueryBuilderSnapshot } from '$lib/types';
  import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { Button } from '$lib/components/ui/button';
  import { Card, CardHeader, CardTitle, CardContent } from '$lib/components/ui/card';
  import { Badge } from '$lib/components/ui/badge';
  import {
    CheckCircle2Icon,
    CircleIcon,
    LightbulbIcon,
    ChevronRightIcon,
    SkipForwardIcon,
  } from '@lucide/svelte';

  interface Props {
    challenge: Challenge;
    challengeIndex: number;
    totalChallenges: number;
    onComplete?: () => void;
    onSkip?: () => void;
  }

  let { challenge, challengeIndex, totalChallenges, onComplete, onSkip }: Props = $props();

  const qb = useQueryBuilder();

  let showHint = $state(false);

  // Evaluate criteria against current state
  const evaluatedCriteria = $derived<ChallengeCriterion[]>(
    challenge.criteria.map((c) => ({
      ...c,
      satisfied: c.check(qb.snapshot, qb.generatedSql),
    }))
  );

  const isComplete = $derived(evaluatedCriteria.every((c) => c.satisfied));
  const completedCount = $derived(evaluatedCriteria.filter((c) => c.satisfied).length);

  // Auto-notify on completion
  $effect(() => {
    if (isComplete && onComplete) {
      onComplete();
    }
  });
</script>

<Card class="border-primary/20">
  <CardHeader class="pb-2">
    <div class="flex items-center justify-between">
      <Badge variant="secondary">
        Challenge {challengeIndex + 1} of {totalChallenges}
      </Badge>
      {#if isComplete}
        <Badge class="bg-green-500 text-white">Complete!</Badge>
      {:else}
        <Badge variant="outline">
          {completedCount}/{evaluatedCriteria.length}
        </Badge>
      {/if}
    </div>
    <CardTitle class="text-base">{challenge.title}</CardTitle>
  </CardHeader>

  <CardContent class="space-y-4">
    <p class="text-sm text-muted-foreground">{challenge.description}</p>

    <!-- Criteria checklist -->
    <div class="space-y-2">
      {#each evaluatedCriteria as criterion (criterion.id)}
        <div class="flex items-center gap-2 text-sm">
          {#if criterion.satisfied}
            <CheckCircle2Icon class="size-4 text-green-500 shrink-0" />
          {:else}
            <CircleIcon class="size-4 text-muted-foreground shrink-0" />
          {/if}
          <span class:text-muted-foreground={!criterion.satisfied}>
            {criterion.description}
          </span>
        </div>
      {/each}
    </div>

    <!-- Hint -->
    {#if challenge.hint}
      {#if showHint}
        <div class="p-3 bg-muted rounded-md">
          <div class="flex items-start gap-2">
            <LightbulbIcon class="size-4 text-yellow-500 shrink-0 mt-0.5" />
            <p class="text-sm">{challenge.hint}</p>
          </div>
        </div>
      {:else}
        <Button variant="ghost" size="sm" onclick={() => (showHint = true)}>
          <LightbulbIcon class="size-4 mr-2" />
          Show Hint
        </Button>
      {/if}
    {/if}

    <!-- Actions -->
    <div class="flex justify-between pt-2">
      <Button variant="ghost" size="sm" onclick={onSkip}>
        <SkipForwardIcon class="size-4 mr-2" />
        Skip
      </Button>

      {#if isComplete}
        <Button size="sm" onclick={onComplete}>
          Next Challenge
          <ChevronRightIcon class="size-4 ml-2" />
        </Button>
      {/if}
    </div>
  </CardContent>
</Card>
```

**Step 2: Export from index**

Add to `src/lib/components/query-builder/index.ts`:

```typescript
export { default as ChallengeCard } from './challenge-card.svelte';
```

**Step 3: Verify types compile**

Run: `npm run check`

**Step 4: Commit**

```bash
git add src/lib/components/query-builder/challenge-card.svelte src/lib/components/query-builder/index.ts
git commit -m "feat(tutorial): add ChallengeCard component with criteria validation"
```

---

## Task 17: Lesson Page Route

**Files:**
- Create: `src/routes/learn/[lessonId]/+page.svelte`

**Step 1: Create the dynamic lesson route**

```svelte
<!-- src/routes/learn/[lessonId]/+page.svelte -->
<script lang="ts">
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import { SidebarInset } from '$lib/components/ui/sidebar';
  import SidebarLeft from '$lib/components/sidebar-left.svelte';
  import { Button } from '$lib/components/ui/button';
  import {
    ResizablePaneGroup,
    ResizablePane,
    ResizableHandle,
  } from '$lib/components/ui/resizable';
  import {
    QueryBuilderCanvas,
    TablePalette,
    FilterPanel,
    SqlEditor,
    ChallengeCard,
  } from '$lib/components/query-builder';
  import { QueryBuilderState, setQueryBuilder } from '$lib/hooks/query-builder.svelte';
  import { getLesson } from '$lib/tutorial/lessons';
  import {
    ArrowLeftIcon,
    RotateCcwIcon,
    BookOpenIcon,
    CheckCircle2Icon,
    PlayIcon,
  } from '@lucide/svelte';

  const qb = setQueryBuilder(new QueryBuilderState());

  const lessonId = $derived(page.params.lessonId);
  const lesson = $derived(getLesson(lessonId));

  let currentChallengeIndex = $state(0);
  let completedChallenges = $state<Set<string>>(new Set());

  const currentChallenge = $derived(lesson?.challenges[currentChallengeIndex]);
  const isLessonComplete = $derived(
    lesson ? completedChallenges.size === lesson.challenges.length : false
  );

  function handleChallengeComplete() {
    if (currentChallenge) {
      completedChallenges.add(currentChallenge.id);
      completedChallenges = completedChallenges; // Trigger reactivity
    }
  }

  function handleNextChallenge() {
    if (lesson && currentChallengeIndex < lesson.challenges.length - 1) {
      currentChallengeIndex++;
      qb.reset();
    }
  }

  function handleSkip() {
    handleNextChallenge();
  }

  function handleReset() {
    qb.reset();
  }

  function handleBack() {
    goto('/learn');
  }
</script>

<SidebarLeft />

<SidebarInset class="flex flex-col h-full overflow-hidden">
  {#if lesson}
    <!-- Header -->
    <div class="flex items-center justify-between px-4 py-2 border-b">
      <div class="flex items-center gap-3">
        <Button variant="ghost" size="icon-sm" onclick={handleBack}>
          <ArrowLeftIcon class="size-4" />
        </Button>
        <div>
          <h1 class="font-semibold">{lesson.title}</h1>
          <p class="text-xs text-muted-foreground">
            Challenge {currentChallengeIndex + 1} of {lesson.challenges.length}
          </p>
        </div>
      </div>
      <div class="flex items-center gap-2">
        <Button variant="outline" size="sm" onclick={handleReset}>
          <RotateCcwIcon class="size-4 mr-2" />
          Reset
        </Button>
      </div>
    </div>

    <!-- Main content -->
    <div class="flex-1 flex min-h-0">
      <!-- Left panel: Challenge + Table palette -->
      <div class="w-64 border-r flex flex-col">
        <!-- Challenge card -->
        <div class="p-3 border-b">
          {#if currentChallenge}
            <ChallengeCard
              challenge={currentChallenge}
              challengeIndex={currentChallengeIndex}
              totalChallenges={lesson.challenges.length}
              onComplete={handleNextChallenge}
              onSkip={handleSkip}
            />
          {:else if isLessonComplete}
            <div class="text-center py-8">
              <CheckCircle2Icon class="size-12 mx-auto mb-4 text-green-500" />
              <h2 class="font-semibold mb-2">Lesson Complete!</h2>
              <p class="text-sm text-muted-foreground mb-4">
                You've completed all challenges.
              </p>
              <Button onclick={handleBack}>Back to Lessons</Button>
            </div>
          {/if}
        </div>

        <!-- Table palette -->
        <div class="flex-1 min-h-0">
          <TablePalette />
        </div>
      </div>

      <!-- Canvas + SQL editor -->
      <ResizablePaneGroup direction="horizontal" class="flex-1">
        <ResizablePane defaultSize={60} minSize={30}>
          <div class="flex flex-col h-full">
            <div class="flex-1 min-h-0">
              <QueryBuilderCanvas />
            </div>
            <FilterPanel />
          </div>
        </ResizablePane>

        <ResizableHandle withHandle />

        <ResizablePane defaultSize={40} minSize={20}>
          <SqlEditor />
        </ResizablePane>
      </ResizablePaneGroup>
    </div>
  {:else}
    <!-- Lesson not found -->
    <div class="flex-1 flex items-center justify-center">
      <div class="text-center">
        <BookOpenIcon class="size-12 mx-auto mb-4 text-muted-foreground" />
        <h2 class="font-semibold mb-2">Lesson not found</h2>
        <p class="text-sm text-muted-foreground mb-4">
          The lesson "{lessonId}" doesn't exist.
        </p>
        <Button onclick={handleBack}>Back to Lessons</Button>
      </div>
    </div>
  {/if}
</SidebarInset>
```

**Step 2: Test the lesson page**

Run: `npm run tauri dev`

Navigate to `/learn/select` and verify:
- Lesson loads with introduction
- Challenge card shows with criteria
- Completing criteria shows checkmarks
- Can navigate between challenges

**Step 3: Commit**

```bash
git add src/routes/learn/[lessonId]/+page.svelte
git commit -m "feat(tutorial): add dynamic lesson page with challenge progression"
```

---

## Summary

This plan creates a complete interactive SQL query builder tutorial with:

1. **Types** - TypeScript interfaces for all data structures
2. **Schema** - E-commerce sample database definition
3. **State** - Reactive `QueryBuilderState` class
4. **Components**:
   - `TableNode` - Custom xyflow node with column checkboxes
   - `JoinEdge` - Custom edge with join type selector
   - `TablePalette` - Draggable table list
   - `FilterPanel` - WHERE/ORDER BY/LIMIT controls
   - `SqlEditor` - Monaco editor with two-way sync
   - `ChallengeCard` - Progress display with criteria checklist
   - `Canvas` - Main xyflow canvas
5. **Database** - SQLite setup with seed data
6. **Lessons** - SELECT lesson with 5 challenges
7. **Routes** - `/learn`, `/learn/sandbox`, `/learn/[lessonId]`

Each task is atomic and can be committed independently. The total is 17 tasks.
