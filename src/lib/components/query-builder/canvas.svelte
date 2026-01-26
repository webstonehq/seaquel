<script lang="ts">
	import {
		SvelteFlow,
		Controls,
		Background,
		BackgroundVariant,
		type Node,
		type Edge,
		type Connection
	} from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';

	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import TableNode from './table-node.svelte';
	import JoinEdge from './join-edge.svelte';

	const qb = useQueryBuilder();

	// Custom node types
	const nodeTypes = {
		tableNode: TableNode
	};

	// Custom edge types
	const edgeTypes = {
		joinEdge: JoinEdge
	};

	// Convert state to xyflow nodes
	const nodes = $derived<Node[]>(
		qb.tables.map((table) => ({
			id: table.id,
			type: 'tableNode',
			position: table.position,
			data: {
				tableName: table.tableName,
				tableId: table.id,
				selectedColumns: table.selectedColumns
			}
		}))
	);

	// Convert joins to xyflow edges
	const edges = $derived<Edge[]>(
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

	function handleDrop(event: DragEvent) {
		event.preventDefault();
		const tableName = event.dataTransfer?.getData('application/table-name');
		if (!tableName) return;

		// Get drop position relative to the canvas
		const canvas = event.currentTarget as HTMLElement;
		const rect = canvas.getBoundingClientRect();
		const position = {
			x: event.clientX - rect.left - 100,
			y: event.clientY - rect.top - 50
		};

		qb.addTable(tableName, position);
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		if (event.dataTransfer) {
			event.dataTransfer.dropEffect = 'copy';
		}
	}
</script>

<div
	class="flex-1 h-full"
	ondrop={handleDrop}
	ondragover={handleDragOver}
	role="application"
	aria-label="Query builder canvas"
>
	<SvelteFlow
		{nodes}
		{edges}
		{nodeTypes}
		{edgeTypes}
		fitView
		onnodedragstop={handleNodeDragStop}
		onconnect={handleConnect}
		deleteKey="Delete"
	>
		<Controls />
		<Background variant={BackgroundVariant.Dots} gap={16} />
	</SvelteFlow>
</div>
