//! The five `.seaquel` file formats (Decision 45): ports of today's
//! readers and writers, with CRLF and a leading BOM accepted, the quoting
//! fixed so every value reads back, a `,` quoted in lists, a description's
//! newline written as `\n`, and the stable file id (Q22) read as `file_id`
//! and written as the first line or key.
//!
//! Each kind's `*_content` is its canonical text without the id (and for a
//! dashboard without the viewport, for a template without its labels):
//! the text Decision 34's hash is taken over.

use std::collections::HashMap;
use std::fmt;

use seaquel_types::names::js_trim;
use seaquel_types::storage::PersistedQueryParameter;

use super::json::{self, J};
use super::yaml::{
    indented, key_line, normalise, read_list_in, read_value_in, skip_line, write_list, write_value,
    Quoting,
};
use crate::imports::parse_int;

/// The keys a template never carries, at the top level and in
/// `sshTunnel` (`CREDENTIAL_FIELDS`).
pub const CREDENTIAL_FIELDS: [&str; 6] = [
    "username",
    "password",
    "connectionString",
    "sshPassword",
    "sshKeyPassphrase",
    "sshUsername",
];

fn file_name(rel_path: &str) -> &str {
    rel_path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("untitled")
}

/// A file's name without `ext` (any case), `-` and `_` as spaces: the
/// name of a file that doesn't give one.
fn name_from_file(rel_path: &str, ext: &str) -> String {
    let f = file_name(rel_path);
    let stem = if f.len() >= ext.len()
        && f.is_char_boundary(f.len() - ext.len())
        && f[f.len() - ext.len()..].eq_ignore_ascii_case(ext)
    {
        &f[..f.len() - ext.len()]
    } else {
        f
    };
    stem.replace(['-', '_'], " ")
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

// ── Queries ──

/// A `.sql` file: YAML frontmatter, then the query.
#[derive(Clone, PartialEq, Default)]
pub struct QueryFile {
    pub name: String,
    pub description: Option<String>,
    /// `database:`, written as it is (engine names only).
    pub database: Option<String>,
    pub tags: Vec<String>,
    pub parameters: Vec<PersistedQueryParameter>,
    pub query: String,
    /// The path below `queries/`, `""` at its root. Not written: it is the
    /// file's place.
    pub folder: String,
    pub file_id: Option<String>,
}

impl fmt::Debug for QueryFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryFile")
            .field("description", &self.description.is_some())
            .field("tags", &self.tags.len())
            .field("parameters", &self.parameters.len())
            .field("query_bytes", &self.query.len())
            .field("file_id", &self.file_id.is_some())
            .finish_non_exhaustive()
    }
}

/// `^---\n(.*?)\n---\n(.*)$`: the frontmatter and the body, or `None`.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---\n")?;
    let at = rest.find("\n---\n")?;
    Some((&rest[..at], &rest[at + 5..]))
}

#[derive(Default)]
struct ParamDraft {
    name: Option<String>,
    ty: Option<String>,
    default_value: Option<String>,
    description: Option<String>,
}

impl ParamDraft {
    fn set(&mut self, key: &str, value: &str, quoting: Quoting) {
        let v = read_value_in(value, quoting);
        match key {
            "name" => self.name = Some(v),
            "type" => self.ty = Some(v),
            "default" | "defaultValue" => self.default_value = Some(v),
            "description" => self.description = Some(v),
            _ => {}
        }
    }

    fn finish(self) -> Option<PersistedQueryParameter> {
        let name = self.name.filter(|n| !n.is_empty())?;
        Some(PersistedQueryParameter {
            name,
            ty: self
                .ty
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "text".into()),
            default_value: self.default_value,
            description: self.description,
        })
    }
}

/// Reads a query file at `rel_path` (repo-relative) under `queries_dir`.
/// A file without frontmatter, or whose `name:` is empty, is named after
/// its file.
pub fn parse_query(text: &str, rel_path: &str, queries_dir: &str) -> QueryFile {
    parse_query_in(text, rel_path, queries_dir, None)
}

/// Whether `yaml` (a file's YAML, or a query's frontmatter) carries Core's
/// `id:` line with a value (Q22): then Core wrote it, and its double quotes
/// hold Core's escapes (probe fix 2).
fn quoting_of(yaml: &str) -> Quoting {
    let has_id = yaml
        .split('\n')
        .any(|line| key_line(line).is_some_and(|(k, v)| k == "id" && !v.is_empty()));
    if has_id {
        Quoting::Core
    } else {
        Quoting::Legacy
    }
}

/// [`parse_query`] of text Core wrote for a hash (a kind's `*_content`,
/// which has no id): Core's escapes always undone.
pub(crate) fn parse_query_core(text: &str, rel_path: &str, queries_dir: &str) -> QueryFile {
    parse_query_in(text, rel_path, queries_dir, Some(Quoting::Core))
}

fn parse_query_in(
    text: &str,
    rel_path: &str,
    queries_dir: &str,
    quoting: Option<Quoting>,
) -> QueryFile {
    let text = normalise(text);
    let mut q = QueryFile::default();
    match split_frontmatter(&text) {
        None => q.query = js_trim(&text).to_string(),
        Some((yaml, body)) => {
            let quoting = quoting.unwrap_or_else(|| quoting_of(yaml));
            q.query = js_trim(body).to_string();
            let mut in_params = false;
            let mut current: Option<ParamDraft> = None;
            for line in yaml.split('\n') {
                if skip_line(line) {
                    continue;
                }
                if let Some((key, value)) = key_line(line) {
                    if key == "parameters" {
                        in_params = true;
                        continue;
                    }
                    in_params = false;
                    match key {
                        "id" => q.file_id = non_empty(read_value_in(value, quoting)),
                        "name" => q.name = read_value_in(value, quoting),
                        "description" => q.description = Some(read_value_in(value, quoting)),
                        "database" => q.database = Some(read_value_in(value, quoting)),
                        "tags" => q.tags = read_list_in(value, quoting),
                        _ => {}
                    }
                    continue;
                }
                if !in_params {
                    continue;
                }
                if let Some((key, value)) = indented(line, true) {
                    if let Some(p) = current.take().and_then(ParamDraft::finish) {
                        q.parameters.push(p);
                    }
                    let mut p = ParamDraft::default();
                    p.set(key, value, quoting);
                    current = Some(p);
                } else if let (Some((key, value)), Some(p)) =
                    (indented(line, false), current.as_mut())
                {
                    p.set(key, value, quoting);
                }
            }
            if let Some(p) = current.and_then(ParamDraft::finish) {
                q.parameters.push(p);
            }
        }
    }
    if js_trim(&q.name).is_empty() {
        q.name = name_from_file(rel_path, ".sql");
    }
    let dir = rel_path.rsplit_once('/').map_or("", |(d, _)| d);
    q.folder = if dir == queries_dir {
        String::new()
    } else {
        match dir
            .strip_prefix(queries_dir)
            .and_then(|r| r.strip_prefix('/'))
        {
            Some(rest) => rest.to_string(),
            None => dir.to_string(),
        }
    };
    q
}

fn query_text(q: &QueryFile, with_id: bool) -> String {
    let mut lines = Vec::new();
    if let Some(id) = q.file_id.as_deref().filter(|_| with_id) {
        lines.push(format!("id: {}", write_value(id)));
    }
    lines.push(format!("name: {}", write_value(&q.name)));
    if let Some(d) = q.description.as_deref().filter(|d| !d.is_empty()) {
        lines.push(format!("description: {}", write_value(d)));
    }
    if let Some(db) = q.database.as_deref().filter(|d| !d.is_empty()) {
        lines.push(format!("database: {}", write_value(db)));
    }
    if !q.tags.is_empty() {
        lines.push(format!("tags: {}", write_list(&q.tags)));
    }
    if !q.parameters.is_empty() {
        lines.push("parameters:".into());
        for p in &q.parameters {
            lines.push(format!("  - name: {}", write_value(&p.name)));
            lines.push(format!("    type: {}", write_value(&p.ty)));
            if let Some(d) = &p.default_value {
                lines.push(format!("    default: {}", write_value(d)));
            }
            if let Some(d) = p.description.as_deref().filter(|d| !d.is_empty()) {
                lines.push(format!("    description: {}", write_value(d)));
            }
        }
    }
    format!(
        "---\n{}\n---\n{}\n",
        lines.join("\n"),
        q.query.replace("\r\n", "\n")
    )
}

/// The file Core writes for `q`, its id first.
pub fn write_query(q: &QueryFile) -> String {
    query_text(q, true)
}

/// `q`'s canonical text (no id).
pub fn query_content(q: &QueryFile) -> String {
    query_text(q, false)
}

// ── Dashboards ──

/// A dashboard's `.json` file. The JSON values are held as compact text
/// (`JSON.stringify`), as the rows store them.
#[derive(Clone, PartialEq, Default)]
pub struct DashboardFile {
    pub name: String,
    pub description: Option<String>,
    /// A JSON array, run state stripped.
    pub widgets: String,
    /// `None` leaves it out of the file (and of the content, Q24).
    pub viewport: Option<String>,
    pub date_filter: Option<String>,
    pub file_id: Option<String>,
}

impl fmt::Debug for DashboardFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardFile")
            .field("description", &self.description.is_some())
            .field("widgets_bytes", &self.widgets.len())
            .field("viewport", &self.viewport.is_some())
            .field("date_filter", &self.date_filter.is_some())
            .field("file_id", &self.file_id.is_some())
            .finish_non_exhaustive()
    }
}

/// The viewport a file without one gets.
pub const DEFAULT_VIEWPORT: &str = r#"{"x":0,"y":0,"zoom":1}"#;

fn strip_widgets(widgets: &J) -> Option<J> {
    match widgets {
        J::Arr(items) => items
            .iter()
            .map(json::strip_widget)
            .collect::<Option<Vec<J>>>()
            .map(J::Arr),
        _ => None,
    }
}

/// Reads a dashboard file, or `None` when it doesn't parse (not JSON,
/// `null` or another non-object, a `widgets` that isn't a list, a `null`
/// widget, or a `name` that is neither text nor empty). A missing or blank
/// name is the file's.
pub fn parse_dashboard(text: &str, rel_path: &str) -> Option<DashboardFile> {
    let data = json::parse(&normalise(text))?;
    if !matches!(data, J::Obj(_) | J::Arr(_)) {
        return None;
    }
    let name = match data.get("name") {
        Some(J::Str(s)) if !js_trim(s).is_empty() => s.clone(),
        Some(v) if v.truthy() && !matches!(v, J::Str(_)) => return None,
        _ => name_from_file(rel_path, ".json"),
    };
    let widgets = match data.get("widgets") {
        None | Some(J::Null) => J::Arr(Vec::new()),
        Some(w) => strip_widgets(w)?,
    };
    let viewport = match data.get("viewport") {
        None | Some(J::Null) => DEFAULT_VIEWPORT.to_string(),
        Some(v) => v.compact(),
    };
    let date_filter = match data.get("dateFilter") {
        None | Some(J::Null) => None,
        Some(f) => Some(f.compact()),
    };
    let description = match data.get("description") {
        Some(J::Str(s)) => Some(s.clone()),
        _ => None,
    };
    let file_id = match data.get("id") {
        Some(J::Str(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    Some(DashboardFile {
        name,
        description,
        widgets: widgets.compact(),
        viewport: Some(viewport),
        date_filter,
        file_id,
    })
}

fn dashboard_text(d: &DashboardFile, with_id: bool, with_viewport: bool) -> String {
    let mut obj: Vec<(String, J)> = Vec::new();
    if let Some(id) = d.file_id.as_deref().filter(|_| with_id) {
        obj.push(("id".into(), J::Str(id.to_string())));
    }
    obj.push(("name".into(), J::Str(d.name.clone())));
    if let Some(desc) = d.description.as_deref().filter(|d| !d.is_empty()) {
        obj.push(("description".into(), J::Str(desc.to_string())));
    }
    let widgets = json::parse(&d.widgets)
        .and_then(|w| match &w {
            J::Arr(items) => Some(J::Arr(
                items
                    .iter()
                    .map(|i| json::strip_widget(i).unwrap_or(J::Null))
                    .collect(),
            )),
            _ => None,
        })
        .unwrap_or(J::Arr(Vec::new()));
    obj.push(("widgets".into(), widgets));
    if with_viewport {
        if let Some(v) = &d.viewport {
            let v = json::parse(v)
                .or_else(|| json::parse(DEFAULT_VIEWPORT))
                .unwrap_or(J::Null);
            obj.push(("viewport".into(), v));
        }
    }
    if let Some(f) = d.date_filter.as_deref().and_then(json::parse) {
        if f.truthy() {
            obj.push(("dateFilter".into(), f));
        }
    }
    let mut out = J::Obj(obj).pretty();
    out.push('\n');
    out
}

/// The file Core writes for `d`: pretty-printed JSON, its id first.
pub fn write_dashboard(d: &DashboardFile) -> String {
    dashboard_text(d, true, true)
}

/// `d`'s canonical text: no id, no viewport (Q24).
pub fn dashboard_content(d: &DashboardFile) -> String {
    dashboard_text(d, false, false)
}

// ── Connection templates ──

/// A template's SSH tunnel, present only when the file enables it.
#[derive(Clone, PartialEq, Default)]
pub struct TemplateSsh {
    pub host: Option<String>,
    pub port: Option<f64>,
}

impl fmt::Debug for TemplateSsh {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateSsh")
            .field("host", &self.host.is_some())
            .field("port", &self.port.is_some())
            .finish()
    }
}

/// A connection template (`connections/<file>.yaml`): no credentials.
#[derive(Clone, PartialEq, Default)]
pub struct TemplateFile {
    pub name: String,
    pub ty: String,
    pub host: String,
    pub port: f64,
    pub database_name: String,
    pub ssl_mode: Option<String>,
    pub ssh_tunnel: Option<TemplateSsh>,
    /// Read and kept, never applied (Decision 46).
    pub labels: Vec<String>,
    pub file_id: Option<String>,
}

impl fmt::Debug for TemplateFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateFile")
            .field("ty", &self.ty)
            .field("ssl_mode", &self.ssl_mode.is_some())
            .field("ssh_tunnel", &self.ssh_tunnel.is_some())
            .field("labels", &self.labels.len())
            .field("file_id", &self.file_id.is_some())
            .finish_non_exhaustive()
    }
}

/// `getDefaultPort`: an engine's port, 5432 for any other type.
pub fn default_port(ty: &str) -> f64 {
    match ty {
        "mysql" | "mariadb" => 3306.0,
        "mssql" => 1433.0,
        _ => 5432.0,
    }
}

/// `parseInt(v, 10) || fallback`.
fn int_or(v: Option<&str>, fallback: f64) -> f64 {
    match v.and_then(parse_int) {
        Some(n) if n != 0.0 => n,
        _ => fallback,
    }
}

/// Reads a template, or `None` without a name. Credentials are dropped at
/// both levels; `type` defaults to `postgres`, `host` to `localhost`, the
/// port to the engine's.
pub fn parse_template(text: &str) -> Option<TemplateFile> {
    parse_template_in(text, None)
}

/// [`parse_template`] of text Core wrote for a hash (no id): Core's
/// escapes always undone.
pub(crate) fn parse_template_core(text: &str) -> Option<TemplateFile> {
    parse_template_in(text, Some(Quoting::Core))
}

fn parse_template_in(text: &str, quoting: Option<Quoting>) -> Option<TemplateFile> {
    let text = normalise(text);
    let quoting = quoting.unwrap_or_else(|| quoting_of(&text));
    // A repeated key keeps its last value; looked up, not scanned (I4).
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut labels = Vec::new();
    let mut ssh: Option<(bool, Option<String>, Option<f64>)> = None;
    let mut in_ssh = false;
    for line in text.split('\n') {
        if skip_line(line) {
            continue;
        }
        if let Some((key, value)) = key_line(line) {
            if CREDENTIAL_FIELDS.contains(&key) {
                in_ssh = false;
                continue;
            }
            if key == "sshTunnel" {
                in_ssh = true;
                ssh = Some((false, None, None));
                continue;
            }
            in_ssh = false;
            if key == "labels" {
                labels = read_list_in(value, quoting);
                continue;
            }
            fields.insert(key.to_string(), read_value_in(value, quoting));
            continue;
        }
        if !in_ssh {
            continue;
        }
        if let (Some((key, value)), Some(s)) = (indented(line, false), ssh.as_mut()) {
            if CREDENTIAL_FIELDS.contains(&key) {
                continue;
            }
            match key {
                "enabled" => s.0 = value == "true",
                "host" => s.1 = Some(read_value_in(value, quoting)),
                "port" => s.2 = Some(int_or(Some(value), 22.0)),
                _ => {}
            }
        }
    }
    let field = |k: &str| fields.get(k).cloned();
    let name = field("name").filter(|n| !n.is_empty())?;
    let ty = field("type").unwrap_or_else(|| "postgres".into());
    let port = int_or(field("port").as_deref(), default_port(&ty));
    Some(TemplateFile {
        name,
        host: field("host").unwrap_or_else(|| "localhost".into()),
        port,
        database_name: field("databaseName").unwrap_or_default(),
        ssl_mode: field("sslMode").filter(|s| !s.is_empty()),
        ssh_tunnel: ssh
            .filter(|s| s.0)
            .map(|(_, host, port)| TemplateSsh { host, port }),
        labels,
        file_id: field("id").filter(|s| !s.is_empty()),
        ty,
    })
}

/// A port as an integer (I3); a port that isn't a whole number in
/// 0–65535 never reaches a file, since the library refuses it (C1).
fn port_text(port: f64) -> String {
    if port.is_finite() && port.abs() < 1e15 {
        format!("{}", port.trunc() as i64)
    } else {
        json::js_number(port)
    }
}

fn template_text(t: &TemplateFile, with_id: bool, with_labels: bool) -> String {
    let mut lines = Vec::new();
    if let Some(id) = t.file_id.as_deref().filter(|_| with_id) {
        lines.push(format!("id: {}", write_value(id)));
    }
    lines.push(format!("name: {}", write_value(&t.name)));
    lines.push(format!("type: {}", write_value(&t.ty)));
    lines.push(format!("host: {}", write_value(&t.host)));
    lines.push(format!("port: {}", port_text(t.port)));
    lines.push(format!("databaseName: {}", write_value(&t.database_name)));
    if let Some(s) = t.ssl_mode.as_deref().filter(|s| !s.is_empty()) {
        lines.push(format!("sslMode: {}", write_value(s)));
    }
    if with_labels && !t.labels.is_empty() {
        lines.push(format!("labels: {}", write_list(&t.labels)));
    }
    if let Some(ssh) = &t.ssh_tunnel {
        lines.push("sshTunnel:".into());
        lines.push("  enabled: true".into());
        lines.push(format!(
            "  host: {}",
            write_value(ssh.host.as_deref().unwrap_or(""))
        ));
        lines.push(format!("  port: {}", port_text(ssh.port.unwrap_or(22.0))));
    }
    lines.join("\n") + "\n"
}

/// The file Core writes for `t`, its id first. Never a credential: the
/// type has none.
pub fn write_template(t: &TemplateFile) -> String {
    template_text(t, true, true)
}

/// `t`'s canonical text: no id, no labels (Decision 41's fields).
pub fn template_content(t: &TemplateFile) -> String {
    template_text(t, false, false)
}

// ── project.yaml and labels.yaml ──

/// `project.yaml`: a missing name is the directory's.
#[derive(Clone, PartialEq, Default)]
pub struct ProjectFile {
    pub name: String,
    pub description: Option<String>,
    pub dir: String,
}

impl fmt::Debug for ProjectFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectFile")
            .field("description", &self.description.is_some())
            .finish_non_exhaustive()
    }
}

pub fn parse_project(text: &str, dir: &str) -> ProjectFile {
    let text = normalise(text);
    let mut p = ProjectFile {
        name: dir.to_string(),
        description: None,
        dir: dir.to_string(),
    };
    for line in text.split('\n') {
        if skip_line(line) {
            continue;
        }
        if let Some((key, value)) = key_line(line) {
            // `project.yaml` never has an id, and Core rewrites it (Q25),
            // so its own escapes are read (probe fix 2's known limit: a
            // 2026.9.x name with `\` before `"`, `\` or `n` reads as Core's
            // escape there).
            let v = read_value_in(value, Quoting::Core);
            match key {
                "name" => p.name = v,
                "description" => p.description = non_empty(v),
                _ => {}
            }
        }
    }
    p
}

pub fn write_project(p: &ProjectFile) -> String {
    let mut out = format!("name: {}\n", write_value(&p.name));
    if let Some(d) = p.description.as_deref().filter(|d| !d.is_empty()) {
        out.push_str(&format!("description: {}\n", write_value(d)));
    }
    out
}

/// A label from `labels.yaml` (read, never applied: Decision 46).
#[derive(Clone, PartialEq, Default)]
pub struct LabelFile {
    /// `shared-<name in lower case, whitespace runs as ->`.
    pub id: String,
    pub name: String,
    pub color: String,
}

impl fmt::Debug for LabelFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LabelFile").finish_non_exhaustive()
    }
}

fn label_id(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut out = String::from("shared-");
    let mut space = false;
    for c in lower.chars() {
        if seaquel_types::names::is_js_space(c) {
            space = true;
            continue;
        }
        if space {
            out.push('-');
            space = false;
        }
        out.push(c);
    }
    if space {
        out.push('-');
    }
    out
}

/// `labels.yaml`'s list; it ends at the next top-level key.
pub fn parse_labels(text: &str) -> Vec<LabelFile> {
    let text = normalise(text);
    let mut out = Vec::new();
    let mut in_labels = false;
    let mut current: Option<(Option<String>, Option<String>)> = None;
    let finish = |cur: Option<(Option<String>, Option<String>)>, out: &mut Vec<LabelFile>| {
        if let Some((Some(name), color)) = cur {
            if !name.is_empty() {
                out.push(LabelFile {
                    id: label_id(&name),
                    color: color.unwrap_or_else(|| "#6b7280".into()),
                    name,
                });
            }
        }
    };
    for line in text.split('\n') {
        if skip_line(line) {
            continue;
        }
        if let Some(("labels", _)) = key_line(line) {
            in_labels = true;
            continue;
        }
        if !in_labels {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        let set = |cur: &mut (Option<String>, Option<String>), key: &str, value: &str| match key {
            "name" => cur.0 = Some(read_value_in(value, Quoting::Core)),
            "color" => cur.1 = Some(read_value_in(value, Quoting::Core)),
            _ => {}
        };
        if let Some((key, value)) = indented(line, true) {
            finish(current.take(), &mut out);
            let mut cur = (None, None);
            set(&mut cur, key, value);
            current = Some(cur);
        } else if let (Some((key, value)), Some(cur)) = (indented(line, false), current.as_mut()) {
            set(cur, key, value);
        }
    }
    finish(current, &mut out);
    out
}

pub fn write_labels(labels: &[LabelFile]) -> String {
    let mut lines = vec!["labels:".to_string()];
    for l in labels {
        lines.push(format!("  - name: {}", write_value(&l.name)));
        lines.push(format!("    color: \"{}\"", l.color));
    }
    lines.join("\n") + "\n"
}
