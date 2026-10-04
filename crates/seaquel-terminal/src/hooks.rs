//! The terminal binaries' test hooks, read in **debug builds only**: a
//! release build ignores them, so a shipped binary has no way to take
//! secrets from a file. Each binary reads its own prefix
//! (`SEAQUEL_CLI_TEST`, `SEAQUEL_TUI_TEST`) and nothing else:
//!
//! - `<prefix>_SECRETS`: a JSON file `{"db:<id>": "…"}` loaded into a
//!   `MemoryStore` in place of the keychain (which also takes the binary's
//!   writes);
//! - `<prefix>_KNOWN_HOSTS`: the known_hosts file to check (and, for the
//!   TUI, write);
//! - `<prefix>_CALL_TIMEOUT_MS`: the MCP server's per-call timeout;
//! - `<prefix>_ORIGIN`: the TUI's write origin, fixed for snapshot tests;
//! - `<prefix>_PANIC`: makes the TUI panic (`main` or `task`), for the
//!   terminal-restore tests;
//! - `<prefix>_DUCKDB_HELPER`: a built `seaquel-duckdb`, linked (or
//!   copied) into the helper's folder layout when Core is built, so DuckDB
//!   connects through it (`CoreOptions::with_hooks`);
//! - `<prefix>_DUCKDB_RELEASES`: a release server on a loopback address
//!   (`http://127.0.0.1:<port>`, serving `/api/v<version>` and
//!   `/download/v<version>/<asset>` as `seaquel-http`'s `MockReleases`
//!   does), in place of GitHub's, for the helper's download.
//!
//! An empty value counts as unset. The tests set these (with
//! `SEAQUEL_DATA_DIR`), so they never touch the real keychain, data dir or
//! `~/.ssh`.

use std::sync::Arc;
use std::time::Duration;

use seaquel_core::secrets::{KeychainStore, SecretStore, DESKTOP_SERVICE};

/// One binary's test hooks; every field `None` in a release build.
#[derive(Clone, Default)]
pub struct TestHooks {
    prefix: String,
    secrets: Option<String>,
    known_hosts: Option<String>,
    call_timeout: Option<String>,
    origin: Option<String>,
    panic: Option<String>,
    duckdb_helper: Option<String>,
    duckdb_releases: Option<String>,
}

/// Which hooks are set, never their values (paths and origins stay out of
/// logs).
impl std::fmt::Debug for TestHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestHooks")
            .field("prefix", &self.prefix)
            .field("secrets", &self.secrets.is_some())
            .field("known_hosts", &self.known_hosts.is_some())
            .field("call_timeout", &self.call_timeout.is_some())
            .field("origin", &self.origin.is_some())
            .field("panic", &self.panic.is_some())
            .field("duckdb_helper", &self.duckdb_helper.is_some())
            .field("duckdb_releases", &self.duckdb_releases.is_some())
            .finish()
    }
}

impl TestHooks {
    /// The hooks under `prefix` from the process environment.
    pub fn from_env(prefix: &str) -> Self {
        Self::from_lookup(prefix, |name| std::env::var(name).ok())
    }

    /// The hooks under `prefix`, read through `lookup` (tests pass a map).
    /// Always empty in a release build.
    pub fn from_lookup(prefix: &str, lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut hooks = TestHooks {
            prefix: prefix.to_string(),
            ..TestHooks::default()
        };
        if !cfg!(debug_assertions) {
            return hooks;
        }
        let read = |hook: &str| lookup(&hooks.env_name(hook)).filter(|v| !v.is_empty());
        let secrets = read("SECRETS");
        let known_hosts = read("KNOWN_HOSTS");
        let call_timeout = read("CALL_TIMEOUT_MS");
        let origin = read("ORIGIN");
        let panic = read("PANIC");
        let duckdb_helper = read("DUCKDB_HELPER");
        let duckdb_releases = read("DUCKDB_RELEASES");
        hooks.duckdb_helper = duckdb_helper;
        hooks.duckdb_releases = duckdb_releases;
        hooks.secrets = secrets;
        hooks.known_hosts = known_hosts;
        hooks.call_timeout = call_timeout;
        hooks.origin = origin;
        hooks.panic = panic;
        hooks
    }

    /// The variable a hook is read from: `<prefix>_<hook>`.
    pub fn env_name(&self, hook: &str) -> String {
        format!("{}_{hook}", self.prefix)
    }

    /// `<prefix>_KNOWN_HOSTS`.
    pub fn known_hosts(&self) -> Option<&str> {
        self.known_hosts.as_deref()
    }

    /// `<prefix>_CALL_TIMEOUT_MS`, when it's a whole number of milliseconds.
    pub fn call_timeout(&self) -> Option<Duration> {
        self.call_timeout
            .as_deref()
            .and_then(|v| v.parse().ok())
            .map(Duration::from_millis)
    }

    /// `<prefix>_ORIGIN`.
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// `<prefix>_PANIC`.
    pub fn panic(&self) -> Option<&str> {
        self.panic.as_deref()
    }

    /// `<prefix>_DUCKDB_HELPER`: a built helper's file.
    pub fn duckdb_helper(&self) -> Option<&str> {
        self.duckdb_helper.as_deref()
    }

    /// `<prefix>_DUCKDB_RELEASES`: the release server's base address.
    pub fn duckdb_releases(&self) -> Option<&str> {
        self.duckdb_releases.as_deref()
    }

    /// Whether `<prefix>_SECRETS` is set (the store is then a
    /// `MemoryStore`).
    pub fn has_test_secrets(&self) -> bool {
        self.secrets.is_some()
    }

    /// The secret store: the `<prefix>_SECRETS` file loaded into a
    /// `MemoryStore`, else the keychain (service `app.seaquel.desktop` in
    /// every build). An unreadable file is an error naming the variable.
    pub async fn secret_store(&self) -> Result<Arc<dyn SecretStore>, String> {
        let Some(path) = self.secrets.as_deref() else {
            return Ok(Arc::new(KeychainStore::new(DESKTOP_SERVICE)));
        };
        let var = self.env_name("SECRETS");
        let text = std::fs::read_to_string(path).map_err(|e| format!("{var}: {e}"))?;
        let entries: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&text).map_err(|e| format!("{var}: {e}"))?;
        let store = seaquel_core::secrets::MemoryStore::new();
        for (key, value) in entries {
            store
                .set(&key, &value)
                .await
                .map_err(|e| format!("{var}: {e}"))?;
        }
        Ok(Arc::new(store))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    const BOTH: &[(&str, &str)] = &[
        ("SEAQUEL_CLI_TEST_KNOWN_HOSTS", "/cli/known_hosts"),
        ("SEAQUEL_CLI_TEST_CALL_TIMEOUT_MS", "250"),
        ("SEAQUEL_TUI_TEST_KNOWN_HOSTS", "/tui/known_hosts"),
        ("SEAQUEL_TUI_TEST_ORIGIN", "tui-0000abcd"),
        ("SEAQUEL_TUI_TEST_PANIC", "task"),
        ("SEAQUEL_TUI_TEST_CALL_TIMEOUT_MS", "soon"),
        ("SEAQUEL_TUI_TEST_DUCKDB_HELPER", "/tui/seaquel-duckdb"),
        ("SEAQUEL_CLI_TEST_DUCKDB_HELPER", "/cli/seaquel-duckdb"),
        ("SEAQUEL_TUI_TEST_DUCKDB_RELEASES", "http://127.0.0.1:9"),
    ];

    #[cfg(debug_assertions)]
    #[test]
    fn reads_its_own_prefix_only() {
        let tui = TestHooks::from_lookup("SEAQUEL_TUI_TEST", lookup(BOTH));
        assert_eq!(tui.known_hosts(), Some("/tui/known_hosts"));
        assert_eq!(tui.origin(), Some("tui-0000abcd"));
        assert_eq!(tui.panic(), Some("task"));
        assert_eq!(tui.call_timeout(), None, "not a number");
        assert!(!tui.has_test_secrets());
        assert_eq!(tui.duckdb_helper(), Some("/tui/seaquel-duckdb"));
        assert_eq!(tui.duckdb_releases(), Some("http://127.0.0.1:9"));

        let cli = TestHooks::from_lookup("SEAQUEL_CLI_TEST", lookup(BOTH));
        assert_eq!(cli.known_hosts(), Some("/cli/known_hosts"));
        assert_eq!(cli.call_timeout(), Some(Duration::from_millis(250)));
        assert_eq!(cli.origin(), None);
        assert_eq!(cli.panic(), None);
        assert_eq!(cli.env_name("SECRETS"), "SEAQUEL_CLI_TEST_SECRETS");
        assert_eq!(cli.duckdb_helper(), Some("/cli/seaquel-duckdb"));
        assert_eq!(cli.duckdb_releases(), None);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn an_empty_value_is_unset() {
        let hooks = TestHooks::from_lookup(
            "SEAQUEL_TUI_TEST",
            lookup(&[
                ("SEAQUEL_TUI_TEST_ORIGIN", ""),
                ("SEAQUEL_TUI_TEST_SECRETS", ""),
            ]),
        );
        assert_eq!(hooks.origin(), None);
        assert!(!hooks.has_test_secrets());
    }

    /// Run with `cargo test --release -p seaquel-terminal`.
    #[cfg(not(debug_assertions))]
    #[test]
    fn a_release_build_reads_nothing() {
        let mut vars = BOTH.to_vec();
        vars.push(("SEAQUEL_TUI_TEST_SECRETS", "/tui/secrets.json"));
        let hooks = TestHooks::from_lookup("SEAQUEL_TUI_TEST", lookup(&vars));
        assert_eq!(hooks.known_hosts(), None);
        assert_eq!(hooks.origin(), None);
        assert_eq!(hooks.panic(), None);
        assert_eq!(hooks.call_timeout(), None);
        assert!(!hooks.has_test_secrets());
        assert_eq!(hooks.duckdb_helper(), None);
        assert_eq!(hooks.duckdb_releases(), None);
    }

    #[test]
    fn debug_names_the_hooks_not_their_values() {
        let hooks = TestHooks::from_lookup("SEAQUEL_TUI_TEST", lookup(BOTH));
        let text = format!("{hooks:?}");
        assert!(!text.contains("/tui"), "{text}");
        assert!(!text.contains("tui-0000abcd"), "{text}");
        assert!(!text.contains("127.0.0.1"), "{text}");
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn the_secrets_file_becomes_a_memory_store() {
        let dir =
            std::env::temp_dir().join(format!("seaquel-terminal-hooks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("secrets.json");
        std::fs::write(&file, r#"{"db:conn-1": "pw"}"#).unwrap();
        let path = file.to_str().unwrap().to_string();
        let hooks = TestHooks::from_lookup(
            "SEAQUEL_TUI_TEST",
            lookup(&[("SEAQUEL_TUI_TEST_SECRETS", &path)]),
        );
        assert!(hooks.has_test_secrets());
        let store = hooks.secret_store().await.unwrap();
        assert_eq!(store.get("db:conn-1").await.unwrap().as_deref(), Some("pw"));

        std::fs::write(&file, "not json").unwrap();
        let err = match hooks.secret_store().await {
            Err(e) => e,
            Ok(_) => panic!("a bad file is refused"),
        };
        assert!(err.starts_with("SEAQUEL_TUI_TEST_SECRETS: "), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
