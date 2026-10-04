//! The SQL editor (Q7 A, Decision 14): `ratatui-textarea` keeps the buffer,
//! the cursor and undo; this layer adds the modes (Insert by default, Esc to
//! Normal with `h j k l w b e 0 $ gg G dd yy p u x i a o O`, `:` commands),
//! the scroll position the view draws from, and highlighting from
//! `seaquel_core::sql`'s scanner with the connection's engine, so what is
//! coloured as a string or a comment is what Core splits and runs.
//!
//! The view draws the lines itself (the textarea has no per-token styling):
//! tabs expand to [`TAB_WIDTH`], control characters show as `�`, and wide
//! characters are measured with `unicode-width`.
//!
//! Everything here is pure; the text never reaches `Debug` or a log.

use std::fmt;

use crossterm::event::KeyCode;
use ratatui_textarea::{CursorMove, TextArea};
use seaquel_core::sql::scan::{scan, ScanOptions, TokenKind};
use seaquel_core::sql::SqlEngine;
use unicode_width::UnicodeWidthChar;

use super::completion::Completion;

/// Columns a tab advances to the next multiple of.
pub const TAB_WIDTH: usize = 4;

/// Above this many bytes, highlighting is redone on the next tick instead of
/// on each key (a 2 MB text then types without a scan per key).
pub const HIGHLIGHT_NOW_BYTES: usize = 256 * 1024;

/// The editor's mode (Q7 A).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Insert,
    Normal,
}

/// A Normal-mode command (each a binding in the keymap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normal {
    Left,
    Down,
    Up,
    Right,
    WordForward,
    WordBack,
    WordEnd,
    LineStart,
    LineEnd,
    /// `g`: `gg` goes to the first line.
    G,
    Bottom,
    /// `d`: `dd` deletes the line.
    D,
    /// `y`: `yy` yanks the line.
    Y,
    Paste,
    DeleteChar,
    Undo,
    Insert,
    Append,
    AppendEnd,
    InsertStart,
    OpenBelow,
    OpenAbove,
}

/// How a token is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Keyword,
    /// A function's name (a word right before `(`).
    Function,
    String,
    /// A quoted name (`"Order"`, `` `order` ``, `[order]`).
    Name,
    Number,
    Comment,
}

/// A coloured byte range of one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlSpan {
    pub start: usize,
    pub end: usize,
    pub class: Class,
}

/// The highlighting for one version of the text.
#[derive(Clone, PartialEq, Eq)]
pub struct Highlighted {
    pub gen: u64,
    pub engine: SqlEngine,
    /// One list per line.
    pub lines: Vec<Vec<HlSpan>>,
}

/// What `yy`, `dd` and `x` keep for `p`.
#[derive(Clone, PartialEq, Eq)]
struct Register {
    text: String,
    linewise: bool,
}

/// The editor of one query tab.
#[derive(Clone)]
pub struct Editor {
    area: TextArea<'static>,
    pub mode: Mode,
    /// A Normal-mode prefix waiting for its second key (`g`, `d`, `y`).
    pub pending: Option<char>,
    /// A `:` command being typed.
    pub command: Option<String>,
    register: Option<Register>,
    /// Moves on with every change of the text.
    gen: u64,
    /// Every line ended in `\r\n` when the text was set: the lines are kept
    /// without the `\r` and [`Editor::text`] joins them with `\r\n` again
    /// (review M3). A text with mixed endings is read as `\n`.
    crlf: bool,
    /// The first line and display column drawn.
    pub top: usize,
    pub left: usize,
    /// The last highlighting (for [`Editor::gen`] when it's current).
    pub highlight: Option<Highlighted>,
    /// The completion popup, when open.
    pub completion: Option<Completion>,
}

impl fmt::Debug for Editor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Editor")
            .field("lines", &self.area.lines().len())
            .field("mode", &self.mode)
            .field("gen", &self.gen)
            .finish_non_exhaustive()
    }
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new("")
    }
}

impl Editor {
    /// An editor holding `text` (`\r\n` read as a newline), in Insert mode
    /// with the cursor at the start.
    pub fn new(text: &str) -> Editor {
        let crlf = is_crlf(text);
        let lines: Vec<String> = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
            .collect();
        let mut area = TextArea::new(lines);
        area.set_tab_length(TAB_WIDTH as u8);
        area.set_hard_tab_indent(false);
        area.set_max_histories(200);
        Editor {
            area,
            mode: Mode::Insert,
            pending: None,
            command: None,
            register: None,
            gen: 0,
            crlf,
            top: 0,
            left: 0,
            highlight: None,
            completion: None,
        }
    }

    /// Whether the text is exactly `other` (no join: the `modified` check
    /// runs on every key).
    pub fn text_is(&self, other: &str) -> bool {
        let separator = self.separator();
        let lines = self.area.lines();
        let mut rest = other;
        for (i, line) in lines.iter().enumerate() {
            let Some(after) = rest.strip_prefix(line.as_str()) else {
                return false;
            };
            rest = after;
            if i + 1 < lines.len() {
                let Some(after) = rest.strip_prefix(separator) else {
                    return false;
                };
                rest = after;
            }
        }
        rest.is_empty()
    }

    fn separator(&self) -> &'static str {
        if self.crlf {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// The text, lines joined with `\n` (`\r\n` for a CRLF text).
    pub fn text(&self) -> String {
        self.area.lines().join(self.separator())
    }

    pub fn lines(&self) -> &[String] {
        self.area.lines()
    }

    /// Moves on with every change of the text.
    pub fn gen(&self) -> u64 {
        self.gen
    }

    /// The cursor: line and character (not byte) column.
    pub fn cursor(&self) -> (usize, usize) {
        let c = self.area.cursor();
        (c.0, c.1)
    }

    /// The cursor's byte offset on its line.
    fn cursor_line_byte(&self) -> usize {
        let (row, col) = self.cursor();
        let line = &self.area.lines()[row];
        line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
    }

    /// The cursor as a byte offset into [`Editor::text`].
    pub fn cursor_byte(&self) -> usize {
        let (row, _) = self.cursor();
        let separator = self.separator().len();
        let before: usize = self.area.lines()[..row]
            .iter()
            .map(|l| l.len() + separator)
            .sum();
        before + self.cursor_line_byte()
    }

    /// The cursor as a UTF-16 offset into [`Editor::text`] (`db.run`'s
    /// `Current` target, Monaco's unit).
    pub fn cursor_utf16(&self) -> usize {
        let (row, _) = self.cursor();
        let lines = self.area.lines();
        let separator = self.separator().len();
        let before: usize = lines[..row]
            .iter()
            .map(|l| seaquel_core::sql::offsets::utf16_len(l) + separator)
            .sum();
        before + seaquel_core::sql::offsets::utf16_len(&lines[row][..self.cursor_line_byte()])
    }

    /// The cursor's display column on its line (tabs and wide characters
    /// counted as drawn).
    pub fn cursor_column(&self) -> usize {
        let (row, _) = self.cursor();
        display_width(&self.area.lines()[row][..self.cursor_line_byte()])
    }

    fn changed(&mut self) {
        self.gen += 1;
    }

    /// Replaces the whole text (one undo step), keeping the cursor's line
    /// where it can.
    pub fn set_text(&mut self, text: &str) {
        let (row, _) = self.cursor();
        self.crlf = is_crlf(text);
        self.area.select_all();
        self.area.insert_str(text);
        self.area.move_cursor(CursorMove::Top);
        for _ in 0..row.min(self.area.lines().len().saturating_sub(1)) {
            self.area.move_cursor(CursorMove::Down);
        }
        self.area.move_cursor(CursorMove::Head);
        self.completion = None;
        self.changed();
    }

    /// Types a character at the cursor (Insert mode).
    pub fn type_char(&mut self, c: char) {
        self.area.insert_char(c);
        self.changed();
    }

    /// Inserts `text` at the cursor.
    pub fn insert_str(&mut self, text: &str) {
        if self.area.insert_str(text) {
            self.changed();
        }
    }

    /// Moves the cursor to line `row`, character `col` (clamped to the
    /// text): where Ask AI's inserted statement ends.
    pub fn jump(&mut self, row: usize, col: usize) {
        let row = u16::try_from(row).unwrap_or(u16::MAX);
        let col = u16::try_from(col).unwrap_or(u16::MAX);
        self.area.move_cursor(CursorMove::Jump(row, col));
    }

    /// Deletes `chars` characters before the cursor on its line (the
    /// completion's prefix before its item goes in).
    pub fn delete_before(&mut self, chars: usize) {
        let mut any = false;
        for _ in 0..chars.min(self.cursor().1) {
            any |= self.area.delete_char();
        }
        if any {
            self.changed();
        }
    }

    /// Indents to the next tab stop with spaces.
    pub fn indent(&mut self) {
        if self.area.insert_tab() {
            self.changed();
        }
    }

    /// An editing key in Insert mode (Enter, Backspace, Delete, the arrows,
    /// Home, End, Page Up and Down); `false` for any other key.
    pub fn insert_key(&mut self, code: KeyCode) -> bool {
        let edit = match code {
            KeyCode::Enter => {
                self.area.insert_newline();
                true
            }
            KeyCode::Backspace => self.area.delete_char(),
            KeyCode::Delete => self.area.delete_next_char(),
            _ => {
                let motion = match code {
                    KeyCode::Left => CursorMove::Back,
                    KeyCode::Right => CursorMove::Forward,
                    KeyCode::Up => CursorMove::Up,
                    KeyCode::Down => CursorMove::Down,
                    KeyCode::Home => CursorMove::Head,
                    KeyCode::End => CursorMove::End,
                    KeyCode::PageUp => {
                        for _ in 0..PAGE_LINES {
                            self.area.move_cursor(CursorMove::Up);
                        }
                        return true;
                    }
                    KeyCode::PageDown => {
                        for _ in 0..PAGE_LINES {
                            self.area.move_cursor(CursorMove::Down);
                        }
                        return true;
                    }
                    _ => return false,
                };
                self.area.move_cursor(motion);
                return true;
            }
        };
        if edit {
            self.changed();
        }
        true
    }

    /// The line the cursor is on.
    fn line(&self) -> &str {
        &self.area.lines()[self.cursor().0]
    }

    /// A Normal-mode command; `true` when it changed the text.
    pub fn normal(&mut self, command: Normal) -> bool {
        let pending = self.pending.take();
        let changed = match command {
            Normal::G if pending == Some('g') => {
                self.area.move_cursor(CursorMove::Top);
                self.area.move_cursor(CursorMove::Head);
                false
            }
            Normal::G => {
                self.pending = Some('g');
                false
            }
            Normal::D if pending == Some('d') => self.delete_line(),
            Normal::D => {
                self.pending = Some('d');
                false
            }
            Normal::Y if pending == Some('y') => {
                self.register = Some(Register {
                    text: self.line().to_string(),
                    linewise: true,
                });
                false
            }
            Normal::Y => {
                self.pending = Some('y');
                false
            }
            Normal::Left => self.motion(CursorMove::Back, true),
            Normal::Right => self.motion(CursorMove::Forward, true),
            Normal::Up => self.motion(CursorMove::Up, false),
            Normal::Down => self.motion(CursorMove::Down, false),
            Normal::WordForward => self.motion(CursorMove::WordForward, false),
            Normal::WordBack => self.motion(CursorMove::WordBack, false),
            Normal::WordEnd => self.motion(CursorMove::WordEnd, false),
            Normal::LineStart => self.motion(CursorMove::Head, false),
            Normal::LineEnd => self.motion(CursorMove::End, false),
            Normal::Bottom => {
                self.area.move_cursor(CursorMove::Bottom);
                self.area.move_cursor(CursorMove::Head);
                false
            }
            Normal::Paste => self.paste(),
            Normal::DeleteChar => {
                let (row, col) = self.cursor();
                let ch = self.area.lines()[row].chars().nth(col);
                match ch {
                    Some(c) if self.area.delete_next_char() => {
                        self.register = Some(Register {
                            text: c.to_string(),
                            linewise: false,
                        });
                        true
                    }
                    _ => false,
                }
            }
            Normal::Undo => self.area.undo(),
            Normal::Insert => {
                self.mode = Mode::Insert;
                false
            }
            Normal::Append => {
                if self.cursor().1 < self.line().chars().count() {
                    self.area.move_cursor(CursorMove::Forward);
                }
                self.mode = Mode::Insert;
                false
            }
            Normal::AppendEnd => {
                self.area.move_cursor(CursorMove::End);
                self.mode = Mode::Insert;
                false
            }
            Normal::InsertStart => {
                self.area.move_cursor(CursorMove::Head);
                self.mode = Mode::Insert;
                false
            }
            Normal::OpenBelow => {
                self.area.move_cursor(CursorMove::End);
                self.area.insert_newline();
                self.mode = Mode::Insert;
                true
            }
            Normal::OpenAbove => {
                self.area.move_cursor(CursorMove::Head);
                self.area.insert_newline();
                self.area.move_cursor(CursorMove::Up);
                self.mode = Mode::Insert;
                true
            }
        };
        if changed {
            self.changed();
        }
        changed
    }

    /// A move that stays on the line when `within_line` (`h`, `l`).
    fn motion(&mut self, motion: CursorMove, within_line: bool) -> bool {
        let (_, col) = self.cursor();
        if within_line {
            let len = self.line().chars().count();
            if (matches!(motion, CursorMove::Back) && col == 0)
                || (matches!(motion, CursorMove::Forward) && col >= len)
            {
                return false;
            }
        }
        self.area.move_cursor(motion);
        false
    }

    /// `dd`: the cursor's line goes to the register, as one undo step.
    fn delete_line(&mut self) -> bool {
        let (row, _) = self.cursor();
        let count = self.area.lines().len();
        let text = self.line().to_string();
        if row + 1 < count {
            self.area.move_cursor(CursorMove::Head);
            self.area.start_selection();
            self.area.move_cursor(CursorMove::Down);
            self.area.move_cursor(CursorMove::Head);
        } else if row > 0 {
            self.area.move_cursor(CursorMove::Up);
            self.area.move_cursor(CursorMove::End);
            self.area.start_selection();
            self.area.move_cursor(CursorMove::Down);
            self.area.move_cursor(CursorMove::End);
        } else {
            self.area.move_cursor(CursorMove::Head);
            self.area.start_selection();
            self.area.move_cursor(CursorMove::End);
        }
        let cut = self.area.cut();
        self.area.cancel_selection();
        if row + 1 >= count && row > 0 {
            self.area.move_cursor(CursorMove::Head);
        }
        self.register = Some(Register {
            text,
            linewise: true,
        });
        cut
    }

    /// `p`: a line below the cursor's (linewise), else after the cursor.
    fn paste(&mut self) -> bool {
        let Some(register) = self.register.clone() else {
            return false;
        };
        if register.linewise {
            self.area.move_cursor(CursorMove::End);
            self.area.insert_str(format!("\n{}", register.text));
            self.area.move_cursor(CursorMove::Head);
        } else {
            if self.cursor().1 < self.line().chars().count() {
                self.area.move_cursor(CursorMove::Forward);
            }
            self.area.insert_str(&register.text);
        }
        true
    }

    /// Keeps the cursor within `height` lines and `width` columns from
    /// [`Editor::top`] and [`Editor::left`].
    pub fn scroll_into_view(&mut self, height: usize, width: usize) {
        let row = self.cursor().0;
        let height = height.max(1);
        if row < self.top {
            self.top = row;
        } else if row >= self.top + height {
            self.top = row + 1 - height;
        }
        let column = self.cursor_column();
        let width = width.max(1);
        if column < self.left {
            self.left = column;
        } else if column >= self.left + width {
            self.left = column + 1 - width;
        }
    }

    /// Highlights the text again unless [`Editor::highlight`] is current.
    pub fn refresh_highlight(&mut self, engine: SqlEngine) {
        if self.highlight_current(engine) {
            return;
        }
        self.highlight = Some(Highlighted {
            gen: self.gen,
            engine,
            lines: highlight(&self.text(), engine),
        });
    }

    /// Whether [`Editor::highlight`] is for the text as it is.
    pub fn highlight_current(&self, engine: SqlEngine) -> bool {
        self.highlight
            .as_ref()
            .is_some_and(|h| h.gen == self.gen && h.engine == engine)
    }
}

/// Whether every line of `text` ends in `\r\n` (and there's at least one).
fn is_crlf(text: &str) -> bool {
    let newlines = text.matches('\n').count();
    newlines > 0 && text.matches("\r\n").count() == newlines
}

/// Lines Page Up and Page Down move.
const PAGE_LINES: usize = 10;

/// The display width of one character as the editor draws it: a tab to the
/// next stop from `column`, a control character as `�` (1).
pub fn char_width(c: char, column: usize) -> usize {
    match c {
        '\t' => TAB_WIDTH - column % TAB_WIDTH,
        c if c.is_control() => 1,
        c => UnicodeWidthChar::width(c).unwrap_or(0),
    }
}

/// The display width of `text` drawn from column 0.
pub fn display_width(text: &str) -> usize {
    text.chars().fold(0, |col, c| col + char_width(c, col))
}

/// SQL words the editor colours as keywords (and the completion offers).
pub const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "ANALYZE",
    "AND",
    "AS",
    "ASC",
    "BEGIN",
    "BETWEEN",
    "BY",
    "CASE",
    "CAST",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DATABASE",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "EXPLAIN",
    "FALSE",
    "FETCH",
    "FILTER",
    "FIRST",
    "FOREIGN",
    "FROM",
    "FULL",
    "FUNCTION",
    "GRANT",
    "GROUP",
    "HAVING",
    "IF",
    "ILIKE",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LATERAL",
    "LEFT",
    "LIKE",
    "LIMIT",
    "MERGE",
    "NATURAL",
    "NEXT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "PARTITION",
    "PRAGMA",
    "PRIMARY",
    "PROCEDURE",
    "RECURSIVE",
    "REFERENCES",
    "REPLACE",
    "RETURNING",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "ROW",
    "ROWS",
    "SCHEMA",
    "SELECT",
    "SET",
    "SHOW",
    "TABLE",
    "THEN",
    "TOP",
    "TRIGGER",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USING",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
];

/// Whether `word` is one of [`KEYWORDS`], in any case.
pub fn is_keyword(word: &str) -> bool {
    let upper = word.to_ascii_uppercase();
    KEYWORDS.binary_search(&upper.as_str()).is_ok()
}

/// The highlighting of `text` read with `engine`'s quoting: one list of
/// spans per line (`text.split('\n')`), byte ranges into that line. A
/// string or comment that runs over several lines is a span on each.
pub fn highlight(text: &str, engine: SqlEngine) -> Vec<Vec<HlSpan>> {
    // Where each line starts.
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut lines: Vec<Vec<HlSpan>> = vec![Vec::new(); starts.len()];
    let options = ScanOptions {
        exec_comments: true,
        ..ScanOptions::default()
    };
    let tokens = scan(text, engine, options);
    let mut line = 0;
    for (k, t) in tokens.iter().enumerate() {
        let token = &text[t.start..t.end];
        let class = match t.kind {
            TokenKind::Comment => Class::Comment,
            TokenKind::Quoted => quoted_class(token, engine),
            TokenKind::Punct => continue,
            TokenKind::Word => {
                let first = token.chars().next().unwrap_or(' ');
                if first.is_ascii_digit() || (first == '.' && token.len() > 1) {
                    Class::Number
                } else if is_keyword(token) {
                    Class::Keyword
                } else if tokens
                    .get(k + 1)
                    .is_some_and(|n| n.kind == TokenKind::Punct && &text[n.start..n.end] == "(")
                {
                    Class::Function
                } else {
                    continue;
                }
            }
        };
        // Split the token at line ends; `line` only moves forward.
        while line + 1 < starts.len() && starts[line + 1] <= t.start {
            line += 1;
        }
        let mut at = t.start;
        let mut l = line;
        while at < t.end {
            let line_end = starts
                .get(l + 1)
                .map_or(text.len(), |next| next - 1)
                .min(t.end);
            if line_end > at {
                lines[l].push(HlSpan {
                    start: at - starts[l],
                    end: line_end - starts[l],
                    class,
                });
            }
            l += 1;
            match starts.get(l) {
                Some(next) => at = *next,
                None => break,
            }
        }
    }
    lines
}

/// A quoted token is a string or a name by its opening quote, as `engine`
/// reads it.
fn quoted_class(token: &str, engine: SqlEngine) -> Class {
    let first = token.chars().next().unwrap_or('\'');
    match first {
        '\'' | '$' => Class::String,
        '"' if engine.is_mysql() => Class::String,
        '"' | '`' | '[' => Class::Name,
        // `E'…'`, `N'…'`, `X'…'`, `B'…'`.
        _ => Class::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_text(text: &str, engine: SqlEngine) -> Vec<(String, Class)> {
        let lines: Vec<&str> = text.split('\n').collect();
        highlight(text, engine)
            .iter()
            .enumerate()
            .flat_map(|(i, spans)| {
                let line = lines[i];
                spans
                    .iter()
                    .map(move |s| (line[s.start..s.end].to_string(), s.class))
            })
            .collect()
    }

    fn has(spans: &[(String, Class)], text: &str, class: Class) -> bool {
        spans.iter().any(|(t, c)| t == text && *c == class)
    }

    #[test]
    fn keywords_are_sorted_for_the_lookup() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
        assert!(is_keyword("select") && is_keyword("From"));
        assert!(!is_keyword("invoices"));
    }

    // Q7 A: keywords, strings, comments, quoted names and numbers follow the
    // scanner's spans.
    #[test]
    fn highlighting_follows_the_scanner() {
        let sql = "SELECT c.name, sum(total) AS \"Total\" -- why\nFROM invoices WHERE id = 42 AND note = 'it''s' /* a\nb */";
        let spans = spans_text(sql, SqlEngine::Postgres);
        for (text, class) in [
            ("SELECT", Class::Keyword),
            ("AS", Class::Keyword),
            ("FROM", Class::Keyword),
            ("WHERE", Class::Keyword),
            ("sum", Class::Function),
            ("\"Total\"", Class::Name),
            ("-- why", Class::Comment),
            ("42", Class::Number),
            ("'it''s'", Class::String),
            ("/* a", Class::Comment),
            ("b */", Class::Comment),
        ] {
            assert!(has(&spans, text, class), "{text} {class:?}: {spans:?}");
        }
        assert!(
            !spans.iter().any(|(t, _)| t == "invoices" || t == "name"),
            "plain names aren't coloured: {spans:?}"
        );
    }

    #[test]
    fn dollar_quotes_on_postgres_and_backticks_on_mysql() {
        let sql = "SELECT $$it's -- not a comment$$, `x`";
        let pg = spans_text(sql, SqlEngine::Postgres);
        assert!(
            has(&pg, "$$it's -- not a comment$$", Class::String),
            "{pg:?}"
        );
        assert!(!pg.iter().any(|(_, c)| *c == Class::Comment), "{pg:?}");

        let sql = "SELECT `order`, \"text\", # comment\n1";
        let my = spans_text(sql, SqlEngine::Mysql);
        assert!(has(&my, "`order`", Class::Name), "{my:?}");
        assert!(has(&my, "\"text\"", Class::String), "{my:?}");
        assert!(has(&my, "# comment", Class::Comment), "{my:?}");

        let ms = spans_text("SELECT [order] FROM t", SqlEngine::Mssql);
        assert!(has(&ms, "[order]", Class::Name), "{ms:?}");

        // A `$$` on MySQL isn't a string: the quote inside starts one.
        let my = spans_text("SELECT $$it's$$", SqlEngine::Mysql);
        assert!(!has(&my, "$$it's$$", Class::String), "{my:?}");
    }

    #[test]
    fn an_unterminated_string_runs_to_the_end_over_lines() {
        let sql = "SELECT 'one\ntwo\nthree";
        let lines = highlight(sql, SqlEngine::Sqlite);
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[1],
            [HlSpan {
                start: 0,
                end: 3,
                class: Class::String
            }]
        );
        assert_eq!(
            lines[2],
            [HlSpan {
                start: 0,
                end: 5,
                class: Class::String
            }]
        );
    }

    fn typed(e: &mut Editor, text: &str) {
        for c in text.chars() {
            if c == '\n' {
                e.insert_key(KeyCode::Enter);
            } else {
                e.type_char(c);
            }
        }
    }

    // Q7 A: Insert by default; Esc (the keymap's) puts it in Normal.
    #[test]
    fn insert_mode_types_and_edits() {
        let mut e = Editor::default();
        assert_eq!(e.mode, Mode::Insert);
        typed(&mut e, "SELECT 1;\nSELECT 2");
        assert_eq!(e.text(), "SELECT 1;\nSELECT 2");
        assert_eq!(e.cursor(), (1, 8));
        let gen = e.gen();
        e.insert_key(KeyCode::Backspace);
        assert_eq!(e.text(), "SELECT 1;\nSELECT ");
        assert!(e.gen() > gen, "a change moves the generation");
        let gen = e.gen();
        assert!(e.insert_key(KeyCode::Left));
        assert!(e.insert_key(KeyCode::Home));
        assert_eq!(e.cursor(), (1, 0));
        assert_eq!(e.gen(), gen, "a move doesn't");
        assert!(e.insert_key(KeyCode::Up));
        assert!(e.insert_key(KeyCode::End));
        assert_eq!(e.cursor(), (0, 9));
        e.insert_key(KeyCode::Delete);
        assert_eq!(e.text(), "SELECT 1;SELECT ");
        assert!(!e.insert_key(KeyCode::F(2)));
        e.indent();
        assert_eq!(e.text(), "SELECT 1;   SELECT ", "to the next stop of 4");
    }

    #[test]
    fn normal_moves() {
        let mut e = Editor::new("select a, b\nfrom t\nwhere x");
        e.mode = Mode::Normal;
        e.normal(Normal::WordForward);
        assert_eq!(e.cursor(), (0, 7));
        e.normal(Normal::WordEnd);
        assert!(e.cursor().1 > 7, "past `a`: {:?}", e.cursor());
        e.normal(Normal::LineEnd);
        assert_eq!(e.cursor(), (0, 11));
        e.normal(Normal::WordBack);
        assert_eq!(e.cursor(), (0, 10));
        e.normal(Normal::LineStart);
        assert_eq!(e.cursor(), (0, 0));
        e.normal(Normal::Down);
        e.normal(Normal::Right);
        assert_eq!(e.cursor(), (1, 1));
        e.normal(Normal::Left);
        e.normal(Normal::Up);
        assert_eq!(e.cursor(), (0, 0));
        e.normal(Normal::Bottom);
        assert_eq!(e.cursor().0, 2);
        e.normal(Normal::G);
        assert_eq!(e.pending, Some('g'));
        assert_eq!(e.cursor().0, 2, "one g waits");
        e.normal(Normal::G);
        assert_eq!((e.cursor(), e.pending), ((0, 0), None));
        // Another command drops a waiting prefix.
        e.normal(Normal::G);
        e.normal(Normal::Down);
        assert_eq!((e.cursor().0, e.pending), (1, None));
    }

    #[test]
    fn dd_yy_p_x_and_u() {
        let mut e = Editor::new("one\ntwo\nthree");
        e.mode = Mode::Normal;
        e.normal(Normal::Down);
        assert!(!e.normal(Normal::D));
        assert!(e.normal(Normal::D));
        assert_eq!(e.text(), "one\nthree");
        assert_eq!(e.cursor().0, 1);
        e.normal(Normal::Paste);
        assert_eq!(e.text(), "one\nthree\ntwo", "p puts the line below");
        assert_eq!(e.cursor(), (2, 0));
        e.normal(Normal::G);
        e.normal(Normal::G);
        e.normal(Normal::Y);
        e.normal(Normal::Y);
        assert_eq!(e.text(), "one\nthree\ntwo", "yy changes nothing");
        e.normal(Normal::Paste);
        assert_eq!(e.text(), "one\none\nthree\ntwo");
        e.normal(Normal::Undo);
        assert_eq!(e.text(), "one\nthree\ntwo");
        // The last line, then the only one.
        e.normal(Normal::Bottom);
        e.normal(Normal::D);
        e.normal(Normal::D);
        assert_eq!(e.text(), "one\nthree");
        assert_eq!(e.cursor().0, 1);
        let mut one = Editor::new("only");
        one.mode = Mode::Normal;
        one.normal(Normal::D);
        one.normal(Normal::D);
        assert_eq!(one.text(), "");
        // x deletes the character under the cursor.
        let mut x = Editor::new("abc");
        x.mode = Mode::Normal;
        x.normal(Normal::Right);
        assert!(x.normal(Normal::DeleteChar));
        assert_eq!(x.text(), "ac");
        x.normal(Normal::Undo);
        assert_eq!(x.text(), "abc");
    }

    #[test]
    fn i_a_o_and_capital_o_go_back_to_insert() {
        let mut e = Editor::new("ab\ncd");
        e.mode = Mode::Normal;
        e.normal(Normal::Insert);
        assert_eq!((e.mode, e.cursor()), (Mode::Insert, (0, 0)));
        e.mode = Mode::Normal;
        e.normal(Normal::Append);
        assert_eq!((e.mode, e.cursor()), (Mode::Insert, (0, 1)));
        e.mode = Mode::Normal;
        e.normal(Normal::OpenBelow);
        assert_eq!((e.mode, e.text().as_str()), (Mode::Insert, "ab\n\ncd"));
        assert_eq!(e.cursor(), (1, 0));
        e.mode = Mode::Normal;
        e.normal(Normal::Bottom);
        e.normal(Normal::OpenAbove);
        assert_eq!(e.text(), "ab\n\n\ncd");
        assert_eq!((e.mode, e.cursor()), (Mode::Insert, (2, 0)));
        e.mode = Mode::Normal;
        e.normal(Normal::AppendEnd);
        assert_eq!((e.mode, e.cursor()), (Mode::Insert, (2, 0)));
        e.type_char('x');
        e.mode = Mode::Normal;
        e.normal(Normal::InsertStart);
        assert_eq!(e.cursor(), (2, 0));
    }

    // The emoji is 2 UTF-16 units and 4 bytes: Core's statement choice
    // (`Current` with the UTF-16 cursor) must match the editor's.
    #[test]
    fn the_cursor_in_bytes_and_utf16_past_an_emoji() {
        let mut e = Editor::new("SELECT '😀' AS a;\nSELECT 2 AS b");
        e.normal(Normal::Down);
        e.normal(Normal::Right);
        let text = e.text();
        assert_eq!(e.cursor_byte(), "SELECT '😀' AS a;\nS".len());
        assert_eq!(
            e.cursor_utf16(),
            seaquel_core::sql::offsets::byte_to_utf16(&text, e.cursor_byte())
        );
        assert_eq!(e.cursor_utf16(), e.cursor_byte() - 2);
        let byte = seaquel_core::sql::offsets::utf16_to_byte(&text, e.cursor_utf16());
        let stmt = seaquel_core::sql::scan::statement_at(&text, byte, SqlEngine::Sqlite).unwrap();
        assert_eq!(&text[stmt.text], "SELECT 2 AS b");
        // On the first line, past the emoji.
        let mut first = Editor::new("SELECT '😀' AS a;\nSELECT 2 AS b");
        for _ in 0..12 {
            first.insert_key(KeyCode::Right);
        }
        let byte = seaquel_core::sql::offsets::utf16_to_byte(&text, first.cursor_utf16());
        let stmt = seaquel_core::sql::scan::statement_at(&text, byte, SqlEngine::Sqlite).unwrap();
        assert_eq!(&text[stmt.text], "SELECT '😀' AS a");
    }

    #[test]
    fn scrolling_keeps_the_cursor_in_view() {
        let text: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let mut e = Editor::new(&text.join("\n"));
        e.normal(Normal::Bottom);
        e.scroll_into_view(10, 40);
        assert_eq!(e.top, 90);
        e.normal(Normal::G);
        e.normal(Normal::G);
        e.scroll_into_view(10, 40);
        assert_eq!(e.top, 0);
        let mut wide = Editor::new(&"x".repeat(100));
        wide.normal(Normal::LineEnd);
        wide.scroll_into_view(5, 40);
        assert_eq!(wide.left, 61, "the cursor's cell is the last column");
        wide.normal(Normal::LineStart);
        wide.scroll_into_view(5, 40);
        assert_eq!(wide.left, 0);
        assert_eq!(display_width("a\tb"), 5);
        assert_eq!(display_width("日本"), 4);
        let mut tabbed = Editor::new("\tx");
        tabbed.normal(Normal::LineEnd);
        assert_eq!(tabbed.cursor_column(), 5);
    }

    #[test]
    fn set_text_replaces_everything_and_undoes() {
        let mut e = Editor::new("one\ntwo");
        e.set_text("three\nfour\n");
        assert_eq!(e.text(), "three\nfour\n");
        e.normal(Normal::Undo);
        e.normal(Normal::Undo);
        assert_eq!(e.text(), "one\ntwo");
    }

    // Review M3: a text whose lines all end in `\r\n` keeps them, in `new`
    // and `set_text` alike, so a saved query opens unmodified and a
    // literal keeps its bytes; a mixed one is normalised to `\n` by both.
    #[test]
    fn crlf_is_kept_the_same_way_by_new_and_set_text() {
        let crlf = "SELECT 'a\r\nb';\r\nSELECT 2\r\n";
        let e = Editor::new(crlf);
        assert_eq!(e.text(), crlf);
        assert!(e.text_is(crlf));
        let mut e = Editor::new("x");
        e.set_text(crlf);
        assert_eq!(e.text(), crlf);
        // Typing a new line keeps the file's ending.
        e.normal(Normal::Bottom);
        e.insert_key(KeyCode::Enter);
        e.type_char('z');
        assert!(e.text().ends_with("\r\n\r\nz"), "{:?}", e.text());
        // The cursor's offsets count the `\r`.
        let e = Editor::new("ab\r\ncd");
        let mut e2 = e.clone();
        e2.normal(Normal::Down);
        e2.normal(Normal::Right);
        assert_eq!(e2.cursor_byte(), "ab\r\nc".len());
        assert_eq!(e2.cursor_utf16(), "ab\r\nc".len());
        // Mixed: `\n` in both.
        let mixed = "a\r\nb\nc";
        assert_eq!(Editor::new(mixed).text(), "a\nb\nc");
        let mut e = Editor::new("");
        e.set_text(mixed);
        assert_eq!(e.text(), "a\nb\nc");
        assert!(!Editor::new("a\nb").text_is("a\r\nb"));
        assert!(Editor::new("a\nb").text_is("a\nb"));
        assert!(!Editor::new("a\nb").text_is("a\nbc"));
    }

    #[test]
    fn the_highlight_cache_follows_the_text_and_engine() {
        let mut e = Editor::new("SELECT 1");
        assert!(!e.highlight_current(SqlEngine::Postgres));
        e.refresh_highlight(SqlEngine::Postgres);
        assert!(e.highlight_current(SqlEngine::Postgres));
        assert!(!e.highlight_current(SqlEngine::Mysql));
        e.type_char('2');
        assert!(!e.highlight_current(SqlEngine::Postgres));
    }

    #[test]
    fn debug_shows_no_text() {
        let e = Editor::new("SELECT 'sql-marker'");
        assert!(!format!("{e:?}").contains("sql-marker"));
    }
}
