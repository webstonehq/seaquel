<script lang="ts">
	/**
	 * Custom edge for visualizing subquery connections.
	 * Uses a dashed line with indigo color to distinguish from join edges.
	 */
	import { BaseEdge, EdgeLabel, getBezierPath, type Position } from '@xyflow/svelte';

	interface Props {
		id: string;
		sourceX: number;
		sourceY: number;
		targetX: number;
		targetY: number;
		sourcePosition: Position;
		targetPosition: Position;
		data?: {
			operator?: string;
			subqueryId?: string;
			filterId?: string;
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

	// Get operator label from data
	const operatorLabel = $derived(data?.operator ?? '');
</script>

<BaseEdge
	path={edgePath}
	{markerEnd}
	style="stroke: rgb(99 102 241); stroke-width: 2; stroke-dasharray: 5 3; {style ?? ''}"
/>

<!-- Small indicator circles at endpoints -->
<circle
	cx={sourceX}
	cy={sourceY}
	r={4}
	class="fill-indigo-500 stroke-background"
	stroke-width="2"
/>
<circle
	cx={targetX}
	cy={targetY}
	r={4}
	class="fill-indigo-500 stroke-background"
	stroke-width="2"
/>

<!-- Label showing the operator -->
<EdgeLabel x={labelX} y={labelY} class="nodrag nopan !bg-transparent">
	<div class="bg-indigo-500/90 text-white text-xs px-1.5 py-0.5 rounded font-medium shadow-sm">
		{operatorLabel || 'subquery'}
	</div>
</EdgeLabel>
