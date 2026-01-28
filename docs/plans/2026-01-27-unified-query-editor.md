# Unified Query Editor Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Consolidate query execution by having the query builder workspace embed the main query-editor component with an executor prop.

**Architecture:** Add a `QueryExecutor` interface. Query-editor accepts an optional `executor` prop - when provided, it uses that for execution and hides Manage-only features. The workspace passes a tutorial executor.

**Tech Stack:** TypeScript, Svelte 5, existing query-params utilities

---

## Task 1: Add QueryExecutor Type

**Files:**
- Modify: `src/lib/types/query.ts`

**Step 1: Add the QueryExecutor interface**

Add at the end of the file (before the closing, after AIMessage):

```typescript
/**
 * Interface for query execution backends.
 * Allows the query editor to work with different database backends.
 */
export interface QueryExecutor {
	/** Execute a SQL query and return rows */
	execute(sql: string): Promise<Record<string, unknown>[]>;
	/** Database type for parameter substitution style (defaults to inline) */
	dbType?: DatabaseType;
}
```

**Step 2: Add DatabaseType import if needed**

Check if DatabaseType is already imported/exported. If not, add at the top:

```typescript
import type { DatabaseType } from './connection';
```

**Step 3: Export from types index**

In `src/lib/types/index.ts`, ensure QueryExecutor is exported.

**Step 4: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 5: Commit**

```bash
git add src/lib/types/query.ts src/lib/types/index.ts
git commit -m "feat: add QueryExecutor interface for pluggable execution backends"
```

---

## Task 2: Create Standalone Query Editor Component

The current query-editor.svelte is tightly coupled to useDatabase(). We'll create a simpler standalone version that the workspace can use.

**Files:**
- Create: `src/lib/components/standalone-query-editor.svelte`

**Step 1: Create the standalone editor component**

```svelte
<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { PlayIcon, CopyIcon } from '@lucide/svelte';
	import { toast } from 'svelte-sonner';
	import MonacoEditor from '$lib/components/monaco-editor.svelte';
	import VirtualResultsTable from '$lib/components/virtual-results-table.svelte';
	import ParameterInputDialog from '$lib/components/parameter-input-dialog.svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { hasParameters, extractParameters, createDefaultParameters, substituteParameters } from '$lib/db/query-params.js';
	import type { QueryExecutor, QueryParameter, ParameterValue, SchemaTable } from '$lib/types';
	import { m } from '$lib/paraglide/messages.js';

	interface Props {
		/** The query executor backend */
		executor: QueryExecutor;
		/** Current SQL value */
		value?: string;
		/** Callback when SQL changes */
		onSqlChange?: (sql: string) => void;
		/** Schema for Monaco autocomplete */
		schema?: SchemaTable[];
	}

	let { executor, value = '', onSqlChange, schema }: Props = $props();

	// Local state
	let editorValue = $state(value);
	let isExecuting = $state(false);
	let queryResults = $state<Record<string, unknown>[] | null>(null);
	let queryError = $state<string | null>(null);
	let executionTime = $state<number | null>(null);

	// Parameter dialog state
	let showParamsDialog = $state(false);
	let pendingParams = $state<QueryParameter[]>([]);

	// Sync external value changes
	$effect(() => {
		if (value !== editorValue) {
			editorValue = value;
		}
	});

	// Derived
	const resultColumns = $derived(
		queryResults && queryResults.length > 0 ? Object.keys(queryResults[0]) : []
	);
	const canRunQuery = $derived(editorValue.trim().length > 0 && !isExecuting);

	function handleEditorChange(sql: string) {
		editorValue = sql;
		onSqlChange?.(sql);
	}

	function handleExecute() {
		if (!canRunQuery) return;

		// Check for parameters
		if (hasParameters(editorValue)) {
			const paramNames = extractParameters(editorValue);
			pendingParams = createDefaultParameters(paramNames);
			showParamsDialog = true;
		} else {
			executeQuery(editorValue);
		}
	}

	function handleParamExecute(values: ParameterValue[]) {
		showParamsDialog = false;
		// Substitute parameters with inline values (tutorial DB doesn't support bind params)
		const dbType = executor.dbType ?? 'duckdb';
		const { sql: substitutedSql } = substituteParameters(editorValue, values, dbType);
		executeQuery(substitutedSql);
	}

	function handleParamCancel() {
		showParamsDialog = false;
	}

	async function executeQuery(sql: string) {
		isExecuting = true;
		queryError = null;
		const startTime = performance.now();

		try {
			queryResults = await executor.execute(sql);
			executionTime = performance.now() - startTime;
		} catch (err) {
			queryError = err instanceof Error ? err.message : String(err);
			queryResults = null;
			executionTime = null;
		} finally {
			isExecuting = false;
		}
	}

	async function handleCopy() {
		try {
			await navigator.clipboard.writeText(editorValue);
			toast.success(m.workspace_sql_copied?.() ?? 'SQL copied to clipboard');
		} catch {
			toast.error('Failed to copy');
		}
	}

	async function copyError() {
		if (!queryError) return;
		try {
			await navigator.clipboard.writeText(queryError);
			toast.success(m.workspace_error_copied());
		} catch {
			toast.error(m.workspace_copy_error_failed());
		}
	}

	// Expose methods for parent components
	export function reset() {
		queryResults = null;
		queryError = null;
		executionTime = null;
	}

	export function runQuery() {
		handleExecute();
	}

	export function getState() {
		return {
			isExecuting,
			queryResults,
			queryError,
			executionTime,
			canRunQuery
		};
	}
</script>

<div class="flex flex-col h-full">
	<Resizable.PaneGroup direction="vertical" class="flex-1">
		<!-- SQL Editor Pane -->
		<Resizable.Pane defaultSize={50} minSize={20}>
			<div class="flex flex-col h-full border-b">
				<!-- Toolbar -->
				<div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
					<span class="font-medium text-sm">SQL</span>
					<div class="flex items-center gap-1">
						<Button
							variant="ghost"
							size="sm"
							class="h-7 px-2 gap-1.5 text-xs"
							onclick={handleCopy}
						>
							<CopyIcon class="size-3.5" />
							Copy
						</Button>
						<Button
							variant="default"
							size="sm"
							class="h-7 px-3 gap-1.5 text-xs"
							onclick={handleExecute}
							disabled={!canRunQuery}
						>
							<PlayIcon class="size-3.5" />
							{isExecuting ? 'Running...' : 'Run'}
						</Button>
					</div>
				</div>

				<!-- Editor -->
				<div class="flex-1 min-h-0">
					<MonacoEditor
						bind:value={editorValue}
						{schema}
						onChange={handleEditorChange}
					/>
				</div>
			</div>
		</Resizable.Pane>

		<Resizable.Handle withHandle />

		<!-- Results Pane -->
		<Resizable.Pane defaultSize={50} minSize={15}>
			<div class="flex flex-col h-full">
				<div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
					<span class="font-medium text-sm">{m.workspace_results()}</span>
					{#if queryResults !== null}
						<span class="text-xs text-muted-foreground">
							{queryResults.length === 1 ? m.workspace_row_count({ count: 1 }) : m.workspace_rows_count({ count: queryResults.length })}
							{#if executionTime !== null}
								· {executionTime.toFixed(0)}ms
							{/if}
						</span>
					{/if}
				</div>
				<div class="flex-1 min-h-0 overflow-auto">
					{#if queryError}
						<div class="p-4 text-sm text-destructive bg-destructive/10">
							<div class="flex items-start justify-between gap-2">
								<div>
									<p class="font-medium">{m.workspace_error()}</p>
									<p class="font-mono text-xs mt-1">{queryError}</p>
								</div>
								<Button
									variant="ghost"
									size="icon"
									class="shrink-0 size-8 text-destructive/70 hover:text-destructive hover:bg-destructive/10"
									onclick={copyError}
								>
									<CopyIcon class="size-4" />
								</Button>
							</div>
						</div>
					{:else if queryResults !== null && queryResults.length > 0}
						<VirtualResultsTable columns={resultColumns} rows={queryResults} compact />
					{:else if queryResults !== null}
						<div class="p-4 text-sm text-muted-foreground text-center">
							{m.workspace_no_rows()}
						</div>
					{:else}
						<div class="p-4 text-sm text-muted-foreground text-center">
							{m.workspace_run_query_hint()}
						</div>
					{/if}
				</div>
			</div>
		</Resizable.Pane>
	</Resizable.PaneGroup>
</div>

<ParameterInputDialog
	bind:open={showParamsDialog}
	parameters={pendingParams}
	onExecute={handleParamExecute}
	onCancel={handleParamCancel}
/>
```

**Step 2: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 3: Commit**

```bash
git add src/lib/components/standalone-query-editor.svelte
git commit -m "feat: add standalone query editor component with executor prop"
```

---

## Task 3: Update Query Builder Workspace

**Files:**
- Modify: `src/lib/components/query-builder/workspace.svelte`

**Step 1: Replace execution logic with standalone editor**

Replace the entire file content:

```svelte
<script lang="ts">
	import { SvelteFlowProvider } from '@xyflow/svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { executeQuery } from '$lib/tutorial/database';
	import { getTutorialSchema } from '$lib/tutorial/database';
	import QueryBuilderCanvas from './canvas.svelte';
	import FilterPanel from './filter-panel.svelte';
	import DndProvider from './dnd-provider.svelte';
	import StandaloneQueryEditor from '$lib/components/standalone-query-editor.svelte';
	import type { QueryExecutor } from '$lib/types';

	interface Props {
		/** Content for the left sidebar (e.g., table palette, challenge card) */
		leftPanel?: import('svelte').Snippet;
	}

	let { leftPanel }: Props = $props();

	const qb = useQueryBuilder();

	// Create tutorial executor
	const tutorialExecutor: QueryExecutor = {
		execute: executeQuery,
		dbType: 'duckdb'
	};

	// Get schema for Monaco autocomplete
	const tutorialSchema = getTutorialSchema();

	// The SQL value - use custom SQL if user has typed something, otherwise use generated SQL
	const sqlValue = $derived(qb.customSql ?? qb.generatedSql);

	// Handle SQL changes from the editor
	function handleSqlChange(sql: string) {
		qb.customSql = sql;
	}

	// Reference to the editor for external control
	let editorRef: { reset: () => void; runQuery: () => void; getState: () => unknown } | undefined;

	export function reset() {
		editorRef?.reset();
	}

	export async function runQuery() {
		editorRef?.runQuery();
	}

	export function getState() {
		return editorRef?.getState() ?? {
			isExecuting: false,
			queryResults: null,
			queryError: null,
			executionTime: null,
			canRunQuery: false
		};
	}
</script>

<SvelteFlowProvider>
	<DndProvider>
		<div class="flex-1 flex min-h-0">
			<!-- Left panel -->
			{#if leftPanel}
				{@render leftPanel()}
			{/if}

			<!-- Canvas + Filter panel | SQL editor + Results -->
			<Resizable.PaneGroup direction="horizontal" class="flex-1">
				<Resizable.Pane defaultSize={55} minSize={30}>
					<Resizable.PaneGroup direction="vertical">
						<Resizable.Pane defaultSize={55} minSize={15}>
							<!-- Canvas -->
							<div class="h-full">
								<QueryBuilderCanvas />
							</div>
						</Resizable.Pane>

						<Resizable.Handle withHandle />

						<Resizable.Pane defaultSize={45} minSize={10}>
							<!-- Filter panel -->
							<div class="h-full">
								<FilterPanel />
							</div>
						</Resizable.Pane>
					</Resizable.PaneGroup>
				</Resizable.Pane>

				<Resizable.Handle withHandle />

				<Resizable.Pane defaultSize={45} minSize={20}>
					<!-- SQL Editor + Results (unified component) -->
					<div class="h-full border-l">
						<StandaloneQueryEditor
							bind:this={editorRef}
							executor={tutorialExecutor}
							value={sqlValue}
							onSqlChange={handleSqlChange}
							schema={tutorialSchema}
						/>
					</div>
				</Resizable.Pane>
			</Resizable.PaneGroup>
		</div>
	</DndProvider>
</SvelteFlowProvider>
```

**Step 2: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 3: Commit**

```bash
git add src/lib/components/query-builder/workspace.svelte
git commit -m "refactor: use standalone query editor in workspace"
```

---

## Task 4: Add Two-Way SQL Sync

The standalone editor needs to sync SQL changes back to the query builder for two-way editing (like the old sql-editor.svelte did).

**Files:**
- Modify: `src/lib/components/standalone-query-editor.svelte`

**Step 1: Add parsing and sync logic**

Add imports at top of script:

```typescript
import { parseSql } from '$lib/tutorial/sql-parser';
import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
```

Add to Props interface:

```typescript
/** Enable two-way sync with visual query builder */
enableVisualSync?: boolean;
```

Add after props destructuring:

```typescript
// Get query builder if visual sync is enabled
const qb = enableVisualSync ? useQueryBuilder() : null;
```

Update handleEditorChange:

```typescript
function handleEditorChange(sql: string) {
	editorValue = sql;
	onSqlChange?.(sql);

	// Two-way sync with visual query builder
	if (qb) {
		const parsed = parseSql(sql, { validTableNames: null });
		if (parsed) {
			qb.applyFromParsedSql(parsed);
		}
	}
}
```

**Step 2: Update workspace to enable visual sync**

In workspace.svelte, add the prop:

```svelte
<StandaloneQueryEditor
	bind:this={editorRef}
	executor={tutorialExecutor}
	value={sqlValue}
	onSqlChange={handleSqlChange}
	schema={tutorialSchema}
	enableVisualSync
/>
```

**Step 3: Run type check**

Run: `npm run check`
Expected: No type errors

**Step 4: Commit**

```bash
git add src/lib/components/standalone-query-editor.svelte src/lib/components/query-builder/workspace.svelte
git commit -m "feat: add two-way SQL sync to standalone editor"
```

---

## Task 5: Clean Up Old SQL Editor

**Files:**
- Remove: `src/lib/components/query-builder/sql-editor.svelte` (optional - keep if used elsewhere)

**Step 1: Check if sql-editor.svelte is used elsewhere**

Run: `grep -r "query-builder/sql-editor" src/`

If only used in workspace.svelte (which we've replaced), it can be removed.

**Step 2: Remove if unused**

```bash
git rm src/lib/components/query-builder/sql-editor.svelte
git commit -m "chore: remove unused sql-editor component"
```

---

## Task 6: Manual Testing

**Step 1: Start dev server**

Run: `npm run tauri dev`

**Step 2: Test in Learn section**

1. Go to Learn → any lesson or sandbox
2. Add tables to canvas
3. Add a WHERE filter with `{{customer_id}}`
4. Click Run
5. **Verify**: Parameter prompt dialog appears
6. Enter a value and submit
7. **Verify**: Query executes with substituted value

**Step 3: Test SQL sync**

1. Edit SQL manually in the editor
2. **Verify**: Visual canvas updates (if SQL is valid)
3. Modify canvas (add filter, select columns)
4. **Verify**: SQL editor updates

**Step 4: Test existing Manage section**

1. Go to Manage with a database connection
2. Run queries with parameters
3. **Verify**: Still works as before (no regression)

---

## Task 7: Final Verification

**Step 1: Run type check**

Run: `npm run check`
Expected: No errors

**Step 2: Review all changes**

```bash
git diff main --stat
```

**Step 3: Final commit if needed**

```bash
git status
# If any uncommitted changes:
git add -A
git commit -m "chore: final cleanup for unified query editor"
```
