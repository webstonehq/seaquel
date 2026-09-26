use sqlparser::dialect::{
    Dialect, DuckDbDialect, MsSqlDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};

use crate::SqlEngine;

/// The sqlparser dialect for an engine. MariaDB has no dialect of its own in
/// sqlparser; it uses MySQL's.
pub(crate) fn for_engine(engine: SqlEngine) -> Box<dyn Dialect> {
    match engine {
        SqlEngine::Postgres => Box::new(PostgreSqlDialect {}),
        SqlEngine::Mysql | SqlEngine::Mariadb => Box::new(MySqlDialect {}),
        SqlEngine::Sqlite => Box::new(SQLiteDialect {}),
        SqlEngine::Mssql => Box::new(MsSqlDialect {}),
        SqlEngine::Duckdb => Box::new(DuckDbDialect {}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mariadb_uses_the_mysql_dialect() {
        let d = for_engine(SqlEngine::Mariadb);
        assert!(d.is::<MySqlDialect>());
        assert!(for_engine(SqlEngine::Postgres).is::<PostgreSqlDialect>());
        assert!(for_engine(SqlEngine::Duckdb).is::<DuckDbDialect>());
    }
}
