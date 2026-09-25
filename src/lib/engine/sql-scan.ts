/**
 * A small SQL tokenizer for the checks the query runner makes on a user's
 * query before it pages it: whether the query limits its own rows
 * (`hasRowLimit`) and, for SQL Server's row count, its trailing ORDER BY
 * (`stripTrailingOrderBy`). Keywords count only outside string literals,
 * quoted names and comments, at the top level (not inside parentheses) and
 * not after a `.` (`t.limit` is a column).
 *
 * Each engine's quoting: `'…'` strings everywhere (`''` inside); Postgres and
 * DuckDB `"…"` names, `E'…'` strings with backslash escapes and `$tag$…$tag$`
 * strings; MySQL and MariaDB `` `…` `` names, `"…"` strings, backslash
 * escapes and `#` comments; SQLite `"…"`, `` `…` `` and `[…]` names; SQL
 * Server `"…"` and `[…]` names (`]]` inside). Block comments nest on
 * Postgres, DuckDB and SQL Server.
 */

import type { DatabaseType } from "$lib/types";

export interface SqlToken {
  /** `word`: keyword, name or number. `quoted`: string literal or quoted name. */
  kind: "word" | "quoted" | "punct";
  text: string;
  /** Offsets into the SQL, `end` exclusive. */
  start: number;
  end: number;
  /** Parenthesis depth the token is at (`(` and `)` themselves at the outer depth). */
  depth: number;
}

/** `#` is part of a T-SQL name (`#temp`); on MySQL it starts a comment, even mid-word. */
const isWordChar = (c: string, mysql: boolean) =>
  /[\p{L}\p{N}_$@]/u.test(c) || (c === "#" && !mysql);
const isSpace = (c: string) => /\s/.test(c);

/** The significant tokens of `sql` (no whitespace, no comments). */
export function sqlTokens(sql: string, type: DatabaseType): SqlToken[] {
  const mysql = type === "mysql" || type === "mariadb";
  const pgLike = type === "postgres" || type === "duckdb";
  const nestedComments = pgLike || type === "mssql";
  const nameQuotes = mysql ? "`" : type === "sqlite" ? '"`[' : type === "mssql" ? '"[' : '"';
  const tokens: SqlToken[] = [];
  const n = sql.length;
  let depth = 0;
  let i = 0;

  /** The end of a quoted run opening at `i` (after its closing quote). */
  const skipQuoted = (open: string, backslash: boolean): number => {
    const close = open === "[" ? "]" : open;
    let j = i + 1;
    while (j < n) {
      const c = sql[j];
      if (backslash && c === "\\") {
        j += 2;
        continue;
      }
      if (c === close) {
        // `[…]` escapes `]` as `]]` on SQL Server only; SQLite's has no escape.
        const doubles = open !== "[" || type === "mssql";
        if (doubles && sql[j + 1] === close) {
          j += 2;
          continue;
        }
        return j + 1;
      }
      j++;
    }
    return n;
  };

  const push = (kind: SqlToken["kind"], end: number, at = depth) => {
    tokens.push({ kind, text: sql.slice(i, end), start: i, end, depth: at });
    i = end;
  };

  while (i < n) {
    const c = sql[i];
    const next = sql[i + 1];
    if (isSpace(c)) {
      i++;
    } else if (c === "-" && next === "-" && (!mysql || i + 2 >= n || isSpace(sql[i + 2]))) {
      const eol = sql.indexOf("\n", i);
      i = eol === -1 ? n : eol + 1;
    } else if (c === "#" && mysql) {
      const eol = sql.indexOf("\n", i);
      i = eol === -1 ? n : eol + 1;
    } else if (c === "/" && next === "*") {
      let level = 1;
      let j = i + 2;
      while (j < n && level > 0) {
        if (nestedComments && sql[j] === "/" && sql[j + 1] === "*") {
          level++;
          j += 2;
        } else if (sql[j] === "*" && sql[j + 1] === "/") {
          level--;
          j += 2;
        } else {
          j++;
        }
      }
      i = j;
    } else if (c === "'") {
      push("quoted", skipQuoted("'", mysql));
    } else if (nameQuotes.includes(c)) {
      push("quoted", skipQuoted(c, mysql && c === '"'));
    } else if (mysql && c === '"') {
      push("quoted", skipQuoted('"', true));
    } else if (pgLike && (c === "e" || c === "E") && next === "'") {
      i++;
      const end = skipQuoted("'", true);
      i--;
      push("quoted", end);
    } else if (
      pgLike &&
      c === "$" &&
      /^\$(?:[A-Za-z_\p{L}][\w\p{L}]*)?\$/u.test(sql.slice(i, i + 64))
    ) {
      const tag = /^\$(?:[A-Za-z_\p{L}][\w\p{L}]*)?\$/u.exec(sql.slice(i))![0];
      const close = sql.indexOf(tag, i + tag.length);
      push("quoted", close === -1 ? n : close + tag.length);
    } else if (isWordChar(c, mysql)) {
      let j = i + 1;
      while (j < n && isWordChar(sql[j], mysql)) j++;
      push("word", j);
    } else if (c === "(") {
      push("punct", i + 1);
      depth++;
    } else if (c === ")") {
      depth = Math.max(0, depth - 1);
      push("punct", i + 1);
    } else {
      push("punct", i + 1);
    }
  }
  return tokens;
}

/** Top-level keywords: upper-cased words at depth 0 that don't follow a `.`, with their token. */
function topLevelWords(tokens: SqlToken[]): Array<{ word: string; token: SqlToken }> {
  const words: Array<{ word: string; token: SqlToken }> = [];
  tokens.forEach((t, k) => {
    if (t.depth !== 0 || t.kind !== "word") return;
    if (k > 0 && tokens[k - 1].text === ".") return;
    words.push({ word: t.text.toUpperCase(), token: t });
  });
  return words;
}

/** Whether the words hold a row limit: LIMIT, OFFSET, FETCH FIRST/NEXT, or TOP on SQL Server. */
function limits(words: Array<{ word: string }>, type: DatabaseType): boolean {
  return words.some(
    ({ word }, k) =>
      word === "LIMIT" ||
      word === "OFFSET" ||
      (word === "FETCH" && ["FIRST", "NEXT"].includes(words[k + 1]?.word ?? "")) ||
      (type === "mssql" && word === "TOP"),
  );
}

/**
 * Whether the query limits its own rows at the top level (LIMIT, OFFSET,
 * FETCH FIRST/NEXT, or TOP on SQL Server), so the runner shouldn't page it.
 * A limit in a subquery, a string, a quoted name or a comment doesn't count.
 */
export function hasRowLimit(sql: string, type: DatabaseType): boolean {
  return limits(topLevelWords(sqlTokens(sql, type)), type);
}

/**
 * `sql` without its top-level ORDER BY and what follows it, for a row count
 * that wraps the query in a derived table: SQL Server rejects ORDER BY there
 * unless TOP or OFFSET goes with it. A query that limits its rows keeps its
 * ORDER BY (it decides which rows count). ORDER BY inside parentheses (OVER,
 * subqueries) stays. A trailing `FOR XML`/`FOR JSON` or `OPTION (…)` after
 * the ORDER BY goes with it; such a query can't be counted as a derived
 * table anyway, and the runner falls back to an estimate.
 */
export function stripTrailingOrderBy(sql: string, type: DatabaseType): string {
  const words = topLevelWords(sqlTokens(sql, type));
  if (limits(words, type)) return sql;
  for (let k = words.length - 2; k >= 0; k--) {
    if (words[k].word === "ORDER" && words[k + 1].word === "BY") {
      return sql.slice(0, words[k].token.start).trimEnd();
    }
  }
  return sql;
}

/**
 * The row count of `sql` as a query: `SELECT COUNT(*) AS total FROM (…)`, with
 * the trailing ORDER BY stripped on SQL Server (`stripTrailingOrderBy`).
 */
export function countQuery(sql: string, type: DatabaseType): string {
  const inner = type === "mssql" ? stripTrailingOrderBy(sql, type) : sql;
  return `SELECT COUNT(*) as total FROM (${inner}) AS count_query`;
}
