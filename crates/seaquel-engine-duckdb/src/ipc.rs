//! DuckDB results as Arrow IPC bytes, read into rows by the shared decoder
//! ([`crate::decode`]): DuckDB-WASM's for the browser driver, and the DuckDB
//! helper's for its client (the `remote` feature).
//!
//! DuckDB-WASM hands out two formats: `runQuery` returns a whole result in
//! IPC **file** format (`ARROW1…`), and a pending query returns its schema
//! message first and then one chunk of **stream** messages per fetch. Both
//! are read by `arrow-ipc` with validation on. The browser driver uses only
//! the stream form; the helper sends stream messages too.

#![cfg_attr(
    not(all(feature = "browser", target_arch = "wasm32")),
    allow(dead_code)
)]

use std::sync::Arc;

use arrow_array::{Array, RecordBatch};
use arrow_buffer::Buffer;
use arrow_ipc::reader::FileReader;
use arrow_ipc::reader::{StreamDecoder, StreamReader};
use arrow_schema::Schema;
#[cfg(test)]
use seaquel_engine::CappedResult;
use seaquel_engine::{DbError, RowCap, Value};

use crate::decode::{self, Kind};

/// How a column's DuckDB type is told from its Arrow field.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct KindRules {
    /// DuckDB-WASM 1.4.3 sends HUGEINT as a plain `Decimal128(38, 0)`, with
    /// no extension metadata, whatever `arrow_lossless_conversion` says. With
    /// this set, such a column decodes as HUGEINT (an `Int` when it fits),
    /// so `sum(…)` and `count(…)`-style results read as on desktop; a real
    /// `DECIMAL(38, 0)` then does too (a documented difference).
    pub decimal38_is_hugeint: bool,
}

/// The magic that starts (and ends) an IPC file.
#[cfg(test)]
const FILE_MAGIC: &[u8] = b"ARROW1";

/// A result's column names and kinds, from its schema.
pub(crate) struct Columns {
    pub names: Vec<String>,
    kinds: Vec<Kind>,
}

impl Columns {
    pub(crate) fn of(schema: &Schema, rules: KindRules) -> Self {
        Columns {
            names: schema.fields().iter().map(|f| f.name().clone()).collect(),
            kinds: schema
                .fields()
                .iter()
                .map(|f| Kind::of_field(f, rules.decimal38_is_hugeint))
                .collect(),
        }
    }

    /// The columns with kinds given by the DuckDB helper (from DuckDB's
    /// logical types), one per field: the client decodes by them instead
    /// of guessing from the fields. Kinds that don't match the fields are
    /// `HELPER_PROTOCOL`.
    #[cfg(any(feature = "remote", test))]
    pub(crate) fn with_kinds(schema: &Schema, kinds: Vec<Kind>) -> Result<Self, DbError> {
        if kinds.len() != schema.fields().len() {
            return Err(crate::wire::protocol_error(format!(
                "{} column kinds for {} columns",
                kinds.len(),
                schema.fields().len()
            )));
        }
        Ok(Columns {
            names: schema.fields().iter().map(|f| f.name().clone()).collect(),
            kinds,
        })
    }

    /// Row `row` of `batch`, decoded. An Arrow type the decoder doesn't read
    /// is `UNSUPPORTED_TYPE` naming the column, as in the native driver.
    pub(crate) fn row(&self, batch: &RecordBatch, row: usize) -> Result<Vec<Value>, DbError> {
        let mut out = Vec::with_capacity(self.kinds.len());
        for (i, kind) in self.kinds.iter().enumerate() {
            let (Some(column), Some(name)) = (batch.columns().get(i), self.names.get(i)) else {
                return Err(DbError::query_error(
                    "DuckDB sent a batch that doesn't match its schema",
                ));
            };
            out.push(decode::decode(kind, column.as_ref(), row).map_err(|problem| {
                DbError {
                    message: format!(
                        "Column \"{name}\" has a type Seaquel can't read yet (Arrow {}): {problem}. \
                         Cast it in the query, e.g. to VARCHAR.",
                        column.data_type()
                    ),
                    code: "UNSUPPORTED_TYPE".to_string(),
                }
            })?);
        }
        Ok(out)
    }

    /// Adds `batch`'s rows to `rows` under `cap`, counting the kept bytes
    /// when the cap has a byte budget. `false` once the cap stopped it
    /// (`truncated`); past a failing cap, `RESULT_TOO_LARGE`.
    pub(crate) fn collect(
        &self,
        batch: &RecordBatch,
        cap: RowCap,
        rows: &mut Vec<Vec<Value>>,
        kept_bytes: &mut usize,
    ) -> Result<bool, DbError> {
        for row in 0..batch.num_rows() {
            if !cap.admit(rows.len(), *kept_bytes)? {
                return Ok(false);
            }
            let row = self.row(batch, row)?;
            if cap.max_bytes().is_some() {
                *kept_bytes = kept_bytes.saturating_add(seaquel_engine::row_bytes(&row));
            }
            rows.push(row);
        }
        Ok(true)
    }
}

fn ipc_error(e: impl std::fmt::Display) -> DbError {
    DbError::query_error(format!("Couldn't read DuckDB's result: {e}"))
}

/// An IPC stream read as it arrives: the schema message first (a pending
/// query's header), then each fetched chunk. Chunks may split messages
/// anywhere.
pub(crate) struct IpcStream {
    decoder: StreamDecoder,
    columns: Arc<Columns>,
    /// The first column holding a dictionary (an ENUM), if any. DuckDB-WASM
    /// 1.32's pending results carry no dictionary batch (its `runQuery`
    /// files do), so such a column can't be read from a stream.
    dictionary_column: Option<String>,
}

/// The IPC end-of-stream marker: a continuation and a zero length.
const END_OF_STREAM: [u8; 8] = [0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0];

impl IpcStream {
    /// Reads the schema message at the start of `header` (and any batches
    /// after it, returned).
    pub(crate) fn start(
        header: Vec<u8>,
        rules: KindRules,
    ) -> Result<(Self, Vec<RecordBatch>), DbError> {
        Self::start_from(header, |schema| Ok(Columns::of(schema, rules)))
    }

    /// [`IpcStream::start`] with the columns' kinds given (the DuckDB
    /// helper's, from DuckDB's logical types: [`Columns::with_kinds`]).
    #[cfg(any(feature = "remote", test))]
    pub(crate) fn start_with_kinds(
        header: Vec<u8>,
        kinds: Vec<Kind>,
    ) -> Result<(Self, Vec<RecordBatch>), DbError> {
        Self::start_from(header, |schema| Columns::with_kinds(schema, kinds))
    }

    fn start_from(
        header: Vec<u8>,
        columns: impl FnOnce(&Schema) -> Result<Columns, DbError>,
    ) -> Result<(Self, Vec<RecordBatch>), DbError> {
        // `StreamDecoder` holds a message back until bytes after it arrive
        // (a schema message has an empty body), so the schema is read here
        // on its own, and the decoder gets the same bytes.
        let schema = StreamReader::try_new(std::io::Cursor::new(&header[..]), None)
            .map_err(ipc_error)?
            .schema();
        let dictionary_column = schema
            .fields()
            .iter()
            .find(|f| has_dictionary(f.data_type()))
            .map(|f| f.name().clone());
        let mut stream = IpcStream {
            decoder: StreamDecoder::new(),
            columns: Arc::new(columns(&schema)?),
            dictionary_column,
        };
        let batches = stream.push(header)?;
        Ok((stream, batches))
    }

    pub(crate) fn columns(&self) -> Arc<Columns> {
        self.columns.clone()
    }

    /// The first column holding a dictionary (an ENUM), if any.
    pub(crate) fn dictionary_column(&self) -> Option<&str> {
        self.dictionary_column.as_deref()
    }

    /// Feeds `bytes`, returning the batches they complete.
    pub(crate) fn push(&mut self, bytes: Vec<u8>) -> Result<Vec<RecordBatch>, DbError> {
        let mut buffer = Buffer::from(bytes);
        let mut batches = Vec::new();
        while !buffer.is_empty() {
            match self.decoder.decode(&mut buffer) {
                Ok(Some(batch)) => batches.push(batch),
                Ok(None) => {}
                Err(e) => return Err(self.error(e)),
            }
        }
        Ok(batches)
    }

    /// The input ended (DuckDB-WASM sends no end marker): the message held
    /// back, if any, is read with the marker after it, and anything left
    /// half-read is an error.
    pub(crate) fn finish(&mut self) -> Result<Vec<RecordBatch>, DbError> {
        if self.decoder.finish().is_ok() {
            return Ok(Vec::new());
        }
        let batches = self.push(END_OF_STREAM.to_vec())?;
        self.decoder.finish().map_err(|e| self.error(e))?;
        Ok(batches)
    }

    /// A batch that couldn't be read. With a dictionary column, that's the
    /// missing dictionary: `UNSUPPORTED_TYPE` naming the column.
    fn error(&self, e: arrow_schema::ArrowError) -> DbError {
        match &self.dictionary_column {
            Some(name) => DbError {
                message: format!(
                    "Column \"{name}\" is an ENUM, which DuckDB in the browser can't send yet. \
                     Cast it in the query, e.g. to VARCHAR. ({e})"
                ),
                code: "UNSUPPORTED_TYPE".to_string(),
            },
            None => ipc_error(e),
        }
    }
}

/// Whether `t` is or holds a dictionary.
fn has_dictionary(t: &arrow_schema::DataType) -> bool {
    use arrow_schema::DataType as T;
    match t {
        T::Dictionary(..) => true,
        T::List(f)
        | T::LargeList(f)
        | T::FixedSizeList(f, _)
        | T::ListView(f)
        | T::LargeListView(f)
        | T::Map(f, _) => has_dictionary(f.data_type()),
        T::Struct(fields) => fields.iter().any(|f| has_dictionary(f.data_type())),
        T::Union(fields, _) => fields.iter().any(|(_, f)| has_dictionary(f.data_type())),
        _ => false,
    }
}

/// A whole result in IPC file format (`runQuery`'s): its columns and
/// batches. Dictionaries (ENUM labels) come with it.
pub(crate) fn read_file(
    ipc: &[u8],
    rules: KindRules,
) -> Result<(Arc<Columns>, Vec<RecordBatch>), DbError> {
    let reader = FileReader::try_new(std::io::Cursor::new(ipc), None).map_err(ipc_error)?;
    let columns = Arc::new(Columns::of(&reader.schema(), rules));
    let batches = reader.collect::<Result<Vec<_>, _>>().map_err(ipc_error)?;
    Ok((columns, batches))
}

/// Every row of an IPC result in either format, under `cap`. The driver
/// reads pending queries chunk by chunk ([`IpcStream`]); this whole-result
/// form (`runQuery`'s file format included) is for the tests.
#[cfg(test)]
pub(crate) fn decode_batches(
    ipc: &[u8],
    cap: RowCap,
    rules: KindRules,
) -> Result<CappedResult, DbError> {
    let mut rows = Vec::new();
    let mut kept_bytes = 0;
    let mut truncated = false;
    if ipc.starts_with(FILE_MAGIC) {
        let (columns, batches) = read_file(ipc, rules)?;
        for batch in &batches {
            if !columns.collect(batch, cap, &mut rows, &mut kept_bytes)? {
                truncated = true;
                break;
            }
        }
        return Ok(CappedResult {
            columns: columns.names.clone(),
            rows,
            truncated,
        });
    }
    let (mut stream, mut batches) = IpcStream::start(ipc.to_vec(), rules)?;
    batches.extend(stream.finish()?);
    let columns = stream.columns();
    for batch in &batches {
        if !columns.collect(batch, cap, &mut rows, &mut kept_bytes)? {
            truncated = true;
            break;
        }
    }
    Ok(CappedResult {
        columns: columns.names.clone(),
        rows,
        truncated,
    })
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use super::*;
    use seaquel_engine::Value;
    use seaquel_engine_testkit::same_value;

    use crate::test_cells as cells;

    fn connection() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        // As the native driver opens it.
        conn.execute_batch("SET arrow_lossless_conversion = true")
            .unwrap();
        conn
    }

    /// `sql`'s result as DuckDB's own Arrow, written as IPC in `file` or
    /// stream format.
    fn ipc_of(conn: &duckdb::Connection, sql: &str, file: bool) -> Vec<u8> {
        let mut stmt = conn.prepare(sql).unwrap();
        let arrow = stmt.query_arrow([]).unwrap();
        let schema = arrow.get_schema();
        let batches: Vec<_> = arrow.collect();
        let mut out = Vec::new();
        if file {
            let mut w = arrow_ipc::writer::FileWriter::try_new(&mut out, &schema).unwrap();
            for b in &batches {
                w.write(b).unwrap();
            }
            w.finish().unwrap();
        } else {
            let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut out, &schema).unwrap();
            for b in &batches {
                w.write(b).unwrap();
            }
            w.finish().unwrap();
        }
        out
    }

    fn same_rows(a: &[Vec<Value>], b: &[Vec<Value>]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q)))
    }

    /// Native DuckDB's Arrow, written as IPC and read back the way the
    /// browser driver reads DuckDB-WASM's, decodes to the same cells as the
    /// native driver: every typed-cell case (`tests/common/cells.rs`), in
    /// both formats, plus results that span several chunks, an empty result
    /// (its columns still named) and a dictionary column.
    #[test]
    fn decode_from_ipc_matches_decode_from_duckdb() {
        let conn = connection();
        let mut selects: Vec<(Vec<String>, String, Vec<String>)> = cells::cases()
            .into_iter()
            .map(|c| (c.setup, c.select, c.teardown))
            .collect();
        for sql in [
            "SELECT range AS i, 'x' || range AS s, range::DOUBLE / 7 AS f FROM range(5000)",
            "SELECT 1 AS a, 'b' AS b WHERE false",
            "SELECT ['sad', 'ok'][1 + (range % 2)]::ENUM('sad', 'ok') AS e FROM range(3000)",
            "SELECT '99999999999999999999999999999999999999'::DECIMAL(38,0) AS d, NULL AS n",
        ] {
            selects.push((vec![], sql.to_string(), vec![]));
        }
        let mut failures = Vec::new();
        for (setup, select, teardown) in &selects {
            for sql in setup {
                conn.execute_batch(sql).unwrap();
            }
            let native = crate::driver::native_rows(&conn, select).unwrap();
            for file in [true, false] {
                let ipc = ipc_of(&conn, select, file);
                let cap = RowCap::fail(seaquel_engine::max_query_rows());
                match decode_batches(&ipc, cap, KindRules::default()) {
                    Ok(r) if r.columns == native.columns && same_rows(&r.rows, &native.rows) => {}
                    Ok(r) => failures.push(format!(
                        "{select} (file: {file}):\n  native: {:?} {:?}\n  ipc:    {:?} {:?}",
                        native.columns,
                        native.rows.iter().take(2).collect::<Vec<_>>(),
                        r.columns,
                        r.rows.iter().take(2).collect::<Vec<_>>()
                    )),
                    Err(e) => failures.push(format!("{select} (file: {file}): {e:?}")),
                }
            }
            for sql in teardown {
                conn.execute_batch(sql).unwrap();
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} differ:\n{}",
            failures.len(),
            selects.len() * 2,
            failures.join("\n")
        );
    }

    /// The DuckDB helper's client reads the kinds the helper sent (from
    /// DuckDB's logical types), not the fields: with
    /// `arrow_lossless_conversion` reset, UHUGEINT is a bare
    /// `Decimal128(38, 0)` and BIT a plain binary, which the fields read as
    /// a signed decimal and bytes. Kinds that don't match the schema's
    /// columns are a broken wire.
    #[test]
    fn given_kinds_decide_over_the_fields() {
        let conn = connection();
        conn.execute_batch("RESET arrow_lossless_conversion")
            .unwrap();
        let sql = "SELECT '340282366920938463463374607431768211455'::UHUGEINT AS u, \
                   '101'::BIT AS b, ['1'::BIT] AS l";
        let native = crate::driver::native_rows(&conn, sql).unwrap();
        let max = "340282366920938463463374607431768211455";
        let expected = vec![
            Value::Decimal(max.into()),
            Value::Text("101".into()),
            Value::Array(vec![Value::Text("1".into())]),
        ];
        assert_eq!(native.rows, vec![expected.clone()]);

        let ipc = ipc_of(&conn, sql, false);
        let kinds = vec![Kind::UHugeInt, Kind::Bit, Kind::List(Box::new(Kind::Bit))];
        let (mut stream, mut batches) = IpcStream::start_with_kinds(ipc.clone(), kinds).unwrap();
        batches.extend(stream.finish().unwrap());
        let columns = stream.columns();
        let mut rows = Vec::new();
        let mut kept = 0;
        for b in &batches {
            columns
                .collect(b, RowCap::fail(10), &mut rows, &mut kept)
                .unwrap();
        }
        assert_eq!(rows, vec![expected]);
        assert_eq!(columns.names, native.columns);

        for kinds in [vec![], vec![Kind::Plain; 2], vec![Kind::Plain; 4]] {
            let e = IpcStream::start_with_kinds(ipc.clone(), kinds)
                .err()
                .expect("kinds that don't match the columns");
            assert_eq!(e.code, crate::wire::HELPER_PROTOCOL, "{e:?}");
        }
    }

    /// As DuckDB-WASM sends a pending query: the schema message alone, then
    /// chunks cut anywhere, and no end-of-stream marker. A schema message
    /// has an empty body, which `StreamDecoder` holds back until more bytes
    /// arrive; the first DuckDB-WASM run found every result without its
    /// columns.
    #[test]
    fn a_stream_cut_anywhere_without_its_end_marker_reads_whole() {
        let conn = connection();
        for (sql, rows) in [
            (
                "SELECT range AS i, 'x' || range AS s FROM range(5000)",
                5000,
            ),
            ("SELECT 1 AS a, 'b' AS b WHERE false", 0),
            (
                "SELECT ['a', 'b'][1 + (range % 2)]::ENUM('a', 'b') AS e FROM range(10)",
                10,
            ),
        ] {
            let mut ipc = ipc_of(&conn, sql, false);
            assert_eq!(ipc[ipc.len() - 8..], END_OF_STREAM);
            ipc.truncate(ipc.len() - 8);
            let schema_len = 8 + u32::from_le_bytes(ipc[4..8].try_into().unwrap()) as usize;
            let rest = ipc.split_off(schema_len);
            let (mut stream, mut batches) = IpcStream::start(ipc, KindRules::default()).unwrap();
            assert_eq!(
                stream.columns().names.len(),
                if rows == 10 { 1 } else { 2 },
                "{sql}"
            );
            for chunk in rest.chunks(7) {
                batches.extend(stream.push(chunk.to_vec()).unwrap());
            }
            batches.extend(stream.finish().unwrap());
            let total: usize = batches.iter().map(|b| b.num_rows()).sum();
            assert_eq!(total, rows, "{sql}");
            let native = crate::driver::native_rows(&conn, sql).unwrap();
            let columns = stream.columns();
            let mut decoded = Vec::new();
            let mut kept = 0;
            for b in &batches {
                columns
                    .collect(b, RowCap::fail(100_000), &mut decoded, &mut kept)
                    .unwrap();
            }
            assert!(same_rows(&decoded, &native.rows), "{sql}");
            assert_eq!(columns.names, native.columns, "{sql}");
        }
    }
}
