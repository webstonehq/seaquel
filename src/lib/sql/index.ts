/**
 * SQL text work for the app: statement splitting, statement at cursor,
 * `{{param}}` substitution, the statement checks, `CREATE TABLE` parsing and
 * the AST helpers (query builder, tutorial, Visual tab, column sources).
 *
 * All of it runs in `seaquel-sql` (Rust), through the `seaquel-wasm` module
 * the root layout loads before anything renders, so every function here is
 * synchronous. The names and signatures are the ones the call sites used with
 * the TypeScript this replaces: `validateReadOnlyQuery` in
 * `services/ai/context.ts`, and `db/sql-parser.ts`, `db/query-params.ts`,
 * `db/query-utils.ts`, `engine/sql-scan.ts`, `db/parse-create-table.ts`,
 * `tutorial/sql-parser.ts`, `db/sql-ast-parser.ts` and `db/column-sources.ts`,
 * which phase 2b deleted. The statement checks take the engine as a new
 * argument, because they follow its quoting now (fixes 11, 12 and 14 in
 * `docs/plans/2026-09-27-rust-core-phase-2b-plan.md`).
 *
 * Offsets are UTF-16 code units, as JS strings and Monaco count them; the
 * conversion from Rust's byte offsets happens inside the module. Statement
 * text is sliced from the caller's string, so it's exactly what was passed
 * in (a lone surrogate included).
 */
import { callWasm, type SeaquelWasm } from "$lib/wasm";
import { decodeCell, encodeParam } from "$lib/values";
import { TUTORIAL_SCHEMA, getTableNames } from "$lib/tutorial/schema";
import type {
  ColumnSourceInfo,
  CreateTableDefinition,
  DatabaseType,
  ParameterValue,
  SchemaTable,
} from "$lib/types";
import type { ColumnRef } from "$lib/types/generated/ColumnRef";
import type { DestructiveReason } from "$lib/types/generated/DestructiveReason";
import type { ParsedQuery } from "$lib/types/generated/ParsedQuery";
import type { ParsedQueryVisual } from "$lib/types/generated/ParsedQueryVisual";
import type { QueryType } from "$lib/types/generated/QueryType";
import type { SubstituteResult } from "$lib/types/generated/SubstituteResult";
import type { TableRef } from "$lib/types/generated/TableRef";
import type { VisualResult } from "$lib/types/generated/VisualResult";
import type { WasmStatement } from "$lib/types/generated/WasmStatement";
import type { WireParameterValue } from "$lib/types/generated/WireParameterValue";
import { ParameterSubstitutionError } from "./parameters";

export { ParameterSubstitutionError, createDefaultParameters, coerceValue } from "./parameters";
export type {
  DestructiveReason,
  QueryType,
  TableRef,
  ParsedQuery,
  ParsedQueryVisual,
  ColumnRef,
  VisualResult,
};
export type { AggregateFunction } from "$lib/types/generated/AggregateFunction";
export type { FilterOperator } from "$lib/types/generated/FilterOperator";
export type { HavingOperator } from "$lib/types/generated/HavingOperator";
export type { JoinType } from "$lib/types/generated/JoinType";
export type { ParsedColumnAggregate } from "$lib/types/generated/ParsedColumnAggregate";
export type { ParsedCTE } from "$lib/types/generated/ParsedCTE";
export type { ParsedFilter } from "$lib/types/generated/ParsedFilter";
export type { ParsedGroupBy } from "$lib/types/generated/ParsedGroupBy";
export type { ParsedHaving } from "$lib/types/generated/ParsedHaving";
export type { ParsedJoin } from "$lib/types/generated/ParsedJoin";
export type { ParsedOrderBy } from "$lib/types/generated/ParsedOrderBy";
export type { ParsedSelectAggregate } from "$lib/types/generated/ParsedSelectAggregate";
export type { ParsedSubquery } from "$lib/types/generated/ParsedSubquery";
export type { ParsedTable } from "$lib/types/generated/ParsedTable";

// --- calling the module --------------------------------------------------------

/**
 * Every JSON export answers `{ok}` or `{error}` (a bad call, not a result).
 * Hand-mirrored from `crates/seaquel-wasm/src/lib.rs` (`respond`): the Rust
 * side serializes borrowed, generic structs that ts-rs can't export as one
 * type. The shapes inside `ok` are generated.
 */
type Envelope<T> = { ok: T } | { error: string };

/** A bad call into the module: an unknown engine id, malformed JSON. */
export class SeaquelSqlError extends Error {
  override name = "SeaquelSqlError";
}

function envelope<T>(fn: (m: SeaquelWasm) => string): Envelope<T> {
  return JSON.parse(callWasm(fn)) as Envelope<T>;
}

/** Runs an export and returns its `ok`, throwing its `error`. */
function call<T>(fn: (m: SeaquelWasm) => string): T {
  const out = envelope<T>(fn);
  if ("error" in out) throw new SeaquelSqlError(out.error);
  return out.ok;
}

/**
 * `s` with each unpaired surrogate replaced by U+FFFD, what wasm-bindgen does
 * to a string argument anyway. Needed before `JSON.stringify`, which writes a
 * lone surrogate as a `\uD83D` escape that serde_json refuses.
 */
function wellFormed(s: string): string {
  let out: string[] | undefined;
  let from = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0xd800 || c > 0xdfff) continue;
    if (c <= 0xdbff) {
      const next = s.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        i++;
        continue;
      }
    }
    (out ??= []).push(s.slice(from, i), "\uFFFD");
    from = i + 1;
  }
  return out ? out.join("") + s.slice(from) : s;
}

/**
 * Runs `fn` and returns `fallback` if it throws: a `SeaquelSqlError`, or
 * whatever `callWasm` rethrew after a trap (it has already re-instantiated the
 * module). For the functions the UI calls from `$derived`/`$effect` or on
 * every keystroke, which answered with `null`, `[]` and the like when the TS
 * couldn't parse, and must never take the component down.
 */
function orFallback<T>(what: string, fallback: T, fn: () => T): T {
  try {
    return fn();
  } catch (e) {
    console.error(`$lib/sql: ${what} failed`, e);
    return fallback;
  }
}

function toJson(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) => (typeof v === "string" ? wellFormed(v) : v));
}

// --- statements ------------------------------------------------------------------

export interface ParsedStatement {
  /** The statement's text, trimmed. */
  sql: string;
  /** Position among the non-empty statements. */
  index: number;
  /** UTF-16 offset one past the previous statement's `;` (0 for the first). */
  startOffset: number;
  /** UTF-16 offset of the statement's `;`, or the input's length minus one. */
  endOffset: number;
}

function toParsed(sql: string, s: WasmStatement): ParsedStatement {
  return { sql: sql.slice(s.from, s.to), index: s.index, startOffset: s.start, endOffset: s.end };
}

/**
 * Splits SQL into statements on `;`, following the engine's quoting (strings,
 * quoted names, comments, dollar quotes). Comment-only statements are
 * dropped. `[]` if the module fails (logged).
 */
export function splitSqlStatements(sql: string, dbType: DatabaseType): ParsedStatement[] {
  return orFallback("splitSqlStatements", [], () => splitSqlStatementsOrThrow(sql, dbType));
}

/**
 * `splitSqlStatements` for the run path: throws if the module fails, where
 * `[]` would read as "nothing to run".
 */
export function splitSqlStatementsOrThrow(sql: string, dbType: DatabaseType): ParsedStatement[] {
  return call<WasmStatement[]>((m) => m.split_statements(sql, dbType)).map((s) => toParsed(sql, s));
}

/**
 * Finds the statement at a UTF-16 cursor offset.
 * - If cursor is within a statement, returns that statement
 * - If cursor is before all statements, returns the first statement
 * - If cursor is after all statements, returns the last statement
 * - If cursor is between statements, returns the next statement
 *
 * `null` if the module fails (logged).
 */
export function getStatementAtOffset(
  sql: string,
  offset: number,
  dbType: DatabaseType,
): ParsedStatement | null {
  return orFallback("getStatementAtOffset", null, () =>
    getStatementAtOffsetOrThrow(sql, offset, dbType),
  );
}

/**
 * `getStatementAtOffset` for the run path: throws if the module fails, where
 * `null` would make the caller skip the destructive check and run the whole
 * buffer. `null` still means the buffer has no statement.
 */
export function getStatementAtOffsetOrThrow(
  sql: string,
  offset: number,
  dbType: DatabaseType,
): ParsedStatement | null {
  const s = call<WasmStatement | null>((m) => m.statement_at(sql, offset, dbType));
  return s ? toParsed(sql, s) : null;
}

// --- parameters --------------------------------------------------------------------

/**
 * Extract parameter names from a query string.
 * Returns unique parameter names in order of first appearance.
 */
export function extractParameters(query: string): string[] {
  return orFallback("extractParameters", [], () =>
    call<string[]>((m) => m.extract_parameters(query)),
  );
}

/** Check if a query contains parameters. */
export function hasParameters(query: string): boolean {
  return orFallback("hasParameters", false, () => callWasm((m) => m.has_parameters(query)));
}

/**
 * Substitute `{{param}}` placeholders for the engine: bound placeholders
 * (`$n` on Postgres and SQLite, `?` on MySQL and MariaDB) with their values in
 * `bindValues`, or the values inlined (SQL Server, DuckDB, and every engine
 * with `forceInline`, which the Visual tab and parsing use). Comments, quoted
 * names and strings follow the engine's quoting.
 *
 * Throws `ParameterSubstitutionError` for a value that can't be substituted
 * safely (and for any other failure to substitute).
 */
export function substituteParameters(
  query: string,
  parameterValues: ParameterValue[],
  dbType: DatabaseType,
  forceInline: boolean = false,
): { sql: string; bindValues: unknown[] } {
  let out: Envelope<SubstituteResult>;
  try {
    const values: WireParameterValue[] = parameterValues.map((p) => ({
      name: p.name,
      // Every substituter treats a Date as its ISO text (decision 11).
      value: encodeParam(p.value instanceof Date ? p.value.toISOString() : p.value),
    }));
    const json = toJson(values);
    out = envelope<SubstituteResult>((m) =>
      m.substitute_parameters(query, json, dbType, forceInline),
    );
  } catch (e) {
    // An invalid Date, a value JSON can't hold (a cycle), or a trap in the
    // module. `resolve-query.ts` shows only this class's message.
    throw new ParameterSubstitutionError(e instanceof Error ? e.message : String(e), {
      cause: e,
    });
  }
  if ("error" in out) throw new ParameterSubstitutionError(out.error);
  return { sql: out.ok.sql, bindValues: out.ok.bindValues.map(decodeCell) };
}

// --- statement checks ------------------------------------------------------------------

export interface DestructiveStatement {
  sql: string;
  index: number;
  reason: DestructiveReason;
}

/**
 * The statement's kind, from its first keyword (after comments and `WITH …`).
 * `"other"` if the module fails (logged): the runner then neither pages the
 * query nor offers inline editing.
 */
export function detectQueryType(query: string, dbType: DatabaseType): QueryType {
  return orFallback("detectQueryType", "other", () => detectQueryTypeOrThrow(query, dbType));
}

/**
 * `detectQueryType` for the run path: throws if the module fails, where
 * `"other"` would run a SELECT unpaged as a utility statement.
 */
export function detectQueryTypeOrThrow(query: string, dbType: DatabaseType): QueryType {
  return call<QueryType>((m) => m.query_type(query, dbType));
}

/** Returns true if the query is a SELECT statement. */
export function isSelectQuery(query: string, dbType: DatabaseType): boolean {
  return detectQueryType(query, dbType) === "select";
}

/**
 * The first top-level FROM's table, unquoted, for inline editing. `null` for a
 * subquery, a table function, or no FROM, and if the module fails (logged),
 * which leaves the result read-only.
 */
export function extractTableFromSelect(query: string, dbType: DatabaseType): TableRef | null {
  return orFallback("extractTableFromSelect", null, () =>
    call<TableRef | null>((m) => m.table_from_select(query, dbType)),
  );
}

/**
 * Checks if a SQL statement is destructive (could cause irreversible data/schema loss).
 * Returns the reason if destructive, null otherwise.
 *
 * Throws if the module fails, rather than answer "not destructive": it runs
 * when the user runs a script, not on a keystroke, and the runner reports the
 * error instead of running the script unconfirmed.
 */
export function isDestructiveStatement(
  sql: string,
  dbType: DatabaseType,
): DestructiveReason | null {
  return call<DestructiveReason | null>((m) => m.destructive_reason(sql, dbType));
}

/** Finds all destructive statements in a batch of parsed SQL statements. */
export function findDestructiveStatements(
  statements: ParsedStatement[],
  dbType: DatabaseType,
): DestructiveStatement[] {
  const results: DestructiveStatement[] = [];
  for (const stmt of statements) {
    const reason = isDestructiveStatement(stmt.sql, dbType);
    if (reason) results.push({ sql: stmt.sql, index: stmt.index, reason });
  }
  return results;
}

/**
 * Whether the query limits its own rows at the top level (LIMIT, OFFSET,
 * FETCH FIRST/NEXT, or TOP on SQL Server), so the runner shouldn't page it.
 * Throws if the module fails (the runner reports it): either answer would
 * change what runs.
 */
export function hasRowLimit(sql: string, type: DatabaseType): boolean {
  return call<boolean>((m) => m.has_row_limit(sql, type));
}

/**
 * A query counting the rows `sql` returns, without its top-level trailing
 * ORDER BY (SQL Server rejects one in a derived table). Throws if the module
 * fails; the runner then logs it and estimates the total, as when the count
 * query itself fails, so no rewritten SQL runs.
 */
export function countQuery(sql: string, type: DatabaseType): string {
  return call<string>((m) => m.count_query(sql, type));
}

/** `seaquel_sql::read_only`'s message, word for word the TS text. */
export const READ_ONLY_REFUSAL = "Only read-only SELECT queries are permitted";

/**
 * The AI's read-only check: the refusal message, or `null` when every
 * statement is a read-only SELECT or WITH query. It gates which statements
 * run; it isn't a sandbox.
 *
 * Fails closed: if the module fails, the query is refused with the same
 * message.
 */
export function validateReadOnlyQuery(query: string, dbType: DatabaseType): string | null {
  return orFallback("validateReadOnlyQuery", READ_ONLY_REFUSAL, () =>
    call<string | null>((m) => m.read_only_error(query, dbType)),
  );
}

// --- CREATE TABLE ------------------------------------------------------------------------

/**
 * Parse a CREATE TABLE statement back into a CreateTableDefinition, for the
 * table editor's SQL pane. Accepts `"…"`, `` `…` `` and `[…]` names on every
 * engine. `null` when it can't be read (or it would take too long, so the
 * pane never freezes on a keystroke), and if the module fails (logged).
 */
export function parseCreateTableSql(sql: string): CreateTableDefinition | null {
  const def = orFallback("parseCreateTableSql", null, () =>
    call<CreateTableDefinition | null>((m) => m.parse_create_table(sql)),
  );
  if (!def) return null;
  // The module's ids (`column-N`, …) are unique within one definition only;
  // split panes can drag a column from one table editor into another.
  for (const c of def.columns) c.id = crypto.randomUUID();
  for (const i of def.indexes) i.id = crypto.randomUUID();
  for (const f of def.foreignKeys) f.id = crypto.randomUUID();
  return def;
}

// --- query builder and tutorial -------------------------------------------------------------

export interface ParseSqlOptions {
  /**
   * List of valid table names to accept. If not provided, uses tutorial schema.
   * Pass null to accept all table names (for real database schemas).
   */
  validTableNames?: string[] | null;
  /**
   * The dialect to parse in: the connection's engine for the query builder
   * (fix 9). Defaults to `postgres`, which the tutorial uses.
   */
  engine?: DatabaseType;
}

let tutorialSchemaJson: string | undefined;

/** The tutorial schema as `{table: [column]}`, which `t.*` expands from. */
function tutorialSchema(): string {
  tutorialSchemaJson ??= toJson(
    Object.fromEntries(TUTORIAL_SCHEMA.map((t) => [t.name, t.columns.map((c) => c.name)])),
  );
  return tutorialSchemaJson;
}

/**
 * Parse SQL and extract query builder components.
 * Returns null if SQL cannot be parsed or is not a SELECT statement, and if
 * the module fails (logged).
 */
export function parseSql(sql: string, options?: ParseSqlOptions): ParsedQuery | null {
  const validTableNames =
    options?.validTableNames === null ? null : (options?.validTableNames ?? getTableNames());
  const engine = options?.engine ?? "postgres";
  return orFallback("parseSql", null, () => {
    const valid = toJson(validTableNames);
    return call<ParsedQuery | null>((m) =>
      m.parse_builder_query(sql, engine, tutorialSchema(), valid),
    );
  });
}

// --- Visual tab ------------------------------------------------------------------------------

/** What `getParseError` said when the TS parser gave no message. */
const UNPARSEABLE = "Unable to parse SQL query";

/**
 * The Visual tab's AST and parse error from one parse: `visual` is the AST
 * or `null`; `parseError` is `getParseError`'s answer. Use it instead of
 * calling `parseQueryForVisualization` and then `getParseError`, which parses
 * twice. If the module fails (logged), `{ visual: null, parseError:
 * "Unable to parse SQL query" }`.
 */
export function parseVisualQuery(sql: string, dbType: DatabaseType = "postgres"): VisualResult {
  return orFallback("parseVisualQuery", { visual: null, parseError: UNPARSEABLE }, () => {
    const out = call<VisualResult>((m) => m.parse_visual(sql, dbType));
    return {
      visual: out.visual,
      parseError: out.parseError === null ? null : out.parseError || UNPARSEABLE,
    };
  });
}

/**
 * Parse a SQL query and extract structured information for visualization.
 * `null` when it doesn't parse, and if the module fails (logged).
 */
export function parseQueryForVisualization(
  sql: string,
  dbType: DatabaseType = "postgres",
): ParsedQueryVisual | null {
  return orFallback(
    "parseQueryForVisualization",
    null,
    () => call<VisualResult>((m) => m.parse_visual(sql, dbType)).visual,
  );
}

/**
 * The parser's message when `sql` doesn't parse, or `null`. Its
 * `Line: L, Column: C` counts the column in UTF-16 units, as the editor does.
 * `"Unable to parse SQL query"` if the module fails (logged).
 */
export function getParseError(sql: string, dbType: DatabaseType = "postgres"): string | null {
  return orFallback("getParseError", UNPARSEABLE, () => {
    const message = call<string | null>((m) => m.parse_error(sql, dbType));
    return message === null ? null : message || UNPARSEABLE;
  });
}

// --- column sources ------------------------------------------------------------------------

/**
 * Look up a table by (schema, name) in the cached schemas list. When `schema`
 * is unspecified the first table matching by name wins — same fallback the
 * rest of the codebase uses when the query doesn't qualify a table.
 */
function findTable(schemas: SchemaTable[], name: string, schema?: string): SchemaTable | undefined {
  if (schema) {
    return schemas.find((t) => t.name === name && t.schema === schema);
  }
  return schemas.find((t) => t.name === name);
}

/**
 * Resolve per-column source info for a SELECT query, for inline editing.
 *
 * Returns `undefined` when the columns can't be mapped confidently:
 * unparseable SQL, a statement that isn't a SELECT, a subquery in FROM,
 * `SELECT *` or `t.*`, and if the module fails (logged). Otherwise the array is parallel to the query's output
 * columns; an entry is `undefined` where the column isn't a base-table column
 * (an expression, an aggregate, an unqualified column of a join) or its table
 * isn't in `schemas` or has no primary key.
 *
 * The module resolves the column references; the table and primary-key
 * lookup stays here, so the schema cache never crosses into it.
 */
export function resolveColumnSources(
  query: string,
  dbType: DatabaseType,
  schemas: SchemaTable[],
): (ColumnSourceInfo | undefined)[] | undefined {
  const refs = orFallback("resolveColumnSources", null, () =>
    call<(ColumnRef | null)[] | null>((m) => m.column_refs(query, dbType)),
  );
  if (!refs) return undefined;
  return refs.map((ref) => {
    if (!ref) return undefined;
    const table = findTable(schemas, ref.table, ref.schema);
    if (!table) return undefined;
    const primaryKeys = table.columns.filter((c) => c.isPrimaryKey).map((c) => c.name);
    // A source with no PKs can't support row-bound edits anyway.
    if (primaryKeys.length === 0) return undefined;
    return { schema: table.schema, table: table.name, primaryKeys, column: ref.column };
  });
}
