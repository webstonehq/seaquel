<script lang="ts">
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { Collapsible, CollapsibleTrigger, CollapsibleContent } from '$lib/components/ui/collapsible';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Label } from '$lib/components/ui/label';
	import * as Select from '$lib/components/ui/select';
	import FilterIcon from '@lucide/svelte/icons/filter';
	import ArrowUpDownIcon from '@lucide/svelte/icons/arrow-up-down';
	import GroupIcon from '@lucide/svelte/icons/group';
	import HashIcon from '@lucide/svelte/icons/hash';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import type { FilterOperator, SortDirection, AggregateFunction, HavingOperator, DisplayAggregate } from '$lib/types';
	import CalculatorIcon from '@lucide/svelte/icons/calculator';

	const qb = useQueryBuilder();

	// Collapsible state
	let whereOpen = $state(true);
	let groupByOpen = $state(true);
	let havingOpen = $state(true);
	let orderByOpen = $state(true);
	let limitOpen = $state(true);
	let aggregatesOpen = $state(true);

	// Filter operators with labels
	const FILTER_OPERATORS: { value: FilterOperator; label: string }[] = [
		{ value: '=', label: 'equals' },
		{ value: '!=', label: 'not equals' },
		{ value: '>', label: 'greater than' },
		{ value: '<', label: 'less than' },
		{ value: '>=', label: '>=' },
		{ value: '<=', label: '<=' },
		{ value: 'LIKE', label: 'contains' },
		{ value: 'IS NULL', label: 'is null' },
		{ value: 'IS NOT NULL', label: 'is not null' }
	];

	// Sort directions
	const SORT_DIRECTIONS: { value: SortDirection; label: string }[] = [
		{ value: 'ASC', label: 'ASC' },
		{ value: 'DESC', label: 'DESC' }
	];

	// Connectors
	const CONNECTORS: { value: 'AND' | 'OR'; label: string }[] = [
		{ value: 'AND', label: 'AND' },
		{ value: 'OR', label: 'OR' }
	];

	// Aggregate functions for HAVING
	const AGGREGATE_FUNCTIONS: { value: AggregateFunction; label: string }[] = [
		{ value: 'COUNT', label: 'COUNT' },
		{ value: 'SUM', label: 'SUM' },
		{ value: 'AVG', label: 'AVG' },
		{ value: 'MIN', label: 'MIN' },
		{ value: 'MAX', label: 'MAX' }
	];

	// Operators for HAVING (numeric comparisons only)
	const HAVING_OPERATORS: { value: HavingOperator; label: string }[] = [
		{ value: '=', label: '=' },
		{ value: '!=', label: '!=' },
		{ value: '>', label: '>' },
		{ value: '<', label: '<' },
		{ value: '>=', label: '>=' },
		{ value: '<=', label: '<=' }
	];

	// Helper to check if operator needs a value input
	function operatorNeedsValue(operator: FilterOperator): boolean {
		return operator !== 'IS NULL' && operator !== 'IS NOT NULL';
	}

	// Add new filter
	function handleAddFilter() {
		qb.addFilter('', '=', '', 'AND');
	}

	// Add new group by
	function handleAddGroupBy() {
		qb.addGroupBy('');
	}

	// Add new having
	function handleAddHaving() {
		qb.addHaving('COUNT', '', '>', '', 'AND');
	}

	// Add new order by
	function handleAddOrderBy() {
		qb.addOrderBy('', 'ASC');
	}

	// Add new select aggregate
	function handleAddSelectAggregate() {
		qb.addSelectAggregate('COUNT', '*', '');
	}

	// Handle removing an aggregate (either column or select type)
	function handleRemoveAggregate(aggregate: DisplayAggregate) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.clearColumnAggregate(aggregate.tableId, aggregate.columnName);
		} else if (aggregate.source === 'select') {
			qb.removeSelectAggregate(aggregate.id);
		}
	}

	// Handle updating aggregate function (either column or select type)
	function handleUpdateAggregateFunction(aggregate: DisplayAggregate, func: AggregateFunction) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.setColumnAggregate(aggregate.tableId, aggregate.columnName, func, aggregate.alias);
		} else if (aggregate.source === 'select') {
			qb.updateSelectAggregate(aggregate.id, { function: func });
		}
	}

	// Handle updating aggregate alias (either column or select type)
	function handleUpdateAggregateAlias(aggregate: DisplayAggregate, alias: string) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.setColumnAggregate(aggregate.tableId, aggregate.columnName, aggregate.function, alias || undefined);
		} else if (aggregate.source === 'select') {
			qb.updateSelectAggregate(aggregate.id, { alias: alias || undefined });
		}
	}

	// Handle updating aggregate expression (converts column aggregates to select aggregates if expression changes)
	function handleUpdateAggregateExpression(aggregate: DisplayAggregate, expression: string) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			// Clear the column aggregate and create a select aggregate with the new expression
			qb.clearColumnAggregate(aggregate.tableId, aggregate.columnName);
			qb.addSelectAggregate(aggregate.function, expression, aggregate.alias);
		} else if (aggregate.source === 'select') {
			qb.updateSelectAggregate(aggregate.id, { expression });
		}
	}

	// Handle limit input change
	function handleLimitChange(event: Event) {
		const target = event.target as HTMLInputElement;
		const value = parseInt(target.value, 10);
		if (!isNaN(value) && value > 0) {
			qb.setLimit(value);
		}
	}

	// Handle no limit checkbox
	function handleNoLimitChange(checked: boolean) {
		if (checked) {
			qb.setLimit(null);
		} else {
			qb.setLimit(100);
		}
	}
</script>

<div class="border-t bg-muted/30 max-h-64 overflow-auto">
	<!-- WHERE Section -->
	<Collapsible bind:open={whereOpen}>
		<div class="border-b border-border">
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {whereOpen ? '' : '-rotate-90'}"
					/>
					<FilterIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">WHERE</span>
					{#if qb.filters.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.filters.length}
						</span>
					{/if}
				</div>
				<Button
					variant="ghost"
					size="sm"
					class="h-6 px-2 text-xs gap-1"
					onclick={(e: MouseEvent) => {
						e.stopPropagation();
						handleAddFilter();
					}}
				>
					<PlusIcon class="size-3" />
					Add
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.filters as filter, index (filter.id)}
						<div class="flex items-center gap-2 flex-wrap">
							{#if index > 0}
								<!-- Connector before this filter -->
								<Select.Root
									type="single"
									value={qb.filters[index - 1].connector}
									onValueChange={(value) => {
										if (value) {
											qb.updateFilter(qb.filters[index - 1].id, { connector: value as 'AND' | 'OR' });
										}
									}}
								>
									<Select.Trigger size="sm" class="w-16 h-7 text-xs">
										{qb.filters[index - 1].connector}
									</Select.Trigger>
									<Select.Content>
										{#each CONNECTORS as connector}
											<Select.Item value={connector.value} label={connector.label}>
												{connector.label}
											</Select.Item>
										{/each}
									</Select.Content>
								</Select.Root>
							{/if}

							<!-- Column input -->
							<Input
								type="text"
								placeholder="table.column"
								value={filter.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateFilter(filter.id, { column: target.value });
								}}
								class="h-7 text-xs w-32 font-mono"
							/>

							<!-- Operator select -->
							<Select.Root
								type="single"
								value={filter.operator}
								onValueChange={(value) => {
									if (value) {
										qb.updateFilter(filter.id, { operator: value as FilterOperator });
									}
								}}
							>
								<Select.Trigger size="sm" class="w-28 h-7 text-xs">
									{FILTER_OPERATORS.find(op => op.value === filter.operator)?.label ?? 'equals'}
								</Select.Trigger>
								<Select.Content>
									{#each FILTER_OPERATORS as op}
										<Select.Item value={op.value} label={op.label}>
											{op.label}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>

							<!-- Value input (hidden for IS NULL / IS NOT NULL) -->
							{#if operatorNeedsValue(filter.operator)}
								<Input
									type="text"
									placeholder="value"
									value={filter.value}
									oninput={(e: Event) => {
										const target = e.target as HTMLInputElement;
										qb.updateFilter(filter.id, { value: target.value });
									}}
									class="h-7 text-xs w-24"
								/>
							{/if}

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeFilter(filter.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.filters.length === 0}
						<p class="text-xs text-muted-foreground">No filters. Click "Add" to add a WHERE condition.</p>
					{/if}
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>

	<!-- GROUP BY Section -->
	<Collapsible bind:open={groupByOpen}>
		<div class="border-b border-border">
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {groupByOpen ? '' : '-rotate-90'}"
					/>
					<GroupIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">GROUP BY</span>
					{#if qb.groupBy.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.groupBy.length}
						</span>
					{/if}
				</div>
				<Button
					variant="ghost"
					size="sm"
					class="h-6 px-2 text-xs gap-1"
					onclick={(e: MouseEvent) => {
						e.stopPropagation();
						handleAddGroupBy();
					}}
				>
					<PlusIcon class="size-3" />
					Add
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.groupBy as group (group.id)}
						<div class="flex items-center gap-2">
							<!-- Column input -->
							<Input
								type="text"
								placeholder="table.column"
								value={group.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateGroupBy(group.id, target.value);
								}}
								class="h-7 text-xs w-40 font-mono"
							/>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeGroupBy(group.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.groupBy.length === 0}
						<p class="text-xs text-muted-foreground">No grouping. Click "Add" to add a GROUP BY column.</p>
					{/if}
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>

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
					{#if qb.allDisplayAggregates.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.allDisplayAggregates.length}
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
					{#each qb.allDisplayAggregates as aggregate (aggregate.id)}
						<div class="flex items-center gap-2 flex-wrap">
							<!-- Aggregate function select -->
							<Select.Root
								type="single"
								value={aggregate.function}
								onValueChange={(value) => {
									if (value) {
										handleUpdateAggregateFunction(aggregate, value as AggregateFunction);
									}
								}}
							>
								<Select.Trigger size="sm" class="w-20 h-7 text-xs">
									{aggregate.function}
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
								placeholder={aggregate.function === 'COUNT' ? '*' : 'expression'}
								value={aggregate.expression}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									handleUpdateAggregateExpression(aggregate, target.value);
								}}
								class="h-7 text-xs w-28 font-mono"
							/>
							<span class="text-xs text-muted-foreground">)</span>

							<!-- AS label and alias input -->
							<span class="text-xs text-muted-foreground">AS</span>
							<Input
								type="text"
								placeholder="alias"
								value={aggregate.alias ?? ''}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									handleUpdateAggregateAlias(aggregate, target.value);
								}}
								class="h-7 text-xs w-20 font-mono"
							/>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => handleRemoveAggregate(aggregate)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.allDisplayAggregates.length === 0}
						<p class="text-xs text-muted-foreground">No aggregates. Click "Add" to add COUNT, SUM, etc.</p>
					{/if}
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>

	<!-- HAVING Section -->
	<Collapsible bind:open={havingOpen}>
		<div class="border-b border-border">
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {havingOpen ? '' : '-rotate-90'}"
					/>
					<CalculatorIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">HAVING</span>
					{#if qb.having.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.having.length}
						</span>
					{/if}
				</div>
				<Button
					variant="ghost"
					size="sm"
					class="h-6 px-2 text-xs gap-1"
					onclick={(e: MouseEvent) => {
						e.stopPropagation();
						handleAddHaving();
					}}
				>
					<PlusIcon class="size-3" />
					Add
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.having as having, index (having.id)}
						<div class="flex items-center gap-2 flex-wrap">
							{#if index > 0}
								<!-- Connector before this having -->
								<Select.Root
									type="single"
									value={qb.having[index - 1].connector}
									onValueChange={(value) => {
										if (value) {
											qb.updateHaving(qb.having[index - 1].id, { connector: value as 'AND' | 'OR' });
										}
									}}
								>
									<Select.Trigger size="sm" class="w-16 h-7 text-xs">
										{qb.having[index - 1].connector}
									</Select.Trigger>
									<Select.Content>
										{#each CONNECTORS as connector}
											<Select.Item value={connector.value} label={connector.label}>
												{connector.label}
											</Select.Item>
										{/each}
									</Select.Content>
								</Select.Root>
							{/if}

							<!-- Aggregate function select -->
							<Select.Root
								type="single"
								value={having.aggregateFunction}
								onValueChange={(value) => {
									if (value) {
										qb.updateHaving(having.id, { aggregateFunction: value as AggregateFunction });
									}
								}}
							>
								<Select.Trigger size="sm" class="w-20 h-7 text-xs">
									{having.aggregateFunction}
								</Select.Trigger>
								<Select.Content>
									{#each AGGREGATE_FUNCTIONS as fn}
										<Select.Item value={fn.value} label={fn.label}>
											{fn.label}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>

							<!-- Column input with parentheses -->
							<span class="text-xs text-muted-foreground">(</span>
							<Input
								type="text"
								placeholder={having.aggregateFunction === 'COUNT' ? '*' : 'column'}
								value={having.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateHaving(having.id, { column: target.value });
								}}
								class="h-7 text-xs w-20 font-mono"
							/>
							<span class="text-xs text-muted-foreground">)</span>

							<!-- Operator select -->
							<Select.Root
								type="single"
								value={having.operator}
								onValueChange={(value) => {
									if (value) {
										qb.updateHaving(having.id, { operator: value as HavingOperator });
									}
								}}
							>
								<Select.Trigger size="sm" class="w-16 h-7 text-xs">
									{having.operator}
								</Select.Trigger>
								<Select.Content>
									{#each HAVING_OPERATORS as op}
										<Select.Item value={op.value} label={op.label}>
											{op.label}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>

							<!-- Value input -->
							<Input
								type="text"
								placeholder="value"
								value={having.value}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateHaving(having.id, { value: target.value });
								}}
								class="h-7 text-xs w-20"
							/>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeHaving(having.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.having.length === 0}
						<p class="text-xs text-muted-foreground">No HAVING conditions. Click "Add" to filter groups.</p>
					{/if}
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>

	<!-- ORDER BY Section -->
	<Collapsible bind:open={orderByOpen}>
		<div class="border-b border-border">
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {orderByOpen ? '' : '-rotate-90'}"
					/>
					<ArrowUpDownIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">ORDER BY</span>
					{#if qb.orderBy.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.orderBy.length}
						</span>
					{/if}
				</div>
				<Button
					variant="ghost"
					size="sm"
					class="h-6 px-2 text-xs gap-1"
					onclick={(e: MouseEvent) => {
						e.stopPropagation();
						handleAddOrderBy();
					}}
				>
					<PlusIcon class="size-3" />
					Add
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.orderBy as sort (sort.id)}
						<div class="flex items-center gap-2">
							<!-- Column input -->
							<Input
								type="text"
								placeholder="table.column"
								value={sort.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateOrderByColumn(sort.id, target.value);
								}}
								class="h-7 text-xs w-40 font-mono"
							/>

							<!-- Direction select -->
							<Select.Root
								type="single"
								value={sort.direction}
								onValueChange={(value) => {
									if (value) {
										qb.updateOrderBy(sort.id, value as SortDirection);
									}
								}}
							>
								<Select.Trigger size="sm" class="w-20 h-7 text-xs">
									{sort.direction}
								</Select.Trigger>
								<Select.Content>
									{#each SORT_DIRECTIONS as dir}
										<Select.Item value={dir.value} label={dir.label}>
											{dir.label}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeOrderBy(sort.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.orderBy.length === 0}
						<p class="text-xs text-muted-foreground">No sorting. Click "Add" to add an ORDER BY clause.</p>
					{/if}
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>

	<!-- LIMIT Section -->
	<Collapsible bind:open={limitOpen}>
		<div>
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {limitOpen ? '' : '-rotate-90'}"
					/>
					<HashIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">LIMIT</span>
				</div>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3">
					<div class="flex items-center gap-3">
						<span class="text-xs text-muted-foreground">Return first</span>
						<Input
							type="number"
							value={qb.limit ?? ''}
							oninput={handleLimitChange}
							disabled={qb.limit === null}
							min={1}
							class="h-7 text-xs w-20"
						/>
						<span class="text-xs text-muted-foreground">rows</span>

						<div class="flex items-center gap-2 ml-4">
							<Checkbox
								id="no-limit"
								checked={qb.limit === null}
								onCheckedChange={handleNoLimitChange}
							/>
							<Label for="no-limit" class="text-xs cursor-pointer">No limit</Label>
						</div>
					</div>
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>
</div>
