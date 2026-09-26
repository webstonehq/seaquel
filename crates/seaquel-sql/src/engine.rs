use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The engine whose quoting and dialect apply: one variant per `DatabaseType`
/// in `src/lib/types/database.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SqlEngine {
    Postgres,
    Mysql,
    /// Quotes like MySQL. sqlparser has no MariaDB dialect, so the AST helpers
    /// use its MySQL one.
    Mariadb,
    Sqlite,
    Mssql,
    Duckdb,
}

impl SqlEngine {
    pub const ALL: [SqlEngine; 6] = [
        SqlEngine::Postgres,
        SqlEngine::Mysql,
        SqlEngine::Mariadb,
        SqlEngine::Sqlite,
        SqlEngine::Mssql,
        SqlEngine::Duckdb,
    ];

    /// The `DatabaseType` id.
    pub fn as_str(self) -> &'static str {
        match self {
            SqlEngine::Postgres => "postgres",
            SqlEngine::Mysql => "mysql",
            SqlEngine::Mariadb => "mariadb",
            SqlEngine::Sqlite => "sqlite",
            SqlEngine::Mssql => "mssql",
            SqlEngine::Duckdb => "duckdb",
        }
    }

    /// MySQL or MariaDB.
    pub fn is_mysql(self) -> bool {
        matches!(self, SqlEngine::Mysql | SqlEngine::Mariadb)
    }
}

impl fmt::Display for SqlEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An engine id that isn't a `DatabaseType`. The wasm boundary reports it as
/// an error instead of falling back to some dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEngine(pub String);

impl fmt::Display for UnknownEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown engine: {:?}", self.0)
    }
}

impl std::error::Error for UnknownEngine {}

impl FromStr for SqlEngine {
    type Err = UnknownEngine;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SqlEngine::ALL
            .into_iter()
            .find(|e| e.as_str() == s)
            .ok_or_else(|| UnknownEngine(s.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_database_type() {
        for e in SqlEngine::ALL {
            assert_eq!(e.as_str().parse::<SqlEngine>(), Ok(e));
            assert_eq!(
                serde_json::to_string(&e).unwrap(),
                format!("\"{}\"", e.as_str())
            );
        }
    }

    #[test]
    fn rejects_unknown_ids() {
        for id in ["", "postgresql", "Postgres", "generic", "mysql "] {
            let err = id.parse::<SqlEngine>().unwrap_err();
            assert_eq!(err, UnknownEngine(id.to_string()));
            assert_eq!(err.to_string(), format!("unknown engine: {id:?}"));
        }
    }
}
