//! The command log's lines: what Core reports, kept in memory
//! only (the last [`MAX_LINES`]), never written to a file or the log. Its
//! lines hold SQL, so `Debug` shows only the count.

use std::collections::VecDeque;

/// How many lines the log keeps.
pub const MAX_LINES: usize = 500;

/// A line's tag (`staged`, `undo`, …), which colours it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    Staged,
    StagedDelete,
    Unstaged,
    Undo,
    Committed,
    ReadOnly,
    Error,
}

/// One line: the wall-clock time it was logged (`HH:MM:SS`, from the
/// message that carried it: `update` reads no clock), an optional tag, the
/// text and how long it took.
#[derive(Clone, PartialEq, Eq)]
pub struct LogLine {
    pub time: String,
    pub tag: Option<Tag>,
    pub text: String,
    pub elapsed: Option<String>,
}

impl std::fmt::Debug for LogLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogLine")
            .field("tag", &self.tag)
            .field("text_bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// The command log.
#[derive(Clone, Default)]
pub struct CommandLog {
    lines: VecDeque<LogLine>,
}

impl std::fmt::Debug for CommandLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CommandLog({} lines)", self.lines.len())
    }
}

impl CommandLog {
    /// Appends a line, dropping the oldest past [`MAX_LINES`].
    pub fn push(&mut self, line: LogLine) {
        if self.lines.len() == MAX_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The newest `n` lines, oldest first.
    pub fn last(&self, n: usize) -> impl Iterator<Item = &LogLine> {
        self.lines.iter().skip(self.lines.len().saturating_sub(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> LogLine {
        LogLine {
            time: "12:00:00".into(),
            tag: None,
            text: text.into(),
            elapsed: None,
        }
    }

    #[test]
    fn keeps_the_last_500_lines() {
        let mut log = CommandLog::default();
        for i in 0..MAX_LINES + 20 {
            log.push(line(&format!("SELECT {i}")));
        }
        assert_eq!(log.len(), MAX_LINES);
        let last: Vec<_> = log.last(2).map(|l| l.text.clone()).collect();
        assert_eq!(last, ["SELECT 518", "SELECT 519"]);
        assert_eq!(log.last(MAX_LINES).next().unwrap().text, "SELECT 20");
    }

    #[test]
    fn debug_shows_no_sql() {
        let mut log = CommandLog::default();
        log.push(line("SELECT 'secret-marker'"));
        assert!(!format!("{log:?}").contains("secret-marker"));
        assert!(!format!("{:?}", log.last(1).next().unwrap()).contains("secret-marker"));
    }
}
