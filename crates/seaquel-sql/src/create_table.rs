//! `CREATE TABLE` text to a table definition, for the table editor's SQL pane
//! (Task 5). Port of `src/lib/db/parse-create-table.ts` (deleted in phase 2b)
//! with bug fixes 16 and 17, as the recorder's `create-table-fixed.ts` defines
//! them (`docs/plans/artifacts/2026-09-27-sql-recorder-create-table-fixed.ts.txt`).
//! Not sqlparser: the pane matches type names as the user wrote them.
//!
//! The TS is a handful of regular expressions over three small scanners of its
//! own (`stripComments`, `findMatchingParen`, `splitTopLevel`), not the
//! statement tokenizer, and its output depends on regex details: where a
//! lazy `DEFAULT (.+?)` stops, a keyword matched without a word boundary,
//! which of a doubled quote's readings a name takes. So this file ports the
//! scanners as they are and each regex as a pattern for [`Matcher`], a small
//! backtracking matcher with JavaScript's semantics for the few constructs
//! the patterns use (ASCII case-insensitive literals, `\s`, `\w`, `\b`, `.`,
//! greedy and lazy repeats). No `regex` crate (module size), and no recursion
//! per input character, so a long input can't overflow the stack. Where a
//! repeat's retries can't change the result it doesn't retry (possessive
//! `\s+` before a keyword), and a step budget bounds the rest.
//!
//! Everything works on `char`s. The TS works on UTF-16 units, which differs
//! only for lone surrogates, and those can't reach a Rust `&str`.

use std::cell::Cell;

use seaquel_types::{
    CreateTableColumn, CreateTableDefinition, CreateTableForeignKey, CreateTableIndex,
};

use crate::js_ws::is_js_space;

/// The definition in `sql`, or `None` if it isn't a `CREATE TABLE` the TS
/// understood. Engine-agnostic: `"…"`, `` `…` `` and `[…]` names work on every
/// engine, as in the TS, and the schema defaults to `public` on every engine.
///
/// Ids are placeholders: `column-N`, `index-N` and `fk-N`, unique only within
/// this definition. The editor needs globally unique ids (split panes can drag
/// a column from one editor to another), so the TS wrapper replaces every id
/// with `crypto.randomUUID()`, as the TS did.
///
/// On a pathological input (a backtracking blowup) the parse gives up past a
/// step budget and returns `None`, rather than freeze the editor, which
/// parses on every keystroke.
pub fn parse_create_table(sql: &str) -> Option<CreateTableDefinition> {
    let input: Vec<char> = sql.chars().collect();
    let budget = Budget::for_input(input.len());
    // FIX 16: comments go before anything else, so a column after one is kept.
    let sql = strip_comments(&input);
    let sql = sql.as_slice();

    // ── 1. The CREATE TABLE header ────────────────────────────────────
    // CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?(ID)(?:\s*\.\s*(ID))?(?:\s*\.\s*(ID))?\s*\(
    let header = seq(vec![
        lit("CREATE"),
        ws1(),
        lit("TABLE"),
        ws1(),
        opt(seq(vec![
            lit("IF"),
            ws1(),
            lit("NOT"),
            ws1(),
            lit("EXISTS"),
            ws1(),
        ])),
        group(1, Node::Id),
        opt(seq(vec![ws0(), lit("."), ws0(), group(2, Node::Id)])),
        opt(seq(vec![ws0(), lit("."), ws0(), group(3, Node::Id)])),
        ws0(),
        lit("("),
    ]);
    let m = Matcher::new(sql, &budget).search(&header, 0)?;

    let (schema_name, table_name) = match (m.text(1), m.text(2), m.text(3)) {
        // FIX 17: catalog.schema.table. A part holding `.` or `"` is
        // double-quoted, as DuckDB's part() writes it, so the schema name
        // round-trips through the DuckDB engine's quoting.
        (Some(catalog), Some(schema), Some(table)) => (
            format!("{}.{}", duckdb_part(catalog), duckdb_part(schema)),
            unquote(table),
        ),
        (Some(schema), Some(table), None) => (unquote(schema), unquote(table)),
        (Some(table), _, _) => ("public".to_string(), unquote(table)),
        _ => return None,
    };

    // ── 2. The body between the outer ( … ) ──────────────────────────
    // The match ends with the `(`.
    let body_start = m.end.checked_sub(1)?;
    let body_end = find_matching_paren(sql, body_start)?;
    let body = sql.get(body_start + 1..body_end)?;

    // ── 3. Items: columns and table constraints ──────────────────────
    let mut columns: Vec<CreateTableColumn> = Vec::new();
    let mut pk_columns: Vec<String> = Vec::new();
    let mut unique_columns: Vec<String> = Vec::new();
    let mut foreign_keys: Vec<CreateTableForeignKey> = Vec::new();

    // FIX 16: `CONSTRAINT name` before PRIMARY KEY / UNIQUE. FIX 17: the
    // constraint's name is a name (it may be quoted, with spaces) before
    // FOREIGN KEY too.
    let constraint_name = || opt(seq(vec![lit("CONSTRAINT"), ws1(), Node::Id, ws1()]));
    let table_pk = seq(vec![
        Node::Start,
        constraint_name(),
        lit("PRIMARY"),
        ws1(),
        lit("KEY"),
        Node::WordBoundary,
    ]);
    let table_unique = seq(vec![
        Node::Start,
        constraint_name(),
        lit("UNIQUE"),
        Node::WordBoundary,
    ]);
    let table_fk = seq(vec![
        Node::Start,
        constraint_name(),
        lit("FOREIGN"),
        ws1(),
        lit("KEY"),
        Node::WordBoundary,
    ]);
    let skipped = seq(vec![
        Node::Start,
        alt(vec![lit("CONSTRAINT"), lit("CHECK")]),
        Node::WordBoundary,
    ]);

    for raw in split_top_level(body) {
        let item = js_trim(&raw);
        if item.is_empty() {
            continue;
        }
        let matcher = Matcher::new(item, &budget);

        if matcher.test(&table_pk) {
            pk_columns.extend(extract_paren_list(item));
            continue;
        }
        if matcher.test(&table_unique) {
            unique_columns.extend(extract_paren_list(item));
            continue;
        }
        if matcher.test(&table_fk) {
            if let Some(mut fk) = parse_foreign_key(item, &budget) {
                fk.id = format!("fk-{}", foreign_keys.len());
                foreign_keys.push(fk);
            }
            continue;
        }
        if matcher.test(&skipped) {
            continue;
        }

        let Some(mut column) = parse_column_def(item, &budget) else {
            continue;
        };
        column.id = format!("column-{}", columns.len());
        // FIX 16: an inline REFERENCES is a foreign key.
        if let Some(mut fk) = parse_inline_reference(item, &column.name, &budget) {
            fk.id = format!("fk-{}", foreign_keys.len());
            foreign_keys.push(fk);
        }
        columns.push(column);
    }

    // Table-level PRIMARY KEY / UNIQUE back onto the columns.
    for column in &mut columns {
        if pk_columns.contains(&column.name) {
            column.is_primary_key = true;
            column.nullable = false;
        }
        if unique_columns.contains(&column.name) {
            column.is_unique = true;
        }
    }

    // ── 4. CREATE INDEX statements after the table ───────────────────
    // FIX 17: index and table names follow the name rules.
    // CREATE\s+(UNIQUE\s+)?INDEX\s+(?:IF\s+NOT\s+EXISTS\s+)?(ID)\s+ON\s+ID(?:\s*\.\s*ID){0,2}\s*\(([^)]+)\)
    let index = seq(vec![
        lit("CREATE"),
        ws1(),
        opt(group(1, seq(vec![lit("UNIQUE"), ws1()]))),
        lit("INDEX"),
        ws1(),
        opt(seq(vec![
            lit("IF"),
            ws1(),
            lit("NOT"),
            ws1(),
            lit("EXISTS"),
            ws1(),
        ])),
        group(2, Node::Id),
        ws1(),
        lit("ON"),
        ws1(),
        Node::Id,
        Node::Rep(Box::new(seq(vec![ws0(), lit("."), ws0(), Node::Id])), 0, 2),
        ws0(),
        lit("("),
        group(3, class(|c| c != ')', 1, usize::MAX, Mode::Possessive)),
        lit(")"),
    ]);
    let after_table = sql.get(body_end + 1..).unwrap_or_default();
    let matcher = Matcher::new(after_table, &budget);
    let mut indexes: Vec<CreateTableIndex> = Vec::new();
    let mut from = 0;
    while let Some(m) = matcher.search(&index, from) {
        from = if m.end > m.start { m.end } else { m.end + 1 };
        let columns = m
            .text(3)
            .map(|cols| split_top_level(cols).iter().map(|c| unquote(c)).collect())
            .unwrap_or_default();
        indexes.push(CreateTableIndex {
            id: format!("index-{}", indexes.len()),
            name: m.text(2).map(unquote).unwrap_or_default(),
            columns,
            unique: m.text(1).is_some(),
            ty: "btree".to_string(),
        });
    }

    // Out of budget: some match failed for want of steps, not on the text.
    if budget.exhausted() {
        return None;
    }

    Some(CreateTableDefinition {
        table_name,
        schema_name,
        columns,
        indexes,
        foreign_keys,
    })
}

// ── item parsers ──────────────────────────────────────────────────────

/// The keywords that end a column's type, with FIX 16's changes: `SERIAL`
/// is a type, not a constraint (the TS cut `serial` to `""`), and a
/// `COLLATE` clause isn't part of the type.
const TYPE_END: &[&str] = &[
    "NOT NULL",
    "NULL",
    "DEFAULT",
    "PRIMARY KEY",
    "UNIQUE",
    "REFERENCES",
    "CHECK",
    "CONSTRAINT",
    "AUTO_INCREMENT",
    "AUTOINCREMENT",
    "IDENTITY",
    "GENERATED",
    "COLLATE",
];

/// The keywords that end a `DEFAULT` expression (FIX 16 adds `COLLATE`).
const DEFAULT_END: &[&str] = &[
    "NOT NULL",
    "NULL",
    "PRIMARY",
    "UNIQUE",
    "CHECK",
    "REFERENCES",
    "CONSTRAINT",
    "AUTO_INCREMENT",
    "AUTOINCREMENT",
    "IDENTITY",
    "GENERATED",
    "COLLATE",
];

/// `<name> <type> [constraints…]`.
fn parse_column_def(item: &[char], budget: &Budget) -> Option<CreateTableColumn> {
    // ^(ID)\s+
    let name_re = seq(vec![Node::Start, group(1, Node::Id), ws1()]);
    let m = Matcher::new(item, budget).search(&name_re, 0)?;
    let name = unquote(m.text(1)?);
    let mut rest = item.get(m.end..).unwrap_or_default();

    // The type: everything up to the first constraint keyword, or the end.
    let type_end = seq(vec![
        Node::WordBoundary,
        alt(TYPE_END.iter().map(|k| keyword(k)).collect()),
        Node::WordBoundary,
    ]);
    let type_str = match Matcher::new(rest, budget).search(&type_end, 0) {
        Some(m) => {
            let type_str = js_trim(rest.get(..m.start).unwrap_or_default());
            rest = rest.get(m.start..).unwrap_or_default();
            type_str
        }
        None => {
            let type_str = js_trim(rest);
            rest = &[];
            type_str
        }
    };
    // A trailing comma off the type.
    let type_str = js_trim(type_str.strip_suffix(&[',']).unwrap_or(type_str));

    // Base type + length or precision: ^([^(]+)\(([^)]+)\)
    let mut ty = to_string(type_str);
    let mut length = None;
    let mut precision = None;
    if let Some(open) = type_str.iter().position(|&c| c == '(').filter(|&i| i > 0) {
        let inner = type_str.get(open + 1..).unwrap_or_default();
        if let Some(close) = inner.iter().position(|&c| c == ')').filter(|&i| i > 0) {
            ty = to_string(js_trim(type_str.get(..open).unwrap_or_default()));
            let params = js_trim(inner.get(..close).unwrap_or_default());
            if params.contains(&',') {
                precision = Some(to_string(params));
            } else {
                length = Some(to_string(params));
            }
        }
    }

    // Constraints, from the rest.
    let rest_matcher = Matcher::new(rest, budget);
    let not_null = to_string(rest).to_uppercase().contains("NOT NULL");
    let is_primary_key = rest_matcher.test(&keyword("PRIMARY KEY"));
    let is_unique = rest_matcher.test(&seq(vec![
        Node::WordBoundary,
        lit("UNIQUE"),
        Node::WordBoundary,
    ]));

    // DEFAULT\s+(.+?)(?:\s+(?:NOT\s+NULL|NULL|…|COLLATE)|$)
    let default_re = seq(vec![
        lit("DEFAULT"),
        // Greedy, not possessive: `.` matches whitespace too.
        class(is_js_space, 1, usize::MAX, Mode::Greedy),
        group(1, class(is_js_dot, 1, usize::MAX, Mode::Lazy)),
        alt(vec![
            seq(vec![
                ws1(),
                alt(DEFAULT_END.iter().map(|k| keyword(k)).collect()),
            ]),
            Node::End,
        ]),
    ]);
    let default_value = rest_matcher
        .search(&default_re, 0)
        .and_then(|m| m.text(1))
        .map(|d| {
            let d = js_trim(d);
            to_string(js_trim(d.strip_suffix(&[',']).unwrap_or(d)))
        })
        .unwrap_or_default();

    // FIX 16: `COLLATE name` goes into `collation` (MSSQL's own DDL writes it).
    let collate_re = seq(vec![
        Node::WordBoundary,
        lit("COLLATE"),
        ws1(),
        group(1, Node::Id),
    ]);
    let collation = rest_matcher
        .search(&collate_re, 0)
        .and_then(|m| m.text(1))
        .map(unquote);

    Some(CreateTableColumn {
        id: String::new(),
        name,
        ty,
        length,
        precision,
        nullable: !is_primary_key && !not_null,
        default_value,
        is_primary_key,
        is_unique,
        collation,
        in_unique_constraint: false,
    })
}

/// `[CONSTRAINT name] FOREIGN KEY (col) REFERENCES [schema.]table (col)`.
fn parse_foreign_key(item: &[char], budget: &Budget) -> Option<CreateTableForeignKey> {
    // FOREIGN\s+KEY\s*\(\s*(ID)\s*\)\s*REFERENCES\s+(ID)(?:\.(ID))?\s*\(\s*(ID)\s*\)
    let fk_re = seq(vec![
        lit("FOREIGN"),
        ws1(),
        lit("KEY"),
        ws0(),
        lit("("),
        ws0(),
        group(1, Node::Id),
        ws0(),
        lit(")"),
        ws0(),
        lit("REFERENCES"),
        ws1(),
        group(2, Node::Id),
        opt(seq(vec![lit("."), group(3, Node::Id)])),
        ws0(),
        lit("("),
        ws0(),
        group(4, Node::Id),
        ws0(),
        lit(")"),
    ]);
    let m = Matcher::new(item, budget).search(&fk_re, 0)?;
    let column = unquote(m.text(1)?);
    let (referenced_schema, referenced_table) = match m.text(3) {
        Some(table) => (m.text(2).map(unquote).unwrap_or_default(), unquote(table)),
        None => (String::new(), m.text(2).map(unquote).unwrap_or_default()),
    };
    Some(CreateTableForeignKey {
        id: String::new(),
        column,
        referenced_schema,
        referenced_table,
        referenced_column: m.text(4).map(unquote).unwrap_or_default(),
    })
}

/// FIX 16: a column's inline `REFERENCES [schema.]table [(col)]`. Without a
/// column list the referenced column is `""`.
fn parse_inline_reference(
    item: &[char],
    column: &str,
    budget: &Budget,
) -> Option<CreateTableForeignKey> {
    // \bREFERENCES\s+(ID)(?:\s*\.\s*(ID))?(?:\s*\(\s*(ID)\s*\))?, searched
    // from the item's first whitespace on (`item.slice(item.search(/\s/))`).
    let ref_re = seq(vec![
        Node::WordBoundary,
        lit("REFERENCES"),
        ws1(),
        group(1, Node::Id),
        opt(seq(vec![ws0(), lit("."), ws0(), group(2, Node::Id)])),
        opt(seq(vec![
            ws0(),
            lit("("),
            ws0(),
            group(3, Node::Id),
            ws0(),
            lit(")"),
        ])),
    ]);
    // `slice(-1)` when there's no whitespace: the last character.
    let from = item
        .iter()
        .position(|&c| is_js_space(c))
        .unwrap_or(item.len().saturating_sub(1));
    let m = Matcher::new(item.get(from..).unwrap_or_default(), budget).search(&ref_re, 0)?;
    let first = m.text(1)?;
    let (referenced_schema, referenced_table) = match m.text(2) {
        Some(table) => (unquote(first), unquote(table)),
        None => (String::new(), unquote(first)),
    };
    Some(CreateTableForeignKey {
        id: String::new(),
        column: column.to_string(),
        referenced_schema,
        referenced_table,
        referenced_column: m.text(3).map(unquote).unwrap_or_default(),
    })
}

// ── names ─────────────────────────────────────────────────────────────

/// Strips one layer of quotes (`"…"`, `` `…` ``, `[…]`) from a trimmed name,
/// and (FIX 17) turns a doubled quote inside into one.
fn unquote(s: &[char]) -> String {
    let s = js_trim(s);
    let (Some(&first), Some(&last)) = (s.first(), s.last()) else {
        return String::new();
    };
    let close = match first {
        '"' | '`' => first,
        '[' => ']',
        _ => return to_string(s),
    };
    if last != close {
        return to_string(s);
    }
    // `slice(1, -1)`: empty for a lone quote character.
    let inner = s.get(1..s.len().saturating_sub(1)).unwrap_or_default();
    let mut out = String::with_capacity(inner.len());
    let mut i = 0;
    while let Some(&c) = inner.get(i) {
        out.push(c);
        i += if c == close && inner.get(i + 1) == Some(&close) {
            2
        } else {
            1
        };
    }
    out
}

/// A three-part name's catalog or schema: unquoted, and double-quoted again
/// if it holds `.` or `"`, as DuckDB's `part()` writes it.
fn duckdb_part(raw: &[char]) -> String {
    let part = unquote(raw);
    if part.contains(['.', '"']) {
        format!("\"{}\"", part.replace('"', "\"\""))
    } else {
        part
    }
}

// ── scanners ──────────────────────────────────────────────────────────

/// The quote a character opens, if any, and its closing character.
fn opens(c: char) -> Option<char> {
    match c {
        '\'' | '"' | '`' => Some(c),
        '[' => Some(']'),
        _ => None,
    }
}

/// FIX 16: `sql` without its comments (`--` to the end of the line,
/// `/* … */`), leaving quoted text alone. A comment becomes one space; the
/// newline that ends a `--` comment stays.
fn strip_comments(sql: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(sql.len());
    let mut i = 0;
    while let Some(&c) = sql.get(i) {
        let next = sql.get(i + 1).copied();
        if let Some(close) = opens(c) {
            let mut j = i + 1;
            while let Some(&d) = sql.get(j) {
                if d == close && sql.get(j + 1) == Some(&close) {
                    j += 2;
                } else if d == close {
                    j += 1;
                    break;
                } else {
                    j += 1;
                }
            }
            out.extend_from_slice(sql.get(i..j).unwrap_or_default());
            i = j;
        } else if c == '-' && next == Some('-') {
            out.push(' ');
            i = sql
                .get(i..)
                .and_then(|s| s.iter().position(|&d| d == '\n'))
                .map_or(sql.len(), |p| i + p);
        } else if c == '/' && next == Some('*') {
            out.push(' ');
            i = sql
                .get(i + 2..)
                .and_then(|s| s.windows(2).position(|w| w == ['*', '/']))
                .map_or(sql.len(), |p| i + 2 + p + 2);
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The index of the `)` matching the `(` at `start`. A doubled quote toggles
/// twice, so it stays inside the quoted text.
fn find_matching_paren(sql: &[char], start: usize) -> Option<usize> {
    let mut depth = 0i64;
    let mut close: Option<char> = None;
    for (i, &c) in sql.iter().enumerate().skip(start) {
        if let Some(q) = close {
            if c == q {
                close = None;
            }
        } else if let Some(q) = opens(c) {
            close = Some(q);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// `s` split at commas outside parentheses and quotes. A last part that is
/// only whitespace is dropped; empty parts before it are kept.
fn split_top_level(s: &[char]) -> Vec<Vec<char>> {
    let mut parts = Vec::new();
    let mut depth = 0i64;
    let mut close: Option<char> = None;
    let mut current = Vec::new();
    for &c in s {
        if let Some(q) = close {
            if c == q {
                close = None;
            }
        } else if let Some(q) = opens(c) {
            close = Some(q);
        } else if c == '(' {
            depth += 1;
        } else if c == ')' {
            depth -= 1;
        } else if c == ',' && depth == 0 {
            parts.push(std::mem::take(&mut current));
            continue;
        }
        current.push(c);
    }
    if !js_trim(&current).is_empty() {
        parts.push(current);
    }
    parts
}

/// The names in the first parenthesised group. FIX 17: a quoted name may
/// hold `)` or `,`.
fn extract_paren_list(s: &[char]) -> Vec<String> {
    let Some(start) = s.iter().position(|&c| c == '(') else {
        return Vec::new();
    };
    let Some(end) = find_matching_paren(s, start) else {
        return Vec::new();
    };
    split_top_level(s.get(start + 1..end).unwrap_or_default())
        .iter()
        .map(|c| unquote(c))
        .collect()
}

// ── JavaScript character classes ──────────────────────────────────────

/// JavaScript's `\w` without the `u` flag: ASCII only, also under `i`.
fn is_js_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// JavaScript's `.` without the `s` flag: anything but a line terminator.
fn is_js_dot(c: char) -> bool {
    !matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// JavaScript's `trim()`.
fn js_trim(s: &[char]) -> &[char] {
    let start = s.iter().position(|&c| !is_js_space(c)).unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| !is_js_space(c))
        .map_or(start, |i| i + 1);
    s.get(start..end).unwrap_or_default()
}

fn to_string(s: &[char]) -> String {
    s.iter().collect()
}

// ── the matcher ───────────────────────────────────────────────────────

/// How a [`Node::Class`] repeat backtracks.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Longest first, then each shorter count (JavaScript's `+`, `*`).
    Greedy,
    /// Shortest first (`+?`).
    Lazy,
    /// Longest only. Used where the next node can't start with a character of
    /// the class (`\s+` before a keyword, `[^)]+` before `)`): there every
    /// shorter count fails on the next node anyway, so the result is
    /// JavaScript's, without the quadratic retries (V8 has the same retries,
    /// but runs them about 45 times faster than this matcher).
    Possessive,
}

/// A pattern: the parts of JavaScript's regex syntax the TS's patterns use,
/// with its backtracking order.
enum Node {
    /// ASCII text, matched case-insensitively (the `i` flag without `u`
    /// never folds a non-ASCII character onto an ASCII one).
    Lit(&'static str),
    /// A single-character class repeated `min..=max` times. Looped, not
    /// recursed, so a long run can't overflow the stack.
    Class {
        test: fn(char) -> bool,
        min: usize,
        max: usize,
        mode: Mode,
    },
    /// The TS's possibly-quoted name (FIX 17: doubled quotes inside):
    /// `"(?:[^"]|"")+"`, `` `(?:[^`]|``)+` ``, `\[(?:[^\]]|\]\])+\]` or
    /// `[A-Za-z_]\w*`.
    Id,
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    /// A capturing group.
    Group(usize, Box<Node>),
    /// A greedy repeat of a pattern, `min..=max` times (only small counts).
    Rep(Box<Node>, usize, usize),
    /// `\b`, on ASCII word characters.
    WordBoundary,
    /// `^` (no `m` flag).
    Start,
    /// `$` (no `m` flag).
    End,
}

fn lit(text: &'static str) -> Node {
    Node::Lit(text)
}

fn class(test: fn(char) -> bool, min: usize, max: usize, mode: Mode) -> Node {
    Node::Class {
        test,
        min,
        max,
        mode,
    }
}

/// `\s+` before a node that can't start with whitespace (every `\s+` in the
/// TS's patterns but the one after `DEFAULT`), so possessive.
fn ws1() -> Node {
    class(is_js_space, 1, usize::MAX, Mode::Possessive)
}

/// `\s*`, likewise before a node that can't start with whitespace.
fn ws0() -> Node {
    class(is_js_space, 0, usize::MAX, Mode::Possessive)
}

fn seq(nodes: Vec<Node>) -> Node {
    Node::Seq(nodes)
}

fn alt(nodes: Vec<Node>) -> Node {
    Node::Alt(nodes)
}

fn opt(node: Node) -> Node {
    Node::Rep(Box::new(node), 0, 1)
}

fn group(index: usize, node: Node) -> Node {
    Node::Group(index, Box::new(node))
}

/// Words with `\s+` between them: `keyword("NOT NULL")` is `NOT\s+NULL`.
fn keyword(words: &'static str) -> Node {
    let mut nodes = Vec::new();
    for (i, word) in words.split(' ').enumerate() {
        if i > 0 {
            nodes.push(ws1());
        }
        nodes.push(lit(word));
    }
    seq(nodes)
}

/// Capture slots: groups are numbered from 1, as in the TS patterns.
const GROUPS: usize = 5;
type Captures = [Option<(usize, usize)>; GROUPS];
type Cont<'k> = &'k mut dyn FnMut(usize, &mut Captures) -> bool;

/// The work one `parse_create_table` call may do, counted in matcher steps
/// (a node tried, or a character scanned). Past it every match fails and the
/// parse gives `None`: the table editor parses on every keystroke, and a
/// pathological input must not freeze it. Ordinary DDL takes well under 100
/// steps per character, so the limit only bites on backtracking blowups.
struct Budget {
    used: Cell<u64>,
    limit: u64,
}

impl Budget {
    const BASE: u64 = 2_000_000;
    const PER_CHAR: u64 = 100;

    fn for_input(chars: usize) -> Self {
        Self {
            used: Cell::new(0),
            limit: Self::BASE.saturating_add(Self::PER_CHAR.saturating_mul(chars as u64)),
        }
    }

    /// Spends `steps`; `false` once the budget is gone.
    fn spend(&self, steps: usize) -> bool {
        let used = self.used.get().saturating_add(steps as u64);
        self.used.set(used);
        used <= self.limit
    }

    fn exhausted(&self) -> bool {
        self.used.get() > self.limit
    }
}

struct Match<'s> {
    text: &'s [char],
    start: usize,
    end: usize,
    captures: Captures,
}

impl<'s> Match<'s> {
    /// The text of group `index`, or `None` if it didn't take part.
    fn text(&self, index: usize) -> Option<&'s [char]> {
        let (start, end) = (*self.captures.get(index)?)?;
        self.text.get(start..end)
    }
}

/// Runs the cache remembers: one per class node.
const RUNS: usize = 8;

struct Matcher<'s, 'b> {
    text: &'s [char],
    budget: &'b Budget,
    /// For each class node (by address), the last run of matching
    /// characters found: `(node, start, end)`. A later position inside that
    /// run reuses its end instead of scanning again, so a search that retries
    /// a class at every position of a long run stays linear.
    runs: Cell<[(usize, usize, usize); RUNS]>,
}

impl<'s, 'b> Matcher<'s, 'b> {
    fn new(text: &'s [char], budget: &'b Budget) -> Self {
        Self {
            text,
            budget,
            runs: Cell::new([(0, 0, 0); RUNS]),
        }
    }

    /// The leftmost match at or after `from` (`RegExp.prototype.exec`).
    fn search(&self, pattern: &Node, from: usize) -> Option<Match<'s>> {
        for start in from..=self.text.len() {
            if self.budget.exhausted() {
                return None;
            }
            let mut captures: Captures = [None; GROUPS];
            let mut end = None;
            if self.at(pattern, start, &mut captures, &mut |p, _| {
                end = Some(p);
                true
            }) {
                return Some(Match {
                    text: self.text,
                    start,
                    end: end?,
                    captures,
                });
            }
        }
        None
    }

    /// `RegExp.prototype.test`.
    fn test(&self, pattern: &Node) -> bool {
        self.search(pattern, 0).is_some()
    }

    fn char_at(&self, pos: usize) -> Option<char> {
        self.text.get(pos).copied()
    }

    /// How many characters from `pos` on pass `test`, through the run cache.
    fn run_len(&self, node: &Node, test: fn(char) -> bool, pos: usize) -> usize {
        let key = node as *const Node as usize;
        let mut runs = self.runs.get();
        if let Some(&(_, _, end)) = runs
            .iter()
            .find(|&&(k, start, end)| k == key && start <= pos && pos <= end)
        {
            return end - pos;
        }
        let mut end = pos;
        while self.char_at(end).is_some_and(test) {
            end += 1;
        }
        self.budget.spend(end - pos);
        let slot = runs
            .iter()
            .position(|&(k, _, _)| k == key)
            .or_else(|| runs.iter().position(|&(k, _, _)| k == 0))
            .unwrap_or(0);
        if let Some(entry) = runs.get_mut(slot) {
            *entry = (key, pos, end);
        }
        self.runs.set(runs);
        end - pos
    }

    /// Matches `node` at `pos`, then calls `k` with the end; on `false` from
    /// `k`, tries the next way `node` can match, in JavaScript's order.
    fn at(&self, node: &Node, pos: usize, caps: &mut Captures, k: Cont) -> bool {
        if !self.budget.spend(1) {
            return false;
        }
        match node {
            Node::Lit(text) => {
                let mut p = pos;
                for expected in text.chars() {
                    match self.char_at(p) {
                        Some(c) if c.eq_ignore_ascii_case(&expected) => p += 1,
                        _ => return false,
                    }
                }
                k(p, caps)
            }
            Node::Class {
                test,
                min,
                max,
                mode,
            } => {
                let n = self.run_len(node, *test, pos).min(*max);
                if n < *min {
                    return false;
                }
                match mode {
                    Mode::Greedy => (*min..=n).rev().any(|count| k(pos + count, caps)),
                    Mode::Lazy => (*min..=n).any(|count| k(pos + count, caps)),
                    Mode::Possessive => k(pos + n, caps),
                }
            }
            Node::Id => {
                let IdEnds {
                    quoted,
                    unquoted,
                    scanned,
                } = id_ends(self.text, pos);
                if !self.budget.spend(scanned.saturating_sub(pos)) {
                    return false;
                }
                quoted.into_iter().any(|end| k(end, caps))
                    || unquoted.is_some_and(|ends| ends.rev().any(|end| k(end, caps)))
            }
            Node::Seq(nodes) => self.seq(nodes, pos, caps, k),
            Node::Alt(nodes) => nodes.iter().any(|n| self.at(n, pos, caps, k)),
            Node::Group(index, inner) => {
                let index = *index;
                self.at(inner, pos, caps, &mut |end, caps| {
                    let Some(slot) = caps.get_mut(index) else {
                        return false;
                    };
                    let old = slot.replace((pos, end));
                    if k(end, caps) {
                        return true;
                    }
                    if let Some(slot) = caps.get_mut(index) {
                        *slot = old;
                    }
                    false
                })
            }
            Node::Rep(inner, min, max) => self.rep(inner, *min, *max, 0, pos, caps, k),
            Node::WordBoundary => {
                let before = pos
                    .checked_sub(1)
                    .and_then(|p| self.char_at(p))
                    .is_some_and(is_js_word);
                let after = self.char_at(pos).is_some_and(is_js_word);
                before != after && k(pos, caps)
            }
            Node::Start => pos == 0 && k(pos, caps),
            Node::End => pos == self.text.len() && k(pos, caps),
        }
    }

    fn seq(&self, nodes: &[Node], pos: usize, caps: &mut Captures, k: Cont) -> bool {
        match nodes.split_first() {
            None => k(pos, caps),
            Some((first, rest)) => {
                self.at(first, pos, caps, &mut |p, caps| self.seq(rest, p, caps, k))
            }
        }
    }

    /// A greedy repeat: one more iteration first, then stopping here. As in
    /// JavaScript, an iteration past `min` that matches nothing fails.
    #[allow(clippy::too_many_arguments)]
    fn rep(
        &self,
        inner: &Node,
        min: usize,
        max: usize,
        count: usize,
        pos: usize,
        caps: &mut Captures,
        k: Cont,
    ) -> bool {
        if count < max
            && self.at(inner, pos, caps, &mut |p, caps| {
                !(p == pos && count >= min) && self.rep(inner, min, max, count + 1, p, caps, k)
            })
        {
            return true;
        }
        count >= min && k(pos, caps)
    }
}

/// [`id_ends`]'s answer.
#[derive(Default)]
struct IdEnds {
    quoted: Vec<usize>,
    unquoted: Option<std::ops::Range<usize>>,
    /// Where the scan stopped, for the budget.
    scanned: usize,
}

/// Where a name starting at `pos` can end, in the order the TS's regex tries
/// them: longest first. A quoted name can end at any quote that isn't
/// escaped by the one before it (`"a""b"` can also be read as `"a"`): those
/// ends come first, longest first. An unquoted one can end after any of its
/// characters: the range, tried from its end down.
fn id_ends(text: &[char], pos: usize) -> IdEnds {
    let Some(&first) = text.get(pos) else {
        return IdEnds::default();
    };
    match first {
        '"' | '`' | '[' => {
            let close = if first == '[' { ']' } else { first };
            let mut ends = Vec::new();
            let mut j = pos + 1;
            while let Some(&c) = text.get(j) {
                if c != close {
                    j += 1;
                    continue;
                }
                // `+`: the name holds at least one character.
                if j > pos + 1 {
                    ends.push(j + 1);
                }
                if text.get(j + 1) == Some(&close) {
                    j += 2;
                } else {
                    break;
                }
            }
            ends.reverse();
            IdEnds {
                quoted: ends,
                unquoted: None,
                scanned: j.min(text.len()),
            }
        }
        c if c.is_ascii_alphabetic() || c == '_' => {
            let mut j = pos + 1;
            while text.get(j).copied().is_some_and(is_js_word) {
                j += 1;
            }
            IdEnds {
                quoted: Vec::new(),
                unquoted: Some(pos + 1..j + 1),
                scanned: j,
            }
        }
        _ => IdEnds::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn unquote_unescapes_doubled_quotes() {
        assert_eq!(unquote(&chars(r#" "a""b" "#)), r#"a"b"#);
        assert_eq!(unquote(&chars("`a``b`")), "a`b");
        assert_eq!(unquote(&chars("[a]]b]")), "a]b");
        assert_eq!(unquote(&chars("\"")), "");
        assert_eq!(unquote(&chars("plain")), "plain");
    }

    #[test]
    fn a_doubled_quote_can_also_end_a_name() {
        let text = chars(r#""a""b""#);
        assert_eq!(id_ends(&text, 0).quoted, vec![6, 3]);
        assert!(id_ends(&chars(r#""""#), 0).quoted.is_empty());
        let unquoted: Vec<usize> = id_ends(&chars("ab c"), 0).unquoted.unwrap().rev().collect();
        assert_eq!(unquoted, vec![2, 1]);
    }

    #[test]
    fn default_stops_at_the_first_keyword_or_the_end() {
        let def = parse_create_table(
            "CREATE TABLE t (a int DEFAULT (1, 2) NOT NULL, b text DEFAULT 'x' COLLATE \"C\")",
        )
        .expect("parses");
        assert_eq!(def.columns[0].default_value, "(1, 2)");
        assert!(!def.columns[0].nullable);
        assert_eq!(def.columns[1].default_value, "'x'");
        assert_eq!(def.columns[1].collation.as_deref(), Some("C"));
        assert_eq!(def.columns[1].ty, "text");
    }

    #[test]
    fn js_whitespace_is_not_rusts() {
        // U+0085 isn't JavaScript whitespace; U+FEFF is.
        assert_eq!(js_trim(&chars("\u{feff}a\u{85}")), &['a', '\u{85}']);
    }
}
