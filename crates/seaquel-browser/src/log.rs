//! The module's log (phase 8): WARN and above, to the page's
//! console. The rules are the server's (`seaquel-server`'s `startup.rs`):
//! records carry activities, ids, counts, kinds and codes, never SQL, values,
//! names or file contents, and the formatter bounds what a record can hold:
//!
//! - the message is cut at 1 KiB and each key-value at 128 bytes, so an id
//!   that came from the page can't flood the console;
//! - a value holding a space, `=`, `"`, `\` or a control character is
//!   quoted, logfmt style, with `"` and `\` escaped, so it can't forge a
//!   field;
//! - control characters are escaped everywhere, so a record is one line.

/// The longest key-value [`format_record`] writes, in bytes (before
/// escaping); longer ones end in `…`.
pub const MAX_LOG_VALUE_BYTES: usize = 128;

/// The longest message [`format_record`] writes, in bytes.
pub const MAX_LOG_MESSAGE_BYTES: usize = 1024;

/// One record as the console line it becomes.
pub fn format_record(record: &log::Record) -> String {
    struct Pairs(String);
    impl<'kvs> log::kv::VisitSource<'kvs> for Pairs {
        fn visit_pair(
            &mut self,
            key: log::kv::Key<'kvs>,
            value: log::kv::Value<'kvs>,
        ) -> Result<(), log::kv::Error> {
            self.0.push(' ');
            push_value(&mut self.0, format_args!("{}", key.as_str()));
            self.0.push('=');
            push_value(&mut self.0, format_args!("{value}"));
            Ok(())
        }
    }
    let mut line = format!("[seaquel-browser] {} {}: ", record.level(), record.target());
    let (message, cut) = capped(*record.args(), MAX_LOG_MESSAGE_BYTES);
    push_escaped(&mut line, &message, false);
    if cut {
        line.push('…');
    }
    let mut pairs = Pairs(String::new());
    let _ = record.key_values().visit(&mut pairs);
    line.push_str(&pairs.0);
    line
}

/// `args` formatted into at most `max` bytes (cut on a char boundary), and
/// whether it was cut. Formatting stops at the cap: a long value is never
/// formatted whole.
fn capped(args: std::fmt::Arguments<'_>, max: usize) -> (String, bool) {
    struct Capped {
        out: String,
        max: usize,
        cut: bool,
    }
    impl std::fmt::Write for Capped {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if self.out.len() + c.len_utf8() > self.max {
                    self.cut = true;
                    // Stops the formatting.
                    return Err(std::fmt::Error);
                }
                self.out.push(c);
            }
            Ok(())
        }
    }
    let mut w = Capped {
        out: String::new(),
        max,
        cut: false,
    };
    let _ = std::fmt::Write::write_fmt(&mut w, args);
    (w.out, w.cut)
}

/// One key or value, cut at [`MAX_LOG_VALUE_BYTES`], bare or quoted.
fn push_value(out: &mut String, args: std::fmt::Arguments<'_>) {
    let (text, cut) = capped(args, MAX_LOG_VALUE_BYTES);
    let quote = text.is_empty()
        || text
            .chars()
            .any(|c| matches!(c, ' ' | '=' | '"' | '\\') || c.is_control());
    if quote {
        out.push('"');
    }
    push_escaped(out, &text, quote);
    if cut {
        out.push('…');
    }
    if quote {
        out.push('"');
    }
}

/// `text` with control characters escaped (`\n`, `\u{1b}`), and in a
/// quoted value `"` and `\` too.
fn push_escaped(out: &mut String, text: &str, quoted: bool) {
    for c in text.chars() {
        if c.is_control() || (quoted && matches!(c, '"' | '\\')) {
            out.extend(c.escape_debug());
        } else {
            out.push(c);
        }
    }
}

/// Writes WARN and above to `console.warn`/`console.error`.
#[cfg(target_arch = "wasm32")]
struct ConsoleLogger;

#[cfg(target_arch = "wasm32")]
mod console {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_namespace = console)]
        pub fn warn(s: &str);
        #[wasm_bindgen(js_namespace = console)]
        pub fn error(s: &str);
    }
}

#[cfg(target_arch = "wasm32")]
impl log::Log for ConsoleLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_record(record);
        if record.level() == log::Level::Error {
            console::error(&line);
        } else {
            console::warn(&line);
        }
    }

    fn flush(&self) {}
}

/// Installs the console logger. Does nothing if one is already set (a
/// second `open` on the same instance).
#[cfg(target_arch = "wasm32")]
pub fn install() {
    static LOGGER: ConsoleLogger = ConsoleLogger;
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Pairs<'a>(&'a [(&'a str, &'a str)]);

    impl log::kv::Source for Pairs<'_> {
        fn visit<'kvs>(
            &'kvs self,
            visitor: &mut dyn log::kv::VisitSource<'kvs>,
        ) -> Result<(), log::kv::Error> {
            for (k, v) in self.0 {
                visitor.visit_pair(log::kv::Key::from_str(k), log::kv::Value::from(*v))?;
            }
            Ok(())
        }
    }

    fn line(level: log::Level, message: std::fmt::Arguments<'_>, kvs: &[(&str, &str)]) -> String {
        let pairs = Pairs(kvs);
        let record = log::Record::builder()
            .level(level)
            .target("seaquel_core::library")
            .args(message)
            .key_values(&pairs)
            .build();
        format_record(&record)
    }

    #[test]
    fn a_record_is_its_level_target_message_and_pairs() {
        assert_eq!(
            line(
                log::Level::Warn,
                format_args!("Write failed"),
                &[
                    ("activity", "library.connectionCreate"),
                    ("code", "STORAGE_ERROR")
                ]
            ),
            "[seaquel-browser] WARN seaquel_core::library: Write failed \
             activity=library.connectionCreate code=STORAGE_ERROR"
        );
    }

    #[test]
    fn values_that_could_forge_a_field_are_quoted_and_escaped() {
        assert_eq!(
            line(
                log::Level::Error,
                format_args!("a\nb"),
                &[("id", "x code=OK"), ("q", "say \"hi\" \\"), ("e", "")]
            ),
            "[seaquel-browser] ERROR seaquel_core::library: a\\nb \
             id=\"x code=OK\" q=\"say \\\"hi\\\" \\\\\" e=\"\""
        );
    }

    #[test]
    fn long_messages_and_values_are_cut() {
        let long = "é".repeat(MAX_LOG_MESSAGE_BYTES);
        let value = "v".repeat(MAX_LOG_VALUE_BYTES + 10);
        let out = line(log::Level::Warn, format_args!("{long}"), &[("id", &value)]);
        let message = out
            .strip_prefix("[seaquel-browser] WARN seaquel_core::library: ")
            .unwrap();
        let (message, pair) = message.split_once(" id=").unwrap();
        assert_eq!(
            message,
            format!("{}…", "é".repeat(MAX_LOG_MESSAGE_BYTES / 2))
        );
        assert_eq!(pair, format!("{}…", "v".repeat(MAX_LOG_VALUE_BYTES)));
    }
}
