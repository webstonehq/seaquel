# Aggregate Functions in Query Builder UI

**Date:** 2026-01-26
**Status:** Approved

## Overview

Add visual support for aggregate functions (SUM, AVG, COUNT, MIN, MAX) in the query builder SELECT clause through two complementary interfaces:

1. **Per-column aggregates** - Wrap selected columns in aggregate functions directly on the table node
2. **Dedicated Aggregates panel** - Add standalone expressions like `COUNT(*)` or `SUM(price * quantity)`

## Data Model

### New Types (`src/lib/types/query-builder.ts`)

```typescript
// Standalone aggregate expression (for COUNT(*), expressions)
interface SelectAggregate {
  id: string;
  function: AggregateFunction;  // COUNT | SUM | AVG | MIN | MAX
  expression: string;           // "*", "price", "price * quantity"
  alias?: string;               // optional AS clause
}
```

### Changes to Existing Types

**CanvasTable** - add per-column aggregate tracking:

```typescript
interface CanvasTable {
  id: string;
  tableName: string;
  position: { x: number; y: number };
  selectedColumns: SvelteSet<string>;
  // NEW: maps column name → aggregate info
  columnAggregates: Map<string, { function: AggregateFunction; alias?: string }>;
}
```

**QueryBuilderState** - add standalone aggregates array:

```typescript
selectAggregates: SelectAggregate[] = $state([]);
```

## UI Design

### Per-Column Aggregates (Table Node)

Location: `table-node.svelte` - each selected column row

Interaction:
- Selected columns show a small aggregate button (Σ icon) next to the checkbox
- Clicking opens a dropdown: `None | COUNT | SUM | AVG | MIN | MAX`
- When aggregate applied:
  - Column label changes: `price` → `SUM(price)`
  - Optional inline alias input appears
  - Visual indicator (highlight/badge) shows it's aggregated

```
┌─ products ──────────────────┐
│ [x] id          INTEGER  PK │
│ [x] price  [Σ▾] DECIMAL     │  ← dropdown shows SUM selected
│     ↳ SUM(price) AS total   │  ← inline alias (optional)
│ [ ] stock       INTEGER     │
│ [ ] name        VARCHAR     │
└─────────────────────────────┘
```

### Dedicated Aggregates Panel (Filter Panel)

Location: New collapsible section in `filter-panel.svelte`, between columns and GROUP BY

Purpose: Handle `COUNT(*)`, expressions, and aggregates not tied to a specific column

```
┌─ Aggregates (2) ─────────────────── [+ Add] ─┐
│                                              │
│  [COUNT ▾] [  *                ] AS [count ] │
│                                         [×]  │
│  [SUM   ▾] [  price * quantity ] AS [total ] │
│                                         [×]  │
└──────────────────────────────────────────────┘
```

Fields per row:
1. Function dropdown: COUNT | SUM | AVG | MIN | MAX
2. Expression input: Free text with column autocomplete
3. Alias input: Optional, generates `AS alias`
4. Remove button

## SQL Generation

Modified `buildSql()` in `query-builder.svelte.ts`:

```typescript
const selectParts: string[] = [];

// 1. Regular columns + per-column aggregates
for (const table of this.tables) {
  for (const col of table.selectedColumns) {
    const agg = table.columnAggregates.get(col);
    if (agg) {
      const expr = `${agg.function}(${table.tableName}.${col})`;
      selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
    } else {
      selectParts.push(`${table.tableName}.${col}`);
    }
  }
}

// 2. Standalone aggregates from panel
for (const agg of this.selectAggregates) {
  const expr = `${agg.function}(${agg.expression})`;
  selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
}

// 3. Fallback if nothing selected
if (selectParts.length === 0) {
  selectParts.push(`${this.tables[0].tableName}.*`);
}
```

## SQL Parser (Two-Way Sync)

Modified `sql-parser.ts` to extract aggregates from SELECT:

```typescript
for (const col of ast.columns) {
  if (col.expr?.type === 'aggr_func') {
    const func = col.expr.name;        // "SUM"
    const arg = col.expr.args?.expr;   // column or expression
    const alias = col.as;              // optional alias

    if (isSimpleColumnRef(arg)) {
      // Per-column aggregate: add to table.columnAggregates
    } else {
      // Standalone aggregate (expression or *): add to selectAggregates
    }
  } else {
    // Regular column selection
  }
}
```

Mapping rules:
- `SUM(products.price) AS total` → per-column aggregate on products.price
- `COUNT(*)` → standalone aggregate
- `SUM(price * quantity)` → standalone aggregate (expression)
- `COUNT(DISTINCT name)` → standalone (DISTINCT not supported in per-column UI)

## State Management Methods

New methods in `QueryBuilderState`:

```typescript
// Per-column aggregates
setColumnAggregate(tableId: string, column: string, func: AggregateFunction | null, alias?: string)
clearColumnAggregate(tableId: string, column: string)

// Standalone aggregates
addSelectAggregate(func: AggregateFunction, expression: string, alias?: string): string
updateSelectAggregate(id: string, updates: Partial<SelectAggregate>)
removeSelectAggregate(id: string)
```

Initialization changes:
- `addTable()` initializes `columnAggregates: new Map()`
- `toggleColumn()` clears aggregate when column is deselected
- `reset()` clears `selectAggregates` array

## Files to Modify

1. `src/lib/types/query-builder.ts` - Add SelectAggregate type, extend CanvasTable
2. `src/lib/hooks/query-builder.svelte.ts` - State, methods, SQL generation
3. `src/lib/components/query-builder/table-node.svelte` - Per-column aggregate UI
4. `src/lib/components/query-builder/filter-panel.svelte` - Aggregates panel section
5. `src/lib/tutorial/sql-parser.ts` - Parse SELECT aggregates
