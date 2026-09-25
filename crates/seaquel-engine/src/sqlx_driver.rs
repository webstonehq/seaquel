//! The `Driver` implementation shared by the sqlx-based engines.

/// Generates the `Driver` impl shared by the sqlx-based engines (Postgres,
/// MySQL, SQLite). They differ only in the sqlx database type, the arguments
/// type, the decode function, and how `last_insert_id` is read from the
/// execute result.
///
/// Native only. Expand it in a module that defines `$driver_name` with a
/// `pool: sqlx::Pool<$db>` field. The calling crate must depend on `sqlx` and
/// `serde_json`: those paths in the expansion resolve in the caller. Everything
/// else goes through `$crate`, so callers don't need async-trait, async-stream
/// or futures.
#[macro_export]
macro_rules! impl_sqlx_driver {
    (
        $driver_name:ident,
        $db:ty,
        $args:ty,
        decode_fn = $decode_fn:path,
        last_insert_id = $last_insert_id:expr
    ) => {
        fn bind_params<'q>(
            mut query: sqlx::query::Query<'q, $db, $args>,
            values: &'q [serde_json::Value],
        ) -> sqlx::query::Query<'q, $db, $args> {
            use serde_json::Value as JsonValue;
            for value in values {
                if value.is_null() {
                    query = query.bind(None::<JsonValue>);
                } else if let Some(b) = value.as_bool() {
                    // Bind JS booleans as native `bool`. sqlx encodes this as
                    // BOOLEAN for Postgres and as TINYINT 0/1 for MySQL/SQLite.
                    // Without this branch the fallback serializes to JSON text
                    // ("true"/"false") and MySQL rejects it for tinyint columns.
                    query = query.bind(b);
                } else if value.is_string() {
                    query = query.bind(value.as_str().unwrap().to_owned());
                } else if let Some(number) = value.as_number() {
                    query = query.bind(number.as_f64().unwrap_or_default());
                } else {
                    query = query.bind(value.clone());
                }
            }
            query
        }

        #[$crate::__private::async_trait::async_trait]
        impl $crate::Driver for $driver_name {
            async fn query(
                &self,
                sql: &str,
                params: Vec<serde_json::Value>,
            ) -> Result<$crate::QueryResult, $crate::DbError> {
                use sqlx::{Column, Row};
                use $crate::__private::futures::StreamExt;

                let query = sqlx::query(sql);
                let query = bind_params(query, &params);

                // Stream rows (rather than fetch_all) so we can bail out as
                // soon as the per-query cap is hit. Without this, a `SELECT *
                // FROM big_table` loads everything into RAM before we can
                // reject it.
                let cap = $crate::max_query_rows();
                let mut stream = query.fetch(&self.pool);

                let mut columns: Vec<String> = Vec::new();
                let mut result_rows: Vec<Vec<serde_json::Value>> = Vec::new();

                while let Some(row_result) = stream.next().await {
                    let row = row_result.map_err($crate::DbError::query_error)?;

                    if columns.is_empty() {
                        columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                    }
                    if result_rows.len() >= cap {
                        return Err($crate::DbError::result_too_large(cap));
                    }
                    let mut values = Vec::with_capacity(columns.len());
                    for i in 0..row.columns().len() {
                        let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                        values.push($decode_fn(v)?);
                    }
                    result_rows.push(values);
                }

                Ok($crate::QueryResult {
                    columns,
                    rows: result_rows,
                })
            }

            fn query_stream<'a>(
                &'a self,
                sql: String,
                params: Vec<serde_json::Value>,
                cancel: $crate::CancellationToken,
            ) -> $crate::BoxStream<'a, Result<$crate::StreamBatch, $crate::DbError>> {
                Box::pin($crate::__private::async_stream::try_stream! {
                    use sqlx::{Column, Row};
                    use $crate::__private::futures::StreamExt;

                    const BATCH_SIZE: usize = 5000;

                    let sqlx_query = sqlx::query(&sql);
                    let sqlx_query = bind_params(sqlx_query, &params);

                    // `take_until` ends the row stream as soon as `cancel`
                    // fires, even in the middle of a batch. Dropping the fetch
                    // releases the pooled connection.
                    let mut stream = std::pin::pin!(sqlx_query
                        .fetch(&self.pool)
                        .take_until(cancel.cancelled()));

                    let mut buffer: Vec<Vec<serde_json::Value>> = Vec::with_capacity(BATCH_SIZE);
                    let mut captured_columns: Option<Vec<String>> = None;
                    let mut first_batch = true;

                    while let Some(row_result) = stream.next().await {
                        let row = row_result.map_err($crate::DbError::query_error)?;

                        if captured_columns.is_none() {
                            captured_columns = Some(
                                row.columns().iter().map(|c| c.name().to_string()).collect(),
                            );
                        }
                        let col_count = row.columns().len();
                        let mut values = Vec::with_capacity(col_count);
                        for i in 0..col_count {
                            let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                            values.push($decode_fn(v)?);
                        }
                        buffer.push(values);

                        if buffer.len() >= BATCH_SIZE {
                            let batch_cols = if first_batch { captured_columns.clone() } else { None };
                            first_batch = false;
                            yield $crate::StreamBatch {
                                columns: batch_cols,
                                rows: std::mem::take(&mut buffer),
                                is_final: false,
                            };
                        }
                    }

                    // Terminal batch — empty buffer is fine, but still needs to carry
                    // the columns if no row ever arrived (empty result set).
                    let final_cols = if first_batch {
                        Some(captured_columns.unwrap_or_default())
                    } else {
                        None
                    };
                    yield $crate::StreamBatch {
                        columns: final_cols,
                        rows: buffer,
                        is_final: true,
                    };
                })
            }

            async fn execute(
                &self,
                sql: &str,
                params: Vec<serde_json::Value>,
            ) -> Result<$crate::ExecuteResult, $crate::DbError> {
                use sqlx::Executor;

                let query = sqlx::query(sql);
                let query = bind_params(query, &params);

                let result = self
                    .pool
                    .execute(query)
                    .await
                    .map_err($crate::DbError::execute_error)?;

                Ok($crate::ExecuteResult {
                    rows_affected: result.rows_affected(),
                    last_insert_id: ($last_insert_id)(&result),
                })
            }

            async fn transaction(&self, statements: Vec<$crate::BatchStatement>) -> Result<(), $crate::DbError> {
                use sqlx::{Acquire, Executor};

                let mut conn = self
                    .pool
                    .acquire()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                let mut tx = conn
                    .begin()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                for stmt in &statements {
                    let query = sqlx::query(&stmt.sql);
                    let query = bind_params(query, &stmt.params);
                    tx.execute(query)
                        .await
                        .map_err($crate::DbError::execute_error)?;
                }

                tx.commit()
                    .await
                    .map_err($crate::DbError::execute_error)?;

                Ok(())
            }

            async fn close(&self) -> Result<(), $crate::DbError> {
                self.pool.close().await;
                Ok(())
            }
        }
    };
}
