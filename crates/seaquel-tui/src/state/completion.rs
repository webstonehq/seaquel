//! The editor's completion popup: table names after `FROM`
//! and `JOIN`, `alias.` columns, then keywords from a short fixed list,
//! from the `schema_tables` panel 2 already read. Which alias names which
//! table comes from a scan of the statement's `FROM`/`JOIN` words with
//! `seaquel_core::sql`'s tokenizer (the text being typed rarely parses, so
//! the AST's column references can't say). The popup is advice: what it
//! inserts is a name, never SQL that runs.

use std::collections::HashMap;
use std::fmt;

use ratatui::layout::Rect;
use seaquel_core::sql::scan::{statement_at, tokens, Token, TokenKind};
use seaquel_core::sql::SqlEngine;
use unicode_width::UnicodeWidthStr;

use super::panels::TableItem;

/// The most items the popup lists.
pub const MAX_ITEMS: usize = 50;
/// The most rows it shows at once.
pub const POPUP_ROWS: usize = 8;

/// Keywords the popup offers (a short list).
pub const COMPLETION_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "ON",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS NULL",
    "IS NOT NULL",
    "IN",
    "LIKE",
    "AS",
    "DISTINCT",
    "GROUP BY",
    "ORDER BY",
    "HAVING",
    "LIMIT",
    "OFFSET",
    "INSERT INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE FROM",
    "WITH",
    "UNION",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "DESC",
    "ASC",
    "COUNT",
    "RETURNING",
];

/// What an item names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Table,
    Column,
    Keyword,
    /// Ask AI's `@` list.
    SavedQuery,
    Dashboard,
}

/// One line of the popup: `issued_at  date`.
#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub detail: String,
    pub kind: ItemKind,
}

impl fmt::Debug for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Item({:?})", self.kind)
    }
}

/// The open popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub items: Vec<Item>,
    pub selected: usize,
    /// The line and character column the typed prefix starts at.
    pub row: usize,
    pub col: usize,
    /// How many characters of prefix an accepted item replaces.
    pub prefix_chars: usize,
}

/// The items for the cursor at byte `cursor` of `text`, and where the typed
/// prefix starts (a byte offset). `None` when there's nothing to offer:
/// no schema read yet, or (unless `explicit`, as Tab and Ctrl+Space are)
/// no `alias.` right before the cursor.
pub fn candidates(
    text: &str,
    cursor: usize,
    engine: SqlEngine,
    schema: Option<&[TableItem]>,
    explicit: bool,
) -> Option<(usize, Vec<Item>)> {
    let schema = schema?;
    let cursor = seaquel_core::sql::offsets::floor_char_boundary(text, cursor);
    let before = &text[..cursor];
    let prefix_len: usize = before
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .map(char::len_utf8)
        .sum();
    let start = cursor - prefix_len;
    let prefix = &text[start..cursor];
    // Inside a string or a comment there's nothing to complete.
    if in_string_or_comment(text, start, engine) {
        return None;
    }
    let lower = prefix.to_lowercase();
    let matches = |name: &str| name.to_lowercase().starts_with(&lower);

    // `alias.`: the qualifier before the dot.
    if let Some(head) = text[..start].strip_suffix('.') {
        let qualifier = qualifier_before(head);
        if qualifier.is_empty() {
            return None;
        }
        let statement = statement_at(text, cursor, engine)?;
        let range = statement.start.min(cursor)..statement.end.max(cursor).min(text.len());
        let map = aliases(&text[range], engine);
        let key = qualifier.to_lowercase();
        let aliased = map
            .get(&key)
            .and_then(|(schema_name, table)| find_table(schema, schema_name.as_deref(), table));
        let items: Vec<Item> = if let Some(found) = aliased {
            found
                .columns
                .iter()
                .filter(|(name, _)| matches(name))
                .map(|(name, ty)| Item {
                    label: name.clone(),
                    detail: ty.clone(),
                    kind: ItemKind::Column,
                })
                .collect()
        } else {
            // A schema's tables.
            let mut tables: Vec<&TableItem> = schema
                .iter()
                .filter(|t| t.schema.eq_ignore_ascii_case(&qualifier) && matches(&t.name))
                .collect();
            tables.sort_by(|a, b| a.name.cmp(&b.name));
            tables.into_iter().map(table_item).collect()
        };
        return non_empty(start, items);
    }
    // No prefix: nothing (Tab then indents), unless right after a `.`.
    if !explicit || prefix.is_empty() {
        return None;
    }
    let previous = previous_word(text, start, engine);
    let tables_only = matches!(
        previous.as_deref(),
        Some("FROM" | "JOIN" | "UPDATE" | "INTO" | "TABLE")
    );
    let mut tables: Vec<&TableItem> = schema.iter().filter(|t| matches(&t.name)).collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name).then(a.schema.cmp(&b.schema)));
    let mut items: Vec<Item> = tables.into_iter().map(table_item).collect();
    if !tables_only && !prefix.is_empty() {
        items.extend(
            COMPLETION_KEYWORDS
                .iter()
                .filter(|k| matches(k))
                .map(|k| Item {
                    label: k.to_string(),
                    detail: String::new(),
                    kind: ItemKind::Keyword,
                }),
        );
    }
    non_empty(start, items)
}

/// The table an `alias.` right before the cursor names (its index in
/// `schema`), when it's one panel 2 lists: completion then needs its
/// columns. `None` inside a string or a comment, or when the
/// qualifier names no table.
pub fn alias_target(
    text: &str,
    cursor: usize,
    engine: SqlEngine,
    schema: &[TableItem],
) -> Option<usize> {
    let cursor = seaquel_core::sql::offsets::floor_char_boundary(text, cursor);
    let prefix_len: usize = text[..cursor]
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .map(char::len_utf8)
        .sum();
    let start = cursor - prefix_len;
    if in_string_or_comment(text, start, engine) {
        return None;
    }
    let head = text[..start].strip_suffix('.')?;
    let qualifier = qualifier_before(head);
    if qualifier.is_empty() {
        return None;
    }
    let statement = statement_at(text, cursor, engine)?;
    let range = statement.start.min(cursor)..statement.end.max(cursor).min(text.len());
    let map = aliases(&text[range], engine);
    let (schema_name, table) = map.get(&qualifier.to_lowercase())?;
    let found = find_table(schema, schema_name.as_deref(), table)?;
    schema.iter().position(|t| std::ptr::eq(t, found))
}

/// Whether byte `at` is inside a string literal or a comment as `engine`
/// reads it (the scanner's tokens; an unterminated one runs to the end).
pub fn in_string_or_comment(text: &str, at: usize, engine: SqlEngine) -> bool {
    use seaquel_core::sql::scan::{scan, ScanOptions};
    let options = ScanOptions {
        exec_comments: true,
        ..ScanOptions::default()
    };
    scan(text, engine, options).iter().any(|t| {
        let token = t.text(text);
        let string = t.kind == TokenKind::Quoted
            && quoted_class(token, engine) == super::editor::Class::String;
        if t.kind != TokenKind::Comment && !string {
            return false;
        }
        t.start < at && (at < t.end || (at == t.end && !closed(token, t.kind)))
    })
}

/// A string's or a quoted name's class as the editor colours it.
fn quoted_class(token: &str, engine: SqlEngine) -> super::editor::Class {
    use super::editor::Class;
    match token.chars().next().unwrap_or('\'') {
        '"' if engine.is_mysql() => Class::String,
        '"' | '`' | '[' => Class::Name,
        _ => Class::String,
    }
}

/// Whether a string or comment token ends with its closing delimiter (an
/// unterminated one runs to the end of the text).
fn closed(token: &str, kind: TokenKind) -> bool {
    if kind == TokenKind::Comment {
        return if token.starts_with("/*") {
            token.len() >= 4 && token.ends_with("*/")
        } else {
            token.ends_with('\n') || token.ends_with('\r')
        };
    }
    if let Some(rest) = token.strip_prefix('$') {
        // `$tag$…$tag$`.
        let tag_end = rest.find('$').map_or(0, |i| i + 2);
        let tag = &token[..tag_end];
        return !tag.is_empty() && token.len() >= 2 * tag.len() && token.ends_with(tag);
    }
    let quote = token
        .chars()
        .find(|c| matches!(c, '\'' | '"'))
        .unwrap_or('\'');
    let open = token.find(quote).unwrap_or(0);
    token.len() > open + 1 && token.ends_with(quote)
}

fn non_empty(start: usize, mut items: Vec<Item>) -> Option<(usize, Vec<Item>)> {
    items.truncate(MAX_ITEMS);
    (!items.is_empty()).then_some((start, items))
}

fn table_item(t: &TableItem) -> Item {
    Item {
        label: t.name.clone(),
        detail: t.schema.clone(),
        kind: ItemKind::Table,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The name right before a `.`: a word, or a quoted name's contents.
fn qualifier_before(head: &str) -> String {
    if let Some(inner) = head.strip_suffix('"').or_else(|| head.strip_suffix('`')) {
        let open = head.as_bytes()[head.len() - 1] as char;
        if let Some(i) = inner.rfind(open) {
            return inner[i + 1..].to_string();
        }
    }
    if let Some(inner) = head.strip_suffix(']') {
        if let Some(i) = inner.rfind('[') {
            return inner[i + 1..].to_string();
        }
    }
    let len: usize = head
        .chars()
        .rev()
        .take_while(|c| is_word_char(*c))
        .map(char::len_utf8)
        .sum();
    head[head.len() - len..].to_string()
}

/// The upper-cased word before byte `at` in its statement, if a word comes
/// right before it.
fn previous_word(text: &str, at: usize, engine: SqlEngine) -> Option<String> {
    let statement = statement_at(text, at, engine);
    let from = statement.map_or(0, |s| s.start.min(at));
    let piece = &text[from..at];
    let last = tokens(piece, engine).into_iter().last()?;
    (last.kind == TokenKind::Word).then(|| last.text(piece).to_ascii_uppercase())
}

/// The listed table `table` (in `schema`, when given), ignoring case.
fn find_table<'a>(
    tables: &'a [TableItem],
    schema: Option<&str>,
    table: &str,
) -> Option<&'a TableItem> {
    tables.iter().find(|t| {
        t.name.eq_ignore_ascii_case(table)
            && schema.is_none_or(|s| t.schema.eq_ignore_ascii_case(s))
    })
}

/// A name token's text, unquoted.
fn unquote(token: &Token, sql: &str) -> String {
    let text = token.text(sql);
    if token.kind != TokenKind::Quoted || text.len() < 2 {
        return text.to_string();
    }
    let (open, close) = (text.as_bytes()[0], text.as_bytes()[text.len() - 1]);
    let inner = &text[1..text.len() - 1];
    match (open, close) {
        (b'"', b'"') => inner.replace("\"\"", "\""),
        (b'`', b'`') => inner.replace("``", "`"),
        (b'[', b']') => inner.replace("]]", "]"),
        _ => text.to_string(),
    }
}

fn is_name(token: &Token, sql: &str) -> bool {
    match token.kind {
        TokenKind::Word => !super::editor::is_keyword(token.text(sql)),
        TokenKind::Quoted => matches!(token.text(sql).as_bytes()[0], b'"' | b'`' | b'['),
        _ => false,
    }
}

fn is_punct(token: Option<&Token>, sql: &str, p: &str) -> bool {
    token.is_some_and(|t| t.kind == TokenKind::Punct && t.text(sql) == p)
}

/// Which table each alias (and each table's own name) in the statement
/// names, lower-cased: `(schema, table)`.
pub fn aliases(statement: &str, engine: SqlEngine) -> HashMap<String, (Option<String>, String)> {
    let toks = tokens(statement, engine);
    let mut map = HashMap::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let starts_list = t.kind == TokenKind::Word
            && matches!(
                t.text(statement).to_ascii_uppercase().as_str(),
                "FROM" | "JOIN" | "UPDATE" | "INTO"
            );
        i += 1;
        if !starts_list {
            continue;
        }
        // One or more `name[.name] [AS] [alias]`, comma separated.
        while let Some(first) = toks.get(i).filter(|t| is_name(t, statement)) {
            let mut parts = vec![unquote(first, statement)];
            i += 1;
            while is_punct(toks.get(i), statement, ".")
                && toks.get(i + 1).is_some_and(|t| is_name(t, statement))
            {
                parts.push(unquote(&toks[i + 1], statement));
                i += 2;
            }
            let table = parts.pop().unwrap_or_default();
            let schema = parts.pop();
            let target = (schema, table.clone());
            map.insert(table.to_lowercase(), target.clone());
            if toks
                .get(i)
                .is_some_and(|t| t.text(statement).eq_ignore_ascii_case("AS"))
            {
                i += 1;
            }
            if let Some(alias) = toks.get(i).filter(|t| is_name(t, statement)) {
                map.insert(unquote(alias, statement).to_lowercase(), target);
                i += 1;
            }
            if !is_punct(toks.get(i), statement, ",") {
                break;
            }
            i += 1;
        }
    }
    map
}

/// Where the popup goes: under the prefix at `anchor` (a screen cell),
/// above it when there's no room below, kept within `bounds`.
pub fn popup_area(bounds: Rect, anchor: (u16, u16), items: &[Item]) -> Rect {
    let width = popup_width(items).min(bounds.width);
    let height = (items.len().min(POPUP_ROWS) as u16 + 2).min(bounds.height);
    let (x, y) = anchor;
    // One left of the prefix, for the border, and shifted left to fit.
    let x = x
        .saturating_sub(1)
        .max(bounds.x)
        .min((bounds.x + bounds.width).saturating_sub(width));
    let below = y + 1;
    let y = if below + height <= bounds.y + bounds.height {
        below
    } else {
        y.saturating_sub(height).max(bounds.y)
    };
    Rect::new(x, y, width, height)
}

/// The popup's width: the widest label and detail, two columns apart, plus
/// its border.
pub fn popup_width(items: &[Item]) -> u16 {
    let label = items.iter().map(|i| i.label.width()).max().unwrap_or(0);
    let detail = items.iter().map(|i| i.detail.width()).max().unwrap_or(0);
    (label + 2 + detail + 2).min(60) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::panels::TableKind;

    fn table(schema: &str, name: &str, columns: &[(&str, &str)]) -> TableItem {
        TableItem {
            schema: schema.into(),
            name: name.into(),
            kind: TableKind::Table,
            row_count: Some(1),
            columns: columns
                .iter()
                .map(|(n, t)| (n.to_string(), t.to_string()))
                .collect(),
        }
    }

    fn schema() -> Vec<TableItem> {
        vec![
            table(
                "public",
                "invoices",
                &[
                    ("id", "int8"),
                    ("issued_at", "date"),
                    ("issuer_id", "int8"),
                    ("is_subscription", "bool"),
                    ("total", "numeric"),
                ],
            ),
            table("public", "customers", &[("id", "int8"), ("name", "text")]),
            table(
                "public",
                "invoice_line_items",
                &[("invoice_id", "int8"), ("quantity", "int4")],
            ),
            table("analytics", "events", &[("at", "timestamptz")]),
        ]
    }

    fn labels(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    /// The candidates with the cursor at the `|` in `text`.
    fn at(text: &str, explicit: bool) -> Option<(usize, Vec<Item>)> {
        let cursor = text.find('|').unwrap();
        let text = text.replace('|', "");
        let s = schema();
        candidates(&text, cursor, SqlEngine::Postgres, Some(&s), explicit)
    }

    // Design 1b: `i.iss_` lists `issued_at date`, `issuer_id int8`, …
    #[test]
    fn alias_dot_lists_the_aliased_table_s_columns() {
        let sql = "SELECT c.name\nFROM   invoices i\nJOIN   customers c ON c.id = i.customer_id\nWHERE  i.is|\nGROUP BY 1";
        let (start, items) = at(sql, false).unwrap();
        assert_eq!(
            labels(&items),
            ["issued_at", "issuer_id", "is_subscription"]
        );
        assert_eq!(items[0].detail, "date");
        assert_eq!(items[0].kind, ItemKind::Column);
        assert_eq!(&sql.replace('|', "")[start..start + 2], "is");
        // Right after the dot, every column; `c.` the other table's.
        let (_, items) = at("SELECT c.| FROM customers AS c", false).unwrap();
        assert_eq!(labels(&items), ["id", "name"]);
        let (_, items) = at("SELECT x FROM invoices i, customers c WHERE c.n|", false).unwrap();
        assert_eq!(labels(&items), ["name"]);
        // A table's own name works as a qualifier, a schema lists its tables.
        let (_, items) = at("SELECT invoices.to| FROM public.invoices", false).unwrap();
        assert_eq!(labels(&items), ["total"]);
        let (_, items) = at("SELECT * FROM analytics.|", false).unwrap();
        assert_eq!(labels(&items), ["events"]);
        // Another statement's aliases don't count.
        assert!(at("SELECT 1 FROM invoices i; SELECT i.|", false).is_none());
    }

    #[test]
    fn tables_after_from_and_join_then_keywords() {
        let (_, items) = at("SELECT * FROM inv|", true).unwrap();
        assert_eq!(labels(&items), ["invoice_line_items", "invoices"]);
        assert!(items.iter().all(|i| i.kind == ItemKind::Table));
        assert_eq!(items[1].detail, "public");
        let (_, items) = at("SELECT * FROM invoices JOIN cu|", true).unwrap();
        assert_eq!(labels(&items), ["customers"]);
        // Elsewhere: tables first, then keywords.
        let (_, items) = at("SELECT * FROM t WHERE x = 1 o|", true).unwrap();
        assert_eq!(labels(&items), ["ON", "OR", "ORDER BY", "OFFSET"]);
        let (_, items) = at("sel|", true).unwrap();
        assert_eq!(labels(&items), ["SELECT"]);
        let (_, items) = at("SELECT i|", true).unwrap();
        assert_eq!(items[0].kind, ItemKind::Table);
        assert!(items.iter().any(|i| i.kind == ItemKind::Keyword));
    }

    #[test]
    fn nothing_without_a_schema_or_a_prompt() {
        let text = "SELECT i. FROM invoices i";
        assert!(candidates(text, 9, SqlEngine::Postgres, None, true).is_none());
        // Typing a word doesn't open it; Tab does.
        assert!(at("SELECT * FROM inv|", false).is_none());
        assert!(at("SELECT * FROM zzz|", true).is_none(), "nothing matches");
    }

    #[test]
    fn aliases_from_from_and_join_with_as_and_quotes() {
        let a = aliases(
            "SELECT 1 FROM public.invoices AS i, items JOIN \"Customers\" c ON true",
            SqlEngine::Postgres,
        );
        assert_eq!(a["i"], (Some("public".into()), "invoices".into()));
        assert_eq!(a["invoices"], (Some("public".into()), "invoices".into()));
        assert_eq!(a["c"], (None, "Customers".into()));
        assert_eq!(a["items"], (None, "items".into()));
        assert!(!a.contains_key("on"), "{a:?}");
        let a = aliases("UPDATE `orders` o SET x = 1", SqlEngine::Mysql);
        assert_eq!(a["o"], (None, "orders".into()));
    }

    // The popup sits under the prefix, or above when there's no room.
    #[test]
    fn the_popup_follows_the_cursor() {
        let items: Vec<Item> = ["issued_at", "issuer_id", "is_subscription"]
            .iter()
            .map(|l| Item {
                label: l.to_string(),
                detail: "int8".into(),
                kind: ItemKind::Column,
            })
            .collect();
        let bounds = Rect::new(49, 1, 99, 14);
        let r = popup_area(bounds, (60, 7), &items);
        assert_eq!((r.x, r.y), (59, 8), "one row down, one left for the border");
        assert_eq!(r.height, 5, "three items and the border");
        assert_eq!(r.width, popup_width(&items));
        // Near the bottom: above the line.
        let r = popup_area(bounds, (60, 13), &items);
        assert_eq!(r.y + r.height, 13);
        // Near the right edge: shifted left to fit.
        let r = popup_area(bounds, (146, 3), &items);
        assert!(r.x + r.width <= bounds.x + bounds.width, "{r:?}");
        // Many items: at most POPUP_ROWS show.
        let many: Vec<Item> = (0..30).map(|_| items[0].clone()).collect();
        let r = popup_area(Rect::new(0, 0, 148, 42), (10, 2), &many);
        assert_eq!(r.height as usize, POPUP_ROWS + 2);
    }
}
