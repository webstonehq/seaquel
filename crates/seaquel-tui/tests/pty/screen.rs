//! A small terminal screen for the pty tests:
//! the TUI's output parsed into the grid a terminal
//! would show, so a wait matches what is on screen rather than raw bytes.
//! ratatui redraws only the cells that changed, so a word that changed in
//! part, or was drawn over two frames, is never in the byte stream whole.
//!
//! It understands what ratatui's crossterm backend and the TUI send: text,
//! CR, LF and BS, cursor positioning and moves (`H`, `f`, `A`–`D`, `G`,
//! `d`), erasing (`J`, `K`, `X`) and the alternate screen (`?1049h/l`,
//! which clears). Colours and every other sequence are ignored. Parsing is
//! `anstyle-parse` (already in the lockfile; a VTE-compatible parser), so
//! no new crate is fetched. Characters take one column each, which holds
//! for everything the TUI draws (box drawing, `█`, `░`, `✓`).

#![allow(dead_code)]

use anstyle_parse::{Params, Parser, Perform};

pub struct Screen {
    rows: usize,
    cols: usize,
    cells: Vec<Vec<char>>,
    row: usize,
    col: usize,
}

impl Screen {
    /// `bytes` as a `cols` × `rows` terminal would show them now.
    pub fn parse(bytes: &[u8], cols: u16, rows: u16) -> Screen {
        let (cols, rows) = (usize::from(cols.max(1)), usize::from(rows.max(1)));
        let mut screen = Screen {
            rows,
            cols,
            cells: vec![vec![' '; cols]; rows],
            row: 0,
            col: 0,
        };
        let mut parser = Parser::<anstyle_parse::DefaultCharAccumulator>::new();
        for &b in bytes {
            parser.advance(&mut screen, b);
        }
        screen
    }

    /// The rows, trailing spaces trimmed, joined by newlines.
    pub fn text(&self) -> String {
        self.cells
            .iter()
            .map(|r| r.iter().collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether some row shows `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        self.cells
            .iter()
            .any(|r| r.iter().collect::<String>().contains(needle))
    }

    fn clear_all(&mut self) {
        for r in &mut self.cells {
            r.fill(' ');
        }
    }

    fn line_feed(&mut self) {
        if self.row + 1 >= self.rows {
            self.cells.remove(0);
            self.cells.push(vec![' '; self.cols]);
        } else {
            self.row += 1;
        }
    }

    fn clamp(&mut self) {
        self.row = self.row.min(self.rows - 1);
        self.col = self.col.min(self.cols - 1);
    }
}

/// The `i`th parameter, or `default` when it is missing or 0.
fn param(params: &Params, i: usize, default: usize) -> usize {
    params
        .iter()
        .nth(i)
        .and_then(|p| p.first().copied())
        .map(usize::from)
        .filter(|&n| n != 0)
        .unwrap_or(default)
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        if self.col >= self.cols {
            // Pending wrap.
            self.col = 0;
            self.line_feed();
        }
        self.cells[self.row][self.col] = c;
        self.col += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\r' => self.col = 0,
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            0x08 => self.col = self.col.saturating_sub(1),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: u8) {
        if intermediates.first() == Some(&b'?') {
            if matches!(action, b'h' | b'l') && params.iter().any(|p| p.first() == Some(&1049)) {
                self.clear_all();
                self.row = 0;
                self.col = 0;
            }
            return;
        }
        if !intermediates.is_empty() {
            return;
        }
        let n = param(params, 0, 1);
        match action {
            b'H' | b'f' => {
                self.row = n - 1;
                self.col = param(params, 1, 1) - 1;
            }
            b'A' => self.row = self.row.saturating_sub(n),
            b'B' => self.row += n,
            b'C' => self.col += n,
            b'D' => self.col = self.col.saturating_sub(n),
            b'G' => self.col = n - 1,
            b'd' => self.row = n - 1,
            b'J' => {
                self.clamp();
                let (row, col) = (self.row, self.col);
                match params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0)
                {
                    0 => {
                        self.cells[row][col..].fill(' ');
                        for r in &mut self.cells[row + 1..] {
                            r.fill(' ');
                        }
                    }
                    1 => {
                        for r in &mut self.cells[..row] {
                            r.fill(' ');
                        }
                        self.cells[row][..=col].fill(' ');
                    }
                    _ => self.clear_all(),
                }
                return;
            }
            b'K' => {
                self.clamp();
                let (row, col) = (self.row, self.col);
                match params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0)
                {
                    0 => self.cells[row][col..].fill(' '),
                    1 => self.cells[row][..=col].fill(' '),
                    _ => self.cells[row].fill(' '),
                }
                return;
            }
            b'X' => {
                self.clamp();
                let end = (self.col + n).min(self.cols);
                let row = self.row;
                self.cells[row][self.col..end].fill(' ');
                return;
            }
            _ => return,
        }
        self.clamp();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_drawn_in_pieces_reads_whole() {
        // "warehouse (duckdb)" where only "(duckdb)" changed in the second
        // frame, as ratatui's diff sends it.
        let bytes = b"\x1b[?1049h\x1b[2;3Hwarehouse (sqlite)\x1b[2;14Hduckdb)";
        let s = Screen::parse(bytes, 40, 5);
        assert!(s.contains("warehouse (duckdb)"), "{}", s.text());
        assert!(!s.contains("sqlite"));
    }

    #[test]
    fn erasing_and_the_alternate_screen_clear() {
        let s = Screen::parse(b"abc\x1b[1;2H\x1b[K", 10, 2);
        assert_eq!(s.text(), "a\n");
        let s = Screen::parse(b"abc\x1b[?1049l", 10, 2);
        assert_eq!(s.text(), "\n");
        let s = Screen::parse("\x1b[1;1H█░✓".as_bytes(), 10, 1);
        assert_eq!(s.text(), "█░✓");
    }
}
