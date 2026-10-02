//! The projection replay through Core (phase 5e, Task 5).
//!
//! Each case of `seaquel-workspace/tests/fixtures/shared/projection.json`
//! runs on a real temp directory (its `/repos`, `/home` and `/outside`
//! rooted in it, real symlinks, `unreadable` directories at mode 000), a
//! real `git init` of each repo with the seed committed and pushed to a
//! bare `origin`, and a real `Storage` (the beta-era file for
//! `v2026.4.5-beta.1`) with a test secret store. Each step's `core` calls go
//! to Core's methods; a `git.pull` first plays the case's scripted pull
//! through a teammate's clone (its conflicts made real: both sides change
//! the file, then the conflicted files are given the scripted text), and
//! `failing` paths fail through Core's per-path write hook.
//!
//! After every step it compares, with `changes.json` applied exactly:
//! the rows of every recorded table (link columns, `name_key` and other
//! columns the recording doesn't have dropped after their checks;
//! `shared_repos` by `id`, `path` and `name`, and byte for byte where
//! `*` (5) says), the whole tree (Core's id line or key checked as a v4
//! uuid and removed from the files Core wrote), the step's `links`,
//! `notices`, `projection`, `outcome` and `pathless`, `*` (4a) and (4b),
//! and the keychain (`secrets` minus the removed connections').

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};

use seaquel_core::domain::library::{
    name_key, ConnectionPatch, ProjectPatch, SavedQueryDraft, SavedQueryPatch,
};
use seaquel_core::domain::shared::format::{parse_dashboard, parse_query, parse_template};
use seaquel_core::domain::shared::plan::{row_hash, template_path};
use seaquel_core::domain::shared::{file_hash, Kind, Link, RowChange};
use seaquel_core::domain::state::{DashboardDraft, DashboardPatch};
use seaquel_core::storage::{connections, dashboards, saved_queries};
use seaquel_core::{SyncTarget, WriteOrigin};
use serde_json::value::RawValue;
use serde_json::{json, Map, Value};

use super::world::{commit_all, git, has_head, World};
use crate::common::{dump, insert_rows};

const BETA_SCHEMA: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-storage/tests/fixtures/schemas/v2026.4.5-beta.1.sql"
);
const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../seaquel-workspace/tests/fixtures/shared"
);
const ROOTS: [&str; 3] = ["repos", "home", "outside"];
const SEED_TABLES: [&str; 9] = [
    "projects",
    "connections",
    "project_state",
    "saved_queries",
    "query_versions",
    "dashboards",
    "dashboard_versions",
    "shared_repos",
    "app_state",
];
const COMPARED: [&str; 8] = [
    "projects",
    "connections",
    "project_state",
    "saved_queries",
    "query_versions",
    "dashboards",
    "dashboard_versions",
    "shared_repos",
];

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

/// `2030-01-01T00:00:00.000Z`.
fn is_time(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 24
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && b[19] == b'.'
        && b[23] == b'Z'
}

// ── Matching, with `<id:n>`, `<core:n>` and `<now>` ──

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
            if let Some(rest) = e.strip_prefix("<now>") {
                if a.len() >= 24 && a.is_char_boundary(24) && is_time(&a[..24]) {
                    e = rest;
                    a = &a[24..];
                    continue;
                }
                return false;
            }
            if let Some(tok) = token_at(e) {
                let value = if a.starts_with(tok) {
                    tok
                } else if a.len() >= 36 && a.is_char_boundary(36) && is_v4(&a[..36]) {
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

/// JSON text without the whitespace between tokens, key order kept.
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

pub fn fixtures() -> (Vec<Value>, Map<String, Value>) {
    let p = serde_json::from_str(
        &std::fs::read_to_string(format!("{FIXTURES}/projection.json")).unwrap(),
    )
    .unwrap();
    let c =
        serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/changes.json")).unwrap())
            .unwrap();
    (p, c)
}

fn raw_cases() -> Vec<RawCase> {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIXTURES}/projection.json")).unwrap())
        .unwrap()
}

/// The columns each recorded table has, across every case.
fn recorded_columns(cases: &[Value]) -> HashMap<String, HashSet<String>> {
    let mut cols: HashMap<String, HashSet<String>> = HashMap::new();
    let mut add = |rows: &Value| {
        for (t, rows) in rows.as_object().unwrap() {
            for r in rows.as_array().unwrap() {
                cols.entry(t.clone())
                    .or_default()
                    .extend(r.as_object().unwrap().keys().cloned());
            }
        }
    };
    for c in cases {
        add(&c["seed"]["rows"]);
        for s in c["steps"].as_array().unwrap() {
            add(&s["rows"]);
        }
    }
    cols
}

#[derive(Default)]
struct CallResult {
    ok: bool,
    error: Option<String>,
    conflicted: bool,
    projection: Option<(String, Option<String>)>,
    notices: Vec<Value>,
}

/// A scripted pull waiting for Core's `git.pull`.
struct Script {
    repo: PathBuf,
    files: Vec<(String, Option<String>)>,
    conflicts: HashSet<String>,
}

struct Replay {
    w: World,
    case: Value,
    origin: WriteOrigin,
    pulls: HashMap<String, usize>,
    secrets: BTreeMap<String, String>,
    repo_calls: HashSet<String>,
    seeded_repos: HashMap<String, String>,
    errors: Vec<String>,
}

impl Replay {
    async fn new(case: &Value) -> Replay {
        let schema = (case["file"] == "v2026.4.5-beta.1")
            .then(|| std::fs::read_to_string(BETA_SCHEMA).unwrap());
        let w = World::with_file(schema.as_deref()).await;
        let mut r = Replay {
            w,
            case: case.clone(),
            origin: WriteOrigin::new(Some("main")),
            pulls: HashMap::new(),
            secrets: BTreeMap::new(),
            repo_calls: HashSet::new(),
            seeded_repos: HashMap::new(),
            errors: Vec::new(),
        };
        r.seed().await;
        r
    }

    /// `/repos/a` → `<root>/repos/a`, in a plain value or inside JSON text.
    fn map_in(&self, s: &str) -> String {
        let root = self.w.root.to_string_lossy();
        let mut out = s.to_string();
        for r in ROOTS {
            out = out.replace(&format!("\"/{r}/"), &format!("\"{root}/{r}/"));
            out = out.replace(&format!("\"/{r}\""), &format!("\"{root}/{r}\""));
        }
        for r in ROOTS {
            if out.starts_with(&format!("/{r}/")) || out == format!("/{r}") {
                out = format!("{root}{out}");
                break;
            }
        }
        out
    }

    fn map_out(&self, s: &str) -> String {
        s.replace(&*self.w.root.to_string_lossy(), "")
    }

    fn map_value_in(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.map_in(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.map_value_in(x)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), self.map_value_in(x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    async fn seed(&mut self) {
        let seed = self.case["seed"].clone();
        for table in SEED_TABLES {
            if let Some(rows) = seed["rows"][table].as_array() {
                let rows: Vec<Value> = rows.iter().map(|r| self.map_value_in(r)).collect();
                insert_rows(self.w.ws.storage(), table, &rows).await;
            }
        }
        for r in seed["rows"]["shared_repos"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let mapped = self.map_value_in(r);
            self.seeded_repos.insert(
                mapped["id"].as_str().unwrap().to_string(),
                mapped["data"].as_str().unwrap().to_string(),
            );
        }
        if let Some(secrets) = self.case["secrets"].as_object() {
            for (k, v) in secrets {
                self.w.store.put(k, v.as_str().unwrap());
                self.secrets
                    .insert(k.clone(), v.as_str().unwrap().to_string());
            }
        }
        // The disk: directories, files, symlinks.
        let mut repos = Vec::new();
        for (path, v) in seed["tree"].as_object().unwrap() {
            let abs = self.w.abs(path);
            match v {
                Value::String(t) => {
                    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
                    std::fs::write(&abs, t).unwrap();
                }
                Value::Object(o) if o.contains_key("symlink") => {
                    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
                    let target = o["symlink"].as_str().unwrap();
                    let target = if target.starts_with('/') {
                        self.w.abs(target)
                    } else {
                        PathBuf::from(target)
                    };
                    symlink(target, &abs).unwrap();
                }
                _ => {
                    if let Some(repo) = path.strip_suffix("/.git") {
                        repos.push(repo.to_string());
                    } else {
                        std::fs::create_dir_all(&abs).unwrap();
                    }
                }
            }
        }
        // Every repo the projects name is a git repo, seeded and pushed.
        for p in seed["rows"]["projects"].as_array().into_iter().flatten() {
            if let Some(path) = p["git_repo_path"].as_str() {
                if !repos.iter().any(|r| r == path) && self.w.abs(path).is_dir() {
                    repos.push(path.to_string());
                }
            }
        }
        for repo in &repos {
            let name = repo.trim_start_matches("/repos/");
            let abs = self.w.abs(repo);
            assert!(
                abs.starts_with(self.w.root.join("repos")),
                "a repo outside /repos"
            );
            let _ = name;
            self.git_repo(repo);
        }
        for d in seed["unreadable"].as_array().into_iter().flatten() {
            let abs = self.w.abs(d.as_str().unwrap());
            std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
        let failing: HashSet<PathBuf> = seed["failing"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|p| self.w.abs(p.as_str().unwrap()))
            .collect();
        *self.w.hook.failing.lock().unwrap() = failing;
        *self.w.hook.fold_case.lock().unwrap() = seed["caseInsensitive"].as_bool().unwrap_or(false);
    }

    fn remote_of(&self, repo: &str) -> PathBuf {
        let key = repo.trim_start_matches('/').replace('/', "_");
        self.w.data.join(format!("remotes/{key}.git"))
    }

    fn git_repo(&self, repo: &str) {
        let abs = self.w.abs(repo);
        std::fs::create_dir_all(&abs).unwrap();
        git(&abs, &["init", "-q"]);
        commit_all(&abs, "seed");
        let bare = self.remote_of(repo);
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare"]);
        git(
            &abs,
            &[
                "remote",
                "add",
                "origin",
                &format!("file://{}", bare.display()),
            ],
        );
        if has_head(&abs) {
            git(&abs, &["push", "-q", "origin", "main"]);
        }
    }

    /// The case's scripted pull for `repo` (the next one of a queue),
    /// committed in a teammate's clone and pushed; the local repo's own
    /// changes are committed and pushed first, so the pull fast-forwards.
    /// Conflicts are made real: the local repo and the teammate both
    /// change the file.
    fn script_pull(&mut self, repo: &str) -> Option<Script> {
        let abs = self.w.abs(repo);
        commit_all(&abs, "local");
        git(&abs, &["push", "-q", "origin", "main"]);
        let n = self.pulls.entry(repo.to_string()).or_insert(0);
        let g = &self.case["seed"]["git"][repo];
        let pull = match &g["pull"] {
            Value::Array(a) => a.get(*n).cloned(),
            Value::Null => None,
            other => (*n == 0).then(|| other.clone()),
        };
        *n += 1;
        let k = *n;
        let pull = pull?;
        let conflicts: HashSet<String> = pull["conflicts"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| c.as_str().unwrap().to_string())
            .collect();
        let files: Vec<(String, Option<String>)> = pull["files"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(rel, v)| (rel.clone(), v.as_str().map(String::from)))
            .collect();
        let bare = self.remote_of(repo);
        let clone = self.w.data.join(format!(
            "clones/{}-{k}",
            repo.trim_start_matches('/').replace('/', "_")
        ));
        std::fs::create_dir_all(clone.parent().unwrap()).unwrap();
        git(
            clone.parent().unwrap(),
            &[
                "clone",
                "-q",
                &format!("file://{}", bare.display()),
                &clone.to_string_lossy(),
            ],
        );
        for (rel, text) in &files {
            let p = clone.join(rel);
            if conflicts.contains(rel) {
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, "teammate side\n").unwrap();
                continue;
            }
            match text {
                Some(t) => {
                    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                    std::fs::write(&p, t).unwrap();
                }
                None => {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        commit_all(&clone, "teammate");
        git(&clone, &["push", "-q", "origin", "main"]);
        if !conflicts.is_empty() {
            for c in &conflicts {
                std::fs::write(abs.join(c), "local side\n").unwrap();
            }
            commit_all(&abs, "local side");
        }
        Some(Script {
            repo: abs,
            files,
            conflicts,
        })
    }

    /// After Core's pull: the conflicted files get the scripted text, and
    /// every file the pull changed is the teammate's, not Core's.
    fn finish_pull(&mut self, script: Script) {
        let mut written = self.w.hook.written.lock().unwrap();
        for (rel, text) in &script.files {
            let p = script.repo.join(rel);
            written.remove(&p);
            if script.conflicts.contains(rel) {
                std::fs::write(&p, text.as_deref().unwrap_or("")).unwrap();
            }
        }
    }

    async fn call(
        &mut self,
        call: &Value,
        raw_params: &str,
        bindings: &mut Bindings,
    ) -> CallResult {
        let raw = self.map_in(&minify(&bindings.apply(raw_params)));
        let params: Value = serde_json::from_str(&raw).unwrap();
        let members: HashMap<String, Box<RawValue>> = serde_json::from_str(&raw).unwrap();
        let member = |k: &str| members[k].get().to_string();
        let s = |k: &str| params[k].as_str().unwrap().to_string();
        let group = call["group"].as_str().unwrap().to_string();
        let method = call["method"].as_str().unwrap().to_string();
        let repo_call = (group == "shared"
            && matches!(
                method.as_str(),
                "linkProject" | "unlinkProject" | "importProjects"
            ))
            || (group == "git" && matches!(method.as_str(), "pull" | "push"));
        if repo_call {
            // `*` (5): which seeded repos a repo call may change.
            let named_path = params
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string);
            let ids: Vec<String> = self.seeded_repos.keys().cloned().collect();
            for id in ids {
                let data = &self.seeded_repos[&id];
                let named = named_path
                    .as_deref()
                    .is_some_and(|p| data.contains(&format!("\"path\":{}", json!(p))));
                if named || method == "unlinkProject" || method == "linkProject" {
                    self.repo_calls.insert(format!("{id}:{method}"));
                }
            }
        }
        let mut res = CallResult {
            ok: true,
            ..Default::default()
        };
        let mut errs: Vec<String> = Vec::new();
        macro_rules! fail {
            ($e:expr) => {{
                let e: seaquel_core::CoreError = $e;
                res.ok = false;
                res.error = Some(format!("{group}.{method}: {}: {}", e.code, e.message));
            }};
        }
        fn report(res: &mut CallResult, r: &seaquel_core::SyncReport) {
            res.conflicted |= r.conflicted;
            for f in &r.failures {
                res.ok = false;
                res.error = Some(format!("a project failed: {}: {}", f.code, f.message));
            }
            for n in &r.notices {
                res.notices.push(serde_json::to_value(n).unwrap());
            }
        }
        fn proj(
            p: &Option<seaquel_core::domain::shared::PublishOutcome>,
        ) -> Option<(String, Option<String>)> {
            p.as_ref().map(|p| {
                (
                    serde_json::to_value(&p.status)
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .to_string(),
                    p.code.clone(),
                )
            })
        }
        if group == "git" && method == "pull" {
            let fixture_path = self.map_out(&s("path"));
            let script = self.script_pull(&fixture_path);
            let git = self.w.git_client();
            let pulled = self
                .w
                .ws
                .shared_git_pull(&self.w.core, &self.origin, &git, &s("path"), None)
                .await;
            if let Some(script) = script {
                self.finish_pull(script);
            }
            if let Err(e) = pulled {
                fail!(e);
            }
            self.errors.extend(errs);
            return res;
        }
        let (core, ws, origin) = (&self.w.core, self.w.ws.clone(), self.origin.clone());
        match (group.as_str(), method.as_str()) {
            ("shared", "sync") => {
                match ws
                    .shared_sync(core, &origin, SyncTarget::Project(s("projectId")))
                    .await
                {
                    Ok(r) => report(&mut res, &r.value),
                    Err(e) => fail!(e),
                }
            }
            ("shared", "syncRepo") => {
                match ws
                    .shared_sync(core, &origin, SyncTarget::Repo(s("repoId")))
                    .await
                {
                    Ok(r) => report(&mut res, &r.value),
                    Err(e) => fail!(e),
                }
            }
            ("shared", "linkProject") => {
                let share: Vec<String> = serde_json::from_value(params["share"].clone()).unwrap();
                match ws
                    .shared_link_project(core, &origin, &s("projectId"), &s("path"), &share)
                    .await
                {
                    Ok(r) => report(&mut res, &r.value),
                    Err(e) => fail!(e),
                }
            }
            ("shared", "unlinkProject") => {
                // Q31: a call recorded without `removeImported` is the
                // recording's GUI, which removed the template connections:
                // the user confirmed (fixtures README, Corrections).
                let remove = params
                    .get("removeImported")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                if let Err(e) = ws
                    .shared_unlink_project(core, &origin, &s("projectId"), remove)
                    .await
                {
                    fail!(e);
                }
            }
            ("shared", "scan") => {
                if let Err(e) = ws.shared_scan(core, &s("path")).await {
                    fail!(e);
                }
            }
            ("shared", "importProjects") => {
                let dirs: Vec<String> = serde_json::from_value(params["dirs"].clone()).unwrap();
                match ws
                    .shared_import_projects(core, &origin, &s("path"), &dirs)
                    .await
                {
                    Ok(r) => {
                        for f in &r.value.failures {
                            res.ok = false;
                            res.error =
                                Some(format!("an import failed: {}: {}", f.code, f.message));
                        }
                    }
                    Err(e) => fail!(e),
                }
            }
            ("git", "commit") => {
                let git = self.w.git_client();
                let message = params["message"].as_str().unwrap_or("commit").to_string();
                if let Err(e) = ws.shared_git_commit(core, &git, &s("path"), &message).await {
                    fail!(e);
                }
            }
            ("library", m) => {
                let binds = call.get("binds").and_then(Value::as_str);
                let made: Result<Option<String>, seaquel_core::CoreError> = match m {
                    "savedQueryCreate" => {
                        let draft: SavedQueryDraft =
                            serde_json::from_str(&member("query")).unwrap();
                        ws.create_saved_query(core, &origin, draft).await.map(|r| {
                            res.projection = proj(&r.projection);
                            Some(r.value.id)
                        })
                    }
                    "savedQueryUpdate" => {
                        let patch: SavedQueryPatch =
                            serde_json::from_str(&member("patch")).unwrap();
                        ws.update_saved_query(core, &origin, &s("id"), patch)
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    "savedQueryRemove" => {
                        ws.remove_saved_query(core, &origin, &s("id"))
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    "dashboardCreate" => {
                        let draft: DashboardDraft =
                            serde_json::from_str(&member("dashboard")).unwrap();
                        ws.create_dashboard(core, &origin, draft).await.map(|r| {
                            res.projection = proj(&r.projection);
                            Some(r.value.id)
                        })
                    }
                    "dashboardUpdate" => {
                        let patch: DashboardPatch = serde_json::from_str(&member("patch")).unwrap();
                        ws.update_dashboard(core, &origin, &s("id"), patch)
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    "dashboardRemove" => {
                        ws.remove_dashboard(core, &origin, &s("id")).await.map(|r| {
                            res.projection = proj(&r.projection);
                            None
                        })
                    }
                    "connectionUpdate" => {
                        let patch: ConnectionPatch =
                            serde_json::from_str(&member("patch")).unwrap();
                        ws.update_connection(core, &origin, &s("id"), patch, Default::default())
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    "connectionRemove" => {
                        ws.remove_connection(core, &origin, &s("id"))
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    "projectUpdate" => {
                        let patch: ProjectPatch = serde_json::from_str(&member("patch")).unwrap();
                        ws.update_project(core, &origin, &s("id"), patch)
                            .await
                            .map(|r| {
                                res.projection = proj(&r.projection);
                                None
                            })
                    }
                    other => {
                        errs.push(format!("library.{other} isn't replayed"));
                        Ok(None)
                    }
                };
                match made {
                    Ok(Some(id)) => {
                        if let Some(tok) =
                            binds.and_then(|b| b.find('<').and_then(|at| token_at(&b[at..])))
                        {
                            let uuid = &id[id.len().saturating_sub(36)..];
                            if !bindings.bind(tok, uuid) {
                                errs.push(format!("{tok} was bound already"));
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => fail!(e),
                }
            }
            (g, m) => errs.push(format!("{g}.{m} isn't replayed")),
        }
        self.errors.extend(errs);
        res
    }

    // ── What the step shows ──

    async fn table(&self, t: &str) -> Vec<Value> {
        let order = match t {
            "project_state" => "project_id",
            _ => "rowid",
        };
        dump(self.w.ws.storage(), t, order).await
    }

    /// The compared tables, mapped back to the fixture's paths, with the
    /// columns the recording doesn't have dropped.
    async fn dump_rows(
        &mut self,
        cols: &HashMap<String, HashSet<String>>,
    ) -> BTreeMap<String, Vec<Value>> {
        let mut out = BTreeMap::new();
        for t in COMPARED {
            let rows = self.table(t).await;
            let keep = cols.get(t).cloned().unwrap_or_default();
            let mut mapped = Vec::new();
            for row in rows {
                let mut obj = row.as_object().unwrap().clone();
                if let (Some(Value::String(k)), Some(Value::String(n))) =
                    (obj.get("name_key"), obj.get("name"))
                {
                    if *k != name_key(n) {
                        self.errors.push(format!("{t}: a stale name_key"));
                    }
                }
                if t == "shared_repos" {
                    let id = obj["id"].as_str().unwrap().to_string();
                    let data = obj["data"].as_str().unwrap().to_string();
                    self.check_repo_bytes(&id, &data);
                    let parsed: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
                    mapped.push(json!({
                        "id": id,
                        "path": parsed["path"].as_str().map(|p| self.map_out(p)),
                        "name": parsed["name"],
                    }));
                    continue;
                }
                obj.retain(|k, _| keep.contains(k));
                let v = Value::Object(obj);
                mapped.push(serde_json::from_str(&self.map_out(&v.to_string())).unwrap());
            }
            if !mapped.is_empty() {
                out.insert(t.to_string(), mapped);
            }
        }
        out
    }

    /// `*` (5): a seeded repo no repo call named keeps its data byte for
    /// byte; one only a pull or push named, all but `lastSyncAt`.
    fn check_repo_bytes(&mut self, id: &str, data: &str) {
        let Some(seeded) = self.seeded_repos.get(id).cloned() else {
            return;
        };
        let calls: Vec<&String> = self
            .repo_calls
            .iter()
            .filter(|c| c.starts_with(&format!("{id}:")))
            .collect();
        if calls.is_empty() {
            if data != seeded {
                self.errors
                    .push(format!("repo {id}'s data changed with no repo call"));
            }
        } else if calls
            .iter()
            .all(|c| c.ends_with(":pull") || c.ends_with(":push"))
        {
            let strip = |s: &str| -> Value {
                let mut v: Value = serde_json::from_str(s).unwrap_or(Value::Null);
                if let Some(o) = v.as_object_mut() {
                    o.remove("lastSyncAt");
                }
                v
            };
            if strip(data) != strip(&seeded) {
                self.errors
                    .push(format!("repo {id}'s data changed beyond lastSyncAt"));
            }
        }
    }

    fn walk(&self, dir: &Path, out: &mut BTreeMap<String, Value>) {
        let locked = std::fs::read_dir(dir).is_err();
        if locked {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let ty = e.file_type().unwrap();
            if ty.is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.insert(
                    self.map_out(&p.to_string_lossy()),
                    json!({"symlink": self.map_out(&target.to_string_lossy())}),
                );
            } else if ty.is_dir() {
                if name == ".git" {
                    continue;
                }
                self.walk(&p, out);
            } else {
                let text = std::fs::read_to_string(&p).unwrap_or_default();
                let written = self.w.hook.written.lock().unwrap().contains(&p);
                let shown = if written {
                    match strip_file_id(&p.to_string_lossy(), &text) {
                        Ok(t) => t,
                        Err(why) => format!("<{why}> {text}"),
                    }
                } else {
                    text
                };
                out.insert(self.map_out(&p.to_string_lossy()), json!(shown));
            }
        }
        if locked {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }
    }

    fn dump_tree(&self) -> Value {
        let mut out = BTreeMap::new();
        for r in ROOTS {
            let d = self.w.root.join(r);
            if d.is_dir() {
                self.walk(&d, &mut out);
            }
        }
        Value::Object(out.into_iter().collect())
    }

    /// `*` (4a) and (4b), against the database and the disk.
    async fn invariants(&self, all_ok: bool, pathless: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        let projects = self.table("projects").await;
        for (t, kind) in [
            ("saved_queries", Kind::SavedQuery),
            ("dashboards", Kind::Dashboard),
            ("connections", Kind::Connection),
        ] {
            for r in self.table(t).await {
                let id = r["id"].as_str().unwrap().to_string();
                let pid = r["project_id"].as_str().unwrap_or_default();
                let Some(p) = projects.iter().find(|p| p["id"] == pid) else {
                    continue;
                };
                let Some(repo) = p["git_repo_path"].as_str() else {
                    continue;
                };
                let (path, base, file_id) = if kind == Kind::Connection {
                    (
                        r["shared_connection_id"]
                            .as_str()
                            .and_then(template_path)
                            .map(String::from),
                        r["shared_base"].as_str().map(String::from),
                        r["shared_file_id"].as_str().map(String::from),
                    )
                } else {
                    (
                        r["shared_path"].as_str().map(String::from),
                        r["shared_base"].as_str().map(String::from),
                        r["shared_file_id"].as_str().map(String::from),
                    )
                };
                let shared = if kind == Kind::Connection {
                    path.is_some()
                } else {
                    r["shared"] == 1
                };
                let seen = !p["shared_dir"].is_null();
                if all_ok && seen && shared && path.is_none() && !pathless.contains(&id) {
                    out.push(format!("(4b) {t} {id} is shared without a path"));
                }
                let Some(path) = path else { continue };
                let root = Path::new(repo);
                if git2::Repository::open(root)
                    .ok()
                    .and_then(|r| r.index().ok())
                    .is_some_and(|i| i.has_conflicts())
                {
                    continue;
                }
                if !plain_file(root, &path) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
                    continue;
                };
                let Some((hash, fid)) = file_hash(&path, &text) else {
                    continue;
                };
                if base.as_deref() != Some(hash.as_str())
                    && !self
                        .withheld_name(kind, &id, &path, &text, base.as_deref())
                        .await
                {
                    out.push(format!("(4a) {t} {id}: the base isn't its file's hash"));
                }
                if file_id != fid {
                    out.push(format!("(4a) {t} {id}: the file id isn't its file's"));
                }
            }
        }
        out
    }

    /// Review (replay gap): after a sync, link or import, every row with a
    /// stored path in a linked, non-conflicted project matches its base:
    /// the row hashes to it, and its file (when it's there and readable)
    /// does too, unless a name was withheld. A sync that left a local change
    /// unwritten fails the first; one that wrote the wrong file the second.
    async fn synced_rows_match_their_bases(&self) -> Vec<String> {
        let mut out = Vec::new();
        let projects = self.table("projects").await;
        let st = self.w.ws.storage();
        let none = Link::default();
        for (t, kind) in [
            ("saved_queries", Kind::SavedQuery),
            ("dashboards", Kind::Dashboard),
            ("connections", Kind::Connection),
        ] {
            for r in self.table(t).await {
                let id = r["id"].as_str().unwrap().to_string();
                let pid = r["project_id"].as_str().unwrap_or_default();
                let Some(p) = projects.iter().find(|p| p["id"] == pid) else {
                    continue;
                };
                let Some(repo) = p["git_repo_path"].as_str() else {
                    continue;
                };
                let path = if kind == Kind::Connection {
                    r["shared_connection_id"]
                        .as_str()
                        .and_then(template_path)
                        .map(String::from)
                } else {
                    r["shared_path"].as_str().map(String::from)
                };
                let Some(path) = path else { continue };
                let root = Path::new(repo);
                if git2::Repository::open(root)
                    .ok()
                    .and_then(|r| r.index().ok())
                    .is_some_and(|i| i.has_conflicts())
                    || !plain_file(root, &path)
                {
                    continue;
                }
                let base = r["shared_base"].as_str();
                let row_hash_now = match kind {
                    Kind::SavedQuery => saved_queries::get(st, &id).await.unwrap().and_then(|x| {
                        row_hash(&RowChange::Query {
                            row: Some(&x),
                            link: &none,
                            renamed: false,
                        })
                    }),
                    Kind::Dashboard => dashboards::get(st, &id).await.unwrap().and_then(|x| {
                        row_hash(&RowChange::Dashboard {
                            row: Some(&x),
                            link: &none,
                            renamed: false,
                        })
                    }),
                    Kind::Connection => connections::get(st, &id).await.unwrap().and_then(|x| {
                        row_hash(&RowChange::Connection {
                            row: Some(&x),
                            link: &none,
                            renamed: false,
                            shared_now: false,
                        })
                    }),
                };
                let failing = self
                    .w
                    .hook
                    .failing
                    .lock()
                    .unwrap()
                    .contains(&root.join(&path));
                if row_hash_now.as_deref() != base && !failing {
                    out.push(format!(
                        "{t} {id}: the row doesn't hash to its base after a sync"
                    ));
                }
                if let Ok(text) = std::fs::read_to_string(root.join(&path)) {
                    let file = file_hash(&path, &text).map(|(h, _)| h);
                    if file.as_deref() != base
                        && !self.withheld_name(kind, &id, &path, &text, base).await
                        && !failing
                    {
                        out.push(format!(
                            "{t} {id}: the file doesn't hash to its base after a sync"
                        ));
                    }
                }
            }
        }
        out
    }

    /// The replay's epilogue (review, replay gap): no recorded step leaves a
    /// local change for a sync to write, so after a case's last step each
    /// linked shared query with a readable file gets one, straight to the
    /// row (R ≠ B, F = B, as an older release's edit). A sync of its
    /// project must write it and record the new base. `None` when the case
    /// ends with nothing to try.
    async fn epilogue(&mut self) -> Option<Vec<String>> {
        let projects = self.table("projects").await;
        let mut edited: Vec<(String, String, PathBuf)> = Vec::new();
        for r in self.table("saved_queries").await {
            let pid = r["project_id"].as_str().unwrap_or_default().to_string();
            let Some(p) = projects.iter().find(|p| p["id"] == pid.as_str()) else {
                continue;
            };
            let (Some(repo), Some(path)) = (p["git_repo_path"].as_str(), r["shared_path"].as_str())
            else {
                continue;
            };
            let root = Path::new(repo);
            if r["shared"] != 1
                || r["shared_base"].is_null()
                || git2::Repository::open(root)
                    .ok()
                    .and_then(|x| x.index().ok())
                    .is_some_and(|i| i.has_conflicts())
                || !plain_file(root, path)
                || self
                    .w
                    .hook
                    .failing
                    .lock()
                    .unwrap()
                    .contains(&root.join(path))
            {
                continue;
            }
            let id = r["id"].as_str().unwrap().to_string();
            sqlx::query("UPDATE saved_queries SET query = query || ' -- epilogue' WHERE id = ?")
                .bind(&id)
                .execute(self.w.ws.storage().pool())
                .await
                .unwrap();
            edited.push((id, pid, root.join(path)));
        }
        if edited.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        let pids: std::collections::BTreeSet<String> =
            edited.iter().map(|(_, p, _)| p.clone()).collect();
        for pid in pids {
            if let Err(e) = self
                .w
                .ws
                .shared_sync(&self.w.core, &self.origin, SyncTarget::Project(pid))
                .await
            {
                out.push(format!("epilogue sync: {}: {}", e.code, e.message));
            }
        }
        for (id, _, file) in &edited {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            if !text.contains("-- epilogue") {
                out.push(format!("epilogue: {id}'s local change wasn't written"));
            }
        }
        out.extend(self.synced_rows_match_their_bases().await);
        Some(out)
    }

    /// `*` (4a)'s exception: the sync withheld the file's name.
    async fn withheld_name(
        &self,
        kind: Kind,
        id: &str,
        path: &str,
        text: &str,
        base: Option<&str>,
    ) -> bool {
        let none = Link::default();
        let st = self.w.ws.storage();
        let (now, renamed) = match kind {
            Kind::SavedQuery => {
                let Some(row) = saved_queries::get(st, id).await.unwrap() else {
                    return false;
                };
                let dir = &path[..path.find("/queries/").map_or(0, |i| i + 8)];
                let mut other = row.clone();
                other.name = parse_query(text, path, dir).name;
                let ch = |r| RowChange::Query {
                    row: Some(r),
                    link: &none,
                    renamed: false,
                };
                (row_hash(&ch(&row)), row_hash(&ch(&other)))
            }
            Kind::Dashboard => {
                let Some(row) = dashboards::get(st, id).await.unwrap() else {
                    return false;
                };
                let Some(file) = parse_dashboard(text, path) else {
                    return false;
                };
                let mut other = row.clone();
                other.name = file.name;
                let ch = |r| RowChange::Dashboard {
                    row: Some(r),
                    link: &none,
                    renamed: false,
                };
                (row_hash(&ch(&row)), row_hash(&ch(&other)))
            }
            Kind::Connection => {
                let Some(row) = connections::get(st, id).await.unwrap() else {
                    return false;
                };
                let Some(file) = parse_template(text) else {
                    return false;
                };
                let mut other = row.clone();
                other.name = file.name;
                let ch = |r| RowChange::Connection {
                    row: Some(r),
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

/// A regular file at `rel` with no symlink or unreadable folder on the way.
fn plain_file(root: &Path, rel: &str) -> bool {
    let mut cur = root.to_path_buf();
    for part in rel.split('/') {
        cur.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return false,
            Ok(m) if m.is_dir() => {
                if std::fs::read_dir(&cur).is_err() {
                    return false;
                }
            }
            Ok(_) => {}
            Err(_) => return false,
        }
    }
    true
}

/// Removes the id Core wrote (Q22) from a file it wrote, checking it is a
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
    } else if lower.ends_with("project.yaml") || lower.ends_with("project.yml") {
        Ok(text.to_string())
    } else {
        take("id: ", "\n", text)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replays_every_projection_case() {
    let (cases, changes) = fixtures();
    assert_eq!(cases.len(), 52);
    let cols = recorded_columns(&cases);
    let raw = raw_cases();
    let only = std::env::var("SHARED_CASE").ok();
    let mut failures = Vec::new();
    let mut steps = 0;
    let mut epilogues = 0;
    for (case, raw_case) in cases.iter().zip(&raw) {
        let name = case["name"].as_str().unwrap();
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let mut r = Replay::new(case).await;
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
            let mut errs: Vec<String> = Vec::new();
            if step["op"] == "disk.allowWrites" {
                r.w.hook.failing.lock().unwrap().clear();
            }
            let calls = step["core"].as_array().cloned().unwrap_or_default();
            let raw_calls = raw_case.steps[i].core.as_deref().unwrap_or_default();
            let mut results = Vec::new();
            for (c, raw_call) in calls.iter().zip(raw_calls) {
                results.push(r.call(c, raw_call.params.get(), &mut bindings).await);
            }
            errs.append(&mut r.errors);
            let refusal_expected = want.pointer("/outcome/ok") == Some(&json!(false));
            for e in results.iter().filter_map(|x| x.error.clone()) {
                if !refusal_expected {
                    errs.push(e);
                }
            }

            // Rows.
            let rows_want: Map<String, Value> = want
                .get("rows")
                .or(step.get("rows"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap();
            let got = r.dump_rows(&cols).await;
            errs.append(&mut r.errors);
            let mut tables: Vec<String> = rows_want
                .keys()
                .filter(|t| *t != "app_state")
                .cloned()
                .collect();
            for t in got.keys() {
                if !tables.contains(t) {
                    tables.push(t.clone());
                }
            }
            for t in tables {
                let exp: Vec<Value> = rows_want
                    .get(&t)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| {
                        if t == "shared_repos" {
                            let data: Value =
                                serde_json::from_str(row["data"].as_str().unwrap_or("null"))
                                    .unwrap_or(Value::Null);
                            json!({"id": row["id"], "path": data["path"], "name": data["name"]})
                        } else {
                            row
                        }
                    })
                    .collect();
                let act = got.get(&t).cloned().unwrap_or_default();
                if !bindings.set_matches(&exp, &act) {
                    errs.push(format!(
                        "rows of {t}\n    want {}\n    got  {}",
                        Value::Array(exp),
                        Value::Array(act)
                    ));
                }
            }

            // Files.
            let tree_want = want.get("tree").or(step.get("tree")).cloned().unwrap();
            let tree_got = r.dump_tree();
            if std::env::var("SHARED_DEBUG").is_ok() {
                eprintln!(
                    "{name} #{i}: calls {} results {}\n  want {tree_want}\n  got  {tree_got}",
                    calls.len(),
                    results.len()
                );
            }
            if !bindings.value_matches(&tree_want, &tree_got) {
                errs.push(format!("tree\n    want {tree_want}\n    got  {tree_got}"));
            }

            // Links.
            if let Some(links) = want.get("links").and_then(Value::as_object) {
                for (t, by_id) in links {
                    let rows = r.table(t).await;
                    for (id, cols) in by_id.as_object().unwrap() {
                        let id = bindings.apply(id);
                        let row = rows.iter().find(|x| x["id"] == id.as_str());
                        for (col, v) in cols.as_object().unwrap() {
                            let got = row.map_or(Value::Null, |x| x[col].clone());
                            let got: Value =
                                serde_json::from_str(&r.map_out(&got.to_string())).unwrap();
                            if !bindings.value_matches(v, &got) {
                                errs.push(format!("link {t}.{col} of {id}: want {v}, got {got}"));
                            }
                        }
                    }
                }
            }

            // Notices.
            if calls.iter().any(|c| c["group"] == "shared") {
                let exp = want
                    .get("notices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let act: Vec<Value> = results.iter().flat_map(|x| x.notices.clone()).collect();
                if !bindings.set_matches(&exp, &act) {
                    errs.push(format!(
                        "notices\n    want {}\n    got  {}",
                        Value::Array(exp),
                        Value::Array(act)
                    ));
                }
            }

            // Projection and outcome.
            if let Some(p) = want.get("projection") {
                let got = results.iter().rev().find_map(|x| x.projection.clone());
                let got = got.map_or(
                    Value::Null,
                    |(status, code)| json!({"status": status, "code": code}),
                );
                if got["status"] != p["status"]
                    || (p.get("code").is_some() && got["code"] != p["code"])
                {
                    errs.push(format!("projection: want {p}, got {got}"));
                }
            }
            if let Some(o) = want.get("outcome") {
                let last = results.last();
                if o["ok"].as_bool() != Some(last.is_some_and(|x| x.ok)) {
                    errs.push(format!("outcome: want {o}"));
                }
                if let Some(c) = o.pointer("/value/conflicted") {
                    if c.as_bool() != Some(last.is_some_and(|x| x.conflicted)) {
                        errs.push(format!("outcome: want {o}"));
                    }
                }
            } else if results.iter().any(|x| x.conflicted) {
                errs.push("a conflicted answer nobody expected".into());
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
            let all_ok = results.iter().all(|x| x.ok);
            errs.extend(r.invariants(all_ok, &pathless).await);
            if calls.iter().any(|c| {
                c["group"] == "shared"
                    && matches!(
                        c["method"].as_str(),
                        Some("sync" | "syncRepo" | "linkProject" | "importProjects")
                    )
            }) {
                errs.extend(r.synced_rows_match_their_bases().await);
            }

            // The keychain: the case's secrets minus removed connections'.
            let conns: HashSet<String> = r
                .table("connections")
                .await
                .iter()
                .map(|c| c["id"].as_str().unwrap().to_string())
                .collect();
            let want_secrets: BTreeMap<String, String> = r
                .secrets
                .iter()
                .filter(|(k, _)| k.split_once(':').is_none_or(|(_, id)| conns.contains(id)))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if r.w.store.entries() != want_secrets {
                errs.push(format!(
                    "keychain: want {:?}, got {:?}",
                    want_secrets.keys().collect::<Vec<_>>(),
                    r.w.store.entries().keys().collect::<Vec<_>>()
                ));
            }

            for e in errs {
                failures.push(format!("{name} #{i} ({}): {e}", step["op"]));
            }
        }
        if let Some(errs) = r.epilogue().await {
            epilogues += 1;
            for e in errs {
                failures.push(format!("{name} (epilogue): {e}"));
            }
        }
        // Restore the locked directories so the temp dir can go.
        for d in case["seed"]["unreadable"].as_array().into_iter().flatten() {
            let abs = r.w.abs(d.as_str().unwrap());
            let _ = std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o755));
        }
    }
    if only.is_none() {
        assert_eq!(steps, 225);
        assert!(
            epilogues >= 20,
            "only {epilogues} cases had a query to edit"
        );
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
