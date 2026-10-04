//! Scripted input.

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

use crate::state::app::Msg;

fn event(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

/// A typed character (Shift for upper case and symbols, as terminals send
/// them).
pub fn key(c: char) -> Msg {
    let modifiers = if c.is_ascii_uppercase() || "?{}~!@#$%^&*()_+|:\"<>".contains(c) {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    Msg::Key(event(KeyCode::Char(c), modifiers))
}

/// A non-character key.
pub fn press(code: KeyCode) -> Msg {
    let modifiers = if code == KeyCode::BackTab {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    Msg::Key(event(code, modifiers))
}

/// Ctrl + a letter.
pub fn ctrl(c: char) -> Msg {
    Msg::Key(event(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// A left click at a cell.
pub fn click(column: u16, row: u16) -> Msg {
    Msg::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// Alt + a letter.
pub fn alt(c: char) -> Msg {
    Msg::Key(event(KeyCode::Char(c), KeyModifiers::ALT))
}

/// One wheel step at a cell: down (towards the end) or up.
pub fn wheel(down: bool, column: u16, row: u16) -> Msg {
    Msg::Mouse(MouseEvent {
        kind: if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        },
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}
