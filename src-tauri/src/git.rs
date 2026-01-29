use git2::{
    build::RepoBuilder, Cred, CredentialType, FetchOptions, PushOptions,
    RemoteCallbacks, Repository, Signature, StatusOptions,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct GitError {
    pub message: String,
    pub code: String,
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for GitError {}

#[derive(Debug, Serialize, Deserialize)]
pub struct RepoStatus {
    pub is_clean: bool,
    pub pending_changes: usize,
    pub ahead_by: usize,
    pub behind_by: usize,
    pub has_conflicts: bool,
    pub current_branch: String,
    pub modified_files: Vec<String>,
    pub untracked_files: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncResult {
    pub success: bool,
    pub message: String,
    pub conflicts: Vec<String>,
    pub files_changed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitCredentials {
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_key_path: Option<String>,
    pub ssh_passphrase: Option<String>,
}

fn create_callbacks(credentials: Option<GitCredentials>) -> RemoteCallbacks<'static> {
    let mut callbacks = RemoteCallbacks::new();
    let creds = credentials.clone();

    callbacks.credentials(move |_url, username_from_url, allowed_types| {
        if allowed_types.contains(CredentialType::SSH_KEY) {
            // Try SSH agent first
            if let Some(username) = username_from_url {
                if let Ok(cred) = Cred::ssh_key_from_agent(username) {
                    return Ok(cred);
                }
            }

            // Try SSH key from path
            if let Some(ref creds) = creds {
                if let Some(ref key_path) = creds.ssh_key_path {
                    let username = username_from_url.unwrap_or("git");
                    let passphrase = creds.ssh_passphrase.as_deref();
                    return Cred::ssh_key(username, None, Path::new(key_path), passphrase);
                }
            }

            // Try default SSH key locations
            if let Some(home) = dirs::home_dir() {
                let username = username_from_url.unwrap_or("git");
                let id_rsa = home.join(".ssh/id_rsa");
                let id_ed25519 = home.join(".ssh/id_ed25519");

                if id_ed25519.exists() {
                    if let Ok(cred) = Cred::ssh_key(username, None, &id_ed25519, None) {
                        return Ok(cred);
                    }
                }
                if id_rsa.exists() {
                    if let Ok(cred) = Cred::ssh_key(username, None, &id_rsa, None) {
                        return Ok(cred);
                    }
                }
            }
        }

        if allowed_types.contains(CredentialType::USER_PASS_PLAINTEXT) {
            if let Some(ref creds) = creds {
                if let (Some(username), Some(password)) = (&creds.username, &creds.password) {
                    return Cred::userpass_plaintext(username, password);
                }
            }
        }

        Cred::default()
    });

    callbacks
}

#[tauri::command]
pub fn git_clone_repo(
    url: String,
    path: String,
    credentials: Option<GitCredentials>,
) -> Result<(), GitError> {
    let callbacks = create_callbacks(credentials);

    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);

    RepoBuilder::new()
        .fetch_options(fetch_opts)
        .clone(&url, Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to clone repository: {}", e),
            code: "CLONE_ERROR".to_string(),
        })?;

    Ok(())
}

#[tauri::command]
pub fn git_init_repo(path: String) -> Result<(), GitError> {
    Repository::init(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to initialize repository: {}", e),
            code: "INIT_ERROR".to_string(),
        })?;
    Ok(())
}

#[tauri::command]
pub fn git_pull_repo(path: String, credentials: Option<GitCredentials>) -> Result<SyncResult, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Get the current branch name - handle unborn branch
    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Ok(SyncResult {
                success: true,
                message: "Repository has no commits yet. Create a commit first.".to_string(),
                conflicts: vec![],
                files_changed: vec![],
            });
        }
        Err(e) => return Err(GitError {
            message: format!("Failed to get HEAD: {}", e),
            code: "REPO_ERROR".to_string(),
        }),
    };
    let branch_name = head
        .shorthand()
        .ok_or(GitError {
            message: "Failed to get branch name".to_string(),
            code: "REPO_ERROR".to_string(),
        })?
        .to_string();

    // Fetch from remote
    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| GitError {
            message: format!("Failed to find remote 'origin': {}", e),
            code: "REMOTE_ERROR".to_string(),
        })?;

    let callbacks = create_callbacks(credentials);
    let mut fetch_opts = FetchOptions::new();
    fetch_opts.remote_callbacks(callbacks);

    remote
        .fetch(&[&branch_name], Some(&mut fetch_opts), None)
        .map_err(|e| GitError {
            message: format!("Failed to fetch: {}", e),
            code: "PULL_ERROR".to_string(),
        })?;

    // Get the fetch head - try FETCH_HEAD first, fall back to remote tracking branch
    let fetch_commit = match repo.find_reference("FETCH_HEAD") {
        Ok(fetch_head) => repo
            .reference_to_annotated_commit(&fetch_head)
            .map_err(|e| GitError {
                message: format!("Failed to get annotated commit: {}", e),
                code: "PULL_ERROR".to_string(),
            })?,
        Err(_) => {
            // FETCH_HEAD may not exist or be corrupted - try using remote tracking branch
            let remote_ref = format!("refs/remotes/origin/{}", branch_name);
            match repo.find_reference(&remote_ref) {
                Ok(remote_branch) => repo
                    .reference_to_annotated_commit(&remote_branch)
                    .map_err(|e| GitError {
                        message: format!("Failed to get annotated commit from remote branch: {}", e),
                        code: "PULL_ERROR".to_string(),
                    })?,
                Err(_) => {
                    // No remote tracking branch either - nothing to pull
                    return Ok(SyncResult {
                        success: true,
                        message: "No remote changes to pull".to_string(),
                        conflicts: vec![],
                        files_changed: vec![],
                    });
                }
            }
        }
    };

    // Analyze merge
    let (analysis, _) = repo
        .merge_analysis(&[&fetch_commit])
        .map_err(|e| GitError {
            message: format!("Failed to analyze merge: {}", e),
            code: "MERGE_ERROR".to_string(),
        })?;

    if analysis.is_up_to_date() {
        return Ok(SyncResult {
            success: true,
            message: "Already up to date".to_string(),
            conflicts: vec![],
            files_changed: vec![],
        });
    }

    if analysis.is_fast_forward() {
        // Fast-forward merge
        let refname = format!("refs/heads/{}", branch_name);
        let mut reference = repo
            .find_reference(&refname)
            .map_err(|e| GitError {
                message: format!("Failed to find reference: {}", e),
                code: "PULL_ERROR".to_string(),
            })?;

        reference
            .set_target(fetch_commit.id(), "Fast-forward pull")
            .map_err(|e| GitError {
                message: format!("Failed to update reference: {}", e),
                code: "PULL_ERROR".to_string(),
            })?;

        repo.set_head(&refname)
            .map_err(|e| GitError {
                message: format!("Failed to set HEAD: {}", e),
                code: "PULL_ERROR".to_string(),
            })?;

        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .map_err(|e| GitError {
                message: format!("Failed to checkout: {}", e),
                code: "PULL_ERROR".to_string(),
            })?;

        return Ok(SyncResult {
            success: true,
            message: "Fast-forward merge successful".to_string(),
            conflicts: vec![],
            files_changed: vec![],
        });
    }

    if analysis.is_normal() {
        // Perform merge
        let fetch_commit_obj = repo
            .find_commit(fetch_commit.id())
            .map_err(|e| GitError {
                message: format!("Failed to find commit: {}", e),
                code: "MERGE_ERROR".to_string(),
            })?;

        repo.merge(&[&fetch_commit], None, None)
            .map_err(|e| GitError {
                message: format!("Failed to merge: {}", e),
                code: "MERGE_ERROR".to_string(),
            })?;

        // Check for conflicts
        let mut index = repo.index().map_err(|e| GitError {
            message: format!("Failed to get index: {}", e),
            code: "INDEX_ERROR".to_string(),
        })?;

        if index.has_conflicts() {
            let conflicts: Vec<String> = index
                .conflicts()
                .map_err(|e| GitError {
                    message: format!("Failed to get conflicts: {}", e),
                    code: "CONFLICT_ERROR".to_string(),
                })?
                .filter_map(|c| c.ok())
                .filter_map(|c| c.our.map(|entry| String::from_utf8_lossy(&entry.path).to_string()))
                .collect();

            return Ok(SyncResult {
                success: false,
                message: "Merge conflicts detected".to_string(),
                conflicts,
                files_changed: vec![],
            });
        }

        // Create merge commit
        let sig = get_signature(&repo)?;
        let head_commit = repo
            .head()
            .map_err(|e| GitError {
                message: format!("Failed to get HEAD: {}", e),
                code: "REPO_ERROR".to_string(),
            })?
            .peel_to_commit()
            .map_err(|e| GitError {
                message: format!("Failed to peel to commit: {}", e),
                code: "REPO_ERROR".to_string(),
            })?;

        let tree_id = index
            .write_tree()
            .map_err(|e| GitError {
                message: format!("Failed to write tree: {}", e),
                code: "COMMIT_ERROR".to_string(),
            })?;

        let tree = repo
            .find_tree(tree_id)
            .map_err(|e| GitError {
                message: format!("Failed to find tree: {}", e),
                code: "COMMIT_ERROR".to_string(),
            })?;

        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "Merge remote-tracking branch",
            &tree,
            &[&head_commit, &fetch_commit_obj],
        )
        .map_err(|e| GitError {
            message: format!("Failed to create merge commit: {}", e),
            code: "COMMIT_ERROR".to_string(),
        })?;

        repo.cleanup_state()
            .map_err(|e| GitError {
                message: format!("Failed to cleanup state: {}", e),
                code: "REPO_ERROR".to_string(),
            })?;

        return Ok(SyncResult {
            success: true,
            message: "Merge successful".to_string(),
            conflicts: vec![],
            files_changed: vec![],
        });
    }

    Err(GitError {
        message: "Unable to merge".to_string(),
        code: "MERGE_ERROR".to_string(),
    })
}

#[tauri::command]
pub fn git_push_repo(
    path: String,
    credentials: Option<GitCredentials>,
) -> Result<SyncResult, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Handle unborn branch (no commits yet)
    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            return Ok(SyncResult {
                success: false,
                message: "Repository has no commits yet. Create a commit first before pushing.".to_string(),
                conflicts: vec![],
                files_changed: vec![],
            });
        }
        Err(e) => return Err(GitError {
            message: format!("Failed to get HEAD: {}", e),
            code: "REPO_ERROR".to_string(),
        }),
    };
    let branch_name = head
        .shorthand()
        .ok_or(GitError {
            message: "Failed to get branch name".to_string(),
            code: "REPO_ERROR".to_string(),
        })?
        .to_string();

    let mut remote = repo
        .find_remote("origin")
        .map_err(|e| GitError {
            message: format!("Failed to find remote 'origin': {}", e),
            code: "REMOTE_ERROR".to_string(),
        })?;

    let callbacks = create_callbacks(credentials);
    let mut push_opts = PushOptions::new();
    push_opts.remote_callbacks(callbacks);

    let refspec = format!("refs/heads/{}:refs/heads/{}", branch_name, branch_name);
    remote
        .push(&[&refspec], Some(&mut push_opts))
        .map_err(|e| GitError {
            message: format!("Failed to push: {}", e),
            code: "PUSH_ERROR".to_string(),
        })?;

    Ok(SyncResult {
        success: true,
        message: "Push successful".to_string(),
        conflicts: vec![],
        files_changed: vec![],
    })
}

#[tauri::command]
pub fn git_get_repo_status(path: String) -> Result<RepoStatus, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Get current branch - handle unborn branch (no commits yet)
    let (current_branch, is_unborn) = match repo.head() {
        Ok(head) => (head.shorthand().unwrap_or("HEAD").to_string(), false),
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            // Repository has no commits yet, get the target branch name
            let branch_name = repo
                .config()
                .ok()
                .and_then(|config| config.get_string("init.defaultBranch").ok())
                .unwrap_or_else(|| "main".to_string());
            (branch_name, true)
        }
        Err(e) => return Err(GitError {
            message: format!("Failed to get HEAD: {}", e),
            code: "REPO_ERROR".to_string(),
        }),
    };

    // Get file status
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    opts.include_ignored(false);

    let statuses = repo
        .statuses(Some(&mut opts))
        .map_err(|e| GitError {
            message: format!("Failed to get status: {}", e),
            code: "REPO_ERROR".to_string(),
        })?;

    let mut modified_files = Vec::new();
    let mut untracked_files = Vec::new();

    for entry in statuses.iter() {
        let status = entry.status();
        let path = entry.path().unwrap_or("").to_string();

        if status.is_wt_new() {
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

    // Calculate ahead/behind counts (skip if unborn branch)
    let (ahead, behind) = if is_unborn {
        (0, 0)
    } else {
        calculate_ahead_behind(&repo, &current_branch).unwrap_or((0, 0))
    };

    // Check for conflicts
    let index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;
    let has_conflicts = index.has_conflicts();

    Ok(RepoStatus {
        is_clean: pending_changes == 0 && !has_conflicts,
        pending_changes,
        ahead_by: ahead,
        behind_by: behind,
        has_conflicts,
        current_branch,
        modified_files,
        untracked_files,
    })
}

#[tauri::command]
pub fn git_commit_changes(path: String, message: String) -> Result<String, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    let mut index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    // Stage all changes
    index
        .add_all(["."].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| GitError {
            message: format!("Failed to add files: {}", e),
            code: "STAGE_ERROR".to_string(),
        })?;

    index.write().map_err(|e| GitError {
        message: format!("Failed to write index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    let tree_id = index
        .write_tree()
        .map_err(|e| GitError {
            message: format!("Failed to write tree: {}", e),
            code: "COMMIT_ERROR".to_string(),
        })?;

    let tree = repo
        .find_tree(tree_id)
        .map_err(|e| GitError {
            message: format!("Failed to find tree: {}", e),
            code: "COMMIT_ERROR".to_string(),
        })?;

    let sig = get_signature(&repo)?;

    // Get parent commit if it exists
    let parent_commit = repo.head().ok().and_then(|head| head.peel_to_commit().ok());

    let parents: Vec<&git2::Commit> = parent_commit.iter().collect();

    let commit_id = repo
        .commit(Some("HEAD"), &sig, &sig, &message, &tree, &parents)
        .map_err(|e| GitError {
            message: format!("Failed to create commit: {}", e),
            code: "COMMIT_ERROR".to_string(),
        })?;

    Ok(commit_id.to_string())
}

#[tauri::command]
pub fn git_stage_file(path: String, file_path: String) -> Result<(), GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    let mut index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    index
        .add_path(Path::new(&file_path))
        .map_err(|e| GitError {
            message: format!("Failed to stage file: {}", e),
            code: "STAGE_ERROR".to_string(),
        })?;

    index.write().map_err(|e| GitError {
        message: format!("Failed to write index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    Ok(())
}

#[tauri::command]
pub fn git_discard_file(path: String, file_path: String) -> Result<(), GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Handle unborn branch - can't discard if no commits exist
    let head = match repo.head() {
        Ok(head) => head,
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => {
            // For unborn branch, just remove from index if it was staged
            let mut index = repo.index().map_err(|e| GitError {
                message: format!("Failed to get index: {}", e),
                code: "INDEX_ERROR".to_string(),
            })?;
            index.remove_path(Path::new(&file_path)).ok();
            index.write().map_err(|e| GitError {
                message: format!("Failed to write index: {}", e),
                code: "INDEX_ERROR".to_string(),
            })?;
            return Ok(());
        }
        Err(e) => return Err(GitError {
            message: format!("Failed to get HEAD: {}", e),
            code: "REPO_ERROR".to_string(),
        }),
    };

    let head_commit = head
        .peel_to_commit()
        .map_err(|e| GitError {
            message: format!("Failed to get HEAD commit: {}", e),
            code: "REPO_ERROR".to_string(),
        })?;

    let head_tree = head_commit
        .tree()
        .map_err(|e| GitError {
            message: format!("Failed to get tree: {}", e),
            code: "REPO_ERROR".to_string(),
        })?;

    let mut checkout_opts = git2::build::CheckoutBuilder::new();
    checkout_opts.path(&file_path);
    checkout_opts.force();

    repo.checkout_tree(head_tree.as_object(), Some(&mut checkout_opts))
        .map_err(|e| GitError {
            message: format!("Failed to discard changes: {}", e),
            code: "DISCARD_ERROR".to_string(),
        })?;

    Ok(())
}

#[tauri::command]
pub fn git_resolve_conflict(path: String, file_path: String, resolution: String) -> Result<(), GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Write the resolved content
    let full_path = Path::new(&path).join(&file_path);
    std::fs::write(&full_path, &resolution)
        .map_err(|e| GitError {
            message: format!("Failed to write resolved file: {}", e),
            code: "CONFLICT_ERROR".to_string(),
        })?;

    // Stage the resolved file
    let mut index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    index
        .add_path(Path::new(&file_path))
        .map_err(|e| GitError {
            message: format!("Failed to stage resolved file: {}", e),
            code: "STAGE_ERROR".to_string(),
        })?;

    // Remove conflict markers from index
    index
        .remove_path(Path::new(&file_path))
        .ok(); // Ignore if not present

    index
        .add_path(Path::new(&file_path))
        .map_err(|e| GitError {
            message: format!("Failed to add resolved file: {}", e),
            code: "STAGE_ERROR".to_string(),
        })?;

    index.write().map_err(|e| GitError {
        message: format!("Failed to write index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    Ok(())
}

#[tauri::command]
pub fn git_get_conflict_content(path: String, file_path: String) -> Result<ConflictContent, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    let index = repo.index().map_err(|e| GitError {
        message: format!("Failed to get index: {}", e),
        code: "INDEX_ERROR".to_string(),
    })?;

    let mut ours = String::new();
    let mut theirs = String::new();
    let mut base = String::new();

    for conflict in index.conflicts().map_err(|e| GitError {
        message: format!("Failed to get conflicts: {}", e),
        code: "CONFLICT_ERROR".to_string(),
    })? {
        let conflict = conflict.map_err(|e| GitError {
            message: format!("Failed to read conflict: {}", e),
            code: "CONFLICT_ERROR".to_string(),
        })?;

        let conflict_path = conflict
            .our
            .as_ref()
            .or(conflict.their.as_ref())
            .map(|e| String::from_utf8_lossy(&e.path).to_string())
            .unwrap_or_default();

        if conflict_path != file_path {
            continue;
        }

        if let Some(entry) = conflict.ancestor {
            let blob = repo.find_blob(entry.id).ok();
            if let Some(blob) = blob {
                base = String::from_utf8_lossy(blob.content()).to_string();
            }
        }

        if let Some(entry) = conflict.our {
            let blob = repo.find_blob(entry.id).ok();
            if let Some(blob) = blob {
                ours = String::from_utf8_lossy(blob.content()).to_string();
            }
        }

        if let Some(entry) = conflict.their {
            let blob = repo.find_blob(entry.id).ok();
            if let Some(blob) = blob {
                theirs = String::from_utf8_lossy(blob.content()).to_string();
            }
        }

        break;
    }

    Ok(ConflictContent { base, ours, theirs })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConflictContent {
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

#[tauri::command]
pub fn git_set_remote(path: String, url: String) -> Result<(), GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    // Remove existing origin if present
    repo.remote_delete("origin").ok();

    // Add new origin
    repo.remote("origin", &url)
        .map_err(|e| GitError {
            message: format!("Failed to set remote: {}", e),
            code: "REMOTE_ERROR".to_string(),
        })?;

    Ok(())
}

#[tauri::command]
pub fn git_get_remote_url(path: String) -> Result<Option<String>, GitError> {
    let repo = Repository::open(Path::new(&path))
        .map_err(|e| GitError {
            message: format!("Failed to open repository: {}", e),
            code: "REPO_OPEN_ERROR".to_string(),
        })?;

    let result = match repo.find_remote("origin") {
        Ok(remote) => remote.url().map(|s| s.to_string()),
        Err(_) => None,
    };
    Ok(result)
}

fn get_signature(repo: &Repository) -> Result<Signature<'_>, GitError> {
    // Try to get signature from repo config
    if let Ok(sig) = repo.signature() {
        return Ok(sig);
    }

    // Fallback to default values
    Signature::now("Seaquel User", "seaquel@local")
        .map_err(|e| GitError {
            message: format!("Failed to create signature: {}", e),
            code: "COMMIT_ERROR".to_string(),
        })
}

fn calculate_ahead_behind(repo: &Repository, branch: &str) -> Option<(usize, usize)> {
    let local_branch = repo.find_branch(branch, git2::BranchType::Local).ok()?;
    let local_commit = local_branch.get().peel_to_commit().ok()?;

    // Try to get upstream tracking branch
    match local_branch.upstream() {
        Ok(upstream) => {
            let upstream_commit = upstream.get().peel_to_commit().ok()?;
            repo.graph_ahead_behind(local_commit.id(), upstream_commit.id())
                .ok()
        }
        Err(_) => {
            // No upstream tracking branch - check if remote exists
            // This happens when cloning an empty repo and making local commits
            if repo.find_remote("origin").is_ok() {
                // Check if remote branch exists
                let remote_ref = format!("refs/remotes/origin/{}", branch);
                match repo.find_reference(&remote_ref) {
                    Ok(remote_branch) => {
                        // Remote branch exists, calculate normally
                        let upstream_commit = remote_branch.peel_to_commit().ok()?;
                        repo.graph_ahead_behind(local_commit.id(), upstream_commit.id())
                            .ok()
                    }
                    Err(_) => {
                        // No remote branch - count all local commits as ahead
                        let mut count = 0usize;
                        let mut revwalk = repo.revwalk().ok()?;
                        revwalk.push(local_commit.id()).ok()?;
                        for _ in revwalk {
                            count += 1;
                        }
                        Some((count, 0))
                    }
                }
            } else {
                None
            }
        }
    }
}
