use sqlx::{Pool, Postgres};

use seaquel_engine::{
    ConnectConfig, DatabaseStatistics, DbError, Dialect, ExplainResult, SchemaColumn, SchemaIndex,
    SchemaTable, Value,
};

use crate::dialect::PostgresDialect;
use crate::introspect;

pub struct PostgresDriver {
    pool: Pool<Postgres>,
}

impl PostgresDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config.connection_string.as_deref().ok_or_else(|| {
            DbError::connection_error("connection_string is required for PostgreSQL")
        })?;

        let pool = Pool::<Postgres>::connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    PostgresDriver,
    Postgres,
    sqlx::postgres::PgArguments,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |_: &_| None,
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
            // Bug fix 1: bound as $1 = table, $2 = schema.
            let params = || vec![Value::from(table), Value::from(schema)];
            let (columns, indexes, partial) = futures::join!(
                self.query(introspect::COLUMNS_SQL, params()),
                self.query(introspect::INDEXES_SQL, params()),
                self.query(introspect::PARTIAL_UNIQUE_SQL, params()),
            );
            let mut columns = introspect::parse_columns(&columns?);
            let indexes = introspect::parse_indexes(&indexes?);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's),
            // without partial and INCLUDE unique indexes.
            let partial: Vec<Value> = partial?
                .rows
                .into_iter()
                .map(|mut r| r.swap_remove(0))
                .collect();
            let plain: Vec<SchemaIndex> = indexes
                .iter()
                .filter(|i| !partial.contains(&Value::Text(i.name.clone())))
                .cloned()
                .collect();
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &plain);
            Ok((columns, indexes))
        }

        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes: introspect::parse_table_sizes(&table_sizes?),
                index_usage: introspect::parse_index_usage(&index_usage?),
            })
        }

        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let r = self
                .query(&PostgresDialect.explain_sql(sql, analyze), params)
                .await?;
            introspect::parse_explain(&r, analyze)
        }
    }
);
