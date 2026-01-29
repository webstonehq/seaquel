# Query Builder Variable Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `{{variable}}` template support to the visual query builder's value fields.

**Architecture:** Variables are detected by regex pattern during SQL generation and passed through unquoted. The LIMIT type is widened to accept strings. The SQL parser recognizes variable tokens during round-trip parsing.

**Tech Stack:** TypeScript, Svelte 5, node-sql-parser

---

## Task 1: Update Types for LIMIT Field

**Files:**
- Modify: `src/lib/types/query-builder.ts:246` (SubqueryInnerState.limit)
- Modify: `src/lib/types/query-builder.ts:309` (QueryBuilderSnapshot.limit)

**Step 1: Update SubqueryInnerState.limit type**

In `SubqueryInnerState` interface, change:

```typescript
// Before (line 246)
limit: number | null;

// After
limit: string | number | null;
```

**Step 2: Update QueryBuilderSnapshot.limit type**

In `QueryBuilderSnapshot` interface, change:

```typescript
// Before (line 309)
limit: number | null;

// After
limit: string | number | null;
```

**Step 3: Commit**

```bash
git add src/lib/types/query-builder.ts
git commit -m "feat(query-builder): widen limit type to accept variable strings"
```

---

## Task 2: Add Variable Detection Helper

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts`

**Step 1: Add isVariable helper function**

Add this helper near the top of the file (after imports, around line 20):

```typescript
/**
 * Check if a value is a template variable like {{my_var}}.
 */
function isTemplateVariable(value: string): boolean {
	return /^\{\{.+\}\}$/.test(value.trim());
}
```

**Step 2: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(query-builder): add isTemplateVariable helper"
```

---

## Task 3: Update buildFilterCondition for Variables

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts:2156-2186` (buildFilterCondition method)

**Step 1: Update LIKE/NOT LIKE case to detect variables**

Change the LIKE/NOT LIKE case (around line 2178-2180):

```typescript
// Before
case 'LIKE':
case 'NOT LIKE':
	return `${column} ${operator} '${value}'`;

// After
case 'LIKE':
case 'NOT LIKE':
	if (isTemplateVariable(value)) {
		return `${column} ${operator} ${value}`;
	}
	return `${column} ${operator} '${value}'`;
```

**Step 2: Update default case to detect variables**

Change the default case (around line 2181-2185):

```typescript
// Before
default:
	// Check if value looks like a number
	const isNumeric = !isNaN(Number(value)) && value.trim() !== '';
	const formattedValue = isNumeric ? value : `'${value}'`;
	return `${column} ${operator} ${formattedValue}`;

// After
default: {
	// Check if value is a template variable
	if (isTemplateVariable(value)) {
		return `${column} ${operator} ${value}`;
	}
	// Check if value looks like a number
	const isNumeric = !isNaN(Number(value)) && value.trim() !== '';
	const formattedValue = isNumeric ? value : `'${value}'`;
	return `${column} ${operator} ${formattedValue}`;
}
```

**Step 3: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(query-builder): support {{variables}} in WHERE filter values"
```

---

## Task 4: Update buildHavingCondition for Variables

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts:2192-2197` (buildHavingCondition method)

**Step 1: Update buildHavingCondition to handle variables in value**

Change the method:

```typescript
// Before
private buildHavingCondition(having: HavingCondition): string {
	const { aggregateFunction, column, operator, value } = having;
	// Use * for empty column (COUNT(*)), otherwise use the column name
	const columnPart = column === '' ? '*' : column;
	return `${aggregateFunction}(${columnPart}) ${operator} ${value}`;
}

// After
private buildHavingCondition(having: HavingCondition): string {
	const { aggregateFunction, column, operator, value } = having;
	// Use * for empty column (COUNT(*)), otherwise use the column name
	const columnPart = column === '' ? '*' : column;
	// Template variables and numbers pass through as-is, strings would need quoting
	// but HAVING values are typically numeric, so we don't quote them
	return `${aggregateFunction}(${columnPart}) ${operator} ${value}`;
}
```

Note: HAVING values are already passed through as-is (no quoting), so variables work automatically. The comment clarifies intent.

**Step 2: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "docs(query-builder): clarify HAVING value handling for variables"
```

---

## Task 5: Update LIMIT SQL Generation

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts:2104-2108` (LIMIT clause in buildQuerySql)

**Step 1: Update LIMIT clause generation**

Change the LIMIT clause generation (around line 2104-2108):

```typescript
// Before
// Build LIMIT clause
let limitClause = '';
if (limit !== null) {
	limitClause = `\nLIMIT ${limit}`;
}

// After
// Build LIMIT clause
let limitClause = '';
if (limit !== null) {
	limitClause = `\nLIMIT ${limit}`;
}
```

Note: This already works because template interpolation handles both strings and numbers. No change needed - the type widening in Task 1 is sufficient.

**Step 2: Commit**

Skip - no code change needed.

---

## Task 6: Update QueryBuilderState limit Property

**Files:**
- Modify: `src/lib/hooks/query-builder.svelte.ts` (limit property declaration and setActiveLimit method)

**Step 1: Find and update the limit property type**

Search for `limit` property in the class. Update its type:

```typescript
// Before
limit: number | null = $state(100);

// After
limit: string | number | null = $state(100);
```

**Step 2: Update setActiveLimit method signature**

Find the `setActiveLimit` method and update its parameter type:

```typescript
// Before
setActiveLimit(limit: number | null): void {

// After
setActiveLimit(limit: string | number | null): void {
```

**Step 3: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/hooks/query-builder.svelte.ts
git commit -m "feat(query-builder): update limit property to accept string variables"
```

---

## Task 7: Update Filter Panel LIMIT Input

**Files:**
- Modify: `src/lib/components/query-builder/filter-panel.svelte:843-850` (LIMIT input)

**Step 1: Change input type and update handler**

Replace the LIMIT input section:

```svelte
<!-- Before -->
<Input
	type="number"
	value={qb.activeLimit ?? ''}
	oninput={handleLimitChange}
	disabled={qb.activeLimit === null}
	min={1}
	class="h-7 text-xs w-20"
/>

<!-- After -->
<Input
	type="text"
	value={qb.activeLimit ?? ''}
	oninput={handleLimitChange}
	disabled={qb.activeLimit === null}
	placeholder="100"
	class="h-7 text-xs w-20 font-mono"
/>
```

**Step 2: Update handleLimitChange function**

Replace the handleLimitChange function (around line 206-212):

```typescript
// Before
function handleLimitChange(event: Event) {
	const target = event.target as HTMLInputElement;
	const value = parseInt(target.value, 10);
	if (!isNaN(value) && value > 0) {
		qb.setActiveLimit(value);
	}
}

// After
function handleLimitChange(event: Event) {
	const target = event.target as HTMLInputElement;
	const value = target.value.trim();

	// Allow template variables like {{limit}}
	if (/^\{\{.+\}\}$/.test(value)) {
		qb.setActiveLimit(value);
		return;
	}

	// Otherwise parse as number
	const numValue = parseInt(value, 10);
	if (!isNaN(numValue) && numValue > 0) {
		qb.setActiveLimit(numValue);
	} else if (value === '') {
		// Allow clearing to reset to default behavior
		qb.setActiveLimit(100);
	}
}
```

**Step 3: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/components/query-builder/filter-panel.svelte
git commit -m "feat(query-builder): allow {{variables}} in LIMIT input"
```

---

## Task 8: Update SQL Parser to Recognize Variables

**Files:**
- Modify: `src/lib/tutorial/sql-parser.ts:916-929` (extractValue function)

**Step 1: Update extractValue to handle variable tokens**

The SQL parser uses node-sql-parser which won't parse `{{variable}}` as valid SQL. When a user types `{{limit}}` in the visual builder and it generates `LIMIT {{limit}}`, parsing will fail.

However, this is acceptable because:
1. Variables are meant for execution-time substitution
2. Round-trip sync (SQL → visual → SQL) won't work with variables in the SQL
3. Users building queries visually with variables don't need to edit the raw SQL

The existing parameter system in `src/lib/db/query-params.ts` handles extraction and substitution at execution time.

**Step 2: Commit**

Skip - no change needed. The parser doesn't need to handle variables since:
- Variables entered in visual builder → generate SQL with variables → execute with parameter prompt
- Editing raw SQL with variables won't round-trip back to visual (by design)

---

## Task 9: Manual Testing

**Step 1: Start dev server**

Run: `npm run tauri dev`

**Step 2: Test WHERE filter with variable**

1. Open query builder (Learn section or Manage with a connection)
2. Add a table to the canvas
3. Add a WHERE filter
4. Enter `{{customer_id}}` as the value
5. Verify generated SQL shows: `WHERE column = {{customer_id}}` (no quotes)

**Step 3: Test HAVING with variable**

1. Add a GROUP BY column
2. Add a HAVING condition
3. Enter `{{min_count}}` as the value
4. Verify generated SQL shows: `HAVING COUNT(*) > {{min_count}}`

**Step 4: Test LIMIT with variable**

1. In the LIMIT section, clear the value
2. Enter `{{max_rows}}`
3. Verify generated SQL shows: `LIMIT {{max_rows}}`

**Step 5: Test execution (requires database connection)**

1. Connect to a database in Manage section
2. Build a query with `{{id}}` in a WHERE filter
3. Click Run
4. Verify parameter prompt appears asking for `id` value
5. Enter a value and verify query executes

---

## Task 10: Final Commit

**Step 1: Verify all changes**

Run: `npm run check`
Expected: No type errors

**Step 2: Create final commit if any uncommitted changes**

```bash
git status
# If changes exist:
git add -A
git commit -m "feat(query-builder): complete variable support implementation"
```
