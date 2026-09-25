<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { getEngineClient, usesRustEngine } from "$lib/engine";
	import { log } from "$lib/utils/logger";
	import { latestDebounced } from "$lib/utils/latest-debounced";
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Separator } from "$lib/components/ui/separator";
	import * as Select from "$lib/components/ui/select/index.js";
	import * as Collapsible from "$lib/components/ui/collapsible/index.js";
	import { dndzone } from "svelte-dnd-action";
	import {
		PlusIcon,
		TrashIcon,
		CopyIcon,
		PlayIcon,
		ChevronRightIcon,
		GripVerticalIcon,
	} from "@lucide/svelte";
	import type {
		CreateTableColumn,
		CreateTableDefinition,
		CreateTableIndex,
		CreateTableForeignKey,
		ColumnTypeInfo,
	} from "$lib/types";
	import { parseCreateTableSql } from "$lib/db/parse-create-table";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";

	let { tabId }: { tabId: string } = $props();

	const db = useDatabase();
	const tab = $derived(db.state.createTableTabs.find((t) => t.id === tabId) ?? null);

	// The tab's connection. Effects key on these primitives rather than the
	// connection object, which reconnect() replaces with a copy.
	const connection = $derived(
		tab ? db.state.connections.find((c) => c.id === tab.connectionId) : null,
	);
	const connectionId = $derived(connection?.id);
	const connectionType = $derived(connection?.type);
	const providerConnectionId = $derived(connection?.providerConnectionId);
	// Rust-engine dialects need a live connection even for pure SQL generation;
	// the demo's TypeScript DuckDB adapter works offline.
	const needsConnection = $derived(
		!!connectionType && usesRustEngine({ type: connectionType }) && !providerConnectionId,
	);

	/** A client for the tab's connection; it reads the live provider id on every call. */
	function engineClient() {
		if (!connectionId || !connectionType) throw new Error("No connection");
		return getEngineClient(
			{ id: connectionId, type: connectionType, providerConnectionId },
			db.state,
		);
	}

	// Column types, loaded once per connection (after it connects, for Rust-engine dialects)
	let columnTypes = $state<ColumnTypeInfo[]>([]);
	$effect(() => {
		if (!connectionId || needsConnection) return;
		let cancelled = false;
		void (async () => {
			try {
				const types = await engineClient().columnTypes();
				if (!cancelled) columnTypes = types;
			} catch {
				if (!cancelled) columnTypes = [];
			}
		})();
		return () => {
			cancelled = true;
		};
	});

	// Group column types by category
	const typesByCategory = $derived(
		columnTypes.reduce<Record<string, ColumnTypeInfo[]>>((acc, t) => {
			(acc[t.category] ??= []).push(t);
			return acc;
		}, {}),
	);

	// Available schemas
	let availableSchemas = $state<string[]>([]);
	$effect(() => {
		if (!connectionId || !providerConnectionId) return;
		let cancelled = false;

		void (async () => {
			try {
				const schemas = await engineClient().listSchemas();
				if (cancelled) return;
				availableSchemas = schemas;
				if (availableSchemas.length === 1 && !tab?.tableDefinition.schemaName) {
					updateDefinition((def) => ({ ...def, schemaName: availableSchemas[0] }));
				}
			} catch {
				if (!cancelled) availableSchemas = [];
			}
		})();
		return () => {
			cancelled = true;
		};
	});

	// ── Two-way SQL ↔ Form sync ──────────────────────────────────────
	// `sqlText` is the editable SQL string shown in the textarea.
	// When the form changes we regenerate SQL; when the user types in
	// the textarea we parse it back into the form.
	// `lastEditSource` prevents infinite loops.
	let sqlText = $state("");
	let lastEditSource = $state<"form" | "sql">("form");
	let parseError = $state(false);

	// Form → SQL: regenerate when the definition changes (and the last edit was from the form).
	// Debounced; a stale reply never overwrites a newer preview. On error, keep the current
	// sqlText and show why next to the pane until the next successful preview.
	let previewError = $state<string | null>(null);
	const ddlPreview = latestDebounced<string>(
		150,
		(sql) => {
			sqlText = sql;
			parseError = false;
			previewError = null;
		},
		(error) => {
			const reason = error instanceof Error ? error.message : String(error);
			void log.debug(`Create-table SQL preview failed: ${reason}`);
			previewError = reason;
		},
	);
	$effect(() => {
		// Snapshot the definition so this effect re-runs on every form change
		const def = tab?.tableDefinition ? $state.snapshot(tab.tableDefinition) : undefined;
		if (!def) return;
		if (lastEditSource !== "form") return;
		const original =
			tab?.isEditMode && tab.originalDefinition ? $state.snapshot(tab.originalDefinition) : undefined;
		if (needsConnection) return; // the pane shows "Connect to preview SQL"

		// Made here (not in the timer) so a connection change also regenerates
		let client: ReturnType<typeof engineClient>;
		try {
			client = engineClient();
		} catch {
			return; // keep current sqlText
		}
		ddlPreview.schedule(() =>
			original ? client.alterTable(original, def) : client.createTable(def),
		);
		return () => ddlPreview.cancel();
	});

	// SQL → Form: parse when the user edits the textarea
	function handleSqlInput(newSql: string) {
		sqlText = newSql;
		lastEditSource = "sql";
		parseError = false;

		if (!newSql.trim()) {
			db.createTableTabs.updateDefinition(tabId, (def) => ({
				...def,
				tableName: "",
				columns: [],
				indexes: [],
				foreignKeys: [],
			}));
			return;
		}

		const parsed = parseCreateTableSql(newSql);
		if (parsed) {
			db.createTableTabs.updateDefinition(tabId, () => parsed);
		} else {
			parseError = true;
		}
	}

	// Column type lookup for conditional UI
	const typeInfoMap = $derived(
		new Map(columnTypes.map((t) => [t.name.toUpperCase(), t])),
	);

	function updateDefinition(updater: (def: CreateTableDefinition) => CreateTableDefinition) {
		if (!tab) return;
		lastEditSource = "form";
		db.createTableTabs.updateDefinition(tabId, updater);
	}

	let newColumnId = $state<string | null>(null);

	function addColumn() {
		const id = crypto.randomUUID();
		newColumnId = id;
		updateDefinition((def) => ({
			...def,
			columns: [
				...def.columns,
				{
					id,
					name: "",
					type: columnTypes[0]?.name ?? "TEXT",
					nullable: true,
					defaultValue: "",
					isPrimaryKey: false,
					isUnique: false,
				},
			],
		}));
	}

	function removeColumn(id: string) {
		updateDefinition((def) => ({
			...def,
			columns: def.columns.filter((c) => c.id !== id),
		}));
	}

	function updateColumn(id: string, updates: Partial<CreateTableColumn>) {
		updateDefinition((def) => ({
			...def,
			columns: def.columns.map((c) => (c.id === id ? { ...c, ...updates } : c)),
		}));
	}

	function addIndex() {
		updateDefinition((def) => ({
			...def,
			indexes: [
				...def.indexes,
				{
					id: crypto.randomUUID(),
					name: "",
					columns: [],
					unique: false,
					type: "btree",
				},
			],
		}));
	}

	function removeIndex(id: string) {
		updateDefinition((def) => ({
			...def,
			indexes: def.indexes.filter((i) => i.id !== id),
		}));
	}

	function addForeignKey() {
		updateDefinition((def) => ({
			...def,
			foreignKeys: [
				...def.foreignKeys,
				{
					id: crypto.randomUUID(),
					column: "",
					referencedSchema: def.schemaName,
					referencedTable: "",
					referencedColumn: "",
				},
			],
		}));
	}

	function removeForeignKey(id: string) {
		updateDefinition((def) => ({
			...def,
			foreignKeys: def.foreignKeys.filter((f) => f.id !== id),
		}));
	}

	async function copySql() {
		if (sqlText) {
			const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
			await writeText(sqlText);
			toast.success("SQL copied to clipboard");
		}
	}

	let isCreating = $state(false);

	async function handleCreate() {
		if (!tab || isCreating) return;
		if (!tab.tableDefinition.tableName.trim()) {
			errorToast("Table name is required");
			return;
		}
		if (tab.tableDefinition.columns.length === 0) {
			errorToast("At least one column is required");
			return;
		}
		const hasEmptyColumnName = tab.tableDefinition.columns.some((c) => !c.name.trim());
		if (hasEmptyColumnName) {
			errorToast("All columns must have names");
			return;
		}

		isCreating = true;
		try {
			const success = await db.createTableTabs.executeCreate(tabId);
			if (success && !tab.isEditMode) {
				const schemaName = tab.tableDefinition.schemaName;
				const tableName = tab.tableDefinition.tableName;
				db.createTableTabs.remove(tabId);
				const dataTabId = db.dataTabs.addWithoutRefresh({ name: tableName, schema: schemaName, type: "table", columns: [], indexes: [] });
				db.ui.setActiveView("data");
				if (dataTabId) await db.dataTabs.refresh(dataTabId);
			}
		} finally {
			isCreating = false;
		}
	}

	// Collapsible states
	let indexesOpen = $state(false);
	let foreignKeysOpen = $state(false);

	// Drag-and-drop column reordering
	let dragColumns = $state<CreateTableColumn[]>([]);
	let isDragging = $state(false);

	const displayColumns = $derived(
		isDragging ? dragColumns : (tab?.tableDefinition.columns ?? []),
	);

	function handleDndConsider(e: CustomEvent<{ items: CreateTableColumn[] }>) {
		isDragging = true;
		dragColumns = e.detail.items;
	}

	function handleDndFinalize(e: CustomEvent<{ items: CreateTableColumn[] }>) {
		isDragging = false;
		dragColumns = [];
		updateDefinition((def) => ({ ...def, columns: e.detail.items }));
	}
</script>

<div class="flex flex-col h-full overflow-hidden">
	{#if tab}
		<div class="flex h-full">
			<!-- Left: Form -->
			<div class="flex-1 overflow-auto p-4 space-y-6 min-w-0">
				<!-- Table Name & Schema -->
				<div class="grid grid-cols-2 gap-4">
					<div class="space-y-2">
						<Label>{m.create_table_table_name()}</Label>
						<Input
							value={tab.tableDefinition.tableName}
							placeholder="my_table"
							oninput={(e) =>
								updateDefinition((def) => ({
									...def,
									tableName: (e.target as HTMLInputElement).value,
								}))}
						/>
					</div>
					<div class="space-y-2">
						<Label>{m.create_table_schema()}</Label>
						{#if availableSchemas.length > 0}
							<Select.Root
								type="single"
								value={tab.tableDefinition.schemaName}
								onValueChange={(v) =>
									updateDefinition((def) => ({
										...def,
										schemaName: v,
									}))}
							>
								<Select.Trigger class="w-full">
									{tab.tableDefinition.schemaName || "Select schema"}
								</Select.Trigger>
								<Select.Content>
									{#each availableSchemas as schema}
										<Select.Item value={schema}>{schema}</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						{:else}
							<Input
								value={tab.tableDefinition.schemaName}
								placeholder="public"
								oninput={(e) =>
									updateDefinition((def) => ({
										...def,
										schemaName: (e.target as HTMLInputElement).value,
									}))}
							/>
						{/if}
					</div>
				</div>

				<Separator />

				<!-- Columns -->
				<div class="space-y-3">
					<h3 class="text-sm font-semibold">{m.create_table_columns()}</h3>

					{#if tab.tableDefinition.columns.length > 0}
						{@const colGrid = "32px 1fr 140px 72px 40px 52px 52px 1fr 36px"}
						<div class="border rounded-lg overflow-hidden text-sm">
							<!-- Header -->
							<div class="grid bg-muted" style="grid-template-columns: {colGrid};">
								<div class="px-2 py-2"></div>
								<div class="px-2 py-2 font-medium">{m.create_table_column_name()}</div>
								<div class="px-2 py-2 font-medium">{m.create_table_column_type()}</div>
								<div class="px-2 py-2 font-medium">{m.create_table_column_params()}</div>
								<div class="px-2 py-2 font-medium text-center">{m.create_table_column_pk()}</div>
								<div class="px-2 py-2 font-medium text-center">{m.create_table_column_nullable()}</div>
								<div class="px-2 py-2 font-medium text-center">{m.create_table_column_unique()}</div>
								<div class="px-2 py-2 font-medium">{m.create_table_column_default()}</div>
								<div class="px-2 py-2"></div>
							</div>
							<!-- Draggable rows -->
							<div
								use:dndzone={{
									items: displayColumns,
									type: "create-table-columns",
									dropTargetStyle: {},
									dragDisabled: false,
								}}
								onconsider={handleDndConsider}
								onfinalize={handleDndFinalize}
							>
								{#each displayColumns as column, i (column.id)}
									{@const typeInfo = typeInfoMap.get(column.type.toUpperCase())}
									<div
										class={["grid items-center border-t hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}
										style="grid-template-columns: {colGrid};"
									>
										<div class="px-1 py-1 flex justify-center cursor-grab">
											<GripVerticalIcon class="size-3 text-muted-foreground" />
										</div>
										<div class="px-2 py-1">
											<Input
												value={column.name}
												placeholder="column_name"
												class="h-7 text-xs"
												oninput={(e) =>
													updateColumn(column.id, {
														name: (e.target as HTMLInputElement).value,
													})}
												{@attach (el) => {
													if (column.id === newColumnId) {
														el.focus();
														newColumnId = null;
													}
												}}
											/>
										</div>
										<div class="px-2 py-1">
											<Select.Root
												type="single"
												value={column.type}
												onValueChange={(v) =>
													updateColumn(column.id, { type: v })}
											>
												<Select.Trigger class="h-7 text-xs w-full">
													{column.type}
												</Select.Trigger>
												<Select.Content class="max-h-60">
													{#each Object.entries(typesByCategory) as [category, types]}
														<Select.Group>
															<Select.GroupHeading>{category}</Select.GroupHeading>
															{#each types as t}
																<Select.Item value={t.name}>{t.name}</Select.Item>
															{/each}
														</Select.Group>
													{/each}
												</Select.Content>
											</Select.Root>
										</div>
										<div class="px-2 py-1">
											{#if typeInfo?.hasLength || typeInfo?.hasPrecision}
												<Input
													value={typeInfo?.hasPrecision ? (column.precision ?? "") : (column.length ?? "")}
													placeholder={typeInfo?.hasPrecision ? "10,2" : "255"}
													class="h-7 text-xs"
													oninput={(e) => {
														const val = (e.target as HTMLInputElement).value;
														if (typeInfo?.hasPrecision) {
															updateColumn(column.id, { precision: val });
														} else {
															updateColumn(column.id, { length: val });
														}
													}}
												/>
											{/if}
										</div>
										<div class="px-2 py-1 flex justify-center">
											<Checkbox
												checked={column.isPrimaryKey}
												onCheckedChange={(v) =>
													updateColumn(column.id, {
														isPrimaryKey: v === true,
														nullable: v === true ? false : column.nullable,
													})}
											/>
										</div>
										<div class="px-2 py-1 flex justify-center">
											<Checkbox
												checked={column.nullable}
												disabled={column.isPrimaryKey}
												onCheckedChange={(v) =>
													updateColumn(column.id, { nullable: v === true })}
											/>
										</div>
										<div class="px-2 py-1 flex justify-center">
											<Checkbox
												checked={column.isUnique}
												onCheckedChange={(v) =>
													updateColumn(column.id, { isUnique: v === true })}
											/>
										</div>
										<div class="px-2 py-1">
											<Input
												value={column.defaultValue}
												placeholder=""
												class="h-7 text-xs"
												oninput={(e) =>
													updateColumn(column.id, {
														defaultValue: (e.target as HTMLInputElement).value,
													})}
											/>
										</div>
										<div class="px-2 py-1 flex justify-center">
											<Button
												size="icon"
												variant="ghost"
												class="size-6"
												onclick={() => removeColumn(column.id)}
											>
												<TrashIcon class="size-3" />
											</Button>
										</div>
									</div>
								{/each}
							</div>
							<!-- Add column row -->
							<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
							<div
								class="flex items-center gap-2 px-2.5 py-2 border-t border-dashed text-muted-foreground hover:text-foreground hover:bg-muted/50 cursor-pointer transition-colors"
								onclick={addColumn}
							>
								<PlusIcon class="size-3.5" />
								<span class="text-xs pl-3">{m.create_table_add_column()}</span>
							</div>
						</div>
					{:else}
						<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
						<div
							class="border border-dashed rounded-lg flex items-center justify-center py-8 text-muted-foreground hover:text-foreground hover:border-foreground/30 cursor-pointer transition-colors"
							onclick={addColumn}
						>
							<PlusIcon class="size-4 me-2" />
							<span class="text-sm">{m.create_table_add_column()}</span>
						</div>
					{/if}
				</div>

				<!-- Indexes (collapsible) -->
				<Collapsible.Root bind:open={indexesOpen}>
					<Collapsible.Trigger class="flex items-center gap-2 text-sm font-semibold w-full hover:text-foreground">
						<ChevronRightIcon
							class={["size-4 transition-transform", indexesOpen && "rotate-90"]}
						/>
						{m.create_table_indexes()} ({tab.tableDefinition.indexes.length})
					</Collapsible.Trigger>
					<Collapsible.Content class="mt-3 space-y-3">
						{#each tab.tableDefinition.indexes as index (index.id)}
							<div class="flex items-center gap-2 border rounded-lg p-2">
								<Input
									value={index.name}
									placeholder="index_name"
									class="h-7 text-xs flex-1"
									oninput={(e) =>
										updateDefinition((def) => ({
											...def,
											indexes: def.indexes.map((idx) =>
												idx.id === index.id
													? { ...idx, name: (e.target as HTMLInputElement).value }
													: idx,
											),
										}))}
								/>
								<Select.Root
									type="multiple"
									value={index.columns}
									onValueChange={(v) =>
										updateDefinition((def) => ({
											...def,
											indexes: def.indexes.map((idx) =>
												idx.id === index.id ? { ...idx, columns: v } : idx,
											),
										}))}
								>
									<Select.Trigger class="h-7 text-xs flex-1 min-w-[120px]">
										{index.columns.length > 0
											? index.columns.join(", ")
											: "Select columns"}
									</Select.Trigger>
									<Select.Content>
										{#each tab.tableDefinition.columns as col (col.id)}
											<Select.Item value={col.name}>{col.name}</Select.Item>
										{/each}
									</Select.Content>
								</Select.Root>
								<label class="flex items-center gap-1 text-xs shrink-0">
									<Checkbox
										checked={index.unique}
										onCheckedChange={(v) =>
											updateDefinition((def) => ({
												...def,
												indexes: def.indexes.map((idx) =>
													idx.id === index.id ? { ...idx, unique: v === true } : idx,
												),
											}))}
									/>
									Unique
								</label>
								<Button
									size="icon"
									variant="ghost"
									class="size-6 shrink-0"
									onclick={() => removeIndex(index.id)}
								>
									<TrashIcon class="size-3" />
								</Button>
							</div>
						{/each}
						<Button size="sm" variant="outline" onclick={addIndex}>
							<PlusIcon class="size-3 me-1" />
							Add Index
						</Button>
					</Collapsible.Content>
				</Collapsible.Root>

				<!-- Foreign Keys (collapsible) -->
				<Collapsible.Root bind:open={foreignKeysOpen}>
					<Collapsible.Trigger class="flex items-center gap-2 text-sm font-semibold w-full hover:text-foreground">
						<ChevronRightIcon
							class={["size-4 transition-transform", foreignKeysOpen && "rotate-90"]}
						/>
						{m.create_table_foreign_keys()} ({tab.tableDefinition.foreignKeys.length})
					</Collapsible.Trigger>
					<Collapsible.Content class="mt-3 space-y-3">
						{#each tab.tableDefinition.foreignKeys as fk (fk.id)}
							<div class="flex items-center gap-2 border rounded-lg p-2">
								<Input
									value={fk.column}
									placeholder="column"
									class="h-7 text-xs flex-1"
									oninput={(e) =>
										updateDefinition((def) => ({
											...def,
											foreignKeys: def.foreignKeys.map((f) =>
												f.id === fk.id
													? { ...f, column: (e.target as HTMLInputElement).value }
													: f,
											),
										}))}
								/>
								<span class="text-xs text-muted-foreground shrink-0">references</span>
								<Input
									value={fk.referencedTable}
									placeholder="table"
									class="h-7 text-xs flex-1"
									oninput={(e) =>
										updateDefinition((def) => ({
											...def,
											foreignKeys: def.foreignKeys.map((f) =>
												f.id === fk.id
													? { ...f, referencedTable: (e.target as HTMLInputElement).value }
													: f,
											),
										}))}
								/>
								<span class="text-xs text-muted-foreground shrink-0">(</span>
								<Input
									value={fk.referencedColumn}
									placeholder="column"
									class="h-7 text-xs flex-1"
									oninput={(e) =>
										updateDefinition((def) => ({
											...def,
											foreignKeys: def.foreignKeys.map((f) =>
												f.id === fk.id
													? { ...f, referencedColumn: (e.target as HTMLInputElement).value }
													: f,
											),
										}))}
								/>
								<span class="text-xs text-muted-foreground shrink-0">)</span>
								<Button
									size="icon"
									variant="ghost"
									class="size-6 shrink-0"
									onclick={() => removeForeignKey(fk.id)}
								>
									<TrashIcon class="size-3" />
								</Button>
							</div>
						{/each}
						<Button size="sm" variant="outline" onclick={addForeignKey}>
							<PlusIcon class="size-3 me-1" />
							Add Foreign Key
						</Button>
					</Collapsible.Content>
				</Collapsible.Root>
			</div>

			<!-- Right: SQL Preview -->
			<div class="w-[400px] border-l flex flex-col shrink-0">
				<div class="flex items-center justify-between px-4 py-2 border-b bg-muted/30">
					<div class="flex items-center gap-2">
						<h3 class="text-sm font-semibold">{m.create_table_sql_preview()}</h3>
						{#if parseError}
							<span class="text-xs text-destructive">Parse error</span>
						{/if}
					</div>
					<div class="flex gap-1">
						<Button size="sm" variant="ghost" onclick={copySql}>
							<CopyIcon class="size-3 me-1" />
							{m.create_table_copy_sql()}
						</Button>
					</div>
				</div>
				{#if needsConnection}
					<p class="px-4 py-1.5 border-b text-xs text-muted-foreground">
						{m.create_table_connect_to_preview()}
					</p>
				{:else if previewError}
					<p class="px-4 py-1.5 border-b text-xs text-muted-foreground truncate" title={previewError}>
						{m.create_table_preview_unavailable({ reason: previewError })}
					</p>
				{/if}
				<div class="flex-1 overflow-auto">
					<textarea
						value={sqlText}
						oninput={(e) => handleSqlInput((e.target as HTMLTextAreaElement).value)}
						placeholder={needsConnection
							? m.create_table_connect_to_preview()
							: "-- Write or edit CREATE TABLE SQL here"}
						spellcheck={false}
						class="w-full h-full resize-none border-0 bg-transparent p-4 text-xs font-mono focus:outline-none focus:ring-0"
					></textarea>
				</div>
				<div class="p-4 border-t">
					<Button
						class="w-full"
						onclick={handleCreate}
						disabled={isCreating || !tab.tableDefinition.tableName.trim() || !tab.tableDefinition.schemaName || tab.tableDefinition.columns.length === 0}
					>
						<PlayIcon class="size-3 me-1" />
						{#if isCreating}
							{tab.isEditMode ? "Updating..." : "Creating..."}
						{:else}
							{tab.isEditMode ? m.create_table_update() : m.create_table_execute()}
						{/if}
					</Button>
				</div>
			</div>
		</div>
	{/if}
</div>
