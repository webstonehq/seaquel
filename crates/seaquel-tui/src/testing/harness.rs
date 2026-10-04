//! The TUI over a real Core, without a terminal: the model, the runner and
//! its inbox, driven by scripted keys. [`Harness::until`] feeds Core's
//! answers (and ticks) to `update` until the model gets where the test
//! expects, as the event loop would.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::KeyCode;
use seaquel_core::secrets::SecretStore;

use super::fixtures;
use super::keys;
use crate::runtime::core::{self, OpenOptions, Session};
use crate::runtime::effects::{Inbox, Runner};
use crate::state::app::{update, Effect, Model, Msg};
use crate::state::connect;
use crate::state::picker::{resolve_start, Remembered};

/// How long a step may take before the test fails.
pub const WAIT: Duration = Duration::from_secs(20);

/// The polling interval the tests open with (the TUI's is 1 s).
pub const TEST_POLL: Duration = Duration::from_millis(50);

pub struct Harness {
    pub model: Model,
    pub runner: Runner,
    inbox: Inbox,
    pub session: Arc<Session>,
    /// Every effect `update` asked for, in order.
    pub effects: Vec<Effect>,
}

/// What a harness opens with.
pub struct HarnessOptions<'a> {
    pub data_dir: &'a Path,
    pub store: Arc<dyn SecretStore>,
    pub known_hosts: Option<&'a Path>,
    pub project: Option<&'a str>,
    pub connection: Option<&'a str>,
    pub remembered: Remembered,
    pub origin: &'a str,
}

impl<'a> HarnessOptions<'a> {
    pub fn new(data_dir: &'a Path, store: Arc<dyn SecretStore>) -> Self {
        HarnessOptions {
            data_dir,
            store,
            known_hosts: None,
            project: None,
            connection: None,
            remembered: Remembered::default(),
            origin: "tui-harness1",
        }
    }
}

impl Harness {
    /// Opens the TUI's Core on the data dir and starts as `run_app` does.
    pub async fn open(options: HarnessOptions<'_>) -> Harness {
        let session = Arc::new(
            core::open(OpenOptions {
                data_dir: options.data_dir.to_path_buf(),
                store: options.store,
                known_hosts: options.known_hosts.map(Path::to_path_buf),
                poll: TEST_POLL,
                origin: Some(options.origin.into()),
            })
            .await
            .unwrap_or_else(|e| panic!("open: {}: {}", e.code, e.message)),
        );
        let library = session.library().await.unwrap();
        let start = resolve_start(
            &library,
            options.project,
            options.connection,
            &options.remembered,
        )
        .unwrap();
        let (mut runner, inbox) =
            Runner::new(Some(session.clone()), Some(options.data_dir.to_path_buf()));
        runner.listen();
        let mut model = fixtures::model();
        model.remembered = options.remembered;
        let mut harness = Harness {
            model,
            runner,
            inbox,
            session,
            effects: Vec::new(),
        };
        harness.send(Msg::Library(Ok(library)));
        let effects = connect::start(&mut harness.model, start);
        harness.apply(effects);
        harness
    }

    fn apply(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            self.effects.push(effect.clone());
            match effect {
                Effect::Quit | Effect::Suspend | Effect::Redraw => {}
                other => self.runner.perform(other),
            }
        }
    }

    /// Applies a message and carries out its effects.
    pub fn send(&mut self, msg: Msg) {
        let effects = update(&mut self.model, msg);
        self.apply(effects);
    }

    /// Types `text`, one key each.
    pub fn keys(&mut self, text: &str) {
        for c in text.chars() {
            self.send(keys::key(c));
        }
    }

    pub fn press(&mut self, code: KeyCode) {
        self.send(keys::press(code));
    }

    /// Feeds Core's answers and a tick every 10 ms to `update` until
    /// `done(model)`; fails after [`WAIT`] naming `what`.
    pub async fn until(&mut self, what: &str, done: impl Fn(&Model) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.model) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: conn {:?}, modal {:?}, log {:?}",
                self.model.conn,
                self.model.modal,
                self.model
                    .log
                    .last(5)
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>()
            );
            tokio::select! {
                msg = self.inbox.recv() => {
                    if let Some(msg) = msg {
                        self.send(msg);
                    }
                }
                () = tokio::time::sleep(Duration::from_millis(10)) => {
                    self.send(Msg::Tick(Instant::now()));
                }
            }
        }
    }

    /// Closes the session.
    pub async fn close(mut self) {
        self.runner.close().await;
    }
}
