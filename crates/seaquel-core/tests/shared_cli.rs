//! Q26: the shared projection and the imports on a workspace built as
//! `seaquel-cli` builds it (read-only storage, a keychain): reads answer,
//! writes are `STORAGE_READ_ONLY`, and nothing reaches the repo.

// The shared test helpers' clock reads the wall clock (native tests).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
#![cfg(unix)]

mod common;
#[path = "shared/world.rs"]
mod world;

use std::path::Path;

use seaquel_core::domain::imports::ImportSource;
use seaquel_core::storage::StorageOptions;
use seaquel_core::{ImportPaths, LocalFiles, SyncTarget, WorkspaceSpec, WriteOrigin};
use serde_json::json;

use common::{insert_rows, T0};
use world::{git, World};

const Q: &str = "/repos/a/.seaquel/projects/team/queries";

fn origin() -> WriteOrigin {
    WriteOrigin::new(Some("main"))
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_calls_on_a_read_only_workspace() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    w.put(
        "/repos/a/.seaquel/projects/team/project.yaml",
        "name: Team\n",
    );
    let repo = w.git_repo("a").to_string_lossy().into_owned();
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": repo,
                "created_at": T0, "updated_at": T0}),
            json!({"id": "p2", "name": "Solo", "description": null, "git_repo_path": null,
                "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    let data = json!({"id": "repo-a", "name": "team-repo", "path": repo});
    insert_rows(
        w.ws.storage(),
        "shared_repos",
        &[json!({"id": "repo-a", "data": data.to_string()})],
    )
    .await;
    let home = tempfile::tempdir().unwrap();
    let dbeaver = home.path().join(
        seaquel_core::domain::imports::default_path(ImportSource::Dbeaver, std::env::consts::OS)
            .unwrap(),
    );
    std::fs::create_dir_all(dbeaver.parent().unwrap()).unwrap();
    std::fs::write(
        &dbeaver,
        r#"{"connections": {"pg-1": {"provider": "postgresql", "name": "Shop",
            "configuration": {"host": "db.example.com", "port": "5432", "database": "shop",
            "user": "app"}}}}"#,
    )
    .unwrap();
    // A Core built as `seaquel-cli` builds it: `LocalFiles` and the home.
    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(std::sync::Arc::new(seaquel_runtime::TokioExecutor))
        .local_files(LocalFiles::Allowed)
        .import_paths(ImportPaths::new(home.path()))
        .build();
    // Close the app's workspace, then open the file as `seaquel-cli` does.
    w.ws.close().await;
    let cli = core
        .open_workspace(
            WorkspaceSpec::new(&w.data)
                .with_storage_options(StorageOptions {
                    read_only: true,
                    ..Default::default()
                })
                .with_secrets(w.store.clone()),
        )
        .await
        .unwrap();
    let preview = cli.shared_scan(&core, &repo).await.unwrap();
    assert_eq!(preview.projects.len(), 1);
    assert_eq!(preview.projects[0].queries, 1);
    assert_eq!(preview.projects[0].linked_project_ids, ["p1"]);
    let err = cli
        .shared_sync(&core, &origin(), SyncTarget::Project("p1".into()))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_READ_ONLY");
    let err = cli
        .shared_repo_register(&core, &origin(), "/elsewhere", None, None)
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_READ_ONLY");
    let err = cli
        .shared_link_project(&core, &origin(), "p2", &repo, &[])
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_READ_ONLY");
    // The imports: candidates answer, the create is refused.
    let found = cli
        .import_candidates(&core, ImportSource::Dbeaver, "p1", None)
        .await
        .unwrap();
    assert_eq!(found.candidates.as_ref().map(Vec::len), Some(1));
    let err = cli
        .import_create(
            &core,
            &origin(),
            ImportSource::Dbeaver,
            "p1",
            &["pg-1".to_string()],
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_READ_ONLY");
    // Nothing reached the repo.
    assert_eq!(git(Path::new(&repo), &["status", "--porcelain"]).trim(), "");
    let _ = Q;
}
