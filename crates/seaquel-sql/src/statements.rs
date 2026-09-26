//! Statement checks on significant tokens (Task 3): query type, the
//! destructive-statement check (fix 11) and the source table for inline
//! editing (fix 12). Ports of `src/lib/db/query-utils.ts` (deleted in phase
//! 2b), with the rules of the recorder's `statements-model.ts`
//! (`docs/plans/artifacts/2026-09-27-sql-recorder-statements-model.ts.txt`).
//!
//! Every check reads the tokens with a MySQL/MariaDB executable comment as
//! code ([`crate::scan::ScanOptions::exec_comments`]). Strings, comments and
//! quoted names don't count.

use std::cell::Cell;

use serde::Serialize;

use crate::scan::{code_tokens, is_punct, Token, TokenKind};
use crate::SqlEngine;

/// The TS `QueryType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum QueryType {
    Select,
    Insert,
    Update,
    Delete,
    Other,
}

/// The TS `DestructiveReason`, plus fix 11's `drop_sequence`,
/// `drop_function` (DROP FUNCTION and DROP PROCEDURE) and `merge_delete`
/// (MERGE … THEN DELETE).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum DestructiveReason {
    DropTable,
    DropIndex,
    DropView,
    DropSchema,
    DropDatabase,
    DropSequence,
    DropFunction,
    DropColumn,
    Truncate,
    DeleteNoWhere,
    UpdateNoWhere,
    MergeDelete,
}

/// A table named in SQL, unquoted per engine. The TS `{ schema?, table }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TableRef {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub schema: Option<String>,
    pub table: String,
}

/// Significant tokens with their upper-cased word text, for the checks.
pub(crate) struct Tokens<'a> {
    pub(crate) sql: &'a str,
    pub(crate) toks: Vec<Token>,
    words: Vec<Option<String>>,
}

impl<'a> Tokens<'a> {
    pub(crate) fn new(sql: &'a str, toks: Vec<Token>) -> Self {
        let words = toks
            .iter()
            .map(|t| (t.kind == TokenKind::Word).then(|| t.text(sql).to_uppercase()))
            .collect();
        Tokens { sql, toks, words }
    }

    pub(crate) fn len(&self) -> usize {
        self.toks.len()
    }

    pub(crate) fn get(&self, k: usize) -> Option<&Token> {
        self.toks.get(k)
    }

    /// The token at `k` before `k` (`None` at the start).
    pub(crate) fn before(&self, k: usize, by: usize) -> Option<&Token> {
        k.checked_sub(by).and_then(|j| self.toks.get(j))
    }

    /// The upper-cased word at `k`, if the token is a word.
    pub(crate) fn word(&self, k: usize) -> Option<&str> {
        self.words.get(k).and_then(|w| w.as_deref())
    }

    /// The upper-cased word `by` tokens before `k`.
    pub(crate) fn word_before(&self, k: usize, by: usize) -> Option<&str> {
        k.checked_sub(by).and_then(|j| self.word(j))
    }

    pub(crate) fn is(&self, t: Option<&Token>, p: u8) -> bool {
        is_punct(self.sql, t, p)
    }

    pub(crate) fn text(&self, t: &Token) -> &'a str {
        t.text(self.sql)
    }
}

/// Whether a word token is a number: it starts with a digit or `.` (fix 18's
/// rule, `0x…` included), or it's SQL Server money (`$1`). A MySQL name that
/// starts with digits (`2fa_codes`) counts as a number here, which only makes
/// the `.` exemption stricter.
fn is_number(text: &str) -> bool {
    let b = text.as_bytes();
    match b.first() {
        Some(c) if c.is_ascii_digit() || *c == b'.' => true,
        Some(b'$') => b.get(1).is_some_and(|c| c.is_ascii_digit() || *c == b'.'),
        _ => false,
    }
}

/// The quote characters that open a name on `engine` (`ansi_mysql`: MySQL
/// with `ANSI_QUOTES`, where `"` does too).
fn name_quotes(engine: SqlEngine, ansi_mysql: bool) -> &'static [char] {
    match engine {
        SqlEngine::Mysql | SqlEngine::Mariadb if ansi_mysql => &['`', '"'],
        SqlEngine::Mysql | SqlEngine::Mariadb => &['`'],
        SqlEngine::Sqlite => &['"', '`', '['],
        SqlEngine::Mssql => &['"', '['],
        SqlEngine::Postgres | SqlEngine::Duckdb => &['"'],
    }
}

/// Whether a token is a name: a word that isn't a number, or a quoted token
/// that isn't a string.
pub(crate) fn is_name(sql: &str, t: Option<&Token>, engine: SqlEngine) -> bool {
    let Some(t) = t else { return false };
    let text = t.text(sql);
    match t.kind {
        TokenKind::Word => !is_number(text),
        TokenKind::Quoted => text
            .chars()
            .next()
            .is_some_and(|c| name_quotes(engine, false).contains(&c)),
        _ => false,
    }
}

/// A name token's name, unquoted per engine; `None` for a string literal, an
/// unterminated name or anything else.
pub(crate) fn unquote_name(
    sql: &str,
    t: &Token,
    engine: SqlEngine,
    ansi_mysql: bool,
) -> Option<String> {
    let text = t.text(sql);
    match t.kind {
        TokenKind::Word => return Some(text.to_string()),
        TokenKind::Quoted => {}
        _ => return None,
    }
    let mut chars = text.chars();
    let open = chars.next()?;
    if !name_quotes(engine, ansi_mysql).contains(&open) {
        return None;
    }
    let close = if open == '[' { ']' } else { open };
    // SQLite's `[…]` has no escape; every other name quote doubles itself.
    let doubles = !(open == '[' && engine == SqlEngine::Sqlite);
    let mut name = String::new();
    let mut rest = chars.peekable();
    while let Some(c) = rest.next() {
        if c != close {
            name.push(c);
        } else if doubles && rest.peek() == Some(&close) {
            rest.next();
            name.push(close);
        } else {
            return rest.peek().is_none().then_some(name);
        }
    }
    None // unterminated
}

/// The type of the statement, from its first significant token (fix 11): a
/// leading `#` or nested comment no longer hides it, a MySQL executable
/// comment's contents count, and the token has to be the keyword
/// (`SELECTED` isn't a SELECT).
pub fn query_type(sql: &str, engine: SqlEngine) -> QueryType {
    let toks = code_tokens(sql, engine);
    let Some(first) = toks.first().filter(|t| t.kind == TokenKind::Word) else {
        return QueryType::Other;
    };
    match first.text(sql).to_uppercase().as_str() {
        "SELECT" => QueryType::Select,
        "INSERT" => QueryType::Insert,
        "UPDATE" => QueryType::Update,
        "DELETE" => QueryType::Delete,
        _ => QueryType::Other,
    }
}

/// Words after ALTER TABLE … DROP that don't name a column.
const NOT_A_COLUMN: &[&str] = &[
    "CONSTRAINT",
    "INDEX",
    "KEY",
    "PRIMARY",
    "FOREIGN",
    "CHECK",
    "DEFAULT",
    "NOT",
    "IDENTITY",
    "EXPRESSION",
    "PARTITION",
    "UNIQUE",
    "TRIGGER",
    "ROW",
    "SYSTEM",
    "PERIOD",
    "FULLTEXT",
    "SPATIAL",
];

/// Words after ON DELETE / ON UPDATE in a foreign key or a MySQL column default.
const ON_ACTIONS: &[&str] = &[
    "CASCADE",
    "SET",
    "NO",
    "RESTRICT",
    "CURRENT_TIMESTAMP",
    "NOW",
];

/// T-SQL statement words that end the statement before them when there's no `;`.
const TSQL_STATEMENTS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "MERGE", "DECLARE", "EXEC", "EXECUTE", "DROP",
    "CREATE", "ALTER", "TRUNCATE", "SET",
];

/// Words before DELETE/UPDATE that make it an event or privilege, not a
/// statement: a trigger's or policy's event list, GRANT/REVOKE.
const NOT_DML_AFTER: &[&str] = &["AFTER", "BEFORE", "OF", "OR", "GRANT", "REVOKE"];

struct Destructive<'a> {
    t: Tokens<'a>,
    engine: SqlEngine,
    mssql: bool,
    /// Per token: for a DELETE/UPDATE, whether a WHERE follows it at its own
    /// depth before its statement ends ([`where_follows`]). One pass instead
    /// of a scan per DELETE/UPDATE, which is quadratic on nested or repeated
    /// statements.
    has_where: Vec<bool>,
    /// Per `)` token: the nearest `(` before it at the same depth.
    open_of: Vec<Option<usize>>,
    /// `analyze[k]`: how many ANALYZE words come before token `k`.
    analyze: Vec<u32>,
    /// Where the last [`Self::drops_column`] that found nothing stopped: a
    /// later ALTER TABLE before it finds nothing either.
    no_drop_before: Cell<usize>,
}

impl Destructive<'_> {
    fn w(&self, k: usize) -> Option<&str> {
        self.t.word(k)
    }

    fn statement_start(&self, k: usize) -> bool {
        k == 0 || self.t.is(self.t.before(k, 1), b';')
    }

    fn drop_reason(&self, k: usize) -> Option<DestructiveReason> {
        let mut j = k + 1;
        if matches!(self.w(j), Some("TEMPORARY" | "TEMP")) {
            j += 1;
        }
        match self.w(j)? {
            "TABLE" => Some(DestructiveReason::DropTable),
            "INDEX" => Some(DestructiveReason::DropIndex),
            "VIEW" => Some(DestructiveReason::DropView),
            "MATERIALIZED" => {
                (self.w(j + 1) == Some("VIEW")).then_some(DestructiveReason::DropView)
            }
            "SCHEMA" => Some(DestructiveReason::DropSchema),
            "DATABASE" => Some(DestructiveReason::DropDatabase),
            "SEQUENCE" => Some(DestructiveReason::DropSequence),
            "FUNCTION" | "PROCEDURE" => Some(DestructiveReason::DropFunction),
            _ => None,
        }
    }

    fn drops_column(&self, k: usize) -> bool {
        if k < self.no_drop_before.get() {
            return false;
        }
        let mut j = k + 2;
        while j < self.t.len() && !self.t.is(self.t.get(j), b';') {
            if self.w(j) == Some("DROP") {
                if self.w(j + 1) == Some("COLUMN") {
                    return true;
                }
                let next = self.t.get(j + 1);
                if is_name(self.t.sql, next, self.engine)
                    && !NOT_A_COLUMN.contains(&self.w(j + 1).unwrap_or(""))
                {
                    return true;
                }
            }
            j += 1;
        }
        self.no_drop_before.set(j);
        false
    }

    /// Whether the `(…)` group closing just before `k` follows EXPLAIN and
    /// holds no ANALYZE.
    fn explain_without_analyze(&self, k: usize) -> bool {
        let Some(close) = k.checked_sub(1) else {
            return false;
        };
        let Some(open) = self.open_of.get(close).copied().flatten() else {
            return false; // no opening parenthesis
        };
        let analyze = self.analyze[close] > self.analyze[open + 1];
        open > 0 && self.w(open - 1) == Some("EXPLAIN") && !analyze
    }

    fn dml_start(&self, k: usize) -> bool {
        let prev = self.t.before(k, 1);
        let p = self.t.word_before(k, 1);
        if p.is_some_and(|p| NOT_DML_AFTER.contains(&p)) {
            return false;
        }
        if self.t.is(prev, b',') {
            return false;
        }
        // T-SQL's IF UPDATE(col) and UPDATE STATISTICS.
        if self.t.is(self.t.get(k + 1), b'(') || self.w(k + 1) == Some("STATISTICS") {
            return false;
        }
        if self.t.is(prev, b')') && self.explain_without_analyze(k) {
            return false;
        }
        // FOR UPDATE, ON DUPLICATE KEY UPDATE, MERGE … THEN UPDATE.
        if self.t.is(prev, b'.') || matches!(p, Some("FOR" | "KEY" | "THEN")) {
            return false;
        }
        if p == Some("ON") && self.w(k + 1).is_some_and(|w| ON_ACTIONS.contains(&w)) {
            return false;
        }
        if self.statement_start(k) || self.t.is(prev, b'(') || self.t.is(prev, b')') {
            return true;
        }
        if matches!(p, Some("ANALYZE" | "VERBOSE")) {
            return true;
        }
        self.mssql
    }

    fn has_where(&self, k: usize) -> bool {
        self.has_where.get(k).copied().unwrap_or(false)
    }

    fn reason(&self) -> Option<DestructiveReason> {
        for k in 0..self.t.len() {
            let Some(wk) = self.w(k) else { continue };
            let start =
                self.statement_start(k) || (self.mssql && !self.t.is(self.t.before(k, 1), b'.'));
            if start && wk == "DROP" {
                if let Some(reason) = self.drop_reason(k) {
                    return Some(reason);
                }
            }
            if start && wk == "TRUNCATE" {
                return Some(DestructiveReason::Truncate);
            }
            if start && wk == "ALTER" && self.w(k + 1) == Some("TABLE") && self.drops_column(k) {
                return Some(DestructiveReason::DropColumn);
            }
            if wk == "DELETE" && self.t.word_before(k, 1) == Some("THEN") {
                return Some(DestructiveReason::MergeDelete);
            }
            if (wk == "DELETE" || wk == "UPDATE") && self.dml_start(k) && !self.has_where(k) {
                return Some(if wk == "DELETE" {
                    DestructiveReason::DeleteNoWhere
                } else {
                    DestructiveReason::UpdateNoWhere
                });
            }
        }
        None
    }
}

/// Why running `sql` could lose data or schema irreversibly, if it could
/// (fix 11). The first reason in the text wins:
///
/// - DROP TABLE (TEMPORARY/TEMP TABLE too), INDEX, VIEW (MATERIALIZED VIEW
///   too), SCHEMA, DATABASE, SEQUENCE, FUNCTION/PROCEDURE, and TRUNCATE, at
///   the start of a statement (on SQL Server, after any token but a `.`).
/// - ALTER TABLE with DROP COLUMN, or DROP followed by a column's name.
/// - DELETE or UPDATE where a statement can start (first, after `;`, `(` or
///   `)`, after EXPLAIN ANALYZE [VERBOSE], or anywhere on SQL Server) with no
///   WHERE at its own parenthesis depth before its statement ends. Not an
///   event list, a privilege list, FOR UPDATE, ON DUPLICATE KEY UPDATE, ON
///   DELETE CASCADE, `IF UPDATE(col)`, UPDATE STATISTICS or `EXPLAIN (…)`
///   without ANALYZE.
/// - MERGE … THEN DELETE.
pub fn destructive_reason(sql: &str, engine: SqlEngine) -> Option<DestructiveReason> {
    Destructive {
        t: Tokens::new(sql, code_tokens(sql, engine)),
        engine,
        mssql: engine == SqlEngine::Mssql,
        no_drop_before: Cell::new(0),
        has_where: Vec::new(),
        open_of: Vec::new(),
        analyze: Vec::new(),
    }
    .prepared()
    .reason()
}

impl Destructive<'_> {
    /// Fills the per-token tables, each in one pass.
    fn prepared(mut self) -> Self {
        let n = self.t.len();
        self.has_where = where_follows(&self.t, self.mssql);
        let mut last_open: Vec<Option<usize>> = Vec::new();
        self.open_of = vec![None; n];
        self.analyze = vec![0; n + 1];
        for (k, tok) in self.t.toks.iter().enumerate() {
            let d = tok.depth as usize;
            if self.t.is(Some(tok), b'(') {
                if last_open.len() <= d {
                    last_open.resize(d + 1, None);
                }
                last_open[d] = Some(k);
            } else if self.t.is(Some(tok), b')') {
                self.open_of[k] = last_open.get(d).copied().flatten();
            }
            self.analyze[k + 1] = self.analyze[k] + u32::from(self.t.word(k) == Some("ANALYZE"));
        }
        self
    }
}

/// For each DELETE/UPDATE token, whether a WHERE token follows it at its own
/// parenthesis depth before its statement ends: a `;` at that depth, the end
/// of its parentheses, or on SQL Server the next statement word (UPDATE's
/// own first SET aside). Every other entry is `false`.
///
/// One pass: the DELETE/UPDATE tokens still waiting for an answer sit on a
/// stack, deepest on top (a token at depth `d` has already settled every
/// deeper one), and each token settles the entries it ends.
fn where_follows(t: &Tokens, mssql: bool) -> Vec<bool> {
    struct Pending {
        k: usize,
        depth: u32,
        update: bool,
        saw_set: bool,
    }
    let mut out = vec![false; t.len()];
    let mut pending: Vec<Pending> = Vec::new();
    for (j, tok) in t.toks.iter().enumerate() {
        let depth = tok.depth;
        // The end of their parentheses.
        while pending.last().is_some_and(|p| p.depth > depth) {
            pending.pop();
        }
        let wj = t.word(j);
        let semicolon = t.is(Some(tok), b';');
        let statement_word = mssql
            && wj.is_some_and(|w| TSQL_STATEMENTS.contains(&w))
            && !t.is(t.before(j, 1), b'.');
        // Only an event looks at the entries at its depth, and a `;` or WHERE
        // settles them all, so this stays linear.
        if semicolon || wj == Some("WHERE") || statement_word {
            let same = pending
                .iter()
                .rev()
                .take_while(|p| p.depth == depth)
                .count();
            let at = pending.len() - same;
            if semicolon {
                pending.truncate(at);
            } else if wj == Some("WHERE") {
                for p in pending.drain(at..) {
                    out[p.k] = true;
                }
            } else {
                // A new statement, except for UPDATE's own first SET.
                let set = wj == Some("SET");
                let mut k = at;
                while k < pending.len() {
                    let p = &mut pending[k];
                    if set && p.update && !p.saw_set {
                        p.saw_set = true;
                        k += 1;
                    } else {
                        pending.remove(k);
                    }
                }
            }
        }
        if let Some(w @ ("DELETE" | "UPDATE")) = t.word(j) {
            pending.push(Pending {
                k: j,
                depth,
                update: w == "UPDATE",
                saw_set: false,
            });
        }
    }
    out
}

/// The table of the first top-level `FROM` (not in parentheses, not after a
/// `.`), for inline editing (fix 12). A name is a word or the engine's quoted
/// name, unquoted; `schema.table` gives both. Nothing for a subquery, a table
/// function, a string, or a name of three parts or more.
pub fn table_from_select(sql: &str, engine: SqlEngine) -> Option<TableRef> {
    let toks = code_tokens(sql, engine);
    let from = toks.iter().enumerate().position(|(i, t)| {
        t.depth == 0
            && t.kind == TokenKind::Word
            && t.text(sql).to_uppercase() == "FROM"
            && (i == 0 || !is_punct(sql, toks.get(i - 1), b'.'))
    })?;
    let mut parts: Vec<String> = Vec::new();
    let mut i = from + 1;
    loop {
        let name = unquote_name(sql, toks.get(i)?, engine, false)?;
        parts.push(name);
        i += 1;
        if !is_punct(sql, toks.get(i), b'.') {
            break;
        }
        i += 1;
    }
    if is_punct(sql, toks.get(i), b'(') {
        return None;
    }
    let mut parts = parts.into_iter();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(table), None, _) => Some(TableRef {
            schema: None,
            table,
        }),
        (Some(schema), Some(table), None) => Some(TableRef {
            schema: Some(schema),
            table,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PG: SqlEngine = SqlEngine::Postgres;

    #[test]
    fn sees_through_strings_and_comments() {
        use DestructiveReason::*;
        assert_eq!(
            destructive_reason("DELETE FROM t -- WHERE id = 1", PG),
            Some(DeleteNoWhere)
        );
        assert_eq!(
            destructive_reason("UPDATE t SET note = 'see WHERE'", PG),
            Some(UpdateNoWhere)
        );
        assert_eq!(
            destructive_reason("# c\nDELETE FROM t", SqlEngine::Mysql),
            Some(DeleteNoWhere)
        );
        assert_eq!(
            destructive_reason("SELECT 1 DELETE FROM t", SqlEngine::Mssql),
            Some(DeleteNoWhere)
        );
        assert_eq!(destructive_reason("SELECT 1 DELETE FROM t", PG), None);
        assert_eq!(
            destructive_reason(
                "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d WHERE 1=1",
                PG
            ),
            Some(DeleteNoWhere)
        );
        assert_eq!(
            destructive_reason("EXPLAIN (COSTS) DELETE FROM t", PG),
            None
        );
        assert_eq!(
            destructive_reason("EXPLAIN (ANALYZE) DELETE FROM t", PG),
            Some(DeleteNoWhere)
        );
        assert_eq!(
            destructive_reason("ALTER TABLE t DROP c", SqlEngine::Mysql),
            Some(DropColumn)
        );
        assert_eq!(
            destructive_reason("ALTER TABLE t DROP CONSTRAINT c", PG),
            None
        );
        assert_eq!(
            destructive_reason("DROP MATERIALIZED VIEW v", PG),
            Some(DropView)
        );
    }

    #[test]
    fn table_of_the_first_top_level_from() {
        let t = |sql, e| table_from_select(sql, e);
        assert_eq!(
            t("SELECT EXTRACT(YEAR FROM created_at) FROM orders", PG),
            Some(TableRef {
                schema: None,
                table: "orders".into()
            })
        );
        assert_eq!(
            t("SELECT * FROM [dbo].[a]]b]", SqlEngine::Mssql),
            Some(TableRef {
                schema: Some("dbo".into()),
                table: "a]b".into()
            })
        );
        assert_eq!(t("SELECT * FROM (SELECT 1) x", PG), None);
        assert_eq!(t("SELECT * FROM f(1)", PG), None);
        assert_eq!(t("SELECT * FROM a.b.c", PG), None);
        assert_eq!(t("SELECT * FROM 'x'", PG), None);
    }

    #[test]
    fn query_type_from_the_first_token() {
        assert_eq!(query_type("/* /* */ */ select 1", PG), QueryType::Select);
        assert_eq!(query_type("SELECTED", PG), QueryType::Other);
        assert_eq!(
            query_type("/*!DELETE FROM t */", SqlEngine::Mysql),
            QueryType::Delete
        );
        assert_eq!(query_type("", PG), QueryType::Other);
    }

    /// The model's scans, one per DELETE/UPDATE (`hasWhere`) and per `)`
    /// (`explainWithoutAnalyze`), to check the one-pass tables against.
    fn naive_has_where(d: &Destructive, k: usize) -> bool {
        let depth = d.t.toks[k].depth;
        let mut saw_set = false;
        for j in k + 1..d.t.len() {
            let t = &d.t.toks[j];
            if t.depth < depth || (t.depth == depth && d.t.is(Some(t), b';')) {
                return false;
            }
            if t.depth != depth {
                continue;
            }
            let wj = d.w(j);
            if wj == Some("WHERE") {
                return true;
            }
            if d.mssql
                && wj.is_some_and(|w| TSQL_STATEMENTS.contains(&w))
                && !d.t.is(d.t.before(j, 1), b'.')
            {
                if wj == Some("SET") && d.w(k) == Some("UPDATE") && !saw_set {
                    saw_set = true;
                    continue;
                }
                return false;
            }
        }
        false
    }

    fn naive_explain_without_analyze(d: &Destructive, k: usize) -> bool {
        let Some(close) = k.checked_sub(1) else {
            return false;
        };
        let depth = d.t.toks[close].depth;
        let mut analyze = false;
        let mut j = close;
        loop {
            let Some(prev) = j.checked_sub(1) else {
                return false;
            };
            j = prev;
            let t = &d.t.toks[j];
            if d.t.is(Some(t), b'(') && t.depth == depth {
                break;
            }
            if d.w(j) == Some("ANALYZE") {
                analyze = true;
            }
        }
        j > 0 && d.w(j - 1) == Some("EXPLAIN") && !analyze
    }

    /// The one-pass tables give the model's answer for every token of
    /// 20,000 random statements built from the words the scans react to.
    #[test]
    fn one_pass_tables_match_the_scans() {
        let words = [
            "DELETE", "UPDATE", "SET", "WHERE", ";", "(", ")", "EXPLAIN", "ANALYZE", "x", ".",
            "SELECT", "t",
        ];
        let mut seed: u64 = 0x5eed;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as usize
        };
        for _ in 0..20_000 {
            let len = next() % 24;
            let sql: Vec<&str> = (0..len).map(|_| words[next() % words.len()]).collect();
            let sql = sql.join(" ");
            for engine in [SqlEngine::Postgres, SqlEngine::Mssql] {
                let d = Destructive {
                    t: Tokens::new(&sql, code_tokens(&sql, engine)),
                    engine,
                    mssql: engine == SqlEngine::Mssql,
                    no_drop_before: Cell::new(0),
                    has_where: Vec::new(),
                    open_of: Vec::new(),
                    analyze: Vec::new(),
                }
                .prepared();
                for k in 0..d.t.len() {
                    if matches!(d.w(k), Some("DELETE" | "UPDATE")) {
                        assert_eq!(
                            d.has_where(k),
                            naive_has_where(&d, k),
                            "{sql:?} [{engine}] at {k}"
                        );
                    }
                    // Only asked after a `)`, as `dml_start` does.
                    if !d.t.is(d.t.before(k, 1), b')') {
                        continue;
                    }
                    assert_eq!(
                        d.explain_without_analyze(k),
                        naive_explain_without_analyze(&d, k),
                        "{sql:?} [{engine}] at {k}"
                    );
                }
            }
        }
    }
}
