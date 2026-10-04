//! The terminal's events as the loop reads them: crossterm's `EventStream`
//! behind a handle that can stop it. A stopped stream ends crossterm's
//! reader thread (its `Drop` wakes and ends it), so a program the TUI runs
//! in the terminal (`$EDITOR`) gets every key; the next poll starts a new
//! one.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crossterm::event::{Event, EventStream};
use futures::Stream;

/// The loop's event stream; clones share it.
#[derive(Clone, Default)]
pub struct Input {
    stream: Arc<Mutex<Option<EventStream>>>,
}

impl Input {
    /// Stops reading the terminal until the stream is polled again.
    pub fn pause(&self) {
        let stopped = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(stopped);
    }
}

impl Stream for Input {
    type Item = io::Result<Event>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut slot = self.stream.lock().unwrap_or_else(|e| e.into_inner());
        let stream = slot.get_or_insert_with(EventStream::new);
        Pin::new(stream).poll_next(cx)
    }
}
