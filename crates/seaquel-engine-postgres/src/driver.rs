use std::sync::OnceLock;

use futures::future::BoxFuture;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use sqlx::postgres::{PgConnection, PgQueryResult, PgRow, PgStatement, PgTypeInfo};
use sqlx::{Either, Pool, Postgres};

use seaquel_engine::{
    ConnectConfig, DatabaseStatistics, DbError, Dialect, ExplainResult, QueryResult, SchemaColumn,
    SchemaIndex, SchemaTable, Value,
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
    },
    read_only = {
        /// A read-only transaction (AI safety plan, Decision 1) on a pooled
        /// connection that is closed afterwards, never returned: `ROLLBACK`
        /// doesn't undo session state such as advisory locks or `PREPARE`d
        /// statements. The user's SQL goes through `fetch_capped`, which uses
        /// the extended protocol, so a second statement is refused; the
        /// simple protocol would let `SET transaction_read_only = off;
        /// DELETE …` through as the first statement of the transaction.
        ///
        /// Taking the connection from the pool means a pool at its limit
        /// makes this wait, like any other query, instead of opening more.
        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
        ) -> Result<QueryResult, DbError> {
            let mut conn = self.pool.acquire().await.map_err(DbError::query_error)?;
            // Before the first statement, so an error or a dropped future
            // closes it too.
            conn.close_on_drop();
            sqlx::Executor::execute(&mut *conn, "BEGIN READ ONLY")
                .await
                .map_err(DbError::query_error)?;
            let refusal = OnceLock::new();
            let executor = NoteRefusal {
                conn: &mut conn,
                refusal: &refusal,
            };
            let result = fetch_capped(executor, sql, &params).await;
            // Only after a query that read all its rows. After an error or
            // the row cap, the connection may still be receiving the rest of
            // the result, and `ROLLBACK` would read every remaining row
            // before its own reply; closing makes the server abort the
            // transaction instead.
            if result.is_ok() {
                if let Err(e) = sqlx::Executor::execute(&mut *conn, "ROLLBACK").await {
                    seaquel_engine::__private::log::warn!(
                        activity = "db.query_read_only";
                        "ROLLBACK failed; closing the connection anyway: {e}"
                    );
                }
            }
            // Closing now rather than on drop frees its session state
            // (advisory locks) as soon as the server sees it go.
            let _ = conn.close().await;
            result.map_err(|e| match refusal.into_inner() {
                Some(message) => DbError::read_only(message),
                None => e,
            })
        }
    }
);

/// SQLSTATE `read_only_sql_transaction`: a write refused in a read-only
/// transaction.
const READ_ONLY_SQLSTATE: &str = "25006";

/// One connection, as an executor that notes the message of a read-only
/// refusal (SQLSTATE 25006) its query fails with. `fetch_capped` maps a
/// failure to its text, and Postgres's text has no SQLSTATE in it.
#[derive(Debug)]
struct NoteRefusal<'c> {
    conn: &'c mut PgConnection,
    refusal: &'c OnceLock<String>,
}

fn note_refusal(refusal: &OnceLock<String>, e: &sqlx::Error) {
    if let sqlx::Error::Database(db) = e {
        if db.code().as_deref() == Some(READ_ONLY_SQLSTATE) {
            let _ = refusal.set(db.message().to_string());
        }
    }
}

impl<'c> sqlx::Executor<'c> for NoteRefusal<'c> {
    type Database = Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxStream<'e, Result<Either<PgQueryResult, PgRow>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + sqlx::Execute<'q, Postgres>,
    {
        let refusal = self.refusal;
        self.conn
            .fetch_many(query)
            .inspect_err(move |e| note_refusal(refusal, e))
            .boxed()
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<PgRow>, sqlx::Error>>
    where
        'c: 'e,
        E: 'q + sqlx::Execute<'q, Postgres>,
    {
        let refusal = self.refusal;
        self.conn
            .fetch_optional(query)
            .inspect_err(move |e| note_refusal(refusal, e))
            .boxed()
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [PgTypeInfo],
    ) -> BoxFuture<'e, Result<PgStatement<'q>, sqlx::Error>>
    where
        'c: 'e,
    {
        self.conn.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Postgres>, sqlx::Error>>
    where
        'c: 'e,
    {
        self.conn.describe(sql)
    }
}
