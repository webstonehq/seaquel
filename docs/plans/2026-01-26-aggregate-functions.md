# Aggregate Functions Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add visual support for aggregate functions (SUM, AVG, COUNT, MIN, MAX) in the query builder SELECT clause.

**Architecture:** Two complementary interfaces - per-column aggregates on table nodes and a dedicated Aggregates panel in the filter panel. Both integrate with the existing two-way SQL sync.

**Tech Stack:** SvelteKit 5, Svelte 5 runes, TypeScript, bits-ui components, node-sql-parser

---

## Task 1: Add SelectAggregate Type

**Files:**
- Modify: `src/lib/types/query-builder.ts:115` (after AggregateFunction type)
- Modify: `src/lib/types/index.ts` (add export)

**Step 1: Add SelectAggregate interface to query-builder.ts**

Add after line 115 (after `AggregateFunction` type):

```typescript
/**
 * A standalone aggregate in the SELECT clause.
 * Used for COUNT(*), expressions, or aggregates not tied to a specific column.
 */
export interface SelectAggregate {
	/** Unique identifier for this aggregate */
	id: string;
	/** Aggregate function (COUNT, SUM, AVG, MIN, MAX) */
	function: AggregateFunction;
	/** Expression inside the aggregate (*, column name, or expression like "price * quantity") */
	expression: string;
	/** Optional alias for AS clause */
	alias?: string;
}

/**
 * Per-column aggregate applied to a selected column.
 */
export interface ColumnAggregate {
	/** Aggregate function (COUNT, SUM, AVG, MIN, MAX) */
	function: AggregateFunction;
	/** Optional alias for AS clause */
	alias?: string;
}
```

**Step 2: Update CanvasTable to include columnAggregates**

Modify `CanvasTable` interface (around line 37) to add the new field:

```typescript
export interface CanvasTable {
	/** Unique identifier for this canvas table instance */
	id: string;
	/** Name of the table from the schema */
	tableName: string;
	/** Position on the canvas */
	position: { x: number; y: number };
	/** Set of column names currently selected for the query */
	selectedColumns: Set<string>;
	/** Map of column name to aggregate function applied to it */
	columnAggregates: Map<string, ColumnAggregate>;
}
```

**Step 3: Update QueryBuilderSnapshot to include selectAggregates**

Modify `QueryBuilderSnapshot` interface (around line 170) to add:

```typescript
export interface QueryBuilderSnapshot {
	/** Tables placed on the canvas */
	tables: CanvasTable[];
	/** Joins between tables */
	joins: CanvasJoin[];
	/** WHERE clause conditions */
	filters: FilterCondition[];
	/** GROUP BY columns */
	groupBy: GroupByCondition[];
	/** HAVING clause conditions */
	having: HavingCondition[];
	/** ORDER BY clauses */
	orderBy: SortCondition[];
	/** LIMIT value, or null for no limit */
	limit: number | null;
	/** Standalone aggregates in SELECT clause */
	selectAggregates: SelectAggregate[];
}
```

**Step 4: Export new types from index.ts**

Add to `src/lib/types/index.ts`:

```typescript
export type { SelectAggregate, ColumnAggregate } from './query-builder';
```

**Step 5: Run type check**

Run: `npm run check`
Expected: PASS (may have errors in other files that use CanvasTable - that's expected, we'll fix in next tasks)

**Step 6: Commit**

```bash
git add src/lib/types/query-builder.ts src/lib/types/index.ts
git commit -m "feat(types): add SelectAggregate and ColumnAggregate types"
```

---

## Task 2: Update QueryBuilderState for Aggregates

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Add selectAggregates state property**

Add after line 52 (after `customSql` state):

```typescript
/** Standalone aggregates in SELECT clause */
selectAggregates = $state<SelectAggregate[]>([]);
```

Add import at top:

```typescript
import type {
	// ... existing imports ...
	SelectAggregate,
	ColumnAggregate
} from '$lib/types';
```

**Step 2: Update snapshot getter**

Modify the `snapshot` getter (around line 70) to include selectAggregates:

```typescript
get snapshot(): QueryBuilderSnapshot {
	return {
		tables: this.tables.map((t) => ({
			...t,
			selectedColumns: new Set(t.selectedColumns),
			columnAggregates: new Map(t.columnAggregates)
		})),
		joins: [...this.joins],
		filters: [...this.filters],
		groupBy: [...this.groupBy],
		having: [...this.having],
		orderBy: [...this.orderBy],
		limit: this.limit,
		selectAggregates: [...this.selectAggregates]
	};
}
```

**Step 3: Update addTable to initialize columnAggregates**

Modify `addTable` method (around line 93) to include columnAggregates:

```typescript
const canvasTable: CanvasTable = {
	id: generateId(),
	tableName,
	position,
	selectedColumns: new SvelteSet<string>(),
	columnAggregates: new Map<string, ColumnAggregate>()
};
```

Add import for SvelteMap if needed (already using SvelteSet).

**Step 4: Update toggleColumn to clear aggregate when deselected**

Modify `toggleColumn` method (around line 152):

```typescript
toggleColumn(tableId: string, columnName: string): void {
	const table = this.tables.find((t) => t.id === tableId);
	if (!table) return;

	if (table.selectedColumns.has(columnName)) {
		table.selectedColumns.delete(columnName);
		// Clear any aggregate on this column when deselected
		table.columnAggregates.delete(columnName);
	} else {
		table.selectedColumns.add(columnName);
	}
	// Trigger reactivity by reassigning the tables array
	this.tables = [...this.tables];
	this.customSql = null;
}
```

**Step 5: Add per-column aggregate methods**

Add after the ORDER BY management section (after line 449):

```typescript
// === COLUMN AGGREGATE MANAGEMENT ===

/**
 * Set an aggregate function on a selected column.
 * @param tableId - ID of the canvas table
 * @param column - Column name
 * @param func - Aggregate function, or null to clear
 * @param alias - Optional alias for AS clause
 */
setColumnAggregate(
	tableId: string,
	column: string,
	func: AggregateFunction | null,
	alias?: string
): void {
	const table = this.tables.find((t) => t.id === tableId);
	if (!table) return;

	if (func === null) {
		table.columnAggregates.delete(column);
	} else {
		table.columnAggregates.set(column, { function: func, alias });
	}
	// Trigger reactivity
	this.tables = [...this.tables];
	this.customSql = null;
}

/**
 * Clear aggregate from a column.
 * @param tableId - ID of the canvas table
 * @param column - Column name
 */
clearColumnAggregate(tableId: string, column: string): void {
	this.setColumnAggregate(tableId, column, null);
}
```

**Step 6: Add standalone aggregate methods**

Add after the column aggregate methods:

```typescript
// === SELECT AGGREGATE MANAGEMENT ===

/**
 * Add a standalone aggregate to the SELECT clause.
 * @param func - Aggregate function
 * @param expression - Expression inside the aggregate (*, column, or expression)
 * @param alias - Optional alias for AS clause
 * @returns The created aggregate's ID
 */
addSelectAggregate(
	func: AggregateFunction,
	expression: string,
	alias?: string
): string {
	const aggregate: SelectAggregate = {
		id: generateId(),
		function: func,
		expression,
		alias
	};

	this.selectAggregates = [...this.selectAggregates, aggregate];
	this.customSql = null;
	return aggregate.id;
}

/**
 * Update a standalone aggregate.
 * @param id - ID of the aggregate
 * @param updates - Partial updates
 */
updateSelectAggregate(id: string, updates: Partial<Omit<SelectAggregate, 'id'>>): void {
	this.selectAggregates = this.selectAggregates.map((a) =>
		a.id === id ? { ...a, ...updates } : a
	);
	this.customSql = null;
}

/**
 * Remove a standalone aggregate.
 * @param id - ID of the aggregate to remove
 */
removeSelectAggregate(id: string): void {
	this.selectAggregates = this.selectAggregates.filter((a) => a.id !== id);
	this.customSql = null;
}
```

**Step 7: Update reset method**

Modify `reset` method (around line 696) to clear aggregates:

```typescript
reset(): void {
	this.tables = [];
	this.joins = [];
	this.filters = [];
	this.groupBy = [];
	this.having = [];
	this.orderBy = [];
	this.limit = 100;
	this.customSql = null;
	this.selectAggregates = [];
}
```

**Step 8: Run type check**

Run: `npm run check`
Expected: May still have errors in serialization - we'll fix next

**Step 9: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(state): add aggregate state management methods"
```

---

## Task 3: Update SQL Generation for Aggregates

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts` (buildSql method)

**Step 1: Update buildSql to include aggregates in SELECT**

Replace the column collection logic in `buildSql` (around line 473-483):

```typescript
private buildSql(): string {
	// No tables = empty query
	if (this.tables.length === 0) {
		return '';
	}

	// Collect all selected columns and aggregates
	const selectParts: string[] = [];

	for (const table of this.tables) {
		for (const column of table.selectedColumns) {
			const agg = table.columnAggregates.get(column);
			if (agg) {
				// Column with aggregate
				const expr = `${agg.function}(${table.tableName}.${column})`;
				selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
			} else {
				// Regular column
				selectParts.push(`${table.tableName}.${column}`);
			}
		}
	}

	// Add standalone aggregates
	for (const agg of this.selectAggregates) {
		const expr = `${agg.function}(${agg.expression})`;
		selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
	}

	// If no columns selected, use * from first table
	const selectClause =
		selectParts.length > 0 ? selectParts.join(', ') : `${this.tables[0].tableName}.*`;

	// ... rest of the method stays the same ...
```

**Step 2: Run type check**

Run: `npm run check`
Expected: PASS

**Step 3: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(sql): generate SELECT with aggregate functions"
```

---

## Task 4: Update Serialization for Aggregates

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts` (serialization methods)

**Step 1: Update SerializableQueryBuilderState interface**

Modify the interface (around line 754):

```typescript
export interface SerializableQueryBuilderState {
	tables: Array<{
		id: string;
		tableName: string;
		position: { x: number; y: number };
		selectedColumns: string[];
		columnAggregates?: Array<[string, { function: AggregateFunction; alias?: string }]>;
	}>;
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy?: GroupByCondition[];
	having?: HavingCondition[];
	orderBy: SortCondition[];
	limit: number | null;
	customSql?: string | null;
	selectAggregates?: SelectAggregate[];
}
```

**Step 2: Update toSerializable method**

Modify `toSerializable` (around line 713):

```typescript
toSerializable(): SerializableQueryBuilderState {
	return {
		tables: this.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: Array.from(t.selectedColumns),
			columnAggregates: Array.from(t.columnAggregates.entries())
		})),
		joins: [...this.joins],
		filters: [...this.filters],
		groupBy: [...this.groupBy],
		having: [...this.having],
		orderBy: [...this.orderBy],
		limit: this.limit,
		customSql: this.customSql,
		selectAggregates: [...this.selectAggregates]
	};
}
```

**Step 3: Update fromSerializable method**

Modify `fromSerializable` (around line 734):

```typescript
fromSerializable(state: SerializableQueryBuilderState): void {
	this.tables = state.tables.map((t) => ({
		id: t.id,
		tableName: t.tableName,
		position: t.position,
		selectedColumns: new SvelteSet(t.selectedColumns),
		columnAggregates: new Map(t.columnAggregates ?? [])
	}));
	this.joins = state.joins.map((j) => ({ ...j }));
	this.filters = state.filters.map((f) => ({ ...f }));
	this.groupBy = (state.groupBy ?? []).map((g) => ({ ...g }));
	this.having = (state.having ?? []).map((h) => ({ ...h }));
	this.orderBy = state.orderBy.map((o) => ({ ...o }));
	this.limit = state.limit;
	this.customSql = state.customSql ?? null;
	this.selectAggregates = (state.selectAggregates ?? []).map((a) => ({ ...a }));
}
```

**Step 4: Run type check**

Run: `npm run check`
Expected: PASS

**Step 5: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(state): add aggregate serialization support"
```

---

## Task 5: Add Aggregates Panel to Filter Panel

**Files:**
- Modify: `src/lib/components/query-builder/filter-panel.svelte`

**Step 1: Add state for Aggregates section**

Add after line 26 (after `limitOpen`):

```typescript
let aggregatesOpen = $state(true);
```

**Step 2: Add handler for adding aggregates**

Add after line 95 (after `handleAddOrderBy`):

```typescript
// Add new select aggregate
function handleAddSelectAggregate() {
	qb.addSelectAggregate('COUNT', '*', '');
}
```

**Step 3: Add Aggregates section UI**

Add after the GROUP BY section (after line 307, before HAVING section):

```svelte
<!-- AGGREGATES Section -->
<Collapsible bind:open={aggregatesOpen}>
	<div class="border-b border-border">
		<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
			<div class="flex items-center gap-2">
				<ChevronDownIcon
					class="size-4 text-muted-foreground transition-transform duration-200 {aggregatesOpen ? '' : '-rotate-90'}"
				/>
				<CalculatorIcon class="size-4 text-muted-foreground" />
				<span class="font-medium text-sm">AGGREGATES</span>
				{#if qb.selectAggregates.length > 0}
					<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
						{qb.selectAggregates.length}
					</span>
				{/if}
			</div>
			<Button
				variant="ghost"
				size="sm"
				class="h-6 px-2 text-xs gap-1"
				onclick={(e: MouseEvent) => {
					e.stopPropagation();
					handleAddSelectAggregate();
				}}
			>
				<PlusIcon class="size-3" />
				Add
			</Button>
		</CollapsibleTrigger>

		<CollapsibleContent>
			<div class="px-3 pb-3 space-y-2">
				{#each qb.selectAggregates as agg (agg.id)}
					<div class="flex items-center gap-2 flex-wrap">
						<!-- Aggregate function select -->
						<Select.Root
							type="single"
							value={agg.function}
							onValueChange={(value) => {
								if (value) {
									qb.updateSelectAggregate(agg.id, { function: value as AggregateFunction });
								}
							}}
						>
							<Select.Trigger size="sm" class="w-20 h-7 text-xs">
								{agg.function}
							</Select.Trigger>
							<Select.Content>
								{#each AGGREGATE_FUNCTIONS as fn}
									<Select.Item value={fn.value} label={fn.label}>
										{fn.label}
									</Select.Item>
								{/each}
							</Select.Content>
						</Select.Root>

						<!-- Expression input with parentheses -->
						<span class="text-xs text-muted-foreground">(</span>
						<Input
							type="text"
							placeholder="*"
							value={agg.expression}
							oninput={(e: Event) => {
								const target = e.target as HTMLInputElement;
								qb.updateSelectAggregate(agg.id, { expression: target.value });
							}}
							class="h-7 text-xs w-28 font-mono"
						/>
						<span class="text-xs text-muted-foreground">)</span>

						<!-- Alias input -->
						<span class="text-xs text-muted-foreground">AS</span>
						<Input
							type="text"
							placeholder="alias"
							value={agg.alias ?? ''}
							oninput={(e: Event) => {
								const target = e.target as HTMLInputElement;
								qb.updateSelectAggregate(agg.id, { alias: target.value || undefined });
							}}
							class="h-7 text-xs w-20 font-mono"
						/>

						<!-- Remove button -->
						<Button
							variant="ghost"
							size="icon-sm"
							class="size-7 text-muted-foreground hover:text-destructive"
							onclick={() => qb.removeSelectAggregate(agg.id)}
						>
							<XIcon class="size-3" />
						</Button>
					</div>
				{/each}

				{#if qb.selectAggregates.length === 0}
					<p class="text-xs text-muted-foreground">No aggregates. Click "Add" to add COUNT(*), SUM, etc.</p>
				{/if}
			</div>
		</CollapsibleContent>
	</div>
</Collapsible>
```

**Step 4: Run dev server and test**

Run: `npm run tauri dev`
Expected: New "AGGREGATES" section appears in filter panel, can add/edit/remove aggregates

**Step 5: Commit**

```bash
git add src/lib/components/query-builder/filter-panel.svelte
git commit -m "feat(ui): add Aggregates panel to filter panel"
```

---

## Task 6: Add Per-Column Aggregate UI to Table Node

**Files:**
- Modify: `src/lib/components/query-builder/table-node.svelte`

**Step 1: Add import for Select component and AggregateFunction type**

Add to imports (around line 8):

```typescript
import * as Select from '$lib/components/ui/select';
import type { AggregateFunction } from '$lib/types';
```

**Step 2: Add aggregate functions constant**

Add after the props definition (around line 24):

```typescript
const AGGREGATE_FUNCTIONS: { value: AggregateFunction | 'none'; label: string }[] = [
	{ value: 'none', label: 'None' },
	{ value: 'COUNT', label: 'COUNT' },
	{ value: 'SUM', label: 'SUM' },
	{ value: 'AVG', label: 'AVG' },
	{ value: 'MIN', label: 'MIN' },
	{ value: 'MAX', label: 'MAX' }
];
```

**Step 3: Add handler for changing column aggregate**

Add after the existing handlers (around line 39):

```typescript
function handleAggregateChange(columnName: string, value: string) {
	if (value === 'none') {
		queryBuilder.clearColumnAggregate(data.tableId, columnName);
	} else {
		queryBuilder.setColumnAggregate(data.tableId, columnName, value as AggregateFunction);
	}
}
```

**Step 4: Update Props interface to include columnAggregates**

Update the `data` prop type (around line 14):

```typescript
interface Props {
	id: string;
	data: {
		tableName: string;
		tableId: string;
		selectedColumns: Set<string>;
		columnAggregates: Map<string, { function: AggregateFunction; alias?: string }>;
	};
	isConnectable?: boolean;
}
```

**Step 5: Add aggregate dropdown to selected columns**

Modify the column row template (around line 79). After the checkbox and before the FK icon, add the aggregate dropdown for selected columns:

```svelte
{#each tableSchema.columns as column (column.name)}
	{@const isSelected = data.selectedColumns.has(column.name)}
	{@const isForeignKey = Boolean(column.foreignKey)}
	{@const columnAgg = data.columnAggregates.get(column.name)}
	<div class="px-3 py-1.5 flex items-center gap-2 hover:bg-muted/30 relative">
		<!-- Left Handle (target) for receiving connections -->
		<Handle
			type="target"
			position={Position.Left}
			id="{column.name}-target"
			{isConnectable}
		/>

		<!-- Checkbox -->
		<Checkbox
			checked={isSelected}
			onCheckedChange={() => handleToggleColumn(column.name)}
		/>

		<!-- Aggregate dropdown (only for selected columns) -->
		{#if isSelected}
			<Select.Root
				type="single"
				value={columnAgg?.function ?? 'none'}
				onValueChange={(value) => {
					if (value) handleAggregateChange(column.name, value);
				}}
			>
				<Select.Trigger size="sm" class="h-5 w-14 text-[10px] px-1">
					{columnAgg?.function ?? 'col'}
				</Select.Trigger>
				<Select.Content>
					{#each AGGREGATE_FUNCTIONS as fn}
						<Select.Item value={fn.value} label={fn.label}>
							{fn.label}
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		{/if}

		<!-- FK Icon or spacer -->
		<div class="w-4 flex items-center justify-center shrink-0">
			{#if isForeignKey}
				<span title="FK -> {column.foreignKey?.table}.{column.foreignKey?.column}">
					<LinkIcon class="size-3 text-blue-500" />
				</span>
			{/if}
		</div>

		<!-- Column name -->
		<span class="flex-1 truncate font-mono" class:text-primary={columnAgg} title={column.name}>
			{column.name}
		</span>

		<!-- Column type -->
		<span class="text-muted-foreground shrink-0 font-mono">
			{column.type}
		</span>

		<!-- Right Handle (source) for creating connections -->
		<Handle
			type="source"
			position={Position.Right}
			id="{column.name}-source"
			{isConnectable}
		/>
	</div>
{/each}
```

**Step 6: Run dev server and test**

Run: `npm run tauri dev`
Expected: Selected columns show aggregate dropdown, selecting SUM/AVG/etc updates the SQL

**Step 7: Commit**

```bash
git add src/lib/components/query-builder/table-node.svelte
git commit -m "feat(ui): add per-column aggregate dropdown to table node"
```

---

## Task 7: Update Canvas to Pass columnAggregates to Table Node

**Files:**
- Modify: `src/lib/components/query-builder/canvas.svelte`

**Step 1: Read the canvas file to understand current node data structure**

First read the file to find where nodes are created.

**Step 2: Update node data to include columnAggregates**

Find where `TableNode` data is created and add `columnAggregates`:

```typescript
data: {
	tableName: table.tableName,
	tableId: table.id,
	selectedColumns: table.selectedColumns,
	columnAggregates: table.columnAggregates
}
```

**Step 3: Run dev server and test end-to-end**

Run: `npm run tauri dev`
Expected: Per-column aggregates work, SQL updates correctly

**Step 4: Commit**

```bash
git add src/lib/components/query-builder/canvas.svelte
git commit -m "feat(canvas): pass columnAggregates to table nodes"
```

---

## Task 8: Update SQL Parser for SELECT Aggregates

**Files:**
- Modify: `src/lib/tutorial/sql-parser.ts`

**Step 1: Add ParsedSelectAggregate interface**

Add after `ParsedHaving` interface (around line 44):

```typescript
export interface ParsedSelectAggregate {
	function: AggregateFunction;
	expression: string;
	alias?: string;
}

export interface ParsedColumnAggregate {
	tableName: string;
	column: string;
	function: AggregateFunction;
	alias?: string;
}
```

**Step 2: Update ParsedQuery to include aggregates**

Modify `ParsedQuery` interface:

```typescript
export interface ParsedQuery {
	tables: ParsedTable[];
	joins: ParsedJoin[];
	filters: ParsedFilter[];
	groupBy: ParsedGroupBy[];
	having: ParsedHaving[];
	orderBy: ParsedOrderBy[];
	limit: number | null;
	selectAggregates: ParsedSelectAggregate[];
	columnAggregates: ParsedColumnAggregate[];
}
```

**Step 3: Update parseSql to detect aggregates in SELECT columns**

Modify the SELECT columns parsing section (around line 131):

```typescript
// Parse SELECT columns
const selectAggregates: ParsedSelectAggregate[] = [];
const columnAggregates: ParsedColumnAggregate[] = [];

if (stmt.columns && Array.isArray(stmt.columns)) {
	for (const col of stmt.columns) {
		if (col === '*') {
			// SELECT * - select all columns from all tables
			for (const table of tables) {
				const tableSchema = getTable(table.tableName);
				if (tableSchema) {
					table.selectedColumns = tableSchema.columns.map((c) => c.name);
				}
			}
		} else if (col.expr) {
			const colExpr = col.expr;

			// Check if it's an aggregate function
			if (colExpr.type === 'aggr_func' && colExpr.name) {
				const funcName = colExpr.name.toUpperCase();
				if (isAggregateFunction(funcName)) {
					const alias = col.as || undefined;
					const args = colExpr.args as {
						expr?: {
							type?: string;
							table?: string;
							column?: string | { expr?: { value?: string } };
							value?: string;
						};
					};

					if (args?.expr) {
						if (args.expr.type === 'star') {
							// COUNT(*) or similar - standalone aggregate
							selectAggregates.push({
								function: funcName as AggregateFunction,
								expression: '*',
								alias
							});
						} else if (args.expr.type === 'column_ref') {
							// Aggregate on a specific column
							const columnName =
								typeof args.expr.column === 'string'
									? args.expr.column
									: args.expr.column?.expr?.value;

							if (columnName) {
								const tableName = args.expr.table
									? resolveTableName(args.expr.table, tableAliasMap)
									: findTableForColumn(tables, columnName)?.tableName;

								if (tableName) {
									// Per-column aggregate
									columnAggregates.push({
										tableName,
										column: columnName,
										function: funcName as AggregateFunction,
										alias
									});
									// Also select the column
									const table = tables.find((t) => t.tableName === tableName);
									if (table && !table.selectedColumns.includes(columnName)) {
										table.selectedColumns.push(columnName);
									}
								}
							}
						} else {
							// Expression aggregate (e.g., SUM(price * quantity))
							// Store as standalone with the expression text
							selectAggregates.push({
								function: funcName as AggregateFunction,
								expression: extractExpressionText(args.expr),
								alias
							});
						}
					}
				}
			} else if (colExpr.type === 'star' && colExpr.table) {
				// ... existing table.* handling ...
			} else if (colExpr.type === 'column_ref' && colExpr.column === '*') {
				// ... existing * handling ...
			} else if (colExpr.type === 'column_ref') {
				// ... existing column_ref handling ...
			}
		}
	}
}
```

**Step 4: Add helper function for extracting expression text**

Add helper function:

```typescript
function extractExpressionText(expr: unknown): string {
	// Simplified - for complex expressions, return a placeholder
	// Real implementation would reconstruct the expression
	const e = expr as { type?: string; value?: unknown; column?: string };
	if (e.type === 'number' || e.type === 'string') {
		return String(e.value);
	}
	if (e.type === 'column_ref' && e.column) {
		return typeof e.column === 'string' ? e.column : '*';
	}
	return '*';
}
```

**Step 5: Update return statement**

Update the return to include new fields:

```typescript
return { tables, joins, filters, groupBy, having, orderBy, limit, selectAggregates, columnAggregates };
```

**Step 6: Update empty return**

Update the empty return (around line 62):

```typescript
return { tables: [], joins: [], filters: [], groupBy: [], having: [], orderBy: [], limit: null, selectAggregates: [], columnAggregates: [] };
```

**Step 7: Run type check**

Run: `npm run check`
Expected: PASS

**Step 8: Commit**

```bash
git add src/lib/tutorial/sql-parser.ts
git commit -m "feat(parser): parse SELECT aggregate functions"
```

---

## Task 9: Update applyFromParsedSql to Handle Aggregates

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Update applyFromParsedSql signature**

Update the method parameter type (around line 594):

```typescript
applyFromParsedSql(parsed: {
	tables: Array<{ tableName: string; selectedColumns: string[] }>;
	joins: Array<{
		sourceTable: string;
		sourceColumn: string;
		targetTable: string;
		targetColumn: string;
		joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';
	}>;
	filters: Array<{
		column: string;
		operator: FilterOperator;
		value: string;
		connector: 'AND' | 'OR';
	}>;
	groupBy: Array<{ column: string }>;
	having: Array<{
		aggregateFunction: AggregateFunction;
		column: string;
		operator: HavingOperator;
		value: string;
		connector: 'AND' | 'OR';
	}>;
	orderBy: Array<{ column: string; direction: 'ASC' | 'DESC' }>;
	limit: number | null;
	selectAggregates: Array<{
		function: AggregateFunction;
		expression: string;
		alias?: string;
	}>;
	columnAggregates: Array<{
		tableName: string;
		column: string;
		function: AggregateFunction;
		alias?: string;
	}>;
}): void {
```

**Step 2: Build columnAggregates map for each table**

Update the table building logic (around line 621):

```typescript
for (let i = 0; i < parsed.tables.length; i++) {
	const pt = parsed.tables[i];
	const existing = existingTableMap.get(pt.tableName);

	const position = existing?.position ?? { x: 50 + i * 280, y: 50 + (i % 2) * 150 };

	// Build columnAggregates map for this table
	const columnAggsForTable = new Map<string, ColumnAggregate>();
	for (const ca of parsed.columnAggregates) {
		if (ca.tableName === pt.tableName) {
			columnAggsForTable.set(ca.column, {
				function: ca.function,
				alias: ca.alias
			});
		}
	}

	newTables.push({
		id: existing?.id ?? generateId(),
		tableName: pt.tableName,
		position,
		selectedColumns: new SvelteSet(pt.selectedColumns),
		columnAggregates: columnAggsForTable
	});
}
```

**Step 3: Build selectAggregates**

Add after building tables:

```typescript
// Build new select aggregates
const newSelectAggregates: SelectAggregate[] = parsed.selectAggregates.map((pa) => ({
	id: generateId(),
	function: pa.function,
	expression: pa.expression,
	alias: pa.alias
}));
```

**Step 4: Apply selectAggregates**

Update the apply section to include selectAggregates:

```typescript
// Apply all at once
this.tables = newTables;
this.joins = newJoins;
this.filters = newFilters;
this.groupBy = newGroupBy;
this.having = newHaving;
this.orderBy = newOrderBy;
this.limit = parsed.limit;
this.selectAggregates = newSelectAggregates;
```

**Step 5: Run type check**

Run: `npm run check`
Expected: PASS

**Step 6: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(state): apply parsed aggregates from SQL"
```

---

## Task 10: Integration Testing

**Files:**
- Manual testing

**Step 1: Start dev server**

Run: `npm run tauri dev`

**Step 2: Test per-column aggregates**

1. Add a table to canvas (e.g., products)
2. Select the "price" column
3. Click the aggregate dropdown, select "SUM"
4. Verify SQL shows: `SELECT SUM(products.price) FROM products`

**Step 3: Test standalone aggregates**

1. Open the AGGREGATES panel
2. Click "Add"
3. Verify COUNT(*) appears
4. Change expression to "products.id", function to "COUNT"
5. Add alias "total"
6. Verify SQL shows: `SELECT COUNT(products.id) AS total FROM products`

**Step 4: Test two-way sync**

1. In SQL editor, type: `SELECT COUNT(*), SUM(price) AS total FROM products`
2. Verify the visual state updates:
   - COUNT(*) appears in Aggregates panel
   - SUM(price) appears either as column aggregate or standalone

**Step 5: Test combined usage**

1. Add products table
2. Select name column (no aggregate)
3. Select price column with SUM aggregate
4. Add COUNT(*) via Aggregates panel
5. Add GROUP BY on name
6. Verify SQL: `SELECT products.name, SUM(products.price), COUNT(*) FROM products GROUP BY products.name`

**Step 6: Final commit**

```bash
git add -A
git commit -m "feat: complete aggregate functions in query builder

Adds visual support for SQL aggregate functions (SUM, AVG, COUNT, MIN, MAX):
- Per-column aggregates via dropdown on selected columns
- Dedicated Aggregates panel for COUNT(*) and expressions
- Two-way sync with SQL editor
- Full serialization support"
```

---

## Summary

This plan implements aggregate functions in 10 tasks:

1. **Types** - Add SelectAggregate, ColumnAggregate types
2. **State** - Add state properties and management methods
3. **SQL Gen** - Update buildSql for aggregates
4. **Serialization** - Add aggregate serialization
5. **Aggregates Panel** - New UI section in filter panel
6. **Table Node UI** - Per-column aggregate dropdown
7. **Canvas** - Pass columnAggregates to nodes
8. **Parser** - Parse SELECT aggregates
9. **Apply Parsed** - Apply parsed aggregates to state
10. **Testing** - Integration testing
