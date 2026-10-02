//! A temp world for the shared projection's Core tests: a canonical temp
//! root holding the repos (real `git init`, real symlinks), a data dir, a
//! Core with `LocalFiles` and the per-path write hook, and a workspace with
//! a test secret store. Nothing reads the user's git config, home or
//! keychain.
#![allow(dead_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use seaquel_core::{Core, LocalFiles, Workspace, WorkspaceSpec};
use tempfile::TempDir;

use crate::common::TestStore;

/// Points libgit2's config search paths at an empty dir, once per binary,
/// before any repository is touched (as `seaquel-git`'s tests do).
pub fn isolate_git() -> &'static Path {
    static EMPTY: OnceLock<TempDir> = OnceLock::new();
    EMPTY
        .get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            for level in [
                git2::ConfigLevel::Global,
                git2::ConfigLevel::XDG,
                git2::ConfigLevel::System,
                git2::ConfigLevel::ProgramData,
            ] {
                // SAFETY: runs once, before any other libgit2 call in this
                // process (every world calls it first; `OnceLock` makes the
                // others wait).
                unsafe { git2::opts::set_search_path(level, dir.path()).unwrap() };
            }
            dir
        })
        .path()
}

/// Runs the git CLI in `dir` with no user or system config.
pub fn git(dir: &Path, args: &[&str]) -> String {
    match git_try(dir, args) {
        Ok(out) => out,
        Err(e) => panic!("git {args:?} failed: {e}"),
    }
}

/// Whether `dir`'s repo has a commit.
pub fn has_head(dir: &Path) -> bool {
    git_try(dir, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
}

/// [`git`], answering its stderr on a failure.
pub fn git_try(dir: &Path, args: &[&str]) -> Result<String, String> {
    let home = isolate_git();
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "core.precomposeunicode=true",
        ])
        .args(args)
        .output()
        .expect("git runs");
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Commits everything in `dir`, if anything changed.
pub fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    if !git(dir, &["status", "--porcelain"]).trim().is_empty() {
        git(dir, &["commit", "-q", "-m", message]);
    }
}

/// The write hook's state: paths whose writes fail, every path Core wrote,
/// and an optional gate that holds a write until released.
#[derive(Default)]
pub struct Hook {
    pub failing: Mutex<HashSet<PathBuf>>,
    /// Paths whose write panics (the file task fails as a whole).
    pub panicking: Mutex<HashSet<PathBuf>>,
    /// Compare paths case-insensitively (a case-insensitive seed).
    pub fold_case: Mutex<bool>,
    pub written: Mutex<HashSet<PathBuf>>,
    gate: Mutex<Option<PathBuf>>,
    held: Mutex<bool>,
    cv: Condvar,
}

fn fold(p: &Path) -> String {
    seaquel_core::domain::shared::names::path_key(&p.to_string_lossy())
}

impl Hook {
    fn check(&self, path: &Path) -> Result<(), String> {
        if self.panicking.lock().unwrap().contains(path) {
            panic!("the test hook fails this file task");
        }
        let fails = {
            let failing = self.failing.lock().unwrap();
            if *self.fold_case.lock().unwrap() {
                let key = fold(path);
                failing.iter().any(|f| fold(f) == key)
            } else {
                failing.contains(path)
            }
        };
        if fails {
            return Err("EACCES (test)".to_string());
        }
        // A gated write waits for `release`.
        let gated = self.gate.lock().unwrap().as_deref() == Some(path);
        if gated {
            let mut held = self.held.lock().unwrap();
            *held = true;
            self.cv.notify_all();
            while *held {
                held = self.cv.wait(held).unwrap();
            }
        }
        self.written.lock().unwrap().insert(path.to_path_buf());
        Ok(())
    }

    /// Holds the next write to `path` until [`Hook::release`].
    pub fn gate(&self, path: &Path) {
        *self.gate.lock().unwrap() = Some(path.to_path_buf());
    }

    /// Waits until a gated write is being held.
    pub fn wait_held(&self) {
        let mut held = self.held.lock().unwrap();
        while !*held {
            held = self.cv.wait(held).unwrap();
        }
    }

    pub fn release(&self) {
        *self.gate.lock().unwrap() = None;
        *self.held.lock().unwrap() = false;
        self.cv.notify_all();
    }
}

pub struct World {
    _root: TempDir,
    /// The canonical temp root: the fixtures' `/repos/…` live under it.
    pub root: PathBuf,
    _data: TempDir,
    pub data: PathBuf,
    pub core: Core,
    pub ws: Arc<Workspace>,
    pub store: Arc<TestStore>,
    pub hook: Arc<Hook>,
}

/// A desktop-like Core with `LocalFiles` and the hook.
pub fn shared_core(hook: Arc<Hook>) -> Core {
    shared_core_with(hook, seaquel_core::LibraryLimits::default())
}

pub fn shared_core_with(hook: Arc<Hook>, limits: seaquel_core::LibraryLimits) -> Core {
    shared_core_full(hook, limits, None)
}

pub fn shared_core_full(
    hook: Arc<Hook>,
    limits: seaquel_core::LibraryLimits,
    planned: Option<seaquel_core::SyncPlanHook>,
) -> Core {
    let h = Arc::clone(&hook);
    let mut b = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(LocalFiles::Allowed)
        .library_limits(limits)
        .file_write_hook(Arc::new(move |p: &Path| h.check(p)));
    if let Some(planned) = planned {
        b = b.sync_plan_hook(planned);
    }
    b.build()
}

impl World {
    pub async fn new() -> World {
        Self::with_file(None).await
    }

    /// A world on a fresh file, or on `schema`'s SQL (an older release's
    /// file) opened through `Storage::open`.
    pub async fn with_file(schema: Option<&str>) -> World {
        Self::with(schema, seaquel_core::LibraryLimits::default()).await
    }

    /// A world whose Core has these library limits.
    pub async fn with_limits(limits: seaquel_core::LibraryLimits) -> World {
        Self::with(None, limits).await
    }

    /// A world whose Core runs `planned` after each optimistic sync plan.
    pub async fn with_plan_hook(planned: seaquel_core::SyncPlanHook) -> World {
        Self::with_all(None, seaquel_core::LibraryLimits::default(), Some(planned)).await
    }

    async fn with(schema: Option<&str>, limits: seaquel_core::LibraryLimits) -> World {
        Self::with_all(schema, limits, None).await
    }

    async fn with_all(
        schema: Option<&str>,
        limits: seaquel_core::LibraryLimits,
        planned: Option<seaquel_core::SyncPlanHook>,
    ) -> World {
        isolate_git();
        let root_dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root_dir.path()).unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let data = data_dir.path().to_path_buf();
        if let Some(sql) = schema {
            let opts = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(data.join("seaquel.db"))
                .create_if_missing(true);
            let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
            sqlx::raw_sql(sql).execute(&pool).await.unwrap();
            pool.close().await;
        }
        let hook = Arc::new(Hook::default());
        let core = shared_core_full(Arc::clone(&hook), limits, planned);
        let store = TestStore::new();
        let ws = core
            .open_workspace(WorkspaceSpec::new(&data).with_secrets(store.clone()))
            .await
            .unwrap();
        sqlx::query("DELETE FROM app_state WHERE key = 'connectionStringSecretsUpgraded'")
            .execute(ws.storage().pool())
            .await
            .unwrap();
        World {
            _root: root_dir,
            root,
            _data: data_dir,
            data,
            core,
            ws,
            store,
            hook,
        }
    }

    /// `/repos/a` → `<root>/repos/a`.
    pub fn abs(&self, fixture_path: &str) -> PathBuf {
        self.root.join(fixture_path.trim_start_matches('/'))
    }

    pub fn put(&self, fixture_path: &str, text: &str) {
        let p = self.abs(fixture_path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    pub fn read(&self, fixture_path: &str) -> Option<String> {
        std::fs::read_to_string(self.abs(fixture_path)).ok()
    }

    /// A git repo at `/repos/<name>` with a bare `origin` beside the root,
    /// its files committed and pushed.
    pub fn git_repo(&self, name: &str) -> PathBuf {
        let repo = self.abs(&format!("/repos/{name}"));
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        commit_all(&repo, "seed");
        let bare = self.data.join(format!("remotes/{name}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare"]);
        let url = format!("file://{}", bare.display());
        git(&repo, &["remote", "add", "origin", &url]);
        if has_head(&repo) {
            git(&repo, &["push", "-q", "origin", "main"]);
        }
        repo
    }

    /// A fresh clone of `/repos/<name>`'s origin, for a teammate's change.
    pub fn teammate(&self, name: &str, n: usize) -> PathBuf {
        let bare = self.data.join(format!("remotes/{name}.git"));
        let clone = self.data.join(format!("clones/{name}-{n}"));
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
        clone
    }

    pub fn git_client(&self) -> seaquel_core::git::Git {
        seaquel_core::git::Git::new(Some(isolate_git().to_path_buf()))
    }
}
