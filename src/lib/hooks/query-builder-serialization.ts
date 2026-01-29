/**
 * Serialization/deserialization for query builder state.
 * Converts between runtime types (with Set/Map) and plain JSON-safe types.
 */

import { SvelteSet } from 'svelte/reactivity';
import type {
	CanvasTable,
	CanvasJoin,
	FilterCondition,
	GroupByCondition,
	HavingCondition,
	SortCondition,
	SelectAggregate,
	CanvasSubquery,
	SubqueryInnerState,
	CanvasCTE,
	ColumnAggregate,
	SubqueryRole
} from '$lib/types';

// === SERIALIZABLE INTERFACES ===

/**
 * Serializable table for persistence.
 */
export interface SerializableTable {
	id: string;
	tableName: string;
	position: { x: number; y: number };
	selectedColumns: string[];
	columnAggregates?: Record<string, ColumnAggregate>;
	/** If set, this table references a CTE instead of schema table */
	cteId?: string;
}

/**
 * Serializable inner query for subqueries.
 */
export interface SerializableInnerQuery {
	tables: SerializableTable[];
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy: GroupByCondition[];
	having: HavingCondition[];
	orderBy: SortCondition[];
	limit: string | number | null;
	selectAggregates: SelectAggregate[];
	subqueries?: SerializableSubquery[];
}

/**
 * Serializable subquery for persistence.
 */
export interface SerializableSubquery {
	id: string;
	position: { x: number; y: number };
	size: { width: number; height: number };
	role: SubqueryRole;
	linkedFilterId?: string;
	alias?: string;
	innerQuery: SerializableInnerQuery;
}

/**
 * Serializable CTE for persistence.
 */
export interface SerializableCTE {
	id: string;
	name: string;
	position: { x: number; y: number };
	size: { width: number; height: number };
	innerQuery: SerializableInnerQuery;
}

/**
 * Serializable version of query builder state for persistence.
 */
export interface SerializableQueryBuilderState {
	tables: SerializableTable[];
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy?: GroupByCondition[];
	having?: HavingCondition[];
	orderBy: SortCondition[];
	limit: string | number | null;
	/** User's custom SQL text (preserved even if it differs from generated) */
	customSql?: string | null;
	/** Standalone aggregates in SELECT clause */
	selectAggregates?: SelectAggregate[];
	/** Subqueries on the canvas */
	subqueries?: SerializableSubquery[];
	/** CTEs (Common Table Expressions) on the canvas */
	ctes?: SerializableCTE[];
}

// === SERIALIZE HELPERS ===

/**
 * Serialize subqueries recursively.
 */
function serializeSubqueries(subqueries: CanvasSubquery[]): SerializableSubquery[] {
	return subqueries.map((sq) => ({
		id: sq.id,
		position: sq.position,
		size: sq.size,
		role: sq.role,
		linkedFilterId: sq.linkedFilterId,
		alias: sq.alias,
		innerQuery: serializeInnerQuery(sq.innerQuery)
	}));
}

/**
 * Serialize CTEs.
 */
function serializeCtes(ctes: CanvasCTE[]): SerializableCTE[] {
	return ctes.map((cte) => ({
		id: cte.id,
		name: cte.name,
		position: cte.position,
		size: cte.size,
		innerQuery: serializeInnerQuery(cte.innerQuery)
	}));
}

/**
 * Serialize inner query state.
 */
function serializeInnerQuery(inner: SubqueryInnerState): SerializableInnerQuery {
	return {
		tables: inner.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: Array.from(t.selectedColumns),
			columnAggregates: Object.fromEntries(t.columnAggregates)
		})),
		joins: [...inner.joins],
		filters: [...inner.filters],
		groupBy: [...inner.groupBy],
		having: [...inner.having],
		orderBy: [...inner.orderBy],
		limit: inner.limit,
		selectAggregates: [...inner.selectAggregates],
		subqueries: serializeSubqueries(inner.subqueries)
	};
}

// === DESERIALIZE HELPERS ===

/**
 * Deserialize subqueries recursively.
 */
function deserializeSubqueries(subqueries: SerializableSubquery[]): CanvasSubquery[] {
	return subqueries.map((sq) => ({
		id: sq.id,
		position: sq.position,
		size: sq.size,
		role: sq.role,
		linkedFilterId: sq.linkedFilterId,
		alias: sq.alias,
		innerQuery: deserializeInnerQuery(sq.innerQuery)
	}));
}

/**
 * Deserialize CTEs.
 */
function deserializeCtes(ctes: SerializableCTE[]): CanvasCTE[] {
	return ctes.map((cte) => ({
		id: cte.id,
		name: cte.name,
		position: cte.position,
		size: cte.size,
		innerQuery: deserializeInnerQuery(cte.innerQuery)
	}));
}

/**
 * Deserialize inner query state.
 */
function deserializeInnerQuery(inner: SerializableInnerQuery): SubqueryInnerState {
	return {
		tables: inner.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: new SvelteSet(t.selectedColumns),
			columnAggregates: new Map(
				Object.entries(t.columnAggregates ?? {}) as [string, ColumnAggregate][]
			)
		})),
		joins: inner.joins.map((j) => ({ ...j })),
		filters: inner.filters.map((f) => ({ ...f })),
		groupBy: inner.groupBy.map((g) => ({ ...g })),
		having: inner.having.map((h) => ({ ...h })),
		orderBy: inner.orderBy.map((o) => ({ ...o })),
		limit: inner.limit,
		selectAggregates: inner.selectAggregates.map((a) => ({ ...a })),
		subqueries: deserializeSubqueries(inner.subqueries ?? [])
	};
}

// === PUBLIC API ===

/**
 * Input shape for serialization — matches the relevant fields of QueryBuilderState.
 */
interface SerializableInput {
	tables: CanvasTable[];
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy: GroupByCondition[];
	having: HavingCondition[];
	orderBy: SortCondition[];
	limit: string | number | null;
	customSql: string | null;
	selectAggregates: SelectAggregate[];
	subqueries: CanvasSubquery[];
	ctes: CanvasCTE[];
}

/**
 * Deserialized runtime state returned from deserialization.
 */
export interface DeserializedQueryBuilderState {
	tables: CanvasTable[];
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy: GroupByCondition[];
	having: HavingCondition[];
	orderBy: SortCondition[];
	limit: string | number | null;
	customSql: string | null;
	selectAggregates: SelectAggregate[];
	subqueries: CanvasSubquery[];
	ctes: CanvasCTE[];
}

/**
 * Convert runtime query builder state to a JSON-safe serializable format.
 * Converts Sets to arrays and Maps to plain objects.
 */
export function serializeQueryBuilderState(state: SerializableInput): SerializableQueryBuilderState {
	return {
		tables: state.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: Array.from(t.selectedColumns),
			columnAggregates: Object.fromEntries(t.columnAggregates),
			cteId: t.cteId
		})),
		joins: [...state.joins],
		filters: [...state.filters],
		groupBy: [...state.groupBy],
		having: [...state.having],
		orderBy: [...state.orderBy],
		limit: state.limit,
		customSql: state.customSql,
		selectAggregates: [...state.selectAggregates],
		subqueries: serializeSubqueries(state.subqueries),
		ctes: serializeCtes(state.ctes)
	};
}

/**
 * Restore runtime query builder state from a serialized snapshot.
 * Converts arrays back to Sets and plain objects back to Maps.
 */
export function deserializeQueryBuilderState(
	state: SerializableQueryBuilderState
): DeserializedQueryBuilderState {
	return {
		tables: state.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: new SvelteSet(t.selectedColumns),
			columnAggregates: new Map(
				Object.entries(t.columnAggregates ?? {}) as [string, ColumnAggregate][]
			),
			cteId: t.cteId
		})),
		joins: state.joins.map((j) => ({ ...j })),
		filters: state.filters.map((f) => ({ ...f })),
		groupBy: (state.groupBy ?? []).map((g) => ({ ...g })),
		having: (state.having ?? []).map((h) => ({ ...h })),
		orderBy: state.orderBy.map((o) => ({ ...o })),
		limit: state.limit,
		customSql: state.customSql ?? null,
		selectAggregates: (state.selectAggregates ?? []).map((a) => ({ ...a })),
		subqueries: deserializeSubqueries(state.subqueries ?? []),
		ctes: deserializeCtes(state.ctes ?? [])
	};
}
