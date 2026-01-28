<script lang="ts">
	import { Handle, Position, NodeResizer } from '@xyflow/svelte';
	import type { SubqueryRole, AggregateFunction } from '$lib/types';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte.js';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Select from '$lib/components/ui/select';
	import BracesIcon from '@lucide/svelte/icons/braces';
	import XIcon from '@lucide/svelte/icons/x';
	import FilterIcon from '@lucide/svelte/icons/filter';
	import TableIcon from '@lucide/svelte/icons/table';
	import CalculatorIcon from '@lucide/svelte/icons/calculator';

	const ROLE_ICONS: Record<SubqueryRole, typeof FilterIcon> = {
		where: FilterIcon,
		from: TableIcon,
		select: CalculatorIcon
	};

	const ROLE_LABELS: Record<SubqueryRole, string> = {
		where: 'WHERE Subquery',
		from: 'FROM (Derived Table)',
		select: 'SELECT (Scalar)'
	};

	const ROLE_COLORS: Record<SubqueryRole, string> = {
		where: 'border-indigo-500/50 bg-indigo-500/5',
		from: 'border-emerald-500/50 bg-emerald-500/5',
		select: 'border-amber-500/50 bg-amber-500/5'
	};

	const ROLE_HEADER_COLORS: Record<SubqueryRole, string> = {
		where: 'bg-indigo-500/10 border-indigo-500/30',
		from: 'bg-emerald-500/10 border-emerald-500/30',
		select: 'bg-amber-500/10 border-amber-500/30'
	};

	const ROLE_OPTIONS: { value: SubqueryRole; label: string }[] = [
		{ value: 'where', label: 'WHERE' },
		{ value: 'from', label: 'FROM' },
		{ value: 'select', label: 'SELECT' }
	];

	interface Props {
		id: string;
		data: {
			subqueryId: string;
			role: SubqueryRole;
			alias?: string;
			tableCount: number;
			hasAggregates: boolean;
			nestedSubqueryCount?: number;
			parentSubqueryId?: string;
		};
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const qb = useQueryBuilder();

	const RoleIcon = $derived(ROLE_ICONS[data.role]);
	const isNested = $derived(Boolean(data.parentSubqueryId));

	/**
	 * Recursively find a subquery by ID in the subquery tree.
	 */
	function findSubqueryById(
		sqId: string,
		subqueries: typeof qb.subqueries
	): typeof qb.subqueries[0] | undefined {
		for (const sq of subqueries) {
			if (sq.id === sqId) return sq;
			const nested = findSubqueryById(sqId, sq.innerQuery.subqueries);
			if (nested) return nested;
		}
		return undefined;
	}

	/**
	 * Find the parent subquery that contains a given subquery ID.
	 */
	function findParentSubquery(
		childId: string,
		subqueries: typeof qb.subqueries,
		parent?: typeof qb.subqueries[0]
	): typeof qb.subqueries[0] | undefined {
		for (const sq of subqueries) {
			if (sq.id === childId) return parent;
			const found = findParentSubquery(childId, sq.innerQuery.subqueries, sq);
			if (found !== undefined) return found;
		}
		return undefined;
	}

	function handleDelete() {
		// Handle deletion for both top-level and nested subqueries
		const parent = findParentSubquery(data.subqueryId, qb.subqueries);
		if (parent) {
			// Remove from parent's inner subqueries
			parent.innerQuery.subqueries = parent.innerQuery.subqueries.filter(
				(s) => s.id !== data.subqueryId
			);
			qb.subqueries = [...qb.subqueries];
		} else {
			// Top-level subquery
			qb.removeSubquery(data.subqueryId);
		}
	}

	function handleRoleChange(value: string) {
		if (value) {
			qb.updateSubqueryRole(data.subqueryId, value as SubqueryRole);
		}
	}

	function handleAliasChange(e: Event) {
		const target = e.target as HTMLInputElement;
		qb.updateSubqueryAlias(data.subqueryId, target.value);
	}
</script>

<NodeResizer
	minWidth={250}
	minHeight={150}
	isVisible={selected}
	lineStyle="border-color: var(--color-primary)"
	handleStyle="background-color: var(--color-primary); width: 8px; height: 8px;"
	onResize={(_, params) => {
		qb.updateSubquerySize(data.subqueryId, { width: params.width, height: params.height });
	}}
/>

<div
	class="rounded-lg border-2 border-dashed {ROLE_COLORS[data.role]} min-w-[250px] min-h-[150px] h-full w-full flex flex-col"
>
	<!-- Header -->
	<div
		class="flex items-center justify-between gap-2 px-3 py-2 rounded-t-md border-b {ROLE_HEADER_COLORS[data.role]}"
	>
		<div class="flex items-center gap-2 min-w-0 flex-1">
			<BracesIcon class="size-4 shrink-0 text-muted-foreground" />

			<!-- Role Selector -->
			<Select.Root
				type="single"
				value={data.role}
				onValueChange={handleRoleChange}
			>
				<Select.Trigger size="sm" class="h-6 w-20 text-xs">
					{ROLE_OPTIONS.find((r) => r.value === data.role)?.label}
				</Select.Trigger>
				<Select.Content>
					{#each ROLE_OPTIONS as role}
						<Select.Item value={role.value} label={role.label}>
							{role.label}
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>

			<!-- Alias (for FROM and SELECT subqueries) -->
			{#if data.role === 'from' || data.role === 'select'}
				<span class="text-xs text-muted-foreground">AS</span>
				<Input
					type="text"
					placeholder="alias"
					value={data.alias ?? ''}
					oninput={handleAliasChange}
					class="h-6 text-xs w-20 font-mono"
				/>
			{/if}
		</div>

		<!-- Info badges -->
		<div class="flex items-center gap-1 shrink-0">
			{#if data.tableCount > 0}
				<span class="text-xs bg-muted/50 rounded px-1.5 py-0.5 text-muted-foreground">
					{data.tableCount} {data.tableCount === 1 ? 'table' : 'tables'}
				</span>
			{/if}
			{#if data.nestedSubqueryCount && data.nestedSubqueryCount > 0}
				<span class="text-xs bg-indigo-500/20 rounded px-1.5 py-0.5 text-indigo-600 dark:text-indigo-400">
					{data.nestedSubqueryCount} nested
				</span>
			{/if}
			{#if data.hasAggregates}
				<CalculatorIcon class="size-3 text-muted-foreground" />
			{/if}
		</div>

		<!-- Delete button -->
		<Button
			variant="ghost"
			size="icon-sm"
			class="size-6 text-muted-foreground hover:text-destructive shrink-0"
			onclick={handleDelete}
		>
			<XIcon class="size-3" />
		</Button>
	</div>

	<!-- Content area - child nodes will render here -->
	<!-- pointer-events-none allows clicks to pass through to nested child nodes -->
	<div class="flex-1 p-2 relative pointer-events-none">
		{#if data.tableCount === 0}
			<div class="absolute inset-0 flex items-center justify-center text-muted-foreground text-xs pointer-events-none">
				Drag tables here to build subquery
			</div>
		{/if}
	</div>

	<!-- Output handle for connecting to outer query -->
	<Handle
		type="source"
		position={Position.Right}
		id="subquery-output"
		{isConnectable}
		class="!w-3 !h-3 !bg-primary !border-2 !border-background"
	/>
</div>
