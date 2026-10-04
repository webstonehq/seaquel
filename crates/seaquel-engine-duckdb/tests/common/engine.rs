//! The DuckDB driver the integration suites run on (the DuckDB helper plan,
//! Task 5). Every suite that opens DuckDB gets its engine from [`engine`],
//! never from `seaquel_engine_duckdb::engine()`, so one suite tests both
//! drivers.
//!
//! `SEAQUEL_TEST_DUCKDB_DRIVER` picks it: `native` (duckdb-rs in this
//! process, the `native` feature) or `remote` (a `seaquel-duckdb` child
//! process, the `remote` feature). Unset, it is `native` when the build has
//! it, else `remote`, so `cargo test --no-default-features --features
//! remote` runs every suite through the helper with no DuckDB in the test
//! binary. Any other value, or a driver the build lacks, panics: a
//! misconfigured run fails instead of testing the other driver.
//! `driver_switch.rs` checks the driver by what it does.
//!
//! The remote driver needs `SEAQUEL_TEST_DUCKDB_HELPER`, a built helper
//! (`cargo build -p seaquel-duckdb`). Each [`engine`] installs it (a hard
//! link, else a copy) as `bin/duckdb/<version>/seaquel-duckdb[.exe]` in a
//! folder of its own under `CARGO_TARGET_TMPDIR`, laid out and checked as a
//! real install is. The engine and every driver it opens hold the folder,
//! so it is removed only after the last of them is dropped, never while
//! its helper may still be starting or running.

#![allow(dead_code)]

use std::sync::Arc;

use seaquel_engine::Engine;

/// The variable that picks the driver.
pub const DRIVER_VAR: &str = "SEAQUEL_TEST_DUCKDB_DRIVER";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Native,
    Remote,
}

/// The driver this run asked for.
pub fn which() -> Which {
    match std::env::var(DRIVER_VAR) {
        Ok(v) if v == "native" => Which::Native,
        Ok(v) if v == "remote" => Which::Remote,
        Ok(v) => panic!("{DRIVER_VAR} is `native` or `remote`, not {v:?}"),
        Err(std::env::VarError::NotPresent) if cfg!(feature = "native") => Which::Native,
        Err(std::env::VarError::NotPresent) => Which::Remote,
        Err(e) => panic!("{DRIVER_VAR}: {e}"),
    }
}

/// The DuckDB engine this run tests.
pub fn engine() -> Arc<dyn Engine> {
    match which() {
        Which::Native => native(),
        Which::Remote => remote::engine(),
    }
}

#[cfg(feature = "native")]
fn native() -> Arc<dyn Engine> {
    seaquel_engine_duckdb::engine()
}

#[cfg(not(feature = "native"))]
fn native() -> Arc<dyn Engine> {
    panic!("{DRIVER_VAR}=native needs the `native` feature")
}

#[cfg(not(feature = "remote"))]
mod remote {
    use std::sync::Arc;

    use seaquel_engine::Engine;

    pub fn engine() -> Arc<dyn Engine> {
        panic!("{}=remote needs the `remote` feature", super::DRIVER_VAR)
    }
}

#[cfg(feature = "remote")]
mod remote {
    use std::path::{Path, PathBuf};
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
    }

    /// A remote driver holding its install folder. Every `Driver` method is
    /// passed on, defaults included, so the suites see the remote driver's
    /// own answers.
    struct Opened {
        inner: Arc<dyn Driver>,
        _dir: Arc<tempfile::TempDir>,
    }

    #[seaquel_runtime::async_trait]
    impl Driver for Opened {
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
        let bin = match std::env::var_os("SEAQUEL_TEST_DUCKDB_HELPER") {
            Some(path) => PathBuf::from(path),
            None => panic!(
                "the remote driver needs SEAQUEL_TEST_DUCKDB_HELPER: a built helper \
                 (cargo build -p seaquel-duckdb)"
            ),
        };
        let version = version(&bin);
        let dir = tempfile::Builder::new()
            .prefix("suite-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let bin_dir = dir.path().join("bin");
        let root = bin_dir.join("duckdb");
        let folder = root.join(&version);
        std::fs::create_dir_all(&folder).unwrap();
        // The start refuses a folder others can write (Decision 9).
        #[cfg(unix)]
        for d in [&bin_dir, &root, &folder] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let locator = HelperLocator { dir: root, version };
        let to = locator.path();
        if std::fs::hard_link(&bin, &to).is_err() {
            std::fs::copy(&bin, &to).unwrap();
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
                    .unwrap_or_else(|e| panic!("SEAQUEL_TEST_DUCKDB_HELPER --version: {e}"));
                let text = String::from_utf8(out.stdout).unwrap();
                text.trim()
                    .strip_prefix("seaquel-duckdb ")
                    .unwrap_or_else(|| panic!("not the DuckDB helper's --version: {text:?}"))
                    .to_string()
            })
            .clone()
    }
}
