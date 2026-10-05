//! Startup, connecting and panels 1–3 in `update` (Task 3): the picker, the
//! connect sequence (password prompts, the keychain wait, host-key trust,
//! "Save password"), and the lists Core's answers fill. Pure, like the rest
//! of the model: every Core call is an [`Effect`], every answer a [`Msg`].
//!
//! **The connect sequence** (Decision 16): a saved connection whose
//! database password isn't saved asks for it first (SQLite and DuckDB
//! never do), then an SSH password the row doesn't save. The typed secrets
//! go to Core as `SuppliedSecrets`; nothing is saved before the connect
//! succeeds, and then only the ones whose box was ticked, through Core's
//! `connectionUpdate` (Decision 23). An unknown SSH host opens the trust
//! dialog, a refused password offers a retry, and a keychain call pending
//! for a while shows the wait box, where Esc gives the connect up. Typed
//! secrets are kept while the connection is up and dropped when it goes.

use super::app::{
    Attempt, Changed, Conn, ConnectCall, Effect, Load, Modal, Model, Msg, Panel, SaveCall,
    SavedTab, Stamp,
};
use super::dialogs::{self, CallError, Notice, PasswordPrompt, Pending, TrustPrompt};
use super::log::{LogLine, Tag};
use super::panels::{ConnItem, Row, TunnelAuth};
use super::picker::{self, Stage, Start};
use super::secrets::SecretKind;
use super::text;

/// How the TUI starts (`runtime::run_app` resolved the arguments).
pub fn start(model: &mut Model, start: Start) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(project) = start.project {
        effects.extend(set_project(model, &project));
    }
    if let Some(id) = start.connect {
        effects.extend(start_connect(model, &id));
    }
    if let Some(picker) = start.picker {
        model.modal = Some(Modal::Picker(picker));
        model.focus = Panel::Connection;
    }
    effects
}

/// A Core answer or event.
pub fn on_msg(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Library(Ok(library)) => {
            model.library = library;
            if let Some(Modal::Picker(p)) = &mut model.modal {
                let len = picker::picker_len(&model.library, p);
                p.selected = p.selected.min(len.saturating_sub(1));
            }
            Vec::new()
        }
        Msg::Library(Err(e)) => vec![failed_load("connections", &e)],
        Msg::Saved { project_id, result } => {
            if model.project.as_deref() != Some(project_id.as_str()) {
                return Vec::new();
            }
            match result {
                Ok(items) => {
                    model.saved_items = items;
                    model.refresh_lists();
                    Vec::new()
                }
                Err(e) => vec![failed_load("saved queries", &e)],
            }
        }
        Msg::History {
            connection_id,
            result,
        } => {
            if model.conn.id() != Some(connection_id.as_str()) {
                return Vec::new();
            }
            match result {
                Ok(items) => {
                    model.history_items = items;
                    model.refresh_lists();
                    Vec::new()
                }
                Err(e) => vec![failed_load("history", &e)],
            }
        }
        Msg::Schema {
            core_id,
            result,
            stamp,
        } => on_schema(model, &core_id, result, stamp),
        Msg::Connected {
            attempt,
            result,
            stamp,
        } => on_connected(model, attempt, result, stamp),
        Msg::PasswordSaved {
            connection_id,
            kinds,
            result,
        } => on_saved(model, &connection_id, &kinds, result),
        Msg::Keychain { pending, at } => {
            model.keychain = pending.then_some(at);
            if !pending {
                model.keychain_hidden = false;
                // The given-up call ended: later waits are the next
                // connect's again.
                model.keychain_stale = false;
            }
            Vec::new()
        }
        Msg::Changed(changed) => on_changed(model, changed),
        Msg::Closed {
            core_id,
            code,
            message,
        } => {
            let Conn::Connected { id, core_id: ours } = &model.conn else {
                return Vec::new();
            };
            if *ours != core_id {
                return Vec::new();
            }
            // Its DuckDB helper died (the desktop DuckDB helper plan,
            // Decision 7): as when a call meets it first ([`lost_by`]).
            if code == "CONNECTION_CLOSED" {
                return lost(model, CallError::new(code, message));
            }
            model.conn = Conn::Closed { id: id.clone() };
            model.typed = Default::default();
            super::browse::close(model);
            vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("connection closed", &code),
            )]
        }
        // `app::update` handles the rest.
        Msg::Key(_)
        | Msg::Mouse(_)
        | Msg::Paste(_)
        | Msg::Resize(..)
        | Msg::Tick(_)
        | Msg::Log(_)
        | Msg::Page { .. }
        | Msg::Meta { .. }
        | Msg::Columns { .. }
        | Msg::Planned { .. }
        | Msg::Applied { .. }
        | Msg::Run { .. }
        | Msg::Explained { .. }
        | Msg::QuerySaved { .. }
        | Msg::Edited { .. }
        | Msg::Generated { .. }
        | Msg::Mentions { .. }
        | Msg::DuckdbOffer { .. }
        | Msg::InstallProgress { .. }
        | Msg::Installed { .. } => Vec::new(),
    }
}

fn failed_load(what: &str, e: &CallError) -> Effect {
    Model::log_effect(Some(Tag::Error), text::failed_line(what, &e.code))
}

/// Panel 3 shows `project_id`; its saved queries load when it changes.
fn set_project(model: &mut Model, project_id: &str) -> Vec<Effect> {
    if model.project.as_deref() == Some(project_id) {
        return Vec::new();
    }
    model.project = Some(project_id.to_string());
    model.saved_items.clear();
    model.folded_folders.clear();
    model.saved.selected = 0;
    model.refresh_lists();
    vec![Effect::LoadSaved {
        project_id: project_id.to_string(),
    }]
}

/// Ends what panel 1 has: a connect in flight is dropped, a connection
/// disconnected, and its typed secrets, tables and history forgotten.
fn drop_connection(model: &mut Model) -> Vec<Effect> {
    let mut effects = Vec::new();
    match std::mem::replace(&mut model.conn, Conn::None) {
        Conn::Connecting(a) => effects.push(Effect::CancelConnect { attempt: a.attempt }),
        Conn::Connected { id, core_id } => {
            effects.push(Effect::Disconnect { core_id });
            let name = model
                .library
                .connection(&id)
                .map_or(id.clone(), |c| c.name.clone());
            effects.push(Model::log_effect(None, format!("disconnected {name}")));
        }
        Conn::None | Conn::Failed { .. } | Conn::Closed { .. } => {}
    }
    model.typed = Default::default();
    super::browse::close(model);
    model.schema.clear();
    model.column_loads.clear();
    model.schema_load = Load::Idle;
    model.history_items.clear();
    model.refresh_lists();
    effects
}

/// Connects saved connection `id` (the picker, `--connection`, and a
/// switch the staged changes' question let through).
pub(crate) fn start_connect(model: &mut Model, id: &str) -> Vec<Effect> {
    // Not while an apply runs on the connection it would drop (review I1).
    if let Err(effects) = super::commit::idle(model) {
        return effects;
    }
    let mut effects = drop_connection(model);
    let Some(row) = model.library.connection(id).cloned() else {
        model.modal = Some(Modal::Problem(dialogs::problem(
            &CallError::new("CONNECTION_NOT_FOUND", "The connection no longer exists."),
            None,
        )));
        return effects;
    };
    effects.extend(set_project(model, &row.project_id));
    effects.extend(proceed(
        model,
        Pending {
            connection_id: id.to_string(),
            ..Pending::default()
        },
    ));
    effects
}

/// The next step of a connect: a prompt for a secret the row doesn't save
/// and nothing typed yet covers, else the connect itself.
pub(crate) fn proceed(model: &mut Model, pending: Pending) -> Vec<Effect> {
    let Some(row) = model.library.connection(&pending.connection_id).cloned() else {
        return removed(model);
    };
    if let Some(kind) = to_ask(model, &row, &pending) {
        prompt(model, pending, kind, None);
        return Vec::new();
    }
    let attempt = model.next_attempt;
    model.next_attempt += 1;
    model.keychain_hidden = false;
    let call = ConnectCall {
        attempt,
        connection_id: pending.connection_id.clone(),
        secrets: pending.typed.clone(),
        trust: pending.trust.clone(),
    };
    model.conn = Conn::Connecting(Attempt { attempt, pending });
    vec![
        Effect::Connect(call),
        Model::log_effect(None, format!("connecting {}", row.name)),
    ]
}

/// The connection a connect was for is gone (removed elsewhere while its
/// helper downloaded, or before `r` reconnected it): the picker opens and
/// the command log says why (review M1 of the DuckDB helper plan's Task 7).
fn removed(model: &mut Model) -> Vec<Effect> {
    model.modal = Some(Modal::Picker(picker::open_picker(
        &model.library,
        model.project.as_deref(),
        &model.remembered,
    )));
    vec![Model::log_effect(
        Some(Tag::Error),
        text::CONNECTION_REMOVED.to_string(),
    )]
}

/// The secret to ask for before connecting, if any: one the row doesn't
/// save and nothing typed covers; and, once the store is known to be
/// unavailable (probe F4), one it would have read from there too.
fn to_ask(model: &Model, row: &ConnItem, pending: &Pending) -> Option<SecretKind> {
    let no_store = model.store_unavailable;
    if !row.is_file() && (!row.save_password || no_store) && pending.typed.db.is_none() {
        return Some(SecretKind::Db);
    }
    let tunnel = row.tunnel.as_ref()?;
    match tunnel.auth {
        TunnelAuth::Password
            if (!row.save_ssh_password || no_store) && pending.typed.ssh.is_none() =>
        {
            Some(SecretKind::Ssh)
        }
        TunnelAuth::Key
            if row.save_ssh_key_passphrase && no_store && pending.typed.ssh_key.is_none() =>
        {
            Some(SecretKind::SshKey)
        }
        _ => None,
    }
}

fn prompt(model: &mut Model, pending: Pending, kind: SecretKind, reason: Option<&'static str>) {
    // With no store in this session nothing can be saved, and the prompt
    // says why it asks for a saved password (probe F4).
    let can_save = !model.store_unavailable;
    let save = can_save && pending.save.contains(&kind);
    let reason = match reason {
        None if !can_save => Some(text::store_unavailable(model.store)),
        reason => reason,
    };
    model.modal = Some(Modal::Password(PasswordPrompt {
        pending,
        kind,
        input: super::secrets::Secret::with_room(),
        save,
        can_save,
        reason,
    }));
}

fn on_connected(
    model: &mut Model,
    attempt: u64,
    result: Result<String, CallError>,
    stamp: Stamp,
) -> Vec<Effect> {
    let current = matches!(&model.conn, Conn::Connecting(a) if a.attempt == attempt);
    if !current {
        // A connect given up or replaced: close what it opened.
        return match result {
            Ok(core_id) => vec![Effect::Disconnect { core_id }],
            Err(_) => Vec::new(),
        };
    }
    let Conn::Connecting(Attempt { pending, .. }) = std::mem::replace(&mut model.conn, Conn::None)
    else {
        return Vec::new();
    };
    let id = pending.connection_id.clone();
    let Some(row) = model.library.connection(&id).cloned() else {
        // Removed elsewhere while it connected: don't keep what opened.
        return match result {
            Ok(core_id) => vec![Effect::Disconnect { core_id }],
            Err(_) => Vec::new(),
        };
    };
    match result {
        Ok(core_id) => {
            let mut effects = vec![
                Effect::LoadSchema {
                    core_id: core_id.clone(),
                },
                Effect::LoadHistory {
                    connection_id: id.clone(),
                },
                Effect::Log(super::app::LogEntry {
                    tag: None,
                    text: text::connected_line(&row.name, row.engine_label()),
                    elapsed: Some(format!("{} ms", stamp.elapsed_ms)),
                }),
            ];
            model.conn = Conn::Connected {
                id: id.clone(),
                core_id,
            };
            model.typed = pending.typed.clone();
            model.schema.clear();
            model.column_loads.clear();
            model.schema_load = Load::Loading;
            model.history_items.clear();
            model.refresh_lists();
            model.remembered.last_project = Some(row.project_id.clone());
            model
                .remembered
                .last_connection
                .insert(row.project_id.clone(), id.clone());
            model.remember();
            // Only a typed, non-empty secret is saved (an empty one is
            // used for this connect and forgotten with it).
            let kinds: Vec<SecretKind> = pending
                .save
                .iter()
                .copied()
                .filter(|k| pending.typed.get(*k).is_some_and(|s| !s.is_empty()))
                .collect();
            if !kinds.is_empty() {
                model.saving = Some(id.clone());
                effects.push(Effect::SavePassword(SaveCall {
                    connection_id: id,
                    secrets: pending.typed.only(&kinds),
                    kinds,
                }));
            }
            effects
        }
        Err(e) => {
            model.conn = Conn::Failed { id };
            let mut effects = vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("connect", &e.code),
            )];
            effects.extend(failed(model, &row, pending, &e));
            effects
        }
    }
}

/// What a failed connect opens: the trust dialog, a prompt for what's
/// missing, DuckDB support's download, or the problem (with a retry when a
/// typed secret could fix it, or a reconnect when the DuckDB helper didn't
/// start).
fn failed(model: &mut Model, row: &ConnItem, pending: Pending, e: &CallError) -> Vec<Effect> {
    let ssh_kind = row.tunnel.as_ref().map(|t| match t.auth {
        TunnelAuth::Password => SecretKind::Ssh,
        TunnelAuth::Key => SecretKind::SshKey,
    });
    let retry_with = |kind: SecretKind, mut pending: Pending| {
        pending.typed.forget(kind);
        Some((kind, pending))
    };
    match e.code.as_str() {
        // DuckDB support isn't installed: offer its download (Q5 A), unless
        // it was just installed (then a download wouldn't help).
        "ENGINE_NOT_INSTALLED" if !pending.after_install => {
            return super::install::open(model, pending);
        }
        // The helper is there and checked but didn't answer in time (Task
        // 3's M5): connecting again may work; a download wouldn't.
        "ENGINE_UNAVAILABLE" => {
            let mut problem = dialogs::problem(e, None);
            problem.reconnect = Some(pending);
            model.modal = Some(Modal::Problem(problem));
        }
        // No store in this session: what it would hold is asked for
        // instead, as for a row that doesn't save it (probe F4).
        "SECRET_STORE_UNAVAILABLE" => {
            model.store_unavailable = true;
            match to_ask(model, row, &pending) {
                Some(kind) => prompt(model, pending, kind, None),
                None => model.modal = Some(Modal::Problem(dialogs::problem(e, None))),
            }
        }
        "UNKNOWN_HOST_KEY" => {
            if let (Some(fingerprint), Some(tunnel)) =
                (dialogs::fingerprint(&e.message), &row.tunnel)
            {
                model.modal = Some(Modal::Trust(TrustPrompt {
                    pending,
                    host: tunnel.host.clone(),
                    port: tunnel.port,
                    fingerprint,
                }));
                return Vec::new();
            }
            model.modal = Some(Modal::Problem(dialogs::problem(e, None)));
        }
        "CREDENTIALS_REQUIRED" => {
            let ssh_missing = ssh_kind == Some(SecretKind::Ssh)
                && pending.typed.ssh.is_none()
                && e.message.contains("SSH");
            if ssh_missing {
                prompt(model, pending, SecretKind::Ssh, None);
            } else if !row.is_file() && pending.typed.db.is_none() {
                prompt(model, pending, SecretKind::Db, None);
            } else {
                model.modal = Some(Modal::Problem(dialogs::problem(e, None)));
            }
        }
        code => {
            let ssh_auth = matches!(code, "AUTH_FAILED" | "KEY_LOAD_ERROR")
                || (code == "AUTH_ERROR"
                    && row.tunnel.is_some()
                    && !e.message.starts_with("Failed to connect to SQL Server"));
            let retry = if ssh_auth {
                ssh_kind.and_then(|kind| retry_with(kind, pending))
            } else if matches!(code, "CONNECTION_ERROR" | "AUTH_ERROR") && !row.is_file() {
                retry_with(SecretKind::Db, pending)
            } else {
                None
            };
            model.modal = Some(Modal::Problem(dialogs::problem(e, retry)));
        }
    }
    Vec::new()
}

fn on_schema(
    model: &mut Model,
    core_id: &str,
    result: Result<Vec<super::panels::TableItem>, CallError>,
    stamp: Stamp,
) -> Vec<Effect> {
    if model.conn.core_id() != Some(core_id) {
        return Vec::new();
    }
    match result {
        Ok(tables) => {
            // A list read again (`r`, a commit's DDL, another process's
            // change) may follow DDL: the columns read before go with the
            // old list and are read again when next needed (review I2).
            model.column_loads.clear();
            model.schema = tables;
            model.schema_load = Load::Loaded;
            model.refresh_lists();
            model.log.push(LogLine {
                time: stamp.time,
                tag: None,
                text: "schema tables".to_string(),
                elapsed: Some(format!("{} ms", stamp.elapsed_ms)),
            });
            Vec::new()
        }
        Err(e) => {
            model.schema_load = Load::Failed;
            model.log.push(LogLine {
                time: stamp.time,
                tag: Some(Tag::Error),
                text: text::failed_line("schema tables", &e.code),
                elapsed: None,
            });
            Vec::new()
        }
    }
}

fn on_saved(
    model: &mut Model,
    connection_id: &str,
    kinds: &[SecretKind],
    result: Result<(), CallError>,
) -> Vec<Effect> {
    if model.saving.as_deref() == Some(connection_id) {
        model.saving = None;
    }
    model.keychain_hidden = false;
    let notice = match result {
        Ok(()) => {
            if let Some(row) = model
                .library
                .connections
                .iter_mut()
                .find(|c| c.id == connection_id)
            {
                for kind in kinds {
                    match kind {
                        SecretKind::Db => row.save_password = true,
                        SecretKind::Ssh => row.save_ssh_password = true,
                        SecretKind::SshKey => row.save_ssh_key_passphrase = true,
                    }
                }
            }
            text::password_saved(cfg!(target_os = "macos"))
        }
        Err(e) => text::password_not_saved(&e.message),
    };
    let tag = if notice.starts_with("Saved") {
        None
    } else {
        Some(Tag::Error)
    };
    let effect = Model::log_effect(tag, notice.clone());
    if model.modal.is_none() {
        model.modal = Some(Modal::Notice(Notice(notice)));
    }
    vec![effect]
}

fn on_changed(model: &mut Model, changed: Changed) -> Vec<Effect> {
    let library = || Effect::LoadLibrary;
    let saved = |m: &Model| {
        m.project
            .clone()
            .map(|project_id| Effect::LoadSaved { project_id })
    };
    let history = |m: &Model| {
        m.conn.id().map(|id| Effect::LoadHistory {
            connection_id: id.to_string(),
        })
    };
    match changed {
        Changed::Library => vec![library()],
        Changed::SavedQueries => saved(model).into_iter().collect(),
        Changed::History => history(model).into_iter().collect(),
        Changed::External => std::iter::once(library())
            .chain(saved(model))
            .chain(history(model))
            .collect(),
    }
}

/// Panel 1's Enter.
pub fn open_picker(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = super::commit::idle(model) {
        return effects;
    }
    model.modal = Some(Modal::Picker(picker::open_picker(
        &model.library,
        model.project.as_deref(),
        &model.remembered,
    )));
    Vec::new()
}

pub fn picker_step(model: &mut Model, down: bool) {
    if let Some(Modal::Picker(p)) = &mut model.modal {
        let len = picker::picker_len(&model.library, p);
        p.selected = if down {
            (p.selected + 1).min(len.saturating_sub(1))
        } else {
            p.selected.saturating_sub(1)
        };
    }
}

pub fn picker_back(model: &mut Model) {
    let Some(Modal::Picker(p)) = &model.modal else {
        return;
    };
    model.modal = match &p.stage {
        Stage::Projects => None,
        Stage::Connections { project_id } => Some(Modal::Picker(picker::Picker {
            stage: Stage::Projects,
            selected: model
                .library
                .projects
                .iter()
                .position(|q| &q.id == project_id)
                .unwrap_or(0),
        })),
    };
}

pub fn picker_choose(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = super::commit::idle(model) {
        model.modal = None;
        return effects;
    }
    let Some(Modal::Picker(p)) = &model.modal else {
        return Vec::new();
    };
    if let Some(project) = picker::selected_project(&model.library, p) {
        let next = picker::connections_stage(&model.library, &project.id, &model.remembered);
        model.modal = Some(Modal::Picker(next));
        return Vec::new();
    }
    let Some(conn) = picker::selected_connection(&model.library, p) else {
        return Vec::new();
    };
    let id = conn.id.clone();
    model.modal = None;
    // Changes staged on another connection: keep them, discard them, or
    // stay (Task 5).
    match model.queue.connection().map(str::to_string) {
        Some(from) if from != id && !model.queue.is_empty() => {
            return super::commit::ask_switch(model, &from, Some(id));
        }
        // Only undo steps left there: nothing to keep.
        Some(from) if from != id => model.queue.clear(),
        _ => {}
    }
    start_connect(model, &id)
}

/// Enter in a list panel: a group folds or unfolds, anything else opens in
/// the main view.
pub fn open(model: &mut Model) -> Vec<Effect> {
    let group = match model.focus {
        Panel::Tables => model.selected_table_row(),
        Panel::Saved if model.saved_tab == SavedTab::Saved => model.selected_saved_row(),
        _ => None,
    };
    if let Some(Row::Group { name, .. }) = group {
        let folded = if model.focus == Panel::Tables {
            &mut model.folded_schemas
        } else {
            &mut model.folded_folders
        };
        if !folded.remove(&name) {
            folded.insert(name);
        }
        model.refresh_lists();
        return Vec::new();
    }
    if model.focus == Panel::Tables && matches!(group, Some(Row::Item(_))) {
        return super::browse::open(model);
    }
    model.focus_panel(Panel::Main);
    Vec::new()
}

/// `r` in panel 2 or 3.
pub fn reload(model: &mut Model) -> Vec<Effect> {
    match model.focus {
        Panel::Tables => match model.conn.core_id() {
            Some(core_id) => {
                model.schema_load = Load::Loading;
                vec![Effect::LoadSchema {
                    core_id: core_id.to_string(),
                }]
            }
            None => vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_LOG)],
        },
        Panel::Saved => {
            let mut effects = Vec::new();
            if let Some(project_id) = model.project.clone() {
                effects.push(Effect::LoadSaved { project_id });
            }
            if let Some(id) = model.conn.id() {
                effects.push(Effect::LoadHistory {
                    connection_id: id.to_string(),
                });
            }
            effects
        }
        _ => Vec::new(),
    }
}

pub fn submit_password(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::Password(prompt)) = model.modal.take() else {
        return Vec::new();
    };
    let PasswordPrompt {
        mut pending,
        kind,
        input,
        save,
        can_save: prompt_can_save,
        ..
    } = prompt;
    pending.typed.set(kind, input);
    if save && prompt_can_save {
        pending.save.insert(kind);
    } else {
        pending.save.remove(&kind);
    }
    proceed(model, pending)
}

pub fn trust(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::Trust(prompt)) = model.modal.take() else {
        return Vec::new();
    };
    let mut pending = prompt.pending;
    pending.trust = Some(prompt.fingerprint);
    proceed(model, pending)
}

pub fn retry(model: &mut Model) {
    let Some(Modal::Problem(problem)) = &model.modal else {
        return;
    };
    let Some((kind, pending)) = problem.retry.clone() else {
        return;
    };
    prompt(model, pending, kind, Some(text::PASSWORD_AGAIN));
}

/// `r` on a problem a reconnect may fix (a DuckDB helper that didn't
/// start, or one that stopped): the same connect again.
pub fn reconnect(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::Problem(problem)) = &model.modal else {
        return Vec::new();
    };
    let Some(pending) = problem.reconnect.clone() else {
        return Vec::new();
    };
    if let Err(effects) = super::commit::idle(model) {
        return effects;
    }
    model.modal = None;
    let mut effects = drop_connection(model);
    effects.extend(proceed(model, pending));
    effects
}

/// The `CONNECTION_CLOSED` error in `msg`, when it answers a call on the
/// connected connection: the DuckDB helper behind it stopped (the DuckDB
/// helper plan, Decision 8). Core also announces that as `ConnectionClosed`
/// (the desktop DuckDB helper plan, Decision 7), which `Msg::Closed` takes
/// to [`lost`] too; whichever arrives first wins. Only answers about the
/// connection panel 1 has now count, by Core's id.
pub(crate) fn lost_by(model: &Model, msg: &Msg) -> Option<CallError> {
    use super::query::RunMsg;
    let core_id = model.conn.core_id()?;
    let (current, error) = match msg {
        Msg::Schema {
            core_id: c,
            result: Err(e),
            ..
        }
        | Msg::Meta {
            core_id: c,
            result: Err(e),
            ..
        }
        | Msg::Columns {
            core_id: c,
            result: Err(e),
            ..
        } => (c == core_id, e),
        Msg::Page {
            op, result: Err(e), ..
        } => (
            model.browse.loading == Some(*op)
                && model
                    .browse
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.core_id == core_id),
            e,
        ),
        // An apply's own outcome can carry it too (probe F1): Core answers
        // a lost connection mid-apply as a failed change.
        Msg::Applied {
            op,
            result:
                Err(e)
                | Ok(super::commit::Applied::Applied {
                    failed: Some(super::commit::Failure { error: e, .. }),
                    ..
                }),
            ..
        } => (
            model
                .committing
                .as_ref()
                .is_some_and(|c| c.op == *op && c.core_id == core_id),
            e,
        ),
        Msg::Run {
            tab,
            op,
            event: RunMsg::Failed { error, .. } | RunMsg::Refused { error, .. },
        } => (
            model
                .query
                .tabs
                .iter()
                .find(|t| t.id == *tab)
                .and_then(|t| t.op.as_ref())
                .is_some_and(|o| o.op == *op && o.core_id == core_id),
            error,
        ),
        _ => return None,
    };
    (current && error.code == "CONNECTION_CLOSED").then(|| error.clone())
}

/// The connection's helper stopped: panel 1 says closed, and the problem
/// offers the same connect again, with the secrets typed for it.
pub(crate) fn lost(model: &mut Model, error: CallError) -> Vec<Effect> {
    let Conn::Connected { id, core_id } = std::mem::replace(&mut model.conn, Conn::None) else {
        return Vec::new();
    };
    let pending = Pending {
        connection_id: id.clone(),
        typed: std::mem::take(&mut model.typed),
        ..Pending::default()
    };
    model.conn = Conn::Closed { id };
    super::browse::close(model);
    let mut problem = dialogs::problem(&error, None);
    problem.reconnect = Some(pending);
    model.modal = Some(Modal::Problem(problem));
    vec![
        // Core still lists it: let it go.
        Effect::Disconnect { core_id },
        Model::log_effect(
            Some(Tag::Error),
            text::failed_line("connection closed", &error.code),
        ),
    ]
}

/// Esc on the keychain box, or its limit passed: a connect waiting on the
/// keychain is given up (`SECRET_UNREADABLE`); a save goes on, with the box
/// hidden.
pub fn give_up_keychain(model: &mut Model) -> Vec<Effect> {
    match &model.conn {
        Conn::Connecting(a) => {
            let attempt = a.attempt;
            let id = a.pending.connection_id.clone();
            model.conn = Conn::Failed { id };
            // The keychain call it waited on may stay pending (its dialog
            // is still up): it isn't the next connect's.
            model.keychain_stale = true;
            model.modal = Some(Modal::Problem(dialogs::problem(
                &CallError::new("SECRET_UNREADABLE", text::keychain_gave_up(model.store)),
                None,
            )));
            vec![
                Effect::CancelConnect { attempt },
                Model::log_effect(
                    Some(Tag::Error),
                    text::failed_line("connect", "SECRET_UNREADABLE"),
                ),
            ]
        }
        _ => {
            if model.saving.is_some() {
                model.keychain_hidden = true;
            }
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crossterm::event::KeyCode;

    use super::*;
    use crate::state::app::{
        Attempt, Changed, Conn, ConnectCall, Load, Modal, Panel, SaveCall, SavedTab, Stamp,
        TablesTab,
    };
    use crate::state::dialogs::CallError;
    use crate::state::keymap::BarContext;
    use crate::state::log::Tag;
    use crate::state::panels::{HistoryItem, Row, SavedItem, TableItem, TableKind, TunnelAuth};
    use crate::state::picker::{Picker, Stage};
    use crate::state::secrets::{Secret, SecretKind, Typed};
    use crate::testing::fixtures::{library, model};
    use crate::testing::keys::{ctrl, key, press};

    fn keys(m: &mut Model, typed: &str) -> Vec<Effect> {
        typed.chars().flat_map(|c| update_(m, key(c))).collect()
    }

    fn update_(m: &mut Model, msg: Msg) -> Vec<Effect> {
        crate::state::app::update(m, msg)
    }

    fn enter(m: &mut Model) -> Vec<Effect> {
        update_(m, press(KeyCode::Enter))
    }

    fn esc(m: &mut Model) -> Vec<Effect> {
        update_(m, press(KeyCode::Esc))
    }

    /// A model with the fixture library, on project-a, nothing connected.
    fn ready() -> Model {
        let mut m = model();
        update_(&mut m, Msg::Library(Ok(library())));
        m.project = Some("project-a".into());
        m
    }

    fn connect_effect(effects: &[Effect]) -> Option<&ConnectCall> {
        effects.iter().find_map(|e| match e {
            Effect::Connect(call) => Some(call),
            _ => None,
        })
    }

    fn connected(m: &mut Model, attempt: u64, core_id: &str) -> Vec<Effect> {
        update_(
            m,
            Msg::Connected {
                attempt,
                result: Ok(core_id.into()),
                stamp: Stamp::default(),
            },
        )
    }

    fn failed(m: &mut Model, attempt: u64, code: &str, message: &str) -> Vec<Effect> {
        update_(
            m,
            Msg::Connected {
                attempt,
                result: Err(CallError::new(code, message)),
                stamp: Stamp::default(),
            },
        )
    }

    fn connecting_attempt(m: &Model) -> u64 {
        match &m.conn {
            Conn::Connecting(Attempt { attempt, .. }) => *attempt,
            other => panic!("not connecting: {other:?}"),
        }
    }

    /// Starts `id` from the picker on its project.
    fn pick(m: &mut Model, project_index: usize, conn_index: usize) -> Vec<Effect> {
        keys(m, "1");
        enter(m);
        assert!(matches!(m.modal, Some(Modal::Picker(_))), "{:?}", m.modal);
        for _ in 0..project_index {
            keys(m, "j");
        }
        enter(m);
        for _ in 0..conn_index {
            keys(m, "j");
        }
        enter(m)
    }

    // ── The picker ──

    #[test]
    fn panel_one_s_enter_opens_the_picker_projects_then_connections() {
        let mut m = ready();
        keys(&mut m, "1");
        assert!(enter(&mut m).is_empty());
        assert_eq!(
            m.modal,
            Some(Modal::Picker(Picker {
                stage: Stage::Projects,
                selected: 0
            }))
        );
        assert_eq!(m.bar_context(), BarContext::Picker);
        keys(&mut m, "jjj");
        let Some(Modal::Picker(p)) = &m.modal else {
            panic!()
        };
        assert_eq!(p.selected, 1, "two projects: the selection stops");
        enter(&mut m);
        assert_eq!(
            m.modal,
            Some(Modal::Picker(Picker {
                stage: Stage::Connections {
                    project_id: "project-b".into()
                },
                selected: 0
            }))
        );
        // Esc steps back, then closes.
        esc(&mut m);
        assert!(matches!(
            &m.modal,
            Some(Modal::Picker(Picker {
                stage: Stage::Projects,
                selected: 1
            }))
        ));
        esc(&mut m);
        assert_eq!(m.modal, None);
    }

    #[test]
    fn choosing_a_file_connection_connects_at_once_and_loads_its_project() {
        let mut m = ready();
        // project-b's first connection is the SQLite file.
        let effects = pick(&mut m, 1, 0);
        assert_eq!(m.modal, None);
        let call = connect_effect(&effects).expect("connects");
        assert_eq!(call.connection_id, "conn-file");
        assert_eq!(call.secrets, Typed::default());
        assert_eq!(call.trust, None);
        assert!(effects.contains(&Effect::LoadSaved {
            project_id: "project-b".into()
        }));
        assert_eq!(m.project.as_deref(), Some("project-b"));
        assert!(matches!(m.conn, Conn::Connecting(_)));
    }

    #[test]
    fn a_saved_password_connects_without_asking() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let call = connect_effect(&effects).expect("connects");
        assert_eq!(call.connection_id, "conn-saved");
        assert_eq!(call.secrets, Typed::default());
        // Same project: nothing to reload.
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::LoadSaved { .. })));
    }

    #[test]
    fn a_project_with_no_connections_chooses_nothing() {
        let mut m = ready();
        m.library
            .connections
            .retain(|c| c.project_id != "project-b");
        let effects = pick(&mut m, 1, 0);
        assert!(effects.is_empty());
        assert!(matches!(m.modal, Some(Modal::Picker(_))));
    }

    // ── Passwords ──

    #[test]
    fn an_unsaved_password_is_asked_first_masked_and_esc_cancels() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 1);
        assert!(connect_effect(&effects).is_none());
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Db);
        assert!(!prompt.save);
        // Global keys type into the prompt.
        keys(&mut m, "q1?");
        let text = format!("{m:?}");
        assert!(!text.contains("q1?"), "the model's Debug shows a password");
        assert!(text.contains("<password>"));
        assert_eq!(m.focus, Panel::Connection);
        update_(&mut m, press(KeyCode::Backspace));
        assert!(esc(&mut m).is_empty());
        assert_eq!(m.modal, None);
        assert_eq!(m.conn, Conn::None);
        assert!(m.typed.is_empty());
    }

    #[test]
    fn the_typed_password_connects_and_is_saved_only_when_ticked_and_connected() {
        for (tick, outcome) in [
            (true, Ok("core-1")),
            (false, Ok("core-1")),
            (true, Err("CONNECTION_ERROR")),
        ] {
            let mut m = ready();
            pick(&mut m, 0, 1);
            keys(&mut m, "s3cret");
            if tick {
                update_(&mut m, press(KeyCode::Tab));
            }
            let effects = enter(&mut m);
            let call = connect_effect(&effects).expect("connects");
            assert_eq!(call.secrets.db.as_ref().map(Secret::expose), Some("s3cret"));
            assert_eq!(m.modal, None);
            let attempt = call.attempt;
            let effects = match outcome {
                Ok(core) => connected(&mut m, attempt, core),
                Err(code) => failed(&mut m, attempt, code, "password authentication failed"),
            };
            let saves: Vec<_> = effects
                .iter()
                .filter_map(|e| match e {
                    Effect::SavePassword(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();
            if tick && outcome.is_ok() {
                assert_eq!(
                    saves,
                    [SaveCall {
                        connection_id: "conn-ask".into(),
                        kinds: vec![SecretKind::Db],
                        secrets: {
                            let mut t = Typed::default();
                            t.set(SecretKind::Db, Secret::new("s3cret"));
                            t
                        },
                    }]
                );
                assert_eq!(m.saving.as_deref(), Some("conn-ask"));
            } else {
                assert!(saves.is_empty(), "{tick} {outcome:?}");
            }
        }
    }

    #[test]
    fn connected_loads_tables_and_history_and_remembers() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        let effects = connected(&mut m, attempt, "core-7");
        assert_eq!(
            m.conn,
            Conn::Connected {
                id: "conn-saved".into(),
                core_id: "core-7".into()
            }
        );
        assert!(effects.contains(&Effect::LoadSchema {
            core_id: "core-7".into()
        }));
        assert!(effects.contains(&Effect::LoadHistory {
            connection_id: "conn-saved".into()
        }));
        assert_eq!(m.schema_load, Load::Loading);
        assert_eq!(m.remembered.last_project.as_deref(), Some("project-a"));
        assert_eq!(
            m.remembered
                .last_connection
                .get("project-a")
                .map(String::as_str),
            Some("conn-saved")
        );
        assert!(m.remember_dirty.is_some());
        assert!(effects.iter().any(|e| matches!(e, Effect::Log(_))));
    }

    #[test]
    fn an_auth_failure_offers_a_retry_with_a_typed_password() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        failed(
            &mut m,
            attempt,
            "CONNECTION_ERROR",
            "password authentication failed",
        );
        assert_eq!(
            m.conn,
            Conn::Failed {
                id: "conn-saved".into()
            }
        );
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert!(p.message.contains("password authentication failed"));
        assert_eq!(m.bar_context(), BarContext::ProblemRetry);
        keys(&mut m, "r");
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Db);
        // It knows the connect failed, not that the password was wrong.
        let reason = prompt.reason.expect("says why it asks again");
        assert!(
            !reason.contains("refused") && !reason.contains("wrong"),
            "{reason}"
        );
        keys(&mut m, "pw");
        let effects = enter(&mut m);
        let call = connect_effect(&effects).expect("connects again");
        assert_eq!(call.secrets.db.as_ref().map(Secret::expose), Some("pw"));
        assert!(call.attempt > attempt);
    }

    /// Probe F4: a store that isn't there (a headless Linux host over SSH)
    /// counts as no saved password: the TUI asks, with Save password off
    /// and disabled, says why, and connects with what was typed. It asks
    /// first from then on, for every connection whose password the store
    /// would hold, and never saves.
    #[test]
    fn an_unavailable_store_asks_for_the_password_and_never_saves() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        assert!(failed(
            &mut m,
            attempt,
            "SECRET_STORE_UNAVAILABLE",
            "Seaquel couldn't reach the system keyring",
        )
        .iter()
        .all(|e| !matches!(e, Effect::Connect(_))));
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Db);
        assert!(!prompt.save && !prompt.can_save);
        assert_eq!(prompt.reason, Some(text::store_unavailable(m.store)));
        assert_eq!(m.bar_context(), BarContext::PasswordNoSave);
        // Its toggle doesn't tick it.
        update_(&mut m, press(KeyCode::Tab));
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert!(!prompt.save);
        keys(&mut m, "pw");
        let effects = enter(&mut m);
        let call = connect_effect(&effects).expect("connects with the typed password");
        assert_eq!(call.secrets.db.as_ref().map(Secret::expose), Some("pw"));
        let attempt = call.attempt;
        let effects = connected(&mut m, attempt, "core-1");
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::SavePassword(_))),
            "{effects:?}"
        );
        // Another connection that saves its password asks first now.
        let effects = pick(&mut m, 1, 2);
        assert!(connect_effect(&effects).is_none(), "{effects:?}");
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.pending.connection_id, "conn-key");
        assert!(!prompt.can_save);
    }

    /// A file connection reads no secret: nothing to ask, so the problem
    /// shows instead of a loop of connects.
    #[test]
    fn an_unavailable_store_with_nothing_to_ask_shows_the_problem() {
        let mut m = ready();
        let effects = pick(&mut m, 1, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        let effects = failed(&mut m, attempt, "SECRET_STORE_UNAVAILABLE", "no keyring");
        assert!(connect_effect(&effects).is_none());
        assert!(matches!(m.modal, Some(Modal::Problem(_))), "{:?}", m.modal);
    }

    #[test]
    fn a_file_connection_s_failure_has_no_retry() {
        let mut m = ready();
        let effects = pick(&mut m, 1, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        failed(
            &mut m,
            attempt,
            "CONNECTION_ERROR",
            "unable to open database file",
        );
        assert_eq!(m.bar_context(), BarContext::Problem);
        keys(&mut m, "r");
        assert!(matches!(m.modal, Some(Modal::Problem(_))));
        enter(&mut m);
        assert_eq!(m.modal, None);
    }

    #[test]
    fn credentials_required_asks_for_what_is_missing() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        failed(
            &mut m,
            attempt,
            "CREDENTIALS_REQUIRED",
            "Enter the password.",
        );
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Db);
    }

    #[test]
    fn an_ssh_password_is_asked_after_the_database_s() {
        let mut m = ready();
        // project-b's second connection: database and SSH passwords unsaved.
        pick(&mut m, 1, 1);
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Db);
        keys(&mut m, "db");
        update_(&mut m, press(KeyCode::Tab));
        assert!(connect_effect(&enter(&mut m)).is_none());
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::Ssh);
        keys(&mut m, "ssh");
        update_(&mut m, press(KeyCode::Tab));
        let effects = enter(&mut m);
        let call = connect_effect(&effects).unwrap().clone();
        assert_eq!(call.secrets.db.as_ref().map(Secret::expose), Some("db"));
        assert_eq!(call.secrets.ssh.as_ref().map(Secret::expose), Some("ssh"));
        let effects = connected(&mut m, call.attempt, "core-1");
        let save = effects
            .iter()
            .find_map(|e| match e {
                Effect::SavePassword(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(save.kinds, [SecretKind::Db, SecretKind::Ssh]);
    }

    #[test]
    fn an_ssh_auth_failure_retries_with_the_ssh_secret() {
        let mut m = ready();
        let effects = pick(&mut m, 1, 2);
        let call = connect_effect(&effects).expect("key auth asks nothing first");
        failed(
            &mut m,
            call.attempt,
            "AUTH_FAILED",
            "Key authentication failed",
        );
        keys(&mut m, "r");
        let Some(Modal::Password(prompt)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(prompt.kind, SecretKind::SshKey);
        assert_eq!(
            m.library
                .connection("conn-key")
                .unwrap()
                .tunnel
                .as_ref()
                .unwrap()
                .auth,
            TunnelAuth::Key
        );
    }

    // ── Host keys ──

    #[test]
    fn an_unknown_host_key_asks_to_trust_its_fingerprint() {
        let mut m = ready();
        let effects = pick(&mut m, 1, 2);
        let call = connect_effect(&effects).unwrap().clone();
        failed(
            &mut m,
            call.attempt,
            "UNKNOWN_HOST_KEY",
            "The host key for 127.0.0.1:2222 is not in known_hosts.\nFingerprint: SHA256:fp",
        );
        let Some(Modal::Trust(t)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(
            (t.host.as_str(), t.port, t.fingerprint.as_str()),
            ("bastion.example", 2222, "SHA256:fp")
        );
        // Esc doesn't trust.
        let mut cancelled = m.clone();
        assert!(esc(&mut cancelled).is_empty());
        assert_eq!(cancelled.modal, None);
        // `t` connects again, trusting exactly that key.
        let effects = keys(&mut m, "t");
        let again = connect_effect(&effects).unwrap();
        assert_eq!(again.trust.as_deref(), Some("SHA256:fp"));
        assert!(again.attempt > call.attempt);
    }

    #[test]
    fn a_changed_host_key_has_no_trust_button() {
        let mut m = ready();
        let effects = pick(&mut m, 1, 2);
        let call = connect_effect(&effects).unwrap().clone();
        failed(
            &mut m,
            call.attempt,
            "HOST_KEY_MISMATCH",
            "does not match\nFingerprint: SHA256:other",
        );
        assert_eq!(m.bar_context(), BarContext::Problem);
        assert!(keys(&mut m, "tr").is_empty());
        assert!(matches!(m.modal, Some(Modal::Problem(_))));
    }

    // ── The keychain ──

    #[test]
    fn the_keychain_box_shows_after_a_moment_and_esc_gives_up() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        let t0 = Instant::now();
        update_(
            &mut m,
            Msg::Keychain {
                pending: true,
                at: t0,
            },
        );
        update_(&mut m, Msg::Tick(t0 + Duration::from_millis(100)));
        assert!(!m.keychain_box(), "a quick read doesn't flash it");
        update_(&mut m, Msg::Tick(t0 + Duration::from_millis(400)));
        assert!(m.keychain_box());
        assert_eq!(m.bar_context(), BarContext::Keychain);
        // Nothing but Esc reaches anything.
        assert!(keys(&mut m, "1q").is_empty());
        let effects = esc(&mut m);
        assert!(effects.contains(&Effect::CancelConnect { attempt }));
        assert_eq!(
            m.conn,
            Conn::Failed {
                id: "conn-saved".into()
            }
        );
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.code, "SECRET_UNREADABLE");
        // The late answer of the dropped connect changes nothing.
        let mut after = m.clone();
        connected(&mut after, attempt, "core-late");
        assert_eq!(after.conn, m.conn);
    }

    #[test]
    fn the_keychain_wait_gives_up_after_its_limit() {
        let mut m = ready();
        let effects = pick(&mut m, 0, 0);
        let attempt = connect_effect(&effects).unwrap().attempt;
        let t0 = Instant::now();
        update_(
            &mut m,
            Msg::Keychain {
                pending: true,
                at: t0,
            },
        );
        let effects = update_(&mut m, Msg::Tick(t0 + Duration::from_secs(301)));
        assert!(effects.contains(&Effect::CancelConnect { attempt }));
    }

    #[test]
    fn a_late_answer_for_a_dropped_connect_is_disconnected() {
        let mut m = ready();
        let first = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        let second = connect_effect(&pick(&mut m, 1, 0)).unwrap().attempt;
        assert!(second > first);
        let effects = connected(&mut m, first, "core-old");
        assert_eq!(
            effects,
            [Effect::Disconnect {
                core_id: "core-old".into()
            }]
        );
        assert_eq!(connecting_attempt(&m), second);
    }

    #[test]
    fn the_box_shows_during_a_save_and_esc_only_hides_it() {
        let mut m = ready();
        pick(&mut m, 0, 1);
        keys(&mut m, "pw");
        update_(&mut m, press(KeyCode::Tab));
        let attempt = connect_effect(&enter(&mut m)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        let t0 = Instant::now();
        update_(
            &mut m,
            Msg::Keychain {
                pending: true,
                at: t0,
            },
        );
        update_(&mut m, Msg::Tick(t0 + Duration::from_millis(400)));
        assert!(m.keychain_box(), "a save waits on the keychain too");
        assert!(esc(&mut m).is_empty(), "the save goes on");
        assert!(!m.keychain_box());
        assert_eq!(m.saving.as_deref(), Some("conn-ask"));
    }

    #[test]
    fn a_saved_password_sets_the_flag_and_says_so() {
        let mut m = ready();
        pick(&mut m, 0, 1);
        keys(&mut m, "pw");
        update_(&mut m, press(KeyCode::Tab));
        let attempt = connect_effect(&enter(&mut m)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        update_(
            &mut m,
            Msg::PasswordSaved {
                connection_id: "conn-ask".into(),
                kinds: vec![SecretKind::Db],
                result: Ok(()),
            },
        );
        assert_eq!(m.saving, None);
        assert!(m.library.connection("conn-ask").unwrap().save_password);
        let Some(Modal::Notice(n)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert!(n.0.starts_with("Saved."), "{}", n.0);
        // Still connected, with the session's password.
        assert!(m.conn.core_id().is_some());
    }

    #[test]
    fn a_failed_save_keeps_the_flag_off_and_the_connection_up() {
        let mut m = ready();
        pick(&mut m, 0, 1);
        keys(&mut m, "pw");
        update_(&mut m, press(KeyCode::Tab));
        let attempt = connect_effect(&enter(&mut m)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        update_(
            &mut m,
            Msg::PasswordSaved {
                connection_id: "conn-ask".into(),
                kinds: vec![SecretKind::Db],
                result: Err(CallError::new("SECRET_STORE_ERROR", "denied")),
            },
        );
        assert!(!m.library.connection("conn-ask").unwrap().save_password);
        let Some(Modal::Notice(n)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert!(n.0.contains("wasn't saved"), "{}", n.0);
        assert!(n.0.contains("denied"), "{}", n.0);
        assert_eq!(
            m.conn,
            Conn::Connected {
                id: "conn-ask".into(),
                core_id: "core-1".into()
            }
        );
    }

    // ── Disconnects ──

    #[test]
    fn switching_connections_disconnects_and_drops_typed_passwords() {
        let mut m = ready();
        pick(&mut m, 0, 1);
        keys(&mut m, "pw");
        let attempt = connect_effect(&enter(&mut m)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        assert!(!m.typed.is_empty(), "kept for the session");
        let effects = pick(&mut m, 0, 0);
        assert!(effects.contains(&Effect::Disconnect {
            core_id: "core-1".into()
        }));
        assert!(m.typed.is_empty());
        assert!(m.schema.is_empty());
    }

    /// Core announces a connection whose DuckDB helper died
    /// (`CONNECTION_CLOSED`, the desktop DuckDB helper plan, Decision 7),
    /// possibly before the call that met it answers: the same "connection
    /// was lost" dialog with Reconnect as a failed call gives, typed
    /// secrets kept for the reconnect.
    #[test]
    fn a_lost_helper_core_announces_offers_to_reconnect() {
        let mut m = ready();
        let attempt = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        m.typed.set(SecretKind::Db, Secret::new("pw"));
        update_(
            &mut m,
            Msg::Closed {
                core_id: "core-1".into(),
                code: "CONNECTION_CLOSED".into(),
                message: "The DuckDB helper stopped (signal 9). Reconnect to continue.".into(),
            },
        );
        assert_eq!(
            m.conn,
            Conn::Closed {
                id: "conn-saved".into()
            }
        );
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("no problem dialog: {:?}", m.modal)
        };
        assert_eq!(p.code, "CONNECTION_CLOSED");
        let pending = p.reconnect.as_ref().expect("Reconnect offered");
        assert!(!pending.typed.is_empty(), "typed secrets kept");
    }

    #[test]
    fn a_connection_core_closed_shows_and_refuses_reloads() {
        let mut m = ready();
        let attempt = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        m.typed.set(SecretKind::Db, Secret::new("pw"));
        // Another connection's close is not ours.
        update_(
            &mut m,
            Msg::Closed {
                core_id: "core-9".into(),
                code: "WORKSPACE_EVICTED".into(),
                message: String::new(),
            },
        );
        assert!(m.conn.core_id().is_some());
        update_(
            &mut m,
            Msg::Closed {
                core_id: "core-1".into(),
                code: "WORKSPACE_EVICTED".into(),
                message: String::new(),
            },
        );
        assert_eq!(
            m.conn,
            Conn::Closed {
                id: "conn-saved".into()
            }
        );
        assert!(m.typed.is_empty());
        keys(&mut m, "2");
        let effects = keys(&mut m, "r");
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::LoadSchema { .. })));
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::Log(l) if l.tag == Some(Tag::Error))));
    }

    // ── Starting ──

    #[test]
    fn start_connects_or_opens_the_picker() {
        let mut m = ready();
        m.project = None;
        let effects = start(
            &mut m,
            Start {
                project: Some("project-a".into()),
                connect: Some("conn-saved".into()),
                picker: None,
            },
        );
        assert!(effects.contains(&Effect::LoadSaved {
            project_id: "project-a".into()
        }));
        assert_eq!(
            connect_effect(&effects).unwrap().connection_id,
            "conn-saved"
        );

        let mut m = ready();
        let picker = Picker {
            stage: Stage::Projects,
            selected: 1,
        };
        let effects = start(
            &mut m,
            Start {
                project: Some("project-b".into()),
                connect: None,
                picker: Some(picker.clone()),
            },
        );
        assert_eq!(
            effects,
            [Effect::LoadSaved {
                project_id: "project-b".into()
            }]
        );
        assert_eq!(m.modal, Some(Modal::Picker(picker)));
        assert_eq!(m.focus, Panel::Connection);
    }

    #[test]
    fn remembered_tabs_and_a_dirty_state_are_written_after_a_delay() {
        let mut m = ready();
        let t0 = Instant::now();
        update_(&mut m, Msg::Tick(t0));
        keys(&mut m, "3]");
        assert_eq!(m.saved_tab, SavedTab::History);
        assert_eq!(m.remembered.saved_tab.as_deref(), Some("history"));
        assert!(update_(&mut m, Msg::Tick(t0 + Duration::from_millis(100))).is_empty());
        let effects = update_(&mut m, Msg::Tick(t0 + Duration::from_millis(600)));
        assert_eq!(effects, [Effect::SaveState(m.remembered.clone())]);
        assert!(update_(&mut m, Msg::Tick(t0 + Duration::from_millis(700))).is_empty());
    }

    // ── The lists ──

    fn table(schema: &str, name: &str, kind: TableKind, rows: i64) -> TableItem {
        TableItem {
            schema: schema.into(),
            name: name.into(),
            kind,
            row_count: Some(rows),
            columns: vec![("id".into(), "int8".into())],
        }
    }

    fn connected_model() -> Model {
        let mut m = ready();
        let attempt = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        connected(&mut m, attempt, "core-1");
        keys(&mut m, "2");
        m
    }

    fn schema_loaded(m: &mut Model) {
        update_(
            m,
            Msg::Schema {
                core_id: "core-1".into(),
                result: Ok(vec![
                    table("public", "customers", TableKind::Table, 12_400),
                    table("public", "invoices", TableKind::Table, 48_100),
                    table("public", "active", TableKind::View, 0),
                    table("auth", "users", TableKind::Table, 3),
                ]),
                stamp: Stamp {
                    time: "12:00:00".into(),
                    elapsed_ms: 23,
                },
            },
        );
    }

    #[test]
    fn panel_two_lists_schemas_that_fold_and_reloads_with_r() {
        let mut m = connected_model();
        schema_loaded(&mut m);
        assert_eq!(m.schema_load, Load::Loaded);
        assert_eq!(m.tables.len, 5, "2 schemas + 3 tables");
        assert_eq!(m.views.len, 2);
        let last = m.log.last(1).next().unwrap();
        assert_eq!(last.text, "schema tables");
        assert_eq!(last.elapsed.as_deref(), Some("23 ms"));
        // Enter on a schema folds it, and again unfolds it.
        assert!(enter(&mut m).is_empty());
        assert!(m.folded_schemas.contains("public"));
        assert_eq!(m.tables.len, 3);
        assert_eq!(m.focus, Panel::Tables, "folding stays in the panel");
        enter(&mut m);
        assert_eq!(m.tables.len, 5);
        // Enter on a table opens it in the main view.
        keys(&mut m, "j");
        assert_eq!(m.selected_table_row(), Some(Row::Item(0)));
        enter(&mut m);
        assert_eq!(m.focus, Panel::Main);
        // Views.
        keys(&mut m, "2]");
        assert_eq!(m.tables_tab, TablesTab::Views);
        assert_eq!(m.list(Panel::Tables).unwrap().len, 2);
        // `r` reads the tables again.
        let effects = keys(&mut m, "r");
        assert!(effects.contains(&Effect::LoadSchema {
            core_id: "core-1".into()
        }));
        // A stale answer (another connection's) is dropped.
        let before = m.schema.clone();
        update_(
            &mut m,
            Msg::Schema {
                core_id: "core-old".into(),
                result: Ok(Vec::new()),
                stamp: Stamp::default(),
            },
        );
        assert_eq!(m.schema, before);
    }

    #[test]
    fn panel_three_lists_saved_queries_and_history() {
        let mut m = connected_model();
        let saved = |id: &str, folder: Option<&str>| SavedItem {
            id: id.into(),
            name: id.into(),
            folder: folder.map(Into::into),
            shared: id == "saved-b",
            sql: format!("SELECT '{id}'"),
        };
        update_(
            &mut m,
            Msg::Saved {
                project_id: "project-a".into(),
                result: Ok(vec![
                    saved("saved-a", None),
                    saved("saved-b", Some("reports")),
                ]),
            },
        );
        assert_eq!(m.saved.len, 3);
        // Another project's answer is dropped.
        update_(
            &mut m,
            Msg::Saved {
                project_id: "project-b".into(),
                result: Ok(Vec::new()),
            },
        );
        assert_eq!(m.saved_items.len(), 2);
        update_(
            &mut m,
            Msg::History {
                connection_id: "conn-saved".into(),
                result: Ok(vec![HistoryItem {
                    id: "h1".into(),
                    when: "12:03:51".into(),
                    sql: "SELECT 1".into(),
                    elapsed_ms: 4.0,
                    rows: 1.0,
                }]),
            },
        );
        assert_eq!(m.history.len, 1);
        keys(&mut m, "3");
        enter(&mut m);
        assert_eq!(m.focus, Panel::Main, "a saved query opens its SQL");
        keys(&mut m, "3j");
        enter(&mut m);
        assert!(m.folded_folders.contains("reports"));
        assert_eq!(m.saved.len, 2);
        let effects = keys(&mut m, "r");
        assert!(effects.contains(&Effect::LoadSaved {
            project_id: "project-a".into()
        }));
        assert!(effects.contains(&Effect::LoadHistory {
            connection_id: "conn-saved".into()
        }));
    }

    #[test]
    fn another_writer_s_change_reloads_what_it_touched() {
        let mut m = connected_model();
        assert_eq!(
            update_(&mut m, Msg::Changed(Changed::SavedQueries)),
            [Effect::LoadSaved {
                project_id: "project-a".into()
            }]
        );
        assert_eq!(
            update_(&mut m, Msg::Changed(Changed::History)),
            [Effect::LoadHistory {
                connection_id: "conn-saved".into()
            }]
        );
        assert_eq!(
            update_(&mut m, Msg::Changed(Changed::Library)),
            [Effect::LoadLibrary]
        );
        let effects = update_(&mut m, Msg::Changed(Changed::External));
        for e in [
            Effect::LoadLibrary,
            Effect::LoadSaved {
                project_id: "project-a".into(),
            },
            Effect::LoadHistory {
                connection_id: "conn-saved".into(),
            },
        ] {
            assert!(effects.contains(&e), "{e:?}");
        }
    }

    #[test]
    fn a_failed_load_says_so_in_the_log() {
        let mut m = connected_model();
        update_(
            &mut m,
            Msg::Schema {
                core_id: "core-1".into(),
                result: Err(CallError::new("QUERY_ERROR", "denied")),
                stamp: Stamp::default(),
            },
        );
        assert_eq!(m.schema_load, Load::Failed);
        assert_eq!(m.log.last(1).next().unwrap().tag, Some(Tag::Error));
    }

    #[test]
    fn an_empty_ticked_password_is_used_but_never_saved() {
        let mut m = ready();
        pick(&mut m, 0, 1);
        update_(&mut m, press(KeyCode::Tab));
        let attempt = connect_effect(&enter(&mut m)).unwrap().attempt;
        let effects = connected(&mut m, attempt, "core-1");
        assert!(!effects.iter().any(|e| matches!(e, Effect::SavePassword(_))));
        assert_eq!(m.saving, None);
    }

    #[test]
    fn a_connection_whose_row_vanished_meanwhile_is_disconnected() {
        let mut m = ready();
        let attempt = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        m.library.connections.retain(|c| c.id != "conn-saved");
        let effects = connected(&mut m, attempt, "core-9");
        assert!(effects.contains(&Effect::Disconnect {
            core_id: "core-9".into()
        }));
        assert_eq!(m.conn.core_id(), None);
    }

    #[test]
    fn a_keychain_wait_left_by_a_given_up_connect_doesnt_touch_the_next() {
        let mut m = ready();
        let first = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        let t0 = Instant::now();
        update_(
            &mut m,
            Msg::Keychain {
                pending: true,
                at: t0,
            },
        );
        update_(&mut m, Msg::Tick(t0 + Duration::from_millis(400)));
        assert!(esc(&mut m).contains(&Effect::CancelConnect { attempt: first }));
        esc(&mut m); // closes the problem
                     // The given-up read is still pending; the next connect starts.
        let second = connect_effect(&pick(&mut m, 0, 0)).unwrap().attempt;
        update_(&mut m, Msg::Tick(t0 + Duration::from_secs(1)));
        assert!(!m.keychain_box(), "the old wait isn't the new connect's");
        assert!(esc(&mut m)
            .iter()
            .all(|e| *e != Effect::CancelConnect { attempt: second }));
        let effects = update_(&mut m, Msg::Tick(t0 + Duration::from_secs(400)));
        assert!(!effects.contains(&Effect::CancelConnect { attempt: second }));
        assert_eq!(connecting_attempt(&m), second);
        // Once it ends, a new wait is the new connect's again.
        update_(
            &mut m,
            Msg::Keychain {
                pending: false,
                at: t0,
            },
        );
        let t1 = t0 + Duration::from_secs(401);
        update_(
            &mut m,
            Msg::Keychain {
                pending: true,
                at: t1,
            },
        );
        update_(&mut m, Msg::Tick(t1 + Duration::from_millis(400)));
        assert!(m.keychain_box());
    }

    #[test]
    fn altgr_characters_type_into_a_prompt() {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let mut m = ready();
        pick(&mut m, 0, 1);
        // AltGr arrives as Ctrl+Alt on Windows and some terminals.
        for c in ['@', '€', '{'] {
            update_(
                &mut m,
                Msg::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::CONTROL | KeyModifiers::ALT,
                )),
            );
        }
        let Some(Modal::Password(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.input.expose(), "@€{");
        // Ctrl alone (Ctrl+C) isn't text.
        update_(&mut m, ctrl('c'));
        let Some(Modal::Password(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.input.expose(), "@€{");
    }

    #[test]
    fn ctrl_c_still_asks_to_quit_with_a_dialog_closed() {
        let mut m = ready();
        update_(&mut m, ctrl('c'));
        assert_eq!(m.modal, Some(Modal::ConfirmQuit));
    }
    // ── The DuckDB helper (Task 7 of the DuckDB helper plan) ──

    /// The fixture library plus a DuckDB file connection in project-b.
    fn with_duckdb() -> Model {
        let mut m = ready();
        let file = m.library.connection("conn-file").unwrap().clone();
        m.library.connections.push(crate::state::panels::ConnItem {
            id: "conn-duck".into(),
            name: "warehouse".into(),
            engine: "duckdb".into(),
            ..file
        });
        m
    }

    fn duck_pending() -> crate::state::dialogs::Pending {
        let mut typed = Typed::default();
        typed.set(SecretKind::Db, Secret::new("typed-pw"));
        crate::state::dialogs::Pending {
            connection_id: "conn-duck".into(),
            typed,
            ..Default::default()
        }
    }

    /// Task 3's M5: a helper that didn't answer in time is there and
    /// checked, so a download wouldn't help. The problem offers the same
    /// connect again instead.
    #[test]
    fn a_helper_that_didnt_start_offers_reconnect_not_a_download() {
        let mut m = with_duckdb();
        m.conn = Conn::Connecting(Attempt {
            attempt: 3,
            pending: duck_pending(),
        });
        let effects = failed(
            &mut m,
            3,
            "ENGINE_UNAVAILABLE",
            "The DuckDB helper didn't start in time",
        );
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::CheckDuckdb { .. } | Effect::InstallDuckdb { .. })));
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.title, text::PROBLEM_TITLE_HELPER);
        assert!(p.message.contains("didn't start in time"));
        assert!(p.retry.is_none());
        assert_eq!(m.bar_context(), BarContext::ProblemReconnect);
        let effects = keys(&mut m, "r");
        let call = connect_effect(&effects).expect("connects again");
        assert_eq!(call.connection_id, "conn-duck");
        assert_eq!(
            call.secrets.db.as_ref().map(Secret::expose),
            Some("typed-pw")
        );
        assert_eq!(m.modal, None);
        assert!(matches!(m.conn, Conn::Connecting(_)));
    }

    /// A connected DuckDB whose helper stopped: the first call that says
    /// so (any panel's) closes the connection and offers to connect again,
    /// with the secrets typed for it.
    #[test]
    fn a_stopped_helper_closes_the_connection_and_offers_reconnect() {
        let mut m = with_duckdb();
        m.conn = Conn::Connected {
            id: "conn-duck".into(),
            core_id: "core-9".into(),
        };
        m.typed = duck_pending().typed;
        let closed = |core_id: &str| Msg::Schema {
            core_id: core_id.into(),
            result: Err(CallError::new(
                "CONNECTION_CLOSED",
                "The DuckDB helper stopped (signal 9). Reconnect to continue.",
            )),
            stamp: Stamp::default(),
        };
        // Another connection's answer changes nothing.
        update_(&mut m, closed("core-old"));
        assert!(matches!(m.conn, Conn::Connected { .. }));
        assert_eq!(m.modal, None);
        let effects = update_(&mut m, closed("core-9"));
        assert_eq!(
            m.conn,
            Conn::Closed {
                id: "conn-duck".into()
            }
        );
        assert!(effects.contains(&Effect::Disconnect {
            core_id: "core-9".into()
        }));
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.title, text::PROBLEM_TITLE_CLOSED);
        assert!(p.message.contains("signal 9"));
        assert_eq!(m.bar_context(), BarContext::ProblemReconnect);
        let effects = keys(&mut m, "r");
        let call = connect_effect(&effects).expect("connects again");
        assert_eq!(call.connection_id, "conn-duck");
        assert_eq!(
            call.secrets.db.as_ref().map(Secret::expose),
            Some("typed-pw")
        );
    }

    /// Review M1: `r` to reconnect a connection removed meanwhile opens
    /// the picker and says it was removed.
    #[test]
    fn reconnecting_a_removed_connection_opens_the_picker_and_says_so() {
        let mut m = with_duckdb();
        m.conn = Conn::Connecting(Attempt {
            attempt: 3,
            pending: duck_pending(),
        });
        failed(&mut m, 3, "ENGINE_UNAVAILABLE", "didn't start in time");
        m.library.connections.retain(|c| c.id != "conn-duck");
        let effects = keys(&mut m, "r");
        assert!(connect_effect(&effects).is_none());
        assert!(matches!(m.modal, Some(Modal::Picker(_))), "{:?}", m.modal);
        assert!(effects.contains(&Model::log_effect(
            Some(Tag::Error),
            text::CONNECTION_REMOVED.to_string()
        )));
    }

    /// The same from a table's columns read for completion, and from a
    /// query tab's run: the run's statement fails first, then the
    /// connection closes.
    #[test]
    fn a_stopped_helper_is_noticed_by_columns_and_runs() {
        use crate::state::query::RunMsg;
        let closed = || {
            CallError::new(
                "CONNECTION_CLOSED",
                "The DuckDB helper stopped (signal 9). Reconnect to continue.",
            )
        };
        let mut m = with_duckdb();
        m.conn = Conn::Connected {
            id: "conn-duck".into(),
            core_id: "core-9".into(),
        };
        update_(
            &mut m,
            Msg::Columns {
                core_id: "core-9".into(),
                target: seaquel_core::domain::edits::TableTarget {
                    schema: "main".into(),
                    table: "t".into(),
                },
                result: Err(closed()),
            },
        );
        assert!(matches!(m.conn, Conn::Closed { .. }), "{:?}", m.conn);
        assert!(matches!(m.modal, Some(Modal::Problem(_))));

        let mut m = with_duckdb();
        m.conn = Conn::Connected {
            id: "conn-duck".into(),
            core_id: "core-9".into(),
        };
        update_(&mut m, key('Q'));
        let tab = m.query.active().expect("a query tab").id;
        let t = m.query.active_mut().unwrap();
        t.op = Some(crate::state::query::Op {
            op: 5,
            stream_id: "tui-run-x-5".into(),
            kind: crate::state::query::OpKind::Run,
            page_size: 100,
            pending: None,
            connection_id: Some("conn-duck".into()),
            core_id: "core-old".into(),
        });
        let failed = |op| Msg::Run {
            tab,
            op,
            event: RunMsg::Failed {
                index: 0,
                error: closed(),
                elapsed_ms: 1.0,
                sql: None,
            },
        };
        // A run left over from an earlier connection: not this one's.
        update_(&mut m, failed(5));
        assert!(matches!(m.conn, Conn::Connected { .. }), "{:?}", m.conn);
        m.query.active_mut().unwrap().op.as_mut().unwrap().core_id = "core-9".into();
        update_(&mut m, failed(5));
        assert!(matches!(m.conn, Conn::Closed { .. }), "{:?}", m.conn);
        assert!(matches!(m.modal, Some(Modal::Problem(_))));
    }
}
