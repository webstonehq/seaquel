//! The one SQL tokenizer, a port of `src/lib/engine/sql-scan.ts` (Task 3;
//! deleted in phase 2b), and what sits on it: statement splitting, statement
//! at cursor, row-limit detection and the count query.
//!
//! Each engine's quoting (the TS header, unchanged): `'…'` strings everywhere
//! (`''` inside); Postgres and DuckDB `"…"` names, `E'…'` strings with
//! backslash escapes and `$tag$…$tag$` strings; MySQL and MariaDB `` `…` ``
//! names, `"…"` strings, backslash escapes and `#` comments; SQLite `"…"`,
//! `` `…` `` and `[…]` names; SQL Server `"…"` and `[…]` names (`]]` inside).
//! Block comments nest on Postgres, DuckDB and SQL Server.
//!
//! Where the port differs from `sqlTokens` (the rules are in the recorder's
//! `scan-model.ts`, `docs/plans/artifacts/2026-09-27-sql-recorder-scan-model.ts.txt`):
//!
//! - Fix 18: a number token ends where the numeric literal ends (digits, an
//!   optional `.` and digits, an optional exponent), so `1INTO` is `1` and
//!   `INTO`. On SQL Server `0x…` and money (`$1.5`) are numbers too. On MySQL
//!   and MariaDB a plain digit run that runs into a word character is a name
//!   (`2fa_codes`), a digit-led run after a `.` is a name (`db.2fa_codes`),
//!   and `@` is a word character only at the start of a word.
//! - Fix 10: a Postgres/DuckDB `--` comment ends at `\r` as well as `\n`; a
//!   MySQL/MariaDB `--` starts a comment only before ASCII space, a control
//!   character or the end; a `$tag$` may be any length.
//! - Fix 19: word characters follow each engine's identifier rule. Postgres,
//!   DuckDB and SQLite take any non-ASCII char, MySQL/MariaDB any non-ASCII
//!   BMP char, SQL Server `[\p{L}\p{N}\p{M}]` ([`crate::js_word`]). A
//!   `$tag$` takes any non-ASCII char. [`ScanOptions::ts_words`] keeps the TS
//!   rule (BMP `[\p{L}\p{N}]`) for the read-only check's second reading.
//! - [`ScanOptions`]: a MySQL/MariaDB executable comment read as code, and the
//!   other sql_mode / `standard_conforming_strings` readings the read-only
//!   check scans.
//! - Comments are tokens too ([`TokenKind::Comment`]), which [`tokens`]
//!   leaves out.
//!
//! The TS walks UTF-16 code units; this walks UTF-8 bytes. Every delimiter is
//! ASCII, and no byte of a multi-byte UTF-8 sequence is ASCII, so the two agree
//! on every boundary. A char that isn't a word character is one punctuation
//! token, as in the model (a surrogate pair stays whole). Whitespace is
//! JavaScript's `/\s/`.
//!
//! Byte offsets, `end` exclusive. Every function is total: unterminated
//! strings, comments and names run to the end of the input.

use std::ops::Range;

use crate::js_word::{is_letter, is_mssql_word_char, is_ts_word_char};
use crate::js_ws::{is_js_space, js_trim, js_trim_end};
use crate::SqlEngine;

/// What a [`Token`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Keyword, name or number.
    Word,
    /// String literal or quoted name.
    Quoted,
    Punct,
    /// Line or block comment, including what ends it (the newline of a line
    /// comment). Also the opener (`/*!50700`, `/*M!`) and the closing `*/` of
    /// an executable comment read as code ([`ScanOptions::exec_comments`]),
    /// whose contents are scanned as SQL. [`tokens`] skips these.
    Comment,
}

/// A token as a byte range into the SQL it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
    /// Parenthesis depth the token is at (`(` and `)` themselves at the outer
    /// depth).
    pub depth: u32,
}

impl Token {
    pub fn text<'a>(&self, sql: &'a str) -> &'a str {
        &sql[self.start..self.end]
    }
}

/// How to read the input, beyond its engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanOptions {
    /// MySQL/MariaDB: an executable comment (`/*! … */`, `/*!50700 … */`, and
    /// `/*M! … */` on MariaDB only) is code: its opener and `*/` are comment
    /// tokens and what's inside is scanned as SQL. The splitter, the statement
    /// checks, the read-only check and forced inline read it this way;
    /// [`has_row_limit`] doesn't (a Follow-up).
    pub exec_comments: bool,
    /// MySQL/MariaDB with `NO_BACKSLASH_ESCAPES` and `ANSI_QUOTES`: no
    /// backslash escapes, and `"…"` is a name.
    pub ansi_mysql: bool,
    /// Postgres with `standard_conforming_strings = off`: `'…'` takes
    /// backslash escapes.
    pub pg_backslash: bool,
    /// The TS's word rule instead of fix 19's: a non-ASCII char is a word
    /// character only if it's a BMP letter or number (`[\p{L}\p{N}]` on one
    /// UTF-16 unit), and a `$tag$` takes `\p{L}` only. The read-only check
    /// reads every input with both rules.
    pub ts_words: bool,
}

/// A statement from [`split_statements`], in the TS splitter's shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// Position among the non-empty statements.
    pub index: usize,
    /// The TS `startOffset`: one past the previous top-level `;`, including a
    /// `;` that ended a dropped (empty or comment-only) statement; 0 for the
    /// first statement.
    pub start: usize,
    /// The byte offset of the terminator: the `;`, or `sql.len()` when there
    /// isn't one. Always a char boundary.
    ///
    /// The TS `endOffset` is `length - 1` for an unterminated last statement.
    /// That mapping (`end == sql.len()` to UTF-16 `len16 - 1`) is one line in
    /// `seaquel-wasm` and the parity test harness, never here: `len - 1` can
    /// land inside a multi-byte char. No TS caller reads `startOffset` or
    /// `endOffset`.
    pub end: usize,
    /// The trimmed statement text (the TS `sql`).
    pub text: Range<usize>,
}

struct Scanner<'a> {
    sql: &'a str,
    b: &'a [u8],
    engine: SqlEngine,
    mysql: bool,
    ansi: bool,
    pg_like: bool,
    nested_comments: bool,
    options: ScanOptions,
    comments: bool,
    tokens: Vec<Token>,
    /// The last significant token pushed: `(kind, end, is ".")`.
    last: Option<(TokenKind, usize, bool)>,
    depth: u32,
    i: usize,
}

impl<'a> Scanner<'a> {
    fn new(sql: &'a str, engine: SqlEngine, options: ScanOptions, comments: bool) -> Self {
        let mysql = engine.is_mysql();
        let pg_like = matches!(engine, SqlEngine::Postgres | SqlEngine::Duckdb);
        Scanner {
            sql,
            b: sql.as_bytes(),
            engine,
            mysql,
            ansi: mysql && options.ansi_mysql,
            pg_like,
            nested_comments: pg_like || engine == SqlEngine::Mssql,
            options,
            comments,
            tokens: Vec::new(),
            last: None,
            depth: 0,
            i: 0,
        }
    }

    fn at(&self, k: usize) -> Option<u8> {
        self.b.get(k).copied()
    }

    fn digit_at(&self, k: usize) -> bool {
        self.at(k).is_some_and(|c| c.is_ascii_digit())
    }

    /// The char at byte `k`, which must be a char boundary.
    fn char_at(&self, k: usize) -> Option<char> {
        self.sql.get(k..).and_then(|s| s.chars().next())
    }

    /// The byte length of the char at `k` (1 past the end or off a boundary).
    fn char_len(&self, k: usize) -> usize {
        self.char_at(k).map_or(1, char::len_utf8)
    }

    fn space_at(&self, k: usize) -> Option<usize> {
        match self.at(k)? {
            b'\t' | b'\n' | 0x0B | 0x0C | b'\r' | b' ' => Some(1),
            c if c < 0x80 => None,
            _ => self
                .char_at(k)
                .filter(|&c| is_js_space(c))
                .map(char::len_utf8),
        }
    }

    /// The length of the word character at `k`, if it is one. `inside`: not
    /// the first character of the word (on MySQL `@` only starts a word).
    fn word_char_at(&self, k: usize, inside: bool) -> Option<usize> {
        let c = self.at(k)?;
        if c < 0x80 {
            let word = c.is_ascii_alphanumeric()
                || c == b'_'
                || c == b'$'
                || (c == b'@' && !(self.mysql && inside))
                || (c == b'#' && !self.mysql);
            return word.then_some(1);
        }
        let ch = self.char_at(k)?;
        let word = if self.options.ts_words {
            is_ts_word_char(ch)
        } else {
            // Fix 19: each engine's identifier rule.
            match self.engine {
                // Their lexers take any byte >= 0x80.
                SqlEngine::Postgres | SqlEngine::Duckdb | SqlEngine::Sqlite => true,
                // Any BMP char; a char outside it isn't.
                SqlEngine::Mysql | SqlEngine::Mariadb => u32::from(ch) <= 0xFFFF,
                SqlEngine::Mssql => is_mssql_word_char(ch),
            }
        };
        word.then_some(ch.len_utf8())
    }

    fn push(&mut self, kind: TokenKind, end: usize, depth: u32) {
        if kind != TokenKind::Comment {
            let dot = kind == TokenKind::Punct && end == self.i + 1 && self.b[self.i] == b'.';
            self.last = Some((kind, end, dot));
        }
        if kind != TokenKind::Comment || self.comments {
            self.tokens.push(Token {
                kind,
                start: self.i,
                end,
                depth,
            });
        }
        self.i = end;
    }

    /// The end of a quoted run opening at `from` (after its closing quote).
    fn skip_quoted(&self, from: usize, open: u8, backslash: bool) -> usize {
        let close = if open == b'[' { b']' } else { open };
        // `[…]` escapes `]` as `]]` on SQL Server only; SQLite's has no escape.
        let doubles = open != b'[' || self.engine == SqlEngine::Mssql;
        let n = self.b.len();
        let mut j = from + 1;
        while j < n {
            let c = self.b[j];
            if backslash && c == b'\\' {
                // Skipping one byte past a multi-byte char is safe: no byte of
                // it is ASCII, so none can close the run.
                j += 2;
                continue;
            }
            if c == close {
                if doubles && self.at(j + 1) == Some(close) {
                    j += 2;
                    continue;
                }
                return j + 1;
            }
            j += 1;
        }
        n
    }

    /// The end of the line comment at `i`: after its `\n` (or `\r` on
    /// Postgres and DuckDB, fix 10), or the end of the input.
    fn line_comment_end(&self) -> usize {
        let rest = &self.b[self.i..];
        let eol = if self.pg_like {
            rest.iter().position(|&c| c == b'\n' || c == b'\r')
        } else {
            rest.iter().position(|&c| c == b'\n')
        };
        eol.map_or(self.b.len(), |k| self.i + k + 1)
    }

    fn block_comment_end(&self) -> usize {
        let n = self.b.len();
        let mut level = 1u32;
        let mut j = self.i + 2;
        while j < n && level > 0 {
            if self.nested_comments && self.b[j] == b'/' && self.at(j + 1) == Some(b'*') {
                level += 1;
                j += 2;
            } else if self.b[j] == b'*' && self.at(j + 1) == Some(b'/') {
                level -= 1;
                j += 2;
            } else {
                j += 1;
            }
        }
        j.min(n)
    }

    /// The length of a `$tag$` or `$$` opening at `i`: the model's
    /// `^\$(?:[A-Za-z_\u0080-\u{10FFFF}][\w\u0080-\u{10FFFF}]*)?\$` (fix 19:
    /// any non-ASCII char, as PG's lexer takes any byte >= 0x80), or with
    /// [`ScanOptions::ts_words`] the TS's `\p{L}` for non-ASCII. Any length
    /// (fix 10: the TS looked at 64 UTF-16 units only).
    fn dollar_tag(&self) -> Option<usize> {
        let mut j = self.i + 1;
        let mut first = true;
        loop {
            let c = self.char_at(j)?;
            if c == '$' {
                return Some(j + 1 - self.i);
            }
            let ok = if c.is_ascii() {
                c == '_' || c.is_ascii_alphabetic() || (!first && c.is_ascii_digit())
            } else {
                !self.options.ts_words || is_letter(c)
            };
            if !ok {
                return None;
            }
            first = false;
            j += c.len_utf8();
        }
    }

    /// Whether a numeric literal starts at `i` (fix 18).
    fn number_starts(&self, c: u8) -> bool {
        let starts = c.is_ascii_digit()
            || (c == b'.' && self.digit_at(self.i + 1))
            || (self.engine == SqlEngine::Mssql
                && c == b'$'
                && (self.digit_at(self.i + 1)
                    || (self.at(self.i + 1) == Some(b'.') && self.digit_at(self.i + 2))));
        if !starts {
            return false;
        }
        if self.mysql {
            if let Some((kind, end, dot)) = self.last {
                if end == self.i {
                    // A digit-led run after a `.` is a name (`db.2fa_codes`),
                    // and a `.` right after a name is the qualifier's dot.
                    if dot {
                        return false;
                    }
                    if c == b'.' && matches!(kind, TokenKind::Word | TokenKind::Quoted) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn number_end(&self, c: u8) -> usize {
        let n = self.b.len();
        let mut j = if c == b'$' { self.i + 1 } else { self.i };
        while self.digit_at(j) {
            j += 1;
        }
        let mut plain = true;
        if self.at(j) == Some(b'.') {
            plain = false;
            j += 1;
            while self.digit_at(j) {
                j += 1;
            }
        }
        if matches!(self.at(j), Some(b'e' | b'E')) {
            let sign = matches!(self.at(j + 1), Some(b'+' | b'-'));
            if self.digit_at(j + 1) || (sign && self.digit_at(j + 2)) {
                plain = false;
                j += if sign { 2 } else { 1 };
                while self.digit_at(j) {
                    j += 1;
                }
            }
        }
        // MySQL: digits running into a word character are a name (`2fa_codes`).
        if self.mysql && plain {
            while j < n {
                match self.word_char_at(j, true) {
                    Some(len) => j += len,
                    None => break,
                }
            }
        }
        j
    }

    fn run(mut self) -> Vec<Token> {
        let n = self.b.len();
        let mut in_exec = false;
        while self.i < n {
            let i = self.i;
            let c = self.b[i];
            let next = self.at(i + 1);
            let depth = self.depth;
            if let Some(len) = self.space_at(i) {
                self.i += len;
            } else if in_exec && c == b'*' && next == Some(b'/') {
                // The end of an executable comment.
                in_exec = false;
                self.push(TokenKind::Comment, i + 2, depth);
            } else if c == b'-'
                && next == Some(b'-')
                // Fix 10: MySQL/MariaDB start a `--` comment only before ASCII
                // space or a control character (`--` + NBSP is minus minus).
                && (!self.mysql || i + 2 >= n || matches!(self.b[i + 2], 0..=0x20 | 0x7F))
            {
                let end = self.line_comment_end();
                self.push(TokenKind::Comment, end, depth);
            } else if c == b'#' && self.mysql {
                let rest = &self.b[i..];
                let end = rest
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(n, |k| i + k + 1);
                self.push(TokenKind::Comment, end, depth);
            } else if c == b'/' && next == Some(b'*') {
                // `/*M!` is MariaDB's; MySQL reads it as a plain comment.
                let bang = self.at(i + 2) == Some(b'!');
                let maria = self.engine == SqlEngine::Mariadb
                    && self.at(i + 2) == Some(b'M')
                    && self.at(i + 3) == Some(b'!');
                if self.mysql && self.options.exec_comments && (bang || maria) && !in_exec {
                    let mut j = i + if maria { 4 } else { 3 };
                    while self.digit_at(j) {
                        j += 1;
                    }
                    in_exec = true;
                    self.push(TokenKind::Comment, j, depth);
                    continue;
                }
                let end = self.block_comment_end();
                self.push(TokenKind::Comment, end, depth);
            } else if c == b'\'' {
                let backslash = (self.mysql && !self.ansi)
                    || (self.engine == SqlEngine::Postgres && self.options.pg_backslash);
                let end = self.skip_quoted(i, b'\'', backslash);
                self.push(TokenKind::Quoted, end, depth);
            } else if self.is_name_quote(c) {
                let end = self.skip_quoted(i, c, self.mysql && !self.ansi && c == b'"');
                self.push(TokenKind::Quoted, end, depth);
            } else if self.mysql && c == b'"' {
                let end = self.skip_quoted(i, b'"', !self.ansi);
                self.push(TokenKind::Quoted, end, depth);
            } else if self.pg_like && (c == b'e' || c == b'E') && next == Some(b'\'') {
                let end = self.skip_quoted(i + 1, b'\'', true);
                self.push(TokenKind::Quoted, end, depth);
            } else if let Some(tag_len) = (self.pg_like && c == b'$')
                .then(|| self.dollar_tag())
                .flatten()
            {
                let tag = &self.sql[i..i + tag_len];
                let end = self.sql[i + tag_len..]
                    .find(tag)
                    .map_or(n, |k| i + tag_len + k + tag_len);
                self.push(TokenKind::Quoted, end, depth);
            } else if self.engine == SqlEngine::Mssql
                && c == b'0'
                && matches!(next, Some(b'x' | b'X'))
            {
                // Fix 18: a T-SQL binary literal `0x…`.
                let mut j = i + 2;
                while self.at(j).is_some_and(|c| c.is_ascii_hexdigit()) {
                    j += 1;
                }
                self.push(TokenKind::Word, j, depth);
            } else if self.number_starts(c) {
                let end = self.number_end(c);
                self.push(TokenKind::Word, end, depth);
            } else if let Some(len) = self.word_char_at(i, false) {
                let mut j = i + len;
                while let Some(len) = self.word_char_at(j, true) {
                    j += len;
                }
                self.push(TokenKind::Word, j, depth);
            } else if c == b'(' {
                self.push(TokenKind::Punct, i + 1, depth);
                self.depth += 1;
            } else if c == b')' {
                self.depth = self.depth.saturating_sub(1);
                let depth = self.depth;
                self.push(TokenKind::Punct, i + 1, depth);
            } else {
                let end = i + self.char_len(i);
                self.push(TokenKind::Punct, end, depth);
            }
        }
        self.tokens
    }

    fn is_name_quote(&self, c: u8) -> bool {
        match self.engine {
            SqlEngine::Mysql | SqlEngine::Mariadb => c == b'`' || (self.ansi && c == b'"'),
            SqlEngine::Sqlite => matches!(c, b'"' | b'`' | b'['),
            SqlEngine::Mssql => matches!(c, b'"' | b'['),
            SqlEngine::Postgres | SqlEngine::Duckdb => c == b'"',
        }
    }
}

/// Every token of `sql`, comments included, read with `options`.
pub fn scan(sql: &str, engine: SqlEngine, options: ScanOptions) -> Vec<Token> {
    Scanner::new(sql, engine, options, true).run()
}

/// The significant tokens of `sql` read with `options`: no comments.
pub(crate) fn significant(sql: &str, engine: SqlEngine, options: ScanOptions) -> Vec<Token> {
    Scanner::new(sql, engine, options, false).run()
}

/// The significant tokens of `sql`: no whitespace, no comments. Port of
/// `sqlTokens` (with fixes 18 and 10; a MySQL executable comment is a
/// comment).
pub fn tokens(sql: &str, engine: SqlEngine) -> Vec<Token> {
    significant(sql, engine, ScanOptions::default())
}

/// The significant tokens with executable comments read as code, as the
/// splitter and the statement checks read them.
pub(crate) fn code_tokens(sql: &str, engine: SqlEngine) -> Vec<Token> {
    significant(
        sql,
        engine,
        ScanOptions {
            exec_comments: true,
            ..ScanOptions::default()
        },
    )
}

/// Whether the token is the one-character punctuation `p`.
pub(crate) fn is_punct(sql: &str, t: Option<&Token>, p: u8) -> bool {
    t.is_some_and(|t| {
        t.kind == TokenKind::Punct && t.end == t.start + 1 && sql.as_bytes()[t.start] == p
    })
}

/// Splits on `;` tokens. Comment-only statements are dropped. Port of the fix
/// 10 model (the recorder's `split-model.ts`).
pub fn split_statements(sql: &str, engine: SqlEngine) -> Vec<Statement> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut has_code = false;
    let push = |statements: &mut Vec<Statement>, start: usize, end: usize| {
        let text = &sql[start..end];
        let trimmed = js_trim(text);
        let from = start + (text.len() - text.trim_start_matches(is_js_space).len());
        statements.push(Statement {
            index: statements.len(),
            start,
            end,
            text: from..from + trimmed.len(),
        });
    };
    for t in code_tokens(sql, engine) {
        if is_punct(sql, Some(&t), b';') {
            if has_code {
                push(&mut statements, start, t.start);
            }
            start = t.end;
            has_code = false;
        } else {
            has_code = true;
        }
    }
    if has_code {
        push(&mut statements, start, sql.len());
    }
    statements
}

/// The statement the cursor at `offset` is in, with the TS rules for a cursor
/// before, between and after statements (a statement holds the cursor when
/// `start <= offset <= end`).
///
/// Any offset is accepted, past the end or inside a char: it is only compared
/// with statement offsets, never used to slice `sql`.
pub fn statement_at(sql: &str, offset: usize, engine: SqlEngine) -> Option<Statement> {
    let statements = split_statements(sql, engine);
    let first = statements.first()?;
    let last = statements.last()?;
    if let Some(found) = statements
        .iter()
        .find(|s| offset >= s.start && offset <= s.end)
    {
        return Some(found.clone());
    }
    if offset < first.start {
        return Some(first.clone());
    }
    if offset > last.end {
        return Some(last.clone());
    }
    // Between statements: the next one.
    Some(
        statements
            .iter()
            .find(|s| s.start > offset)
            .unwrap_or(last)
            .clone(),
    )
}

/// Top-level keywords: upper-cased words at depth 0 that don't follow a `.`,
/// with their token.
fn top_level_words(sql: &str, tokens: &[Token]) -> Vec<(String, usize)> {
    let mut words = Vec::new();
    for (k, t) in tokens.iter().enumerate() {
        if t.depth != 0 || t.kind != TokenKind::Word {
            continue;
        }
        if k > 0 && is_punct(sql, tokens.get(k - 1), b'.') {
            continue;
        }
        words.push((t.text(sql).to_uppercase(), t.start));
    }
    words
}

/// Whether the words hold a row limit: LIMIT, OFFSET, FETCH FIRST/NEXT, or
/// TOP on SQL Server.
fn limits(words: &[(String, usize)], engine: SqlEngine) -> bool {
    words.iter().enumerate().any(|(k, (word, _))| {
        word == "LIMIT"
            || word == "OFFSET"
            || (word == "FETCH"
                && words
                    .get(k + 1)
                    .is_some_and(|(w, _)| w == "FIRST" || w == "NEXT"))
            || (engine == SqlEngine::Mssql && word == "TOP")
    })
}

/// Whether the query limits its own rows at the top level (LIMIT, OFFSET,
/// FETCH FIRST/NEXT, or TOP on SQL Server), so the runner shouldn't page it.
/// A limit in a subquery, a string, a quoted name or a comment doesn't count
/// (a MySQL executable comment is a comment here; a Follow-up).
pub fn has_row_limit(sql: &str, engine: SqlEngine) -> bool {
    limits(&top_level_words(sql, &tokens(sql, engine)), engine)
}

/// `sql` without its top-level trailing `ORDER BY` and what follows it, for a
/// row count that wraps the query in a derived table (SQL Server rejects
/// ORDER BY there). A query that limits its rows keeps its ORDER BY.
pub fn strip_trailing_order_by(sql: &str, engine: SqlEngine) -> &str {
    let words = top_level_words(sql, &tokens(sql, engine));
    if limits(&words, engine) {
        return sql;
    }
    for k in (0..words.len().saturating_sub(1)).rev() {
        if words[k].0 == "ORDER" && words[k + 1].0 == "BY" {
            return js_trim_end(&sql[..words[k].1]);
        }
    }
    sql
}

/// The query that counts `sql`'s rows: `SELECT COUNT(*) as total FROM (…) AS
/// count_query`, with the trailing ORDER BY stripped on SQL Server.
pub fn count_query(sql: &str, engine: SqlEngine) -> String {
    let inner = if engine == SqlEngine::Mssql {
        strip_trailing_order_by(sql, engine)
    } else {
        sql
    };
    format!("SELECT COUNT(*) as total FROM ({inner}) AS count_query")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sql: &str, engine: SqlEngine) -> Vec<&str> {
        tokens(sql, engine).iter().map(|t| t.text(sql)).collect()
    }

    #[test]
    fn number_tokens_end_with_the_literal() {
        let pg = SqlEngine::Postgres;
        assert_eq!(texts("1INTO", pg), ["1", "INTO"]);
        assert_eq!(texts("1.5e+3x", pg), ["1.5e+3", "x"]);
        assert_eq!(texts("1e", pg), ["1", "e"]);
        assert_eq!(texts(".5a", pg), [".5", "a"]);
        let my = SqlEngine::Mysql;
        assert_eq!(texts("1INTO 1.5INTO", my), ["1INTO", "1.5", "INTO"]);
        assert_eq!(texts("db.2fa_codes", my), ["db", ".", "2fa_codes"]);
        assert_eq!(texts("t.5", my), ["t", ".", "5"]);
        assert_eq!(texts("INTO@x @y", my), ["INTO", "@x", "@y"]);
        let ms = SqlEngine::Mssql;
        assert_eq!(texts("a=0x1fINTO", ms), ["a", "=", "0x1f", "INTO"]);
        assert_eq!(texts("a=$1.5INTO", ms), ["a", "=", "$1.5", "INTO"]);
        assert_eq!(texts("#temp", ms), ["#temp"]);
    }

    #[test]
    fn quoting_per_engine() {
        assert_eq!(texts("[a]]b]", SqlEngine::Mssql), ["[a]]b]"]);
        assert_eq!(texts("[a]]b]", SqlEngine::Sqlite), ["[a]", "]", "b", "]"]);
        assert_eq!(texts("x#c\ny", SqlEngine::Mysql), ["x", "y"]);
        assert_eq!(texts("E'\\''x", SqlEngine::Postgres), ["E'\\''", "x"]);
        assert_eq!(texts("$1 $a$;$a$", SqlEngine::Duckdb), ["$1", "$a$;$a$"]);
        assert_eq!(texts("/* /* */ x */ y", SqlEngine::Postgres), ["y"]);
        assert_eq!(
            texts("/* /* */ x */ y", SqlEngine::Mysql),
            ["x", "*", "/", "y"]
        );
        assert_eq!(texts("\"a\\\"b\" c", SqlEngine::Mysql), ["\"a\\\"b\"", "c"]);
        assert_eq!(texts("-- a\rb", SqlEngine::Postgres), ["b"]);
        assert_eq!(texts("-- a\rb", SqlEngine::Sqlite), Vec::<&str>::new());
    }

    #[test]
    fn depth_and_comments() {
        let sql = "(a /* c */ (b)) -- d";
        let all = scan(sql, SqlEngine::Postgres, ScanOptions::default());
        let shape: Vec<(&str, u32)> = all.iter().map(|t| (t.text(sql), t.depth)).collect();
        assert_eq!(
            shape,
            [
                ("(", 0),
                ("a", 1),
                ("/* c */", 1),
                ("(", 1),
                ("b", 2),
                (")", 1),
                (")", 0),
                ("-- d", 0)
            ]
        );
    }

    #[test]
    fn executable_comments() {
        let sql = "SELECT 1 /*!50700 ; DELETE */";
        let e = SqlEngine::Mysql;
        assert_eq!(texts(sql, e), ["SELECT", "1"]);
        let code: Vec<&str> = code_tokens(sql, e).iter().map(|t| t.text(sql)).collect();
        assert_eq!(code, ["SELECT", "1", ";", "DELETE"]);
        let maria = "/*M! DELETE */";
        assert!(code_tokens(maria, SqlEngine::Mysql).is_empty());
        assert_eq!(code_tokens(maria, SqlEngine::Mariadb).len(), 1);
    }

    #[test]
    fn splits_with_the_ts_offsets() {
        let sql = " a; ;-- c\n; b ";
        let s = split_statements(sql, SqlEngine::Postgres);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].start, s[0].end, &sql[s[0].text.clone()]), (0, 2, "a"));
        assert_eq!(
            (s[1].start, s[1].end, &sql[s[1].text.clone()]),
            (11, 14, "b")
        );
        assert_eq!(statement_at(sql, 5, SqlEngine::Postgres).unwrap().index, 1);
    }
}
