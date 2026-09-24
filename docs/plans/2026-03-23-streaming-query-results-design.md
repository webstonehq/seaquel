# Streaming Query Results — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Stream query results from Rust to frontend in chunks via Tauri IPC Channel for progressive rendering of large result sets.

**Architecture:** New `db_query_stream` Tauri command uses `ipc::Channel<ChunkMessage>` to send columnar data in fixed-size chunks. Each driver streams rows via sqlx `fetch()` (or equivalent). Frontend accumulates chunks and progressively updates reactive state, with smart single-update behavior for small results.

**Tech Stack:** Tauri 2 `ipc::Channel`, sqlx streaming (`fetch()` + `futures::TryStreamExt`), tiberius `QueryStream`, Svelte 5 runes reactivity.

---

## Design Decisions

- **Chunk strategy:** Fixed row count (5,000 rows per chunk)
- **Frontend accumulation:** Progressive rendering with smart single-update for small results
- **Execution timing:** Measured on Rust side, sent in final `Done` message
- **Streaming scope:** Always use channel path in Rust (one code path)
- **Existing `db_query`:** Kept for backward compat (browser/WASM providers)

---

### Task 1: Add `futures` dependency and `ChunkMessage` + `Driver` trait changes

**Files:**
- Modify: `src-tauri/Cargo.toml:24` (add futures dep)
- Modify: `src-tauri/src/db/mod.rs:1-124` (add ChunkMessage, extend Driver trait)

**Step 1: Add `futures` to Cargo.toml**

In `src-tauri/Cargo.toml`, add after the `async-trait` line:

```toml
futures = "0.3"
```

**Step 2: Add `ChunkMessage` enum to `mod.rs`**

After the `ExecuteResult` struct (line 27), add:

```rust
/// Chunk message for streaming query results over IPC
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChunkMessage {
    Columns { columns: Vec<String> },
    Rows { rows: Vec<Vec<serde_json::Value>> },
    Done { total_rows: u64, execution_time_ms: f64 },
    Error { message: String, code: String },
}
```

**Step 3: Add `query_stream` to `Driver` trait**

In the `Driver` trait, after the `close` method, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        params: Vec<serde_json::Value>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError>;
```

**Step 4: Verify it compiles (expect errors for unimplemented trait methods)**

Run: `cd src-tauri && cargo check 2>&1 | head -5`
Expected: Errors about `query_stream` not implemented on each driver. This confirms the trait change is correct.

---

### Task 2: Implement `query_stream` for SQLite driver

**Files:**
- Modify: `src-tauri/src/db/sqlite.rs`

**Step 1: Add imports**

At the top of `sqlite.rs`, add `futures::TryStreamExt` and `std::time::Instant`:

```rust
use async_trait::async_trait;
use futures::TryStreamExt;
use serde_json::Value as JsonValue;
use sqlx::{migrate::MigrateDatabase, Column, Executor, Pool, Row, Sqlite};
use std::time::Instant;

use super::{ChunkMessage, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};
```

**Step 2: Add `query_stream` implementation**

Inside the `impl Driver for SqliteDriver` block, after the `close` method, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        params: Vec<JsonValue>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError> {
        let start = Instant::now();
        let query = sqlx::query(sql);
        let query = bind_params(query, &params);

        let mut stream = self.pool.fetch(query);
        let mut columns_sent = false;
        let mut chunk: Vec<Vec<JsonValue>> = Vec::with_capacity(chunk_size);
        let mut total_rows: u64 = 0;

        while let Some(row) = stream.try_next().await.map_err(|e| DbError::query_error(e))? {
            if !columns_sent {
                let columns: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
                sender(ChunkMessage::Columns { columns });
                columns_sent = true;
            }

            let mut values = Vec::with_capacity(row.columns().len());
            for i in 0..row.columns().len() {
                let v = row.try_get_raw(i).map_err(|e| DbError::query_error(e))?;
                values.push(super::decode::sqlite::to_json(v)?);
            }
            chunk.push(values);
            total_rows += 1;

            if chunk.len() >= chunk_size {
                sender(ChunkMessage::Rows { rows: std::mem::replace(&mut chunk, Vec::with_capacity(chunk_size)) });
            }
        }

        // Send remaining rows
        if !chunk.is_empty() {
            sender(ChunkMessage::Rows { rows: chunk });
        }

        // If no rows were returned, still send empty columns
        if !columns_sent {
            sender(ChunkMessage::Columns { columns: vec![] });
        }

        let elapsed = start.elapsed();
        sender(ChunkMessage::Done {
            total_rows,
            execution_time_ms: elapsed.as_secs_f64() * 1000.0,
        });

        Ok(())
    }
```

**Step 3: Verify**

Run: `cd src-tauri && cargo check 2>&1 | grep "sqlite" | head -5`
Expected: No errors for sqlite.rs (other drivers still error).

---

### Task 3: Implement `query_stream` for PostgreSQL driver

**Files:**
- Modify: `src-tauri/src/db/postgres.rs`

**Step 1: Add imports**

Update imports at top of `postgres.rs`:

```rust
use async_trait::async_trait;
use futures::TryStreamExt;
use serde_json::Value as JsonValue;
use sqlx::{Column, Executor, Pool, Postgres, Row};
use std::time::Instant;

use super::{ChunkMessage, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};
```

**Step 2: Add `query_stream` implementation**

Inside `impl Driver for PostgresDriver`, after `close`, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        params: Vec<JsonValue>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError> {
        let start = Instant::now();
        let query = sqlx::query(sql);
        let query = bind_params(query, &params);

        let mut stream = self.pool.fetch(query);
        let mut columns_sent = false;
        let mut chunk: Vec<Vec<JsonValue>> = Vec::with_capacity(chunk_size);
        let mut total_rows: u64 = 0;

        while let Some(row) = stream.try_next().await.map_err(|e| DbError::query_error(e))? {
            if !columns_sent {
                let columns: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
                sender(ChunkMessage::Columns { columns });
                columns_sent = true;
            }

            let mut values = Vec::with_capacity(row.columns().len());
            for i in 0..row.columns().len() {
                let v = row.try_get_raw(i).map_err(|e| DbError::query_error(e))?;
                values.push(super::decode::postgres::to_json(v)?);
            }
            chunk.push(values);
            total_rows += 1;

            if chunk.len() >= chunk_size {
                sender(ChunkMessage::Rows { rows: std::mem::replace(&mut chunk, Vec::with_capacity(chunk_size)) });
            }
        }

        if !chunk.is_empty() {
            sender(ChunkMessage::Rows { rows: chunk });
        }

        if !columns_sent {
            sender(ChunkMessage::Columns { columns: vec![] });
        }

        let elapsed = start.elapsed();
        sender(ChunkMessage::Done {
            total_rows,
            execution_time_ms: elapsed.as_secs_f64() * 1000.0,
        });

        Ok(())
    }
```

---

### Task 4: Implement `query_stream` for MySQL driver

**Files:**
- Modify: `src-tauri/src/db/mysql.rs`

**Step 1: Add imports**

Update imports at top of `mysql.rs`:

```rust
use async_trait::async_trait;
use futures::TryStreamExt;
use serde_json::Value as JsonValue;
use sqlx::{Column, Executor, MySql, Pool, Row};
use std::time::Instant;

use super::{ChunkMessage, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};
```

**Step 2: Add `query_stream` implementation**

Inside `impl Driver for MysqlDriver`, after `close`, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        params: Vec<JsonValue>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError> {
        let start = Instant::now();
        let query = sqlx::query(sql);
        let query = bind_params(query, &params);

        let mut stream = self.pool.fetch(query);
        let mut columns_sent = false;
        let mut chunk: Vec<Vec<JsonValue>> = Vec::with_capacity(chunk_size);
        let mut total_rows: u64 = 0;

        while let Some(row) = stream.try_next().await.map_err(|e| DbError::query_error(e))? {
            if !columns_sent {
                let columns: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
                sender(ChunkMessage::Columns { columns });
                columns_sent = true;
            }

            let mut values = Vec::with_capacity(row.columns().len());
            for i in 0..row.columns().len() {
                let v = row.try_get_raw(i).map_err(|e| DbError::query_error(e))?;
                values.push(super::decode::mysql::to_json(v)?);
            }
            chunk.push(values);
            total_rows += 1;

            if chunk.len() >= chunk_size {
                sender(ChunkMessage::Rows { rows: std::mem::replace(&mut chunk, Vec::with_capacity(chunk_size)) });
            }
        }

        if !chunk.is_empty() {
            sender(ChunkMessage::Rows { rows: chunk });
        }

        if !columns_sent {
            sender(ChunkMessage::Columns { columns: vec![] });
        }

        let elapsed = start.elapsed();
        sender(ChunkMessage::Done {
            total_rows,
            execution_time_ms: elapsed.as_secs_f64() * 1000.0,
        });

        Ok(())
    }
```

---

### Task 5: Implement `query_stream` for MSSQL driver

**Files:**
- Modify: `src-tauri/src/db/mssql.rs`

**Step 1: Add imports**

Add `std::time::Instant` and `futures::TryStreamExt` to the imports, and `ChunkMessage` to the super import:

```rust
use async_native_tls::TlsStream;
use async_trait::async_trait;
use futures::TryStreamExt;
use log::{error, info, trace};
use tiberius::{AuthMethod, Client, Config, Query, QueryItem, Row};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};
use std::time::Instant;

use super::{ChunkMessage, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};
```

**Step 2: Add streaming helper to `MssqlClient`**

Add a new method to the `MssqlClient` enum for streaming (after `execute_sql`):

```rust
    async fn query_stream_rows(
        &mut self,
        sql: &str,
        chunk_size: usize,
        sender: &dyn Fn(ChunkMessage),
    ) -> Result<u64, tiberius::error::Error> {
        let query = Query::new(sql);
        let mut columns_sent = false;
        let mut chunk: Vec<Vec<serde_json::Value>> = Vec::with_capacity(chunk_size);
        let mut total_rows: u64 = 0;

        let mut stream = match self {
            MssqlClient::Tls(client) => query.query(client).await?,
            MssqlClient::Plain(client) => query.query(client).await?,
        };

        while let Some(item) = stream.try_next().await? {
            match item {
                QueryItem::Metadata(meta) => {
                    if !columns_sent {
                        let columns: Vec<String> = meta.columns().iter().map(|c| c.name().to_string()).collect();
                        sender(ChunkMessage::Columns { columns });
                        columns_sent = true;
                    }
                }
                QueryItem::Row(row) => {
                    chunk.push(row_to_values(&row));
                    total_rows += 1;

                    if chunk.len() >= chunk_size {
                        sender(ChunkMessage::Rows {
                            rows: std::mem::replace(&mut chunk, Vec::with_capacity(chunk_size)),
                        });
                    }
                }
            }
        }

        if !chunk.is_empty() {
            sender(ChunkMessage::Rows { rows: chunk });
        }

        if !columns_sent {
            sender(ChunkMessage::Columns { columns: vec![] });
        }

        Ok(total_rows)
    }
```

**Step 3: Add `query_stream` to `impl Driver for MssqlDriver`**

After `close`, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        _params: Vec<serde_json::Value>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError> {
        let start = Instant::now();
        let mut client = self.client.lock().await;

        let total_rows = client.query_stream_rows(sql, chunk_size, &*sender).await.map_err(|e| {
            error!("MSSQL stream error");
            trace!("MSSQL stream error: {}", e);
            DbError::query_error(e)
        })?;

        let elapsed = start.elapsed();
        sender(ChunkMessage::Done {
            total_rows,
            execution_time_ms: elapsed.as_secs_f64() * 1000.0,
        });

        Ok(())
    }
```

**Note:** The MSSQL driver imports need `QueryItem` added to the tiberius import. The `Row` import changes from a direct import to being accessed via `QueryItem::Row`.

---

### Task 6: Implement `query_stream` for DuckDB driver

**Files:**
- Modify: `src-tauri/src/db/duckdb.rs`

**Step 1: Add imports**

Add `std::time::Instant` and `ChunkMessage` to imports:

```rust
use async_trait::async_trait;
use duckdb::{types::ValueRef, Connection};
use std::sync::Mutex;
use std::time::Instant;

use super::{ChunkMessage, ConnectConfig, DbError, Driver, ExecuteResult, QueryResult};
```

**Step 2: Add `query_stream` to `impl Driver for DuckdbDriver`**

After `close`, add:

```rust
    async fn query_stream(
        &self,
        sql: &str,
        _params: Vec<serde_json::Value>,
        chunk_size: usize,
        sender: Box<dyn Fn(ChunkMessage) + Send>,
    ) -> Result<(), DbError> {
        let start = Instant::now();
        let sql = sql.to_string();

        let conn = self.connection.lock().map_err(|e| DbError {
            message: format!("Failed to lock connection: {}", e),
            code: "LOCK_ERROR".to_string(),
        })?;

        let mut stmt = conn.prepare(&sql).map_err(|e| DbError::query_error(e))?;
        let mut result_rows = stmt.query([]).map_err(|e| DbError::query_error(e))?;

        let column_count = result_rows
            .as_ref()
            .map(|s| s.column_count())
            .unwrap_or(0);
        let columns: Vec<String> = (0..column_count)
            .map(|i| {
                result_rows
                    .as_ref()
                    .and_then(|s| s.column_name(i).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();

        sender(ChunkMessage::Columns { columns });

        let mut chunk: Vec<Vec<serde_json::Value>> = Vec::with_capacity(chunk_size);
        let mut total_rows: u64 = 0;

        while let Some(row) = result_rows.next().map_err(|e| DbError::query_error(e))? {
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value = row.get_ref(i).map_err(|e| DbError::query_error(e))?;
                values.push(convert_value_to_json(value));
            }
            chunk.push(values);
            total_rows += 1;

            if chunk.len() >= chunk_size {
                sender(ChunkMessage::Rows {
                    rows: std::mem::replace(&mut chunk, Vec::with_capacity(chunk_size)),
                });
            }
        }

        if !chunk.is_empty() {
            sender(ChunkMessage::Rows { rows: chunk });
        }

        let elapsed = start.elapsed();
        sender(ChunkMessage::Done {
            total_rows,
            execution_time_ms: elapsed.as_secs_f64() * 1000.0,
        });

        Ok(())
    }
```

---

### Task 7: Add `db_query_stream` command and register it

**Files:**
- Modify: `src-tauri/src/db/commands.rs`
- Modify: `src-tauri/src/lib.rs:410-414`

**Step 1: Add the streaming command to `commands.rs`**

Add `tauri::ipc::Channel` to imports and add the new command after `db_execute`:

```rust
use log::{debug, info};
use tauri::{command, ipc::Channel, State};
```

After the `db_execute` function, add:

```rust
const STREAM_CHUNK_SIZE: usize = 5000;

#[command]
pub async fn db_query_stream(
    connection_id: String,
    sql: String,
    values: Vec<serde_json::Value>,
    channel: Channel<super::ChunkMessage>,
    manager: State<'_, ConnectionManager>,
) -> Result<(), DbError> {
    debug!("db_query_stream on {}", connection_id);
    let connections = manager.connections.read().await;
    let driver = connections
        .get(&connection_id)
        .ok_or_else(|| DbError::connection_not_found(&connection_id))?;

    let sender = Box::new(move |msg: super::ChunkMessage| {
        let _ = channel.send(msg);
    });

    driver.query_stream(&sql, values, STREAM_CHUNK_SIZE, sender).await
}
```

**Step 2: Register in `lib.rs`**

In `src-tauri/src/lib.rs`, add `db_query_stream` to the invoke handler after `db_query`:

```rust
            db::commands::db_connect,
            db::commands::db_query,
            db::commands::db_query_stream,
            db::commands::db_execute,
```

**Step 3: Verify Rust compiles**

Run: `cd src-tauri && cargo check`
Expected: Clean compilation with no errors.

---

### Task 8: Frontend — Add `ChunkMessage` type and `selectStream` to provider

**Files:**
- Modify: `src/lib/providers/types.ts`
- Modify: `src/lib/providers/unified-tauri-provider.ts`

**Step 1: Add `ChunkMessage` type and `selectStream` to `DatabaseProvider` interface**

In `src/lib/providers/types.ts`, after the `ExecuteResult` interface, add:

```typescript
/**
 * Chunk message from streaming query results.
 */
export type ChunkMessage =
  | { type: "columns"; columns: string[] }
  | { type: "rows"; rows: unknown[][] }
  | { type: "done"; total_rows: number; execution_time_ms: number }
  | { type: "error"; message: string; code: string };
```

In the `DatabaseProvider` interface, after the `test` method, add:

```typescript
  /**
   * Execute a SELECT query with streaming results via IPC channel.
   * Only available in Tauri providers. Falls back to select() if not implemented.
   */
  selectStream?(
    connectionId: string,
    sql: string,
    params?: unknown[],
    onChunk?: (msg: ChunkMessage) => void,
  ): Promise<void>;
```

**Step 2: Add `selectStream` to `UnifiedTauriProvider`**

In `src/lib/providers/unified-tauri-provider.ts`, add `Channel` to the import:

```typescript
import { invoke, Channel } from "@tauri-apps/api/core";
```

Add the `ChunkMessage` import:

```typescript
import type { DatabaseProvider, ConnectionConfig, ExecuteResult, ChunkMessage } from "./types";
```

After the `test` method, add:

```typescript
  async selectStream(
    connectionId: string,
    sql: string,
    params?: unknown[],
    onChunk?: (msg: ChunkMessage) => void,
  ): Promise<void> {
    try {
      const channel = new Channel<ChunkMessage>();
      if (onChunk) {
        channel.onmessage = onChunk;
      }

      await invoke("db_query_stream", {
        connectionId,
        sql,
        values: params ?? [],
        channel,
      });
    } catch (error) {
      throw formatError(error);
    }
  }
```

---

### Task 9: Frontend — Integrate streaming into query execution

**Files:**
- Modify: `src/lib/hooks/database/query-execution.svelte.ts`

**Step 1: Add `ChunkMessage` import**

Add to the imports at the top:

```typescript
import type { ChunkMessage } from "$lib/providers/types";
```

**Step 2: Replace the SELECT query path in `executeStatement`**

In the `executeStatement` method, replace the current SELECT execution block (the `provider.select` call and result conversion at lines 218-223):

```typescript
    const dbResult = await provider.select<Record<string, unknown>>(
      providerConnectionId,
      paginatedQuery,
      bindValues,
    );
    const resultColumns = (dbResult?.length ?? 0) > 0 ? Object.keys(dbResult[0]) : [];
    const totalMs = performance.now() - start;
```

With the streaming version:

```typescript
    let resultColumns: string[] = [];
    let dbResult: Record<string, unknown>[] = [];
    let streamTotalRows = 0;
    let streamExecutionTimeMs = 0;

    if (provider.selectStream) {
      let chunkCount = 0;

      await provider.selectStream(providerConnectionId, paginatedQuery, bindValues, (msg: ChunkMessage) => {
        switch (msg.type) {
          case "columns":
            resultColumns = msg.columns;
            break;
          case "rows": {
            const newRows = msg.rows.map((row) => {
              const obj: Record<string, unknown> = {};
              resultColumns.forEach((col, i) => {
                obj[col] = row[i];
              });
              return obj;
            });
            dbResult.push(...newRows);
            chunkCount++;
            if (chunkCount > 1) {
              onProgress?.({ columns: resultColumns, rows: [...dbResult] });
            }
            break;
          }
          case "done":
            streamTotalRows = msg.total_rows;
            streamExecutionTimeMs = msg.execution_time_ms;
            break;
          case "error":
            throw new Error(`${msg.code}: ${msg.message}`);
        }
      });
    } else {
      const rows = await provider.select<Record<string, unknown>>(
        providerConnectionId,
        paginatedQuery,
        bindValues,
      );
      dbResult = rows;
      resultColumns = (rows?.length ?? 0) > 0 ? Object.keys(rows[0]) : [];
    }

    const totalMs = provider.selectStream
      ? streamExecutionTimeMs
      : performance.now() - start;
```

**Step 3: Add `onProgress` parameter to `executeStatement`**

Update the `executeStatement` signature to accept an optional progress callback:

```typescript
  private async executeStatement(
    sql: string,
    page: number,
    pageSize: number,
    connection: DatabaseConnection,
    bindValues?: unknown[],
    onProgress?: (partial: { columns: string[]; rows: Record<string, unknown>[] }) => void,
  ): Promise<QueryResult> {
```

**Step 4: Update `totalRows` usage after the streaming block**

After the streaming/fallback block, update the line that uses `totalRows` (line 227-229):

Replace:

```typescript
    // If count failed or query had LIMIT, use result length as total
    if (totalRows < 0) {
      totalRows = dbResult?.length ?? 0;
    }
```

With:

```typescript
    // If count failed or query had LIMIT, use result length or stream total
    if (totalRows < 0) {
      totalRows = provider.selectStream ? streamTotalRows || dbResult.length : dbResult?.length ?? 0;
    }
```

**Step 5: Wire `onProgress` in callers**

In `executeCurrent`, pass an `onProgress` callback to `executeStatement` that calls `updateQueryTabState`:

In the `executeCurrent` method, change the `executeStatement` call to:

```typescript
      const result = await this.executeStatement(
        statement.sql,
        page,
        effectivePageSize,
        connection,
        undefined,
        (partial) => {
          const progressResults: StatementResult[] = [
            {
              columns: partial.columns,
              rows: partial.rows,
              rowCount: partial.rows.length,
              totalRows: partial.rows.length,
              executionTime: 0,
              page,
              pageSize: effectivePageSize,
              totalPages: 1,
              statementIndex: 0,
              statementSql: statement.sql,
              isError: false,
              isStreaming: true,
            },
          ];
          this.updateQueryTabState(tabId, {
            results: progressResults,
            activeResultIndex: 0,
          });
        },
      );
```

Do the same for the `execute` method's `executeStatement` call (inside the for loop), and the `executeCurrentWithParams` / `executeWithParams` methods.

**Step 6: Verify**

Run: `npm run check`
Expected: Clean type checking with no errors.

---

### Task 10: Manual testing

**Step 1: Start dev server**

Run: `npm run tauri dev`

**Step 2: Test with small result**

Connect to the SQLite test database. Run: `SELECT * FROM main.orders LIMIT 10;`
Expected: Results appear instantly, identical behavior to before.

**Step 3: Test with large result**

Run: `SELECT * FROM main.orders LIMIT 1000000;`
Expected: First rows appear within ~100ms. Table progressively fills. Total time should be significantly less than 4 seconds.

**Step 4: Test empty result**

Run: `SELECT * FROM main.orders WHERE 1=0;`
Expected: Empty result, no errors.

**Step 5: Test error handling**

Run: `SELECT * FROM nonexistent_table;`
Expected: Error message displayed correctly.
