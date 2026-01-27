<script lang="ts">
	import {
		SvelteFlow,
		Controls,
		Background,
		BackgroundVariant,
		useSvelteFlow,
		type Node,
		type Edge,
		type Connection,
		type OnDelete
	} from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';

	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import TableNode from './table-node.svelte';
	import JoinEdge from './join-edge.svelte';
	import { useDnD } from './dnd-provider.svelte';

	const type = useDnD();
	const qb = useQueryBuilder();
	const { screenToFlowPosition } = useSvelteFlow();

	// Custom node types
	const nodeTypes = {
		tableNode: TableNode
	};

	// Custom edge types
	const edgeTypes = {
		joinEdge: JoinEdge
	};

	// Convert state to xyflow nodes
	let nodes = $derived<Node[]>(
		qb.tables.map((table) => ({
			id: table.id,
			type: 'tableNode',
			position: table.position,
			data: {
				tableName: table.tableName,
				tableId: table.id,
				selectedColumns: table.selectedColumns,
				columnAggregates: table.columnAggregates
			}
		}))
	);

	// Convert joins to xyflow edges
	let edges = $derived<Edge[]>(
		qb.joins.map((join) => {
			const sourceNode = qb.tables.find((t) => t.tableName === join.sourceTable);
			const targetNode = qb.tables.find((t) => t.tableName === join.targetTable);

			return {
				id: join.id,
				type: 'joinEdge',
				source: sourceNode?.id ?? '',
				sourceHandle: `${join.sourceColumn}-source`,
				target: targetNode?.id ?? '',
				targetHandle: `${join.targetColumn}-target`,
				data: {
					joinId: join.id,
					joinType: join.joinType
				}
			};
		})
	);

	function handleNodeDragStop({
		targetNode
	}: {
		targetNode: Node | null;
		nodes: Node[];
		event: MouseEvent | TouchEvent;
	}) {
		if (targetNode) {
			qb.updateTablePosition(targetNode.id, targetNode.position);
		}
	}

	function handleConnect(connection: Connection) {
		const { source, sourceHandle, target, targetHandle } = connection;
		if (!source || !target || !sourceHandle || !targetHandle) return;

		const sourceNode = qb.tables.find((t) => t.id === source);
		const targetNode = qb.tables.find((t) => t.id === target);
		if (!sourceNode || !targetNode) return;

		// Extract column names from handles
		const sourceColumn = sourceHandle.replace('-source', '');
		const targetColumn = targetHandle.replace('-target', '');

		qb.addJoin(sourceNode.tableName, sourceColumn, targetNode.tableName, targetColumn, 'INNER');
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		if (event.dataTransfer) {
			event.dataTransfer.dropEffect = 'move';
		}
	}

	function handleDrop(event: DragEvent) {
		event.preventDefault();

		const tableName = type.current;
		if (!tableName) return;

		// Convert screen coordinates to flow coordinates
		const position = screenToFlowPosition({
			x: event.clientX,
			y: event.clientY
		});

		// Offset to roughly center the node on cursor
		position.x -= 110;
		position.y -= 20;

		qb.addTable(tableName, position);
	}

	const handleDelete: OnDelete = ({ nodes: deletedNodes, edges: deletedEdges }) => {
		// Remove edges (joins) from query builder
		for (const edge of deletedEdges) {
			qb.removeJoin(edge.id);
		}

		// Remove nodes (tables) from query builder
		for (const node of deletedNodes) {
			qb.removeTable(node.id);
		}
	};

	// Prevent Delete/Backspace from triggering browser navigation
	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Delete' || event.key === 'Backspace') {
			// Prevent browser back navigation
			event.preventDefault();
		}
	}
</script>

<div
	class="flex-1 h-full"
	aria-label="Query builder canvas"
	role="application"
	tabindex="-1"
	onkeydown={handleKeydown}
>
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		{edgeTypes}
		fitView
		onnodedragstop={handleNodeDragStop}
		ondrop={handleDrop}
		ondragover={handleDragOver}
		onconnect={handleConnect}
		ondelete={handleDelete}
		deleteKey={['Delete', 'Backspace']}
	>
		<Controls />
		<Background variant={BackgroundVariant.Dots} gap={16} />
	</SvelteFlow>
</div>
