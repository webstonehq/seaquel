# Visual Query Builder Variable Support

## Summary

Add `{{variable}}` template support to the visual query builder. Users type variables directly into value fields, and the existing parameter prompt appears at execution time.

## Scope

Variables supported in:
- WHERE filter values
- HAVING filter values
- LIMIT field

## Implementation

### 1. SQL Generation (`src/lib/hooks/query-builder.svelte.ts`)

Modify `buildFilterCondition()` and `buildHavingCondition()` to detect variable pattern and pass through unquoted:

```typescript
const isVariable = /^\{\{.+\}\}$/.test(value.trim());
const isNumeric = !isNaN(Number(value)) && value.trim() !== '';
const formattedValue = isVariable ? value : (isNumeric ? value : `'${value}'`);
```

Modify LIMIT handling:

```typescript
if (limit !== null) {
  const limitStr = String(limit);
  const isVariable = /^\{\{.+\}\}$/.test(limitStr.trim());
  parts.push(`LIMIT ${isVariable ? limitStr : limit}`);
}
```

### 2. Type Change (`src/lib/types/query-builder.ts`)

Change LIMIT type to accept variables:

```typescript
// Before
limit: number | null

// After
limit: string | number | null
```

### 3. Filter Panel (`src/lib/components/query-builder/filter-panel.svelte`)

- Change LIMIT input from `type="number"` to `type="text"`
- Add validation: accept positive integers or `{{...}}` pattern
- Optional: visual indicator when value contains a variable

### 4. SQL Parser (`src/lib/tutorial/sql-parser.ts`)

Update value extraction to recognize unquoted `{{variable}}` tokens in WHERE/HAVING conditions and preserve them as literal strings.

## Files to Modify

| File | Change |
|------|--------|
| `src/lib/hooks/query-builder.svelte.ts` | Variable detection in SQL generation |
| `src/lib/types/query-builder.ts` | LIMIT type change |
| `src/lib/components/query-builder/filter-panel.svelte` | LIMIT input, optional visual hint |
| `src/lib/tutorial/sql-parser.ts` | Parse `{{variable}}` tokens |

## Runtime Behavior

1. User builds query visually with `{{customer_id}}` in a WHERE filter
2. Generated SQL contains: `WHERE customers.id = {{customer_id}}`
3. User clicks Run
4. Existing `extractParameters()` finds `{{customer_id}}`
5. Parameter prompt dialog appears asking for value
6. `substituteParameters()` replaces variable with provided value
7. Query executes
