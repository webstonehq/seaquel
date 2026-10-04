//! Render snapshots: a `TestBackend` buffer as plain text lines (trailing
//! spaces trimmed), compared with `tests/snapshots/<name>.txt`.
//! `UPDATE_SNAPSHOTS=1` writes them instead; a missing one fails until it
//! has been written and reviewed. The app's version is written as
//! `<version>`, so a release bump changes no snapshot.

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use crate::state::app::Model;
use crate::view::view;

/// The model drawn at its own size.
pub fn draw(model: &Model) -> Buffer {
    let (w, h) = model.size;
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|frame| view(model, frame)).unwrap();
    terminal.backend().buffer().clone()
}

/// The buffer's text: one line per row, wide characters once, trailing
/// spaces trimmed.
pub fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        let mut skip = 0usize;
        for x in area.left()..area.right() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let symbol = buf[(x, y)].symbol();
            line.push_str(symbol);
            skip = symbol.width().saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"))
}

/// Compares a drawn buffer with its snapshot.
pub fn assert_snapshot(name: &str, buf: &Buffer) {
    assert_text_snapshot(name, &buffer_text(buf));
}

/// Compares text with its snapshot.
pub fn assert_text_snapshot(name: &str, actual: &str) {
    let actual = actual.replace(seaquel_terminal::VERSION, "<version>");
    let actual = actual.as_str();
    let file = path(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some_and(|v| v == "1") {
        std::fs::write(&file, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&file).unwrap_or_else(|_| {
        panic!(
            "no snapshot {}; run with UPDATE_SNAPSHOTS=1 and review it:\n{actual}",
            file.display()
        )
    });
    if expected == actual {
        return;
    }
    let mut diff = String::new();
    let (e, a): (Vec<_>, Vec<_>) = (expected.lines().collect(), actual.lines().collect());
    for i in 0..e.len().max(a.len()) {
        let (el, al) = (e.get(i).copied(), a.get(i).copied());
        if el != al {
            diff.push_str(&format!(
                "line {}:\n  - {}\n  + {}\n",
                i + 1,
                el.unwrap_or("<none>"),
                al.unwrap_or("<none>")
            ));
        }
    }
    panic!("snapshot {name} differs (UPDATE_SNAPSHOTS=1 rewrites it):\n{diff}\nactual:\n{actual}");
}
