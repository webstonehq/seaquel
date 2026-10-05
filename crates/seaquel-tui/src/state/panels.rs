//! What panels 1–3 show, as the model holds it: the saved
//! connections and projects, the connected database's tables and views,
//! the project's saved queries and the connection's history. The runtime
//! builds these from Core's answers; `update` only reads and folds them.
//!
//! Every type that holds a name, a host or SQL has a hand-written `Debug`
//! showing ids and counts only (ground rules).

use std::collections::BTreeSet;
use std::fmt;

/// A saved project.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectItem {
    pub id: String,
    pub name: String,
}

impl fmt::Debug for ProjectItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProjectItem({})", self.id)
    }
}

/// How an SSH tunnel authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelAuth {
    Password,
    Key,
}

/// A saved connection's SSH tunnel, as panel 1 and the prompts need it.
#[derive(Clone, PartialEq, Eq)]
pub struct Tunnel {
    pub host: String,
    pub port: u16,
    pub auth: TunnelAuth,
}

impl fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tunnel({:?})", self.auth)
    }
}

/// A saved connection.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnItem {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// The row's `type`: `postgres`, `mysql`, `mariadb`, `sqlite`, `mssql`
    /// or `duckdb`.
    pub engine: String,
    pub host: String,
    pub port: Option<u16>,
    pub database: String,
    pub save_password: bool,
    pub save_ssh_password: bool,
    pub save_ssh_key_passphrase: bool,
    /// An enabled tunnel only.
    pub tunnel: Option<Tunnel>,
    pub label_ids: Vec<String>,
    /// What Ask AI may share and which model it calls.
    pub ai: ConnAi,
}

/// A connection's AI sharing after the global default (Core's rule,
/// `seaquel_core::ai::sharing`) and its chosen model, for Ask AI's title
/// and status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnAi {
    pub schema: bool,
    pub data: bool,
    pub model: Option<String>,
}

impl Default for ConnAi {
    /// Core's defaults: schema shared, data not.
    fn default() -> Self {
        ConnAi {
            schema: true,
            data: false,
            model: None,
        }
    }
}

impl fmt::Debug for ConnItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnItem")
            .field("id", &self.id)
            .field("engine", &self.engine)
            .field("save_password", &self.save_password)
            .field("tunnel", &self.tunnel)
            .finish_non_exhaustive()
    }
}

impl ConnItem {
    /// SQLite and DuckDB open a file: no password, no tunnel.
    pub fn is_file(&self) -> bool {
        matches!(self.engine.as_str(), "sqlite" | "duckdb")
    }

    /// The engine as panel 1 names it (`pg`, as the design does).
    pub fn engine_label(&self) -> &str {
        match self.engine.as_str() {
            "postgres" => "pg",
            other => other,
        }
    }

    /// Panel 1's place: the host (and port when not the default), or the
    /// file for SQLite and DuckDB.
    pub fn place(&self) -> String {
        if self.is_file() {
            return self.database.clone();
        }
        let default = match self.engine.as_str() {
            "postgres" => Some(5432),
            "mysql" | "mariadb" => Some(3306),
            "mssql" => Some(1433),
            _ => None,
        };
        match self.port {
            Some(port) if port != 0 && Some(port) != default => format!("{}:{port}", self.host),
            _ => self.host.clone(),
        }
    }
}

/// A project's own (custom) label, for the history snapshot an apply
/// records.
#[derive(Clone, PartialEq, Eq)]
pub struct LabelItem {
    pub project_id: String,
    pub id: String,
    pub name: String,
    pub color: String,
}

impl fmt::Debug for LabelItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LabelItem({})", self.id)
    }
}

/// What the TUI knows of the library.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Library {
    pub projects: Vec<ProjectItem>,
    pub connections: Vec<ConnItem>,
    /// Every project's custom labels.
    pub labels: Vec<LabelItem>,
    /// The app's AI settings turn the assistant off.
    pub ai_off: bool,
}

impl fmt::Debug for Library {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Library({} projects, {} connections)",
            self.projects.len(),
            self.connections.len()
        )
    }
}

impl Library {
    pub fn connection(&self, id: &str) -> Option<&ConnItem> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn project(&self, id: &str) -> Option<&ProjectItem> {
        self.projects.iter().find(|p| p.id == id)
    }

    /// A project's connections, in storage order.
    pub fn connections_of<'a>(&'a self, project_id: &'a str) -> impl Iterator<Item = &'a ConnItem> {
        self.connections
            .iter()
            .filter(move |c| c.project_id == project_id)
    }
}

/// A table, view or materialized view of the connected database.
#[derive(Clone, PartialEq, Eq)]
pub struct TableItem {
    pub schema: String,
    pub name: String,
    pub kind: TableKind,
    /// Core's approximate count.
    pub row_count: Option<i64>,
    /// `(name, type)`.
    pub columns: Vec<(String, String)>,
}

impl fmt::Debug for TableItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableItem")
            .field("kind", &self.kind)
            .field("columns", &self.columns.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    Table,
    View,
    MaterializedView,
}

/// A saved query of the active project.
#[derive(Clone, PartialEq, Eq)]
pub struct SavedItem {
    pub id: String,
    pub name: String,
    /// `None` and `""` are one (no folder).
    pub folder: Option<String>,
    /// Published to a shared project's repo (`sharedPath`).
    pub shared: bool,
    pub sql: String,
}

impl fmt::Debug for SavedItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedItem")
            .field("id", &self.id)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

/// A history row of the active connection, newest first.
#[derive(Clone, PartialEq)]
pub struct HistoryItem {
    pub id: String,
    /// Local time, as the runtime formatted it (`12:03:51`, or with the date
    /// when not today).
    pub when: String,
    pub sql: String,
    pub elapsed_ms: f64,
    pub rows: f64,
}

impl fmt::Debug for HistoryItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HistoryItem({})", self.id)
    }
}

/// One line of a folded list: a group (schema, folder) or an item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Group {
        name: String,
        count: usize,
        open: bool,
    },
    /// An index into the list's items.
    Item(usize),
}

/// Panel 2's rows for `kind` (tables, or views and materialized views):
/// every schema as a group in the order Core listed them, its items under
/// it unless folded.
pub fn table_rows(tables: &[TableItem], views: bool, folded: &BTreeSet<String>) -> Vec<Row> {
    let wanted = |t: &TableItem| (t.kind != TableKind::Table) == views;
    let mut schemas: Vec<&str> = Vec::new();
    for t in tables.iter().filter(|t| wanted(t)) {
        if !schemas.contains(&t.schema.as_str()) {
            schemas.push(&t.schema);
        }
    }
    let mut rows = Vec::new();
    for schema in schemas {
        let items: Vec<usize> = tables
            .iter()
            .enumerate()
            .filter(|(_, t)| wanted(t) && t.schema == schema)
            .map(|(i, _)| i)
            .collect();
        grouped(&mut rows, schema, items, folded);
    }
    rows
}

/// A group row, then its items unless folded.
fn grouped(rows: &mut Vec<Row>, name: &str, items: Vec<usize>, folded: &BTreeSet<String>) {
    let open = !folded.contains(name);
    rows.push(Row::Group {
        name: name.to_string(),
        count: items.len(),
        open,
    });
    if open {
        rows.extend(items.into_iter().map(Row::Item));
    }
}

/// Panel 3's saved rows: queries with no folder first, then each folder
/// (by name) as a group with its queries, unless folded.
pub fn saved_rows(saved: &[SavedItem], folded: &BTreeSet<String>) -> Vec<Row> {
    fn folder(s: &SavedItem) -> Option<&str> {
        s.folder.as_deref().filter(|f| !f.is_empty())
    }
    let mut rows: Vec<Row> = saved
        .iter()
        .enumerate()
        .filter(|(_, s)| folder(s).is_none())
        .map(|(i, _)| Row::Item(i))
        .collect();
    let folders: BTreeSet<&str> = saved.iter().filter_map(folder).collect();
    for name in folders {
        let items = saved
            .iter()
            .enumerate()
            .filter(|(_, s)| folder(s) == Some(name))
            .map(|(i, _)| i)
            .collect();
        grouped(&mut rows, name, items, folded);
    }
    rows
}

/// An approximate row count as panel 2 shows it: `86`, `1.3k`, `12.4k`,
/// `312k`, `1.2M`.
pub fn approx_count(n: i64) -> String {
    if n < 0 {
        return "?".to_string();
    }
    if n < 1_000 {
        return n.to_string();
    }
    let short = |value: f64, unit: &str| {
        if value < 100.0 {
            // One decimal, truncated (48,149 is 48.1k, never 48.2k).
            let tenths = (value * 10.0).floor() / 10.0;
            if tenths.fract() == 0.0 && value >= 10.0 {
                format!("{tenths:.0}{unit}")
            } else {
                format!("{tenths:.1}{unit}")
            }
        } else {
            format!("{:.0}{unit}", value.floor())
        }
    };
    if n < 1_000_000 {
        short(n as f64 / 1_000.0, "k")
    } else {
        short(n as f64 / 1_000_000.0, "M")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(schema: &str, name: &str, kind: TableKind) -> TableItem {
        TableItem {
            schema: schema.into(),
            name: name.into(),
            kind,
            row_count: None,
            columns: Vec::new(),
        }
    }

    fn saved(name: &str, folder: Option<&str>) -> SavedItem {
        SavedItem {
            id: format!("saved-{name}"),
            name: name.into(),
            folder: folder.map(Into::into),
            shared: false,
            sql: String::new(),
        }
    }

    #[test]
    fn tables_group_by_schema_and_fold() {
        let tables = [
            table("public", "customers", TableKind::Table),
            table("public", "active", TableKind::View),
            table("analytics", "events", TableKind::Table),
            table("public", "invoices", TableKind::Table),
            table("analytics", "mrr", TableKind::MaterializedView),
        ];
        let open = BTreeSet::new();
        assert_eq!(
            table_rows(&tables, false, &open),
            [
                Row::Group {
                    name: "public".into(),
                    count: 2,
                    open: true
                },
                Row::Item(0),
                Row::Item(3),
                Row::Group {
                    name: "analytics".into(),
                    count: 1,
                    open: true
                },
                Row::Item(2),
            ]
        );
        assert_eq!(
            table_rows(&tables, true, &open),
            [
                Row::Group {
                    name: "public".into(),
                    count: 1,
                    open: true
                },
                Row::Item(1),
                Row::Group {
                    name: "analytics".into(),
                    count: 1,
                    open: true
                },
                Row::Item(4),
            ]
        );
        let folded: BTreeSet<String> = ["public".to_string()].into();
        assert_eq!(
            table_rows(&tables, false, &folded),
            [
                Row::Group {
                    name: "public".into(),
                    count: 2,
                    open: false
                },
                Row::Group {
                    name: "analytics".into(),
                    count: 1,
                    open: true
                },
                Row::Item(2),
            ]
        );
        assert!(table_rows(&[], false, &open).is_empty());
    }

    #[test]
    fn saved_queries_list_loose_ones_then_folders() {
        let items = [
            saved("b", Some("reports")),
            saved("a", None),
            saved("c", Some("")),
            saved("d", Some("adhoc")),
        ];
        assert_eq!(
            saved_rows(&items, &BTreeSet::new()),
            [
                Row::Item(1),
                Row::Item(2),
                Row::Group {
                    name: "adhoc".into(),
                    count: 1,
                    open: true
                },
                Row::Item(3),
                Row::Group {
                    name: "reports".into(),
                    count: 1,
                    open: true
                },
                Row::Item(0),
            ]
        );
        let folded: BTreeSet<String> = ["reports".to_string()].into();
        assert_eq!(saved_rows(&items, &folded).len(), 5);
    }

    #[test]
    fn counts_are_short() {
        for (n, text) in [
            (0, "0"),
            (86, "86"),
            (999, "999"),
            (1_300, "1.3k"),
            (12_400, "12.4k"),
            (48_149, "48.1k"),
            (312_000, "312k"),
            (999_999, "999k"),
            (1_250_000, "1.2M"),
            (31_000_000, "31M"),
            (-1, "?"),
        ] {
            assert_eq!(approx_count(n), text, "{n}");
        }
    }

    #[test]
    fn panel_one_names_the_engine_and_place() {
        let mut c = ConnItem {
            id: "conn-1".into(),
            project_id: "p".into(),
            name: "prod".into(),
            engine: "postgres".into(),
            host: "db.internal".into(),
            port: Some(5432),
            database: "app".into(),
            save_password: true,
            save_ssh_password: false,
            save_ssh_key_passphrase: false,
            tunnel: None,
            label_ids: Vec::new(),
            ai: Default::default(),
        };
        assert_eq!(
            (c.engine_label(), c.place().as_str()),
            ("pg", "db.internal")
        );
        c.port = Some(6543);
        assert_eq!(c.place(), "db.internal:6543");
        c.engine = "mariadb".into();
        c.port = Some(3306);
        assert_eq!(
            (c.engine_label(), c.place().as_str()),
            ("mariadb", "db.internal")
        );
        c.engine = "sqlite".into();
        c.database = "/tmp/app.db".into();
        assert_eq!(
            (c.engine_label(), c.place().as_str()),
            ("sqlite", "/tmp/app.db")
        );
        assert!(c.is_file());
        let text = format!("{c:?}");
        assert!(
            !text.contains("prod") && !text.contains("db.internal"),
            "{text}"
        );
    }
}
