/**
 * Pure functions for applying parsed SQL AST back to query builder state.
 * Handles the SQL-to-state sync (two-way binding between SQL editor and canvas).
 */

import { SvelteSet } from "svelte/reactivity";
import type {
  CanvasTable,
  CanvasJoin,
  FilterCondition,
  GroupByCondition,
  HavingCondition,
  SortCondition,
  SelectAggregate,
  CanvasSubquery,
  CanvasCTE,
  ColumnAggregate,
  QueryBuilderTable,
} from "$lib/types";
import type { ParsedQuery, ParsedSubquery } from "$lib/sql";

/**
 * Generates a unique ID for canvas elements.
 */
function generateId(): string {
  return crypto.randomUUID();
}

/** Result of applying parsed SQL to state */
export interface AppliedParsedSqlState {
  tables: CanvasTable[];
  joins: CanvasJoin[];
  filters: FilterCondition[];
  groupBy: GroupByCondition[];
  having: HavingCondition[];
  orderBy: SortCondition[];
  limit: number | null;
  selectAggregates: SelectAggregate[];
  subqueries: CanvasSubquery[];
  ctes: CanvasCTE[];
}

// === PURE FUNCTIONS ===

/**
 * Expand selectedColumns, converting '*' to all column names from the schema.
 */
export function expandSelectedColumns(
  schema: QueryBuilderTable[],
  tableName: string,
  selectedColumns: string[],
): string[] {
  if (selectedColumns.includes("*")) {
    const schemaTable = schema.find((t) => t.name === tableName);
    if (schemaTable) {
      return schemaTable.columns.map((c) => c.name);
    }
  }
  return selectedColumns;
}

/**
 * Recursively build subqueries from parsed AST.
 */
export function buildSubqueriesFromParsed(
  schema: QueryBuilderTable[],
  parsedSubqueries: ParsedSubquery[],
  existingSubqueries: CanvasSubquery[],
  basePosition: { x: number; y: number } = { x: 50, y: 50 },
  cteNameToId: Map<string, string> = new Map(),
): { subqueries: CanvasSubquery[]; subqueryIdMap: Map<number, string> } {
  const result: CanvasSubquery[] = [];
  const subqueryIdMap = new Map<number, string>();

  for (let i = 0; i < parsedSubqueries.length; i++) {
    const ps = parsedSubqueries[i];
    const existingSubquery = existingSubqueries[i];

    const position = existingSubquery?.position ?? {
      x: basePosition.x + i * 350,
      y: basePosition.y,
    };

    const subqueryId = existingSubquery?.id ?? generateId();
    subqueryIdMap.set(i, subqueryId);

    // Build inner query tables
    const innerTables: CanvasTable[] = [];
    for (let j = 0; j < ps.innerQuery.tables.length; j++) {
      const pt = ps.innerQuery.tables[j];

      const columnAggsForTable = new Map<string, ColumnAggregate>();
      if (ps.innerQuery.columnAggregates) {
        for (const ca of ps.innerQuery.columnAggregates) {
          if (ca.tableName === pt.tableName) {
            columnAggsForTable.set(ca.column, {
              function: ca.function,
              alias: ca.alias,
            });
          }
        }
      }

      // Check if this table references a CTE
      const cteId = cteNameToId.get(pt.tableName);

      innerTables.push({
        id: generateId(),
        tableName: pt.tableName,
        position: { x: 20 + j * 240, y: 50 },
        selectedColumns: new SvelteSet(
          expandSelectedColumns(schema, pt.tableName, pt.selectedColumns),
        ),
        columnAggregates: columnAggsForTable,
        cteId,
      });
    }

    // Build inner query joins
    const innerJoins: CanvasJoin[] = ps.innerQuery.joins.map((pj) => ({
      id: generateId(),
      sourceTable: pj.sourceTable,
      sourceColumn: pj.sourceColumn,
      targetTable: pj.targetTable,
      targetColumn: pj.targetColumn,
      joinType: pj.joinType,
    }));

    // Recursively build nested subqueries
    const nestedResult =
      ps.innerQuery.subqueries && ps.innerQuery.subqueries.length > 0
        ? buildSubqueriesFromParsed(
            schema,
            ps.innerQuery.subqueries,
            existingSubquery?.innerQuery.subqueries ?? [],
            { x: 50, y: 50 },
            cteNameToId,
          )
        : { subqueries: [], subqueryIdMap: new Map<number, string>() };

    // Build inner filters with nested subquery links
    const innerFilters: FilterCondition[] = ps.innerQuery.filters.map((f) => {
      const filter: FilterCondition = {
        id: generateId(),
        column: f.column,
        operator: f.operator,
        value: f.value,
        connector: f.connector,
      };
      if (f.subqueryIndex !== undefined) {
        const nestedSubqueryId = nestedResult.subqueryIdMap.get(f.subqueryIndex);
        if (nestedSubqueryId) {
          filter.subqueryId = nestedSubqueryId;
          const nestedSubquery = nestedResult.subqueries.find((s) => s.id === nestedSubqueryId);
          if (nestedSubquery) {
            nestedSubquery.linkedFilterId = filter.id;
          }
        }
      }
      return filter;
    });

    const subqueryWidth = Math.max(300, 20 + ps.innerQuery.tables.length * 240 + 20);
    const subqueryHeight = Math.max(200, 350);

    result.push({
      id: subqueryId,
      position,
      size: existingSubquery?.size ?? { width: subqueryWidth, height: subqueryHeight },
      role: ps.role,
      innerQuery: {
        tables: innerTables,
        joins: innerJoins,
        filters: innerFilters,
        groupBy: ps.innerQuery.groupBy.map((g) => ({
          id: generateId(),
          column: g.column,
        })),
        having: ps.innerQuery.having.map((h) => ({
          id: generateId(),
          aggregateFunction: h.aggregateFunction,
          column: h.column,
          operator: h.operator,
          value: h.value,
          connector: h.connector,
        })),
        orderBy: ps.innerQuery.orderBy.map((o) => ({
          id: generateId(),
          column: o.column,
          direction: o.direction,
        })),
        limit: ps.innerQuery.limit,
        selectAggregates: ps.innerQuery.selectAggregates.map((a) => ({
          id: generateId(),
          function: a.function,
          expression: a.expression,
          alias: a.alias,
        })),
        subqueries: nestedResult.subqueries,
      },
    });
  }

  return { subqueries: result, subqueryIdMap };
}

/**
 * Apply parsed SQL to produce new state.
 * Returns all new state arrays; the caller assigns them to $state properties.
 */
export function applyParsedSqlToState(
  schema: QueryBuilderTable[],
  parsed: ParsedQuery,
  existingTables: CanvasTable[],
  existingCtes: CanvasCTE[],
  existingSubqueries: CanvasSubquery[],
): AppliedParsedSqlState {
  // Build CTEs first (so we can identify CTE reference tables)
  const newCtes: CanvasCTE[] = [];
  const cteNameToId = new Map<string, string>();

  if (parsed.ctes && parsed.ctes.length > 0) {
    const existingCteMap = new Map(existingCtes.map((c) => [c.name, c]));

    for (let i = 0; i < parsed.ctes.length; i++) {
      const pc = parsed.ctes[i];
      const existing = existingCteMap.get(pc.name);

      // Build inner tables for the CTE
      const innerTables: CanvasTable[] = pc.innerQuery.tables.map((pt, idx) => ({
        id: generateId(),
        tableName: pt.tableName,
        position: { x: 20 + idx * 240, y: 50 },
        selectedColumns: new SvelteSet(
          expandSelectedColumns(schema, pt.tableName, pt.selectedColumns),
        ),
        columnAggregates: new Map(
          (pc.innerQuery.columnAggregates || [])
            .filter((ca) => ca.tableName === pt.tableName)
            .map(
              (ca) =>
                [ca.column, { function: ca.function, alias: ca.alias }] as [
                  string,
                  ColumnAggregate,
                ],
            ),
        ),
      }));

      // Build inner joins for the CTE
      const innerJoins: CanvasJoin[] = pc.innerQuery.joins.map((pj) => ({
        id: generateId(),
        sourceTable: pj.sourceTable,
        sourceColumn: pj.sourceColumn,
        targetTable: pj.targetTable,
        targetColumn: pj.targetColumn,
        joinType: pj.joinType,
      }));

      // Build inner filters for the CTE
      const innerFilters: FilterCondition[] = pc.innerQuery.filters.map((pf) => ({
        id: generateId(),
        column: pf.column,
        operator: pf.operator,
        value: pf.value,
        connector: pf.connector,
      }));

      // Build inner group by for the CTE
      const innerGroupBy: GroupByCondition[] = pc.innerQuery.groupBy.map((pg) => ({
        id: generateId(),
        column: pg.column,
      }));

      // Build inner having for the CTE
      const innerHaving: HavingCondition[] = pc.innerQuery.having.map((ph) => ({
        id: generateId(),
        aggregateFunction: ph.aggregateFunction,
        column: ph.column,
        operator: ph.operator,
        value: ph.value,
        connector: ph.connector,
      }));

      // Build inner order by for the CTE
      const innerOrderBy: SortCondition[] = pc.innerQuery.orderBy.map((po) => ({
        id: generateId(),
        column: po.column,
        direction: po.direction,
      }));

      // Build inner select aggregates for the CTE
      const innerSelectAggregates: SelectAggregate[] = pc.innerQuery.selectAggregates.map((pa) => ({
        id: generateId(),
        function: pa.function,
        expression: pa.expression,
        alias: pa.alias,
      }));

      const cteWidth = Math.max(300, 20 + pc.innerQuery.tables.length * 240 + 20);
      const cteId = existing?.id ?? generateId();

      newCtes.push({
        id: cteId,
        name: pc.name,
        position: existing?.position ?? { x: 50 + i * 350, y: 50 },
        size: existing?.size ?? { width: cteWidth, height: 350 },
        innerQuery: {
          tables: innerTables,
          joins: innerJoins,
          filters: innerFilters,
          groupBy: innerGroupBy,
          having: innerHaving,
          orderBy: innerOrderBy,
          limit: pc.innerQuery.limit,
          selectAggregates: innerSelectAggregates,
          subqueries: [],
        },
      });

      cteNameToId.set(pc.name, cteId);
    }
  }

  // Build new tables with positions
  const newTables: CanvasTable[] = [];
  const existingTableMap = new Map(existingTables.map((t) => [t.tableName, t]));

  for (let i = 0; i < parsed.tables.length; i++) {
    const pt = parsed.tables[i];
    const existing = existingTableMap.get(pt.tableName);

    const position = existing?.position ?? { x: 50 + i * 280, y: 50 + (i % 2) * 150 };

    // Build columnAggregates map for this table
    const columnAggsForTable = new Map<string, ColumnAggregate>();
    for (const ca of parsed.columnAggregates) {
      if (ca.tableName === pt.tableName) {
        columnAggsForTable.set(ca.column, {
          function: ca.function,
          alias: ca.alias,
        });
      }
    }

    // Check if this table references a CTE
    const cteId = cteNameToId.get(pt.tableName);

    newTables.push({
      id: existing?.id ?? generateId(),
      tableName: pt.tableName,
      position,
      selectedColumns: new SvelteSet(
        expandSelectedColumns(schema, pt.tableName, pt.selectedColumns),
      ),
      columnAggregates: columnAggsForTable,
      cteId,
    });
  }

  // Build new joins
  const newJoins: CanvasJoin[] = parsed.joins.map((pj) => ({
    id: generateId(),
    sourceTable: pj.sourceTable,
    sourceColumn: pj.sourceColumn,
    targetTable: pj.targetTable,
    targetColumn: pj.targetColumn,
    joinType: pj.joinType,
  }));

  // Build subqueries recursively
  const subqueryResult =
    parsed.subqueries && parsed.subqueries.length > 0
      ? buildSubqueriesFromParsed(
          schema,
          parsed.subqueries,
          existingSubqueries,
          { x: 400, y: 300 },
          cteNameToId,
        )
      : { subqueries: [], subqueryIdMap: new Map<number, string>() };
  const newSubqueries = subqueryResult.subqueries;
  const subqueryIdMap = subqueryResult.subqueryIdMap;

  // Build new filters with subquery links
  const newFilters: FilterCondition[] = parsed.filters.map((pf) => {
    const filter: FilterCondition = {
      id: generateId(),
      column: pf.column,
      operator: pf.operator,
      value: pf.value,
      connector: pf.connector,
    };

    if (pf.subqueryIndex !== undefined) {
      const subqueryId = subqueryIdMap.get(pf.subqueryIndex);
      if (subqueryId) {
        filter.subqueryId = subqueryId;
        const subquery = newSubqueries.find((s) => s.id === subqueryId);
        if (subquery) {
          subquery.linkedFilterId = filter.id;
        }
      }
    }

    return filter;
  });

  // Build new group by
  const newGroupBy: GroupByCondition[] = parsed.groupBy.map((pg) => ({
    id: generateId(),
    column: pg.column,
  }));

  // Build new having
  const newHaving: HavingCondition[] = parsed.having.map((ph) => ({
    id: generateId(),
    aggregateFunction: ph.aggregateFunction,
    column: ph.column,
    operator: ph.operator,
    value: ph.value,
    connector: ph.connector,
  }));

  // Build new order by
  const newOrderBy: SortCondition[] = parsed.orderBy.map((po) => ({
    id: generateId(),
    column: po.column,
    direction: po.direction,
  }));

  // Build new select aggregates
  const newSelectAggregates: SelectAggregate[] = parsed.selectAggregates.map((pa) => ({
    id: generateId(),
    function: pa.function,
    expression: pa.expression,
    alias: pa.alias,
  }));

  return {
    tables: newTables,
    joins: newJoins,
    filters: newFilters,
    groupBy: newGroupBy,
    having: newHaving,
    orderBy: newOrderBy,
    limit: parsed.limit,
    selectAggregates: newSelectAggregates,
    subqueries: newSubqueries,
    ctes: newCtes,
  };
}
