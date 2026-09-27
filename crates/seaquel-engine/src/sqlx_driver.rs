//! The `Driver` implementation shared by the sqlx-based engines.

/// Generates the `Driver` impl shared by the sqlx-based engines (Postgres,
/// MySQL, SQLite). They differ only in the sqlx database type, the arguments
/// type, the decode function, the bind function, and how `last_insert_id` is
/// read from the execute result.
///
/// - `decode_fn`: `fn(<$db as Database>::ValueRef<'_>) -> Result<Value, DbError>`.
///   A decode error gets the column's name appended (the value doesn't know it).
/// - `bind_fn`: `fn<'q>(Query<'q, $db, $args>, &'q Value) -> Result<Query<'q, $db, $args>, DbError>`.
///   Binds one parameter. Each engine owns its binder, so type choices (how a
///   `Decimal` or an `Array` binds) never become per-database branches here.
/// - `introspection` (optional): extra `Driver` methods pasted into the impl,
///   for engines whose introspection has moved to Rust (`list_schemas`, …).
///   Without it the trait's `NOT_SUPPORTED` defaults apply.
/// - `read_only` (optional, after `introspection`): the engine's
///   `query_read_only`, pasted into the impl the same way. Without it the
///   trait's `NOT_SUPPORTED` default applies, so the engine fails closed.
/// - `stream_start` (optional, after `last_insert_id`):
///   `async fn(&sqlx::Pool<$db>, sqlx::pool::PoolConnection<$db>, &str) -> R`
///   (the pool, the connection and the statement's SQL)
///   where `R: DerefMut<Target = PoolConnection<$db>> + RunningStatement`.
///   `query_stream` takes one connection from the pool, hands it to this,
///   and runs the statement on what it returns; `R` stops the statement on
///   the server when it's dropped unfinished. Without it the connection is
///   used as it is.
///
/// [`RunningStatement`]: crate::RunningStatement
///
/// The expansion also defines, next to the impl:
///
/// - `bind_params(query, &values)`: binds every value, non-persistent.
/// - `pub(crate) async fn fetch_capped(executor, sql, &params, cap)`: runs
///   one query on any sqlx executor (the pool, or `&mut *conn` for one
///   connection) and collects its rows under a [`RowCap`]: failing with
///   `RESULT_TOO_LARGE` past it, or stopping after the row past it (or the
///   first row after its byte budget is spent) and returning the rest as
///   `truncated`. `query()` is
///   `fetch_capped(&self.pool, …, RowCap::fail(max_query_rows()))`; a
///   `read_only` implementation calls it on the connection it set up, with
///   `RowCap::read_only(max_rows, max_bytes)`.
///
/// [`RowCap`]: crate::RowCap
///
/// Native only. Expand it in a module that defines `$driver_name` with a
/// `pool: sqlx::Pool<$db>` field. The calling crate must depend on `sqlx`:
/// those paths in the expansion resolve in the caller. Everything else goes
/// through `$crate`, so callers don't need async-trait, async-stream or
/// futures.
#[macro_export]
macro_rules! impl_sqlx_driver {
    (
        $driver_name:ident,
        $db:ty,
        $args:ty,
        decode_fn = $decode_fn:path,
        bind_fn = $bind_fn:path,
        last_insert_id = $last_insert_id:expr
        $(, stream_start = $stream_start:path)?
        $(, introspection = { $($introspection:tt)* })?
        $(, read_only = { $($read_only:tt)* })?
        $(,)?
    ) => {
        /// Binds `values` onto `query`, which every `Driver` method builds
        /// through here. The statement is not persistent: sqlx caches
        /// prepared statements per connection by SQL text alone and reuses
        /// the first prepare's parameter types, so the same SQL sent later
        /// with a `Float` or `Text` where an `Int` was would be sent as the
        /// wrong type (a FLOAT8 1.5 read as INT8 is 4609434218613702656).
        fn bind_params<'q>(
            query: sqlx::query::Query<'q, $db, $args>,
            values: &'q [$crate::Value],
        ) -> Result<sqlx::query::Query<'q, $db, $args>, $crate::DbError> {
            let mut query = query.persistent(false);
            for value in values {
                query = $bind_fn(query, value)?;
            }
            Ok(query)
        }

        /// Runs `sql` with `params` on `executor` and collects the rows
        /// under `cap`: past it, `RESULT_TOO_LARGE` (`RowCap::fail`) or the
        /// rows so far with `truncated` set (`RowCap::truncate`, or the
        /// byte budget: each kept row's decoded size is summed).
        ///
        /// Rows are streamed (rather than `fetch_all`) so it stops as soon
        /// as the row past the cap arrives. Without this, a `SELECT * FROM
        /// big_table` loads everything into RAM before it can be rejected.
        /// On either stop the rest of the result is left unread; the
        /// read-only paths close their connection afterwards.
        pub(crate) async fn fetch_capped<'q, 'c, E>(
            executor: E,
            sql: &'q str,
            params: &'q [$crate::Value],
            cap: $crate::RowCap,
        ) -> Result<$crate::CappedResult, $crate::DbError>
        where
            E: sqlx::Executor<'c, Database = $db>,
        {
            use sqlx::{Column, Row};
            use $crate::__private::futures::StreamExt;

            let query = sqlx::query(sql);
            let query = bind_params(query, params)?;

            let mut stream = query.fetch(executor);

            let mut columns: Vec<String> = Vec::new();
            let mut result_rows: Vec<Vec<$crate::Value>> = Vec::new();
            let mut truncated = false;
            let mut kept_bytes = 0usize;

            while let Some(row_result) = stream.next().await {
                let row = row_result.map_err($crate::DbError::query_error)?;

                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                if !cap.admit(result_rows.len(), kept_bytes)? {
                    truncated = true;
                    break;
                }
                let mut values = Vec::with_capacity(columns.len());
                for i in 0..row.columns().len() {
                    let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                    values.push(
                        $decode_fn(v).map_err(|e| $crate::__private::in_column(e, row.columns()[i].name()))?,
                    );
                }
                if cap.max_bytes().is_some() {
                    kept_bytes = kept_bytes.saturating_add($crate::row_bytes(&values));
                }
                result_rows.push(values);
            }

            Ok($crate::CappedResult {
                columns,
                rows: result_rows,
                truncated,
            })
        }

        #[$crate::__private::async_trait::async_trait]
        impl $crate::Driver for $driver_name {
            async fn query(
                &self,
                sql: &str,
                params: Vec<$crate::Value>,
            ) -> Result<$crate::QueryResult, $crate::DbError> {
                fetch_capped(&self.pool, sql, &params, $crate::RowCap::fail($crate::max_query_rows()))
                    .await
                    .map(Into::into)
            }

            fn query_stream<'a>(
                &'a self,
                sql: String,
                params: Vec<$crate::Value>,
                cancel: $crate::CancellationToken,
            ) -> $crate::BoxStream<'a, Result<$crate::StreamBatch, $crate::DbError>> {
                Box::pin($crate::__private::async_stream::try_stream! {
                    use sqlx::{Column, Row};
                    use $crate::__private::futures::StreamExt;
                    use $crate::RunningStatement as _;

                    const BATCH_SIZE: usize = 5000;

                    let sqlx_query = sqlx::query(&sql);
                    let sqlx_query = bind_params(sqlx_query, &params)?;

                    // One connection for the whole statement, so the
                    // engine's `stream_start` knows which server session
                    // runs it: dropped unfinished (a cancel, a disconnect,
                    // the consumer gone), it stops the statement there.
                    let conn = self
                        .pool
                        .acquire()
                        .await
                        .map_err($crate::DbError::query_error)?;
                    let mut running =
                        ($crate::__sqlx_stream_start!($($stream_start)?))(&self.pool, conn, &sql).await;

                    // `take_until` ends the row stream as soon as `cancel`
                    // fires, even in the middle of a batch.
                    let mut stream = Box::pin(sqlx_query
                        .fetch(&mut **running)
                        .take_until(cancel.cancelled()));

                    let mut buffer: Vec<Vec<$crate::Value>> = Vec::with_capacity(BATCH_SIZE);
                    let mut captured_columns: Option<Vec<String>> = None;
                    let mut first_batch = true;
                    let mut failed: Option<sqlx::Error> = None;

                    while let Some(row_result) = stream.next().await {
                        let row = match row_result {
                            Ok(row) => row,
                            Err(e) => {
                                failed = Some(e);
                                break;
                            }
                        };

                        if captured_columns.is_none() {
                            captured_columns = Some(
                                row.columns().iter().map(|c| c.name().to_string()).collect(),
                            );
                        }
                        let col_count = row.columns().len();
                        let mut values = Vec::with_capacity(col_count);
                        for i in 0..col_count {
                            let v = row.try_get_raw(i).map_err($crate::DbError::query_error)?;
                            values.push(
                                $decode_fn(v).map_err(|e| $crate::__private::in_column(e, row.columns()[i].name()))?,
                            );
                        }
                        buffer.push(values);

                        if buffer.len() >= BATCH_SIZE {
                            let batch_cols = if first_batch { captured_columns.clone() } else { None };
                            first_batch = false;
                            yield $crate::StreamBatch {
                                columns: batch_cols,
                                rows: std::mem::take(&mut buffer),
                                is_final: false,
                                truncated: false,
                            };
                        }
                    }

                    drop(stream);
                    // The statement is over when every row was read (unless
                    // `cancel` ended the rows early) or the server ended it
                    // with an error. Otherwise `running` stops it when it
                    // drops.
                    let over = match &failed {
                        Some(e) => matches!(e, sqlx::Error::Database(_)),
                        None => !cancel.is_cancelled(),
                    };
                    if over {
                        running.finish();
                    }
                    if let Some(e) = failed {
                        Err::<(), _>($crate::DbError::query_error(e))?;
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
                        truncated: false,
                    };
                })
            }

            async fn execute(
                &self,
                sql: &str,
                params: Vec<$crate::Value>,
            ) -> Result<$crate::ExecuteResult, $crate::DbError> {
                use sqlx::Executor;

                let query = sqlx::query(sql);
                let query = bind_params(query, &params)?;

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

                // An error returned from here drops `tx`, which rolls back.
                // A shortfall against `expect_rows` rolls back explicitly, so
                // the connection goes back to the pool clean; if that fails,
                // the shortfall is still what's returned (dropping `tx` tries
                // again).
                for (index, stmt) in statements.iter().enumerate() {
                    let query = sqlx::query(&stmt.sql);
                    let query = bind_params(query, &stmt.params)?;
                    let result = tx
                        .execute(query)
                        .await
                        .map_err($crate::DbError::execute_error)?;
                    if let Err(e) = stmt.check_affected(index, result.rows_affected()) {
                        if let Err(rollback) = tx.rollback().await {
                            $crate::__private::log::warn!(activity = "db.transaction"; "Rollback after {} failed: {rollback}", e.code);
                        }
                        return Err(e);
                    }
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

            $($($introspection)*)?

            $($($read_only)*)?
        }
    };
}

/// `impl_sqlx_driver!`'s `stream_start`, or the plain default.
#[doc(hidden)]
#[macro_export]
macro_rules! __sqlx_stream_start {
    () => {
        $crate::__private::plain_stream_start
    };
    ($path:path) => {
        $path
    };
}
