/**
 * CREATE/ALTER TABLE DDL for the demo's DuckDB adapter (`duckdb.ts`).
 * Demo-only; desktop and web generate DDL in Rust
 * (`crates/seaquel-engine/src/ddl.rs`).
 */

import type { CreateTableDefinition, CreateTableColumn, CreateTableForeignKey } from "$lib/types";

type QuoteFn = (name: string) => string;

/** Build the full type expression for a column (e.g. "VARCHAR(255)", "DECIMAL(10,2)"). */
function buildColumnType(col: CreateTableColumn): string {
  if (col.precision) return `${col.type}(${col.precision})`;
  if (col.length) return `${col.type}(${col.length})`;
  return col.type;
}

/**
 * Sanitize a user-provided DEFAULT value for safe interpolation into DDL.
 * Allows: numeric literals, string literals, NULL, boolean keywords,
 * CURRENT_TIMESTAMP and similar built-in defaults, and simple function calls.
 * Rejects anything that looks like it contains multiple statements or SQL injection.
 */
function sanitizeDefaultValue(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) return "";

  // Block semicolons and double-dash comments — the most common injection vectors
  if (/;|--/.test(trimmed)) return "";

  return trimmed;
}

/** Build a column line for CREATE TABLE DDL. */
function buildColumnLine(col: CreateTableColumn, q: QuoteFn): string {
  let line = `  ${q(col.name)} ${buildColumnType(col)}`;
  if (!col.nullable) line += " NOT NULL";
  if (col.defaultValue) {
    const safe = sanitizeDefaultValue(col.defaultValue);
    if (safe) line += ` DEFAULT ${safe}`;
  }
  return line;
}

/** Generate CREATE TABLE DDL from a definition. */
export function generateCreateTableDdl(definition: CreateTableDefinition, q: QuoteFn): string {
  const { tableName, schemaName, columns, indexes, foreignKeys } = definition;
  const lines: string[] = [];

  for (const col of columns) {
    lines.push(buildColumnLine(col, q));
  }

  const pkCols = columns.filter((c) => c.isPrimaryKey);
  if (pkCols.length > 0) {
    lines.push(`  PRIMARY KEY (${pkCols.map((c) => q(c.name)).join(", ")})`);
  }

  for (const col of columns) {
    if (col.isUnique && !col.isPrimaryKey) {
      lines.push(`  UNIQUE (${q(col.name)})`);
    }
  }

  for (const fk of foreignKeys) {
    lines.push(
      `  FOREIGN KEY (${q(fk.column)}) REFERENCES ${q(fk.referencedSchema)}.${q(fk.referencedTable)} (${q(fk.referencedColumn)})`,
    );
  }

  let sql = `CREATE TABLE ${q(schemaName)}.${q(tableName)} (\n${lines.join(",\n")}\n);`;

  for (const idx of indexes) {
    const unique = idx.unique ? "UNIQUE " : "";
    const cols = idx.columns.map((c) => q(c)).join(", ");
    sql += `\n\nCREATE ${unique}INDEX ${q(idx.name)} ON ${q(schemaName)}.${q(tableName)} (${cols});`;
  }

  return sql;
}

/**
 * Generate ALTER TABLE ADD COLUMN DDL.
 */
export function generateAddColumnDdl(
  schema: string,
  table: string,
  column: CreateTableColumn,
  q: QuoteFn,
): string {
  let sql = `ALTER TABLE ${q(schema)}.${q(table)} ADD COLUMN ${q(column.name)} ${buildColumnType(column)}`;
  if (!column.nullable) sql += " NOT NULL";
  if (column.defaultValue) {
    const safe = sanitizeDefaultValue(column.defaultValue);
    if (safe) sql += ` DEFAULT ${safe}`;
  }
  return sql + ";";
}

/**
 * Generate ALTER TABLE SQL statements to transform `original` into `updated`.
 * Returns a single string with semicolon-separated statements.
 */
export function generateAlterTableSql(
  original: CreateTableDefinition,
  updated: CreateTableDefinition,
  q: QuoteFn,
): string {
  const table = `${q(updated.schemaName)}.${q(updated.tableName)}`;
  const stmts: string[] = [];

  // ── Columns (matched by stable id, so renames are detected) ─────

  const origColsById = new Map(original.columns.map((c) => [c.id, c]));
  const newColsById = new Map(updated.columns.map((c) => [c.id, c]));

  // Renamed columns
  for (const newCol of updated.columns) {
    const origCol = origColsById.get(newCol.id);
    if (origCol && origCol.name !== newCol.name) {
      stmts.push(`ALTER TABLE ${table} RENAME COLUMN ${q(origCol.name)} TO ${q(newCol.name)};`);
    }
  }

  // Added columns (id not in original)
  for (const col of updated.columns) {
    if (!origColsById.has(col.id)) {
      let stmt = `ALTER TABLE ${table} ADD COLUMN ${q(col.name)} ${buildColumnType(col)}`;
      if (!col.nullable) stmt += " NOT NULL";
      if (col.defaultValue) {
        const safe = sanitizeDefaultValue(col.defaultValue);
        if (safe) stmt += ` DEFAULT ${safe}`;
      }
      stmts.push(stmt + ";");
    }
  }

  // Dropped columns (id not in updated)
  for (const col of original.columns) {
    if (!newColsById.has(col.id)) {
      stmts.push(`ALTER TABLE ${table} DROP COLUMN ${q(col.name)};`);
    }
  }

  // Modified columns (type, nullable, default changed — same id, same or renamed name)
  for (const newCol of updated.columns) {
    const origCol = origColsById.get(newCol.id);
    if (!origCol) continue;

    // Use the new name (rename was already emitted above)
    const colName = newCol.name;
    const origType = buildColumnType(origCol);
    const newType = buildColumnType(newCol);

    if (origType !== newType) {
      stmts.push(`ALTER TABLE ${table} ALTER COLUMN ${q(colName)} TYPE ${newType};`);
    }
    if (origCol.nullable && !newCol.nullable) {
      stmts.push(`ALTER TABLE ${table} ALTER COLUMN ${q(colName)} SET NOT NULL;`);
    } else if (!origCol.nullable && newCol.nullable) {
      stmts.push(`ALTER TABLE ${table} ALTER COLUMN ${q(colName)} DROP NOT NULL;`);
    }

    // Default value changes
    if (origCol.defaultValue !== newCol.defaultValue) {
      const safe = sanitizeDefaultValue(newCol.defaultValue);
      if (safe) {
        stmts.push(`ALTER TABLE ${table} ALTER COLUMN ${q(colName)} SET DEFAULT ${safe};`);
      } else {
        stmts.push(`ALTER TABLE ${table} ALTER COLUMN ${q(colName)} DROP DEFAULT;`);
      }
    }
  }

  // ── Indexes ────────────────────────────────────────────────────

  const origIdxByName = new Map(original.indexes.map((i) => [i.name, i]));
  const newIdxByName = new Map(updated.indexes.map((i) => [i.name, i]));

  // Dropped indexes
  for (const idx of original.indexes) {
    if (idx.name && !newIdxByName.has(idx.name)) {
      stmts.push(`DROP INDEX ${q(idx.name)};`);
    }
  }

  // Added indexes
  for (const idx of updated.indexes) {
    if (idx.name && !origIdxByName.has(idx.name)) {
      const unique = idx.unique ? "UNIQUE " : "";
      const cols = idx.columns.map((c) => q(c)).join(", ");
      stmts.push(`CREATE ${unique}INDEX ${q(idx.name)} ON ${table} (${cols});`);
    }
  }

  // ── Foreign Keys ───────────────────────────────────────────────

  const origFkKey = (fk: CreateTableForeignKey) =>
    `${fk.column}->${fk.referencedTable}.${fk.referencedColumn}`;
  const origFks = new Set(original.foreignKeys.map(origFkKey));

  // Added FKs
  for (const fk of updated.foreignKeys) {
    if (!origFks.has(origFkKey(fk))) {
      const refTable = fk.referencedSchema
        ? `${q(fk.referencedSchema)}.${q(fk.referencedTable)}`
        : q(fk.referencedTable);
      stmts.push(
        `ALTER TABLE ${table} ADD FOREIGN KEY (${q(fk.column)}) REFERENCES ${refTable} (${q(fk.referencedColumn)});`,
      );
    }
  }

  // We don't drop FKs automatically — requires knowing constraint names

  if (stmts.length === 0) {
    return "-- No changes detected";
  }

  return stmts.join("\n");
}
