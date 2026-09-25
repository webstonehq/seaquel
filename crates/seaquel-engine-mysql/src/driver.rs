use sqlx::{MySql, Pool};

use seaquel_engine::{
    ConnectConfig, DatabaseStatistics, DbError, ExplainResult, SchemaColumn, SchemaIndex,
    SchemaTable, Value,
};

use crate::introspect::{self, Flavor};

pub struct MysqlDriver {
    pool: Pool<MySql>,
    /// MySQL or MariaDB, from `SELECT VERSION()` at connect.
    flavor: Flavor,
}

impl MysqlDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for MySQL"))?;

        let pool = Pool::<MySql>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        let version: (String,) = sqlx::query_as(introspect::VERSION_SQL)
            .fetch_one(&pool)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self {
            pool,
            flavor: Flavor::from_version(&version.0),
        })
    }
}

/// The MySQL error number and SQLSTATE of a server error, from the text
/// sqlx gives it (`error returned from database: 1142 (42000): …`), which is
/// all a `DbError` keeps.
fn server_error(e: &DbError) -> Option<(u16, &str)> {
    let rest = e.message.split_once("error returned from database: ")?.1;
    let (number, rest) = rest.split_once(" (")?;
    let (sqlstate, _) = rest.split_once("): ")?;
    Some((number.parse().ok()?, sqlstate))
}

/// A read the server refused for lack of privileges: ER_TABLEACCESS_DENIED
/// (1142), ER_COLUMNACCESS_DENIED (1143) or ER_DBACCESS_DENIED (1044), all
/// SQLSTATE 42000, on MySQL and MariaDB alike.
fn is_permission_error(e: &DbError) -> bool {
    matches!(server_error(e), Some((1142 | 1143 | 1044, "42000")))
}

seaquel_engine::impl_sqlx_driver!(
    MysqlDriver,
    MySql,
    sqlx::mysql::MySqlArguments,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |r: &sqlx::mysql::MySqlQueryResult| Some(r.last_insert_id() as i64),
    introspection = {
        async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
            let r = self.query(introspect::SCHEMAS_SQL, vec![]).await?;
            Ok(introspect::parse_schemas(&r))
        }

        async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
            let r = self.query(introspect::SCHEMA_SQL, vec![]).await?;
            Ok(introspect::parse_schema(&r))
        }

        async fn table_metadata(
            &self,
            schema: &str,
            table: &str,
        ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
            // Bug fix 1: bound as ? = table, ? = schema.
            let params = || vec![Value::from(table), Value::from(schema)];
            let (columns, indexes) = futures::join!(
                self.query(introspect::COLUMNS_SQL, params()),
                self.query(introspect::INDEXES_SQL, params()),
            );
            let mut columns = introspect::parse_columns(&columns?, self.flavor);
            let indexes = introspect::parse_indexes(&indexes?);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's).
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &indexes);
            Ok((columns, indexes))
        }

        /// Index usage reads `mysql.innodb_index_stats`, which needs a grant
        /// on the `mysql` schema. Without one it is empty instead of failing
        /// the whole Statistics view.
        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            let index_usage = match index_usage {
                Ok(r) => introspect::parse_index_usage(&r),
                Err(e) if is_permission_error(&e) => vec![],
                Err(e) => return Err(e),
            };
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes: introspect::parse_table_sizes(&table_sizes?),
                index_usage,
            })
        }

        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let r = self
                .query(&introspect::explain_sql(sql, analyze, self.flavor), params)
                .await?;
            introspect::parse_explain(&r, analyze, self.flavor)
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_errors() {
        let e = DbError::query_error(
            "error returned from database: 1142 (42000): SELECT command denied to user 'u'@'%' for table 'innodb_index_stats'",
        );
        assert!(is_permission_error(&e));
        assert!(!is_permission_error(&DbError::query_error(
            "error returned from database: 1054 (42S22): Unknown column 'x'"
        )));
        // Only the server's code counts, not the words.
        assert!(!is_permission_error(&DbError::query_error(
            "error returned from database: 1146 (42S02): Table 'access denied 1142' doesn't exist"
        )));
        assert!(!is_permission_error(&DbError::query_error(
            "pool timed out: access denied"
        )));
    }
}
