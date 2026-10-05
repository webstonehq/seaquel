//! `seaquel-cli query -c <CONNECTION> [SQL] [-f FILE] [--saved QUERY]`.
//!
//! Runs SQL as typed on a saved connection through Core's run
//! (`Workspace::run`, the editor's `db.run`), every statement in the text
//! in order, and prints each statement's result as it finishes: one JSON
//! line, or an aligned table on stdout (a blank line between two
//! statements' tables) with a footer on stderr.
//!
//! The SQL comes from exactly one of the positional argument, `-f FILE`
//! (`-` is stdin), `--saved QUERY`, or stdin when it isn't a terminal.
//! `{{name}}` parameters are bound as text from `--param NAME=VALUE`; one
//! without a value is refused before anything connects
//! (`MISSING_PARAMETERS`), so Core never binds NULL for it.
//!
//! It isn't read-only: the user wrote the SQL. Core refuses a run holding
//! a destructive statement unless it is confirmed (`CONFIRM_REQUIRED`,
//! before anything runs); the CLI lists them on stderr and asks when it is
//! interactive, and otherwise needs `--yes`. Nothing is recorded in the
//! app's history (`history: None`), so storage stays read-only.
//!
//! `--limit N` is the run's page size: a SELECT shows its first N rows and
//! Core's count of all of them; `--limit 0` streams every row. When Core
//! couldn't count (`countEstimated: true`), the JSON line's `totalRows` is
//! a lower bound (the rows shown plus one), not a count, and the table's
//! footer says the rest weren't counted.
//!
//! A failing statement makes the exit code 1, and the statements after it
//! still run and print. A statement that fails after some of its rows
//! arrived prints only its error: the rows received are dropped.
//!
//! A positional SQL argument that is a single flag-shaped word (`--yse`)
//! is a usage error rather than SQL, so a mistyped flag can't silently
//! replace a script piped to stdin. SQL that starts with `-` and has more
//! than one word (`-- note\nSELECT 1`) or isn't a word (`-1`) runs.

use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::CommandFactory;
use futures::StreamExt;
use seaquel_core::ai::tools::format::cell_text;
use seaquel_core::domain::run::{DestructiveStatement, ParamValue, RunEvent, RunParams, RunTarget};
use seaquel_core::sql::params::extract_parameters;
use seaquel_core::CoreError;
use seaquel_types::Value;
use serde::Serialize;

use crate::output::{self, json_cell, Format};
use crate::prompt::{self, Prompter};
use crate::session::{block_on, fail, say, say_text, until_stopped, Session};
use crate::{connect, resolve, saved, QueryArgs};

const COMMAND: &str = "query";

pub const MISSING_PARAMETERS: &str = "MISSING_PARAMETERS";
const CONFIRM_REQUIRED: &str = "CONFIRM_REQUIRED";

/// The most destructive statements listed before asking.
const MAX_LISTED: usize = 20;

/// A listed statement's first line is cut at this many characters.
const MAX_LISTED_SQL_CHARS: usize = 80;

/// What a table shows for NULL (an empty cell is an empty string).
const NULL_TEXT: &str = "NULL";

const NOTHING_TO_RUN: &str = "nothing to run";

pub fn run(args: QueryArgs) -> ExitCode {
    let format = Format::pick(args.output.format);
    block_on(COMMAND, query(args, format))
}

/// `--param`'s parser: `NAME=VALUE`, split at the first `=`, so the value
/// may hold more. No `=`, or an empty name, is a usage error.
pub fn parse_param(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((name, value)) if !name.is_empty() => Ok((name.to_string(), value.to_string())),
        _ => Err(format!("{s:?} isn't NAME=VALUE")),
    }
}

/// The `{{name}}`s `sql` uses that `given` has no value for, in text order,
/// each once.
fn missing_parameters(sql: &str, given: &[(String, String)]) -> Vec<String> {
    extract_parameters(sql)
        .into_iter()
        .filter(|name| !given.iter().any(|(g, _)| g == name))
        .collect()
}

fn missing_parameters_error(missing: &[String]) -> CoreError {
    CoreError::new(
        MISSING_PARAMETERS,
        format!(
            "no value for {}. Pass --param NAME=VALUE for each",
            missing
                .iter()
                .map(|n| format!("{{{{{n}}}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

/// Where the SQL comes from.
enum Source {
    Text(String),
    Saved(String),
}

/// Prints a clap usage error for `query` and answers exit code 2.
fn usage_error(kind: ErrorKind, message: &str) -> ExitCode {
    let mut cli = crate::Cli::command();
    let mut cmd = cli
        .find_subcommand_mut(COMMAND)
        .map(|c| c.clone().bin_name(format!("seaquel-cli {COMMAND}")))
        .unwrap_or_default();
    let _ = cmd.error(kind, message).print();
    ExitCode::from(2)
}

fn read_stdin() -> Result<String, CoreError> {
    let mut text = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut text)
        .map_err(|e| CoreError::new("FILE_ERROR", format!("can't read stdin: {e}")))?;
    Ok(text)
}

fn read_file(path: &Path) -> Result<String, CoreError> {
    if path == Path::new("-") {
        return read_stdin();
    }
    std::fs::read_to_string(path)
        .map_err(|e| CoreError::new("FILE_ERROR", format!("can't read {}: {e}", path.display())))
}

/// Why there's no source to run.
enum NoSource {
    /// A usage error (exit 2), with clap's wording.
    Usage(String),
    /// Reading the file or stdin failed.
    Failed(CoreError),
}

/// A single word that looks like a flag: `-x` or `--name`, letters, digits
/// and `-` after the first letter, maybe `=value`, no whitespace. clap takes
/// an unknown flag as the SQL (the argument allows a leading `-`), so this
/// one is refused instead of run as a `--` comment.
fn looks_like_a_flag(arg: &str) -> bool {
    if arg.chars().any(char::is_whitespace) {
        return false;
    }
    let rest = arg
        .strip_prefix("--")
        .or_else(|| arg.strip_prefix('-'))
        .unwrap_or(arg);
    if rest.len() == arg.len() {
        return false;
    }
    let name = rest.split_once('=').map_or(rest, |(name, _)| name);
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The SQL's source, read when it is text.
fn source(args: &QueryArgs) -> Result<Source, NoSource> {
    if let Some(sql) = &args.sql {
        if looks_like_a_flag(sql) {
            return Err(NoSource::Usage(format!(
                "unexpected argument '{sql}'; to run SQL that starts with '-', give more than one word"
            )));
        }
        return Ok(Source::Text(sql.clone()));
    }
    if let Some(path) = &args.file {
        return read_file(path).map(Source::Text).map_err(NoSource::Failed);
    }
    if let Some(name) = &args.saved {
        return Ok(Source::Saved(name.clone()));
    }
    if std::io::stdin().is_terminal() {
        return Err(NoSource::Usage(
            "no SQL: pass it as an argument, with --file or --saved, or pipe it to stdin".into(),
        ));
    }
    read_stdin().map(Source::Text).map_err(NoSource::Failed)
}

/// Text that holds nothing to run, or the parameters it lacks: answered
/// before anything opens or connects.
enum Checked {
    Nothing,
    Run,
}

fn check(sql: &str, given: &[(String, String)]) -> Result<Checked, CoreError> {
    if sql.trim().is_empty() {
        return Ok(Checked::Nothing);
    }
    let missing = missing_parameters(sql, given);
    if missing.is_empty() {
        Ok(Checked::Run)
    } else {
        Err(missing_parameters_error(&missing))
    }
}

async fn query(args: QueryArgs, format: Format) -> ExitCode {
    let source = match source(&args) {
        Err(NoSource::Usage(message)) => {
            let kind = if args.sql.is_some() {
                ErrorKind::UnknownArgument
            } else {
                ErrorKind::MissingRequiredArgument
            };
            return usage_error(kind, &message);
        }
        Err(NoSource::Failed(e)) => return fail(COMMAND, &e),
        Ok(source) => source,
    };
    if let Source::Text(sql) = &source {
        match check(sql, &args.params) {
            Ok(Checked::Nothing) => {
                say(NOTHING_TO_RUN);
                return ExitCode::SUCCESS;
            }
            Ok(Checked::Run) => {}
            Err(e) => return fail(COMMAND, &e),
        }
    }
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(COMMAND, &e),
    };
    // Stopped: the run's stream is dropped, which cancels the statement in
    // flight in Core (a Postgres or MySQL cancel goes out on a task Core
    // spawned; `block_on`'s shutdown lets it), and `close` closes the
    // connection.
    let mut prompter = prompt::for_command(args.input.no_input);
    let result = until_stopped(COMMAND, s, async |s| {
        run_on(s, &args, source, format, &mut prompter).await
    })
    .await;
    match result {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => fail(COMMAND, &e),
        Err(stopped) => stopped,
    }
}

/// Resolves the connection (and the saved query), connects and runs. An
/// `Err` is a failure before or around the run; statement errors are
/// printed as they come and make the answer exit code 1.
async fn run_on(
    s: &Session,
    args: &QueryArgs,
    source: Source,
    format: Format,
    prompter: &mut impl Prompter,
) -> Result<ExitCode, CoreError> {
    let projects = s.ws.list_projects().await?.value;
    let sql = match source {
        Source::Text(sql) => sql,
        Source::Saved(name) => {
            let rows = saved::queries_among(s, &projects, args.project.as_deref()).await?;
            let sql = resolve::saved_query(&rows, &name)?.query.clone();
            match check(&sql, &args.params)? {
                Checked::Nothing => {
                    say(NOTHING_TO_RUN);
                    return Ok(ExitCode::SUCCESS);
                }
                Checked::Run => sql,
            }
        }
    };
    let row = resolve::saved_connection_among(s, &projects, &args.connection).await?;
    let id = connect::connect(s, &row, prompter).await?;
    let run = Run {
        connection_id: id,
        sql,
        params: &args.params,
        limit: args.limit,
        yes: args.yes,
        format,
    };
    run_sql(s, &run, prompter, || uuid::Uuid::new_v4().to_string()).await
}

/// What [`run_sql`] runs.
struct Run<'a> {
    /// Core's connection id.
    connection_id: String,
    sql: String,
    params: &'a [(String, String)],
    limit: u32,
    yes: bool,
    format: Format,
}

/// Runs `run.sql` on its connection, asking about destructive statements
/// when `prompter` can, each attempt under a fresh `stream_id()`.
async fn run_sql(
    s: &Session,
    run: &Run<'_>,
    prompter: &mut impl Prompter,
    mut stream_id: impl FnMut() -> String,
) -> Result<ExitCode, CoreError> {
    let params = (!run.params.is_empty()).then(|| {
        run.params
            .iter()
            .map(|(name, value)| ParamValue {
                name: name.clone(),
                value: Value::Text(value.clone()),
            })
            .collect::<Vec<_>>()
    });
    let mut confirmed = run.yes;
    loop {
        let params = RunParams {
            connection_id: run.connection_id.clone(),
            stream_id: stream_id(),
            text: run.sql.clone(),
            target: RunTarget::All,
            params: params.clone(),
            page_size: run.limit,
            confirmed,
            defer_writes: false,
            history: None,
        };
        match follow(s, params, run.format).await {
            Ended::Done(code) => return Ok(code),
            Ended::NothingToRun => {
                say(NOTHING_TO_RUN);
                return Ok(ExitCode::SUCCESS);
            }
            Ended::Failed(e) => return Err(e),
            Ended::ConfirmRequired {
                message,
                destructive,
                total,
            } => {
                say_text(&destructive_listing(&destructive, total));
                // Core checked before anything ran, so nothing ran: a yes
                // runs the whole text again, confirmed.
                if !confirmed && prompter.interactive() {
                    if prompter.confirm("Run them?").await {
                        confirmed = true;
                        continue;
                    }
                    return Err(CoreError::new("CANCELLED", "Nothing was run."));
                }
                return Err(CoreError::new(
                    CONFIRM_REQUIRED,
                    format!("{message}. Pass --yes to run them."),
                ));
            }
        }
    }
}

/// How a run's stream ended.
enum Ended {
    /// `done`: the exit code.
    Done(ExitCode),
    /// `done` with no statements (only comments, say).
    NothingToRun,
    /// A run-level `error`, or a stream that ended with neither.
    Failed(CoreError),
    ConfirmRequired {
        message: String,
        destructive: Vec<DestructiveStatement>,
        total: u32,
    },
}

/// Runs `params` and prints each statement as it finishes.
async fn follow(s: &Session, params: RunParams, format: Format) -> Ended {
    let mut events = s.ws.run(&s.core, params);
    let mut fold = Fold::default();
    let mut printer = Printer::new(format);
    while let Some(event) = events.next().await {
        if let Some(finished) = fold.event(event) {
            match finished {
                Step::Statement(f) => printer.finished(&f),
                Step::End(ended) => return ended,
            }
        }
    }
    Ended::Failed(CoreError::new(
        "CANCELLED",
        "The run ended without an answer.",
    ))
}

/// One statement's result, gathered from its events.
#[derive(Debug, Default)]
struct Statement {
    index: u32,
    sql: String,
    columns: Option<Vec<String>>,
    rows: Vec<Vec<Value>>,
    /// A batch said rows were left out.
    truncated: bool,
}

/// How a statement finished.
#[derive(Debug)]
enum Outcome {
    Done {
        elapsed_ms: f64,
        total_rows: u64,
        count_estimated: bool,
        rows_affected: Option<u64>,
    },
    Error {
        code: String,
        message: String,
    },
}

#[derive(Debug)]
struct Finished {
    statement: Statement,
    outcome: Outcome,
}

impl Finished {
    /// The columns, when the statement returned any.
    fn columns(&self) -> Option<&[String]> {
        self.statement.columns.as_deref().filter(|c| !c.is_empty())
    }
}

enum Step {
    Statement(Finished),
    End(Ended),
}

/// Folds a run's events into finished statements and its end.
#[derive(Default)]
struct Fold {
    current: Option<Statement>,
    failed: bool,
}

impl Fold {
    /// The statement `index`, the current one when it matches.
    fn take(&mut self, index: u32, sql: Option<String>) -> Statement {
        match self.current.take() {
            Some(st) if st.index == index => st,
            _ => Statement {
                index,
                sql: sql.unwrap_or_default(),
                ..Statement::default()
            },
        }
    }

    fn event(&mut self, event: RunEvent) -> Option<Step> {
        match event {
            RunEvent::StatementStart { index, sql, .. } => {
                self.current = Some(Statement {
                    index,
                    sql,
                    ..Statement::default()
                });
                None
            }
            RunEvent::Batch(batch) => {
                if let Some(st) = self.current.as_mut() {
                    if st.columns.is_none() {
                        st.columns = batch.columns;
                    }
                    st.rows.extend(batch.rows);
                    st.truncated |= batch.truncated;
                }
                None
            }
            RunEvent::StatementDone {
                index,
                elapsed_ms,
                total_rows,
                count_estimated,
                rows_affected,
                ..
            } => Some(Step::Statement(Finished {
                statement: self.take(index, None),
                outcome: Outcome::Done {
                    elapsed_ms,
                    total_rows,
                    count_estimated,
                    rows_affected,
                },
            })),
            RunEvent::StatementError {
                index,
                code,
                message,
                sql,
                ..
            } => {
                self.failed = true;
                Some(Step::Statement(Finished {
                    statement: self.take(index, sql),
                    outcome: Outcome::Error { code, message },
                }))
            }
            // `defer_writes` is false, so Core never defers; were it to,
            // the statement didn't run, which is a failure here.
            RunEvent::StatementDeferred { index, sql, .. } => {
                self.failed = true;
                Some(Step::Statement(Finished {
                    statement: self.take(index, Some(sql)),
                    outcome: Outcome::Error {
                        code: "NOT_RUN".into(),
                        message: "Core deferred this statement instead of running it.".into(),
                    },
                }))
            }
            RunEvent::Done {
                statements,
                succeeded,
                ..
            } => Some(Step::End(if statements == 0 {
                Ended::NothingToRun
            } else if succeeded && !self.failed {
                Ended::Done(ExitCode::SUCCESS)
            } else {
                Ended::Done(ExitCode::FAILURE)
            })),
            RunEvent::Error {
                code,
                message,
                destructive,
                destructive_total,
            } => Some(Step::End(match destructive {
                Some(destructive) if code == CONFIRM_REQUIRED => Ended::ConfirmRequired {
                    total: destructive_total
                        .unwrap_or_else(|| u32::try_from(destructive.len()).unwrap_or(u32::MAX)),
                    message,
                    destructive,
                },
                _ => Ended::Failed(CoreError::new(code, message)),
            })),
        }
    }
}

/// Prints each finished statement: its JSON line, or its table on stdout
/// (a blank line between two statements' tables) and its footer or error
/// on stderr.
struct Printer {
    format: Format,
    /// A table went to stdout already.
    printed_table: bool,
}

impl Printer {
    fn new(format: Format) -> Self {
        Self {
            format,
            printed_table: false,
        }
    }

    fn finished(&mut self, f: &Finished) {
        match self.format {
            Format::Json => output::print(&json_line(f)),
            Format::Table => {
                if let Some(text) = self.table(f) {
                    output::print(&text);
                }
                say(&stderr_line(f));
            }
        }
    }

    /// What the table format writes to stdout for `f`, if anything.
    fn table(&mut self, f: &Finished) -> Option<String> {
        let Outcome::Done { .. } = f.outcome else {
            return None;
        };
        let columns = f.columns()?;
        let mut text = String::new();
        if self.printed_table {
            text.push('\n');
        }
        self.printed_table = true;
        text.push_str(&table_text(columns, &f.statement.rows));
        Some(text)
    }
}

/// A statement's JSON line, as `query --format json` documents it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonLine<'a> {
    statement: u32,
    sql: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<JsonRows<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_rows: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    count_estimated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows_affected: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonError<'a>>,
}

/// Rows written cell by cell through [`json_cell`], so a statement's rows
/// aren't held a second time as JSON values.
struct JsonRows<'a>(&'a [Vec<Value>]);

struct JsonRow<'a>(&'a [Value]);

impl Serialize for JsonRows<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(|row| JsonRow(row)))
    }
}

impl Serialize for JsonRow<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().map(json_cell))
    }
}

#[derive(Serialize)]
struct JsonError<'a> {
    code: &'a str,
    message: &'a str,
}

/// The rows a statement sent, as fewer than its total (a page), or as cut
/// by a batch.
fn is_truncated(f: &Finished, total_rows: u64) -> bool {
    f.statement.truncated || (f.statement.rows.len() as u64) < total_rows
}

fn json_line(f: &Finished) -> String {
    let st = &f.statement;
    let mut line = JsonLine {
        statement: st.index,
        sql: &st.sql,
        columns: None,
        rows: None,
        total_rows: None,
        count_estimated: None,
        truncated: None,
        rows_affected: None,
        elapsed_ms: None,
        error: None,
    };
    match &f.outcome {
        Outcome::Done {
            elapsed_ms,
            total_rows,
            count_estimated,
            rows_affected,
        } => {
            if let Some(columns) = f.columns() {
                line.columns = Some(columns);
                line.rows = Some(JsonRows(&st.rows));
                line.total_rows = Some(*total_rows);
                line.count_estimated = Some(*count_estimated);
                line.truncated = Some(is_truncated(f, *total_rows));
            }
            line.rows_affected = *rows_affected;
            line.elapsed_ms = Some(*elapsed_ms);
        }
        Outcome::Error { code, message } => {
            line.error = Some(JsonError { code, message });
        }
    }
    let mut text = serde_json::to_string(&line).unwrap_or_else(|_| "null".into());
    text.push('\n');
    text
}

fn table_text(columns: &[String], rows: &[Vec<Value>]) -> String {
    let header: Vec<&str> = columns.iter().map(String::as_str).collect();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|v| match v {
                    Value::Null => NULL_TEXT.to_string(),
                    v => cell_text(v),
                })
                .collect()
        })
        .collect();
    output::table(&header, &cells)
}

/// What the table format writes to stderr for a statement: its footer, or
/// `statement N: CODE: message` (N counts from 1).
fn stderr_line(f: &Finished) -> String {
    match &f.outcome {
        Outcome::Done {
            elapsed_ms,
            total_rows,
            count_estimated,
            rows_affected,
        } => footer(&Footer {
            has_columns: f.columns().is_some(),
            shown: f.statement.rows.len() as u64,
            total_rows: *total_rows,
            truncated: is_truncated(f, *total_rows),
            count_estimated: *count_estimated,
            rows_affected: *rows_affected,
            elapsed_ms: *elapsed_ms,
        }),
        Outcome::Error { code, message } => {
            format!("statement {}: {code}: {message}", f.statement.index + 1)
        }
    }
}

/// What a finished statement's footer says.
struct Footer {
    has_columns: bool,
    shown: u64,
    total_rows: u64,
    truncated: bool,
    count_estimated: bool,
    rows_affected: Option<u64>,
    elapsed_ms: f64,
}

/// `3 rows (12 ms)`, `1,000 of 52,331 rows (12 ms); --limit 0 for all`,
/// `first 1,000 rows (12 ms); more weren't counted; --limit 0 for all` when
/// Core couldn't count, `3 rows affected (1 ms)`, or `done (1 ms)` for a
/// statement with neither rows nor a count.
fn footer(f: &Footer) -> String {
    let ms = format!("({} ms)", f.elapsed_ms.max(0.0).round() as u64);
    if f.has_columns {
        if f.count_estimated {
            return format!(
                "first {} {} {ms}; more weren't counted; --limit 0 for all",
                thousands(f.shown),
                plural(f.shown, "row")
            );
        }
        if f.truncated {
            return format!(
                "{} of {} rows {ms}; --limit 0 for all",
                thousands(f.shown),
                thousands(f.total_rows)
            );
        }
        return format!("{} {} {ms}", thousands(f.shown), plural(f.shown, "row"));
    }
    match f.rows_affected {
        Some(n) => format!("{} {} affected {ms}", thousands(n), plural(n, "row")),
        None => format!("done {ms}"),
    }
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// `52331` as `52,331`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The destructive statements Core named, for stderr: a heading, up to
/// [`MAX_LISTED`] lines `  3. drops a table: DROP TABLE x` (the statement's
/// position from 1, its first line cut at [`MAX_LISTED_SQL_CHARS`]
/// characters), then `…and N more` for the rest of `total`.
fn destructive_listing(destructive: &[DestructiveStatement], total: u32) -> String {
    let mut text = format!(
        "This run holds {total} destructive statement{}:\n",
        if total == 1 { "" } else { "s" }
    );
    for d in destructive.iter().take(MAX_LISTED) {
        let first = d.sql.trim().lines().next().unwrap_or_default();
        let mut sql: String = first.chars().take(MAX_LISTED_SQL_CHARS).collect();
        if first.chars().count() > MAX_LISTED_SQL_CHARS {
            sql.push('…');
        }
        text.push_str(&format!(
            "  {}. {}: {}\n",
            d.index + 1,
            seaquel_terminal::destructive_reason(d.reason),
            output::table_text(&sql)
        ));
    }
    let listed = destructive.len().min(MAX_LISTED) as u64;
    let rest = u64::from(total).saturating_sub(listed);
    if rest > 0 {
        text.push_str(&format!("…and {} more\n", thousands(rest)));
    }
    text
}

#[cfg(test)]
mod tests {
    use seaquel_core::sql::statements::DestructiveReason;
    use seaquel_types::StreamBatch;

    use super::*;

    #[test]
    fn a_param_splits_at_the_first_equals_sign() {
        assert_eq!(parse_param("a=b=c"), Ok(("a".into(), "b=c".into())));
        assert_eq!(parse_param("a="), Ok(("a".into(), String::new())));
        assert!(parse_param("abc").is_err());
        assert!(parse_param("=x").is_err());
    }

    #[test]
    fn missing_parameters_in_text_order_once() {
        let given = vec![("b".to_string(), "1".to_string())];
        assert_eq!(
            missing_parameters("SELECT {{c}}, {{b}}, {{a}}, {{c}}", &given),
            ["c", "a"]
        );
        assert!(missing_parameters("SELECT 1", &[]).is_empty());
        let e = missing_parameters_error(&["id".into()]);
        assert_eq!(e.code, MISSING_PARAMETERS);
        assert!(e.message.contains("{{id}}"), "{}", e.message);
    }

    #[test]
    fn blank_text_is_nothing_to_run() {
        assert!(matches!(check("  \n\t", &[]), Ok(Checked::Nothing)));
        assert!(matches!(check("SELECT 1", &[]), Ok(Checked::Run)));
        assert_eq!(
            check("SELECT {{x}}", &[]).err().map(|e| e.code),
            Some(MISSING_PARAMETERS.to_string())
        );
    }

    fn rows(n: u64, total: u64, estimated: bool, ms: f64) -> Footer {
        Footer {
            has_columns: true,
            shown: n,
            total_rows: total,
            truncated: n < total,
            count_estimated: estimated,
            rows_affected: None,
            elapsed_ms: ms,
        }
    }

    #[test]
    fn footers() {
        assert_eq!(footer(&rows(3, 3, false, 12.4)), "3 rows (12 ms)");
        assert_eq!(footer(&rows(1, 1, false, 0.2)), "1 row (0 ms)");
        assert_eq!(footer(&rows(0, 0, false, 1.0)), "0 rows (1 ms)");
        assert_eq!(
            footer(&rows(1000, 52_331, false, 12.0)),
            "1,000 of 52,331 rows (12 ms); --limit 0 for all"
        );
        assert_eq!(
            footer(&rows(1000, 1001, true, 12.0)),
            "first 1,000 rows (12 ms); more weren't counted; --limit 0 for all"
        );
        let write = Footer {
            has_columns: false,
            rows_affected: Some(3),
            ..rows(0, 0, false, 1.0)
        };
        assert_eq!(footer(&write), "3 rows affected (1 ms)");
        let one = Footer {
            rows_affected: Some(1),
            ..write
        };
        assert_eq!(footer(&one), "1 row affected (1 ms)");
        let utility = Footer {
            has_columns: false,
            rows_affected: None,
            ..rows(0, 0, false, 2.0)
        };
        assert_eq!(footer(&utility), "done (2 ms)");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn the_listing_cuts_and_counts_the_rest() {
        let long = format!("DROP TABLE {}\nCASCADE", "x".repeat(100));
        let mut list: Vec<DestructiveStatement> = (0..25)
            .map(|i| DestructiveStatement {
                index: i,
                sql: "DELETE FROM t".into(),
                reason: DestructiveReason::DeleteNoWhere,
            })
            .collect();
        list[0] = DestructiveStatement {
            index: 2,
            sql: long,
            reason: DestructiveReason::DropTable,
        };
        let text = destructive_listing(&list, 130);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "This run holds 130 destructive statements:");
        assert!(lines[1].starts_with("  3. drops a table: DROP TABLE xxx"));
        assert!(lines[1].ends_with('…') && !lines[1].contains("CASCADE"));
        assert_eq!(lines[2], "  2. DELETE without WHERE: DELETE FROM t");
        assert_eq!(lines.len(), 1 + MAX_LISTED + 1);
        assert_eq!(lines[MAX_LISTED + 1], "…and 110 more");

        let one = destructive_listing(&list[1..2], 1);
        assert_eq!(
            one,
            "This run holds 1 destructive statement:\n  2. DELETE without WHERE: DELETE FROM t\n"
        );
    }

    fn batch(columns: Option<&[&str]>, rows: Vec<Vec<Value>>) -> RunEvent {
        RunEvent::Batch(StreamBatch {
            columns: columns.map(|c| c.iter().map(|s| s.to_string()).collect()),
            rows,
            is_final: false,
            truncated: false,
        })
    }

    fn start(index: u32, sql: &str) -> RunEvent {
        serde_json::from_value::<seaquel_core::domain::run::PageSource>(
            serde_json::json!({"sql": sql, "params": []}),
        )
        .map(|source| RunEvent::StatementStart {
            index,
            sql: sql.into(),
            source,
            query_type: seaquel_core::sql::statements::QueryType::Select,
            kind: seaquel_core::domain::run::StatementKind::Page,
            page: 1,
            page_size: 1,
            table: None,
            column_refs: None,
        })
        .unwrap()
    }

    fn done(index: u32, total_rows: u64, rows_affected: Option<u64>) -> RunEvent {
        RunEvent::StatementDone {
            index,
            elapsed_ms: 1.5,
            total_rows,
            total_pages: 1,
            count_estimated: false,
            rows_affected,
            last_insert_id: None,
        }
    }

    fn finished(step: Option<Step>) -> Finished {
        match step {
            Some(Step::Statement(f)) => f,
            _ => panic!("expected a finished statement"),
        }
    }

    #[test]
    fn a_page_s_json_line() {
        let mut fold = Fold::default();
        assert!(fold.event(start(0, "SELECT id FROM t")).is_none());
        assert!(fold
            .event(batch(Some(&["id"]), vec![vec![Value::Int(1)]]))
            .is_none());
        // Only the first batch's columns count.
        assert!(fold
            .event(batch(Some(&["other"]), vec![vec![Value::Int(1 << 60)]]))
            .is_none());
        let f = finished(fold.event(done(0, 5, None)));
        let line: serde_json::Value = serde_json::from_str(&json_line(&f)).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "statement": 0, "sql": "SELECT id FROM t", "columns": ["id"],
                "rows": [[1], ["1152921504606846976"]], "totalRows": 5,
                "countEstimated": false, "truncated": true, "elapsedMs": 1.5,
            })
        );
        assert_eq!(stderr_line(&f), "2 of 5 rows (2 ms); --limit 0 for all");
    }

    #[test]
    fn a_write_s_json_line_has_no_rows() {
        let mut fold = Fold::default();
        fold.event(start(1, "DELETE FROM t"));
        fold.event(batch(Some(&[]), vec![]));
        let f = finished(fold.event(done(1, 0, Some(2))));
        let line: serde_json::Value = serde_json::from_str(&json_line(&f)).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "statement": 1, "sql": "DELETE FROM t", "rowsAffected": 2, "elapsedMs": 1.5,
            })
        );
        assert_eq!(stderr_line(&f), "2 rows affected (2 ms)");
    }

    #[test]
    fn a_statement_error_and_the_end() {
        let mut fold = Fold::default();
        // A planned failure has no start: its SQL comes with the error.
        let f = finished(fold.event(RunEvent::StatementError {
            index: 2,
            code: "SQL_ERROR".into(),
            message: "no such column".into(),
            elapsed_ms: 0.0,
            sql: Some("SELECT nope".into()),
        }));
        let line: serde_json::Value = serde_json::from_str(&json_line(&f)).unwrap();
        assert_eq!(
            line,
            serde_json::json!({
                "statement": 2, "sql": "SELECT nope",
                "error": {"code": "SQL_ERROR", "message": "no such column"},
            })
        );
        assert_eq!(stderr_line(&f), "statement 3: SQL_ERROR: no such column");
        // Core says the run didn't succeed; a statement failed either way.
        match fold.event(RunEvent::Done {
            statements: 3,
            succeeded: true,
            history: None,
        }) {
            Some(Step::End(Ended::Done(code))) => assert_eq!(code, ExitCode::FAILURE),
            _ => panic!("expected the end"),
        }
    }

    #[test]
    fn confirm_required_ends_with_the_list() {
        let mut fold = Fold::default();
        let list = vec![DestructiveStatement {
            index: 0,
            sql: "DELETE FROM t".into(),
            reason: DestructiveReason::DeleteNoWhere,
        }];
        match fold.event(RunEvent::confirm_required(list)) {
            Some(Step::End(Ended::ConfirmRequired {
                destructive, total, ..
            })) => {
                assert_eq!(destructive.len(), 1);
                assert_eq!(total, 1);
            }
            _ => panic!("expected CONFIRM_REQUIRED"),
        }
        match fold.event(RunEvent::error("CONNECTION_CLOSED", "gone")) {
            Some(Step::End(Ended::Failed(e))) => assert_eq!(e.code, "CONNECTION_CLOSED"),
            _ => panic!("expected a failure"),
        }
    }

    fn parse(args: &[&str]) -> Result<QueryArgs, clap::error::ErrorKind> {
        use clap::Parser;
        let mut full = vec!["seaquel-cli", "query"];
        full.extend(args);
        match crate::Cli::try_parse_from(full) {
            Ok(crate::Cli {
                command: crate::Command::Query(q),
            }) => Ok(q),
            Ok(_) => unreachable!("parsed another command"),
            Err(e) => Err(e.kind()),
        }
    }

    /// SQL may start with `-` (a comment, a negative number); known flags
    /// before or after it are still flags.
    #[test]
    fn sql_may_start_with_a_hyphen() {
        let q = parse(&["-c", "x", "-- note\nSELECT 1"]).unwrap();
        assert_eq!(q.sql.as_deref(), Some("-- note\nSELECT 1"));
        assert_eq!(
            parse(&["-c", "x", "-1"]).unwrap().sql.as_deref(),
            Some("-1")
        );

        let q = parse(&["-c", "x", "-- c", "--format", "json", "--yes"]).unwrap();
        assert_eq!(q.sql.as_deref(), Some("-- c"));
        assert!(q.yes);
        assert_eq!(q.output.format, Some(crate::FormatArg::Json));

        let q = parse(&["-c", "x", "--yes", "--limit", "5", "SELECT 1"]).unwrap();
        assert_eq!(q.sql.as_deref(), Some("SELECT 1"));
        assert!(q.yes);
        assert_eq!(q.limit, 5);

        // With no SQL, a flag is a flag.
        let q = parse(&["-c", "x", "--yes"]).unwrap();
        assert!(q.sql.is_none() && q.yes);
        // Anything after the SQL that isn't a flag is a usage error.
        assert!(parse(&["-c", "x", "-- c", "--bogus"]).is_err());
        assert!(parse(&["-c", "x", "--param", "nope", "SELECT 1"]).is_err());
    }

    #[test]
    fn the_limit_stops_at_the_page_cap() {
        let q = parse(&["-c", "x", "--limit", "99999", "SELECT 1"]).unwrap();
        assert_eq!(q.limit, 99_999);
        assert_eq!(parse(&["-c", "x", "SELECT 1"]).unwrap().limit, 1000);
        assert_eq!(
            parse(&["-c", "x", "--limit", "100000", "SELECT 1"]).unwrap_err(),
            clap::error::ErrorKind::ValueValidation
        );
    }

    #[test]
    fn a_lone_flag_shaped_word_isnt_sql() {
        for flag in ["--yse", "-y", "--limit=5", "--no-input", "-f", "--x1-y"] {
            assert!(looks_like_a_flag(flag), "{flag}");
        }
        for sql in [
            "-1",
            "-- note\nSELECT 1",
            "-- c",
            "--",
            "-",
            "--1",
            "SELECT 1",
            "--a b",
            "--x_y",
        ] {
            assert!(!looks_like_a_flag(sql), "{sql:?}");
        }
    }

    /// A SQLite file with `t` holding two rows, connected in a session of
    /// its own: Core's id of the connection.
    async fn lite(dir: &std::path::Path) -> (Session, String) {
        use std::sync::Arc;
        let core = Arc::new(
            seaquel_core::with_plugins(|id| id != "duckdb")
                .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
                .executor(Arc::new(seaquel_runtime::TokioExecutor))
                .build(),
        );
        let ws = core
            .open_workspace(seaquel_core::WorkspaceSpec::new(dir.join("data")))
            .await
            .unwrap();
        let form = serde_json::from_value(serde_json::json!({
            "name": "lite", "type": "sqlite",
            "databaseName": dir.join("t.sqlite").to_str().unwrap(),
        }))
        .unwrap();
        let id = ws
            .connect(
                &core,
                seaquel_core::ConnectRequest::form(form).with_create_if_missing(true),
            )
            .await
            .unwrap();
        let s = Session::for_test(core, ws);
        for sql in [
            "CREATE TABLE t (id INTEGER)",
            "INSERT INTO t VALUES (1), (2)",
        ] {
            s.ws.execute(&s.core, &id, sql, vec![]).await.unwrap();
        }
        (s, id)
    }

    async fn count(s: &Session, id: &str) -> Value {
        let r =
            s.ws.query(&s.core, id, "SELECT count(*) FROM t", vec![])
                .await
                .unwrap();
        r.rows[0][0].clone()
    }

    fn delete_all(id: &str) -> Run<'static> {
        Run {
            connection_id: id.to_string(),
            sql: "DELETE FROM t".into(),
            params: &[],
            limit: 1000,
            yes: false,
            format: Format::Json,
        }
    }

    /// Ids handed out, recorded.
    fn ids(seen: &mut Vec<String>) -> impl FnMut() -> String + '_ {
        move || {
            let id = format!("run-{}", seen.len());
            seen.push(id.clone());
            id
        }
    }

    #[tokio::test]
    async fn no_at_the_destructive_question_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (s, id) = lite(dir.path()).await;
        let mut p = crate::prompt::Scripted {
            interactive: true,
            confirms: [false].into(),
            ..Default::default()
        };
        let mut seen = Vec::new();
        let e = run_sql(&s, &delete_all(&id), &mut p, ids(&mut seen))
            .await
            .unwrap_err();
        assert_eq!(e.code, "CANCELLED");
        assert_eq!(p.asked, ["Run them?"]);
        assert_eq!(seen.len(), 1);
        assert_eq!(count(&s, &id).await, Value::Int(2));
        s.close().await;
    }

    #[tokio::test]
    async fn yes_at_the_destructive_question_runs_it_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let (s, id) = lite(dir.path()).await;
        let mut p = crate::prompt::Scripted {
            interactive: true,
            confirms: [true].into(),
            ..Default::default()
        };
        let mut seen = Vec::new();
        let code = run_sql(&s, &delete_all(&id), &mut p, ids(&mut seen))
            .await
            .unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(p.asked, ["Run them?"]);
        // The confirmed run is a run of its own.
        assert_eq!(seen, ["run-0", "run-1"]);
        assert_eq!(count(&s, &id).await, Value::Int(0));
        s.close().await;
    }

    #[tokio::test]
    async fn not_interactive_the_destructive_question_isnt_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (s, id) = lite(dir.path()).await;
        let mut p = crate::prompt::Scripted::default();
        let mut seen = Vec::new();
        let e = run_sql(&s, &delete_all(&id), &mut p, ids(&mut seen))
            .await
            .unwrap_err();
        assert_eq!(e.code, CONFIRM_REQUIRED);
        assert!(
            e.message.ends_with("Pass --yes to run them."),
            "{}",
            e.message
        );
        assert!(p.asked.is_empty());
        assert_eq!(count(&s, &id).await, Value::Int(2));
        s.close().await;
    }

    /// A blank line between two statements' tables, none before the first
    /// or after the last, and none for a statement without a table.
    #[test]
    fn tables_are_separated_by_a_blank_line() {
        let mut fold = Fold::default();
        let mut select = |index: u32| {
            fold.event(start(index, "SELECT 1"));
            fold.event(batch(Some(&["a"]), vec![vec![Value::Int(1)]]));
            finished(fold.event(done(index, 1, None)))
        };
        let (first, second) = (select(0), select(2));
        let mut fold = Fold::default();
        fold.event(start(1, "DELETE FROM t"));
        let write = finished(fold.event(done(1, 0, Some(2))));

        let mut printer = Printer::new(Format::Table);
        let a = printer.table(&first).unwrap();
        assert!(printer.table(&write).is_none());
        let b = printer.table(&second).unwrap();
        assert_eq!(a, "a\n─\n1\n");
        assert_eq!(b, "\na\n─\n1\n");
    }

    #[test]
    fn a_table_shows_null() {
        let text = table_text(
            &["a".into(), "b".into()],
            &[vec![Value::Null, Value::Text(String::new())]],
        );
        assert_eq!(text.lines().nth(2), Some("NULL"));
    }
}
