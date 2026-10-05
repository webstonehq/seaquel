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
    /// is `UNSUPPORTED_TYPE` naming the column.
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

/// The helper's client decodes as the helper's kinds say; a column it
/// can't read fails its row with `UNSUPPORTED_TYPE` naming the column,
/// after the rows before it read fine (the native driver's test, moved
/// here with the driver's deletion). No SQL value fails to decode today,
/// so the column's kind is forced: a BIGNUM needs at least 4 bytes.
#[cfg(all(test, feature = "remote"))]
mod client_tests {
    use std::sync::Arc;

    use arrow_array::{BinaryArray, RecordBatch};
    use arrow_schema::{DataType, Field, Schema};
    use seaquel_engine::{RowCap, Value};

    use super::*;

    #[test]
    fn an_undecodable_cell_is_unsupported_type() {
        let mut cells: Vec<Option<&[u8]>> = vec![None; 3000];
        cells.push(Some(b"\x01"));
        let schema = Arc::new(Schema::new(vec![Field::new("t", DataType::Binary, true)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(BinaryArray::from_opt_vec(cells))],
        )
        .unwrap();
        let mut ipc = Vec::new();
        let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut ipc, &schema).unwrap();
        w.write(&batch).unwrap();
        w.finish().unwrap();
        drop(w);

        let (mut stream, mut batches) =
            IpcStream::start_with_kinds(ipc, vec![Kind::Bignum]).unwrap();
        batches.extend(stream.finish().unwrap());
        let columns = stream.columns();
        let mut rows = Vec::new();
        let mut kept = 0;
        let mut outcome = Ok(true);
        for b in &batches {
            outcome = columns.collect(b, RowCap::fail(100_000), &mut rows, &mut kept);
            if outcome.is_err() {
                break;
            }
        }
        let e = outcome.unwrap_err();
        assert_eq!(rows, vec![vec![Value::Null]; 3000]);
        assert_eq!(e.code, "UNSUPPORTED_TYPE");
        assert!(e.message.contains("Column \"t\""), "{}", e.message);
        assert!(e.message.contains("BIGNUM of 1 bytes"), "{}", e.message);
    }
}

#[cfg(all(test, feature = "helper"))]
mod tests {
    use super::*;
    use seaquel_engine::Value;

    use crate::test_reference::{self as reference, same_rows};

    fn connection() -> duckdb::Connection {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        // As the helper opens it.
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

    /// DuckDB's own Arrow, written as IPC and read back the way the
    /// browser driver reads DuckDB-WASM's, decodes to the reference
    /// (`test_reference`): every typed-cell case of the frozen fixture, in
    /// both formats, plus results that span several chunks, an empty result
    /// (its columns still named) and a dictionary column.
    #[test]
    fn decode_from_ipc_matches_the_reference() {
        let conn = connection();
        let cases = reference::all();
        let mut failures = Vec::new();
        for case in &cases {
            for sql in &case.setup {
                conn.execute_batch(sql).unwrap();
            }
            for file in [true, false] {
                let ipc = ipc_of(&conn, &case.select, file);
                let cap = RowCap::fail(seaquel_engine::max_query_rows());
                match decode_batches(&ipc, cap, KindRules::default()) {
                    Ok(r) => {
                        if let Err(e) = case.check(&r.columns, &r.rows) {
                            failures.push(format!("{e} (file: {file})"));
                        }
                    }
                    Err(e) => failures.push(format!("{} (file: {file}): {e:?}", case.select)),
                }
            }
            for sql in &case.teardown {
                conn.execute_batch(sql).unwrap();
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} differ:\n{}",
            failures.len(),
            cases.len() * 2,
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
        let max = "340282366920938463463374607431768211455";
        let expected = vec![
            Value::Decimal(max.into()),
            Value::Text("101".into()),
            Value::Array(vec![Value::Text("1".into())]),
        ];

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
        assert_eq!(columns.names, ["u", "b", "l"]);

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
        let chunked: Vec<Vec<Value>> = (0..5000i64)
            .map(|r| vec![Value::Int(r), Value::Text(format!("x{r}"))])
            .collect();
        let enum_rows: Vec<Vec<Value>> = (0..10)
            .map(|r| vec![Value::Text(if r % 2 == 0 { "a" } else { "b" }.into())])
            .collect();
        for (sql, rows, want_columns, want_rows) in [
            (
                "SELECT range AS i, 'x' || range AS s FROM range(5000)",
                5000,
                vec!["i", "s"],
                chunked,
            ),
            (
                "SELECT 1 AS a, 'b' AS b WHERE false",
                0,
                vec!["a", "b"],
                vec![],
            ),
            (
                "SELECT ['a', 'b'][1 + (range % 2)]::ENUM('a', 'b') AS e FROM range(10)",
                10,
                vec!["e"],
                enum_rows,
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
            let columns = stream.columns();
            let mut decoded = Vec::new();
            let mut kept = 0;
            for b in &batches {
                columns
                    .collect(b, RowCap::fail(100_000), &mut decoded, &mut kept)
                    .unwrap();
            }
            assert!(same_rows(&decoded, &want_rows), "{sql}");
            assert_eq!(columns.names, want_columns, "{sql}");
        }
    }
}
