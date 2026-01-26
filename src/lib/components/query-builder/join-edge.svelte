<script lang="ts">
	import { BaseEdge, EdgeLabel, getBezierPath, type Position } from "@xyflow/svelte";
	import type { JoinType } from "$lib/types";
	import { useQueryBuilder } from "$lib/hooks/query-builder.svelte.js";
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";

	interface Props {
		id: string;
		sourceX: number;
		sourceY: number;
		targetX: number;
		targetY: number;
		sourcePosition: Position;
		targetPosition: Position;
		data?: {
			joinId: string;
			joinType: JoinType;
		};
		markerEnd?: string;
		style?: string;
	}

	let {
		id,
		sourceX,
		sourceY,
		targetX,
		targetY,
		sourcePosition,
		targetPosition,
		data,
		markerEnd,
		style
	}: Props = $props();

	const queryBuilder = useQueryBuilder();

	// Join type options with descriptions
	const joinOptions: Array<{ type: JoinType; label: string; description: string }> = [
		{ type: "INNER", label: "INNER JOIN", description: "Only matching rows from both tables" },
		{ type: "LEFT", label: "LEFT JOIN", description: "All rows from left table, matching from right" },
		{ type: "RIGHT", label: "RIGHT JOIN", description: "All rows from right table, matching from left" },
		{ type: "FULL", label: "FULL OUTER JOIN", description: "All rows from both tables" }
	];

	// Compute the bezier path and label position
	const pathData = $derived(
		getBezierPath({
			sourceX,
			sourceY,
			sourcePosition,
			targetX,
			targetY,
			targetPosition
		})
	);

	const edgePath = $derived(pathData[0]);
	const labelX = $derived(pathData[1]);
	const labelY = $derived(pathData[2]);

	// Current join type
	const joinType = $derived(data?.joinType ?? "INNER");
	const joinLabel = $derived(
		joinOptions.find((opt) => opt.type === joinType)?.label ?? "INNER JOIN"
	);

	// Dashed style for non-INNER joins
	const isDashed = $derived(joinType !== "INNER");
	const edgeStyle = $derived(
		isDashed ? `${style ?? ""}; stroke-dasharray: 5,5` : style
	);

	function handleJoinTypeChange(newType: JoinType) {
		if (data?.joinId) {
			queryBuilder.updateJoinType(data.joinId, newType);
		}
	}
</script>

<BaseEdge path={edgePath} {markerEnd} style={edgeStyle} />

<EdgeLabel x={labelX} y={labelY} class="nodrag nopan pointer-events-auto">
	<DropdownMenu.Root>
		<DropdownMenu.Trigger>
			{#snippet child({ props })}
				<Button
					{...props}
					variant="outline"
					size="sm"
					class="h-6 px-2 text-xs bg-background shadow-sm"
				>
					{joinLabel}
				</Button>
			{/snippet}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="center">
			{#each joinOptions as option (option.type)}
				<DropdownMenu.Item
					onclick={() => handleJoinTypeChange(option.type)}
					class={option.type === joinType ? "bg-accent" : ""}
				>
					<div class="flex flex-col gap-0.5">
						<span class="font-medium">{option.label}</span>
						<span class="text-xs text-muted-foreground">{option.description}</span>
					</div>
				</DropdownMenu.Item>
			{/each}
		</DropdownMenu.Content>
	</DropdownMenu.Root>
</EdgeLabel>
