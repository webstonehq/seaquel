//! The line-based YAML the `.seaquel` files use (`yaml-utils.ts`), with
//! Values that need quotes go in single quotes when
//! they hold `"` or `\` (older readers strip those exactly), in double
//! quotes with `\"`, `\\` and `\n` escaped when single quotes can't hold
//! them, and the reader undoes exactly that.

use seaquel_types::names::{is_js_space, js_trim};

/// A file as Core reads it: a leading BOM dropped and every `\r\n` turned
/// into `\n` (bug 13).
pub(crate) fn normalise(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    }
}

/// JavaScript's `\w`.
pub(crate) fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// A blank line or a comment, which every reader skips.
pub(crate) fn skip_line(line: &str) -> bool {
    let t = js_trim(line);
    t.is_empty() || t.starts_with('#')
}

/// `/^(\w+):\s*(.*)$/`: a top-level key and its value, trimmed.
pub(crate) fn key_line(line: &str) -> Option<(&str, &str)> {
    let end = line.find(|c: char| !is_word(c)).unwrap_or(line.len());
    if end == 0 || !line[end..].starts_with(':') {
        return None;
    }
    Some((&line[..end], js_trim(&line[end + 1..])))
}

/// `/^\s+-\s*(\w+):\s*(.*)$/` (`dash`) or `/^\s+(\w+):\s*(.*)$/`: an
/// indented key, the first of a list item with `dash`.
pub(crate) fn indented(line: &str, dash: bool) -> Option<(&str, &str)> {
    let rest = line.trim_start_matches(is_js_space);
    if rest.len() == line.len() {
        return None;
    }
    let rest = if dash {
        rest.strip_prefix('-')?.trim_start_matches(is_js_space)
    } else {
        rest
    };
    key_line(rest)
}

/// How a file's double-quoted values read. `Core`: Core
/// wrote the file (it carries Core's `id:` line), so `\"`, `\\` and
/// `\n` inside double quotes are Core's escapes and are undone. `Legacy`:
/// 2026.9.x or a person wrote it; their writer double-quotes a value
/// holding `\` without escaping it, so the quotes are stripped and the rest
/// read literally, as 2026.9.2's reader reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quoting {
    Core,
    Legacy,
}

/// A value as written in a file Core wrote (already trimmed) to its text.
#[cfg(test)]
pub(crate) fn read_value(value: &str) -> String {
    read_value_in(value, Quoting::Core)
}

/// A value as written in the file (already trimmed) to its text.
pub(crate) fn read_value_in(value: &str, quoting: Quoting) -> String {
    let b = value.as_bytes();
    let quoted = |q: u8| b.first() == Some(&q) && b.last() == Some(&q);
    if quoted(b'"') {
        if b.len() < 2 {
            return String::new();
        }
        let inner = &value[1..value.len() - 1];
        return match quoting {
            Quoting::Core => unescape_double(inner),
            Quoting::Legacy => inner.to_string(),
        };
    }
    if quoted(b'\'') {
        if b.len() < 2 {
            return String::new();
        }
        return value[1..value.len() - 1].replace("''", "'");
    }
    value.to_string()
}

/// Inside double quotes: `\"`, `\\` and `\n` are undone, any other
/// backslash pair stays as written.
fn unescape_double(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// An inline list in a file Core wrote.
#[cfg(test)]
pub(crate) fn read_list(value: &str) -> Vec<String> {
    read_list_in(value, Quoting::Core)
}

/// An inline list (`[a, "b,c", 'd']`) or a single value. Items are split
/// only outside quotes, trimmed and read; empty items are dropped. In a
/// `Legacy` file a `\` inside double quotes escapes nothing.
pub(crate) fn read_list_in(value: &str, quoting: Quoting) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let inner = match value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        Some(inner) => inner,
        None => {
            let v = read_value_in(value, quoting);
            return if v.is_empty() { Vec::new() } else { vec![v] };
        }
    };
    let mut items = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            None => {
                if c == ',' {
                    items.push(std::mem::take(&mut cur));
                    continue;
                }
                if (c == '"' || c == '\'') && js_trim(&cur).is_empty() {
                    quote = Some(c);
                }
                cur.push(c);
            }
            Some(q) => {
                cur.push(c);
                if q == '"' && c == '\\' && quoting == Quoting::Core {
                    if let Some(next) = chars.next() {
                        cur.push(next);
                    }
                } else if c == q {
                    if q == '\'' && chars.peek() == Some(&'\'') {
                        cur.push('\'');
                        chars.next();
                    } else {
                        quote = None;
                    }
                }
            }
        }
    }
    items.push(cur);
    items
        .iter()
        .map(|i| read_value_in(js_trim(i), quoting))
        .filter(|i| !i.is_empty())
        .collect()
}

/// Whether `v` must be quoted to read back as it is: today's triggers
/// (`:`, `#`, `"`, `[`, `]`, a newline, an edge space), plus any edge
/// whitespace JavaScript's `trim` drops, a `\r`, a leading `'` (a value
/// that starts and ends with one would lose them) and, in a list, `,`.
pub(crate) fn needs_quotes(v: &str, in_list: bool) -> bool {
    v.contains([':', '#', '"', '[', ']', '\n', '\r'])
        || (in_list && v.contains(','))
        || v.starts_with(is_js_space)
        || v.ends_with(is_js_space)
        || v.starts_with('\'')
}

/// A value as Core writes it.
pub(crate) fn write_value(v: &str) -> String {
    quote(v, false)
}

/// A list item as Core writes it: also quoted when it holds `,`.
fn write_item(v: &str) -> String {
    quote(v, true)
}

fn quote(v: &str, in_list: bool) -> String {
    if !needs_quotes(v, in_list) {
        return v.to_string();
    }
    let has = |c: char| v.contains(c);
    if !has('\'') && !has('\n') && (has('"') || has('\\')) {
        return format!("'{v}'");
    }
    if has('"') || has('\\') || has('\n') {
        // `"` and a newline are escaped; a `\\` only where the reader would
        // take it as an escape (before `"`, `\\`, `n` or a newline, or at the
        // end), so older readers, which undo nothing, keep the rest as typed.
        let mut out = String::with_capacity(v.len() + 4);
        out.push('"');
        let mut chars = v.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => match chars.peek() {
                    None | Some('"' | '\\' | 'n' | '\n') => out.push_str("\\\\"),
                    Some(_) => out.push('\\'),
                },
                '\n' => out.push_str("\\n"),
                c => out.push(c),
            }
        }
        out.push('"');
        return out;
    }
    format!("\"{v}\"")
}

/// `[a, b]`.
pub(crate) fn write_list(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|i| write_item(i)).collect();
    format!("[{}]", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_back() {
        for v in [
            "plain",
            "a: b",
            "Sales \"Q3\"",
            "Dana's \"Q3\"",
            "C:\\temp: x",
            "C:\\new: it's",
            "line one\nline two",
            " edge ",
            "'q'",
            "'",
            "\"",
            "\\",
            "\t",
            "a\\nb: c",
            "",
        ] {
            assert_eq!(read_value(&write_value(v)), v, "{v:?}");
        }
        let items: Vec<String> = [
            "a,b",
            "c",
            "say \"hi\"",
            "it's",
            "'x",
            "x'",
            "[y]",
            "a\"b'c,",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(read_list(&write_list(&items)), items);
    }
}
