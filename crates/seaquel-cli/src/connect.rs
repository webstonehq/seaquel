//! Connecting to (or testing) a saved connection, asking for what's missing.
//!
//! Core reads the saved secrets from the keychain itself. What it can't
//! read is asked for when the CLI is interactive (`prompt`): a password the
//! row doesn't save, or, when the keychain can't be reached this session
//! (`SECRET_STORE_UNAVAILABLE`), the ones it would hold. An unknown SSH host
//! key shows the host and its fingerprint and asks whether to trust it;
//! yes records it in known_hosts, as the TUI does. `HOST_KEY_MISMATCH` is
//! never offered.
//!
//! Each secret is asked for once. Unlike the TUI, a wrong password isn't
//! asked for again: the command fails and the user runs it again. When
//! nothing can be asked, a missing password or an unknown host key fails
//! with a line saying how to get past it.
//!
//! The decisions ([`ask_before`], [`next`]) are pure, ported from the TUI's
//! `state/connect.rs` (`to_ask`, `failed`).

use std::future::Future;

use seaquel_core::{ConnectRequest, CoreError, HostKeyPolicy};
use seaquel_mcp::duckdb_helper::reword_connect_error;
use seaquel_mcp::ToolError;
use seaquel_types::connect::SuppliedSecrets;
use seaquel_types::storage::{PersistedConnection, SshTunnelConfig};
use zeroize::Zeroizing;

use crate::prompt::Prompter;
use crate::session::Session;
use crate::VERSION;

/// At most this many connects (or tests) per command.
const MAX_ATTEMPTS: usize = 4;

const NO_PASSWORD_HINT: &str = " Run this in a terminal to be asked for it.";
const NO_TRUST_HINT: &str = " Run this in a terminal to check and trust the host key.";

/// What [`open`] does once connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keep the connection: [`open`] answers Core's connection id.
    Connect,
    /// Connect and disconnect again ([`seaquel_core::Workspace::test`]).
    Test,
}

/// A secret the CLI may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Db,
    Ssh,
    SshKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshAuth {
    Password,
    Key,
}

/// A saved connection's enabled SSH tunnel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ssh {
    pub host: String,
    pub port: u16,
    pub auth: SshAuth,
    /// The row saves the SSH password (password auth) or the key's
    /// passphrase (key auth).
    pub saved: bool,
}

/// What the decisions need to know about a saved connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFacts {
    /// SQLite and DuckDB: no database password.
    pub is_file: bool,
    pub save_password: bool,
    pub ssh: Option<Ssh>,
}

impl RowFacts {
    pub fn of(row: &PersistedConnection) -> Self {
        Self {
            is_file: matches!(row.ty.as_str(), "sqlite" | "duckdb"),
            save_password: row.save_password,
            ssh: ssh_tunnel(row).map(|t| {
                let auth = if t.auth_method == "key" {
                    SshAuth::Key
                } else {
                    SshAuth::Password
                };
                Ssh {
                    port: port(t.port).filter(|p| *p != 0).unwrap_or(22),
                    saved: match auth {
                        SshAuth::Password => row.save_ssh_password,
                        SshAuth::Key => row.save_ssh_key_passphrase,
                    },
                    auth,
                    host: t.host,
                }
            }),
        }
    }
}

/// The row's SSH tunnel, when it has one and it is enabled (as the TUI
/// reads it; a value that doesn't parse counts as none).
pub fn ssh_tunnel(row: &PersistedConnection) -> Option<SshTunnelConfig> {
    row.ssh_tunnel
        .as_deref()
        .and_then(|raw| serde_json::from_str::<SshTunnelConfig>(raw.get()).ok())
        .filter(|t| t.enabled)
}

/// A stored JSON number as a port, when it is one.
pub fn port(n: f64) -> Option<u16> {
    (n.fract() == 0.0 && (0.0..=65535.0).contains(&n)).then_some(n as u16)
}

/// The secrets typed this command. Never in `Debug` or a log.
#[derive(Default)]
pub struct Typed {
    db: Option<Zeroizing<String>>,
    ssh: Option<Zeroizing<String>>,
    ssh_key: Option<Zeroizing<String>>,
}

impl Typed {
    fn slot(&mut self, kind: Kind) -> &mut Option<Zeroizing<String>> {
        match kind {
            Kind::Db => &mut self.db,
            Kind::Ssh => &mut self.ssh,
            Kind::SshKey => &mut self.ssh_key,
        }
    }

    pub fn has(&self, kind: Kind) -> bool {
        match kind {
            Kind::Db => self.db.is_some(),
            Kind::Ssh => self.ssh.is_some(),
            Kind::SshKey => self.ssh_key.is_some(),
        }
    }

    pub fn set(&mut self, kind: Kind, secret: Zeroizing<String>) {
        *self.slot(kind) = Some(secret);
    }

    /// For Core's request. An empty one is sent as typed; Core counts it
    /// as not given.
    pub fn supplied(&self) -> SuppliedSecrets {
        let copy = |s: &Option<Zeroizing<String>>| s.as_deref().map(|s| s.as_str().to_owned());
        SuppliedSecrets {
            db: copy(&self.db),
            ssh: copy(&self.ssh),
            ssh_key: copy(&self.ssh_key),
        }
    }
}

/// What to do after a failed attempt, when the CLI can ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Ask for this secret and try again.
    Ask(Kind),
    /// The keychain can't be reached this session: ask for what it would
    /// hold ([`ask_before`] with `no_store`) and try again.
    NoStore,
    /// Ask whether to trust the SSH host with this fingerprint.
    Trust(String),
    /// Give up with the error.
    Fail,
}

/// The secret to ask for before connecting, if any: one the row doesn't
/// save and nothing typed covers; and, once the keychain is known to be
/// unavailable (`no_store`), one it would have read from there too.
pub fn ask_before(facts: &RowFacts, typed: &Typed, no_store: bool) -> Option<Kind> {
    if !facts.is_file && (!facts.save_password || no_store) && !typed.has(Kind::Db) {
        return Some(Kind::Db);
    }
    let ssh = facts.ssh.as_ref()?;
    match ssh.auth {
        SshAuth::Password if (!ssh.saved || no_store) && !typed.has(Kind::Ssh) => Some(Kind::Ssh),
        SshAuth::Key if ssh.saved && no_store && !typed.has(Kind::SshKey) => Some(Kind::SshKey),
        _ => None,
    }
}

/// Whether a `CREDENTIALS_REQUIRED` message is about a password (the
/// database's or the SSH tunnel's), not a field the row lacks (a host, a
/// user name, an SSH key file), which no prompt fills.
fn mentions_password(message: &str) -> bool {
    message.to_ascii_lowercase().contains("password")
}

/// `message` without the connection's quoted name. Core's messages quote it
/// (`Connection "<name>" …`, Rust's `{:?}`), and a name holding "SSH" or
/// "password" mustn't steer [`next`].
fn without_name(message: &str, name: &str) -> String {
    message.replace(&format!("{name:?}"), "")
}

/// What a failed connect with `code` and `message` leads to. A secret
/// already typed is never asked for again: a wrong one fails.
pub fn next(facts: &RowFacts, typed: &Typed, code: &str, message: &str) -> Next {
    match code {
        "SECRET_STORE_UNAVAILABLE" => Next::NoStore,
        "UNKNOWN_HOST_KEY" => match seaquel_terminal::host_key_fingerprint(message) {
            Some(fingerprint) => Next::Trust(fingerprint),
            None => Next::Fail,
        },
        "CREDENTIALS_REQUIRED" => {
            let ssh_password = facts
                .ssh
                .as_ref()
                .is_some_and(|s| s.auth == SshAuth::Password);
            let ssh = message.contains("SSH");
            if !mentions_password(message) {
                Next::Fail
            } else if ssh_password && !typed.has(Kind::Ssh) && ssh {
                Next::Ask(Kind::Ssh)
            } else if !facts.is_file && !typed.has(Kind::Db) && !ssh {
                Next::Ask(Kind::Db)
            } else {
                Next::Fail
            }
        }
        _ => Next::Fail,
    }
}

/// The password prompt for `kind`.
fn label(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Db => format!("Password for {name}: "),
        Kind::Ssh => format!("SSH password for {name}: "),
        Kind::SshKey => format!("SSH key passphrase for {name}: "),
    }
}

fn cancelled() -> CoreError {
    CoreError::new("CANCELLED", "No password was given.")
}

/// Connect (or test) the saved connection `row`, asking for what's missing
/// when `prompter` can. At most [`MAX_ATTEMPTS`] attempts, and each secret
/// is asked for once. [`Mode::Connect`] answers Core's connection id,
/// [`Mode::Test`] `None`.
pub async fn open(
    s: &Session,
    row: &PersistedConnection,
    mode: Mode,
    prompter: &mut impl Prompter,
) -> Result<Option<String>, CoreError> {
    let facts = RowFacts::of(row);
    let interactive = prompter.interactive();
    let id = row.id.as_str();
    let result = drive(&facts, &row.name, prompter, |secrets, host_key| {
        let req = ConnectRequest::saved(id)
            .with_secrets(secrets)
            .with_host_key(host_key);
        async move {
            match mode {
                Mode::Connect => s.ws.connect(&s.core, req).await.map(Some),
                Mode::Test => s.ws.test(&s.core, req).await.map(|()| None),
            }
        }
    })
    .await;
    result.map_err(|e| {
        let e = reword(s, &row.ty, e);
        if interactive {
            e
        } else {
            not_interactive_hint(&facts, &row.name, e)
        }
    })
}

/// [`open`] with [`Mode::Connect`]: Core's id of the connection it opened,
/// which the session's `close` closes again.
pub async fn connect(
    s: &Session,
    row: &PersistedConnection,
    prompter: &mut impl Prompter,
) -> Result<String, CoreError> {
    open(s, row, Mode::Connect, prompter)
        .await?
        // Mode::Connect always answers Some; a guard rather than a panic.
        .ok_or_else(|| CoreError::new("CONNECT_FAILED", "Core answered no connection id."))
}

/// The loop behind [`open`], over any `attempt` (tests script it).
async fn drive<T, F, Fut>(
    facts: &RowFacts,
    name: &str,
    prompter: &mut impl Prompter,
    mut attempt: F,
) -> Result<T, CoreError>
where
    F: FnMut(SuppliedSecrets, HostKeyPolicy) -> Fut,
    Fut: Future<Output = Result<T, CoreError>>,
{
    let mut typed = Typed::default();
    let mut no_store = false;
    let mut host_key = HostKeyPolicy::KnownOnly;
    for _ in 0..MAX_ATTEMPTS {
        if prompter.interactive() {
            if let Some(kind) = ask_before(facts, &typed, no_store) {
                let secret = prompter
                    .password(&label(kind, name))
                    .await
                    .ok_or_else(cancelled)?;
                typed.set(kind, secret);
            }
        }
        let e = match attempt(typed.supplied(), host_key.clone()).await {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        if !prompter.interactive() {
            return Err(e);
        }
        match next(facts, &typed, &e.code, &without_name(&e.message, name)) {
            Next::Ask(kind) => {
                let secret = prompter
                    .password(&label(kind, name))
                    .await
                    .ok_or_else(cancelled)?;
                typed.set(kind, secret);
            }
            // Only when there is something left to ask for; else the same
            // attempt would fail the same way. Each round fills one secret
            // (the keychain may hold the database's and the SSH tunnel's),
            // and MAX_ATTEMPTS bounds the rounds.
            Next::NoStore if ask_before(facts, &typed, true).is_some() => {
                no_store = true;
            }
            Next::Trust(fingerprint) if host_key == HostKeyPolicy::KnownOnly => {
                let Some(ssh) = &facts.ssh else {
                    return Err(e);
                };
                let question = format!(
                    "The SSH host {}:{} isn't known. Its key's fingerprint is {fingerprint}. \
                     Trust this host?",
                    ssh.host, ssh.port
                );
                if !prompter.confirm(&question).await {
                    return Err(e);
                }
                host_key = HostKeyPolicy::Trust(fingerprint);
            }
            _ => return Err(e),
        }
    }
    Err(gave_up())
}

/// After [`MAX_ATTEMPTS`] attempts.
fn gave_up() -> CoreError {
    CoreError::new(
        "CONNECT_FAILED",
        format!("Gave up after {MAX_ATTEMPTS} attempts."),
    )
}

/// A DuckDB connection's `ENGINE_NOT_INSTALLED` in the CLI's words (how to
/// install its helper); other errors as Core gave them.
fn reword(s: &Session, engine: &str, e: CoreError) -> CoreError {
    let e = reword_connect_error(&s.core, engine, VERSION, ToolError::new(e.code, e.message));
    CoreError::new(e.code, e.message)
}

/// When nothing could be asked: how to get past a missing password (only
/// when a terminal run would have asked for it, the keychain's included
/// when it can't be reached) or an unknown host key.
fn not_interactive_hint(facts: &RowFacts, name: &str, mut e: CoreError) -> CoreError {
    match e.code.as_str() {
        "SECRET_STORE_UNAVAILABLE" if ask_before(facts, &Typed::default(), true).is_some() => {
            e.message.push_str(NO_PASSWORD_HINT)
        }
        "CREDENTIALS_REQUIRED"
            if matches!(
                next(
                    facts,
                    &Typed::default(),
                    &e.code,
                    &without_name(&e.message, name)
                ),
                Next::Ask(_)
            ) =>
        {
            e.message.push_str(NO_PASSWORD_HINT)
        }
        "UNKNOWN_HOST_KEY" => e.message.push_str(NO_TRUST_HINT),
        _ => {}
    }
    e
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use crate::prompt::Scripted;

    fn row(save_password: bool) -> RowFacts {
        RowFacts {
            is_file: false,
            save_password,
            ssh: None,
        }
    }

    fn tunnelled(auth: SshAuth, saved: bool) -> RowFacts {
        RowFacts {
            ssh: Some(Ssh {
                host: "bastion".into(),
                port: 2222,
                auth,
                saved,
            }),
            ..row(true)
        }
    }

    fn typed(kinds: &[Kind]) -> Typed {
        let mut t = Typed::default();
        for k in kinds {
            t.set(*k, Zeroizing::new("x".into()));
        }
        t
    }

    #[test]
    fn a_row_that_saves_no_password_is_asked_first() {
        assert_eq!(
            ask_before(&row(false), &Typed::default(), false),
            Some(Kind::Db)
        );
        assert_eq!(ask_before(&row(true), &Typed::default(), false), None);
        // No keychain this session: ask for what it would hold.
        assert_eq!(
            ask_before(&row(true), &Typed::default(), true),
            Some(Kind::Db)
        );
        // Typed already.
        assert_eq!(ask_before(&row(false), &typed(&[Kind::Db]), false), None);
        // A file has no database password.
        let file = RowFacts {
            is_file: true,
            ..row(false)
        };
        assert_eq!(ask_before(&file, &Typed::default(), true), None);
    }

    #[test]
    fn an_ssh_secret_is_asked_as_the_tui_asks_it() {
        let db = typed(&[Kind::Db]);
        let pw = tunnelled(SshAuth::Password, false);
        assert_eq!(ask_before(&pw, &db, false), Some(Kind::Ssh));
        assert_eq!(ask_before(&pw, &typed(&[Kind::Db, Kind::Ssh]), false), None);
        assert_eq!(
            ask_before(&tunnelled(SshAuth::Password, true), &db, false),
            None
        );
        assert_eq!(
            ask_before(&tunnelled(SshAuth::Password, true), &db, true),
            Some(Kind::Ssh)
        );
        // A key's passphrase only when one is saved and the store is gone.
        assert_eq!(ask_before(&tunnelled(SshAuth::Key, false), &db, true), None);
        assert_eq!(ask_before(&tunnelled(SshAuth::Key, true), &db, false), None);
        assert_eq!(
            ask_before(&tunnelled(SshAuth::Key, true), &db, true),
            Some(Kind::SshKey)
        );
    }

    #[test]
    fn next_step_after_a_failure() {
        let typed_none = Typed::default();
        assert_eq!(
            next(
                &row(true),
                &typed_none,
                "CREDENTIALS_REQUIRED",
                "password required"
            ),
            Next::Ask(Kind::Db)
        );
        assert_eq!(
            next(
                &row(true),
                &typed_none,
                "UNKNOWN_HOST_KEY",
                "… Fingerprint: SHA256:abc"
            ),
            Next::Trust("SHA256:abc".into())
        );
        assert_eq!(
            next(
                &row(true),
                &typed_none,
                "UNKNOWN_HOST_KEY",
                "no fingerprint"
            ),
            Next::Fail
        );
        assert_eq!(
            next(&row(true), &typed_none, "HOST_KEY_MISMATCH", "…"),
            Next::Fail
        );
        assert_eq!(
            next(&row(true), &typed_none, "SECRET_STORE_UNAVAILABLE", "…"),
            Next::NoStore
        );
        assert_eq!(next(&row(true), &typed_none, "AUTH_ERROR", "…"), Next::Fail);
        // Asked once: a password that was typed and still isn't enough fails.
        assert_eq!(
            next(
                &row(true),
                &typed(&[Kind::Db]),
                "CREDENTIALS_REQUIRED",
                "Enter the password."
            ),
            Next::Fail
        );
        // The SSH password, when Core says that's what is missing.
        let pw = tunnelled(SshAuth::Password, true);
        assert_eq!(
            next(
                &pw,
                &typed(&[Kind::Db]),
                "CREDENTIALS_REQUIRED",
                "SSH password required"
            ),
            Next::Ask(Kind::Ssh)
        );
    }

    /// Core's `CREDENTIALS_REQUIRED` messages (`seaquel-workspace`'s
    /// `connections::missing`): a password is asked for, a missing field
    /// isn't.
    #[test]
    fn only_a_missing_password_is_asked_for() {
        let none = Typed::default();
        let ask =
            |facts: &RowFacts, message: &str| next(facts, &none, "CREDENTIALS_REQUIRED", message);
        let pw = tunnelled(SshAuth::Password, false);
        assert_eq!(
            ask(
                &row(false),
                "Connection \"prod\" has no saved password. Open it in the Seaquel app, enter \
                 the password with \"Save password in keychain\" on, and connect once."
            ),
            Next::Ask(Kind::Db)
        );
        assert_eq!(
            ask(
                &pw,
                "Connection \"prod\" goes through an SSH tunnel whose password isn't saved. \
                 Open it in the Seaquel app, enter the SSH password with saving on, and connect \
                 once."
            ),
            Next::Ask(Kind::Ssh)
        );
        // The SSH password typed already: not the database's instead.
        assert_eq!(
            next(
                &pw,
                &typed(&[Kind::Ssh]),
                "CREDENTIALS_REQUIRED",
                "Enter the SSH password to connect through the SSH tunnel."
            ),
            Next::Fail
        );
        for field in ["host", "username", "SSH host", "SSH username"] {
            let message = format!(
                "Connection \"prod\" has no {field}. Open it in the Seaquel app, fill it in, \
                 and connect once."
            );
            assert_eq!(ask(&row(false), &message), Next::Fail, "{field}");
            assert_eq!(ask(&pw, &message), Next::Fail, "{field}");
        }
        assert_eq!(
            ask(
                &tunnelled(SshAuth::Key, false),
                "Connection \"prod\" uses SSH key authentication but has no key file. Open it \
                 in the Seaquel app, choose the key file, and connect once."
            ),
            Next::Fail
        );
    }

    #[test]
    fn the_password_hint_is_added_only_where_a_terminal_would_ask() {
        let hinted = |facts: &RowFacts, code: &str, message: &str| {
            not_interactive_hint(facts, "prod", CoreError::new(code, message)).message
        };
        assert_eq!(
            hinted(&row(false), "CREDENTIALS_REQUIRED", "Enter the password."),
            format!("Enter the password.{NO_PASSWORD_HINT}")
        );
        // A missing field, or a file's: no prompt would help.
        assert_eq!(
            hinted(
                &row(false),
                "CREDENTIALS_REQUIRED",
                "Enter the host to connect."
            ),
            "Enter the host to connect."
        );
        let file = RowFacts {
            is_file: true,
            ..row(false)
        };
        assert_eq!(
            hinted(&file, "CREDENTIALS_REQUIRED", "Enter the password."),
            "Enter the password."
        );
        assert_eq!(
            hinted(&row(true), "UNKNOWN_HOST_KEY", "unknown"),
            format!("unknown{NO_TRUST_HINT}")
        );
        assert_eq!(hinted(&row(true), "AUTH_ERROR", "denied"), "denied");
        // No keychain: a terminal run would ask for the saved password,
        // but not for a file's, which has none.
        assert_eq!(
            hinted(&row(true), "SECRET_STORE_UNAVAILABLE", "No keychain."),
            format!("No keychain.{NO_PASSWORD_HINT}")
        );
        assert_eq!(
            hinted(&file, "SECRET_STORE_UNAVAILABLE", "No keychain."),
            "No keychain."
        );
        let key_tunnel = RowFacts {
            is_file: true,
            ..tunnelled(SshAuth::Key, true)
        };
        assert_eq!(
            hinted(&key_tunnel, "SECRET_STORE_UNAVAILABLE", "No keychain."),
            format!("No keychain.{NO_PASSWORD_HINT}")
        );
    }

    #[test]
    fn giving_up_names_the_number_of_attempts() {
        assert_eq!(
            gave_up().message,
            format!("Gave up after {MAX_ATTEMPTS} attempts.")
        );
        assert_eq!(gave_up().message, "Gave up after 4 attempts.");
    }

    /// The connection's name is quoted in Core's messages; words in it
    /// don't count.
    #[test]
    fn words_in_the_connection_s_name_dont_steer_the_next_step() {
        let ssh_name = "SSH box";
        let missing_db = format!(
            "Connection {ssh_name:?} has no saved password. Open it in the Seaquel app, enter \
             the password with \"Save password in keychain\" on, and connect once."
        );
        // Without a tunnel the database's password is asked for, though the
        // name says "SSH".
        assert_eq!(
            next(
                &row(false),
                &Typed::default(),
                "CREDENTIALS_REQUIRED",
                &without_name(&missing_db, ssh_name)
            ),
            Next::Ask(Kind::Db)
        );
        assert_eq!(
            not_interactive_hint(
                &row(false),
                ssh_name,
                CoreError::new("CREDENTIALS_REQUIRED", missing_db.clone())
            )
            .message,
            format!("{missing_db}{NO_PASSWORD_HINT}")
        );

        let pw_name = "password vault";
        let missing_host = format!(
            "Connection {pw_name:?} has no host. Open it in the Seaquel app, fill it in, and \
             connect once."
        );
        // A missing host: nothing to ask, though the name says "password".
        assert_eq!(
            next(
                &row(false),
                &Typed::default(),
                "CREDENTIALS_REQUIRED",
                &without_name(&missing_host, pw_name)
            ),
            Next::Fail
        );
        assert_eq!(
            not_interactive_hint(
                &row(false),
                pw_name,
                CoreError::new("CREDENTIALS_REQUIRED", missing_host.clone())
            )
            .message,
            missing_host
        );
    }

    #[tokio::test]
    async fn drive_reads_core_s_message_without_the_name() {
        // A name with "SSH" on a row without a tunnel: the database's
        // password is asked for and sent.
        let name = "SSH box";
        let message = format!("Connection {name:?} has no saved password.");
        let mut answers: VecDeque<Result<(), CoreError>> =
            vec![err("CREDENTIALS_REQUIRED", &message), Ok(())].into();
        let sent = RefCell::new(Vec::new());
        let mut p = interactive(&[Some("pw")], &[]);
        drive(&row(true), name, &mut p, |secrets, _| {
            sent.borrow_mut().push(secrets);
            let answer = answers.pop_front().expect("an answer");
            async move { answer }
        })
        .await
        .unwrap();
        assert_eq!(p.asked, ["Password for SSH box: "]);
        assert_eq!(sent.borrow()[1], SuppliedSecrets::db("pw"));
    }

    /// A scripted attempt: answers in order, recording what each was sent.
    struct Attempts {
        answers: RefCell<VecDeque<Result<(), CoreError>>>,
        sent: RefCell<Vec<(SuppliedSecrets, HostKeyPolicy)>>,
    }

    impl Attempts {
        fn new(answers: Vec<Result<(), CoreError>>) -> Self {
            Self {
                answers: RefCell::new(answers.into()),
                sent: RefCell::new(Vec::new()),
            }
        }

        async fn run(&self, facts: &RowFacts, prompter: &mut Scripted) -> Result<(), CoreError> {
            drive(facts, "prod", prompter, |secrets, host_key| {
                self.sent.borrow_mut().push((secrets, host_key));
                let answer = self.answers.borrow_mut().pop_front().expect("an answer");
                async move { answer }
            })
            .await
        }
    }

    fn err(code: &str, message: &str) -> Result<(), CoreError> {
        Err(CoreError::new(code, message))
    }

    fn interactive(passwords: &[Option<&str>], confirms: &[bool]) -> Scripted {
        Scripted {
            passwords: passwords.iter().map(|p| p.map(str::to_string)).collect(),
            confirms: confirms.iter().copied().collect(),
            interactive: true,
            ..Scripted::default()
        }
    }

    #[tokio::test]
    async fn without_a_terminal_nothing_is_asked_and_one_attempt_is_made() {
        let attempts = Attempts::new(vec![err("CREDENTIALS_REQUIRED", "password required")]);
        let mut p = Scripted::default();
        let e = attempts.run(&row(false), &mut p).await.unwrap_err();
        assert_eq!(e.code, "CREDENTIALS_REQUIRED");
        assert!(p.asked.is_empty());
        assert_eq!(attempts.sent.borrow().len(), 1);
        assert_eq!(attempts.sent.borrow()[0].0, SuppliedSecrets::none());

        let e = not_interactive_hint(&row(false), "prod", e);
        assert!(e.message.ends_with(NO_PASSWORD_HINT), "{}", e.message);
    }

    #[tokio::test]
    async fn a_password_the_row_doesnt_save_is_asked_before_connecting() {
        let attempts = Attempts::new(vec![Ok(())]);
        let mut p = interactive(&[Some("hunter2")], &[]);
        attempts.run(&row(false), &mut p).await.unwrap();
        assert_eq!(p.asked, ["Password for prod: "]);
        assert_eq!(attempts.sent.borrow()[0].0, SuppliedSecrets::db("hunter2"));
    }

    #[tokio::test]
    async fn a_wrong_password_isnt_asked_for_again() {
        let attempts = Attempts::new(vec![err("AUTH_ERROR", "password authentication failed")]);
        let mut p = interactive(&[Some("wrong")], &[]);
        let e = attempts.run(&row(false), &mut p).await.unwrap_err();
        assert_eq!(e.code, "AUTH_ERROR");
        assert_eq!(p.asked.len(), 1);
        // Interactive: no hint.
        assert_eq!(e.message, "password authentication failed");
    }

    #[tokio::test]
    async fn giving_up_at_the_prompt_cancels() {
        let attempts = Attempts::new(vec![]);
        let mut p = interactive(&[None], &[]);
        let e = attempts.run(&row(false), &mut p).await.unwrap_err();
        assert_eq!(e.code, "CANCELLED");
        assert!(attempts.sent.borrow().is_empty());
    }

    #[tokio::test]
    async fn no_keychain_asks_for_the_saved_password() {
        let attempts = Attempts::new(vec![err("SECRET_STORE_UNAVAILABLE", "no keychain"), Ok(())]);
        let mut p = interactive(&[Some("typed")], &[]);
        attempts.run(&row(true), &mut p).await.unwrap();
        let sent = attempts.sent.borrow();
        assert_eq!(sent[0].0, SuppliedSecrets::none());
        assert_eq!(sent[1].0, SuppliedSecrets::db("typed"));
    }

    /// The keychain would hold both the database's and the SSH tunnel's
    /// password: each round asks for one, until both are typed.
    #[tokio::test]
    async fn no_keychain_asks_for_every_saved_secret_in_turn() {
        let attempts = Attempts::new(vec![
            err("SECRET_STORE_UNAVAILABLE", "no keychain"),
            err("SECRET_STORE_UNAVAILABLE", "no keychain"),
            Ok(()),
        ]);
        let mut p = interactive(&[Some("db-pw"), Some("ssh-pw")], &[]);
        attempts
            .run(&tunnelled(SshAuth::Password, true), &mut p)
            .await
            .unwrap();
        assert_eq!(p.asked, ["Password for prod: ", "SSH password for prod: "]);
        let sent: Vec<SuppliedSecrets> = attempts
            .sent
            .borrow()
            .iter()
            .map(|(s, _)| s.clone())
            .collect();
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[1], SuppliedSecrets::db("db-pw"));
        assert_eq!(
            sent[2],
            SuppliedSecrets {
                db: Some("db-pw".into()),
                ssh: Some("ssh-pw".into()),
                ssh_key: None,
            }
        );
    }

    #[tokio::test]
    async fn an_unknown_host_is_trusted_only_when_the_user_says_so() {
        let unknown = "Unknown host key. Fingerprint: SHA256:abc";
        let attempts = Attempts::new(vec![err("UNKNOWN_HOST_KEY", unknown), Ok(())]);
        let mut p = interactive(&[], &[true]);
        attempts
            .run(&tunnelled(SshAuth::Key, false), &mut p)
            .await
            .unwrap();
        assert!(
            p.asked[0].contains("bastion:2222") && p.asked[0].contains("SHA256:abc"),
            "{:?}",
            p.asked
        );
        let policies: Vec<HostKeyPolicy> = attempts
            .sent
            .borrow()
            .iter()
            .map(|(_, h)| h.clone())
            .collect();
        assert_eq!(
            policies,
            [
                HostKeyPolicy::KnownOnly,
                HostKeyPolicy::Trust("SHA256:abc".into())
            ]
        );

        let attempts = Attempts::new(vec![err("UNKNOWN_HOST_KEY", unknown)]);
        let mut p = interactive(&[], &[false]);
        let e = attempts
            .run(&tunnelled(SshAuth::Key, false), &mut p)
            .await
            .unwrap_err();
        assert_eq!(e.code, "UNKNOWN_HOST_KEY");
        assert_eq!(attempts.sent.borrow().len(), 1);
    }
}
