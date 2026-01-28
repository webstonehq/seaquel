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
		type OnDelete,
		type OnSelectionChange
	} from '@xyflow/svelte';
	import '@xyflow/svelte/dist/style.css';
	import { SvelteSet } from 'svelte/reactivity';

	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import TableNode from './table-node.svelte';
	import SubqueryNode from './subquery-node.svelte';
	import CteNode from './cte-node.svelte';
	import JoinEdge from './join-edge.svelte';
	import SubqueryEdge from './subquery-edge.svelte';
	import { useDnD } from './dnd-provider.svelte';

	const type = useDnD();
	const qb = useQueryBuilder();
	const { screenToFlowPosition } = useSvelteFlow();

	// Custom node types
	const nodeTypes = {
		tableNode: TableNode,
		subqueryNode: SubqueryNode,
		cteNode: CteNode
	};

	// Custom edge types
	const edgeTypes = {
		joinEdge: JoinEdge,
		subqueryEdge: SubqueryEdge
	};

	// Helper to add CTE nodes and their children
	function addCteNodes(result: Node[]) {
		for (const cte of qb.ctes) {
			// Add the CTE container node
			result.push({
				id: cte.id,
				type: 'cteNode',
				position: cte.position,
				style: `width: ${cte.size.width}px; height: ${cte.size.height}px;`,
				selected: cte.id === qb.selectedCteId,
				zIndex: 0,
				data: {
					cteId: cte.id,
					name: cte.name,
					tableCount: cte.innerQuery.tables.length,
					hasAggregates: cte.innerQuery.selectAggregates.length > 0 ||
						cte.innerQuery.tables.some((t) => t.columnAggregates.size > 0)
				}
			});

			// Add tables inside this CTE as child nodes
			for (const table of cte.innerQuery.tables) {
				result.push({
					id: table.id,
					type: 'tableNode',
					position: table.position,
					parentId: cte.id,
					extent: 'parent',
					zIndex: 5,
					data: {
						tableName: table.tableName,
						tableId: table.id,
						selectedColumns: table.selectedColumns,
						columnAggregates: table.columnAggregates,
						cteId: cte.id
					}
				});
			}
		}
	}

	// Helper to recursively add subquery nodes and their children
	// depth parameter ensures nested subqueries have higher z-index for proper click handling
	function addSubqueryNodes(
		result: Node[],
		subqueries: typeof qb.subqueries,
		parentId?: string,
		parentSubqueryId?: string,
		depth: number = 0
	) {
		for (const subquery of subqueries) {
			// Add the subquery container node
			// Higher depth = higher zIndex so nested subqueries are clickable above parents
			const nodeConfig: Node = {
				id: subquery.id,
				type: 'subqueryNode',
				position: subquery.position,
				style: `width: ${subquery.size.width}px; height: ${subquery.size.height}px;`,
				selected: subquery.id === qb.selectedSubqueryId,
				zIndex: depth * 10,
				data: {
					subqueryId: subquery.id,
					role: subquery.role,
					alias: subquery.alias,
					tableCount: subquery.innerQuery.tables.length,
					hasAggregates: subquery.innerQuery.selectAggregates.length > 0 ||
						subquery.innerQuery.tables.some((t) => t.columnAggregates.size > 0),
					nestedSubqueryCount: subquery.innerQuery.subqueries.length,
					parentSubqueryId
				}
			};

			// If this subquery is nested inside another subquery, set parentId
			if (parentId) {
				nodeConfig.parentId = parentId;
				nodeConfig.extent = 'parent';
			}

			result.push(nodeConfig);

			// Add tables inside this subquery as child nodes
			// Tables get slightly higher z-index than their parent subquery
			for (const table of subquery.innerQuery.tables) {
				result.push({
					id: table.id,
					type: 'tableNode',
					position: table.position,
					parentId: subquery.id,
					extent: 'parent',
					zIndex: depth * 10 + 5,
					data: {
						tableName: table.tableName,
						tableId: table.id,
						selectedColumns: table.selectedColumns,
						columnAggregates: table.columnAggregates,
						subqueryId: subquery.id,
						cteId: table.cteId // Pass CTE ID for CTE references inside subqueries
					}
				});
			}

			// Recursively add nested subqueries with increased depth
			if (subquery.innerQuery.subqueries.length > 0) {
				addSubqueryNodes(result, subquery.innerQuery.subqueries, subquery.id, subquery.id, depth + 1);
			}
		}
	}

	// Convert state to xyflow nodes (tables + subqueries + CTEs, recursive)
	let nodes = $derived.by<Node[]>(() => {
		const result: Node[] = [];

		// Add CTE nodes
		addCteNodes(result);

		// Add subquery nodes recursively (includes nested subqueries)
		addSubqueryNodes(result, qb.subqueries);

		// Add top-level tables
		for (const table of qb.tables) {
			result.push({
				id: table.id,
				type: 'tableNode',
				position: table.position,
				data: {
					tableName: table.tableName,
					tableId: table.id,
					selectedColumns: table.selectedColumns,
					columnAggregates: table.columnAggregates,
					cteId: table.cteId
				}
			});
		}

		return result;
	});

	// Helper to add edges for CTEs (joins inside CTE containers)
	function addCteEdges(result: Edge[]) {
		for (const cte of qb.ctes) {
			// Add joins inside this CTE
			for (const join of cte.innerQuery.joins) {
				const sourceNode = cte.innerQuery.tables.find((t) => t.tableName === join.sourceTable);
				const targetNode = cte.innerQuery.tables.find((t) => t.tableName === join.targetTable);

				result.push({
					id: join.id,
					type: 'joinEdge',
					source: sourceNode?.id ?? '',
					sourceHandle: `${join.sourceColumn}-source`,
					target: targetNode?.id ?? '',
					targetHandle: `${join.targetColumn}-target`,
					data: {
						joinId: join.id,
						joinType: join.joinType,
						cteId: cte.id
					}
				});
			}
		}
	}

	// Helper to recursively add edges for subqueries
	function addSubqueryEdges(
		result: Edge[],
		subqueries: typeof qb.subqueries,
		parentTables: typeof qb.tables,
		parentFilters: typeof qb.filters,
		parentSubqueryId?: string
	) {
		for (const subquery of subqueries) {
			// Add joins inside this subquery
			for (const join of subquery.innerQuery.joins) {
				const sourceNode = subquery.innerQuery.tables.find((t) => t.tableName === join.sourceTable);
				const targetNode = subquery.innerQuery.tables.find((t) => t.tableName === join.targetTable);

				result.push({
					id: join.id,
					type: 'joinEdge',
					source: sourceNode?.id ?? '',
					sourceHandle: `${join.sourceColumn}-source`,
					target: targetNode?.id ?? '',
					targetHandle: `${join.targetColumn}-target`,
					data: {
						joinId: join.id,
						joinType: join.joinType,
						subqueryId: subquery.id
					}
				});
			}

			// WHERE subquery connections (subquery -> filtered column in parent context)
			if (subquery.role === 'where' && subquery.linkedFilterId) {
				const filter = parentFilters.find((f) => f.id === subquery.linkedFilterId);
				if (filter && filter.column) {
					const [tableName, columnName] = filter.column.split('.');
					if (tableName && columnName) {
						const targetTable = parentTables.find((t) => t.tableName === tableName);
						if (targetTable) {
							result.push({
								id: `subquery-edge-${subquery.id}`,
								type: 'subqueryEdge',
								source: subquery.id,
								sourceHandle: 'subquery-output',
								target: targetTable.id,
								targetHandle: `${columnName}-target`,
								data: {
									operator: filter.operator,
									subqueryId: subquery.id,
									filterId: filter.id
								}
							});
						}
					}
				}
			}

			// Recursively handle nested subqueries
			if (subquery.innerQuery.subqueries.length > 0) {
				addSubqueryEdges(
					result,
					subquery.innerQuery.subqueries,
					subquery.innerQuery.tables,
					subquery.innerQuery.filters,
					subquery.id
				);
			}
		}
	}

	// Convert joins to xyflow edges (recursive for nested subqueries and CTEs)
	let edges = $derived.by<Edge[]>(() => {
		const result: Edge[] = [];

		// CTE joins
		addCteEdges(result);

		// Top-level joins
		for (const join of qb.joins) {
			const sourceNode = qb.tables.find((t) => t.tableName === join.sourceTable);
			const targetNode = qb.tables.find((t) => t.tableName === join.targetTable);

			result.push({
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
			});
		}

		// Add edges for all subqueries recursively
		addSubqueryEdges(result, qb.subqueries, qb.tables, qb.filters);

		return result;
	});

	/**
	 * Find a CTE by ID.
	 */
	function findCteById(id: string): typeof qb.ctes[0] | undefined {
		return qb.ctes.find((c) => c.id === id);
	}

	/**
	 * Recursively find a subquery by ID in the subquery tree.
	 */
	function findSubqueryById(
		id: string,
		subqueries: typeof qb.subqueries = qb.subqueries
	): typeof qb.subqueries[0] | undefined {
		for (const sq of subqueries) {
			if (sq.id === id) return sq;
			const nested = findSubqueryById(id, sq.innerQuery.subqueries);
			if (nested) return nested;
		}
		return undefined;
	}

	/**
	 * Find the parent subquery that contains a given subquery ID.
	 */
	function findParentSubquery(
		childId: string,
		subqueries: typeof qb.subqueries = qb.subqueries,
		parent?: typeof qb.subqueries[0]
	): typeof qb.subqueries[0] | undefined {
		for (const sq of subqueries) {
			if (sq.id === childId) return parent;
			const found = findParentSubquery(childId, sq.innerQuery.subqueries, sq);
			if (found !== undefined) return found;
		}
		return undefined;
	}

	// Estimated node dimensions for expansion calculations
	const TABLE_WIDTH = 220;
	const TABLE_HEIGHT = 200; // Conservative estimate for visible table height
	const SUBQUERY_MIN_WIDTH = 250;
	const SUBQUERY_MIN_HEIGHT = 150;
	const EXPANSION_PROXIMITY = 20; // Expand when within 20px of edge
	const EXPANSION_PADDING = 40; // How much to expand beyond the node
	const MAX_EXPANSION_PER_DRAG = 100; // Maximum pixels to expand in a single drag event

	/**
	 * Get the estimated dimensions of a node based on its type.
	 */
	function getNodeDimensions(node: Node): { width: number; height: number } {
		if (node.type === 'subqueryNode') {
			const subquery = findSubqueryById(node.id);
			return subquery?.size ?? { width: SUBQUERY_MIN_WIDTH, height: SUBQUERY_MIN_HEIGHT };
		}
		return { width: TABLE_WIDTH, height: TABLE_HEIGHT };
	}

	/**
	 * Check if a node is near the edge of its parent and expand if needed.
	 * Works for both tables and nested subqueries within a parent subquery.
	 * Expands in all four directions (top, left, right, bottom).
	 */
	function expandParentIfNeeded(node: Node): void {
		const parentId = node.parentId;
		if (!parentId) return;

		const parentSubquery = findSubqueryById(parentId);
		if (!parentSubquery) return;

		// Sanity check: node position should be reasonable
		if (node.position.x < -500 || node.position.y < -500 ||
			node.position.x > 5000 || node.position.y > 5000) {
			return;
		}

		const nodeDimensions = getNodeDimensions(node);
		const nodeRight = node.position.x + nodeDimensions.width;
		const nodeBottom = node.position.y + nodeDimensions.height;

		// Account for header height in subquery containers (~40px)
		const headerHeight = 40;
		const contentHeight = parentSubquery.size.height - headerHeight;

		let newWidth = parentSubquery.size.width;
		let newHeight = parentSubquery.size.height;
		let positionShiftX = 0;
		let positionShiftY = 0;

		// Check if node is near the LEFT edge (position.x close to or below 0)
		if (node.position.x < EXPANSION_PROXIMITY) {
			const expandBy = Math.min(EXPANSION_PROXIMITY - node.position.x + EXPANSION_PADDING, MAX_EXPANSION_PER_DRAG);
			if (expandBy > 0) {
				newWidth += expandBy;
				positionShiftX = expandBy; // Shift all children right
			}
		}

		// Check if node is near the TOP edge (position.y close to or below 0)
		if (node.position.y < EXPANSION_PROXIMITY) {
			const expandBy = Math.min(EXPANSION_PROXIMITY - node.position.y + EXPANSION_PADDING, MAX_EXPANSION_PER_DRAG);
			if (expandBy > 0) {
				newHeight += expandBy;
				positionShiftY = expandBy; // Shift all children down
			}
		}

		// Check if node is near the RIGHT edge
		if (nodeRight + EXPANSION_PROXIMITY > parentSubquery.size.width) {
			const desiredWidth = nodeRight + EXPANSION_PADDING;
			newWidth = Math.max(newWidth, Math.min(desiredWidth, parentSubquery.size.width + MAX_EXPANSION_PER_DRAG));
		}

		// Check if node is near the BOTTOM edge (relative to content area)
		if (nodeBottom + EXPANSION_PROXIMITY > contentHeight) {
			const desiredHeight = nodeBottom + headerHeight + EXPANSION_PADDING;
			newHeight = Math.max(newHeight, Math.min(desiredHeight, parentSubquery.size.height + MAX_EXPANSION_PER_DRAG));
		}

		// Apply changes if needed
		const sizeChanged = newWidth !== parentSubquery.size.width || newHeight !== parentSubquery.size.height;
		const needsShift = positionShiftX > 0 || positionShiftY > 0;

		if (sizeChanged || needsShift) {
			// If expanding left/top, shift the container position and adjust all children
			if (needsShift) {
				// Move container position to expand left/up
				parentSubquery.position.x -= positionShiftX;
				parentSubquery.position.y -= positionShiftY;

				// Shift all tables inside the subquery
				for (const table of parentSubquery.innerQuery.tables) {
					table.position.x += positionShiftX;
					table.position.y += positionShiftY;
				}

				// Shift all nested subqueries inside
				for (const nestedSq of parentSubquery.innerQuery.subqueries) {
					nestedSq.position.x += positionShiftX;
					nestedSq.position.y += positionShiftY;
				}
			}

			// Update size
			parentSubquery.size.width = newWidth;
			parentSubquery.size.height = newHeight;

			// Trigger reactivity
			qb.subqueries = [...qb.subqueries];
		}
	}

	/**
	 * Handle node drag events - expand parent container when near edge.
	 */
	function handleNodeDrag({
		targetNode
	}: {
		targetNode: Node | null;
		nodes: Node[];
		event: MouseEvent | TouchEvent;
	}) {
		if (!targetNode) return;

		// Only expand for nodes with a parent (tables or subqueries inside a subquery)
		if (targetNode.parentId) {
			expandParentIfNeeded(targetNode);
		}
	}

	function handleNodeDragStop({
		targetNode
	}: {
		targetNode: Node | null;
		nodes: Node[];
		event: MouseEvent | TouchEvent;
	}) {
		if (!targetNode) return;

		// Check if this is a CTE node
		const cte = findCteById(targetNode.id);
		if (cte) {
			cte.position = targetNode.position;
			qb.ctes = [...qb.ctes]; // Trigger reactivity
			return;
		}

		// Check if this is a subquery node (recursive search)
		const subquery = findSubqueryById(targetNode.id);
		if (subquery) {
			subquery.position = targetNode.position;
			qb.subqueries = [...qb.subqueries]; // Trigger reactivity
			return;
		}

		// Check if this is a table inside a CTE
		const parentId = targetNode.parentId;
		if (parentId) {
			const parentCte = findCteById(parentId);
			if (parentCte) {
				const table = parentCte.innerQuery.tables.find((t) => t.id === targetNode.id);
				if (table) {
					table.position = targetNode.position;
					qb.ctes = [...qb.ctes]; // Trigger reactivity
				}
				return;
			}

			// Check if this is a table inside a subquery
			const parentSubquery = findSubqueryById(parentId);
			if (parentSubquery) {
				const table = parentSubquery.innerQuery.tables.find((t) => t.id === targetNode.id);
				if (table) {
					table.position = targetNode.position;
					qb.subqueries = [...qb.subqueries]; // Trigger reactivity
				}
				return;
			}
		}

		// Regular top-level table
		qb.updateTablePosition(targetNode.id, targetNode.position);
	}

	/**
	 * Find a table node by ID in either top-level tables, CTE tables, or subquery tables (recursive).
	 */
	function findTableNode(
		id: string,
		subqueries: typeof qb.subqueries = qb.subqueries
	): { tableName: string; subqueryId?: string; cteId?: string } | undefined {
		// Check top-level tables first (only on initial call)
		if (subqueries === qb.subqueries) {
			const table = qb.tables.find((t) => t.id === id);
			if (table) {
				return { tableName: table.tableName };
			}

			// Check tables in CTEs
			for (const cte of qb.ctes) {
				const cteTable = cte.innerQuery.tables.find((t) => t.id === id);
				if (cteTable) {
					return { tableName: cteTable.tableName, cteId: cte.id };
				}
			}
		}

		// Check tables in subqueries recursively
		for (const subquery of subqueries) {
			const sqTable = subquery.innerQuery.tables.find((t) => t.id === id);
			if (sqTable) {
				return { tableName: sqTable.tableName, subqueryId: subquery.id };
			}
			// Check nested subqueries
			const nested = findTableNode(id, subquery.innerQuery.subqueries);
			if (nested) return nested;
		}

		return undefined;
	}

	function handleConnect(connection: Connection) {
		const { source, sourceHandle, target, targetHandle } = connection;
		if (!source || !target || !sourceHandle || !targetHandle) return;

		// Check if connecting within a CTE or subquery
		const sourceNode = findTableNode(source);
		const targetNode = findTableNode(target);
		if (!sourceNode || !targetNode) return;

		// Extract column names from handles
		const sourceColumn = sourceHandle.replace('-source', '');
		const targetColumn = targetHandle.replace('-target', '');

		// If both are in the same CTE, add join to that CTE
		if (sourceNode.cteId && sourceNode.cteId === targetNode.cteId) {
			const cte = findCteById(sourceNode.cteId);
			if (cte) {
				const join = {
					id: crypto.randomUUID(),
					sourceTable: sourceNode.tableName,
					sourceColumn,
					targetTable: targetNode.tableName,
					targetColumn,
					joinType: 'INNER' as const
				};
				cte.innerQuery.joins = [...cte.innerQuery.joins, join];
				qb.ctes = [...qb.ctes];
			}
			return;
		}

		// If both are in the same subquery, add join to that subquery
		if (sourceNode.subqueryId && sourceNode.subqueryId === targetNode.subqueryId) {
			const subquery = findSubqueryById(sourceNode.subqueryId);
			if (subquery) {
				const join = {
					id: crypto.randomUUID(),
					sourceTable: sourceNode.tableName,
					sourceColumn,
					targetTable: targetNode.tableName,
					targetColumn,
					joinType: 'INNER' as const
				};
				subquery.innerQuery.joins = [...subquery.innerQuery.joins, join];
				qb.subqueries = [...qb.subqueries];
			}
			return;
		}

		// Top-level join
		qb.addJoin(sourceNode.tableName, sourceColumn, targetNode.tableName, targetColumn, 'INNER');
	}

	function handleDragOver(event: DragEvent) {
		event.preventDefault();
		if (event.dataTransfer) {
			event.dataTransfer.dropEffect = 'move';
		}
	}

	/**
	 * Calculate the absolute position of a subquery on the canvas (recursive).
	 */
	function getSubqueryAbsolutePosition(
		subqueryId: string,
		subqueries: typeof qb.subqueries = qb.subqueries,
		offset: { x: number; y: number } = { x: 0, y: 0 }
	): { x: number; y: number } | null {
		for (const sq of subqueries) {
			if (sq.id === subqueryId) {
				return {
					x: sq.position.x + offset.x,
					y: sq.position.y + offset.y
				};
			}
			const nested = getSubqueryAbsolutePosition(
				subqueryId,
				sq.innerQuery.subqueries,
				{ x: sq.position.x + offset.x, y: sq.position.y + offset.y + 40 } // +40 for header
			);
			if (nested) return nested;
		}
		return null;
	}

	function handleDrop(event: DragEvent) {
		event.preventDefault();

		const dragType = type.current;
		if (!dragType) return;

		// Convert screen coordinates to flow coordinates
		const position = screenToFlowPosition({
			x: event.clientX,
			y: event.clientY
		});

		// Check if dropping on a CTE or subquery
		const droppedOnCte = findCteAtPosition(position);
		const droppedOnSubquery = findSubqueryAtPosition(position);

		// Handle new CTE creation
		if (dragType === '__cte__') {
			position.x -= 150;
			position.y -= 100;
			qb.addCte('', position);
			return;
		}

		// Handle CTE reference (dragging CTE from palette)
		if (dragType.startsWith('__cte__')) {
			const cteId = dragType.replace('__cte__', '');

			// Check if dropping on a subquery - CTEs are accessible within subqueries
			if (droppedOnSubquery) {
				const absPos = getSubqueryAbsolutePosition(droppedOnSubquery.id);
				const relativePosition = {
					x: position.x - (absPos?.x ?? 0) - 10,
					y: position.y - (absPos?.y ?? 0) - 40
				};
				qb.addCteReferenceToSubquery(droppedOnSubquery.id, cteId, relativePosition);
				return;
			}

			// Top-level CTE reference
			position.x -= 110;
			position.y -= 20;
			qb.addCteReference(cteId, position);
			return;
		}

		if (dragType === '__subquery__') {
			// Creating a new subquery
			if (droppedOnSubquery) {
				// Add nested subquery inside the target subquery
				const absPos = getSubqueryAbsolutePosition(droppedOnSubquery.id);
				const relativePosition = {
					x: position.x - (absPos?.x ?? 0) - 10,
					y: position.y - (absPos?.y ?? 0) - 40 - 50 // Header offset + some padding
				};
				// Add to the inner subqueries of the target
				const newSubquery = {
					id: crypto.randomUUID(),
					position: relativePosition,
					size: { width: 300, height: 200 },
					role: 'where' as const,
					innerQuery: {
						tables: [],
						joins: [],
						filters: [],
						groupBy: [],
						having: [],
						orderBy: [],
						limit: null,
						selectAggregates: [],
						subqueries: []
					}
				};
				droppedOnSubquery.innerQuery.subqueries = [
					...droppedOnSubquery.innerQuery.subqueries,
					newSubquery
				];
				qb.subqueries = [...qb.subqueries];
			} else {
				// Add top-level subquery
				position.x -= 150;
				position.y -= 100;
				qb.addSubquery('from', position);
			}
		} else if (droppedOnCte) {
			// Adding table to CTE
			const relativePosition = {
				x: position.x - droppedOnCte.position.x - 10,
				y: position.y - droppedOnCte.position.y - 40 // Account for header
			};
			const tableSchema = qb.getSchemaTable(dragType);
			if (tableSchema) {
				const canvasTable = {
					id: crypto.randomUUID(),
					tableName: dragType,
					position: relativePosition,
					selectedColumns: new SvelteSet<string>(),
					columnAggregates: new Map()
				};
				droppedOnCte.innerQuery.tables = [
					...droppedOnCte.innerQuery.tables,
					canvasTable
				];
				qb.ctes = [...qb.ctes];
			}
		} else if (droppedOnSubquery) {
			// Adding table to subquery (works with nested subqueries via findSubqueryById)
			const absPos = getSubqueryAbsolutePosition(droppedOnSubquery.id);
			const relativePosition = {
				x: position.x - (absPos?.x ?? 0) - 10,
				y: position.y - (absPos?.y ?? 0) - 40
			};
			// Add table directly to the found subquery
			const tableSchema = qb.getSchemaTable(dragType);
			if (tableSchema) {
				const canvasTable = {
					id: crypto.randomUUID(),
					tableName: dragType,
					position: relativePosition,
					selectedColumns: new SvelteSet<string>(),
					columnAggregates: new Map()
				};
				droppedOnSubquery.innerQuery.tables = [
					...droppedOnSubquery.innerQuery.tables,
					canvasTable
				];
				qb.subqueries = [...qb.subqueries];
			}
		} else {
			// Adding table to top level
			position.x -= 110;
			position.y -= 20;
			qb.addTable(dragType, position);
		}
	}

	/**
	 * Find if a position is within a CTE's bounds.
	 */
	function findCteAtPosition(position: { x: number; y: number }): typeof qb.ctes[0] | null {
		for (const cte of qb.ctes) {
			const { width, height } = cte.size;

			if (
				position.x >= cte.position.x &&
				position.x <= cte.position.x + width &&
				position.y >= cte.position.y &&
				position.y <= cte.position.y + height
			) {
				return cte;
			}
		}
		return null;
	}

	/**
	 * Find if a position is within a subquery's bounds (recursive, prioritizes innermost).
	 * For nested subqueries, converts position to be relative to parent.
	 */
	function findSubqueryAtPosition(
		position: { x: number; y: number },
		subqueries: typeof qb.subqueries = qb.subqueries,
		parentOffset: { x: number; y: number } = { x: 0, y: 0 }
	): typeof qb.subqueries[0] | null {
		for (const subquery of subqueries) {
			const absX = subquery.position.x + parentOffset.x;
			const absY = subquery.position.y + parentOffset.y;
			const { width, height } = subquery.size;

			if (
				position.x >= absX &&
				position.x <= absX + width &&
				position.y >= absY &&
				position.y <= absY + height
			) {
				// Check for nested subqueries first (innermost takes priority)
				const nested = findSubqueryAtPosition(
					position,
					subquery.innerQuery.subqueries,
					{ x: absX, y: absY + 40 } // Account for header offset
				);
				if (nested) return nested;
				return subquery;
			}
		}
		return null;
	}

	/**
	 * Remove a subquery by ID from the subquery tree (recursive).
	 */
	function removeSubqueryFromTree(
		id: string,
		subqueries: typeof qb.subqueries
	): boolean {
		const idx = subqueries.findIndex((s) => s.id === id);
		if (idx !== -1) {
			subqueries.splice(idx, 1);
			return true;
		}
		for (const sq of subqueries) {
			if (removeSubqueryFromTree(id, sq.innerQuery.subqueries)) {
				return true;
			}
		}
		return false;
	}

	const handleDelete: OnDelete = ({ nodes: deletedNodes, edges: deletedEdges }) => {
		// Remove edges (joins) from query builder
		for (const edge of deletedEdges) {
			const edgeData = edge.data as { subqueryId?: string; cteId?: string } | undefined;
			const cteId = edgeData?.cteId;
			const subqueryId = edgeData?.subqueryId;

			if (cteId) {
				// Remove join from CTE
				const cte = findCteById(cteId);
				if (cte) {
					cte.innerQuery.joins = cte.innerQuery.joins.filter((j) => j.id !== edge.id);
					qb.ctes = [...qb.ctes];
				}
			} else if (subqueryId) {
				// Remove join from subquery (recursive search)
				const subquery = findSubqueryById(subqueryId);
				if (subquery) {
					subquery.innerQuery.joins = subquery.innerQuery.joins.filter((j) => j.id !== edge.id);
					qb.subqueries = [...qb.subqueries];
				}
			} else {
				qb.removeJoin(edge.id);
			}
		}

		// Remove nodes
		for (const node of deletedNodes) {
			// Check if it's a CTE
			const cte = findCteById(node.id);
			if (cte) {
				qb.removeCte(node.id);
				continue;
			}

			// Check if it's a subquery (recursive search)
			const subquery = findSubqueryById(node.id);
			if (subquery) {
				// Check if it's a top-level subquery or nested
				const parent = findParentSubquery(node.id);
				if (parent) {
					// Remove from parent's inner subqueries
					parent.innerQuery.subqueries = parent.innerQuery.subqueries.filter(
						(s) => s.id !== node.id
					);
				} else {
					// Remove from top-level
					qb.subqueries = qb.subqueries.filter((s) => s.id !== node.id);
				}
				qb.subqueries = [...qb.subqueries]; // Trigger reactivity
				continue;
			}

			// Check if it's a table in a CTE or subquery
			const parentId = node.parentId;
			if (parentId) {
				const parentCte = findCteById(parentId);
				if (parentCte) {
					parentCte.innerQuery.tables = parentCte.innerQuery.tables.filter(
						(t) => t.id !== node.id
					);
					qb.ctes = [...qb.ctes];
					continue;
				}

				const parentSubquery = findSubqueryById(parentId);
				if (parentSubquery) {
					parentSubquery.innerQuery.tables = parentSubquery.innerQuery.tables.filter(
						(t) => t.id !== node.id
					);
					qb.subqueries = [...qb.subqueries];
					continue;
				}
			}

			// Regular table
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

	// Track pending selection clear to handle state-update vs user-initiated selection changes
	let pendingClear: ReturnType<typeof setTimeout> | null = null;

	// Handle selection changes to track selected subquery or CTE
	const handleSelectionChange: OnSelectionChange = ({ nodes: selectedNodes }) => {
		// Cancel any pending clear - a new selection event came in
		if (pendingClear !== null) {
			clearTimeout(pendingClear);
			pendingClear = null;
		}

		// Check if a subquery is in the selection
		const selectedSubquery = selectedNodes.find(
			(node: Node) => node.type === 'subqueryNode'
		);

		// Check if a CTE is in the selection
		const selectedCte = selectedNodes.find(
			(node: Node) => node.type === 'cteNode'
		);

		if (selectedSubquery) {
			qb.selectedSubqueryId = selectedSubquery.id;
			qb.selectedCteId = null;
		} else if (selectedCte) {
			qb.selectedCteId = (selectedCte.data as { cteId: string }).cteId;
			qb.selectedSubqueryId = null;
		} else if (selectedNodes.length > 0) {
			// Selected a non-subquery/non-CTE node (table), clear immediately
			qb.selectedSubqueryId = null;
			qb.selectedCteId = null;
		} else {
			// Empty selection - defer clearing to next frame
			// This allows state updates to settle without clearing selection
			// If it's a real user click on empty canvas, the clear will happen
			// If it's a state update, SvelteFlow will restore selection and cancel this
			pendingClear = setTimeout(() => {
				pendingClear = null;
				qb.selectedSubqueryId = null;
				qb.selectedCteId = null;
			}, 0);
		}
	};
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
		onnodedrag={handleNodeDrag}
		onnodedragstop={handleNodeDragStop}
		ondrop={handleDrop}
		ondragover={handleDragOver}
		onconnect={handleConnect}
		ondelete={handleDelete}
		onselectionchange={handleSelectionChange}
		deleteKey={['Delete', 'Backspace']}
	>
		<Controls />
		<Background variant={BackgroundVariant.Dots} gap={16} />
	</SvelteFlow>
</div>
