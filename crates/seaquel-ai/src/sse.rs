//! Server-sent events, framed from bytes.
//!
//! Lines end at `\n`, `\r\n` or a lone `\r` (a `\r` at the end of one read
//! followed by `\n` at the start of the next is one ending). A blank line
//! ends an event. `data:` lines are joined with `\n`, `event:` names it,
//! `:` comments, `id:` and `retry:` are ignored, and an event with no data
//! isn't dispatched (the SSE spec). A UTF-8 BOM at the start is dropped.
//! Bytes are decoded per line: `\n` and `\r` can't be inside a multi-byte
//! sequence, so a line is whole UTF-8 whatever the reads were (spike S1);
//! invalid UTF-8 becomes U+FFFD rather than an error.
//!
//! One event (its data and the line being read) is capped at
//! [`MAX_EVENT_BYTES`], so a provider that never sends a newline can't grow
//! the buffer without bound. The `event:` name is a separate buffer under
//! the same per-line cap, so one parser holds at most about 32 MiB (16 for
//! the name, 16 for the data and the current line), plus what `feed`'s
//! caller hands out in the finished events.

/// The largest event the parser holds: 16 MiB, far past any delta a
/// provider sends (the probe sends 1 MB ones).
pub const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, if the event had one.
    pub name: Option<String>,
    /// The `data:` lines joined with `\n`.
    pub data: String,
}

/// An event (or a line) passed [`MAX_EVENT_BYTES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventTooLarge;

#[derive(Default)]
pub struct SseParser {
    line: Vec<u8>,
    /// The last byte fed was a `\r` ending a line; a `\n` right after it
    /// belongs to the same ending.
    after_cr: bool,
    started: bool,
    name: Option<String>,
    /// The event's `data` lines, joined with `\n` as they come, so the
    /// bytes counted against the cap are the bytes held.
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes this parser holds for the event being read (its line and
    /// data buffers, allocation overhead included), for the memory tests.
    pub fn held_bytes(&self) -> usize {
        self.line.capacity() + self.data.capacity()
    }

    /// Feeds `bytes` and appends every event they complete to `out`.
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<SseEvent>) -> Result<(), EventTooLarge> {
        for &b in bytes {
            match b {
                b'\n' if self.after_cr => self.after_cr = false,
                b'\n' | b'\r' => {
                    self.after_cr = b == b'\r';
                    self.end_line(out)?;
                }
                _ => {
                    self.after_cr = false;
                    if self.line.len() + self.data.len() >= MAX_EVENT_BYTES {
                        return Err(EventTooLarge);
                    }
                    self.line.push(b);
                }
            }
        }
        Ok(())
    }

    /// The end of the stream: a last line without its ending, and an event
    /// without its blank line, still count.
    pub fn finish(&mut self, out: &mut Vec<SseEvent>) -> Result<(), EventTooLarge> {
        if !self.line.is_empty() {
            self.end_line(out)?;
        }
        self.after_cr = false;
        self.dispatch(out);
        Ok(())
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), EventTooLarge> {
        let mut raw = std::mem::take(&mut self.line);
        if !self.started {
            self.started = true;
            if raw.starts_with(b"\xEF\xBB\xBF") {
                raw.drain(..3);
            }
        }
        if raw.is_empty() {
            self.dispatch(out);
            return Ok(());
        }
        let line = String::from_utf8_lossy(&raw);
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line.as_ref(), ""),
        };
        match field {
            "data" => {
                let sep = usize::from(self.has_data);
                if self.data.len() + sep + value.len() > MAX_EVENT_BYTES {
                    return Err(EventTooLarge);
                }
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "event" => self.name = Some(value.to_string()),
            // `""` is a comment; `id`, `retry` and unknown fields are ignored.
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let name = self.name.take();
        if !std::mem::take(&mut self.has_data) {
            return;
        }
        out.push(SseEvent {
            name,
            data: std::mem::take(&mut self.data),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_in_pieces(bytes: &[u8], cuts: &[usize]) -> Vec<SseEvent> {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        let mut start = 0;
        for &cut in cuts {
            p.feed(&bytes[start..cut], &mut out).unwrap();
            start = cut;
        }
        p.feed(&bytes[start..], &mut out).unwrap();
        p.finish(&mut out).unwrap();
        out
    }

    fn parse(bytes: &[u8]) -> Vec<SseEvent> {
        parse_in_pieces(bytes, &[])
    }

    fn ev(name: Option<&str>, data: &str) -> SseEvent {
        SseEvent {
            name: name.map(String::from),
            data: data.to_string(),
        }
    }

    /// S1's Anthropic stream shape: `event:` and `data:` with `\r\n`, a
    /// ping, multi-byte text.
    const ANTHROPIC: &str = "event: message_start\r\ndata: {\"type\":\"message_start\"}\r\n\r\nevent: ping\r\ndata: {\"type\":\"ping\"}\r\n\r\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"café ☕ 𝄞\"}}\r\n\r\n";

    /// S1's OpenAI stream shape: `data:` only, `\n`, `[DONE]`.
    const OPENAI: &str = "data: {\"choices\":[{\"delta\":{\"content\":\"Checking ☕\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

    #[test]
    fn decodes_both_shapes() {
        assert_eq!(
            parse(ANTHROPIC.as_bytes()),
            vec![
                ev(Some("message_start"), "{\"type\":\"message_start\"}"),
                ev(Some("ping"), "{\"type\":\"ping\"}"),
                ev(
                    Some("content_block_delta"),
                    "{\"type\":\"content_block_delta\",\"delta\":{\"text\":\"café ☕ 𝄞\"}}"
                ),
            ]
        );
        let openai = parse(OPENAI.as_bytes());
        assert_eq!(openai.len(), 3);
        assert_eq!(openai[2], ev(None, "[DONE]"));
    }

    /// Every split into two reads, and every split into single bytes,
    /// gives the same events as one read: lines, `\r\n` and multi-byte
    /// characters straddle reads anywhere.
    #[test]
    fn every_split_position_decodes_the_same() {
        for stream in [ANTHROPIC, OPENAI] {
            let bytes = stream.as_bytes();
            let whole = parse(bytes);
            for cut in 0..=bytes.len() {
                assert_eq!(parse_in_pieces(bytes, &[cut]), whole, "cut at {cut}");
            }
            for cut in 0..bytes.len() {
                for cut2 in cut..=bytes.len() {
                    assert_eq!(
                        parse_in_pieces(bytes, &[cut, cut2]),
                        whole,
                        "cuts at {cut}, {cut2}"
                    );
                }
            }
            let singles: Vec<usize> = (1..bytes.len()).collect();
            assert_eq!(parse_in_pieces(bytes, &singles), whole);
        }
    }

    #[test]
    fn line_endings_lf_crlf_and_lone_cr() {
        let lf = parse(b"event: a\ndata: 1\n\n");
        let crlf = parse(b"event: a\r\ndata: 1\r\n\r\n");
        let cr = parse(b"event: a\rdata: 1\r\r");
        assert_eq!(lf, vec![ev(Some("a"), "1")]);
        assert_eq!(crlf, lf);
        assert_eq!(cr, lf);
        // A `\r` at the end of one read and `\n` at the start of the next
        // are one line ending, not a blank line.
        assert_eq!(
            parse_in_pieces(b"data: 1\r\ndata: 2\r\n\r\n", &[7]),
            vec![ev(None, "1\n2")]
        );
    }

    #[test]
    fn comments_ids_and_retry_are_ignored() {
        assert_eq!(
            parse(b": keep-alive\n\n:another\nid: 7\nretry: 100\ndata: x\n\n"),
            vec![ev(None, "x")]
        );
    }

    #[test]
    fn multi_line_data_joins_with_newlines() {
        assert_eq!(
            parse(b"data: {\"a\":\ndata:1}\ndata\n\n"),
            vec![ev(None, "{\"a\":\n1}\n")]
        );
    }

    #[test]
    fn one_leading_space_is_dropped_from_a_value() {
        assert_eq!(parse(b"data:  two\n\n"), vec![ev(None, " two")]);
        assert_eq!(parse(b"event:x\ndata:y\n\n"), vec![ev(Some("x"), "y")]);
    }

    #[test]
    fn a_final_event_without_its_blank_line_still_counts() {
        assert_eq!(parse(b"data: last\n"), vec![ev(None, "last")]);
        assert_eq!(parse(b"data: last"), vec![ev(None, "last")]);
        assert_eq!(
            parse(b"data: a\n\ndata: b"),
            vec![ev(None, "a"), ev(None, "b")]
        );
    }

    #[test]
    fn an_event_without_data_is_not_dispatched_and_its_name_resets() {
        assert_eq!(parse(b"event: lonely\n\ndata: x\n\n"), vec![ev(None, "x")]);
    }

    #[test]
    fn a_bom_at_the_start_is_dropped_even_split() {
        let bytes = b"\xEF\xBB\xBFdata: x\n\n";
        for cut in 0..=bytes.len() {
            assert_eq!(
                parse_in_pieces(bytes, &[cut]),
                vec![ev(None, "x")],
                "cut {cut}"
            );
        }
    }

    #[test]
    fn invalid_utf8_is_replaced_not_an_error() {
        assert_eq!(parse(b"data: \xFF\n\n"), vec![ev(None, "\u{FFFD}")]);
    }

    #[test]
    fn an_endless_line_is_refused() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        let chunk = vec![b'a'; 1024 * 1024];
        let mut refused = false;
        for _ in 0..(MAX_EVENT_BYTES / chunk.len() + 2) {
            if p.feed(&chunk, &mut out).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused);
    }

    /// Review fix 2: empty `data` lines cost what is counted (one `\n`
    /// each), not a `String` header each. 40 MiB of them (8 Mi lines, 8 MiB
    /// of data) are held in about 8 MiB, not ~190.
    #[test]
    fn empty_data_lines_hold_what_they_count() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        let chunk = b"data\n".repeat(1 << 20);
        for _ in 0..8 {
            p.feed(&chunk, &mut out).unwrap();
        }
        let held = p.held_bytes();
        assert!(held <= 20 * 1024 * 1024, "held {held} bytes");
        p.feed(b"\n", &mut out).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].data.len(), 8 * (1 << 20) - 1);
    }

    #[test]
    fn an_event_of_many_data_lines_is_capped() {
        let mut p = SseParser::new();
        let mut out = Vec::new();
        let mut line = b"data: ".to_vec();
        line.extend(vec![b'a'; 1024 * 1024]);
        line.push(b'\n');
        let mut refused = false;
        for _ in 0..(MAX_EVENT_BYTES / (1024 * 1024) + 2) {
            if p.feed(&line, &mut out).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused);
        assert!(out.is_empty());
    }
}
