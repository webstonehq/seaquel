//! The blocking operations behind [`crate::Git`], ported from `src-tauri`'s
//! `git.rs` with its messages and codes. The changes, each in the docs of the
//! function it touches: `commit_changes` finishes a merge and refuses to
//! commit conflicts, `push_repo` tells a non-fast-forward from other
//! refusals and reports the server's, and `pull_repo`'s fast-forward checks
//! out safely, refusing to overwrite local changes (phase 5e).
//! `repo_status` also lists conflicted files, and the credential chain never
//! hands out a rejected credential twice.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use git2::build::{CheckoutBuilder, RepoBuilder};
use git2::{
    BranchType, ErrorCode, FetchOptions, IndexAddOption, PushOptions, Repository, RepositoryState,
    Signature, StatusOptions,
};
use log::{debug, error, info, warn};

use crate::credentials::callbacks;
use crate::{GitConflictContent, GitCredentials, GitError, GitRepoStatus, GitSyncResult};

/// Maps a libgit2 error to `code` with `"<context>: <error>"`.
fn err(code: &'static str, context: &'static str) -> impl FnOnce(git2::Error) -> GitError {
    move |e| GitError::new(code, format!("{context}: {e}"))
}

fn open(path: &Path) -> Result<Repository, GitError> {
    Repository::open(path).map_err(err("REPO_OPEN_ERROR", "Failed to open repository"))
}

fn sync_result(success: bool, message: &str) -> GitSyncResult {
    GitSyncResult {
        success,
        message: message.to_string(),
        conflicts: vec![],
        files_changed: vec![],
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub(crate) fn clone_repo(
    url: &str,
    path: &Path,
    credentials: Option<GitCredentials>,
    home: Option<PathBuf>,
) -> Result<(), GitError> {
    info!(activity = "git.clone"; "Cloning repository");
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks(credentials, home));

    RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(url, path)
        .map_err(|e| {
            error!(activity = "git.clone", error_code = "CLONE_ERROR"; "Clone failed");
            err("CLONE_ERROR", "Failed to clone repository")(e)
        })?;

    info!(activity = "git.clone"; "Clone complete");
    Ok(())
}

pub(crate) fn init_repo(path: &Path) -> Result<(), GitError> {
    info!(activity = "git.init"; "Initializing repository");
    Repository::init(path).map_err(|e| {
        error!(activity = "git.init", error_code = "INIT_ERROR"; "Init failed");
        err("INIT_ERROR", "Failed to initialize repository")(e)
    })?;
    info!(activity = "git.init"; "Repository initialized");
    Ok(())
}

/// The current branch's short name, or `None` on an unborn branch.
fn branch_name(repo: &Repository) -> Result<Option<String>, GitError> {
    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == ErrorCode::UnbornBranch => return Ok(None),
        Err(e) => return Err(err("REPO_ERROR", "Failed to get HEAD")(e)),
    };
    head.shorthand()
        .map(|s| Some(s.to_string()))
        .ok_or_else(|| GitError::new("REPO_ERROR", "Failed to get branch name"))
}

pub(crate) fn pull_repo(
    path: &Path,
    credentials: Option<GitCredentials>,
    home: Option<PathBuf>,
) -> Result<GitSyncResult, GitError> {
    debug!(activity = "git.pull"; "Pulling changes");
    let repo = open(path)?;

    let Some(branch_name) = branch_name(&repo)? else {
        warn!(activity = "git.pull"; "Pull on unborn branch");
        return Ok(sync_result(
            true,
            "Repository has no commits yet. Create a commit first.",
        ));
    };

    let mut remote = repo
        .find_remote("origin")
        .map_err(err("REMOTE_ERROR", "Failed to find remote 'origin'"))?;

    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks(credentials, home));
    remote
        .fetch(&[&branch_name], Some(&mut fetch_opts), None)
        .map_err(err("PULL_ERROR", "Failed to fetch"))?;

    // FETCH_HEAD first, then the remote-tracking branch.
    let fetch_commit = match repo.find_reference("FETCH_HEAD") {
        Ok(fetch_head) => repo
            .reference_to_annotated_commit(&fetch_head)
            .map_err(err("PULL_ERROR", "Failed to get annotated commit"))?,
        Err(_) => {
            let remote_ref = format!("refs/remotes/origin/{branch_name}");
            match repo.find_reference(&remote_ref) {
                Ok(remote_branch) => {
                    repo.reference_to_annotated_commit(&remote_branch)
                        .map_err(err(
                            "PULL_ERROR",
                            "Failed to get annotated commit from remote branch",
                        ))?
                }
                Err(_) => return Ok(sync_result(true, "No remote changes to pull")),
            }
        }
    };

    let (analysis, _) = repo
        .merge_analysis(&[&fetch_commit])
        .map_err(err("MERGE_ERROR", "Failed to analyze merge"))?;

    if analysis.is_up_to_date() {
        info!(activity = "git.pull", result = "up-to-date"; "Already up to date");
        return Ok(sync_result(true, "Already up to date"));
    }

    if analysis.is_fast_forward() {
        // The working tree first, safely, while HEAD still names the commit
        // it was checked out from: libgit2 then refuses before writing
        // anything when a file the pull changes has local changes (phase 5e
        // bug 2; this used to `force()` over them). Only then does the
        // branch move.
        let target = repo
            .find_commit(fetch_commit.id())
            .map_err(err("PULL_ERROR", "Failed to find commit"))?;
        checkout_fast_forward(&repo, target.as_object())?;
        // A narrow window: if the checkout succeeds and `set_target` or
        // `set_head` below then fails (a locked ref, a full disk), the
        // working tree and index hold the fetched commit while the branch
        // still names the old one, so `git status` shows the pull's changes
        // as local edits. Nothing local is lost (the safe checkout refused
        // to overwrite any), and pulling again moves the branch. Left as is
        // in phase 5e Task 1.
        let refname = format!("refs/heads/{branch_name}");
        let mut reference = repo
            .find_reference(&refname)
            .map_err(err("PULL_ERROR", "Failed to find reference"))?;
        reference
            .set_target(fetch_commit.id(), "Fast-forward pull")
            .map_err(err("PULL_ERROR", "Failed to update reference"))?;
        repo.set_head(&refname)
            .map_err(err("PULL_ERROR", "Failed to set HEAD"))?;

        info!(activity = "git.pull", result = "fast-forward"; "Fast-forward merge");
        return Ok(sync_result(true, "Fast-forward merge successful"));
    }

    if analysis.is_normal() {
        let fetch_commit_obj = repo
            .find_commit(fetch_commit.id())
            .map_err(err("MERGE_ERROR", "Failed to find commit"))?;

        if let Err(e) = repo.merge(&[&fetch_commit], None, None) {
            // Probe fix 6: local changes the merge would overwrite refuse
            // it as they refuse a fast-forward, naming the paths.
            if e.code() == ErrorCode::Conflict || e.class() == git2::ErrorClass::Merge {
                let paths = dirty_paths_the_merge_changes(&repo, &fetch_commit_obj);
                if !paths.is_empty() {
                    warn!(
                        activity = "git.pull",
                        error_code = "PULL_ERROR",
                        count = paths.len();
                        "Merge refused: local changes would be overwritten"
                    );
                    return Err(GitError::new("PULL_ERROR", refused_message(&paths)));
                }
            }
            return Err(err("MERGE_ERROR", "Failed to merge")(e));
        }

        let mut index = repo
            .index()
            .map_err(err("INDEX_ERROR", "Failed to get index"))?;

        if index.has_conflicts() {
            let conflicts: Vec<String> = index
                .conflicts()
                .map_err(err("CONFLICT_ERROR", "Failed to get conflicts"))?
                .filter_map(|c| c.ok())
                .filter_map(|c| {
                    // `their` too: a file we deleted has no `our` entry.
                    c.our
                        .or(c.their)
                        .map(|entry| String::from_utf8_lossy(&entry.path).to_string())
                })
                .collect();

            info!(activity = "git.pull", result = "conflicts"; "Merge conflicts detected");
            return Ok(GitSyncResult {
                conflicts,
                ..sync_result(false, "Merge conflicts detected")
            });
        }

        let sig = signature(&repo)?;
        let head_commit = repo
            .head()
            .map_err(err("REPO_ERROR", "Failed to get HEAD"))?
            .peel_to_commit()
            .map_err(err("REPO_ERROR", "Failed to peel to commit"))?;
        let tree_id = index
            .write_tree()
            .map_err(err("COMMIT_ERROR", "Failed to write tree"))?;
        let tree = repo
            .find_tree(tree_id)
            .map_err(err("COMMIT_ERROR", "Failed to find tree"))?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge remote-tracking branch",
            &tree,
            &[&head_commit, &fetch_commit_obj],
        )
        .map_err(err("COMMIT_ERROR", "Failed to create merge commit"))?;

        repo.cleanup_state()
            .map_err(err("REPO_ERROR", "Failed to cleanup state"))?;

        info!(activity = "git.pull", result = "merge"; "Merge successful");
        return Ok(sync_result(true, "Merge successful"));
    }

    error!(activity = "git.pull", error_code = "MERGE_ERROR"; "Unable to merge");
    Err(GitError::new("MERGE_ERROR", "Unable to merge"))
}

/// The words a refused fast-forward's `PULL_ERROR` message starts with:
/// the TS sync button matches them to say what to do.
pub const PULL_REFUSED_LOCAL_CHANGES: &str = "Commit or discard your changes to ";

/// The most paths a refused fast-forward names.
const MAX_REFUSED_PATHS: usize = 10;

/// Checks `target` out over the working tree with `CheckoutBuilder::safe()`.
/// A file the checkout would change that has local changes (an edit, or an
/// untracked file in the way) refuses it with `PULL_ERROR` "Commit or
/// discard your changes to … first", naming at most ten of them, and
/// nothing is written. The paths go in the message for the GUI, never in a
/// log line.
fn checkout_fast_forward(repo: &Repository, target: &git2::Object<'_>) -> Result<(), GitError> {
    let conflicts = RefCell::new(Vec::<String>::new());
    let result = {
        let mut builder = CheckoutBuilder::new();
        builder
            .safe()
            .notify_on(git2::CheckoutNotificationType::CONFLICT)
            .notify(|_, path, _, _, _| {
                if let Some(path) = path {
                    conflicts
                        .borrow_mut()
                        .push(path.to_string_lossy().replace('\\', "/"));
                }
                true
            });
        repo.checkout_tree(target, Some(&mut builder))
    };
    let mut conflicts = conflicts.into_inner();
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ErrorCode::Conflict || !conflicts.is_empty() => {
            warn!(
                activity = "git.pull",
                error_code = "PULL_ERROR",
                count = conflicts.len();
                "Fast-forward refused: local changes would be overwritten"
            );
            conflicts.sort();
            conflicts.dedup();
            Err(GitError::new("PULL_ERROR", refused_message(&conflicts)))
        }
        Err(e) => Err(err("PULL_ERROR", "Failed to checkout")(e)),
    }
}

/// "Commit or discard your changes to "a", "b" and 2 more first": each path
/// as a JSON string, so the GUI can read them back whatever they hold and
/// put them in its own sentence.
fn refused_message(paths: &[String]) -> String {
    let mut named = paths
        .iter()
        .take(MAX_REFUSED_PATHS)
        .map(|path| serde_json::to_string(path).unwrap_or_else(|_| "\"?\"".to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    if named.is_empty() {
        named = "files the pull changes".to_string();
    }
    let more = paths.len().saturating_sub(MAX_REFUSED_PATHS);
    if more > 0 {
        named.push_str(&format!(" and {more} more"));
    }
    format!("{PULL_REFUSED_LOCAL_CHANGES}{named} first")
}

/// The working tree's changed files (modified, added, deleted, untracked)
/// that the merge with `theirs` changes: what a refused merge names. Sorted,
/// `/`-separated.
fn dirty_paths_the_merge_changes(repo: &Repository, theirs: &git2::Commit<'_>) -> Vec<String> {
    let changed_by_them: std::collections::HashSet<String> = (|| {
        let head = repo.head().ok()?.peel_to_commit().ok()?;
        let base = repo.merge_base(head.id(), theirs.id()).ok()?;
        let base_tree = repo.find_commit(base).ok()?.tree().ok()?;
        let diff = repo
            .diff_tree_to_tree(Some(&base_tree), Some(&theirs.tree().ok()?), None)
            .ok()?;
        Some(
            diff.deltas()
                .flat_map(|d| [d.old_file().path(), d.new_file().path()])
                .flatten()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .collect(),
        )
    })()
    .unwrap_or_default();
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let mut out: Vec<String> = repo
        .statuses(Some(&mut opts))
        .map(|st| {
            st.iter()
                .filter(|e| {
                    e.status() != git2::Status::CURRENT && e.status() != git2::Status::IGNORED
                })
                .filter_map(|e| e.path().map(|p| p.replace('\\', "/")))
                .filter(|p| changed_by_them.contains(p))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out.dedup();
    out
}

/// The words in a `PUSH_ERROR` message that mean "pull first": the TS marks
/// the repo "behind" when it sees them, and "error" otherwise.
pub const PUSH_REJECTED_NON_FAST_FORWARD: &str = "rejected (non-fast-forward)";

/// Push the current branch. Failures are `PUSH_ERROR`:
///
/// - **Not a fast-forward.** libgit2 checks this itself before sending
///   anything ("cannot push non-fastforwardable reference", "…contains
///   commits that are not present locally"), and a server may say so per
///   reference ("non-fast-forward", "fetch first"). Either way the message
///   says [`PUSH_REJECTED_NON_FAST_FORWARD`].
/// - **Any other refusal by the server** (a protected branch, a hook):
///   "the remote refused <ref>: <reason>". libgit2 lets the push call
///   succeed and only reports these per reference; the Tauri command called
///   them "Push successful".
pub(crate) fn push_repo(
    path: &Path,
    credentials: Option<GitCredentials>,
    home: Option<PathBuf>,
) -> Result<GitSyncResult, GitError> {
    debug!(activity = "git.push"; "Pushing changes");
    let repo = open(path)?;

    let Some(branch_name) = branch_name(&repo)? else {
        warn!(activity = "git.push"; "Push on unborn branch");
        return Ok(sync_result(
            false,
            "Repository has no commits yet. Create a commit first before pushing.",
        ));
    };

    let mut remote = repo
        .find_remote("origin")
        .map_err(err("REMOTE_ERROR", "Failed to find remote 'origin'"))?;

    let refused: RefCell<Option<String>> = RefCell::new(None);
    let mut cbs = callbacks(credentials, home);
    cbs.push_update_reference(|refname, status| {
        if let Some(status) = status {
            refused.replace(Some(server_refusal(refname, status)));
        }
        Ok(())
    });
    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(cbs);

    let refspec = format!("refs/heads/{branch_name}:refs/heads/{branch_name}");
    let pushed = remote.push(&[&refspec], Some(&mut push_opts));
    drop(push_opts);
    let pushed = pushed
        .map_err(push_error)
        .and_then(|()| match refused.into_inner() {
            Some(message) => Err(GitError::new("PUSH_ERROR", message)),
            None => Ok(()),
        });
    if let Err(e) = pushed {
        error!(activity = "git.push", error_code = "PUSH_ERROR"; "Push failed");
        return Err(e);
    }

    info!(activity = "git.push"; "Push complete");
    Ok(sync_result(true, "Push successful"))
}

/// A failed `remote.push`.
fn push_error(e: git2::Error) -> GitError {
    let non_fast_forward = e.code() == ErrorCode::NotFastForward
        || e.message().contains("non-fastforwardable")
        || e.message().contains("not present locally");
    let message = if non_fast_forward {
        format!("Failed to push: {PUSH_REJECTED_NON_FAST_FORWARD}: {e}")
    } else {
        format!("Failed to push: {e}")
    };
    GitError::new("PUSH_ERROR", message)
}

/// A server's refusal of one reference, from `push_update_reference`.
pub(crate) fn server_refusal(refname: &str, status: &str) -> String {
    let lower = status.to_ascii_lowercase();
    if lower.contains("non-fast-forward") || lower.contains("fetch first") {
        format!("Failed to push: {PUSH_REJECTED_NON_FAST_FORWARD}: {refname} ({status})")
    } else {
        format!("Failed to push: the remote refused {refname}: {status}")
    }
}

pub(crate) fn repo_status(path: &Path) -> Result<GitRepoStatus, GitError> {
    debug!(activity = "git.status"; "Getting repository status");
    let repo = open(path)?;

    let (current_branch, is_unborn) = match repo.head() {
        Ok(head) => (head.shorthand().unwrap_or("HEAD").to_string(), false),
        Err(e) if e.code() == ErrorCode::UnbornBranch => {
            let branch_name = repo
                .config()
                .ok()
                .and_then(|config| config.get_string("init.defaultBranch").ok())
                .unwrap_or_else(|| "main".to_string());
            (branch_name, true)
        }
        Err(e) => return Err(err("REPO_ERROR", "Failed to get HEAD")(e)),
    };

    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.include_ignored(false);
    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(err("REPO_ERROR", "Failed to get status"))?;

    let mut modified_files = Vec::new();
    let mut untracked_files = Vec::new();
    let mut conflict_files = Vec::new();
    for entry in statuses.iter() {
        let status = entry.status();
        let path = entry.path().unwrap_or("").to_string();
        if status.is_conflicted() {
            conflict_files.push(path);
        } else if status.is_wt_new() {
            untracked_files.push(path);
        } else if status.is_wt_modified()
            || status.is_wt_deleted()
            || status.is_index_modified()
            || status.is_index_new()
            || status.is_index_deleted()
        {
            modified_files.push(path);
        }
    }

    let pending_changes = modified_files.len() + untracked_files.len();
    let (ahead, behind) = if is_unborn {
        (0, 0)
    } else {
        ahead_behind(&repo, &current_branch).unwrap_or((0, 0))
    };

    let has_conflicts = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?
        .has_conflicts();

    Ok(GitRepoStatus {
        is_clean: pending_changes == 0 && !has_conflicts,
        pending_changes: count(pending_changes),
        ahead_by: count(ahead),
        behind_by: count(behind),
        has_conflicts,
        current_branch,
        modified_files,
        untracked_files,
        conflict_files,
    })
}

/// Stage everything and commit. Refused with `CONFLICT_ERROR` while any
/// file is still conflicted: staging it would commit the conflict markers.
///
/// During a merge (a pull that stopped on conflicts, then resolved), the
/// commit's parents are HEAD and the merged commits, and the merge state is
/// cleared. The Tauri command made a one-parent commit and left the merge
/// open, so the next push was rejected as not a fast-forward and the next
/// pull conflicted again.
pub(crate) fn commit_changes(path: &Path, message: &str) -> Result<String, GitError> {
    debug!(activity = "git.commit"; "Creating commit");
    let mut repo = open(path)?;

    // The commits a merge in progress brings in (`mergehead_foreach` needs
    // the repo mutably, so before anything borrows it).
    let merging = repo.state() == RepositoryState::Merge;
    let mut merge_heads = Vec::new();
    if merging {
        repo.mergehead_foreach(|id| {
            merge_heads.push(*id);
            true
        })
        .map_err(err("COMMIT_ERROR", "Failed to read MERGE_HEAD"))?;
    }

    let mut index = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?;
    if index.has_conflicts() {
        let files: Vec<String> = index
            .conflicts()
            .map_err(err("CONFLICT_ERROR", "Failed to get conflicts"))?
            .filter_map(|c| c.ok())
            .filter_map(|c| c.our.or(c.their).or(c.ancestor))
            .map(|entry| String::from_utf8_lossy(&entry.path).to_string())
            .collect();
        return Err(GitError::new(
            "CONFLICT_ERROR",
            format!(
                "Resolve the conflicts before committing: {}",
                files.join(", ")
            ),
        ));
    }
    index
        .add_all(["."].iter(), IndexAddOption::DEFAULT, None)
        .map_err(err("STAGE_ERROR", "Failed to add files"))?;
    index
        .write()
        .map_err(err("INDEX_ERROR", "Failed to write index"))?;
    let tree_id = index
        .write_tree()
        .map_err(err("COMMIT_ERROR", "Failed to write tree"))?;
    let tree = repo
        .find_tree(tree_id)
        .map_err(err("COMMIT_ERROR", "Failed to find tree"))?;

    let sig = signature(&repo)?;

    let mut parents: Vec<git2::Commit<'_>> = repo
        .head()
        .ok()
        .and_then(|head| head.peel_to_commit().ok())
        .into_iter()
        .collect();
    for id in merge_heads {
        parents.push(
            repo.find_commit(id)
                .map_err(err("COMMIT_ERROR", "Failed to find merged commit"))?,
        );
    }
    let parents: Vec<&git2::Commit<'_>> = parents.iter().collect();

    let commit_id = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(err("COMMIT_ERROR", "Failed to create commit"))?;

    if merging {
        repo.cleanup_state()
            .map_err(err("REPO_ERROR", "Failed to cleanup state"))?;
    }

    info!(activity = "git.commit"; "Commit created");
    Ok(commit_id.to_string())
}

pub(crate) fn resolve_conflict(
    path: &Path,
    file_path: &str,
    resolution: &str,
) -> Result<(), GitError> {
    debug!(activity = "git.resolve"; "Resolving conflict");
    let repo = open(path)?;
    check_conflicted(&repo, path, file_path)?;

    crate::tree::write_resolved(path, file_path, resolution.as_bytes()).map_err(|e| {
        GitError::new(
            "CONFLICT_ERROR",
            format!("Failed to write resolved file: {e}"),
        )
    })?;

    let mut index = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?;
    index
        .add_path(Path::new(file_path))
        .map_err(err("STAGE_ERROR", "Failed to stage resolved file"))?;
    // Drop any conflict entries left for the path, then add it again.
    index.remove_path(Path::new(file_path)).ok();
    index
        .add_path(Path::new(file_path))
        .map_err(err("STAGE_ERROR", "Failed to add resolved file"))?;
    index
        .write()
        .map_err(err("INDEX_ERROR", "Failed to write index"))?;
    Ok(())
}

pub(crate) fn conflict_content(
    path: &Path,
    file_path: &str,
) -> Result<GitConflictContent, GitError> {
    let repo = open(path)?;
    let index = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?;

    let blob_text = |entry: Option<git2::IndexEntry>| -> String {
        entry
            .and_then(|e| repo.find_blob(e.id).ok())
            .map(|blob| String::from_utf8_lossy(blob.content()).to_string())
            .unwrap_or_default()
    };

    for conflict in index
        .conflicts()
        .map_err(err("CONFLICT_ERROR", "Failed to get conflicts"))?
    {
        let conflict = conflict.map_err(err("CONFLICT_ERROR", "Failed to read conflict"))?;
        let conflict_path = conflict
            .our
            .as_ref()
            .or(conflict.their.as_ref())
            .map(|e| String::from_utf8_lossy(&e.path).to_string())
            .unwrap_or_default();
        if conflict_path != file_path {
            continue;
        }
        // A side with no entry while the base has one deleted the file.
        let had_base = conflict.ancestor.is_some();
        let ours_deleted = had_base && conflict.our.is_none();
        let theirs_deleted = had_base && conflict.their.is_none();
        return Ok(GitConflictContent {
            base: blob_text(conflict.ancestor),
            ours: blob_text(conflict.our),
            theirs: blob_text(conflict.their),
            ours_deleted,
            theirs_deleted,
        });
    }

    Ok(GitConflictContent {
        base: String::new(),
        ours: String::new(),
        theirs: String::new(),
        ours_deleted: false,
        theirs_deleted: false,
    })
}

/// Review A2: `file_path` must be a path the index holds as conflicted,
/// and safe to write below `root` (relative, no `..`, no backslash, no
/// symlinked component). Anything else is `CONFLICT_ERROR` and nothing is
/// touched.
fn check_conflicted(repo: &Repository, root: &Path, file_path: &str) -> Result<(), GitError> {
    let refused = || {
        GitError::new(
            "CONFLICT_ERROR",
            "That file isn't a conflicted file in this repository.",
        )
    };
    if crate::tree::check_resolvable(root, file_path).is_err() {
        return Err(refused());
    }
    let index = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?;
    let conflicts = index
        .conflicts()
        .map_err(err("CONFLICT_ERROR", "Failed to get conflicts"))?;
    for conflict in conflicts {
        let conflict = conflict.map_err(err("CONFLICT_ERROR", "Failed to read conflict"))?;
        let named = [&conflict.ancestor, &conflict.our, &conflict.their]
            .into_iter()
            .flatten()
            .any(|e| e.path.as_slice() == file_path.as_bytes());
        if named {
            return Ok(());
        }
    }
    Err(refused())
}

/// Resolves a conflicted `file_path` by deleting it (probe fix 5: the
/// side the user keeps deleted the file): the file goes from the working
/// tree, its conflict entries from the index, and the deletion is staged.
pub(crate) fn resolve_conflict_deleted(path: &Path, file_path: &str) -> Result<(), GitError> {
    debug!(activity = "git.resolve"; "Resolving a conflict by deleting the file");
    let repo = open(path)?;
    check_conflicted(&repo, path, file_path)?;
    match std::fs::remove_file(path.join(file_path)) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(GitError::new(
                "CONFLICT_ERROR",
                format!("Failed to delete the resolved file: {e}"),
            ))
        }
    }
    let mut index = repo
        .index()
        .map_err(err("INDEX_ERROR", "Failed to get index"))?;
    index
        .remove_path(Path::new(file_path))
        .map_err(err("STAGE_ERROR", "Failed to stage the deletion"))?;
    index
        .write()
        .map_err(err("INDEX_ERROR", "Failed to write index"))?;
    Ok(())
}

pub(crate) fn set_remote(path: &Path, url: &str) -> Result<(), GitError> {
    debug!(activity = "git.remote"; "Setting remote");
    let repo = open(path)?;
    repo.remote_delete("origin").ok();
    repo.remote("origin", url)
        .map_err(err("REMOTE_ERROR", "Failed to set remote"))?;
    Ok(())
}

pub(crate) fn remote_url(path: &Path) -> Result<Option<String>, GitError> {
    let repo = open(path)?;
    let url = match repo.find_remote("origin") {
        Ok(remote) => remote.url().map(str::to_string),
        Err(_) => None,
    };
    Ok(url)
}

/// The repo's configured user, or `Seaquel User <seaquel@local>`.
fn signature(repo: &Repository) -> Result<Signature<'static>, GitError> {
    if let Ok(sig) = repo.signature() {
        return Ok(sig);
    }
    Signature::now("Seaquel User", "seaquel@local")
        .map_err(err("COMMIT_ERROR", "Failed to create signature"))
}

/// Commits ahead of and behind the upstream, or `origin/<branch>` when there
/// is no upstream. With neither, every local commit is ahead (a clone of an
/// empty repo that has commits of its own). `None` without an `origin`.
fn ahead_behind(repo: &Repository, branch: &str) -> Option<(usize, usize)> {
    let local_branch = repo.find_branch(branch, BranchType::Local).ok()?;
    let local_commit = local_branch.get().peel_to_commit().ok()?;

    match local_branch.upstream() {
        Ok(upstream) => {
            let upstream_commit = upstream.get().peel_to_commit().ok()?;
            repo.graph_ahead_behind(local_commit.id(), upstream_commit.id())
                .ok()
        }
        Err(_) => {
            repo.find_remote("origin").ok()?;
            let remote_ref = format!("refs/remotes/origin/{branch}");
            match repo.find_reference(&remote_ref) {
                Ok(remote_branch) => {
                    let upstream_commit = remote_branch.peel_to_commit().ok()?;
                    repo.graph_ahead_behind(local_commit.id(), upstream_commit.id())
                        .ok()
                }
                Err(_) => {
                    let mut revwalk = repo.revwalk().ok()?;
                    revwalk.push(local_commit.id()).ok()?;
                    Some((revwalk.count(), 0))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refused fast-forward names each path as a JSON string, so the GUI
    /// can read them back even when a path holds `, `, `"` or ` first`.
    #[test]
    fn a_refused_fast_forward_quotes_each_path_as_json() {
        let paths = [
            "a, b.sql".to_string(),
            "say \"hi\" first.sql".to_string(),
            "back\\slash\nnew.sql".to_string(),
        ];
        let message = refused_message(&paths);
        let list = message
            .strip_prefix(PULL_REFUSED_LOCAL_CHANGES)
            .and_then(|rest| rest.strip_suffix(" first"))
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&format!("[{list}]")).unwrap();
        assert_eq!(parsed, paths);
    }

    #[test]
    fn past_ten_paths_the_rest_are_counted() {
        let paths: Vec<String> = (0..12).map(|i| format!("q{i:02}.sql")).collect();
        let message = refused_message(&paths);
        let list = message
            .strip_prefix(PULL_REFUSED_LOCAL_CHANGES)
            .and_then(|rest| rest.strip_suffix(" and 2 more first"))
            .unwrap();
        let parsed: Vec<String> = serde_json::from_str(&format!("[{list}]")).unwrap();
        assert_eq!(parsed, paths[..10]);
    }

    #[test]
    fn a_server_refusal_is_behind_only_when_it_says_non_fast_forward() {
        for status in [
            "non-fast-forward",
            "fetch first",
            "Updates were rejected: Non-Fast-Forward",
        ] {
            let message = server_refusal("refs/heads/main", status);
            assert!(
                message.contains(PUSH_REJECTED_NON_FAST_FORWARD),
                "{message}"
            );
        }
        let message = server_refusal("refs/heads/main", "protected branch hook declined");
        assert_eq!(
            message,
            "Failed to push: the remote refused refs/heads/main: protected branch hook declined"
        );
        assert!(!message.contains("rejected"), "{message}");
    }

    #[test]
    fn libgit2s_own_non_fast_forward_errors_say_rejected() {
        for (code, text) in [
            (git2::ErrorCode::NotFastForward, "cannot push non-fastforwardable reference"),
            (
                git2::ErrorCode::GenericError,
                "cannot push because a reference that you are trying to update on the remote contains commits that are not present locally.",
            ),
        ] {
            let e = git2::Error::new(code, git2::ErrorClass::Reference, text);
            let got = push_error(e);
            assert_eq!(got.code, "PUSH_ERROR");
            assert!(got.message.contains(PUSH_REJECTED_NON_FAST_FORWARD), "{}", got.message);
            assert!(got.message.contains(text), "{}", got.message);
        }
        let other = push_error(git2::Error::from_str("connection reset"));
        assert_eq!(other.message, "Failed to push: connection reset");
    }
}
