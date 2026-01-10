import * as monaco from "monaco-editor";
import type { SchemaTable } from "$lib/types";

interface TableReference {
  table: SchemaTable;
  alias?: string;
}

/**
 * Find all tables referenced in FROM and JOIN clauses
 */
function findTablesInQuery(
  queryText: string,
  schema: SchemaTable[],
): TableReference[] {
  const tables: TableReference[] = [];

  // FROM clause: FROM [schema.]table [AS] [alias]
  const fromRegex = /FROM\s+(?:(\w+)\.)?(\w+)(?:\s+(?:AS\s+)?(\w+))?/gi;
  let match;

  while ((match = fromRegex.exec(queryText)) !== null) {
    const schemaName = match[1]?.toLowerCase();
    const tableName = match[2].toLowerCase();
    const alias = match[3];

    // Check if the "alias" is actually a SQL keyword (meaning no alias)
    const sqlKeywords = [
      "WHERE",
      "JOIN",
      "LEFT",
      "RIGHT",
      "INNER",
      "OUTER",
      "FULL",
      "CROSS",
      "ON",
      "ORDER",
      "GROUP",
      "HAVING",
      "LIMIT",
      "OFFSET",
      "UNION",
      "INTERSECT",
      "EXCEPT",
    ];
    const validAlias =
      alias && !sqlKeywords.includes(alias.toUpperCase()) ? alias : undefined;

    const tableMatch = schema.find(
      (t) =>
        t.name.toLowerCase() === tableName &&
        (!schemaName || t.schema.toLowerCase() === schemaName),
    );
    if (tableMatch) {
      tables.push({ table: tableMatch, alias: validAlias });
    }
  }

  // JOIN clause: [LEFT|RIGHT|...] JOIN [schema.]table [AS] [alias]
  const joinRegex = /JOIN\s+(?:(\w+)\.)?(\w+)(?:\s+(?:AS\s+)?(\w+))?/gi;

  while ((match = joinRegex.exec(queryText)) !== null) {
    const schemaName = match[1]?.toLowerCase();
    const tableName = match[2].toLowerCase();
    const alias = match[3];

    const sqlKeywords = ["ON", "WHERE", "AND", "OR", "LEFT", "RIGHT", "INNER"];
    const validAlias =
      alias && !sqlKeywords.includes(alias.toUpperCase()) ? alias : undefined;

    const tableMatch = schema.find(
      (t) =>
        t.name.toLowerCase() === tableName &&
        (!schemaName || t.schema.toLowerCase() === schemaName),
    );
    if (tableMatch && !tables.some((t) => t.table.name === tableMatch.name)) {
      tables.push({ table: tableMatch, alias: validAlias });
    }
  }

  return tables;
}

/**
 * Determine the current SQL clause context based on the last keyword before cursor
 */
function getCurrentClause(
  textBeforeCursor: string,
): "select" | "from" | "where" | "on" | "order" | "group" | "having" | "set" | "other" {
  const upper = textBeforeCursor.toUpperCase();

  // Find positions of all clause keywords
  const clauses: { clause: ReturnType<typeof getCurrentClause>; pos: number }[] = [];

  const patterns: [RegExp, ReturnType<typeof getCurrentClause>][] = [
    [/\bSELECT\b/gi, "select"],
    [/\bFROM\b/gi, "from"],
    [/\bWHERE\b/gi, "where"],
    [/\bON\b/gi, "on"],
    [/\bORDER\s+BY\b/gi, "order"],
    [/\bGROUP\s+BY\b/gi, "group"],
    [/\bHAVING\b/gi, "having"],
    [/\bSET\b/gi, "set"],
    [/\bJOIN\b/gi, "from"], // JOIN is part of FROM clause for table context
  ];

  for (const [pattern, clause] of patterns) {
    let match;
    while ((match = pattern.exec(upper)) !== null) {
      clauses.push({ clause, pos: match.index });
    }
  }

  if (clauses.length === 0) return "other";

  // Return the clause with the highest position (closest to cursor)
  clauses.sort((a, b) => b.pos - a.pos);
  return clauses[0].clause;
}

/**
 * Check if cursor is in a context where column suggestions make sense
 */
function isColumnContext(textBeforeCursor: string): boolean {
  const clause = getCurrentClause(textBeforeCursor);

  // Column context: SELECT, WHERE, ON, ORDER BY, GROUP BY, HAVING, SET
  // Not column context: FROM (expects table names)
  return ["select", "where", "on", "order", "group", "having", "set"].includes(clause);
}

export function createSchemaCompletionProvider(
  getSchema: () => SchemaTable[],
): monaco.languages.CompletionItemProvider {
  return {
    triggerCharacters: [".", " ", ","],

    provideCompletionItems(
      model: monaco.editor.ITextModel,
      position: monaco.Position,
    ): monaco.languages.ProviderResult<monaco.languages.CompletionList> {
      const schema = getSchema();
      const word = model.getWordUntilPosition(position);
      const range = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };

      // Get full query text and text before cursor
      const fullQuery = model.getValue();
      const lineContent = model.getLineContent(position.lineNumber);
      const textBeforeCursor = lineContent.substring(0, position.column - 1);

      // Get all text before cursor position across all lines
      const textBeforeCursorFull = model.getValueInRange({
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: position.lineNumber,
        endColumn: position.column,
      });

      const suggestions: monaco.languages.CompletionItem[] = [];

      // Check if we're after a table name or alias with dot (for column completion)
      const dotMatch = textBeforeCursor.match(/(\w+)\.\s*$/);
      if (dotMatch) {
        const prefix = dotMatch[1].toLowerCase();
        const referencedTables = findTablesInQuery(fullQuery, schema);

        // Find table by name or alias
        const tableRef = referencedTables.find(
          (ref) =>
            ref.table.name.toLowerCase() === prefix ||
            ref.alias?.toLowerCase() === prefix,
        );
        const table = tableRef?.table ?? schema.find((t) => t.name.toLowerCase() === prefix);

        if (table) {
          // Add column suggestions
          table.columns.forEach((col) => {
            const markers: string[] = [];
            if (col.isPrimaryKey) markers.push("PK");
            if (col.isForeignKey) markers.push("FK");
            if (!col.nullable) markers.push("NOT NULL");

            suggestions.push({
              label: col.name,
              kind: monaco.languages.CompletionItemKind.Field,
              detail: `${col.type}${markers.length ? ` (${markers.join(", ")})` : ""}`,
              insertText: col.name,
              range,
            });
          });
          return { suggestions };
        }
      }

      // Find tables referenced in FROM/JOIN clauses
      const referencedTables = findTablesInQuery(fullQuery, schema);

      // Context-aware column completion: if tables are referenced and we're in a column context
      if (referencedTables.length > 0 && isColumnContext(textBeforeCursorFull)) {
        // Add * for select all
        suggestions.push({
          label: "*",
          kind: monaco.languages.CompletionItemKind.Constant,
          detail: "Select all columns",
          insertText: "*",
          range,
          sortText: "!0", // Sort first (! comes before letters/numbers)
        });

        // Add columns from all referenced tables
        referencedTables.forEach((ref) => {
          const prefix = ref.alias ?? ref.table.name;
          const showPrefix = referencedTables.length > 1;

          ref.table.columns.forEach((col, idx) => {
            const markers: string[] = [];
            if (col.isPrimaryKey) markers.push("PK");
            if (col.isForeignKey) markers.push("FK");
            if (!col.nullable) markers.push("NOT NULL");

            const label = showPrefix ? `${prefix}.${col.name}` : col.name;
            suggestions.push({
              label,
              kind: monaco.languages.CompletionItemKind.Field,
              detail: `${ref.table.name}.${col.name} (${col.type}${markers.length ? `, ${markers.join(", ")}` : ""})`,
              insertText: label,
              range,
              sortText: `!1${String(idx).padStart(3, "0")}`, // Sort after *, before other suggestions
            });
          });
        });

        // When in column context, return early to only show column suggestions
        // This makes them much more prominent
        return { suggestions };
      }

      // Determine current clause context
      const currentClause = getCurrentClause(textBeforeCursorFull);

      // Add schema suggestions (unique schema names) - Group 0: Schemas
      const uniqueSchemas = [...new Set(schema.map((t) => t.schema))].sort();
      uniqueSchemas.forEach((schemaName, idx) => {
        suggestions.push({
          label: schemaName,
          kind: monaco.languages.CompletionItemKind.Folder,
          detail: "schema",
          insertText: schemaName,
          range,
          sortText: `0-schema-${String(idx).padStart(3, "0")}`,
        });
      });

      // Add table suggestions - Group 1: Tables
      const sortedTables = [...schema].sort((a, b) => a.name.localeCompare(b.name));
      sortedTables.forEach((table, idx) => {
        // Table name only
        suggestions.push({
          label: table.name,
          kind:
            table.type === "view"
              ? monaco.languages.CompletionItemKind.Interface
              : monaco.languages.CompletionItemKind.Struct,
          detail: `${table.schema}.${table.name} (${table.type})`,
          insertText: table.name,
          range,
          sortText: `1-table-${String(idx).padStart(3, "0")}`,
        });

        // Schema-qualified table name
        suggestions.push({
          label: `${table.schema}.${table.name}`,
          kind:
            table.type === "view"
              ? monaco.languages.CompletionItemKind.Interface
              : monaco.languages.CompletionItemKind.Struct,
          detail: table.columns.length ? `${table.columns.length} columns` : "",
          insertText: `${table.schema}.${table.name}`,
          range,
          sortText: `1-table-${String(idx).padStart(3, "0")}-qualified`,
        });
      });

      // In FROM context, only show table names (no keywords or types)
      if (currentClause === "from") {
        return { suggestions };
      }

      // Add SQL keywords
      const keywords = [
        "SELECT",
        "FROM",
        "WHERE",
        "JOIN",
        "LEFT JOIN",
        "RIGHT JOIN",
        "INNER JOIN",
        "FULL OUTER JOIN",
        "CROSS JOIN",
        "ON",
        "AND",
        "OR",
        "NOT",
        "IN",
        "LIKE",
        "ILIKE",
        "ORDER BY",
        "GROUP BY",
        "HAVING",
        "LIMIT",
        "OFFSET",
        "INSERT INTO",
        "UPDATE",
        "DELETE FROM",
        "VALUES",
        "SET",
        "CREATE TABLE",
        "ALTER TABLE",
        "DROP TABLE",
        "AS",
        "DISTINCT",
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "NULL",
        "IS NULL",
        "IS NOT NULL",
        "BETWEEN",
        "EXISTS",
        "UNION",
        "UNION ALL",
        "INTERSECT",
        "EXCEPT",
        "WITH",
        "RETURNING",
        "COALESCE",
        "NULLIF",
        "CAST",
        "ASC",
        "DESC",
      ];

      keywords.forEach((kw) => {
        suggestions.push({
          label: kw,
          kind: monaco.languages.CompletionItemKind.Keyword,
          insertText: kw,
          range,
        });
      });

      // Add PostgreSQL types
      const pgTypes = [
        "integer",
        "bigint",
        "smallint",
        "serial",
        "bigserial",
        "text",
        "varchar",
        "char",
        "boolean",
        "date",
        "timestamp",
        "timestamptz",
        "time",
        "timetz",
        "interval",
        "uuid",
        "json",
        "jsonb",
        "numeric",
        "decimal",
        "real",
        "double precision",
        "bytea",
        "inet",
        "cidr",
        "macaddr",
      ];

      pgTypes.forEach((type) => {
        suggestions.push({
          label: type,
          kind: monaco.languages.CompletionItemKind.TypeParameter,
          insertText: type,
          range,
        });
      });

      return { suggestions };
    },
  };
}
