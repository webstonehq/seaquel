# HAVING Clause Support in Visual Query Builder

## Overview

Add HAVING clause support to the visual query builder for the HAVING lesson. Uses an aggregate-aware UI where users select an aggregate function, column, operator, and value.

## Data Types

Add to `src/lib/types/query-builder.ts`:

```typescript
// Aggregate functions for HAVING clauses
export type AggregateFunction = 'COUNT' | 'SUM' | 'AVG' | 'MIN' | 'MAX';

// Operators valid for HAVING conditions (numeric comparisons only)
export type HavingOperator = '=' | '!=' | '>' | '<' | '>=' | '<=';

// A single HAVING condition
export interface HavingCondition {
  id: string;
  aggregateFunction: AggregateFunction;
  column: string;  // Empty string = * for COUNT(*)
  operator: HavingOperator;
  value: string;
  connector: 'AND' | 'OR';
}
```

Update `QueryBuilderSnapshot` to include `having: HavingCondition[]`.

## State Management

Add to `QueryBuilderState` class in `src/lib/hooks/query-builder.svelte.ts`:

**State:**
```typescript
having = $state<HavingCondition[]>([]);
```

**Methods:**
- `addHaving(aggregateFunction, column, operator, value, connector)` - Add new HAVING condition
- `updateHaving(havingId, updates)` - Update existing condition
- `removeHaving(havingId)` - Remove condition

**SQL Generation:**
Update `buildSql()` to insert HAVING between GROUP BY and ORDER BY:
```sql
SELECT category_id, COUNT(*)
FROM products
GROUP BY category_id
HAVING COUNT(*) > 3
ORDER BY category_id
```

**Serialization:**
- Add `having` to `SerializableQueryBuilderState`
- Update `toSerializable()` and `fromSerializable()`

## UI Component

Add HAVING section to `src/lib/components/query-builder/filter-panel.svelte` after GROUP BY:

```
┌─────────────────────────────────────────────────────┐
│ ▼  📊 HAVING                              [+ Add]   │
├─────────────────────────────────────────────────────┤
│  [COUNT ▼] ( [*________] ) [> ▼] [3____] [×]       │
│  AND                                                │
│  [AVG   ▼] ( [price____] ) [> ▼] [75___] [×]       │
└─────────────────────────────────────────────────────┘
```

Elements per condition:
1. Aggregate function dropdown (COUNT, SUM, AVG, MIN, MAX)
2. Column input (placeholder shows `*` for COUNT)
3. Operator dropdown (=, !=, >, <, >=, <=)
4. Value input
5. AND/OR connector between conditions
6. Remove button

## SQL Parser

Update `src/lib/tutorial/sql-parser.ts` to parse HAVING clauses:

**Pattern:**
```
HAVING (COUNT|SUM|AVG|MIN|MAX)\s*\(\s*(\*|\w+\.?\w*)\s*\)\s*(=|!=|>|<|>=|<=)\s*(\d+)
```

**Output:**
```typescript
having: Array<{
  aggregateFunction: AggregateFunction;
  column: string;
  operator: HavingOperator;
  value: string;
  connector: 'AND' | 'OR';
}>
```

## Implementation Order

1. Add types to `query-builder.ts`
2. Add state and methods to `query-builder.svelte.ts`
3. Update SQL generation in `buildSql()`
4. Add HAVING section to `filter-panel.svelte`
5. Update SQL parser for two-way sync
6. Update `applyFromParsedSql()` to handle HAVING
