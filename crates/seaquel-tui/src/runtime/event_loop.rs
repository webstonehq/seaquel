//! The event loop: one `tokio::select!`, biased to input, over
//! the terminal's events, Core's answers (the inbox), the spawned tasks,
//! the shutdown signals and a 16 ms tick. Input marks the screen dirty; a tick draws it only then, so
//! a flood of events never costs a frame each. `update` gets every message
//! and the loop carries out the effects it returns.
//!
//! It's generic over the backend and the event stream, so tests drive it
//! with `TestBackend` and scripted events; the real terminal's only
//! special case, Ctrl+Z, goes through [`Host`].

use std::io;
use std::pin::Pin;
use std::time::Duration;

use crossterm::event::Event;
use futures::{Stream, StreamExt};
use ratatui::backend::Backend;
use ratatui::Terminal;
use seaquel_terminal::ShutdownSignals;

use super::effects::{Inbox, Runner};
use crate::state::app::{update, Effect, Model, Msg};
use crate::view::view;

/// How often the loop may draw.
pub const FRAME: Duration = Duration::from_millis(16);

/// Why the loop ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    /// The user quit.
    Quit,
    /// SIGTERM, SIGHUP or SIGINT.
    Signal(&'static str),
    /// A spawned task panicked.
    Crashed,
    /// The terminal's event stream ended.
    EventsEnded,
    /// The terminal went away (EIO before SIGHUP).
    TerminalClosed,
}

/// What only the real terminal can do.
pub trait Host<B: Backend> {
    /// Ctrl+Z: give the terminal back until the process is continued, then
    /// hand the loop a terminal that repaints everything.
    fn suspend(&mut self, terminal: &mut Terminal<B>) -> io::Result<()>;

    /// Ctrl+O: give the terminal to `$EDITOR` with `text`, take it back
    /// (a terminal that repaints everything, never `clear()`), and answer
    /// the edited text or why the editor didn't run.
    fn edit(
        &mut self,
        terminal: &mut Terminal<B>,
        text: &str,
    ) -> io::Result<Result<String, String>>;
}

/// The signals the loop reacts to.
#[derive(Default)]
pub struct LoopSignals {
    /// SIGTERM, SIGHUP, SIGINT: end the loop.
    pub shutdown: Option<ShutdownSignals>,
    /// SIGTSTP from outside (`kill -TSTP`): suspend as Ctrl+Z does.
    pub suspend: Option<Pin<Box<dyn Stream<Item = ()> + Send>>>,
}

/// Whether an event can change the screen. Pointer moves alone (any-motion
/// mouse reporting sends one per cell crossed) never mark it dirty.
pub fn marks_dirty(event: &Event) -> bool {
    !matches!(
        event,
        Event::Mouse(crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Moved,
            ..
        })
    )
}

/// Runs until the user quits, a signal arrives, a task panics or the
/// events end. `initial` are the effects of the start (connect, or open
/// the picker and load the project).
#[allow(clippy::too_many_arguments)]
pub async fn run<B, E, H>(
    terminal: &mut Terminal<B>,
    model: &mut Model,
    events: E,
    host: &mut H,
    runner: &mut Runner,
    inbox: &mut Inbox,
    signals: LoopSignals,
    initial: Vec<Effect>,
) -> io::Result<Exit>
where
    B: Backend,
    B::Error: Send + Sync + 'static,
    E: Stream<Item = io::Result<Event>> + Unpin,
    H: Host<B>,
{
    let mut events = events;
    let LoopSignals {
        shutdown: mut signals,
        mut suspend,
    } = signals;
    let mut tick = tokio::time::interval(FRAME);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut effects = initial;
    loop {
        let mut queue: std::collections::VecDeque<Effect> = std::mem::take(&mut effects).into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Quit => return Ok(Exit::Quit),
                Effect::Suspend => {
                    host.suspend(terminal)?;
                    dirty = true;
                }
                Effect::ExternalEditor { tab, text } => {
                    let result = host.edit(terminal, &text.0)?.map(Into::into);
                    queue.extend(update(model, Msg::Edited { tab, result }));
                    dirty = true;
                }
                Effect::Redraw => dirty = true,
                other => runner.perform(other),
            }
        }
        effects = tokio::select! {
            biased;
            event = events.next() => match event {
                Some(Ok(event)) => match message(event.clone()) {
                    Some(msg) => {
                        dirty |= marks_dirty(&event);
                        update(model, msg)
                    }
                    None => Vec::new(),
                },
                Some(Err(e)) if super::terminal::gone(&e) => return Ok(Exit::TerminalClosed),
                Some(Err(e)) => return Err(e),
                None => return Ok(Exit::EventsEnded),
            },
            Some(msg) = inbox.recv() => {
                dirty = true;
                update(model, msg)
            }
            panicked = runner.tasks.next_panicked() => {
                if panicked {
                    return Ok(Exit::Crashed);
                }
                Vec::new()
            }
            name = wait_signal(&mut signals) => return Ok(Exit::Signal(name)),
            () = wait_suspend(&mut suspend) => vec![Effect::Suspend],
            now = tick.tick() => {
                let effects = update(model, Msg::Tick(now.into_std()));
                if dirty && !super::terminal::crashed() {
                    // The backend's error is kept inside, so a closed
                    // terminal's EIO is still told apart.
                    if let Err(e) = terminal.draw(|frame| view(model, frame)) {
                        let e = io::Error::other(e);
                        if super::terminal::gone(&e) {
                            return Ok(Exit::TerminalClosed);
                        }
                        return Err(e);
                    }
                    dirty = false;
                }
                effects
            }
        };
    }
}

/// The message a terminal event becomes; `None` for what the TUI ignores
/// (focus changes).
fn message(event: Event) -> Option<Msg> {
    match event {
        Event::Key(key) => Some(Msg::Key(key)),
        Event::Mouse(mouse) => Some(Msg::Mouse(mouse)),
        Event::Resize(width, height) => Some(Msg::Resize(width, height)),
        Event::Paste(text) => Some(Msg::Paste(text)),
        Event::FocusGained | Event::FocusLost => None,
    }
}

/// The next outside SIGTSTP; pending forever without one (or once its
/// stream ends).
async fn wait_suspend(suspend: &mut Option<Pin<Box<dyn Stream<Item = ()> + Send>>>) {
    match suspend {
        Some(stream) => match stream.next().await {
            Some(()) => {}
            None => {
                *suspend = None;
                std::future::pending::<()>().await;
            }
        },
        None => std::future::pending().await,
    }
}

/// The next shutdown signal; pending forever without handlers.
pub(crate) async fn wait_signal(signals: &mut Option<ShutdownSignals>) -> &'static str {
    match signals {
        Some(signals) => signals.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app::{Modal, Panel};
    use crate::testing::fixtures::model_sized;
    use crate::testing::snapshot::assert_snapshot;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use ratatui::backend::TestBackend;

    #[derive(Default)]
    struct TestHost {
        suspended: usize,
        /// What `$EDITOR` answers, and the texts it was given.
        edits: Vec<Result<String, String>>,
        given: Vec<String>,
    }

    impl Host<TestBackend> for TestHost {
        fn suspend(&mut self, _: &mut Terminal<TestBackend>) -> io::Result<()> {
            self.suspended += 1;
            Ok(())
        }

        fn edit(
            &mut self,
            _: &mut Terminal<TestBackend>,
            text: &str,
        ) -> io::Result<Result<String, String>> {
            self.given.push(text.to_string());
            Ok(self.edits.remove(0))
        }
    }

    fn key(code: KeyCode, modifiers: KeyModifiers) -> io::Result<Event> {
        Ok(Event::Key(KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }))
    }

    fn ch(c: char) -> io::Result<Event> {
        key(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Scripted events, then a stream that stays open (as a terminal's does).
    fn script(events: Vec<io::Result<Event>>) -> impl Stream<Item = io::Result<Event>> + Unpin {
        futures::stream::iter(events).chain(futures::stream::pending())
    }

    async fn drive(
        model: &mut Model,
        events: Vec<io::Result<Event>>,
        runner: &mut Runner,
    ) -> (io::Result<Exit>, Terminal<TestBackend>, TestHost) {
        let mut terminal = Terminal::new(TestBackend::new(model.size.0, model.size.1)).unwrap();
        let mut host = TestHost::default();
        let (_, mut inbox) = Runner::detached();
        let exit = tokio::time::timeout(
            Duration::from_secs(5),
            run(
                &mut terminal,
                model,
                script(events),
                &mut host,
                runner,
                &mut inbox,
                LoopSignals::default(),
                Vec::new(),
            ),
        )
        .await
        .expect("the loop ends");
        (exit, terminal, host)
    }

    #[tokio::test]
    async fn keys_reach_update_and_q_quits() {
        let mut m = model_sized(80, 24);
        let (exit, _, _) = drive(
            &mut m,
            vec![ch('3'), ch(']'), ch('q')],
            &mut Runner::detached().0,
        )
        .await;
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(m.focus, Panel::Saved);
    }

    #[tokio::test]
    async fn the_screen_is_drawn_and_follows_a_resize() {
        let mut m = model_sized(80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut host = TestHost::default();
        let (mut runner, mut inbox) = Runner::detached();
        // `?`, then a resize the backend has too, then wait for a frame.
        let (tx, rx) = futures::channel::mpsc::unbounded();
        tx.unbounded_send(ch('?')).unwrap();
        let run = run(
            &mut terminal,
            &mut m,
            rx,
            &mut host,
            &mut runner,
            &mut inbox,
            LoopSignals::default(),
            Vec::new(),
        );
        let feed = async {
            tokio::time::sleep(FRAME * 4).await;
            tx.unbounded_send(Ok(Event::Resize(100, 30))).unwrap();
            tokio::time::sleep(FRAME * 4).await;
            tx.unbounded_send(key(KeyCode::Esc, KeyModifiers::NONE))
                .unwrap();
            tokio::time::sleep(FRAME * 4).await;
            drop(tx);
        };
        let (exit, ()) = tokio::join!(run, feed);
        assert_eq!(exit.unwrap(), Exit::EventsEnded);
        assert_eq!(m.size, (100, 30));
        assert_eq!(m.modal, None);
        // The last frame was drawn at the old size (TestBackend doesn't
        // resize itself), from the model as it ended.
        terminal.backend_mut().resize(100, 30);
        terminal.draw(|f| view(&m, f)).unwrap();
        assert_snapshot("empty_100x30", terminal.backend().buffer());
    }

    #[tokio::test]
    async fn ctrl_c_asks_and_y_quits() {
        let mut m = model_sized(80, 24);
        let (exit, _, _) = drive(
            &mut m,
            vec![key(KeyCode::Char('c'), KeyModifiers::CONTROL), ch('y')],
            &mut Runner::detached().0,
        )
        .await;
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(m.modal, Some(Modal::ConfirmQuit));
    }

    #[tokio::test]
    async fn ctrl_z_goes_to_the_host() {
        let mut m = model_sized(80, 24);
        let (exit, _, host) = drive(
            &mut m,
            vec![key(KeyCode::Char('z'), KeyModifiers::CONTROL), ch('q')],
            &mut Runner::detached().0,
        )
        .await;
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(host.suspended, 1);
    }

    // The guard serialises the panicking tests for the whole test, awaits
    // included; nothing else takes it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_panicking_task_ends_the_loop() {
        let _serial = crate::runtime::terminal::PANIC_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut m = model_sized(80, 24);
        let (mut runner, _inbox) = Runner::detached();
        runner.tasks.spawn(async {});
        runner.tasks.spawn(async {
            tokio::time::sleep(FRAME).await;
            panic!("a Core call panicked");
        });
        let (exit, _, _) = drive(&mut m, vec![], &mut runner).await;
        assert_eq!(exit.unwrap(), Exit::Crashed);
    }

    #[tokio::test]
    async fn an_event_error_ends_the_loop_with_it() {
        let mut m = model_sized(80, 24);
        let (exit, _, _) = drive(
            &mut m,
            vec![Err(io::Error::other("tty gone"))],
            &mut Runner::detached().0,
        )
        .await;
        assert_eq!(exit.unwrap_err().to_string(), "tty gone");
    }

    /// Core catches some panics on purpose (MSSQL's session, DuckDB's
    /// blocking calls): the loop goes on drawing.
    // The guard serialises the panicking tests for the whole test, awaits
    // included; nothing else takes it.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_caught_panic_in_a_task_keeps_the_loop_drawing() {
        let _serial = crate::runtime::terminal::PANIC_TESTS
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::runtime::terminal::install_panic_hook_for_tests();
        let mut m = model_sized(80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut host = TestHost::default();
        let (mut runner, mut inbox) = Runner::detached();
        runner.tasks.spawn(async {
            let caught = std::panic::catch_unwind(|| panic!("caught by Core"));
            assert!(caught.is_err());
        });
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let run = run(
            &mut terminal,
            &mut m,
            rx,
            &mut host,
            &mut runner,
            &mut inbox,
            LoopSignals::default(),
            Vec::new(),
        );
        let feed = async {
            tokio::time::sleep(FRAME * 6).await;
            tx.unbounded_send(ch('2')).unwrap();
            tokio::time::sleep(FRAME * 6).await;
            tx.unbounded_send(ch('q')).unwrap();
        };
        let (exit, ()) = tokio::join!(run, feed);
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert!(!crate::runtime::terminal::crashed());
        let text = crate::testing::snapshot::buffer_text(terminal.backend().buffer());
        assert!(text.contains("Command Log"), "{text}");
    }

    #[tokio::test]
    async fn an_outside_sigtstp_suspends_like_ctrl_z() {
        let mut m = model_sized(80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut host = TestHost::default();
        let (mut runner, mut inbox) = Runner::detached();
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let (stop_tx, stop_rx) = futures::channel::mpsc::unbounded::<()>();
        let signals = LoopSignals {
            shutdown: None,
            suspend: Some(Box::pin(stop_rx)),
        };
        let run = run(
            &mut terminal,
            &mut m,
            rx,
            &mut host,
            &mut runner,
            &mut inbox,
            signals,
            Vec::new(),
        );
        let feed = async {
            tokio::time::sleep(FRAME * 2).await;
            stop_tx.unbounded_send(()).unwrap();
            tokio::time::sleep(FRAME * 2).await;
            tx.unbounded_send(ch('q')).unwrap();
        };
        let (exit, ()) = tokio::join!(run, feed);
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(host.suspended, 1);
    }

    #[tokio::test]
    async fn core_answers_arrive_through_the_inbox_and_start_effects_run_first() {
        let mut m = model_sized(80, 24);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut host = TestHost::default();
        let (mut runner, mut inbox) = Runner::detached();
        let sender = runner.sender();
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let looping = run(
            &mut terminal,
            &mut m,
            rx,
            &mut host,
            &mut runner,
            &mut inbox,
            LoopSignals::default(),
            vec![Effect::Redraw],
        );
        let feed = async {
            tokio::time::sleep(FRAME * 2).await;
            sender
                .send(Msg::Log(crate::state::log::LogLine {
                    time: "12:00:00".into(),
                    tag: None,
                    text: "schema tables".into(),
                    elapsed: Some("23 ms".into()),
                }))
                .unwrap();
            tokio::time::sleep(FRAME * 4).await;
            tx.unbounded_send(ch('q')).unwrap();
        };
        let (exit, ()) = tokio::join!(looping, feed);
        assert_eq!(exit.unwrap(), Exit::Quit);
        assert_eq!(m.log.len(), 1);
        let text = crate::testing::snapshot::buffer_text(terminal.backend().buffer());
        assert!(text.contains("schema tables  23 ms"), "{text}");

        // A start effect that quits ends the loop before any event.
        let mut m = model_sized(80, 24);
        let (mut runner, mut inbox) = Runner::detached();
        let exit = run(
            &mut terminal,
            &mut m,
            script(vec![]),
            &mut host,
            &mut runner,
            &mut inbox,
            LoopSignals::default(),
            vec![Effect::Quit],
        )
        .await;
        assert_eq!(exit.unwrap(), Exit::Quit);
    }

    // Ctrl+O: the text goes to the host's editor and comes back; an editor
    // that can't run is said, and the screen is drawn again either way.
    #[tokio::test]
    async fn ctrl_o_gives_the_text_to_the_editor_and_takes_it_back() {
        let mut m = crate::testing::fixtures::querying(80, 24, "SELECT 1", false);
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut host = TestHost {
            edits: vec![Ok("SELECT 2".into()), Err("vi: not found".into())],
            ..TestHost::default()
        };
        let (mut runner, mut inbox) = Runner::detached();
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let run = run(
            &mut terminal,
            &mut m,
            rx,
            &mut host,
            &mut runner,
            &mut inbox,
            LoopSignals::default(),
            Vec::new(),
        );
        let feed = async {
            tokio::time::sleep(FRAME * 2).await;
            tx.unbounded_send(key(KeyCode::Char('o'), KeyModifiers::CONTROL))
                .unwrap();
            tokio::time::sleep(FRAME * 4).await;
            tx.unbounded_send(key(KeyCode::Char('o'), KeyModifiers::CONTROL))
                .unwrap();
            tokio::time::sleep(FRAME * 4).await;
            drop(tx);
        };
        let (exit, ()) = tokio::join!(run, feed);
        assert_eq!(exit.unwrap(), Exit::EventsEnded);
        assert_eq!(host.given, ["SELECT 1", "SELECT 2"]);
        assert_eq!(m.query.tabs[0].editor.text(), "SELECT 2");
        let text = crate::testing::snapshot::buffer_text(terminal.backend().buffer());
        assert!(text.contains("vi: not found"), "drawn again: {text}");
    }

    // A bracketed paste reaches `update` as one message.
    #[test]
    fn a_paste_is_one_message() {
        assert!(matches!(
            message(Event::Paste("a\nb".into())),
            Some(Msg::Paste(t)) if t == "a\nb"
        ));
    }

    #[test]
    fn pointer_moves_alone_dont_redraw() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 3,
                row: 4,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert!(!marks_dirty(&mouse(MouseEventKind::Moved)));
        assert!(marks_dirty(&mouse(MouseEventKind::Down(MouseButton::Left))));
        assert!(marks_dirty(&mouse(MouseEventKind::ScrollDown)));
        assert!(marks_dirty(&ch('j').unwrap()));
        assert!(marks_dirty(&Event::Resize(80, 24)));
    }
}
