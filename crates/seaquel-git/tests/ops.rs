//! The git operations against real repositories in temp dirs: a bare
//! `origin` over `file://` and clones of it.
//!
//! Nothing here reads the user's git config or `~/.ssh`: [`setup`] points
//! libgit2's global, XDG and system config search paths at an empty temp dir
//! before any test touches a repository, and every [`Git`] gets a temp home.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use git2::{ConfigLevel, Repository};
use seaquel_git::{Git, GitError, GitRepoStatus, PUSH_REJECTED_NON_FAST_FORWARD};
use tempfile::TempDir;

/// Isolates libgit2 from the user's config, once per test binary.
fn setup() {
    static EMPTY_CONFIG: OnceLock<TempDir> = OnceLock::new();
    EMPTY_CONFIG.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        for level in [
            ConfigLevel::Global,
            ConfigLevel::XDG,
            ConfigLevel::System,
            ConfigLevel::ProgramData,
        ] {
            // SAFETY: runs once, before any other libgit2 call in this
            // process (every test calls `setup` first, and `OnceLock` makes
            // the others wait for it).
            unsafe { git2::opts::set_search_path(level, dir.path()).unwrap() };
        }
        dir
    });
}

/// A temp dir holding a bare `origin.git`, and the git client under test
/// with a temp home.
struct World {
    dir: TempDir,
    git: Git,
}

impl World {
    fn new() -> Self {
        setup();
        let dir = tempfile::tempdir().unwrap();
        Repository::init_bare(dir.path().join("origin.git")).unwrap();
        std::fs::create_dir(dir.path().join("home")).unwrap();
        let git = Git::new(Some(dir.path().join("home")));
        World { dir, git }
    }

    fn origin_url(&self) -> String {
        format!("file://{}", self.dir.path().join("origin.git").display())
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    async fn clone(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        self.git
            .clone_repo(&self.origin_url(), &path, None)
            .await
            .unwrap();
        path
    }

    async fn status(&self, repo: &Path) -> GitRepoStatus {
        self.git.repo_status(repo).await.unwrap()
    }
}

fn write(repo: &Path, file: &str, content: &str) {
    std::fs::write(repo.join(file), content).unwrap();
}

fn read(repo: &Path, file: &str) -> String {
    std::fs::read_to_string(repo.join(file)).unwrap()
}

fn head_message(repo: &Path) -> String {
    let repo = Repository::open(repo).unwrap();
    let commit = repo.head().unwrap().peel_to_commit().unwrap();
    commit.message().unwrap().to_string()
}

fn ahead_behind(status: &GitRepoStatus) -> (u32, u32) {
    (status.ahead_by, status.behind_by)
}

#[tokio::test]
async fn clone_commit_push_then_a_second_clone_sees_the_commit() {
    let w = World::new();
    let a = w.clone("a").await;

    write(&a, "q.sql", "select 1;\n");
    let status = w.status(&a).await;
    assert!(!status.is_clean);
    assert_eq!(status.untracked_files, ["q.sql"]);
    assert_eq!(status.pending_changes, 1);

    let id = w.git.commit_changes(&a, "Add q").await.unwrap();
    assert_eq!(id.len(), 40, "a full hex commit id: {id}");
    let status = w.status(&a).await;
    assert!(status.is_clean);
    // Cloned from an empty origin: no upstream, no origin branch yet, so
    // every local commit counts as ahead.
    assert_eq!(ahead_behind(&status), (1, 0));

    let pushed = w.git.push_repo(&a, None).await.unwrap();
    assert!(pushed.success, "{pushed:?}");
    assert_eq!(pushed.message, "Push successful");
    assert_eq!(ahead_behind(&w.status(&a).await), (0, 0));

    let b = w.clone("b").await;
    assert_eq!(read(&b, "q.sql"), "select 1;\n");
    assert_eq!(head_message(&b), "Add q");
    let status = w.status(&b).await;
    assert!(status.is_clean);
    assert_eq!(ahead_behind(&status), (0, 0));
}

#[tokio::test]
async fn pull_fast_forward() {
    let w = World::new();
    let a = w.clone("a").await;
    write(&a, "q.sql", "v1\n");
    w.git.commit_changes(&a, "v1").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();
    let b = w.clone("b").await;

    write(&a, "q.sql", "v2\n");
    w.git.commit_changes(&a, "v2").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();

    let pulled = w.git.pull_repo(&b, None).await.unwrap();
    assert!(pulled.success, "{pulled:?}");
    assert_eq!(pulled.message, "Fast-forward merge successful");
    assert!(pulled.conflicts.is_empty());
    assert_eq!(read(&b, "q.sql"), "v2\n");
    let status = w.status(&b).await;
    assert!(status.is_clean);
    assert_eq!(ahead_behind(&status), (0, 0));

    let again = w.git.pull_repo(&b, None).await.unwrap();
    assert!(again.success);
    assert_eq!(again.message, "Already up to date");
}

#[tokio::test]
async fn pull_merges_changes_to_different_files() {
    let w = World::new();
    let a = w.clone("a").await;
    write(&a, "one.sql", "1\n");
    w.git.commit_changes(&a, "one").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();
    let b = w.clone("b").await;

    write(&a, "two.sql", "2\n");
    w.git.commit_changes(&a, "two").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();
    write(&b, "three.sql", "3\n");
    w.git.commit_changes(&b, "three").await.unwrap();

    let pulled = w.git.pull_repo(&b, None).await.unwrap();
    assert!(pulled.success, "{pulled:?}");
    assert_eq!(pulled.message, "Merge successful");
    assert_eq!(read(&b, "two.sql"), "2\n");
    assert_eq!(head_message(&b), "Merge remote-tracking branch");
    let status = w.status(&b).await;
    assert!(status.is_clean, "{status:?}");
    // The local commit and the merge.
    assert_eq!(ahead_behind(&status), (2, 0));
    assert!(w.git.push_repo(&b, None).await.unwrap().success);
}

#[tokio::test]
async fn pull_conflict_then_resolve_commit_and_push() {
    let w = World::new();
    let a = w.clone("a").await;
    write(&a, "q.sql", "base\n");
    w.git.commit_changes(&a, "base").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();
    let b = w.clone("b").await;

    write(&a, "q.sql", "from a\n");
    w.git.commit_changes(&a, "a").await.unwrap();
    w.git.push_repo(&a, None).await.unwrap();

    write(&b, "q.sql", "from b\n");
    w.git.commit_changes(&b, "b").await.unwrap();
    assert_eq!(ahead_behind(&w.status(&b).await), (1, 0));

    // Pushing now is rejected: origin has a commit b doesn't.
    // libgit2 finds that out itself before sending anything, and the
    // message says so in the words the TS maps to "behind".
    let rejected = w.git.push_repo(&b, None).await.unwrap_err();
    assert_eq!(rejected.code, "PUSH_ERROR", "{rejected:?}");
    assert!(
        rejected.message.starts_with(&format!(
            "Failed to push: {PUSH_REJECTED_NON_FAST_FORWARD}: "
        )),
        "{rejected:?}"
    );

    let pulled = w.git.pull_repo(&b, None).await.unwrap();
    assert!(!pulled.success);
    assert_eq!(pulled.message, "Merge conflicts detected");
    assert_eq!(pulled.conflicts, ["q.sql"]);

    let status = w.status(&b).await;
    assert!(status.has_conflicts);
    assert!(!status.is_clean);
    // The fetch moved origin/main: one commit each way.
    assert_eq!(ahead_behind(&status), (1, 1));
    // The conflicted file is listed on its own, not as modified.
    assert_eq!(status.conflict_files, ["q.sql"]);
    assert!(status.modified_files.is_empty(), "{status:?}");

    // Committing now would commit the conflict markers: refused, and
    // nothing changes.
    let head_before = Repository::open(&b).unwrap().head().unwrap().target();
    let refused = w.git.commit_changes(&b, "too early").await.unwrap_err();
    assert_eq!(refused.code, "CONFLICT_ERROR", "{refused:?}");
    assert_eq!(
        refused.message,
        "Resolve the conflicts before committing: q.sql"
    );
    assert_eq!(
        Repository::open(&b).unwrap().head().unwrap().target(),
        head_before
    );
    assert_eq!(w.status(&b).await.conflict_files, ["q.sql"]);

    let content = w.git.conflict_content(&b, "q.sql").await.unwrap();
    assert_eq!(content.base, "base\n");
    assert_eq!(content.ours, "from b\n");
    assert_eq!(content.theirs, "from a\n");

    // A file that isn't conflicted has three empty sides.
    let none = w.git.conflict_content(&b, "other.sql").await.unwrap();
    assert_eq!(
        (none.base, none.ours, none.theirs),
        (String::new(), String::new(), String::new())
    );

    w.git
        .resolve_conflict(&b, "q.sql", "from a and b\n")
        .await
        .unwrap();
    let status = w.status(&b).await;
    assert!(!status.has_conflicts, "{status:?}");
    assert!(status.conflict_files.is_empty(), "{status:?}");
    assert_eq!(read(&b, "q.sql"), "from a and b\n");

    w.git
        .commit_changes(&b, "Resolved merge conflicts")
        .await
        .unwrap();
    let status = w.status(&b).await;
    assert!(status.is_clean, "{status:?}");
    // b's commit and the merge commit; origin's commit is now an ancestor.
    assert_eq!(ahead_behind(&status), (2, 0));
    {
        let repo = Repository::open(&b).unwrap();
        assert_eq!(repo.state(), git2::RepositoryState::Clean);
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(head.parent_count(), 2, "the resolution commit is a merge");
    }

    let pushed = w.git.push_repo(&b, None).await.unwrap();
    assert!(pushed.success, "{pushed:?}");

    // a picks the resolution up as a fast-forward.
    assert_eq!(ahead_behind(&w.status(&a).await), (0, 0));
    let pulled = w.git.pull_repo(&a, None).await.unwrap();
    assert_eq!(pulled.message, "Fast-forward merge successful");
    assert_eq!(read(&a, "q.sql"), "from a and b\n");
}

#[tokio::test]
async fn status_lists_modified_and_untracked_files() {
    let w = World::new();
    let a = w.clone("a").await;
    write(&a, "kept.sql", "1\n");
    write(&a, "gone.sql", "1\n");
    w.git.commit_changes(&a, "two files").await.unwrap();

    write(&a, "kept.sql", "2\n");
    std::fs::remove_file(a.join("gone.sql")).unwrap();
    write(&a, "new.sql", "3\n");

    let mut status = w.status(&a).await;
    status.modified_files.sort();
    assert_eq!(status.modified_files, ["gone.sql", "kept.sql"]);
    assert_eq!(status.untracked_files, ["new.sql"]);
    assert_eq!(status.pending_changes, 3);
    assert!(!status.is_clean);
    assert!(!status.has_conflicts);
}

#[tokio::test]
async fn status_reports_the_branch_name() {
    let w = World::new();
    let a = w.clone("a").await;
    // No init.defaultBranch anywhere: an unborn branch reads as "main".
    assert_eq!(w.status(&a).await.current_branch, "main");

    write(&a, "q.sql", "1\n");
    w.git.commit_changes(&a, "first").await.unwrap();
    let head = Repository::open(&a)
        .unwrap()
        .head()
        .unwrap()
        .shorthand()
        .unwrap()
        .to_string();
    assert_eq!(w.status(&a).await.current_branch, head);
}

#[tokio::test]
async fn set_remote_and_get_remote_url() {
    let w = World::new();
    let repo = w.path("local");
    w.git.init_repo(&repo).await.unwrap();
    assert_eq!(w.git.remote_url(&repo).await.unwrap(), None);

    w.git
        .set_remote(&repo, "https://example.com/team/queries.git")
        .await
        .unwrap();
    assert_eq!(
        w.git.remote_url(&repo).await.unwrap().as_deref(),
        Some("https://example.com/team/queries.git")
    );

    // Setting it again replaces origin.
    w.git.set_remote(&repo, &w.origin_url()).await.unwrap();
    assert_eq!(w.git.remote_url(&repo).await.unwrap(), Some(w.origin_url()));

    // And the new origin works: commit and push, with no upstream set.
    write(&repo, "q.sql", "1\n");
    w.git.commit_changes(&repo, "first").await.unwrap();
    assert_eq!(ahead_behind(&w.status(&repo).await), (1, 0));
    assert!(w.git.push_repo(&repo, None).await.unwrap().success);
    // No upstream, but origin's branch now exists.
    assert_eq!(ahead_behind(&w.status(&repo).await), (0, 0));
    let b = w.clone("b").await;
    assert_eq!(read(&b, "q.sql"), "1\n");
}

#[tokio::test]
async fn init_then_pull_and_push_on_an_unborn_branch() {
    let w = World::new();
    let repo = w.path("fresh");
    w.git.init_repo(&repo).await.unwrap();
    assert!(repo.join(".git").is_dir());

    let status = w.status(&repo).await;
    assert!(status.is_clean);
    assert_eq!(ahead_behind(&status), (0, 0));
    assert_eq!(status.current_branch, "main");

    let pulled = w.git.pull_repo(&repo, None).await.unwrap();
    assert!(pulled.success);
    assert_eq!(
        pulled.message,
        "Repository has no commits yet. Create a commit first."
    );

    let pushed = w.git.push_repo(&repo, None).await.unwrap();
    assert!(!pushed.success);
    assert_eq!(
        pushed.message,
        "Repository has no commits yet. Create a commit first before pushing."
    );
}

#[tokio::test]
async fn signature_falls_back_when_no_git_config_has_a_user() {
    let w = World::new();
    let repo = w.path("r");
    w.git.init_repo(&repo).await.unwrap();
    write(&repo, "q.sql", "1\n");
    w.git.commit_changes(&repo, "first").await.unwrap();
    {
        let r = Repository::open(&repo).unwrap();
        let commit = r.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.author().name(), Some("Seaquel User"));
        assert_eq!(commit.author().email(), Some("seaquel@local"));
        assert_eq!(commit.committer().name(), Some("Seaquel User"));
    }

    // The repo's own config wins when it has a user.
    {
        let r = Repository::open(&repo).unwrap();
        let mut config = r.config().unwrap();
        config.set_str("user.name", "Ada").unwrap();
        config.set_str("user.email", "ada@example.com").unwrap();
    }
    write(&repo, "q.sql", "2\n");
    w.git.commit_changes(&repo, "second").await.unwrap();
    let r = Repository::open(&repo).unwrap();
    let commit = r.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(commit.author().name(), Some("Ada"));
    assert_eq!(commit.author().email(), Some("ada@example.com"));
}

#[tokio::test]
async fn error_codes() {
    fn code(r: Result<impl std::fmt::Debug, GitError>) -> String {
        r.unwrap_err().code
    }
    let w = World::new();
    let nowhere = w.path("not-a-repo");
    std::fs::create_dir(&nowhere).unwrap();

    assert_eq!(code(w.git.repo_status(&nowhere).await), "REPO_OPEN_ERROR");
    assert_eq!(
        code(w.git.pull_repo(&nowhere, None).await),
        "REPO_OPEN_ERROR"
    );
    assert_eq!(
        code(w.git.commit_changes(&nowhere, "m").await),
        "REPO_OPEN_ERROR"
    );
    assert_eq!(code(w.git.remote_url(&nowhere).await), "REPO_OPEN_ERROR");

    let missing = format!("file://{}", w.path("missing.git").display());
    assert_eq!(
        code(w.git.clone_repo(&missing, &w.path("c"), None).await),
        "CLONE_ERROR"
    );

    // A repo with a commit but no origin.
    let repo = w.path("no-origin");
    w.git.init_repo(&repo).await.unwrap();
    write(&repo, "q.sql", "1\n");
    w.git.commit_changes(&repo, "first").await.unwrap();
    assert_eq!(code(w.git.pull_repo(&repo, None).await), "REMOTE_ERROR");
    assert_eq!(code(w.git.push_repo(&repo, None).await), "REMOTE_ERROR");

    let err = w.git.repo_status(&nowhere).await.unwrap_err();
    assert!(
        err.message.starts_with("Failed to open repository: "),
        "{err:?}"
    );
    assert_eq!(err.to_string(), format!("REPO_OPEN_ERROR: {}", err.message));
}
