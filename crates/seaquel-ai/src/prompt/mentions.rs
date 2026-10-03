//! `@mentions` (`services/ai-mentions.ts` before phase 6): each `@token` or
//! `@"name with spaces"` that names a table, a saved query or a dashboard
//! adds a block to a "Referenced context" section after the message.
//!
//! Without schema sharing a mention resolves to its bare name: the message
//! goes as typed (Decision 5, bug 8), since a table's columns, a saved
//! query's SQL and a dashboard's queries all describe the schema.
//!
//! At most [`MAX_MENTIONS`] distinct mentions are resolved per message
//! (Decision 33); the rest stay as typed. Names are looked up in maps built
//! once per message, so a long message over a large schema stays linear.

use std::collections::{HashMap, HashSet};

use seaquel_types::SchemaTable;
use serde::Deserialize;

use crate::tools::saved::is_js_space;

/// The most distinct mentions one message resolves; tokens past them stay
/// as typed.
pub const MAX_MENTIONS: usize = 100;

/// A saved query a mention may name.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct MentionQuery {
    pub name: String,
    pub query: String,
}

/// A dashboard a mention may name.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionDashboard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub widgets: Vec<MentionWidget>,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionWidget {
    pub id: String,
    pub title: String,
    pub widget_type: String,
    #[serde(default)]
    pub query: Option<String>,
}

/// `content` with its mentions' context appended, or as typed when nothing
/// resolves or the connection doesn't share its schema.
pub fn mentions(
    content: &str,
    share_schema: bool,
    tables: &[SchemaTable],
    queries: &[MentionQuery],
    dashboards: &[MentionDashboard],
) -> String {
    if !share_schema {
        return content.to_string();
    }
    let index = Index::new(tables, queries, dashboards);
    let mut seen: HashSet<String> = HashSet::new();
    let mut blocks: Vec<String> = Vec::new();
    for token in tokens(content) {
        if seen.len() >= MAX_MENTIONS {
            break;
        }
        let key = token.to_lowercase();
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(table) = index.table(&key) {
            blocks.push(table_context(table));
        } else if let Some(q) = index.queries.get(&key) {
            blocks.push(format!("Saved query: {}\n```sql\n{}\n```", q.name, q.query));
        } else if let Some(d) = index.dashboards.get(&key) {
            blocks.push(dashboard_context(d));
        }
    }
    if blocks.is_empty() {
        return content.to_string();
    }
    format!("{content}\n\nReferenced context:\n{}", blocks.join("\n\n"))
}

/// What a mention may name, by lowercased name, built once per message.
/// Each map keeps the first item with a name, as the TypeScript's `find`
/// did.
struct Index<'a> {
    by_qualified: HashMap<String, &'a SchemaTable>,
    by_name: HashMap<String, &'a SchemaTable>,
    queries: HashMap<String, &'a MentionQuery>,
    dashboards: HashMap<String, &'a MentionDashboard>,
}

impl<'a> Index<'a> {
    fn new(
        tables: &'a [SchemaTable],
        queries: &'a [MentionQuery],
        dashboards: &'a [MentionDashboard],
    ) -> Self {
        let mut index = Index {
            by_qualified: HashMap::new(),
            by_name: HashMap::new(),
            queries: HashMap::new(),
            dashboards: HashMap::new(),
        };
        for t in tables {
            index
                .by_qualified
                .entry(format!("{}.{}", t.schema, t.name).to_lowercase())
                .or_insert(t);
            index.by_name.entry(t.name.to_lowercase()).or_insert(t);
        }
        for q in queries {
            index.queries.entry(q.name.to_lowercase()).or_insert(q);
        }
        for d in dashboards {
            index.dashboards.entry(d.name.to_lowercase()).or_insert(d);
        }
        index
    }

    /// A table by `schema.name`, else by name alone. `key` is lowercase.
    fn table(&self, key: &str) -> Option<&'a SchemaTable> {
        self.by_qualified
            .get(key)
            .or_else(|| self.by_name.get(key))
            .copied()
    }
}

/// The tokens of `/@"([^"]+)"|@(\S+)/g`, in order: a quoted name, else the
/// run of non-whitespace after `@` (JavaScript's whitespace).
fn tokens(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(at) = content[i..].find('@') {
        let start = i + at + 1;
        let after = &content[start..];
        if let Some(quoted) = after.strip_prefix('"') {
            if let Some(end) = quoted.find('"').filter(|end| *end > 0) {
                out.push(&quoted[..end]);
                i = start + 1 + end + 1;
                continue;
            }
        }
        let len = after
            .char_indices()
            .find(|(_, c)| is_js_space(*c))
            .map_or(after.len(), |(k, _)| k);
        if len > 0 {
            out.push(&after[..len]);
        }
        i = start + len;
    }
    out
}

fn table_context(table: &SchemaTable) -> String {
    let kind = serde_json::to_value(table.kind)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default();
    let mut lines = vec![format!("Table: {}.{} ({kind})", table.schema, table.name)];
    if !table.columns.is_empty() {
        lines.push("Columns:".into());
        for col in &table.columns {
            let mut flags = Vec::new();
            if col.is_primary_key {
                flags.push("PK");
            }
            if col.is_foreign_key {
                flags.push("FK");
            }
            if !col.nullable {
                flags.push("NOT NULL");
            }
            let mut line = format!("  - {}: {}", col.name, col.ty);
            if !flags.is_empty() {
                line.push_str(&format!(" [{}]", flags.join(", ")));
            }
            if let Some(r) = col.foreign_key_ref.as_ref().filter(|_| col.is_foreign_key) {
                line.push_str(&format!(
                    " -> {}.{}",
                    r.referenced_table, r.referenced_column
                ));
            }
            lines.push(line);
        }
    }
    lines.join("\n")
}

fn dashboard_context(d: &MentionDashboard) -> String {
    let mut lines = vec![format!("Dashboard: {} (id: {})", d.name, d.id)];
    if !d.widgets.is_empty() {
        lines.push("Widgets:".into());
        for w in &d.widgets {
            lines.push(format!(
                "  - {} [id: {}] ({})",
                w.title, w.id, w.widget_type
            ));
            if let Some(q) = w.query.as_deref().filter(|q| !q.is_empty()) {
                lines.push(format!("    Query: {q}"));
            }
        }
    }
    lines.join("\n")
}
