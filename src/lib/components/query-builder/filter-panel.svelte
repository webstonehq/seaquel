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
	import BracesIcon from '@lucide/svelte/icons/braces';
	import type { FilterOperator, SortDirection, AggregateFunction, HavingOperator, DisplayAggregate, FilterCondition } from '$lib/types';
	import CalculatorIcon from '@lucide/svelte/icons/calculator';
	import LayersIcon from '@lucide/svelte/icons/layers';
	import { m } from '$lib/paraglide/messages.js';

	const qb = useQueryBuilder();

	// Context indicator - shows when editing subquery or CTE vs top-level
	const isEditingSubquery = $derived(qb.selectedSubqueryId !== null);
	const isEditingCte = $derived(qb.selectedCteId !== null);
	const isEditingNested = $derived(isEditingSubquery || isEditingCte);
	const contextLabel = $derived(
		isEditingCte
			? m.qb_context_cte({ name: qb.selectedCte?.name || 'unnamed' })
			: isEditingSubquery
				? m.qb_context_subquery()
				: m.qb_context_main_query()
	);

	// Get WHERE subqueries from active context for linking (supports nested subqueries)
	const whereSubqueries = $derived(qb.activeSubqueries.filter((s) => s.role === 'where'));

	// Collapsible state
	let whereOpen = $state(true);
	let groupByOpen = $state(true);
	let havingOpen = $state(true);
	let orderByOpen = $state(true);
	let limitOpen = $state(true);
	let aggregatesOpen = $state(true);

	// Filter operators with labels
	const FILTER_OPERATORS: { value: FilterOperator; label: () => string }[] = [
		{ value: '=', label: () => m.qb_op_equals() },
		{ value: '!=', label: () => m.qb_op_not_equals() },
		{ value: '>', label: () => m.qb_op_greater_than() },
		{ value: '<', label: () => m.qb_op_less_than() },
		{ value: '>=', label: () => m.qb_op_greater_than() },
		{ value: '<=', label: () => m.qb_op_less_than() },
		{ value: 'LIKE', label: () => m.qb_op_contains() },
		{ value: 'IN', label: () => m.qb_op_in() },
		{ value: 'NOT IN', label: () => m.qb_op_not_in() },
		{ value: 'IS NULL', label: () => m.qb_op_is_null() },
		{ value: 'IS NOT NULL', label: () => m.qb_op_is_not_null() },
		{ value: 'IS TRUE', label: () => m.qb_op_is_true() },
		{ value: 'IS FALSE', label: () => m.qb_op_is_false() },
		{ value: 'IS NOT TRUE', label: () => m.qb_op_is_not_true() },
		{ value: 'IS NOT FALSE', label: () => m.qb_op_is_not_false() }
	];

	// Helper to get operator label
	function getOperatorLabel(op: FilterOperator): string {
		const found = FILTER_OPERATORS.find(o => o.value === op);
		return found ? found.label() : op;
	}

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
		return !['IS NULL', 'IS NOT NULL', 'IS TRUE', 'IS FALSE', 'IS NOT TRUE', 'IS NOT FALSE'].includes(operator);
	}

	// Helper to check if filter is using a subquery
	function filterUsesSubquery(filter: FilterCondition): boolean {
		return Boolean(filter.subqueryId);
	}

	// Toggle subquery mode for a filter (works in active context - supports nested subqueries)
	function handleToggleSubquery(filterId: string, useSubquery: boolean) {
		if (useSubquery) {
			// Create a new WHERE subquery in the active context and link it
			const subquery = qb.addActiveSubquery('where', { x: 50, y: 50 }, filterId);
			qb.updateActiveFilter(filterId, { subqueryId: subquery.id, value: '' });
		} else {
			// Unlink subquery
			const filter = qb.activeFilters.find((f) => f.id === filterId);
			if (filter?.subqueryId) {
				qb.removeActiveSubquery(filter.subqueryId);
			}
			qb.updateActiveFilter(filterId, { subqueryId: undefined });
		}
	}

	// Link filter to existing subquery (works in active context - supports nested subqueries)
	function handleLinkSubquery(filterId: string, subqueryId: string) {
		if (subqueryId === '__new__') {
			// Create new subquery in active context
			const subquery = qb.addActiveSubquery('where', { x: 50, y: 50 }, filterId);
			qb.updateActiveFilter(filterId, { subqueryId: subquery.id, value: '' });
		} else {
			// Link to existing subquery in active context
			qb.updateActiveFilter(filterId, { subqueryId, value: '' });
			qb.linkActiveSubqueryToFilter(subqueryId, filterId);
		}
	}

	// Add new filter
	function handleAddFilter() {
		qb.addActiveFilter('', '=', '', 'AND');
	}

	// Add new group by
	function handleAddGroupBy() {
		qb.addActiveGroupBy('');
	}

	// Add new having
	function handleAddHaving() {
		qb.addActiveHaving('COUNT', '', '>', '', 'AND');
	}

	// Add new order by
	function handleAddOrderBy() {
		qb.addActiveOrderBy('', 'ASC');
	}

	// Add new select aggregate
	function handleAddSelectAggregate() {
		qb.addActiveSelectAggregate('COUNT', '*', '');
	}

	// Handle removing an aggregate (either column or select type)
	function handleRemoveAggregate(aggregate: DisplayAggregate) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.clearActiveColumnAggregate(aggregate.tableId, aggregate.columnName);
		} else if (aggregate.source === 'select') {
			qb.removeActiveSelectAggregate(aggregate.id);
		}
	}

	// Handle updating aggregate function (either column or select type)
	function handleUpdateAggregateFunction(aggregate: DisplayAggregate, func: AggregateFunction) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.setActiveColumnAggregate(aggregate.tableId, aggregate.columnName, func, aggregate.alias);
		} else if (aggregate.source === 'select') {
			qb.updateActiveSelectAggregate(aggregate.id, { function: func });
		}
	}

	// Handle updating aggregate alias (either column or select type)
	function handleUpdateAggregateAlias(aggregate: DisplayAggregate, alias: string) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			qb.setActiveColumnAggregate(aggregate.tableId, aggregate.columnName, aggregate.function, alias || undefined);
		} else if (aggregate.source === 'select') {
			qb.updateActiveSelectAggregate(aggregate.id, { alias: alias || undefined });
		}
	}

	// Handle updating aggregate expression (converts column aggregates to select aggregates if expression changes)
	function handleUpdateAggregateExpression(aggregate: DisplayAggregate, expression: string) {
		if (aggregate.source === 'column' && aggregate.tableId && aggregate.columnName) {
			// Clear the column aggregate and create a select aggregate with the new expression
			qb.clearActiveColumnAggregate(aggregate.tableId, aggregate.columnName);
			qb.addActiveSelectAggregate(aggregate.function, expression, aggregate.alias);
		} else if (aggregate.source === 'select') {
			qb.updateActiveSelectAggregate(aggregate.id, { expression });
		}
	}

	// Handle limit input change
	function handleLimitChange(event: Event) {
		const target = event.target as HTMLInputElement;
		const value = parseInt(target.value, 10);
		if (!isNaN(value) && value > 0) {
			qb.setActiveLimit(value);
		}
	}

	// Handle no limit checkbox
	function handleNoLimitChange(checked: boolean) {
		if (checked) {
			qb.setActiveLimit(null);
		} else {
			qb.setActiveLimit(100);
		}
	}
</script>

<div class="border-t bg-muted/30 h-full overflow-auto">
	<!-- Context Indicator -->
	{#if isEditingNested}
		<div class="flex items-center gap-2 px-3 py-1.5 border-b {isEditingCte ? 'bg-violet-500/10 border-violet-500/20' : 'bg-indigo-500/10 border-indigo-500/20'}">
			<LayersIcon class="size-3.5 {isEditingCte ? 'text-violet-500' : 'text-indigo-500'}" />
			<span class="text-xs font-medium {isEditingCte ? 'text-violet-600 dark:text-violet-400' : 'text-indigo-600 dark:text-indigo-400'}">
				{m.qb_editing_context({ context: contextLabel })}
			</span>
			<Button
				variant="ghost"
				size="sm"
				class="h-5 px-2 text-xs ml-auto {isEditingCte ? 'text-violet-600 hover:text-violet-700 hover:bg-violet-500/10' : 'text-indigo-600 hover:text-indigo-700 hover:bg-indigo-500/10'}"
				onclick={() => {
					qb.selectedSubqueryId = null;
					qb.selectedCteId = null;
				}}
			>
				{m.qb_back_to_main()}
			</Button>
		</div>
	{/if}

	<!-- WHERE Section -->
	<Collapsible bind:open={whereOpen}>
		<div class="border-b border-border">
			<CollapsibleTrigger class="flex items-center justify-between w-full px-3 py-2 hover:bg-muted/50">
				<div class="flex items-center gap-2">
					<ChevronDownIcon
						class="size-4 text-muted-foreground transition-transform duration-200 {whereOpen ? '' : '-rotate-90'}"
					/>
					<FilterIcon class="size-4 text-muted-foreground" />
					<span class="font-medium text-sm">{m.qb_section_where()}</span>
					{#if qb.activeFilters.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.activeFilters.length}
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
					{m.qb_add()}
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.activeFilters as filter, index (filter.id)}
						<div class="flex items-center gap-2 flex-wrap">
							{#if index > 0}
								<!-- Connector before this filter -->
								<Select.Root
									type="single"
									value={qb.activeFilters[index - 1].connector}
									onValueChange={(value) => {
										if (value) {
											qb.updateActiveFilter(qb.activeFilters[index - 1].id, { connector: value as 'AND' | 'OR' });
										}
									}}
								>
									<Select.Trigger size="sm" class="w-16 h-7 text-xs">
										{qb.activeFilters[index - 1].connector}
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
									qb.updateActiveFilter(filter.id, { column: target.value });
								}}
								class="h-7 text-xs w-32 font-mono"
							/>

							<!-- Operator select -->
							<Select.Root
								type="single"
								value={filter.operator}
								onValueChange={(value) => {
									if (value) {
										qb.updateActiveFilter(filter.id, { operator: value as FilterOperator });
									}
								}}
							>
								<Select.Trigger size="sm" class="w-28 h-7 text-xs">
									{getOperatorLabel(filter.operator)}
								</Select.Trigger>
								<Select.Content>
									{#each FILTER_OPERATORS as op}
										<Select.Item value={op.value} label={op.label()}>
											{op.label()}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>

							<!-- Value input or Subquery selector (hidden for IS NULL / IS NOT NULL) -->
							{#if operatorNeedsValue(filter.operator)}
								{#if filterUsesSubquery(filter)}
									<!-- Subquery indicator and selector -->
									<div class="flex items-center gap-1">
										<Select.Root
											type="single"
											value={filter.subqueryId ?? ''}
											onValueChange={(value) => {
												if (value) handleLinkSubquery(filter.id, value);
											}}
										>
											<Select.Trigger size="sm" class="h-7 w-32 text-xs border-indigo-500/30 bg-indigo-500/5">
												<BracesIcon class="size-3 mr-1 text-indigo-500" />
												{filter.subqueryId ? m.qb_context_subquery() : m.qb_select_placeholder()}
											</Select.Trigger>
											<Select.Content>
												{#each whereSubqueries as sq}
													<Select.Item value={sq.id} label={m.qb_context_subquery()}>
														{m.qb_subquery_tables({ count: sq.innerQuery.tables.length })}
													</Select.Item>
												{/each}
												<Select.Item value="__new__" label={m.qb_create_new_subquery()}>
													{m.qb_create_new_subquery()}
												</Select.Item>
											</Select.Content>
										</Select.Root>
										<Button
											variant="ghost"
											size="icon-sm"
											class="size-7 text-muted-foreground hover:text-foreground"
											title={m.qb_use_literal_value()}
											onclick={() => handleToggleSubquery(filter.id, false)}
										>
											<XIcon class="size-3" />
										</Button>
									</div>
								{:else}
									<!-- Regular value input with subquery toggle -->
									<div class="flex items-center gap-1">
										<Input
											type="text"
											placeholder="value"
											value={filter.value}
											oninput={(e: Event) => {
												const target = e.target as HTMLInputElement;
												qb.updateActiveFilter(filter.id, { value: target.value });
											}}
											class="h-7 text-xs w-24"
										/>
										<Button
											variant="ghost"
											size="icon-sm"
											class="size-7 text-indigo-500/70 hover:text-indigo-500 hover:bg-indigo-500/10"
											title={m.qb_use_subquery()}
											onclick={() => handleToggleSubquery(filter.id, true)}
										>
											<BracesIcon class="size-3" />
										</Button>
									</div>
								{/if}
							{/if}

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeActiveFilter(filter.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.activeFilters.length === 0}
						<p class="text-xs text-muted-foreground">{m.qb_no_filters()}</p>
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
					<span class="font-medium text-sm">{m.qb_section_group_by()}</span>
					{#if qb.activeGroupBy.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.activeGroupBy.length}
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
					{m.qb_add()}
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.activeGroupBy as group (group.id)}
						<div class="flex items-center gap-2">
							<!-- Column input -->
							<Input
								type="text"
								placeholder="table.column"
								value={group.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateActiveGroupBy(group.id, target.value);
								}}
								class="h-7 text-xs w-40 font-mono"
							/>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeActiveGroupBy(group.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.activeGroupBy.length === 0}
						<p class="text-xs text-muted-foreground">{m.qb_no_grouping()}</p>
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
					<span class="font-medium text-sm">{m.qb_section_aggregates()}</span>
					{#if qb.activeDisplayAggregates.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.activeDisplayAggregates.length}
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
					{m.qb_add()}
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.activeDisplayAggregates as aggregate (aggregate.id)}
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
							<span class="text-xs text-muted-foreground">{m.cte_as()}</span>
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

					{#if qb.activeDisplayAggregates.length === 0}
						<p class="text-xs text-muted-foreground">{m.qb_no_aggregates()}</p>
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
					<span class="font-medium text-sm">{m.qb_section_having()}</span>
					{#if qb.activeHaving.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.activeHaving.length}
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
					{m.qb_add()}
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.activeHaving as having, index (having.id)}
						<div class="flex items-center gap-2 flex-wrap">
							{#if index > 0}
								<!-- Connector before this having -->
								<Select.Root
									type="single"
									value={qb.activeHaving[index - 1].connector}
									onValueChange={(value) => {
										if (value) {
											qb.updateActiveHaving(qb.activeHaving[index - 1].id, { connector: value as 'AND' | 'OR' });
										}
									}}
								>
									<Select.Trigger size="sm" class="w-16 h-7 text-xs">
										{qb.activeHaving[index - 1].connector}
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
										qb.updateActiveHaving(having.id, { aggregateFunction: value as AggregateFunction });
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
									qb.updateActiveHaving(having.id, { column: target.value });
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
										qb.updateActiveHaving(having.id, { operator: value as HavingOperator });
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
									qb.updateActiveHaving(having.id, { value: target.value });
								}}
								class="h-7 text-xs w-20"
							/>

							<!-- Remove button -->
							<Button
								variant="ghost"
								size="icon-sm"
								class="size-7 text-muted-foreground hover:text-destructive"
								onclick={() => qb.removeActiveHaving(having.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.activeHaving.length === 0}
						<p class="text-xs text-muted-foreground">{m.qb_no_having()}</p>
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
					<span class="font-medium text-sm">{m.qb_section_order_by()}</span>
					{#if qb.activeOrderBy.length > 0}
						<span class="text-xs bg-muted rounded-full px-2 py-0.5 text-muted-foreground">
							{qb.activeOrderBy.length}
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
					{m.qb_add()}
				</Button>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3 space-y-2">
					{#each qb.activeOrderBy as sort (sort.id)}
						<div class="flex items-center gap-2">
							<!-- Column input -->
							<Input
								type="text"
								placeholder="table.column"
								value={sort.column}
								oninput={(e: Event) => {
									const target = e.target as HTMLInputElement;
									qb.updateActiveOrderByColumn(sort.id, target.value);
								}}
								class="h-7 text-xs w-40 font-mono"
							/>

							<!-- Direction select -->
							<Select.Root
								type="single"
								value={sort.direction}
								onValueChange={(value) => {
									if (value) {
										qb.updateActiveOrderBy(sort.id, value as SortDirection);
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
								onclick={() => qb.removeActiveOrderBy(sort.id)}
							>
								<XIcon class="size-3" />
							</Button>
						</div>
					{/each}

					{#if qb.activeOrderBy.length === 0}
						<p class="text-xs text-muted-foreground">{m.qb_no_sorting()}</p>
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
					<span class="font-medium text-sm">{m.qb_section_limit()}</span>
				</div>
			</CollapsibleTrigger>

			<CollapsibleContent>
				<div class="px-3 pb-3">
					<div class="flex items-center gap-3">
						<span class="text-xs text-muted-foreground">{m.qb_return_first()}</span>
						<Input
							type="number"
							value={qb.activeLimit ?? ''}
							oninput={handleLimitChange}
							disabled={qb.activeLimit === null}
							min={1}
							class="h-7 text-xs w-20"
						/>
						<span class="text-xs text-muted-foreground">{m.qb_rows()}</span>

						<div class="flex items-center gap-2 ml-4">
							<Checkbox
								id="no-limit"
								checked={qb.activeLimit === null}
								onCheckedChange={handleNoLimitChange}
							/>
							<Label for="no-limit" class="text-xs cursor-pointer">{m.qb_no_limit()}</Label>
						</div>
					</div>
				</div>
			</CollapsibleContent>
		</div>
	</Collapsible>
</div>
