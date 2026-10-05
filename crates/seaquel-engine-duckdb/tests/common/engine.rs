//! The DuckDB engine the integration suites run on: the helper's client
//! (the `remote` driver) over a `seaquel-duckdb` child process, the only
//! way any interface reaches DuckDB since the native driver went.
//! Every suite that opens DuckDB gets
//! its engine from [`engine`].
//!
//! The helper is `SEAQUEL_TEST_DUCKDB_HELPER`, else the `seaquel-duckdb`
//! built beside the test binary (`target/<profile>/`, which `cargo test
//! --workspace` builds). With neither, [`engine`] panics naming `cargo
//! build -p seaquel-duckdb`, so a suite can't pass by skipping. Each
//! [`engine`] installs it (a hard link, else a copy) as
//! `bin/duckdb/<version>/seaquel-duckdb[.exe]` in a folder of its own under
//! `CARGO_TARGET_TMPDIR`, laid out and checked as a real install is. The
//! engine and every driver it opens hold the folder, so it is removed only
//! after the last of them is dropped, never while its helper may still be
//! starting or running.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use seaquel_engine::Engine;

/// The variable naming a built helper.
pub const HELPER_VAR: &str = "SEAQUEL_TEST_DUCKDB_HELPER";

/// The helper to run: `var` (the variable's value), else `seaquel-duckdb`
/// in the profile folder above `exe` (a test binary in
/// `target/<profile>/deps/`), else the build hint.
pub fn helper_from(var: Option<OsString>, exe: &Path) -> Result<PathBuf, String> {
    if let Some(path) = var {
        return Ok(PathBuf::from(path));
    }
    let sibling = exe
        .parent()
        .and_then(Path::parent)
        .map(|profile| profile.join(format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX)));
    match sibling {
        Some(bin) if bin.is_file() => Ok(bin),
        _ => Err(format!(
            "the DuckDB suites run through the helper: set {HELPER_VAR} or run \
             cargo build -p seaquel-duckdb"
        )),
    }
}

/// The helper this run uses; panics with the build hint without one.
pub fn helper() -> PathBuf {
    let exe = std::env::current_exe().expect("the test binary's path");
    helper_from(std::env::var_os(HELPER_VAR), &exe).unwrap_or_else(|e| panic!("{e}"))
}

/// The DuckDB engine this run tests.
pub fn engine() -> Arc<dyn Engine> {
    remote::engine()
}

#[cfg(not(feature = "remote"))]
mod remote {
    use std::sync::Arc;

    use seaquel_engine::Engine;

    pub fn engine() -> Arc<dyn Engine> {
        panic!("the DuckDB suites need the `remote` feature (the default)")
    }
}

#[cfg(feature = "remote")]
mod remote {
    use std::path::Path;
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use seaquel_engine::{
        BatchStatement, BoxStream, CancellationToken, CappedResult, ConnectConfig,
        DatabaseStatistics, DbError, Dialect, Driver, Engine, ExecuteResult, ExplainResult,
        OpenOptions, QueryResult, ReadOnlyOptions, SchemaColumn, SchemaIndex, SchemaTable,
        StreamBatch, TransactionError, Value,
    };
    use seaquel_engine_duckdb::{remote_engine, HelperLocator};

    /// The remote engine over a helper installed for it alone. The engine
    /// and every driver it opens share the install folder, so the folder
    /// goes only when the last of them is dropped, never under a helper
    /// still running.
    struct Installed {
        inner: Arc<dyn Engine>,
        dir: Arc<tempfile::TempDir>,
    }

    #[seaquel_runtime::async_trait]
    impl Engine for Installed {
        fn id(&self) -> &'static str {
            self.inner.id()
        }

        async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            let driver = self.inner.open(config).await?;
            Ok(Arc::new(Opened {
                inner: driver,
                _dir: self.dir.clone(),
            }))
        }

        async fn open_with(
            &self,
            config: &ConnectConfig,
            options: OpenOptions,
        ) -> Result<Arc<dyn Driver>, DbError> {
            let driver = self.inner.open_with(config, options).await?;
            Ok(Arc::new(Opened {
                inner: driver,
                _dir: self.dir.clone(),
            }))
        }

        fn dialect(&self) -> Option<&dyn Dialect> {
            self.inner.dialect()
        }

        fn preflight(&self, config: &ConnectConfig) -> Result<(), DbError> {
            self.inner.preflight(config)
        }

        fn exclusive_file(&self, config: &ConnectConfig) -> bool {
            self.inner.exclusive_file(config)
        }
    }

    /// A remote driver holding its install folder. Every `Driver` method is
    /// passed on, defaults included, so the suites see the remote driver's
    /// own answers. (The engine above passes on every `Engine` method the
    /// same way.)
    struct Opened {
        inner: Arc<dyn Driver>,
        _dir: Arc<tempfile::TempDir>,
    }

    #[seaquel_runtime::async_trait]
    impl Driver for Opened {
        fn closed(&self) -> Option<seaquel_runtime::BoxFuture<'static, Option<DbError>>> {
            self.inner.closed()
        }

        async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
            self.inner.query(sql, params).await
        }

        async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
            self.inner.execute(sql, params).await
        }

        async fn transaction(
            &self,
            statements: Vec<BatchStatement>,
        ) -> Result<Vec<u64>, TransactionError> {
            self.inner.transaction(statements).await
        }

        fn query_stream<'a>(
            &'a self,
            sql: String,
            params: Vec<Value>,
            cancel: CancellationToken,
        ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
            self.inner.query_stream(sql, params, cancel)
        }

        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            max_rows: Option<usize>,
        ) -> Result<CappedResult, DbError> {
            self.inner.query_read_only(sql, params, max_rows).await
        }

        async fn query_read_only_with(
            &self,
            sql: &str,
            params: Vec<Value>,
            options: ReadOnlyOptions,
        ) -> Result<CappedResult, DbError> {
            self.inner.query_read_only_with(sql, params, options).await
        }

        async fn explain_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
            timeout: Option<Duration>,
        ) -> Result<ExplainResult, DbError> {
            self.inner.explain_read_only(sql, params, timeout).await
        }

        async fn close(&self) -> Result<(), DbError> {
            self.inner.close().await
        }

        async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
            self.inner.list_schemas().await
        }

        async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
            self.inner.schema_tables().await
        }

        async fn table_metadata(
            &self,
            schema: &str,
            table: &str,
        ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
            self.inner.table_metadata(schema, table).await
        }

        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            self.inner.statistics().await
        }

        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            self.inner.explain(sql, params, analyze).await
        }
    }

    pub fn engine() -> Arc<dyn Engine> {
        let bin = super::helper();
        let version = version(&bin);
        let dir = tempfile::Builder::new()
            .prefix("suite-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let bin_dir = dir.path().join("bin");
        let root = bin_dir.join("duckdb");
        let folder = root.join(&version);
        std::fs::create_dir_all(&folder).unwrap();
        // The start refuses a folder others can write.
        #[cfg(unix)]
        for d in [&bin_dir, &root, &folder] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // On Windows it reads each level's DACL, up to the folder above
        // `bin`; a temp folder inherits whatever the checkout's has.
        #[cfg(windows)]
        for d in [dir.path(), &bin_dir, &root, &folder] {
            seaquel_runtime::acl::make_private(d, true).unwrap();
        }
        let locator = HelperLocator { dir: root, version };
        let to = locator.path();
        // A hard link would share the built file's DACL, so Windows copies
        // it and makes the copy private.
        #[cfg(unix)]
        if std::fs::hard_link(&bin, &to).is_err() {
            std::fs::copy(&bin, &to).unwrap();
        }
        #[cfg(windows)]
        {
            std::fs::copy(&bin, &to).unwrap();
            seaquel_runtime::acl::make_private(&to, false).unwrap();
        }
        Arc::new(Installed {
            inner: remote_engine(locator),
            dir: Arc::new(dir),
        })
    }

    /// The app version the built helper reports (`--version`), once per
    /// test binary.
    fn version(bin: &Path) -> String {
        static VERSION: OnceLock<String> = OnceLock::new();
        VERSION
            .get_or_init(|| {
                let out = std::process::Command::new(bin)
                    .arg("--version")
                    .output()
                    .unwrap_or_else(|e| panic!("{} --version: {e}", bin.display()));
                let text = String::from_utf8(out.stdout).unwrap();
                text.trim()
                    .strip_prefix("seaquel-duckdb ")
                    .unwrap_or_else(|| panic!("not the DuckDB helper's --version: {text:?}"))
                    .to_string()
            })
            .clone()
    }
}
