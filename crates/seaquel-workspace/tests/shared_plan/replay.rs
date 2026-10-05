//! The projection replay through the pure planner.
//!
//! **What it checks.** Each case of `projection.json` runs on an in-memory
//! model of what Core holds: the rows (the fixture's raw SQLite shape),
//! the 0004 link columns beside them, and the disk (files, symlinks,
//! unreadable directories, failing paths, a case-insensitive mode, git
//! pulls that change files or leave conflicts). Each step's `core` calls
//! drive it the way Task 5's Core will, with every decision taken by
//! `seaquel_workspace::shared`: a `shared.sync` scans the model's disk and
//! applies [`plan_sync`]'s rows, links, files and notices; a library call
//! applies the library's own patch functions, then [`plan_publish`]; link,
//! unlink and import use [`pick_project_dir`], `plan_publish` and
//! `plan_sync`. After every step it compares, against the recording with
//! `changes.json` applied:
//!
//! - the **rows** of every table but `app_state` (`shared_repos` by `id`,
//!   `path` and `name`; ids and `<core:n>` tokens bound to the model's
//!   uuids, the same everywhere in a case), versions included;
//! - the **files** (Core's id line or key checked as a v4 uuid that stays
//!   with its row, then removed; files Core didn't write byte for byte);
//! - the step's **links** (`projects.shared_dir`, `shared_path`,
//!   `shared_file_id`, `shared_connection_id`), **notices** (exactly, as a
//!   set, after the once-per-session rule), **projection** and **outcome**
//!   where `changes.json` lists them;
//! - the **bases**: `*` (4a), every linked row's base and file id against
//!   the file on the model's disk, and (4b) every shared row's path.
//!
//! What it doesn't check is Core's: the repo lock, events and `seq`, real
//! symlinks and `O_NOFOLLOW`, git itself, the keychain, and the order of
//! row and file writes against a crash (Task 5's tests).

use std::collections::{BTreeMap, HashMap, HashSet};

use seaquel_types::storage::{PersistedConnection, PersistedDashboard, PersistedSavedQuery};
use seaquel_workspace::library::{
    apply_connection_patch, apply_project_patch, apply_saved_query_patch, check_connection_draft,
    check_connection_patch, check_saved_query_draft, check_saved_query_patch,
    connection_from_draft, free_name, name_key, saved_query_from_draft, ConnectionDraft,
    ConnectionPatch, LibraryLimits, ProjectPatch, SavedQueryDraft, SavedQueryPatch,
};
use seaquel_workspace::shared::file_hash;
use seaquel_workspace::shared::format::{
    parse_dashboard, parse_project, parse_query, parse_template,
};
use seaquel_workspace::shared::names::{legacy_stem, path_key};
use seaquel_workspace::shared::plan::{row_hash, template_path, Limits, NoticeMemory};
use seaquel_workspace::shared::{
    pick_project_dir, plan_publish, plan_sync, DirScan, FileOp, Kind, Link, LinkUpdate,
    LinkedConnection, LinkedDashboard, LinkedQuery, ProjectLink, PublishContext, PublishPlan,
    RawFile, RowChange, RowOp, SharedRows, SkipReason, Skipped,
};
use seaquel_workspace::state::{
    apply_dashboard_patch, check_dashboard_draft, check_dashboard_patch, dashboard_from_draft,
    dashboard_snapshot, DashboardDraft, DashboardPatch, StateLimits,
};
use serde_json::value::RawValue;
use serde_json::{json, Map, Value};

use super::Ids;

/// A v4-shaped uuid from a counter.
pub fn uuid(n: u32) -> String {
    format!("{n:08x}-0000-4000-8000-{n:012x}")
}

fn is_v4(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 36
        && [8, 13, 18, 23].iter().all(|&i| b[i] == b'-')
        && b[14] == b'4'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
        && s.chars()
            .enumerate()
            .all(|(i, c)| [8, 13, 18, 23].contains(&i) || c.is_ascii_hexdigit())
}

/// A small deterministic generator for the property tests.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const NOW: &str = "<now>";

type Row = Map<String, Value>;

#[derive(Clone, PartialEq)]
enum Node {
    File(String),
    Link(String),
}

fn s(r: &Row, k: &str) -> String {
    r.get(k)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn os(r: &Row, k: &str) -> Option<String> {
    r.get(k).and_then(Value::as_str).map(str::to_string)
}

fn b(r: &Row, k: &str) -> bool {
    r.get(k).and_then(Value::as_i64).unwrap_or(0) != 0
}

fn raw_json(text: Option<String>) -> Option<Box<RawValue>> {
    text.and_then(|t| RawValue::from_string(t).ok())
}

fn int(v: bool) -> Value {
    json!(i64::from(v))
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| json!(s))
}

fn sq_from(r: &Row) -> PersistedSavedQuery {
    PersistedSavedQuery {
        id: s(r, "id"),
        name: s(r, "name"),
        query: s(r, "query"),
        project_id: s(r, "project_id"),
        created_at: s(r, "created_at"),
        updated_at: s(r, "updated_at"),
        parameters: raw_json(os(r, "parameters")),
        starred: b(r, "starred"),
        shared: b(r, "shared"),
        description: os(r, "description"),
        database_type: os(r, "database_type"),
        tags: raw_json(os(r, "tags")),
        folder: os(r, "folder"),
        shared_path: None,
    }
}

fn sq_into(p: &PersistedSavedQuery, r: &mut Row) {
    r.insert("id".into(), json!(p.id));
    r.insert("project_id".into(), json!(p.project_id));
    r.insert("name".into(), json!(p.name));
    r.insert("query".into(), json!(p.query));
    r.insert(
        "parameters".into(),
        opt(&p.parameters.as_ref().map(|v| v.get().to_string())),
    );
    r.insert("starred".into(), int(p.starred));
    r.insert("shared".into(), int(p.shared));
    r.insert("description".into(), opt(&p.description));
    r.insert("database_type".into(), opt(&p.database_type));
    r.insert(
        "tags".into(),
        opt(&p.tags.as_ref().map(|v| v.get().to_string())),
    );
    r.insert("folder".into(), opt(&p.folder));
    r.insert("created_at".into(), json!(p.created_at));
    r.insert("updated_at".into(), json!(p.updated_at));
}

fn dash_from(r: &Row) -> PersistedDashboard {
    PersistedDashboard {
        id: s(r, "id"),
        project_id: s(r, "project_id"),
        name: s(r, "name"),
        viewport: s(r, "viewport"),
        widgets: s(r, "widgets"),
        date_filter: os(r, "date_filter"),
        starred: b(r, "starred"),
        shared: b(r, "shared"),
        description: os(r, "description"),
        created_at: s(r, "created_at"),
        updated_at: s(r, "updated_at"),
        shared_path: None,
    }
}

fn dash_into(p: &PersistedDashboard, r: &mut Row) {
    r.insert("id".into(), json!(p.id));
    // A beta-era row in no project keeps its NULL.
    if !(p.project_id.is_empty() && r.get("project_id") == Some(&Value::Null)) {
        r.insert("project_id".into(), json!(p.project_id));
    }
    r.insert("name".into(), json!(p.name));
    r.insert("viewport".into(), json!(p.viewport));
    r.insert("widgets".into(), json!(p.widgets));
    r.insert("date_filter".into(), opt(&p.date_filter));
    r.insert("starred".into(), int(p.starred));
    r.insert("shared".into(), int(p.shared));
    r.insert("description".into(), opt(&p.description));
    r.insert("created_at".into(), json!(p.created_at));
    r.insert("updated_at".into(), json!(p.updated_at));
}

const CONN_COLUMNS: [(&str, &str); 21] = [
    ("id", "id"),
    ("project_id", "projectId"),
    ("name", "name"),
    ("type", "type"),
    ("host", "host"),
    ("port", "port"),
    ("database_name", "databaseName"),
    ("username", "username"),
    ("ssl_mode", "sslMode"),
    ("connection_string", "connectionString"),
    ("last_connected", "lastConnected"),
    ("ssh_tunnel", "sshTunnel"),
    ("save_password", "savePassword"),
    ("save_ssh_password", "saveSshPassword"),
    ("save_ssh_key_passphrase", "saveSshKeyPassphrase"),
    ("is_local_only", "isLocalOnly"),
    ("shared_connection_id", "sharedConnectionId"),
    ("ai_share_schema", "aiShareSchema"),
    ("ai_share_data", "aiShareData"),
    ("active_ai_provider_id", "activeAIProviderId"),
    ("active_ai_model", "activeAIModel"),
];

const BOOL_COLUMNS: [&str; 6] = [
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
    "is_local_only",
    "ai_share_schema",
    "ai_share_data",
];

fn conn_from(r: &Row) -> PersistedConnection {
    let mut o = Map::new();
    for (col, key) in CONN_COLUMNS {
        let v = r.get(col).cloned().unwrap_or(Value::Null);
        let v = match (col, &v) {
            (c, Value::Number(n)) if BOOL_COLUMNS.contains(&c) => json!(n.as_i64() != Some(0)),
            ("ssh_tunnel", Value::String(t)) => {
                serde_json::from_str::<Value>(t).unwrap_or(Value::Null)
            }
            _ => v,
        };
        if col == "is_local_only" && v == json!(false) {
            continue;
        }
        o.insert(key.into(), v);
    }
    o.insert("labelIds".into(), json!([]));
    serde_json::from_value(Value::Object(o)).expect("a connection row")
}

fn conn_into(p: &PersistedConnection, r: &mut Row) {
    let v = serde_json::to_value(p).unwrap();
    for (col, key) in CONN_COLUMNS {
        let x = v.get(key).cloned().unwrap_or(Value::Null);
        let x = match (col, &x) {
            (c, Value::Bool(t)) if BOOL_COLUMNS.contains(&c) => int(*t),
            ("is_local_only", Value::Null) => json!(0),
            ("ssh_tunnel", Value::Null) => Value::Null,
            ("ssh_tunnel", other) => json!(other.to_string()),
            _ => x,
        };
        r.insert(col.into(), x);
    }
}

#[derive(Debug, PartialEq)]
enum FileFailure {
    Refused,
    Failed,
    Stale,
}

/// The disk, the rows and the links: what Core and the repo hold.
struct Sim {
    tables: BTreeMap<String, Vec<Row>>,
    links: HashMap<(Kind, String), Link>,
    shared_dirs: HashMap<String, String>,
    tree: BTreeMap<String, Node>,
    dirs: HashSet<String>,
    written: HashSet<String>,
    failing: HashSet<String>,
    unreadable: HashSet<String>,
    case_insensitive: bool,
    git: Value,
    pulls: HashMap<String, usize>,
    conflicted: HashSet<String>,
    memory: NoticeMemory,
    ids: Ids,
    errors: Vec<String>,
    raw_params: String,
}

#[derive(Default)]
struct CallResult {
    ok: bool,
    conflicted: bool,
    projection: Option<(String, Option<String>)>,
    notices: Vec<Value>,
}

impl Sim {
    fn new(case: &Value) -> Sim {
        let seed = &case["seed"];
        let mut tables: BTreeMap<String, Vec<Row>> = BTreeMap::new();
        for (t, rows) in seed["rows"].as_object().unwrap() {
            if t == "app_state" {
                continue;
            }
            tables.insert(
                t.clone(),
                rows.as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r.as_object().unwrap().clone())
                    .collect(),
            );
        }
        let mut tree = BTreeMap::new();
        let mut dirs = HashSet::new();
        for (path, v) in seed["tree"].as_object().unwrap() {
            match v {
                Value::String(t) => {
                    tree.insert(path.clone(), Node::File(t.clone()));
                }
                Value::Object(o) if o.contains_key("symlink") => {
                    tree.insert(
                        path.clone(),
                        Node::Link(o["symlink"].as_str().unwrap().into()),
                    );
                }
                _ => {
                    dirs.insert(path.clone());
                }
            }
        }
        let set = |k: &str| -> HashSet<String> {
            seed.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut links = HashMap::new();
        for r in tables.get("connections").into_iter().flatten() {
            if let Some(p) = os(r, "shared_connection_id") {
                links.insert(
                    (Kind::Connection, s(r, "id")),
                    Link {
                        path: template_path(&p).map(String::from),
                        ..Default::default()
                    },
                );
            }
        }
        Sim {
            tables,
            links,
            shared_dirs: HashMap::new(),
            tree,
            dirs,
            written: HashSet::new(),
            failing: set("failing"),
            unreadable: set("unreadable"),
            case_insensitive: seed["caseInsensitive"].as_bool().unwrap_or(false),
            git: seed.get("git").cloned().unwrap_or(Value::Null),
            pulls: HashMap::new(),
            conflicted: HashSet::new(),
            memory: NoticeMemory::default(),
            ids: Ids(1000),
            errors: Vec::new(),
            raw_params: String::new(),
        }
    }

    fn table(&mut self, t: &str) -> &mut Vec<Row> {
        self.tables.entry(t.to_string()).or_default()
    }

    fn row(&self, t: &str, id: &str) -> Option<&Row> {
        self.tables.get(t)?.iter().find(|r| s(r, "id") == id)
    }

    fn row_mut(&mut self, t: &str, id: &str) -> Option<&mut Row> {
        self.tables
            .get_mut(t)?
            .iter_mut()
            .find(|r| s(r, "id") == id)
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.ids.0 += 1;
        format!("{prefix}{}", uuid(self.ids.0))
    }

    // ── the disk ──

    fn resolve(&self, abs: &str) -> String {
        if self.case_insensitive {
            let k = path_key(abs);
            if let Some(found) = self.tree.keys().find(|p| path_key(p) == k) {
                return found.clone();
            }
        }
        abs.to_string()
    }

    fn file(&self, abs: &str) -> Option<&str> {
        match self.tree.get(&self.resolve(abs)) {
            Some(Node::File(t)) => Some(t),
            _ => None,
        }
    }

    /// A symlink anywhere on `abs` below the repo.
    fn symlink_on(&self, repo: &str, abs: &str) -> bool {
        let rel = abs.strip_prefix(repo).unwrap_or(abs);
        let mut cur = repo.to_string();
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            cur = format!("{cur}/{part}");
            if matches!(self.tree.get(&cur), Some(Node::Link(_))) {
                return true;
            }
        }
        false
    }

    fn under_unreadable(&self, abs: &str) -> bool {
        self.unreadable
            .iter()
            .any(|d| abs.starts_with(&format!("{d}/")))
    }

    fn children(&self, dir: &str) -> Vec<(String, Option<Node>)> {
        let prefix = format!("{dir}/");
        let mut names: BTreeMap<String, Option<Node>> = BTreeMap::new();
        for (p, n) in &self.tree {
            if let Some(rest) = p.strip_prefix(&prefix) {
                match rest.split_once('/') {
                    None => {
                        names.insert(rest.to_string(), Some(n.clone()));
                    }
                    Some((first, _)) => {
                        names.entry(first.to_string()).or_insert(None);
                    }
                }
            }
        }
        for d in &self.dirs {
            if let Some(rest) = d.strip_prefix(&prefix) {
                let first = rest.split('/').next().unwrap();
                names.entry(first.to_string()).or_insert(None);
            }
        }
        names.into_iter().collect()
    }

    /// Task 5's `tree::scan`, on the model: symlinks and unreadable
    /// directories are skipped and named, only the four extensions read.
    fn scan(&self, repo: &str, dir: &str) -> DirScan {
        let mut out = DirScan {
            files: vec![],
            skipped: vec![],
            conflicted: self.conflicted.contains(repo),
        };
        let root_rel = format!(".seaquel/projects/{dir}");
        let root = format!("{repo}/{root_rel}");
        if self.symlink_on(repo, &root) {
            out.skipped.push(Skipped {
                rel_path: root_rel,
                why: SkipReason::Symlink,
            });
            return out;
        }
        self.walk(repo, &root, &mut out);
        out
    }

    fn walk(&self, repo: &str, dir: &str, out: &mut DirScan) {
        for (name, node) in self.children(dir) {
            let abs = format!("{dir}/{name}");
            let rel = abs[repo.len() + 1..].to_string();
            match node {
                Some(Node::Link(_)) => out.skipped.push(Skipped {
                    rel_path: rel,
                    why: SkipReason::Symlink,
                }),
                Some(Node::File(text)) => {
                    let lower = name.to_ascii_lowercase();
                    if [".sql", ".json", ".yaml", ".yml"]
                        .iter()
                        .any(|e| lower.ends_with(e))
                    {
                        out.files.push(RawFile {
                            rel_path: rel,
                            text,
                        });
                    }
                }
                None => {
                    if self.unreadable.contains(&abs) {
                        out.skipped.push(Skipped {
                            rel_path: rel,
                            why: SkipReason::Unreadable,
                        });
                    } else {
                        self.walk(repo, &abs, out);
                    }
                }
            }
        }
    }

    fn taken(&self, repo: &str) -> HashSet<String> {
        self.tree
            .keys()
            .filter_map(|p| p.strip_prefix(&format!("{repo}/")))
            .map(path_key)
            .collect()
    }

    /// Applies one file operation. A refusal (a symlink on the path)
    /// stores nothing; a failed write stores `on_failure`; a stale one
    /// (M1: the file isn't what the row's base says) writes nothing, and
    /// Core syncs instead.
    fn apply_file(&mut self, repo: &str, op: &FileOp) -> Result<(), FileFailure> {
        match op {
            FileOp::Write {
                rel_path,
                text,
                expect_hash,
            } => {
                let abs = format!("{repo}/{rel_path}");
                if self.symlink_on(repo, &abs) {
                    return Err(FileFailure::Refused);
                }
                if !self.as_expected(rel_path, &abs, expect_hash.as_deref()) {
                    return Err(FileFailure::Stale);
                }
                let target = self.resolve(&abs);
                if self.failing.contains(&target) || self.failing.contains(&abs) {
                    return Err(FileFailure::Failed);
                }
                self.tree.insert(target.clone(), Node::File(text.clone()));
                self.written.insert(target);
                Ok(())
            }
            FileOp::Delete {
                rel_path,
                expect_hash,
            } => {
                let abs = format!("{repo}/{rel_path}");
                if self.symlink_on(repo, &abs) {
                    return Err(FileFailure::Refused);
                }
                if !self.as_expected(rel_path, &abs, expect_hash.as_deref()) {
                    return Err(FileFailure::Stale);
                }
                let target = self.resolve(&abs);
                self.tree.remove(&target);
                self.written.remove(&target);
                Ok(())
            }
        }
    }

    /// The file at `abs` is what the plan expects: one with the hash
    /// `want`, or none at all for `None`.
    fn as_expected(&self, rel_path: &str, abs: &str, want: Option<&str>) -> bool {
        match (self.file(abs), want) {
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
            (Some(text), Some(want)) => file_hash(rel_path, text).is_some_and(|(h, _)| h == want),
        }
    }

    // ── projects and repos ──

    fn project(&self, id: &str) -> Option<&Row> {
        self.row("projects", id)
    }

    fn repo_by_path(&self, path: &str) -> Option<(String, String)> {
        self.tables.get("shared_repos")?.iter().find_map(|r| {
            let data: Value = serde_json::from_str(&s(r, "data")).ok()?;
            (data["path"].as_str() == Some(path)).then(|| (s(r, "id"), path.to_string()))
        })
    }

    fn repo_by_id(&self, id: &str) -> Option<String> {
        let r = self.row("shared_repos", id)?;
        let data: Value = serde_json::from_str(&s(r, "data")).ok()?;
        data["path"].as_str().map(String::from)
    }

    fn register_repo(&mut self, path: &str, name: &str) -> String {
        if let Some((id, _)) = self.repo_by_path(path) {
            return id;
        }
        let id = self.new_id("repo-");
        let data = json!({"id": id, "name": name, "path": path, "remoteUrl": "", "branch": "main",
            "lastSyncAt": null, "syncStatus": "uninitialized"});
        let mut r = Row::new();
        r.insert("id".into(), json!(id));
        r.insert("data".into(), json!(data.to_string()));
        self.table("shared_repos").push(r);
        id
    }

    fn project_link(&self, pid: &str) -> Option<(String, ProjectLink)> {
        let p = self.project(pid)?;
        let repo = os(p, "git_repo_path")?;
        let (repo_id, _) = self.repo_by_path(&repo)?;
        let dir = self
            .shared_dirs
            .get(pid)
            .cloned()
            .unwrap_or_else(|| legacy_stem(&s(p, "name")));
        Some((repo, ProjectLink { repo_id, dir }))
    }

    fn link(&self, kind: Kind, id: &str) -> Link {
        self.links
            .get(&(kind, id.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    fn set_link(&mut self, repo_id: &str, u: &LinkUpdate) {
        if let Some(old) = self.links.get(&(u.kind, u.id.clone())) {
            if let (Some(a), Some(b)) = (&old.file_id, &u.link.file_id) {
                if a != b && old.path == u.link.path {
                    self.errors.push(format!(
                        "{:?} {}: file id changed at the same path",
                        u.kind, u.id
                    ));
                }
            }
        }
        self.links.insert((u.kind, u.id.clone()), u.link.clone());
        if u.kind == Kind::Connection {
            let value = u
                .link
                .path
                .as_ref()
                .map_or(Value::Null, |p| json!(format!("{repo_id}:{p}")));
            if let Some(r) = self.row_mut("connections", &u.id) {
                r.insert("shared_connection_id".into(), value);
            }
        }
    }

    fn shared_rows(&self, pid: &str) -> SharedRows {
        let of = |t: &str| -> Vec<&Row> {
            self.tables
                .get(t)
                .map(|rs| rs.iter().filter(|r| s(r, "project_id") == pid).collect())
                .unwrap_or_default()
        };
        SharedRows {
            project_id: pid.into(),
            queries: of("saved_queries")
                .into_iter()
                .map(|r| LinkedQuery {
                    row: sq_from(r),
                    link: self.link(Kind::SavedQuery, &s(r, "id")),
                })
                .collect(),
            dashboards: of("dashboards")
                .into_iter()
                .map(|r| LinkedDashboard {
                    row: dash_from(r),
                    link: self.link(Kind::Dashboard, &s(r, "id")),
                })
                .collect(),
            connections: of("connections")
                .into_iter()
                .map(|r| {
                    let mut link = self.link(Kind::Connection, &s(r, "id"));
                    link.path = os(r, "shared_connection_id")
                        .as_deref()
                        .and_then(template_path)
                        .map(String::from);
                    LinkedConnection {
                        row: conn_from(r),
                        link,
                    }
                })
                .collect(),
        }
    }

    // ── row writes, as Core's library does them ──

    fn append_version(&mut self, query_id: &str, previous: String) {
        let n = self
            .tables
            .get("query_versions")
            .map(|v| {
                v.iter()
                    .filter(|r| s(r, "saved_query_id") == query_id)
                    .count()
            })
            .unwrap_or(0);
        let id = self.new_id("ver-");
        let mut r = Row::new();
        r.insert("id".into(), json!(id));
        r.insert("saved_query_id".into(), json!(query_id));
        r.insert("version".into(), json!(n + 1));
        r.insert("snapshot".into(), json!(previous));
        r.insert("diff".into(), Value::Null);
        r.insert("created_at".into(), json!(NOW));
        self.table("query_versions").push(r);
    }

    fn update_query(
        &mut self,
        id: &str,
        patch: &SavedQueryPatch,
    ) -> Option<(PersistedSavedQuery, bool)> {
        let mut row = sq_from(self.row("saved_queries", id)?);
        let change = apply_saved_query_patch(&mut row, patch, NOW);
        if let Some(prev) = change.previous_text {
            self.append_version(id, prev);
        }
        sq_into(&row, self.row_mut("saved_queries", id)?);
        Some((row, change.renamed))
    }

    fn update_dashboard(
        &mut self,
        id: &str,
        patch: &DashboardPatch,
    ) -> Option<(PersistedDashboard, bool)> {
        let mut row = dash_from(self.row("dashboards", id)?);
        if patch.capture_version {
            let n = self
                .tables
                .get("dashboard_versions")
                .map(|v| v.iter().filter(|r| s(r, "dashboard_id") == id).count())
                .unwrap_or(0);
            let vid = self.new_id("dver-");
            let mut r = Row::new();
            r.insert("id".into(), json!(vid));
            r.insert("dashboard_id".into(), json!(id));
            r.insert("version".into(), json!(n + 1));
            r.insert("snapshot".into(), json!(dashboard_snapshot(&row)));
            r.insert("created_at".into(), json!(NOW));
            self.table("dashboard_versions").push(r);
        }
        let renamed = apply_dashboard_patch(&mut row, patch, NOW);
        dash_into(&row, self.row_mut("dashboards", id)?);
        Some((row, renamed))
    }

    fn update_connection(
        &mut self,
        id: &str,
        patch: &ConnectionPatch,
    ) -> Option<(PersistedConnection, bool)> {
        let mut row = conn_from(self.row("connections", id)?);
        let before = name_key(&row.name);
        apply_connection_patch(&mut row, patch, NOW);
        let renamed = before != name_key(&row.name);
        conn_into(&row, self.row_mut("connections", id)?);
        Some((row, renamed))
    }

    fn connection_order(&self, pid: &str) -> Vec<String> {
        if let Some(r) = self
            .tables
            .get("project_state")
            .and_then(|t| t.iter().find(|r| s(r, "project_id") == pid))
        {
            return serde_json::from_str(&s(r, "connection_order")).unwrap_or_default();
        }
        let mut ids: Vec<String> = self
            .tables
            .get("connections")
            .map(|t| {
                t.iter()
                    .filter(|r| s(r, "project_id") == pid)
                    .map(|r| s(r, "id"))
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }

    fn set_order(&mut self, pid: &str, order: Vec<String>) {
        let text = serde_json::to_string(&order).unwrap();
        let t = self.table("project_state");
        match t.iter_mut().find(|r| s(r, "project_id") == pid) {
            Some(r) => {
                r.insert("connection_order".into(), json!(text));
            }
            None => {
                let mut r = Row::new();
                r.insert("project_id".into(), json!(pid));
                r.insert("connection_order".into(), json!(text));
                t.push(r);
            }
        }
    }

    fn create_connection(&mut self, pid: &str, id: &str, draft: &ConnectionDraft) {
        let mut order = self.connection_order(pid);
        let mut d = draft.clone();
        if d.rename_if_taken {
            let taken: HashSet<String> = self
                .tables
                .get("connections")
                .into_iter()
                .flatten()
                .filter(|r| s(r, "project_id") == pid)
                .map(|r| name_key(&s(r, "name")))
                .collect();
            d.name = free_name(&d.name, &taken);
        }
        let row = connection_from_draft(id.to_string(), &d, NOW);
        let mut r = Row::new();
        conn_into(&row, &mut r);
        self.table("connections").push(r);
        order.push(id.to_string());
        self.set_order(pid, order);
    }

    /// The library's checks on a planned op (C1): the planner must never
    /// plan one the library would refuse.
    fn check_op(&mut self, op: &RowOp) {
        let lib = LibraryLimits::default();
        let state = StateLimits::default();
        let checked = match op {
            RowOp::CreateQuery { draft, .. } => check_saved_query_draft(draft, &lib),
            RowOp::UpdateQuery { patch, .. } => check_saved_query_patch(patch, &lib),
            RowOp::CreateDashboard { draft, .. } => check_dashboard_draft(draft, &lib, &state),
            RowOp::UpdateDashboard { patch, .. } => check_dashboard_patch(patch, &lib, &state),
            RowOp::CreateConnection { draft, .. } => check_connection_draft(draft, &lib),
            RowOp::UpdateConnection { patch, .. } => check_connection_patch(patch, &lib),
            RowOp::Unshare { .. } => Ok(()),
        };
        if let Err(e) = checked {
            self.errors
                .push(format!("the library refuses {op:?}: {}", e.code));
        }
    }

    fn apply_row_op(&mut self, pid: &str, op: &RowOp) {
        self.check_op(op);
        match op {
            RowOp::CreateQuery { id, draft } => {
                let row = saved_query_from_draft(id.clone(), draft, NOW);
                let mut r = Row::new();
                sq_into(&row, &mut r);
                self.table("saved_queries").push(r);
            }
            RowOp::UpdateQuery { id, patch } => {
                self.update_query(id, patch);
            }
            RowOp::CreateDashboard { id, draft } => {
                let row = dashboard_from_draft(id.clone(), draft, NOW);
                let mut r = Row::new();
                dash_into(&row, &mut r);
                self.table("dashboards").push(r);
            }
            RowOp::UpdateDashboard { id, patch } => {
                self.update_dashboard(id, patch);
            }
            RowOp::CreateConnection { id, draft } => self.create_connection(pid, id, draft),
            RowOp::UpdateConnection { id, patch } => {
                self.update_connection(id, patch);
            }
            RowOp::Unshare { kind, id } => match kind {
                Kind::SavedQuery => {
                    self.update_query(
                        id,
                        &SavedQueryPatch {
                            shared: Some(false),
                            ..Default::default()
                        },
                    );
                }
                Kind::Dashboard => {
                    self.update_dashboard(
                        id,
                        &DashboardPatch {
                            shared: Some(false),
                            ..Default::default()
                        },
                    );
                }
                Kind::Connection => {
                    self.update_connection(
                        id,
                        &ConnectionPatch {
                            is_local_only: Some(true),
                            ..Default::default()
                        },
                    );
                }
            },
        }
    }

    // ── Core's calls ──

    fn sync(&mut self, pid: &str, res: &mut CallResult) {
        let Some((repo, plink)) = self.project_link(pid) else {
            self.errors.push(format!("sync of {pid}: no link"));
            return;
        };
        let scan = self.scan(&repo, &plink.dir);
        let rows = self.shared_rows(pid);
        let plan = plan_sync(&plink, &scan, &rows, &Limits::default(), &mut self.ids);
        if plan.conflicted {
            res.conflicted = true;
            return;
        }
        for op in &plan.rows {
            self.apply_row_op(pid, op);
        }
        for u in plan.links.iter().filter(|u| u.pending_on.is_none()) {
            self.set_link(&plink.repo_id, u);
        }
        let mut done = HashSet::new();
        for op in &plan.files {
            if let FileOp::Write { rel_path, .. } = op {
                if self.apply_file(&repo, op).is_ok() {
                    done.insert(rel_path.clone());
                }
            } else {
                self.errors.push("a sync planned a delete".into());
            }
        }
        for u in &plan.links {
            if u.pending_on.as_ref().is_some_and(|p| done.contains(p)) {
                self.set_link(&plink.repo_id, u);
            }
        }
        self.shared_dirs
            .entry(pid.to_string())
            .or_insert(plink.dir.clone());
        for n in self.memory.filter(plan.notices) {
            res.notices.push(serde_json::to_value(&n).unwrap());
        }
    }

    fn publish(
        &mut self,
        pid: &str,
        kind: Kind,
        id: &str,
        change: &RowChange,
    ) -> Option<(String, Option<String>)> {
        let (repo, plink) = self.project_link(pid)?;
        let existing = match change {
            RowChange::Project { .. } => self
                .file(&format!(
                    "{repo}/.seaquel/projects/{}/project.yaml",
                    plink.dir
                ))
                .map(String::from),
            RowChange::Query { link, .. }
            | RowChange::Dashboard { link, .. }
            | RowChange::Connection { link, .. } => link
                .path
                .as_ref()
                .and_then(|p| self.file(&format!("{repo}/{p}")).map(String::from)),
        };
        let taken = self.taken(&repo);
        let is_taken = |p: &str| taken.contains(&path_key(p));
        let ctx = PublishContext {
            taken: &is_taken,
            existing: existing.as_deref(),
        };
        let plan: PublishPlan = match plan_publish(&plink, change, &ctx, &mut self.ids) {
            Ok(p) => p,
            Err(e) => return Some(("failed".into(), Some(e.code))),
        };
        if plan.files.is_empty() {
            if let Some(u) = &plan.on_success {
                self.set_link(&plink.repo_id, u);
            }
            return None;
        }
        let deletes = plan
            .files
            .iter()
            .all(|f| matches!(f, FileOp::Delete { .. }));
        for op in &plan.files {
            match self.apply_file(&repo, op) {
                Ok(()) => {}
                Err(FileFailure::Stale) => {
                    // M1, Core's side: a teammate's change is on disk; the
                    // project syncs instead, so the file wins.
                    let mut res = CallResult::default();
                    self.sync(pid, &mut res);
                    return Some(("failed".into(), Some("FILE_CHANGED".into())));
                }
                Err(failure) => {
                    if failure == FileFailure::Failed {
                        if let Some(u) = &plan.on_failure {
                            self.set_link(&plink.repo_id, u);
                        }
                    }
                    return Some(("failed".into(), Some("FILE_ERROR".into())));
                }
            }
        }
        if let Some(u) = &plan.on_success {
            debug_assert_eq!((u.kind, u.id.as_str()), (kind, id));
            self.set_link(&plink.repo_id, u);
        }
        Some((if deletes { "deleted" } else { "written" }.into(), None))
    }

    fn linked(&self, pid: &str) -> bool {
        self.project(pid)
            .and_then(|p| os(p, "git_repo_path"))
            .is_some()
    }

    fn library(&mut self, method: &str, params: &Value, binds: Option<&str>, res: &mut CallResult) {
        // The params' members as written, key order kept.
        let raw: HashMap<String, Box<RawValue>> = serde_json::from_str(&self.raw_params).unwrap();
        let from = |v: &Value| -> String {
            let key = ["query", "dashboard", "patch"]
                .into_iter()
                .find(|k| params.get(*k) == Some(v))
                .expect("a draft or patch member");
            raw[key].get().to_string()
        };
        match method {
            "savedQueryCreate" => {
                let draft: SavedQueryDraft = serde_json::from_str(&from(&params["query"])).unwrap();
                let id = binds
                    .map(String::from)
                    .unwrap_or_else(|| self.new_id("saved-"));
                let row = saved_query_from_draft(id.clone(), &draft, NOW);
                let mut r = Row::new();
                sq_into(&row, &mut r);
                self.table("saved_queries").push(r);
                if row.shared && self.linked(&row.project_id) {
                    let l = Link::default();
                    let ch = RowChange::Query {
                        row: Some(&row),
                        link: &l,
                        renamed: false,
                    };
                    res.projection =
                        self.publish(&row.project_id.clone(), Kind::SavedQuery, &id, &ch);
                }
            }
            "savedQueryUpdate" => {
                let id = params["id"].as_str().unwrap().to_string();
                let patch: SavedQueryPatch = serde_json::from_str(&from(&params["patch"])).unwrap();
                let link = self.link(Kind::SavedQuery, &id);
                let (row, renamed) = self.update_query(&id, &patch).expect("the query");
                if self.linked(&row.project_id) && (row.shared || link.path.is_some()) {
                    let ch = RowChange::Query {
                        row: Some(&row),
                        link: &link,
                        renamed,
                    };
                    res.projection =
                        self.publish(&row.project_id.clone(), Kind::SavedQuery, &id, &ch);
                }
            }
            "savedQueryRemove" => {
                let id = params["id"].as_str().unwrap().to_string();
                let link = self.link(Kind::SavedQuery, &id);
                let row = sq_from(self.row("saved_queries", &id).unwrap());
                self.table("saved_queries").retain(|r| s(r, "id") != id);
                self.table("query_versions")
                    .retain(|r| s(r, "saved_query_id") != id);
                if self.linked(&row.project_id) {
                    let ch = RowChange::Query {
                        row: None,
                        link: &link,
                        renamed: false,
                    };
                    res.projection = self.publish(&row.project_id, Kind::SavedQuery, &id, &ch);
                }
                self.links.remove(&(Kind::SavedQuery, id));
            }
            "dashboardCreate" => {
                let draft: DashboardDraft =
                    serde_json::from_str(&from(&params["dashboard"])).unwrap();
                let id = binds
                    .map(String::from)
                    .unwrap_or_else(|| self.new_id("dashboard-"));
                let row = dashboard_from_draft(id, &draft, NOW);
                let mut r = Row::new();
                dash_into(&row, &mut r);
                self.table("dashboards").push(r);
            }
            "dashboardUpdate" => {
                let id = params["id"].as_str().unwrap().to_string();
                let patch: DashboardPatch = serde_json::from_str(&from(&params["patch"])).unwrap();
                let link = self.link(Kind::Dashboard, &id);
                let (row, renamed) = self.update_dashboard(&id, &patch).expect("the dashboard");
                if self.linked(&row.project_id) && (row.shared || link.path.is_some()) {
                    let ch = RowChange::Dashboard {
                        row: Some(&row),
                        link: &link,
                        renamed,
                    };
                    res.projection =
                        self.publish(&row.project_id.clone(), Kind::Dashboard, &id, &ch);
                }
            }
            "dashboardRemove" => {
                let id = params["id"].as_str().unwrap().to_string();
                let link = self.link(Kind::Dashboard, &id);
                let row = dash_from(self.row("dashboards", &id).unwrap());
                self.table("dashboards").retain(|r| s(r, "id") != id);
                self.table("dashboard_versions")
                    .retain(|r| s(r, "dashboard_id") != id);
                if self.linked(&row.project_id) {
                    let ch = RowChange::Dashboard {
                        row: None,
                        link: &link,
                        renamed: false,
                    };
                    res.projection = self.publish(&row.project_id, Kind::Dashboard, &id, &ch);
                }
                self.links.remove(&(Kind::Dashboard, id));
            }
            "connectionUpdate" => {
                let id = params["id"].as_str().unwrap().to_string();
                let patch: ConnectionPatch = serde_json::from_str(&from(&params["patch"])).unwrap();
                let mut link = self.link(Kind::Connection, &id);
                link.path = os(
                    self.row("connections", &id).unwrap(),
                    "shared_connection_id",
                )
                .as_deref()
                .and_then(template_path)
                .map(String::from);
                let shared_now = patch.is_local_only == Some(false);
                let (row, renamed) = self.update_connection(&id, &patch).expect("the connection");
                if self.linked(&row.project_id) {
                    let ch = RowChange::Connection {
                        row: Some(&row),
                        link: &link,
                        renamed,
                        shared_now,
                    };
                    res.projection =
                        self.publish(&row.project_id.clone(), Kind::Connection, &id, &ch);
                }
            }
            "connectionRemove" => {
                let id = params["id"].as_str().unwrap().to_string();
                let r = self.row("connections", &id).unwrap().clone();
                let mut link = self.link(Kind::Connection, &id);
                link.path = os(&r, "shared_connection_id")
                    .as_deref()
                    .and_then(template_path)
                    .map(String::from);
                self.table("connections").retain(|x| s(x, "id") != id);
                let pid = s(&r, "project_id");
                if self.linked(&pid) {
                    let ch = RowChange::Connection {
                        row: None,
                        link: &link,
                        renamed: false,
                        shared_now: false,
                    };
                    res.projection = self.publish(&pid, Kind::Connection, &id, &ch);
                }
                self.links.remove(&(Kind::Connection, id));
            }
            "projectUpdate" => {
                let id = params["id"].as_str().unwrap().to_string();
                let patch: ProjectPatch = serde_json::from_str(&from(&params["patch"])).unwrap();
                let r = self.row_mut("projects", &id).unwrap();
                let mut p = seaquel_types::storage::PersistedProject {
                    id: id.clone(),
                    name: s(r, "name"),
                    description: os(r, "description"),
                    created_at: s(r, "created_at"),
                    updated_at: s(r, "updated_at"),
                    custom_labels: vec![],
                    git_repo_path: os(r, "git_repo_path"),
                };
                apply_project_patch(&mut p, &patch, NOW);
                r.insert("name".into(), json!(p.name));
                r.insert("description".into(), opt(&p.description));
                r.insert("git_repo_path".into(), opt(&p.git_repo_path));
                r.insert("updated_at".into(), json!(p.updated_at));
                if self.linked(&id) {
                    // A linked project's directory is stored before
                    // anything can rename it (the first sync stores it).
                    if let Some((_, l)) = self.project_link(&id) {
                        self.shared_dirs.entry(id.clone()).or_insert(l.dir);
                    }
                    let ch = RowChange::Project { name: &p.name };
                    res.projection = self.publish(&id, Kind::SavedQuery, &id, &ch);
                }
            }
            other => self.errors.push(format!("library.{other} isn't replayed")),
        }
    }

    fn link_project(&mut self, pid: &str, path: &str, share: &[String], res: &mut CallResult) {
        let name = s(self.project(pid).unwrap(), "name");
        let repo_id = self.register_repo(path, &name);
        let base = format!("{path}/.seaquel/projects");
        let dirs: Vec<(String, String)> = self
            .children(&base)
            .into_iter()
            .filter(|(_, n)| n.is_none())
            .map(|(d, _)| {
                let text = self
                    .file(&format!("{base}/{d}/project.yaml"))
                    .unwrap_or_default()
                    .to_string();
                let pname = parse_project(&text, &d).name;
                (d, pname)
            })
            .collect();
        let dir = pick_project_dir(&name, &dirs);
        let r = self.row_mut("projects", pid).unwrap();
        r.insert("git_repo_path".into(), json!(path));
        r.insert("updated_at".into(), json!(NOW));
        self.shared_dirs.insert(pid.into(), dir.clone());
        let plink = ProjectLink { repo_id, dir };
        let yaml = format!("{base}/{}/project.yaml", plink.dir);
        if self.file(&yaml).is_none() {
            let taken = |_: &str| false;
            let ctx = PublishContext {
                taken: &taken,
                existing: None,
            };
            let plan = plan_publish(
                &plink,
                &RowChange::Project { name: &name },
                &ctx,
                &mut self.ids,
            )
            .unwrap();
            for op in &plan.files {
                let _ = self.apply_file(path, op);
            }
        }
        for cid in share {
            let Some(r) = self.row("connections", cid) else {
                self.errors
                    .push(format!("share names {cid}, not in the project"));
                continue;
            };
            if os(r, "shared_connection_id").is_some() {
                continue;
            }
            let row = conn_from(r);
            let link = Link::default();
            let ch = RowChange::Connection {
                row: Some(&row),
                link: &link,
                renamed: false,
                shared_now: true,
            };
            let _ = self.publish(pid, Kind::Connection, cid, &ch);
        }
        self.sync(pid, res);
    }

    fn unlink_project(&mut self, pid: &str) {
        let Some((repo, plink)) = self.project_link(pid) else {
            self.errors.push(format!("unlink of {pid}: no link"));
            return;
        };
        let prefix = format!(".seaquel/projects/{}/connections/", plink.dir);
        let removed: Vec<String> = self
            .tables
            .get("connections")
            .into_iter()
            .flatten()
            .filter(|r| s(r, "project_id") == pid)
            .filter(|r| {
                os(r, "shared_connection_id")
                    .as_deref()
                    .and_then(template_path)
                    .is_some_and(|p| p.starts_with(&prefix))
            })
            .map(|r| s(r, "id"))
            .collect();
        let order: Vec<String> = self
            .connection_order(pid)
            .into_iter()
            .filter(|c| !removed.contains(c))
            .collect();
        self.table("connections")
            .retain(|r| !removed.contains(&s(r, "id")));
        for c in &removed {
            self.links.remove(&(Kind::Connection, c.clone()));
        }
        self.set_order(pid, order);
        for t in ["saved_queries", "dashboards"] {
            let kind = if t == "saved_queries" {
                Kind::SavedQuery
            } else {
                Kind::Dashboard
            };
            let ids: Vec<String> = self
                .tables
                .get(t)
                .into_iter()
                .flatten()
                .filter(|r| s(r, "project_id") == pid)
                .map(|r| s(r, "id"))
                .collect();
            for id in ids {
                self.links.remove(&(kind, id));
            }
        }
        self.shared_dirs.remove(pid);
        let r = self.row_mut("projects", pid).unwrap();
        r.insert("git_repo_path".into(), Value::Null);
        r.insert("updated_at".into(), json!(NOW));
        let used = self
            .tables
            .get("projects")
            .into_iter()
            .flatten()
            .any(|p| os(p, "git_repo_path").as_deref() == Some(repo.as_str()));
        if !used {
            let id = plink.repo_id;
            self.table("shared_repos").retain(|r| s(r, "id") != id);
        }
    }

    fn import_projects(&mut self, path: &str, dirs: &[String], res: &mut CallResult) {
        for dir in dirs {
            let text = self
                .file(&format!("{path}/.seaquel/projects/{dir}/project.yaml"))
                .unwrap_or_default()
                .to_string();
            let wanted = parse_project(&text, dir).name;
            let taken: HashSet<String> = self
                .tables
                .get("projects")
                .into_iter()
                .flatten()
                .map(|r| name_key(&s(r, "name")))
                .collect();
            let name = free_name(&wanted, &taken);
            let pid = self.new_id("project-");
            let mut r = Row::new();
            r.insert("id".into(), json!(pid));
            r.insert("name".into(), json!(name));
            r.insert("description".into(), Value::Null);
            r.insert("git_repo_path".into(), json!(path));
            r.insert("created_at".into(), json!(NOW));
            r.insert("updated_at".into(), json!(NOW));
            self.table("projects").push(r);
            self.register_repo(path, &name);
            self.shared_dirs.insert(pid.clone(), dir.clone());
            self.sync(&pid, res);
        }
    }

    fn pull(&mut self, path: &str) {
        let g = &self.git[path];
        let n = self.pulls.entry(path.to_string()).or_insert(0);
        let pull = match &g["pull"] {
            Value::Array(a) => a.get(*n).cloned(),
            Value::Null => None,
            other => (*n == 0).then(|| other.clone()),
        };
        *n += 1;
        let Some(pull) = pull else { return };
        for (rel, v) in pull["files"].as_object().into_iter().flatten() {
            let abs = format!("{path}/{rel}");
            self.written.remove(&abs);
            match v {
                Value::String(t) => {
                    self.tree.insert(abs, Node::File(t.clone()));
                }
                _ => {
                    self.tree.remove(&abs);
                }
            }
        }
        if pull["conflicts"].as_array().is_some_and(|c| !c.is_empty()) {
            self.conflicted.insert(path.to_string());
        }
    }

    /// `raw_params` is the call's `params` as written in the fixture, so
    /// JSON values keep their key order (`Value` sorts keys).
    fn call(&mut self, call: &Value, raw_params: &str, bindings: &Bindings) -> CallResult {
        let mut res = CallResult {
            ok: true,
            ..Default::default()
        };
        let raw_params = minify(&bindings.apply(raw_params));
        let params: Value = serde_json::from_str(&raw_params).unwrap();
        self.raw_params = raw_params;
        let binds = call.get("binds").and_then(Value::as_str);
        match (
            call["group"].as_str().unwrap(),
            call["method"].as_str().unwrap(),
        ) {
            ("shared", "sync") => self.sync(params["projectId"].as_str().unwrap(), &mut res),
            ("shared", "syncRepo") => {
                let path = self.repo_by_id(params["repoId"].as_str().unwrap()).unwrap();
                let pids: Vec<String> = self
                    .tables
                    .get("projects")
                    .into_iter()
                    .flatten()
                    .filter(|p| os(p, "git_repo_path").as_deref() == Some(path.as_str()))
                    .map(|p| s(p, "id"))
                    .collect();
                for pid in pids {
                    self.sync(&pid, &mut res);
                }
            }
            ("shared", "linkProject") => {
                let share: Vec<String> = serde_json::from_value(params["share"].clone()).unwrap();
                self.link_project(
                    params["projectId"].as_str().unwrap(),
                    params["path"].as_str().unwrap(),
                    &share,
                    &mut res,
                );
            }
            ("shared", "unlinkProject") => {
                self.unlink_project(params["projectId"].as_str().unwrap())
            }
            ("shared", "scan") => {}
            ("shared", "importProjects") => {
                let dirs: Vec<String> = serde_json::from_value(params["dirs"].clone()).unwrap();
                self.import_projects(params["path"].as_str().unwrap(), &dirs, &mut res);
            }
            ("git", "pull") => self.pull(params["path"].as_str().unwrap()),
            ("git", "commit") => {}
            ("library", m) => self.library(m, &params, binds, &mut res),
            (g, m) => self.errors.push(format!("{g}.{m} isn't replayed")),
        }
        res
    }

    // ── what the step shows ──

    fn dump_rows(&self) -> BTreeMap<String, Vec<Value>> {
        let mut out = BTreeMap::new();
        for (t, rows) in &self.tables {
            if rows.is_empty() {
                continue;
            }
            let rows: Vec<Value> = rows
                .iter()
                .map(|r| {
                    if t == "shared_repos" {
                        repo_view(r)
                    } else {
                        Value::Object(r.clone())
                    }
                })
                .collect();
            out.insert(t.clone(), rows);
        }
        out
    }

    /// The disk as the fixtures write it, Core's ids checked and removed.
    fn dump_tree(&mut self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        let mut problems = Vec::new();
        for (p, n) in &self.tree {
            let v = match n {
                Node::Link(t) => json!({ "symlink": t }),
                Node::File(t) if self.written.contains(p) => match strip_file_id(p, t) {
                    Ok(t) => json!(t),
                    Err(why) => {
                        problems.push(format!("{p}: {why}"));
                        json!(t)
                    }
                },
                Node::File(t) => json!(t),
            };
            out.insert(p.clone(), v);
        }
        self.errors.extend(problems);
        out
    }
}

fn repo_view(r: &Row) -> Value {
    let data: Value = serde_json::from_str(&s(r, "data")).unwrap_or(Value::Null);
    json!({"id": s(r, "id"), "path": data["path"], "name": data["name"]})
}

/// Removes the id Core wrote from a file it wrote, checking it is a
/// v4 uuid in the first line or key.
fn strip_file_id(path: &str, text: &str) -> Result<String, String> {
    let lower = path.to_ascii_lowercase();
    let take = |prefix: &str, line_end: &str, text: &str| -> Result<String, String> {
        let rest = text
            .strip_prefix(prefix)
            .ok_or_else(|| "no id where Core writes it".to_string())?;
        let end = rest.find(line_end).ok_or("no end of the id line")?;
        let id = &rest[..end];
        if !is_v4(id) {
            return Err(format!("the id isn't a v4 uuid: {id}"));
        }
        Ok(rest[end + line_end.len()..].to_string())
    };
    if lower.ends_with(".sql") {
        Ok(format!("---\n{}", take("---\nid: ", "\n", text)?))
    } else if lower.ends_with(".json") {
        Ok(format!("{{\n{}", take("{\n  \"id\": \"", "\",\n", text)?))
    } else if lower.ends_with("project.yaml") {
        Ok(text.to_string())
    } else {
        take("id: ", "\n", text)
    }
}

// ── matching expected values, with `<id:n>` and `<core:n>` bound ──

#[derive(Clone, Default)]
struct Bindings {
    to: HashMap<String, String>,
    from: HashMap<String, String>,
}

fn token_at(s: &str) -> Option<&str> {
    let rest = s.strip_prefix('<')?;
    let body = rest
        .strip_prefix("id:")
        .or_else(|| rest.strip_prefix("core:"))?;
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || body.as_bytes().get(digits) != Some(&b'>') {
        return None;
    }
    Some(&s[..s.len() - body.len() + digits + 1])
}

impl Bindings {
    fn apply(&self, text: &str) -> String {
        let mut out = text.to_string();
        for (k, v) in &self.to {
            out = out.replace(k, v);
        }
        out
    }

    fn bind(&mut self, token: &str, value: &str) -> bool {
        match (self.to.get(token), self.from.get(value)) {
            (Some(v), _) => v == value,
            (None, Some(t)) => t == token,
            (None, None) => {
                self.to.insert(token.into(), value.into());
                self.from.insert(value.into(), token.into());
                true
            }
        }
    }

    fn str_matches(&mut self, exp: &str, act: &str) -> bool {
        let (mut e, mut a) = (exp, act);
        loop {
            if e.is_empty() {
                return a.is_empty();
            }
            if let Some(tok) = token_at(e) {
                let value = if a.starts_with(tok) {
                    tok
                } else if a.len() >= 36 && is_v4(&a[..36]) {
                    &a[..36]
                } else {
                    return false;
                };
                if !self.bind(tok, value) {
                    return false;
                }
                e = &e[tok.len()..];
                a = &a[value.len()..];
                continue;
            }
            let c = e.chars().next().unwrap();
            if !a.starts_with(c) {
                return false;
            }
            e = &e[c.len_utf8()..];
            a = &a[c.len_utf8()..];
        }
    }

    fn value_matches(&mut self, exp: &Value, act: &Value) -> bool {
        match (exp, act) {
            (Value::String(e), Value::String(a)) => self.str_matches(e, a),
            (Value::Number(e), Value::Number(a)) => e.as_f64() == a.as_f64(),
            (Value::Object(e), Value::Object(a)) => {
                e.len() == a.len()
                    && e.iter()
                        .all(|(k, v)| a.get(k).is_some_and(|x| self.value_matches(v, x)))
            }
            (Value::Array(e), Value::Array(a)) => {
                e.len() == a.len() && e.iter().zip(a).all(|(x, y)| self.value_matches(x, y))
            }
            (e, a) => e == a,
        }
    }

    /// Whether `exp` and `act` hold the same items in some order, binding
    /// tokens on the way.
    fn set_matches(&mut self, exp: &[Value], act: &[Value]) -> bool {
        if exp.len() != act.len() {
            return false;
        }
        fn go(b: &mut Bindings, exp: &[Value], act: &[Value], used: &mut Vec<bool>) -> bool {
            let Some((first, rest)) = exp.split_first() else {
                return true;
            };
            for i in 0..act.len() {
                if used[i] {
                    continue;
                }
                let mut trial = b.clone();
                if trial.value_matches(first, &act[i]) {
                    used[i] = true;
                    if go(&mut trial, rest, act, used) {
                        *b = trial;
                        return true;
                    }
                    used[i] = false;
                }
            }
            false
        }
        let mut used = vec![false; act.len()];
        go(self, exp, act, &mut used)
    }
}

/// JSON text without the whitespace between tokens, as `JSON.stringify`
/// sends it (the fixture file is pretty-printed), key order kept.
fn minify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_str = false;
    let mut escaped = false;
    for c in text.chars() {
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
            out.push(c);
        } else if !c.is_ascii_whitespace() {
            out.push(c);
        }
    }
    out
}

/// The cases' `core` calls with their `params` as written.
#[derive(serde::Deserialize)]
struct RawCase {
    steps: Vec<RawStep>,
}

#[derive(serde::Deserialize)]
struct RawStep {
    core: Option<Vec<RawCall>>,
}

#[derive(serde::Deserialize)]
struct RawCall {
    params: Box<RawValue>,
}

fn raw_cases() -> Vec<RawCase> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shared");
    serde_json::from_str(&std::fs::read_to_string(format!("{dir}/projection.json")).unwrap())
        .unwrap()
}

fn fixtures() -> (Vec<Value>, Map<String, Value>) {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/shared");
    let p =
        serde_json::from_str(&std::fs::read_to_string(format!("{dir}/projection.json")).unwrap())
            .unwrap();
    let c = serde_json::from_str(&std::fs::read_to_string(format!("{dir}/changes.json")).unwrap())
        .unwrap();
    (p, c)
}

impl Sim {
    /// `*` (4a) and (4b).
    fn invariants(&self, all_ok: bool, pathless: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for (t, kind) in [
            ("saved_queries", Kind::SavedQuery),
            ("dashboards", Kind::Dashboard),
            ("connections", Kind::Connection),
        ] {
            for r in self.tables.get(t).into_iter().flatten() {
                let id = s(r, "id");
                let pid = s(r, "project_id");
                let Some((repo, _)) = self.project_link(&pid) else {
                    continue;
                };
                let link = self.link(kind, &id);
                let path = if kind == Kind::Connection {
                    os(r, "shared_connection_id")
                        .as_deref()
                        .and_then(template_path)
                        .map(String::from)
                } else {
                    link.path.clone()
                };
                let shared = if kind == Kind::Connection {
                    path.is_some()
                } else {
                    b(r, "shared")
                };
                // (4b) holds in a project whose directory is stored (Core
                // stores it at the first sync, link or import).
                let seen = self.shared_dirs.contains_key(&pid);
                if all_ok && seen && shared && path.is_none() && !pathless.contains(&id) {
                    out.push(format!("(4b) {t} {id} is shared without a path"));
                }
                let Some(path) = path else { continue };
                if self.conflicted.contains(&repo) {
                    continue;
                }
                let abs = format!("{repo}/{path}");
                if self.symlink_on(&repo, &abs) || self.under_unreadable(&abs) {
                    continue;
                }
                let Some(text) = self.file(&abs) else {
                    continue;
                };
                let Some((hash, file_id)) = file_hash(&path, text) else {
                    continue;
                };
                if link.base.as_deref() != Some(hash.as_str())
                    && !self.withheld_name(kind, r, &path, text, link.base.as_deref())
                {
                    out.push(format!("(4a) {t} {id}: the base isn't its file's hash"));
                }
                if link.file_id != file_id {
                    out.push(format!("(4a) {t} {id}: the file id isn't its file's"));
                }
            }
        }
        out
    }
}

impl Sim {
    /// `*` (4a)'s exception: the sync withheld the file's name from the row
    /// (another row holds it). The base is then the row's own hash, and the
    /// row under the file's name is the file.
    fn withheld_name(
        &self,
        kind: Kind,
        r: &Row,
        path: &str,
        text: &str,
        base: Option<&str>,
    ) -> bool {
        let none = Link::default();
        let (now, renamed) = match kind {
            Kind::SavedQuery => {
                let row = sq_from(r);
                let dir = &path[..path.find("/queries/").map_or(0, |i| i + 8)];
                let mut other = row.clone();
                other.name = parse_query(text, path, dir).name;
                (
                    row_hash(&RowChange::Query {
                        row: Some(&row),
                        link: &none,
                        renamed: false,
                    }),
                    row_hash(&RowChange::Query {
                        row: Some(&other),
                        link: &none,
                        renamed: false,
                    }),
                )
            }
            Kind::Dashboard => {
                let row = dash_from(r);
                let Some(file) = parse_dashboard(text, path) else {
                    return false;
                };
                let mut other = row.clone();
                other.name = file.name;
                (
                    row_hash(&RowChange::Dashboard {
                        row: Some(&row),
                        link: &none,
                        renamed: false,
                    }),
                    row_hash(&RowChange::Dashboard {
                        row: Some(&other),
                        link: &none,
                        renamed: false,
                    }),
                )
            }
            Kind::Connection => {
                let row = conn_from(r);
                let Some(file) = parse_template(text) else {
                    return false;
                };
                let mut other = row.clone();
                other.name = file.name;
                let ch = |c| RowChange::Connection {
                    row: Some(c),
                    link: &none,
                    renamed: false,
                    shared_now: false,
                };
                (row_hash(&ch(&row)), row_hash(&ch(&other)))
            }
        };
        let file = file_hash(path, text).map(|(h, _)| h);
        base.is_some() && now.as_deref() == base && renamed == file && now != file
    }
}

#[test]
fn replays_every_projection_plan() {
    let (cases, changes) = fixtures();
    assert_eq!(cases.len(), 52);
    let mut failures = Vec::new();
    let mut steps = 0;
    let raw = raw_cases();
    for (case, raw_case) in cases.iter().zip(&raw) {
        let name = case["name"].as_str().unwrap();
        let mut sim = Sim::new(case);
        let mut bindings = Bindings::default();
        let expected_steps = changes
            .get(name)
            .and_then(|c| c["expected"]["steps"].as_object())
            .cloned()
            .unwrap_or_default();
        for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            steps += 1;
            let want = expected_steps
                .get(&i.to_string())
                .cloned()
                .unwrap_or(json!({}));
            let mut fail =
                |what: String| failures.push(format!("{name} #{i} ({}): {what}", step["op"]));
            if step["op"] == "disk.allowWrites" {
                sim.failing.clear();
            }
            let calls = step["core"].as_array().cloned().unwrap_or_default();
            let raw_calls = raw_case.steps[i].core.as_deref().unwrap_or_default();
            let mut results = Vec::new();
            for (c, raw_call) in calls.iter().zip(raw_calls) {
                if let Some(b) = c.get("binds").and_then(Value::as_str) {
                    if let Some(tok) = b.find('<').and_then(|at| token_at(&b[at..])) {
                        bindings.bind(tok, tok);
                    }
                }
                results.push(sim.call(c, raw_call.params.get(), &bindings));
            }
            for e in std::mem::take(&mut sim.errors) {
                fail(e);
            }

            // Rows.
            let rows_want: Map<String, Value> = want
                .get("rows")
                .or(step.get("rows"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap();
            let got = sim.dump_rows();
            let mut tables: Vec<&String> = rows_want.keys().filter(|t| *t != "app_state").collect();
            for t in got.keys() {
                if !tables.contains(&t) {
                    tables.push(t);
                }
            }
            for t in tables {
                let exp: Vec<Value> = rows_want
                    .get(t)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|r| {
                        if t == "shared_repos" {
                            repo_view(r.as_object().unwrap())
                        } else {
                            r
                        }
                    })
                    .collect();
                let act = got.get(t).cloned().unwrap_or_default();
                if !bindings.set_matches(&exp, &act) {
                    fail(format!(
                        "rows of {t}\n    want {}\n    got  {}",
                        Value::Array(exp),
                        Value::Array(act)
                    ));
                }
            }

            // Files.
            let tree_want = want.get("tree").or(step.get("tree")).cloned().unwrap();
            let got_tree = sim.dump_tree();
            for e in std::mem::take(&mut sim.errors) {
                fail(e);
            }
            let tree_got = Value::Object(got_tree.into_iter().collect());
            if !bindings.value_matches(&tree_want, &tree_got) {
                fail(format!("tree\n    want {tree_want}\n    got  {tree_got}"));
            }

            // Links.
            if let Some(links) = want.get("links").and_then(Value::as_object) {
                for (t, by_id) in links {
                    for (id, cols) in by_id.as_object().unwrap() {
                        let id = bindings.apply(id);
                        for (col, v) in cols.as_object().unwrap() {
                            let got = match (t.as_str(), col.as_str()) {
                                ("projects", "shared_dir") => {
                                    opt(&sim.shared_dirs.get(&id).cloned())
                                }
                                ("saved_queries", "shared_path") => {
                                    opt(&sim.link(Kind::SavedQuery, &id).path)
                                }
                                ("saved_queries", "shared_file_id") => {
                                    opt(&sim.link(Kind::SavedQuery, &id).file_id)
                                }
                                ("dashboards", "shared_path") => {
                                    opt(&sim.link(Kind::Dashboard, &id).path)
                                }
                                ("connections", "shared_connection_id") => sim
                                    .row("connections", &id)
                                    .map_or(Value::Null, |r| r["shared_connection_id"].clone()),
                                other => panic!("no link column {other:?}"),
                            };
                            if !bindings.value_matches(v, &got) {
                                fail(format!("link {t}.{col} of {id}: want {v}, got {got}"));
                            }
                        }
                    }
                }
            }

            // Notices: exactly the listed ones on a step with a shared call.
            if calls.iter().any(|c| c["group"] == "shared") {
                let exp = want
                    .get("notices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let act: Vec<Value> = results.iter().flat_map(|r| r.notices.clone()).collect();
                if !bindings.set_matches(&exp, &act) {
                    fail(format!(
                        "notices\n    want {}\n    got  {}",
                        Value::Array(exp),
                        Value::Array(act)
                    ));
                }
            } else if results.iter().any(|r| !r.notices.is_empty()) {
                fail("notices from a step without a shared call".into());
            }

            // Projection and outcome, where listed.
            if let Some(p) = want.get("projection") {
                let got = results.iter().rev().find_map(|r| r.projection.clone());
                let got = got.map_or(
                    Value::Null,
                    |(status, code)| json!({"status": status, "code": code}),
                );
                if got["status"] != p["status"]
                    || (p.get("code").is_some() && got["code"] != p["code"])
                {
                    fail(format!("projection: want {p}, got {got}"));
                }
            }
            if let Some(o) = want.get("outcome") {
                let last = results.last();
                if o["ok"].as_bool() != Some(last.is_some_and(|r| r.ok)) {
                    fail(format!("outcome: want {o}"));
                }
                if let Some(c) = o.pointer("/value/conflicted") {
                    if c.as_bool() != Some(last.is_some_and(|r| r.conflicted)) {
                        fail(format!("outcome: want {o}"));
                    }
                }
            } else if results.iter().any(|r| r.conflicted) {
                fail("a conflicted answer nobody expected".into());
            }

            // Bases and paths.
            let pathless: Vec<String> = want
                .get("pathless")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .map(|v| bindings.apply(v.as_str().unwrap()))
                        .collect()
                })
                .unwrap_or_default();
            let all_ok = results.iter().all(|r| r.ok);
            for e in sim.invariants(all_ok, &pathless) {
                fail(e);
            }
        }
    }
    assert_eq!(steps, 225);
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Drives one Core call on the model, as a step's `core` would.
fn drive(sim: &mut Sim, group: &str, method: &str, params: Value) -> CallResult {
    let call = json!({"group": group, "method": method, "params": params});
    let res = sim.call(&call, &params.to_string(), &Bindings::default());
    let errors = std::mem::take(&mut sim.errors);
    assert!(errors.is_empty(), "{errors:?}");
    res
}

fn connection_names(sim: &Sim) -> Vec<(String, Option<String>)> {
    sim.tables["connections"]
        .iter()
        .map(|r| (s(r, "name"), os(r, "shared_connection_id")))
        .collect()
}

/// I2 with flag 4: `connections/template-type-changed` goes on. While "c1"
/// holds "Warehouse", the imported template's connection keeps
/// "Warehouse (2)" and no sync writes that into the template; once "c1"
/// is removed, the next sync gives it the template's name.
#[test]
fn an_imported_template_takes_its_name_once_it_is_free() {
    let (cases, _) = fixtures();
    let case = cases
        .iter()
        .find(|c| c["name"] == "connections/template-type-changed")
        .unwrap();
    let mut sim = Sim::new(case);
    let template = "/repos/a/.seaquel/projects/team/connections/warehouse.yaml";
    let no_suffix_on_disk = |sim: &Sim| {
        sim.tree
            .values()
            .all(|n| !matches!(n, Node::File(t) if t.contains("(2)")))
    };
    drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
    drive(&mut sim, "git", "pull", json!({"path": "/repos/a"}));
    drive(&mut sim, "shared", "syncRepo", json!({"repoId": "repo-a"}));
    let names = connection_names(&sim);
    assert!(names.contains(&("Warehouse".into(), None)), "{names:?}");
    assert!(
        names
            .iter()
            .any(|(n, l)| n == "Warehouse (2)" && l.is_some()),
        "{names:?}"
    );
    let file = sim.file(template).unwrap().to_string();
    for round in 0..2 {
        let res = drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
        assert!(
            res.notices.is_empty(),
            "named once per session: {:?}",
            res.notices
        );
        assert_eq!(connection_names(&sim), names, "round {round}");
        assert_eq!(
            sim.file(template),
            Some(file.as_str()),
            "nothing written back"
        );
        assert!(no_suffix_on_disk(&sim));
        assert_eq!(sim.invariants(true, &[]), Vec::<String>::new());
    }
    drive(&mut sim, "library", "connectionRemove", json!({"id": "c1"}));
    drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
    let names = connection_names(&sim);
    assert_eq!(names.len(), 1);
    assert_eq!(names[0].0, "Warehouse", "the name lands once it is free");
    assert_eq!(sim.file(template), Some(file.as_str()));
    assert!(no_suffix_on_disk(&sim));
    assert_eq!(sim.invariants(true, &[]), Vec::<String>::new());
}

/// M1, Core's side: a publish over a file a teammate changed doesn't
/// overwrite it; the project syncs, the file wins and the edit is kept as
/// a version.
#[test]
fn a_publish_never_overwrites_a_teammates_change() {
    let (cases, _) = fixtures();
    let case = cases
        .iter()
        .find(|c| c["name"] == "repo/pull-without-activation")
        .unwrap();
    let mut sim = Sim::new(case);
    let path = "/repos/a/.seaquel/projects/team/queries/orders.sql";
    drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
    // A teammate's change lands without a sync (a pull whose sync hasn't
    // run yet).
    sim.tree.insert(
        path.into(),
        Node::File("---\nname: Orders\n---\nSELECT 'theirs'\n".into()),
    );
    let res = drive(
        &mut sim,
        "library",
        "savedQueryUpdate",
        json!({"id": "q1", "patch": {"query": "SELECT 'mine'"}}),
    );
    assert_eq!(
        res.projection,
        Some(("failed".to_string(), Some("FILE_CHANGED".to_string())))
    );
    assert!(sim.file(path).unwrap().contains("SELECT 'theirs'"));
    let q1 = sim.row("saved_queries", "q1").unwrap();
    assert_eq!(s(q1, "query"), "SELECT 'theirs'");
    let versions: Vec<String> = sim.tables["query_versions"]
        .iter()
        .map(|v| s(v, "snapshot"))
        .collect();
    assert!(
        versions.contains(&"SELECT 'mine'".to_string()),
        "{versions:?}"
    );
    assert_eq!(sim.invariants(true, &[]), Vec::<String>::new());
}

fn query_row(id: &str, name: &str, text: &str) -> Value {
    json!({"id": id, "project_id": "p1", "name": name, "query": text, "parameters": null,
        "starred": 0, "shared": 1, "description": null, "database_type": null, "tags": null,
        "folder": null, "created_at": "2024-01-01T00:00:00.000Z",
        "updated_at": "2024-01-01T00:00:00.000Z"})
}

/// R2: while a row's name is withheld (a teammate renamed its file to a
/// name another row holds), a local edit is a local change. Whether the
/// publish writes it or (after a failed write) the next sync does, the
/// file takes the row's content under the teammate's name; later syncs
/// change nothing, name no conflict and add no version.
#[test]
fn an_edit_while_a_name_is_withheld_stays_local() {
    let q = ".seaquel/projects/team/queries";
    let totals = format!("/repos/a/{q}/totals.sql");
    let case = json!({
        "seed": {
            "rows": {
                "projects": [{"id": "p1", "name": "Team", "description": null,
                    "git_repo_path": "/repos/a", "created_at": "2024-01-01T00:00:00.000Z",
                    "updated_at": "2024-01-01T00:00:00.000Z"}],
                "shared_repos": [{"id": "repo-a",
                    "data": "{\"id\":\"repo-a\",\"name\":\"team-repo\",\"path\":\"/repos/a\"}"}],
                "saved_queries": [query_row("q1", "Orders", "SELECT 1"),
                    query_row("q2", "Totals", "SELECT 2")],
                "app_state": [],
            },
            "tree": {
                "/repos/a/.seaquel/projects/team/project.yaml": "name: Team\n",
                (format!("/repos/a/{q}/orders.sql")): "---\nname: Orders\n---\nSELECT 1\n",
                (totals.clone()): "---\nname: Totals\n---\nSELECT 2\n",
            },
            "git": {"/repos/a": {"pull": {"files": {
                (format!("{q}/totals.sql")): "---\nname: Orders\n---\nSELECT 2\n"}}}},
            "failing": [totals.clone()],
        }
    });
    let mut sim = Sim::new(&case);
    drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
    drive(&mut sim, "git", "pull", json!({"path": "/repos/a"}));
    let res = drive(&mut sim, "shared", "syncRepo", json!({"repoId": "repo-a"}));
    assert_eq!(res.notices.len(), 1, "the withheld name: {:?}", res.notices);
    assert_eq!(s(sim.row("saved_queries", "q2").unwrap(), "name"), "Totals");
    let versions = |sim: &Sim| sim.tables.get("query_versions").map_or(0, Vec::len);
    let check = |sim: &mut Sim, text: &str| {
        for _ in 0..2 {
            let res = drive(sim, "shared", "sync", json!({"projectId": "p1"}));
            assert!(res.notices.is_empty(), "{:?}", res.notices);
            let row = sim.row("saved_queries", "q2").unwrap();
            assert_eq!(
                (s(row, "name"), s(row, "query")),
                ("Totals".into(), text.into())
            );
            let file = sim.file(&totals).unwrap();
            assert!(
                file.contains("name: Orders\n") && file.contains(text),
                "{file}"
            );
            assert_eq!(sim.invariants(true, &[]), Vec::<String>::new());
        }
    };
    // An edit whose write fails: the next sync writes it.
    let res = drive(
        &mut sim,
        "library",
        "savedQueryUpdate",
        json!({"id": "q2", "patch": {"query": "SELECT 22"}}),
    );
    assert_eq!(res.projection.map(|p| p.0), Some("failed".to_string()));
    let after_edit = versions(&sim);
    sim.failing.clear();
    check(&mut sim, "SELECT 22");
    assert_eq!(versions(&sim), after_edit, "no version from the syncs");
    // An edit whose write works: the publish writes it.
    let res = drive(
        &mut sim,
        "library",
        "savedQueryUpdate",
        json!({"id": "q2", "patch": {"query": "SELECT 33"}}),
    );
    assert_eq!(res.projection.map(|p| p.0), Some("written".to_string()));
    let after_edit = versions(&sim);
    check(&mut sim, "SELECT 33");
    assert_eq!(versions(&sim), after_edit, "no version from the syncs");
}

/// A name-only change from a
/// teammate merges with a local content change. The teammate renames the
/// file and changes nothing else; before any sync, the user edits the
/// text. The edit goes out under the teammate's name (by the publish, or,
/// when its write fails, by the next sync), and the following sync gives
/// the row the teammate's name: no conflict, no version beyond the edit's
/// own, and the row and the file both end with the user's text and the
/// teammate's name.
#[test]
fn a_teammates_rename_merges_with_a_local_edit() {
    let q = ".seaquel/projects/team/queries";
    let orders = format!("/repos/a/{q}/orders.sql");
    for write_fails in [false, true] {
        let case = json!({
            "seed": {
                "rows": {
                    "projects": [{"id": "p1", "name": "Team", "description": null,
                        "git_repo_path": "/repos/a", "created_at": "2024-01-01T00:00:00.000Z",
                        "updated_at": "2024-01-01T00:00:00.000Z"}],
                    "shared_repos": [{"id": "repo-a",
                        "data": "{\"id\":\"repo-a\",\"name\":\"team-repo\",\"path\":\"/repos/a\"}"}],
                    "saved_queries": [query_row("q1", "Orders", "SELECT 1")],
                    "app_state": [],
                },
                "tree": {
                    "/repos/a/.seaquel/projects/team/project.yaml": "name: Team\n",
                    (orders.clone()): "---\nname: Orders\n---\nSELECT 1\n",
                },
                "git": {"/repos/a": {"pull": {"files": {
                    (format!("{q}/orders.sql")): "---\nname: Order totals\n---\nSELECT 1\n"}}}},
                "failing": if write_fails { json!([orders.clone()]) } else { json!([]) },
            }
        });
        let mut sim = Sim::new(&case);
        drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
        drive(&mut sim, "git", "pull", json!({"path": "/repos/a"}));
        let res = drive(
            &mut sim,
            "library",
            "savedQueryUpdate",
            json!({"id": "q1", "patch": {"query": "SELECT 2"}}),
        );
        let status = res.projection.map(|p| p.0);
        assert_eq!(
            status.as_deref(),
            Some(if write_fails { "failed" } else { "written" })
        );
        sim.failing.clear();
        let versions = sim.tables.get("query_versions").map_or(0, Vec::len);
        assert_eq!(versions, 1, "the edit's own keyframe");
        for _ in 0..3 {
            let res = drive(&mut sim, "shared", "sync", json!({"projectId": "p1"}));
            assert!(res.notices.is_empty(), "{write_fails}: {:?}", res.notices);
            assert_eq!(sim.invariants(true, &[]), Vec::<String>::new());
        }
        let row = sim.row("saved_queries", "q1").unwrap();
        assert_eq!(
            (s(row, "name"), s(row, "query")),
            ("Order totals".into(), "SELECT 2".into()),
            "{write_fails}"
        );
        let file = sim.file(&orders).unwrap();
        assert!(
            file.contains("name: Order totals\n") && file.contains("SELECT 2"),
            "{file}"
        );
        assert_eq!(
            sim.tables["query_versions"].len(),
            versions,
            "no version from the syncs"
        );
    }
}
