//! Pending Changes and commit (Decision 12, Task 5) in `update`: panel 4's
//! list, unstaging (`space`), re-editing a staged value (`e`), discarding
//! the queue (`D`, asks), the commit dialog (`c`) with its counts per kind
//! and table, the destructive list, the SQL preview (`p`) and the typed
//! `prod` confirm (Q11 A), the apply through Core's `apply_changes`, its
//! outcome, and the question a switch of connection asks while changes are
//! staged (keep them, discard them, or stay).
//!
//! **The outcome rules are the GUI's** (`PendingChangesManager.apply`): a
//! full success clears what was sent; an atomic failure keeps everything
//! and marks the failed change; an in-order failure removes the applied
//! prefix and marks the change it stopped at.
//!
//! **An apply can end without saying what ran** (the DuckDB helper plan's
//! probe F1): with DuckDB out of process, the helper can stop mid-commit,
//! and Core answers `CONNECTION_CLOSED` (as an error, or as the failed
//! change of its outcome) though the COMMIT may have landed. That is the
//! GUI's `interrupted`: the connection is lost (`connect::lost_by`), what
//! Core says ran leaves the queue, the rest stays, unmarked, with the queue
//! marked "may be partly applied" ([`Queue::interrupted`]), and committing
//! it again asks first ([`Modal::ConfirmRecommit`]).
//!
//! [`Queue::interrupted`]: super::pending::Queue::interrupted
//!
//! **The TUI decides no SQL.** Every change is the `Change::Edit` Core
//! planned; the dialog lists the statements `seaquel_core::sql`'s
//! destructive check finds in Core's plans (or Core's own list after a
//! `confirmRequired`), and `confirmed` is sent only when it listed some.
//! History comes only from Core's outcome.

use std::fmt;

use seaquel_core::domain::edits::{ApplyMode, Change};
use seaquel_core::sql::statements::destructive_reason;
use seaquel_core::sql::SqlEngine;
use seaquel_core::Value;

use super::app::{Effect, Load, Modal, Model, Stamp};
use super::browse;
use super::connect;
use super::dialogs::CallError;
use super::grid;
use super::log::{LogLine, Tag};
use super::panels::{ConnItem, HistoryItem};
use super::pending::{Entry, Plan, RowValues, Staging};
use super::text;

/// The predefined label every connection may carry that makes a commit ask
/// for `prod` to be typed (Q11 A).
pub const PROD_LABEL: &str = "prod";

/// The predefined labels as the GUI's history snapshot stores them
/// (`PREDEFINED_LABELS`, `src/lib/types/project.ts`): id, name, colour.
pub const PREDEFINED_LABELS: [(&str, &str, &str); 3] = [
    ("local", "Local", "#22c55e"),
    ("staging", "Staging", "#f59e0b"),
    ("prod", "Production", "#ef4444"),
];

/// A statement the destructive check flagged, and why, worded.
#[derive(Clone, PartialEq, Eq)]
pub struct Destructive {
    pub sql: String,
    pub reason: String,
}

impl fmt::Debug for Destructive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Destructive")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

/// The commit dialog.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct CommitDialog {
    /// What's typed into "type prod to confirm" (a `prod` connection only).
    pub typed: String,
    /// `p`: the SQL instead of the counts.
    pub preview: bool,
    /// Core's own list, after a `confirmRequired`: it wins over the TUI's.
    pub from_core: Option<(Vec<Destructive>, u32)>,
}

impl fmt::Debug for CommitDialog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitDialog")
            .field("typed_len", &self.typed.len())
            .field("preview", &self.preview)
            .field("from_core", &self.from_core.as_ref().map(|(_, n)| n))
            .finish()
    }
}

/// "N changes are staged on X": keep them, discard them, or stay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSwitch {
    /// The saved connection the queue belongs to.
    pub from: String,
    /// The saved connection being switched to, if this is a switch (else a
    /// staging on another connection asked).
    pub to: Option<String>,
}

/// `e` on a staged value: its text being edited.
#[derive(Clone, PartialEq, Eq)]
pub struct ValueEdit {
    pub id: String,
    pub text: String,
    pub start: String,
}

impl fmt::Debug for ValueEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValueEdit")
            .field("id", &self.id)
            .field("text_len", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// An apply in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Committing {
    pub op: u64,
    /// The entries sent, in order.
    pub ids: Vec<String>,
    pub core_id: String,
    pub connection_id: String,
}

/// A label in the history snapshot (`ConnectionLabel`). Its name is the
/// user's, so `Debug` shows the id only.
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryLabel {
    pub id: String,
    pub name: String,
    pub predefined: bool,
    pub color: String,
}

impl fmt::Debug for HistoryLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HistoryLabel({})", self.id)
    }
}

/// An apply for the runtime: `apply_changes` with a history context, so
/// the applied changes join History.
#[derive(Clone, PartialEq)]
pub struct ApplyCall {
    pub op: u64,
    pub core_id: String,
    pub connection_id: String,
    pub connection_name: String,
    pub labels: Vec<HistoryLabel>,
    pub changes: Vec<Change>,
    pub confirmed: bool,
}

impl fmt::Debug for ApplyCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyCall")
            .field("op", &self.op)
            .field("core_id", &self.core_id)
            .field("changes", &self.changes.len())
            .field("confirmed", &self.confirmed)
            .finish_non_exhaustive()
    }
}

/// The change an apply stopped at (`ApplyFailure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub id: Option<String>,
    pub error: CallError,
}

/// Core's answer to an apply, in the model's types (history rows as
/// panel 3 shows them).
#[derive(Clone, PartialEq)]
pub enum Applied {
    Applied {
        mode: ApplyMode,
        applied: u32,
        /// The ids of the changes applied one by one (`single`, `inOrder`;
        /// empty for `atomic`).
        results: Vec<String>,
        failed: Option<Failure>,
        ddl: bool,
        /// The rows Core recorded, in queue order.
        history: Vec<HistoryItem>,
    },
    ConfirmRequired {
        destructive: Vec<Destructive>,
        total: u32,
    },
}

impl fmt::Debug for Applied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Applied::Applied {
                mode,
                applied,
                failed,
                ..
            } => f
                .debug_struct("Applied")
                .field("mode", mode)
                .field("applied", applied)
                .field("failed", failed)
                .finish_non_exhaustive(),
            Applied::ConfirmRequired { total, .. } => f
                .debug_struct("ConfirmRequired")
                .field("total", total)
                .finish_non_exhaustive(),
        }
    }
}

/// One line of the commit dialog's counts: `~2  UPDATE  public.items,
/// public.invoices` (prototype `commitLines`). `Debug` leaves the table
/// names out.
#[derive(Clone, PartialEq, Eq)]
pub struct KindLine {
    pub sign: char,
    pub count: usize,
    pub verb: &'static str,
    pub tables: Vec<String>,
}

impl fmt::Debug for KindLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KindLine")
            .field("sign", &self.sign)
            .field("count", &self.count)
            .field("tables", &self.tables.len())
            .finish_non_exhaustive()
    }
}

/// Core's code for a connection lost mid-call: an apply that ends with it
/// may have committed (probe F1).
const CONNECTION_CLOSED: &str = "CONNECTION_CLOSED";

/// How many history rows panel 3 keeps (the GUI's 500, `HISTORY_KEEP`).
pub const HISTORY_KEEP: usize = 500;

/// The engine of the queue's connection (panel 1's when the queue holds
/// nothing): what its values, types and placeholders follow.
pub fn queue_engine(model: &Model) -> String {
    queue_conn(model)
        .map(|c| c.engine.clone())
        .unwrap_or_default()
}

/// Panel 4's selected entry (its list is in [`Queue::display_order`]).
///
/// [`Queue::display_order`]: super::pending::Queue::display_order
pub fn selected(model: &Model) -> Option<&Entry> {
    let order = model.queue.display_order();
    let i = *order.get(model.pending.selected)?;
    model.queue.entries().get(i)
}

/// Keeps panel 4's list and the staged counts in step with the queue
/// (`update` calls it after every message).
pub fn sync(model: &mut Model) {
    let len = model.queue.entries().len();
    model.pending.len = len;
    model.pending.selected = model.pending.selected.min(len.saturating_sub(1));
    model.staged = model.queue.counts();
}

/// The first two values of a row that aren't its key, shown.
fn row_values(entry: &Entry, key: &RowValues, n: usize) -> String {
    entry
        .row
        .iter()
        .filter(|(c, _)| !key.iter().any(|(k, _)| k == c))
        .take(n)
        .map(|(_, v)| grid::display(v))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// An entry as panel 4 lists it: `A`/`M`/`D`, its key (or an insert's
/// first value) and the column or the row's first values.
pub fn entry_line(entry: &Entry) -> (char, String, String) {
    match &entry.staging {
        Staging::Update { key, column, .. } | Staging::SetDefault { key, column } => {
            ('M', browse::key_text(key), column.clone())
        }
        Staging::Delete { key } => ('D', browse::key_text(key), row_values(entry, key, 2)),
        Staging::Insert { values } => {
            let first = values.first().map_or_else(
                || text::NEW_ROW.to_string(),
                |(c, v)| format!("{c} {}", grid::display(v)),
            );
            let second = values
                .get(1)
                .map(|(_, v)| grid::display(v))
                .unwrap_or_default();
            ('A', first, second)
        }
    }
}

/// An entry as the log and undo name it: `edit total · id 48109`.
pub fn describe(entry: &Entry) -> String {
    match &entry.staging {
        Staging::Update { key, column, .. } => text::undo_edit(column, &browse::key_text(key)),
        Staging::SetDefault { key, column } => text::undo_default(column, &browse::key_text(key)),
        Staging::Delete { key } => text::undo_delete(&browse::key_text(key)),
        Staging::Insert { .. } => text::undo_insert(&entry.target.table),
    }
}

/// `schema.table`.
fn table_name(entry: &Entry) -> String {
    format!("{}.{}", entry.target.schema, entry.target.table)
}

/// The queue's saved connection.
fn queue_conn(model: &Model) -> Option<&ConnItem> {
    model
        .queue
        .connection()
        .or(model.conn.id())
        .and_then(|id| model.library.connection(id))
}

/// Whether the queue's connection carries the predefined `prod` label.
pub fn prod(model: &Model) -> bool {
    queue_conn(model).is_some_and(|c| c.label_ids.iter().any(|l| l == PROD_LABEL))
}

/// The entries a commit sends.
fn sendable(model: &Model) -> Vec<&Entry> {
    model
        .queue
        .entries()
        .iter()
        .filter(|e| e.edit().is_some())
        .collect()
}

/// The dialog's counts per kind and table.
pub fn kind_lines(model: &Model) -> Vec<KindLine> {
    let mut lines = Vec::new();
    for (sign, verb) in [('+', "INSERT"), ('~', "UPDATE"), ('-', "DELETE")] {
        let of_kind: Vec<&Entry> = sendable(model)
            .into_iter()
            .filter(|e| entry_line(e).0 == sign_letter(sign))
            .collect();
        if of_kind.is_empty() {
            continue;
        }
        let mut tables: Vec<String> = Vec::new();
        for e in &of_kind {
            let name = table_name(e);
            if !tables.contains(&name) {
                tables.push(name);
            }
        }
        lines.push(KindLine {
            sign,
            count: of_kind.len(),
            verb,
            tables,
        });
    }
    lines
}

fn sign_letter(sign: char) -> char {
    match sign {
        '+' => 'A',
        '~' => 'M',
        _ => 'D',
    }
}

/// The statements the dialog lists as destructive: Core's own after a
/// `confirmRequired`, else what `seaquel_core::sql` finds in Core's plans.
pub fn destructive(model: &Model) -> Vec<Destructive> {
    if let Some(Modal::Commit(CommitDialog {
        from_core: Some((list, _)),
        ..
    })) = &model.modal
    {
        return list.clone();
    }
    let Some(engine) = queue_conn(model).and_then(|c| c.engine.parse::<SqlEngine>().ok()) else {
        return Vec::new();
    };
    sendable(model)
        .into_iter()
        .filter_map(|e| match &e.plan {
            Plan::Planned(p) => destructive_reason(&p.sql, engine).map(|r| Destructive {
                sql: p.sql.clone(),
                reason: text::destructive_reason(r).to_string(),
            }),
            _ => None,
        })
        .collect()
}

/// The history snapshot's labels for a connection: its predefined labels
/// and its project's own, as the GUI stores them.
pub fn history_labels(model: &Model, conn: &ConnItem) -> Vec<HistoryLabel> {
    conn.label_ids
        .iter()
        .filter_map(|id| {
            if let Some((id, name, color)) = PREDEFINED_LABELS.iter().find(|(p, ..)| p == id) {
                return Some(HistoryLabel {
                    id: id.to_string(),
                    name: name.to_string(),
                    predefined: true,
                    color: color.to_string(),
                });
            }
            model
                .library
                .labels
                .iter()
                .find(|l| l.project_id == conn.project_id && l.id == *id)
                .map(|l| HistoryLabel {
                    id: l.id.clone(),
                    name: l.name.clone(),
                    predefined: false,
                    color: l.color.clone(),
                })
        })
        .collect()
}

/// Whether a staging action may run: not while a commit is in flight.
pub fn idle(model: &Model) -> Result<(), Vec<Effect>> {
    if model.committing.is_some() {
        Err(vec![Model::log_effect(
            Some(Tag::Error),
            text::COMMIT_RUNNING,
        )])
    } else {
        Ok(())
    }
}

/// Panel 4's `space` (and `d`): unstages the selected entry.
pub fn unstage(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = idle(model) {
        return effects;
    }
    let Some(entry) = selected(model).cloned() else {
        return Vec::new();
    };
    let what = describe(&entry);
    model.queue.unstage(&entry.id, text::undo_unstage(&what));
    sync(model);
    vec![Model::log_effect(Some(Tag::Unstaged), what)]
}

/// Panel 4's `e`: edits the selected update's (or Set default's) value.
pub fn start_value_edit(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = idle(model) {
        return effects;
    }
    let Some(entry) = selected(model).cloned() else {
        return Vec::new();
    };
    let start = match &entry.staging {
        Staging::Update {
            value: Value::Null, ..
        } => "NULL".to_string(),
        Staging::Update { value, .. } => browse::escape_null(&grid::cell_text(value)),
        Staging::SetDefault { .. } => String::new(),
        Staging::Delete { .. } => {
            return vec![Model::log_effect(
                Some(Tag::ReadOnly),
                text::DELETE_HAS_NO_VALUE,
            )]
        }
        Staging::Insert { .. } => {
            return vec![Model::log_effect(
                Some(Tag::ReadOnly),
                text::INSERT_EDITS_IN_GRID,
            )]
        }
    };
    model.modal = Some(Modal::EditValue(ValueEdit {
        id: entry.id,
        text: start.clone(),
        start,
    }));
    Vec::new()
}

/// A column's type as panel 2 (or the opened table's metadata) knows it.
fn column_type(model: &Model, entry: &Entry, column: &str) -> String {
    // Panel 2 and the grid describe panel 1's connection (review M5).
    if !browse::here(model) {
        return String::new();
    }
    let opened = model
        .browse
        .opened
        .as_ref()
        .is_some_and(|o| o.target == entry.target);
    if opened {
        let ty = browse::column_type(model, column);
        if !ty.is_empty() {
            return ty;
        }
    }
    model
        .schema
        .iter()
        .find(|t| t.schema == entry.target.schema && t.name == entry.target.table)
        .and_then(|t| t.columns.iter().find(|(n, _)| n == column))
        .map(|(_, ty)| ty.clone())
        .unwrap_or_default()
}

/// Enter in the value edit: restages the cell, planned by Core.
pub fn apply_value(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::EditValue(edit)) = model.modal.take() else {
        return Vec::new();
    };
    if edit.text == edit.start {
        return Vec::new();
    }
    let Some(entry) = model.queue.entry(&edit.id).cloned() else {
        return Vec::new();
    };
    let (Staging::Update { key, column, .. } | Staging::SetDefault { key, column }) =
        &entry.staging
    else {
        return Vec::new();
    };
    let original = entry
        .row
        .iter()
        .find(|(c, _)| c == column)
        .map(|(_, v)| v.clone())
        .unwrap_or(Value::Null);
    let ty = column_type(model, &entry, column);
    let engine = queue_engine(model);
    let value = browse::edited_value(Some(&original), &ty, &engine, &edit.text);
    if let Err(effects) = idle(model) {
        return effects;
    }
    if let Some(Err(other)) = model
        .conn
        .id()
        .map(str::to_string)
        .map(|id| model.queue.bind(&id))
    {
        return browse::queue_elsewhere(model, &other);
    }
    let label = text::undo_edit(column, &browse::key_text(key));
    let outcome = model.queue.edit_cell(
        &entry.target,
        key,
        &entry.row,
        column,
        value,
        &original,
        label.clone(),
    );
    browse::staged(model, outcome, label)
}

/// Panel 4's `D`: asks before discarding everything.
pub fn ask_discard(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = idle(model) {
        return effects;
    }
    if !model.queue.is_empty() {
        model.modal = Some(Modal::ConfirmDiscard);
    }
    Vec::new()
}

/// Empties the queue and says so.
fn discard_all(model: &mut Model) -> Vec<Effect> {
    let count = model.queue.entries().len();
    model.queue.clear();
    sync(model);
    if count == 0 {
        return Vec::new();
    }
    vec![Model::log_effect(
        Some(Tag::Unstaged),
        text::discarded(count),
    )]
}

/// `y` in the discard question.
pub fn discard(model: &mut Model) -> Vec<Effect> {
    model.modal = None;
    discard_all(model)
}

/// `c`: the commit dialog (prototype `c`), after asking Core to plan what
/// isn't planned yet.
pub fn open(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = idle(model) {
        return effects;
    }
    if model.queue.is_empty() {
        return Vec::new();
    }
    if sendable(model).is_empty() {
        return vec![Model::log_effect(Some(Tag::Error), text::NOTHING_TO_COMMIT)];
    }
    if let Some(effects) = elsewhere(model) {
        return effects;
    }
    if model.conn.core_id().is_none() {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_LOG)];
    }
    if model.queue.interrupted() {
        model.modal = Some(Modal::ConfirmRecommit);
        return Vec::new();
    }
    open_dialog(model)
}

/// The commit dialog, after asking Core to plan what isn't planned yet.
fn open_dialog(model: &mut Model) -> Vec<Effect> {
    let requests = model.queue.replan();
    let effects = browse::plan(model, requests);
    model.modal = Some(Modal::Commit(CommitDialog::default()));
    effects
}

/// `y` in "Commit again?": the commit dialog, as `c` opens it. The mark
/// stays until an apply answers.
pub fn recommit(model: &mut Model) -> Vec<Effect> {
    if model.modal.take() != Some(Modal::ConfirmRecommit) {
        return Vec::new();
    }
    if let Err(effects) = idle(model) {
        return effects;
    }
    if let Some(effects) = elsewhere(model) {
        return effects;
    }
    if model.conn.core_id().is_none() || model.queue.is_empty() {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_LOG)];
    }
    open_dialog(model)
}

/// The queue belongs to a connection panel 1 doesn't have: says where.
fn elsewhere(model: &Model) -> Option<Vec<Effect>> {
    let id = model.queue.connection()?;
    if Some(id) == model.conn.id() {
        return None;
    }
    let name = model.library.connection(id).map_or(id, |c| c.name.as_str());
    Some(vec![Model::log_effect(
        Some(Tag::Error),
        text::commit_elsewhere(model.queue.entries().len(), name),
    )])
}

/// `p` (or Tab on a `prod` connection): the SQL, or the counts again.
pub fn toggle_preview(model: &mut Model) {
    if let Some(Modal::Commit(dialog)) = &mut model.modal {
        dialog.preview = !dialog.preview;
    }
}

/// Whether the dialog may send now: every change planned and, on a `prod`
/// connection, `prod` typed.
pub fn can_execute(model: &Model) -> bool {
    let Some(Modal::Commit(dialog)) = &model.modal else {
        return false;
    };
    (!prod(model) || dialog.typed == PROD_LABEL) && model.queue.all_planned()
}

/// Enter in the commit dialog: `apply_changes`, once every change is
/// planned and, on a `prod` connection, `prod` is typed.
pub fn execute(model: &mut Model) -> Vec<Effect> {
    if !can_execute(model) || model.committing.is_some() {
        return Vec::new();
    }
    // The connection may have changed under the dialog (review M4).
    if let Some(effects) = elsewhere(model) {
        return effects;
    }
    let (Some(core_id), Some(conn)) = (
        model.conn.core_id().map(str::to_string),
        model
            .conn
            .id()
            .and_then(|id| model.library.connection(id))
            .cloned(),
    ) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_LOG)];
    };
    // `confirmed` only when the dialog listed destructive statements.
    let confirmed = !destructive(model).is_empty();
    let changes = model.queue.changes();
    model.next_apply += 1;
    let op = model.next_apply;
    let call = ApplyCall {
        op,
        core_id: core_id.clone(),
        connection_id: conn.id.clone(),
        connection_name: conn.name.clone(),
        labels: history_labels(model, &conn),
        confirmed,
        changes,
    };
    model.committing = Some(Committing {
        op,
        ids: call.changes.iter().map(|c| c.id().to_string()).collect(),
        core_id,
        connection_id: conn.id,
    });
    model.modal = None;
    vec![Effect::Apply(call)]
}

/// Whether a text field of this module takes printable keys now: the
/// value edit, or the commit dialog's `prod` field.
pub fn typing(model: &Model) -> bool {
    match &model.modal {
        Some(Modal::EditValue(_)) => true,
        Some(Modal::Commit(_)) => prod(model),
        _ => false,
    }
}

/// How long the `prod` field may grow.
const TYPED_MAX: usize = 32;

pub fn type_char(model: &mut Model, c: char) {
    match &mut model.modal {
        Some(Modal::EditValue(edit)) => edit.text.push(c),
        Some(Modal::Commit(dialog)) if dialog.typed.chars().count() < TYPED_MAX => {
            dialog.typed.push(c)
        }
        _ => {}
    }
}

pub fn backspace(model: &mut Model) {
    match &mut model.modal {
        Some(Modal::EditValue(edit)) => {
            edit.text.pop();
        }
        Some(Modal::Commit(dialog)) => {
            dialog.typed.pop();
        }
        _ => {}
    }
}

/// A log line `update` writes itself, stamped with the answer's time.
fn line(stamp: &Stamp, tag: Option<Tag>, text: String, elapsed: Option<String>) -> LogLine {
    LogLine {
        time: stamp.time.clone(),
        tag,
        text: grid::clean(&one_line(&text)),
        elapsed,
    }
}

fn one_line(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A failure as the queue marks it: `NO_ROWS_AFFECTED` names the change's
/// table and key.
fn worded(model: &Model, failure: &Failure) -> CallError {
    let entry = failure.id.as_deref().and_then(|id| model.queue.entry(id));
    match (failure.error.code.as_str(), entry) {
        ("NO_ROWS_AFFECTED", Some(entry)) => {
            let key = entry
                .staging
                .key()
                .map(browse::key_text)
                .unwrap_or_default();
            CallError::new(
                failure.error.code.clone(),
                text::no_row(&table_name(entry), &key),
            )
        }
        _ => failure.error.clone(),
    }
}

/// Core answered an apply.
pub fn on_applied(
    model: &mut Model,
    op: u64,
    result: Result<Applied, CallError>,
    stamp: Stamp,
) -> Vec<Effect> {
    if model.committing.as_ref().map(|c| c.op) != Some(op) {
        return Vec::new();
    }
    let Some(committing) = model.committing.take() else {
        return Vec::new();
    };
    let elapsed = Some(grid::elapsed_text(stamp.elapsed_ms as f64));
    let (mode, applied, results, failed, ddl, history) = match result {
        Err(e) if e.code == CONNECTION_CLOSED => {
            // Nothing says what ran (probe F1): keep the queue, mark it.
            model.queue.mark_interrupted(true);
            model.log.push(line(
                &stamp,
                Some(Tag::Error),
                text::COMMIT_INTERRUPTED.to_string(),
                None,
            ));
            return Vec::new();
        }
        Err(e) => {
            model.log.push(line(
                &stamp,
                Some(Tag::Error),
                text::failed_line("commit", &text::apply_failed(&e.code, &e.message)),
                None,
            ));
            return Vec::new();
        }
        Ok(Applied::ConfirmRequired { destructive, total }) => {
            model.modal = Some(Modal::Commit(CommitDialog {
                from_core: Some((destructive, total)),
                ..CommitDialog::default()
            }));
            return Vec::new();
        }
        Ok(Applied::Applied {
            mode,
            applied,
            results,
            failed,
            ddl,
            history,
        }) => (mode, applied, results, failed, ddl, history),
    };
    let cut_off = failed
        .as_ref()
        .is_some_and(|f| f.error.code == CONNECTION_CLOSED);
    // The command log: what ran, as Core reported it.
    let sql_of = |model: &Model, id: &str| -> Option<String> {
        match &model.queue.entry(id)?.plan {
            Plan::Planned(p) => Some(p.sql.clone()),
            _ => None,
        }
    };
    let mut lines = Vec::new();
    if mode == ApplyMode::Atomic {
        lines.push(line(&stamp, None, text::BEGIN.to_string(), None));
        for id in &committing.ids {
            if let Some(sql) = sql_of(model, id) {
                lines.push(line(&stamp, None, sql, None));
            }
        }
        match &failed {
            None => lines.push(line(
                &stamp,
                Some(Tag::Committed),
                text::committed(committing.ids.len()),
                elapsed,
            )),
            // Whether it rolled back isn't known.
            Some(_) if cut_off => {}
            Some(f) => lines.push(line(
                &stamp,
                Some(Tag::Error),
                text::rolled_back(&f.error.code),
                None,
            )),
        }
    } else {
        for (i, id) in results.iter().enumerate() {
            if let Some(sql) = sql_of(model, id) {
                let ms = history.get(i).map(|h| grid::elapsed_text(h.elapsed_ms));
                lines.push(line(&stamp, Some(Tag::Committed), sql, ms));
            }
        }
    }
    if cut_off {
        lines.push(line(
            &stamp,
            Some(Tag::Error),
            text::COMMIT_INTERRUPTED.to_string(),
            None,
        ));
    } else if let Some(f) = &failed {
        let worded = worded(model, f);
        lines.push(line(
            &stamp,
            Some(Tag::Error),
            text::apply_failed(&worded.code, &worded.message),
            None,
        ));
        if let Some(id) = &f.id {
            if model.queue.entry(id).is_some() {
                model.queue.mark_failed(id, worded);
            }
        }
    }
    for l in lines {
        model.log.push(l);
    }
    // The outcome rules (GUI `PendingChangesManager.apply`).
    let removed: Vec<String> = match &failed {
        None => committing.ids.clone(),
        Some(_) => results,
    };
    if !removed.is_empty() {
        model.queue.remove_applied(&removed);
    }
    // An answer that says what ran settles the mark; one cut off sets it
    // (the change in flight may have landed).
    model.queue.mark_interrupted(cut_off);
    if model.queue.is_empty() {
        model.queue.clear();
    }
    sync(model);
    // History, from Core's answer only (newest first, as panel 3 lists it).
    // Rows already shown aren't added twice; the newest 500 are kept
    // (review M3).
    add_history(model, &committing.connection_id, history);
    if cut_off {
        // The connection is gone (`connect::lost` follows): nothing to read
        // again until it's back.
        return Vec::new();
    }
    let mut effects = Vec::new();
    let changed = applied > 0;
    if changed && ddl && model.conn.core_id() == Some(committing.core_id.as_str()) {
        model.schema_load = Load::Loading;
        effects.push(Effect::LoadSchema {
            core_id: committing.core_id.clone(),
        });
    }
    let shown_here = model
        .browse
        .opened
        .as_ref()
        .is_some_and(|o| o.core_id == committing.core_id);
    if changed && shown_here {
        effects.extend(browse::reload(model));
    }
    effects
}

/// History rows Core recorded for `connection_id`, in the order recorded,
/// added to panel 3 newest first when it shows that connection. Rows
/// already shown aren't added twice; the newest [`HISTORY_KEEP`] are kept
/// (review M3). Task 6's runs add theirs the same way.
pub fn add_history(model: &mut Model, connection_id: &str, history: Vec<HistoryItem>) {
    if model.conn.id() != Some(connection_id) || history.is_empty() {
        return;
    }
    let mut rows: Vec<HistoryItem> = history
        .into_iter()
        .rev()
        .filter(|h| !model.history_items.iter().any(|o| o.id == h.id))
        .collect();
    rows.append(&mut model.history_items);
    rows.truncate(HISTORY_KEEP);
    model.history_items = rows;
    model.refresh_lists();
}

/// The question a staging on another connection, or a switch, asks while
/// changes are staged on `from` (Task 4's `queue_elsewhere` hook).
pub fn ask_switch(model: &mut Model, from: &str, to: Option<String>) -> Vec<Effect> {
    model.modal = Some(Modal::QueueSwitch(QueueSwitch {
        from: from.to_string(),
        to,
    }));
    Vec::new()
}

/// `k`: keep the staged changes (and switch, if this was a switch).
pub fn keep_queue(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::QueueSwitch(question)) = model.modal.take() else {
        return Vec::new();
    };
    if let Err(effects) = idle(model) {
        return effects;
    }
    match question.to {
        Some(to) => connect::start_connect(model, &to),
        None => Vec::new(),
    }
}

/// `d`: discard them (and switch, if this was a switch).
pub fn discard_queue(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::QueueSwitch(question)) = model.modal.take() else {
        return Vec::new();
    };
    if let Err(effects) = idle(model) {
        return effects;
    }
    let mut effects = discard_all(model);
    if let Some(to) = question.to {
        effects.extend(connect::start_connect(model, &to));
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app::{update, Conn, Msg, Panel};
    use crate::state::keymap::BarContext;
    use crate::state::log::Tag;
    use crate::state::pending::{Plan, Staging};
    use crate::state::text;
    use crate::testing::fixtures::{browsing, invoices};
    use crate::testing::keys::{key, press};
    use crossterm::event::KeyCode;
    use seaquel_core::domain::edits::{Edit, PlannedChange};
    use seaquel_core::Value;

    fn keys(model: &mut Model, typed: &str) -> Vec<Effect> {
        typed.chars().flat_map(|c| update(model, key(c))).collect()
    }

    fn enter(model: &mut Model) -> Vec<Effect> {
        update(model, press(KeyCode::Enter))
    }

    fn esc(model: &mut Model) -> Vec<Effect> {
        update(model, press(KeyCode::Esc))
    }

    fn logs(effects: &[Effect]) -> Vec<(Option<Tag>, String)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Log(entry) => Some((entry.tag, entry.text.clone())),
                _ => None,
            })
            .collect()
    }

    fn applies(effects: &[Effect]) -> Vec<&ApplyCall> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Apply(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    fn plans(effects: &[Effect]) -> Vec<&crate::state::browse::PlanCall> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::PlanEdit(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    fn answer(model: &mut Model, effects: &[Effect]) {
        crate::testing::fixtures::answer_plans(model, effects);
    }

    fn plan_of(edit: &Edit) -> PlannedChange {
        crate::testing::fixtures::plan_of(edit)
    }

    fn staged(prod: bool) -> Model {
        crate::testing::fixtures::staged(148, 42, prod)
    }

    fn focus_pending(m: &mut Model) {
        keys(m, "4");
    }

    // Prototype `unstage` (`space` and `d` in panel 4) and `undo`.
    #[test]
    fn space_unstages_the_selected_change_and_u_brings_it_back() {
        let mut m = staged(false);
        focus_pending(&mut m);
        assert_eq!(m.pending.len, 4, "panel 4 lists the queue");
        assert_eq!(m.bar_context(), BarContext::Pending);
        keys(&mut m, "j");
        let second = selected(&m).unwrap().clone();
        let effects = keys(&mut m, " ");
        assert_eq!(m.queue.entries().len(), 3);
        assert!(m.queue.entry(&second.id).is_none());
        assert_eq!(logs(&effects), [(Some(Tag::Unstaged), describe(&second))]);
        assert_eq!(m.staged.total(), 3);
        assert_eq!(m.pending.len, 3);
        let effects = keys(&mut m, "u");
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Undo), text::undo_unstage(&describe(&second)))]
        );
        assert!(m.queue.entry(&second.id).is_some());
        // `d` unstages too; the selection stays in the list.
        keys(&mut m, "jjjjd");
        assert_eq!(m.queue.entries().len(), 3);
        assert_eq!(m.pending.selected, 2);
    }

    // Design 1c: grouped by table, A/M/D with key and column.
    #[test]
    fn entries_are_listed_with_their_kind_key_and_column() {
        let m = staged(false);
        let lines: Vec<(char, String, String)> = m
            .queue
            .display_order()
            .into_iter()
            .map(|i| entry_line(&m.queue.entries()[i]))
            .collect();
        assert_eq!(lines[0], ('M', "id 48109".into(), "total".into()));
        assert_eq!(lines[1].0, 'D');
        assert_eq!(lines[1].1, "id 48106");
        assert!(lines[1].2.contains("Hooli"), "{:?}", lines[1]);
        assert_eq!(lines[2], ('A', "customer New Co".into(), String::new()));
        assert_eq!(lines[3], ('M', "id 48108".into(), "customer".into()));
        assert_eq!(describe(&m.queue.entries()[0]), "edit total · id 48109");
        assert_eq!(describe(&m.queue.entries()[1]), "delete id 48106");
    }

    // Decision 12: `e` re-edits the value; Core plans it again.
    #[test]
    fn e_re_edits_a_staged_value_and_core_plans_it_again() {
        let mut m = staged(false);
        focus_pending(&mut m);
        assert!(keys(&mut m, "e").is_empty());
        assert_eq!(m.bar_context(), BarContext::EditValue);
        assert_eq!(m.mode().as_deref(), Some("EDIT"));
        let Some(Modal::EditValue(edit)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(edit.text, "3150.00");
        // Printable keys are its text: `q`, `1`, `D` don't act.
        for _ in 0..6 {
            update(&mut m, press(KeyCode::Backspace));
        }
        keys(&mut m, "q1D");
        for _ in 0..3 {
            update(&mut m, press(KeyCode::Backspace));
        }
        keys(&mut m, "200");
        let effects = enter(&mut m);
        assert_eq!(m.modal, None);
        let plan = plans(&effects);
        assert_eq!(plan.len(), 1);
        let Edit::UpdateCell { value, column, .. } = &plan[0].request.edit else {
            panic!()
        };
        assert_eq!(
            (column.as_str(), value),
            ("total", &Value::Text("3200".into()))
        );
        assert_eq!(m.queue.entries().len(), 4, "the same entry, restaged");
        // Esc cancels; a delete has no value; an insert edits in the grid.
        keys(&mut m, "e");
        esc(&mut m);
        assert_eq!(m.modal, None);
        keys(&mut m, "j");
        assert_eq!(
            logs(&keys(&mut m, "e")),
            [(Some(Tag::ReadOnly), text::DELETE_HAS_NO_VALUE.to_string())]
        );
        keys(&mut m, "j");
        assert_eq!(
            logs(&keys(&mut m, "e")),
            [(Some(Tag::ReadOnly), text::INSERT_EDITS_IN_GRID.to_string())]
        );
    }

    // `D` discards everything, after asking.
    #[test]
    fn capital_d_discards_all_after_asking() {
        let mut m = staged(false);
        focus_pending(&mut m);
        keys(&mut m, "D");
        assert_eq!(m.modal, Some(Modal::ConfirmDiscard));
        assert_eq!(m.bar_context(), BarContext::ConfirmDiscard);
        esc(&mut m);
        assert_eq!(m.queue.entries().len(), 4, "esc keeps them");
        keys(&mut m, "D");
        let effects = keys(&mut m, "y");
        assert!(m.queue.is_empty());
        assert_eq!(m.queue.connection(), None, "unbound");
        assert_eq!(m.staged.total(), 0);
        assert_eq!(logs(&effects), [(Some(Tag::Unstaged), text::discarded(4))]);
        assert!(keys(&mut m, "u").is_empty(), "nothing to undo");
    }

    // Prototype `commitLines`: counts per kind and table.
    #[test]
    fn c_opens_the_dialog_with_counts_per_kind_and_table() {
        let mut m = staged(false);
        assert!(plans(&keys(&mut m, "c")).is_empty(), "all planned");
        assert!(matches!(m.modal, Some(Modal::Commit(_))));
        assert_eq!(m.bar_context(), BarContext::Commit);
        assert_eq!(
            kind_lines(&m),
            [
                KindLine {
                    sign: '+',
                    count: 1,
                    verb: "INSERT",
                    tables: vec!["public.invoices".into()]
                },
                KindLine {
                    sign: '~',
                    count: 2,
                    verb: "UPDATE",
                    tables: vec!["public.invoices".into()]
                },
                KindLine {
                    sign: '-',
                    count: 1,
                    verb: "DELETE",
                    tables: vec!["public.invoices".into()]
                },
            ]
        );
        // Esc cancels (prototype), and nothing was sent.
        assert!(esc(&mut m).is_empty());
        assert_eq!(m.modal, None);
        // An empty queue: `c` does nothing.
        let mut m = browsing(148, 42);
        assert!(keys(&mut m, "c").is_empty());
        assert_eq!(m.modal, None);
    }

    // Decision 12 (Task 4's note): unplanned entries are planned again
    // before the dialog shows their SQL, and Enter waits for them.
    #[test]
    fn c_plans_again_what_is_unplanned_and_enter_waits_for_it() {
        let mut m = staged(false);
        let id = m.queue.entries()[0].id.clone();
        let seq = m.queue.entries()[0].seq;
        // Make the first entry unplanned, as a failed plan leaves it.
        let mut effects = keys(&mut m, "4e");
        for _ in 0..4 {
            update(&mut m, press(KeyCode::Backspace));
        }
        effects.extend(keys(&mut m, "1"));
        effects.extend(enter(&mut m));
        let request = plans(&effects)[0].request.clone();
        assert!(request.seq > seq);
        update(
            &mut m,
            Msg::Planned {
                id: id.clone(),
                seq: request.seq,
                result: Err(CallError::new("QUERY_ERROR", "the connection hiccuped")),
            },
        );
        assert_eq!(m.queue.entry(&id).unwrap().plan, Plan::Unplanned);
        let effects = keys(&mut m, "c");
        let again = plans(&effects);
        assert_eq!(again.len(), 1, "planned again");
        assert_eq!(again[0].core_id, "core-1");
        assert_eq!(m.queue.entry(&id).unwrap().plan, Plan::Planning);
        assert!(applies(&enter(&mut m)).is_empty(), "waits for the plan");
        answer(&mut m, &effects);
        let effects = enter(&mut m);
        assert_eq!(applies(&effects).len(), 1);
    }

    // `p` shows the SQL; Esc cancels.
    #[test]
    fn p_previews_the_sql_and_esc_cancels() {
        let mut m = staged(false);
        keys(&mut m, "c");
        keys(&mut m, "p");
        assert!(matches!(&m.modal, Some(Modal::Commit(d)) if d.preview));
        keys(&mut m, "p");
        assert!(matches!(&m.modal, Some(Modal::Commit(d)) if !d.preview));
        esc(&mut m);
        assert_eq!(m.modal, None);
        assert!(m.committing.is_none());
    }

    // Q11 A: on a `prod` connection Enter does nothing until `prod` is
    // typed; elsewhere Enter commits.
    #[test]
    fn on_a_prod_connection_enter_waits_for_prod_to_be_typed() {
        let mut m = staged(true);
        keys(&mut m, "c");
        assert!(prod(&m));
        assert_eq!(m.bar_context(), BarContext::CommitProd);
        assert!(applies(&enter(&mut m)).is_empty());
        // `p` and `q` are text here; Tab previews.
        keys(&mut m, "pro");
        assert!(applies(&enter(&mut m)).is_empty());
        assert!(matches!(&m.modal, Some(Modal::Commit(d)) if d.typed == "pro" && !d.preview));
        update(&mut m, press(KeyCode::Tab));
        assert!(matches!(&m.modal, Some(Modal::Commit(d)) if d.preview));
        keys(&mut m, "x");
        update(&mut m, press(KeyCode::Backspace));
        keys(&mut m, "d");
        let effects = enter(&mut m);
        assert_eq!(applies(&effects).len(), 1);
        assert_eq!(m.modal, None);

        let mut m = staged(false);
        keys(&mut m, "c");
        assert!(!prod(&m));
        let effects = enter(&mut m);
        let call = applies(&effects)[0];
        assert_eq!(call.core_id, "core-1");
        assert_eq!(call.connection_id, "conn-saved");
        assert_eq!(call.connection_name, "prod-analytics");
        assert_eq!(call.changes.len(), 4);
        assert!(!call.confirmed, "nothing destructive was listed");
        assert!(m.committing.is_some());
    }

    // GUI: the sheet's confirm dialog lists the destructive statements and
    // sends `confirmed` only when it listed some.
    #[test]
    fn confirmed_only_when_the_dialog_listed_destructive_statements() {
        let mut m = staged(false);
        let entry = m.queue.entries()[1].clone();
        let mut plan = plan_of(&entry.edit().unwrap());
        plan.sql = "DELETE FROM \"public\".\"invoices\"".into();
        // A plan Core answered for this entry's current seq.
        assert!(matches!(
            m.queue.planned(&entry.id, entry.seq, Ok(plan)),
            crate::state::pending::Planned::Kept(_)
        ));
        keys(&mut m, "c");
        let listed = destructive(&m);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reason, "DELETE without WHERE");
        let effects = enter(&mut m);
        assert!(applies(&effects)[0].confirmed);
    }

    fn applied(
        applied: u32,
        failed: Option<Failure>,
        results: Vec<String>,
        mode: ApplyMode,
    ) -> Applied {
        Applied::Applied {
            mode,
            applied,
            results,
            failed,
            ddl: false,
            history: Vec::new(),
        }
    }

    /// Commits `m`'s queue and answers it with `answer`.
    fn commit_with(m: &mut Model, answer: Result<Applied, CallError>) -> Vec<Effect> {
        keys(m, "c");
        let effects = enter(m);
        let op = applies(&effects)[0].op;
        update(
            m,
            Msg::Applied {
                op,
                result: answer,
                stamp: Stamp {
                    time: "12:05:00".into(),
                    elapsed_ms: 6,
                },
            },
        )
    }

    // GUI `PendingChangesManager.apply`: a full success clears the queue;
    // the page reads again; history comes from Core's outcome.
    #[test]
    fn a_full_success_clears_the_queue_reloads_the_page_and_adds_history() {
        let mut m = staged(false);
        let sent: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        // An insert with no value stays out of the commit and in the queue.
        m.browse.row = 0;
        keys(&mut m, "0a");
        let rows = vec![
            HistoryItem {
                id: "hist-1".into(),
                when: "12:05:00".into(),
                sql: "UPDATE".into(),
                elapsed_ms: 3.0,
                rows: 1.0,
            },
            HistoryItem {
                id: "hist-2".into(),
                when: "12:05:00".into(),
                sql: "DELETE".into(),
                elapsed_ms: 3.0,
                rows: 1.0,
            },
        ];
        let effects = commit_with(
            &mut m,
            Ok(Applied::Applied {
                mode: ApplyMode::Atomic,
                applied: 4,
                results: Vec::new(),
                failed: None,
                ddl: false,
                history: rows,
            }),
        );
        for id in &sent {
            assert!(m.queue.entry(id).is_none());
        }
        assert_eq!(m.queue.entries().len(), 1, "the empty insert stays");
        assert!(m.committing.is_none());
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::LoadPage(p) if p.page == 1)),
            "the page reads again"
        );
        assert_eq!(m.history_items[0].id, "hist-2", "newest first");
        assert_eq!(m.history_items[1].id, "hist-1");
        let texts: Vec<String> = m.log.last(6).map(|l| l.text.clone()).collect();
        assert!(texts.contains(&text::BEGIN.to_string()), "{texts:?}");
        assert!(texts.contains(&text::committed(4)), "{texts:?}");
        assert!(
            m.log.last(1).next().unwrap().tag == Some(Tag::Committed),
            "{texts:?}"
        );
    }

    // GUI: an atomic failure keeps everything and marks the failed change;
    // `NO_ROWS_AFFECTED` names its table and key.
    #[test]
    fn an_atomic_failure_keeps_everything_and_marks_the_change() {
        let mut m = staged(false);
        let failed = m.queue.entries()[1].id.clone();
        let effects = commit_with(
            &mut m,
            Ok(applied(
                0,
                Some(Failure {
                    id: Some(failed.clone()),
                    error: CallError::new("NO_ROWS_AFFECTED", "Change 2 matched no row."),
                }),
                Vec::new(),
                ApplyMode::Atomic,
            )),
        );
        assert_eq!(m.queue.entries().len(), 4);
        let mark = m.queue.failure(&failed).unwrap();
        assert_eq!(mark.code, "NO_ROWS_AFFECTED");
        assert_eq!(mark.message, text::no_row("public.invoices", "id 48106"));
        let texts: Vec<String> = m.log.last(8).map(|l| l.text.clone()).collect();
        assert!(
            texts.contains(&text::rolled_back("NO_ROWS_AFFECTED")),
            "{texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|t| t.contains("public.invoices · id 48106")),
            "{texts:?}"
        );
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::LoadPage(_))),
            "nothing changed: no reload"
        );
        assert!(m.committing.is_none());
        // Selecting it in panel 4 shows the mark's message in the diff.
        assert!(m.history_items.len() == 1, "no history row");
    }

    // GUI: an in-order failure removes the applied prefix and marks the
    // change it stopped at; a DDL change reads the tables again.
    #[test]
    fn an_in_order_failure_removes_the_applied_prefix() {
        let mut m = staged(false);
        let ids: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        let effects = commit_with(
            &mut m,
            Ok(Applied::Applied {
                mode: ApplyMode::InOrder,
                applied: 2,
                results: vec![ids[0].clone(), ids[1].clone()],
                failed: Some(Failure {
                    id: Some(ids[2].clone()),
                    error: CallError::new("EXECUTE_ERROR", "value too long"),
                }),
                ddl: true,
                history: Vec::new(),
            }),
        );
        let left: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        assert_eq!(left, ids[2..]);
        assert_eq!(m.queue.failure(&ids[2]).unwrap().message, "value too long");
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::LoadSchema { .. })));
        assert!(effects.iter().any(|e| matches!(e, Effect::LoadPage(_))));
        assert_eq!(m.staged.total(), 2);
    }

    // `confirmRequired` reopens the dialog with Core's list; Enter then
    // sends `confirmed`.
    #[test]
    fn confirm_required_reopens_the_dialog_with_core_s_list() {
        let mut m = staged(false);
        commit_with(
            &mut m,
            Ok(Applied::ConfirmRequired {
                destructive: vec![Destructive {
                    sql: "DELETE FROM t".into(),
                    reason: "DELETE without WHERE".into(),
                }],
                total: 1,
            }),
        );
        let Some(Modal::Commit(dialog)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(dialog.from_core.as_ref().unwrap().1, 1);
        assert_eq!(destructive(&m).len(), 1);
        assert_eq!(m.queue.entries().len(), 4, "nothing ran");
        let effects = enter(&mut m);
        assert!(applies(&effects)[0].confirmed);
    }

    // A Core error (the connection went) keeps the queue as it was.
    #[test]
    fn a_core_error_keeps_the_queue() {
        let mut m = staged(false);
        commit_with(&mut m, Err(CallError::new("CONNECTION_NOT_FOUND", "gone")));
        assert_eq!(m.queue.entries().len(), 4);
        assert!(m.committing.is_none());
        let last = m.log.last(1).next().unwrap();
        assert_eq!(last.tag, Some(Tag::Error));
        assert!(last.text.contains("CONNECTION_NOT_FOUND"), "{}", last.text);
    }

    fn closed(id: Option<String>) -> Failure {
        Failure {
            id,
            error: CallError::new(
                "CONNECTION_CLOSED",
                "The DuckDB helper stopped (signal 9). Reconnect to continue.",
            ),
        }
    }

    /// The connection is lost: panel 1 says closed and `r` reconnects.
    fn assert_lost(m: &Model) {
        assert!(matches!(m.conn, Conn::Closed { .. }), "{:?}", m.conn);
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.code, "CONNECTION_CLOSED");
        assert!(p.reconnect.is_some());
    }

    // DuckDB helper probe F1: the helper died mid-commit, so Core's outcome
    // fails with CONNECTION_CLOSED, but the COMMIT may have landed. The
    // connection is lost and the queue is kept, marked, not failed.
    #[test]
    fn a_connection_closed_mid_commit_is_lost_and_the_queue_may_be_applied() {
        let mut m = staged(false);
        let ids: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        commit_with(
            &mut m,
            Ok(applied(
                0,
                Some(closed(Some(ids[3].clone()))),
                Vec::new(),
                ApplyMode::Atomic,
            )),
        );
        assert_lost(&m);
        assert!(m.committing.is_none());
        assert_eq!(m.queue.entries().len(), 4, "the queue is kept");
        assert!(m.queue.interrupted());
        assert!(m.queue.failure(&ids[3]).is_none(), "not marked as failed");
        let texts: Vec<String> = m.log.last(6).map(|l| l.text.clone()).collect();
        assert!(
            texts.contains(&text::COMMIT_INTERRUPTED.to_string()),
            "{texts:?}"
        );
        assert!(
            !texts.contains(&text::rolled_back("CONNECTION_CLOSED")),
            "a rollback isn't known: {texts:?}"
        );
    }

    #[test]
    fn a_connection_closed_error_mid_commit_is_lost_too() {
        let mut m = staged(false);
        commit_with(&mut m, Err(closed(None).error));
        assert_lost(&m);
        assert_eq!(m.queue.entries().len(), 4);
        assert!(m.queue.interrupted());
    }

    // In order: what Core says ran leaves the queue; the change in flight
    // may have landed, so the rest is marked.
    #[test]
    fn an_in_order_commit_cut_off_drops_what_ran_and_marks_the_rest() {
        let mut m = staged(false);
        let ids: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        commit_with(
            &mut m,
            Ok(applied(
                2,
                Some(closed(Some(ids[2].clone()))),
                vec![ids[0].clone(), ids[1].clone()],
                ApplyMode::InOrder,
            )),
        );
        assert_lost(&m);
        let left: Vec<String> = m.queue.entries().iter().map(|e| e.id.clone()).collect();
        assert_eq!(left, ids[2..]);
        assert!(m.queue.interrupted());
        assert!(m.queue.failure(&ids[2]).is_none());
    }

    // Committing a marked queue again asks first; Esc keeps it; a
    // definite answer clears the mark.
    #[test]
    fn committing_a_queue_that_may_be_applied_asks_first() {
        let mut m = staged(false);
        let Conn::Connected { id, core_id } = m.conn.clone() else {
            panic!("{:?}", m.conn)
        };
        commit_with(&mut m, Err(closed(None).error));
        // Reconnected.
        m.modal = None;
        m.conn = Conn::Connected { id, core_id };
        focus_pending(&mut m);
        keys(&mut m, "c");
        assert_eq!(m.modal, Some(Modal::ConfirmRecommit));
        assert_eq!(m.bar_context(), BarContext::ConfirmRecommit);
        esc(&mut m);
        assert_eq!(m.modal, None);
        keys(&mut m, "c");
        assert!(applies(&keys(&mut m, "y")).is_empty(), "y opens the dialog");
        assert!(matches!(m.modal, Some(Modal::Commit(_))), "{:?}", m.modal);
        let op = applies(&enter(&mut m))[0].op;
        update(
            &mut m,
            Msg::Applied {
                op,
                result: Ok(applied(4, None, Vec::new(), ApplyMode::Atomic)),
                stamp: Stamp {
                    time: "12:06:00".into(),
                    elapsed_ms: 6,
                },
            },
        );
        assert!(!m.queue.interrupted());
    }

    // A late answer (another op) is dropped; staging waits for the apply.
    #[test]
    fn staging_waits_while_a_commit_runs() {
        let mut m = staged(false);
        keys(&mut m, "c");
        let op = applies(&enter(&mut m))[0].op;
        m.focus_panel(Panel::Main);
        m.browse.row = 9;
        let effects = keys(&mut m, "d");
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Error), text::COMMIT_RUNNING.to_string())]
        );
        assert_eq!(m.queue.entries().len(), 4);
        assert_eq!(logs(&keys(&mut m, "u"))[0].1, text::COMMIT_RUNNING);
        assert_eq!(logs(&keys(&mut m, "c"))[0].1, text::COMMIT_RUNNING);
        // Quitting asks while it runs.
        keys(&mut m, "q");
        assert_eq!(m.modal, Some(Modal::ConfirmQuit));
        esc(&mut m);
        update(
            &mut m,
            Msg::Applied {
                op: op + 7,
                result: Err(CallError::new("X", "late")),
                stamp: Stamp::default(),
            },
        );
        assert!(m.committing.is_some(), "not this commit's answer");
    }

    // `c` on another connection than the queue's: refused, naming it.
    #[test]
    fn c_on_another_connection_says_where_the_changes_are() {
        let mut m = staged(false);
        m.conn = Conn::Connected {
            id: "conn-ask".into(),
            core_id: "core-2".into(),
        };
        let effects = keys(&mut m, "c");
        assert_eq!(
            logs(&effects),
            [(
                Some(Tag::Error),
                text::commit_elsewhere(4, "prod-analytics")
            )]
        );
        assert_eq!(m.modal, None);
    }

    // Task 4's hook: a switch with staged changes asks to keep them,
    // discard them, or stay.
    #[test]
    fn a_switch_with_staged_changes_asks_keep_discard_or_cancel() {
        let pick_staging = |m: &mut Model| -> Vec<Effect> {
            keys(m, "1");
            enter(m);
            enter(m); // project-a
            keys(m, "j"); // staging (conn-ask)
            enter(m)
        };
        // Cancel: nothing changes.
        let mut m = staged(false);
        assert!(pick_staging(&mut m).is_empty());
        let Some(Modal::QueueSwitch(q)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(
            (q.from.as_str(), q.to.as_deref()),
            ("conn-saved", Some("conn-ask"))
        );
        assert_eq!(m.bar_context(), BarContext::QueueSwitch);
        esc(&mut m);
        assert_eq!(m.conn.id(), Some("conn-saved"));
        assert_eq!(m.queue.entries().len(), 4);
        // Keep: switches; the changes stay staged on prod-analytics.
        let effects = {
            pick_staging(&mut m);
            keys(&mut m, "k")
        };
        assert!(effects
            .iter()
            .any(|e| matches!(e, Effect::Disconnect { .. })));
        let asks_for = |m: &Model| {
            matches!(&m.modal,
            Some(Modal::Password(p)) if p.pending.connection_id == "conn-ask")
        };
        assert!(asks_for(&m), "connecting staging: {:?}", m.modal);
        assert_eq!(m.queue.connection(), Some("conn-saved"));
        assert_eq!(m.queue.entries().len(), 4);
        // Discard: switches with an empty queue.
        let mut m = staged(false);
        pick_staging(&mut m);
        keys(&mut m, "d");
        assert!(m.queue.is_empty());
        assert_eq!(m.queue.connection(), None);
        assert!(asks_for(&m), "connecting staging: {:?}", m.modal);
    }

    // The same question when a staging lands on another connection; keep
    // closes it, discard frees the queue for this connection.
    #[test]
    fn staging_on_another_connection_asks_too() {
        let mut m = staged(false);
        m.conn = Conn::Connected {
            id: "conn-ask".into(),
            core_id: "core-2".into(),
        };
        m.browse.opened.as_mut().unwrap().core_id = "core-2".into();
        m.focus_panel(Panel::Main);
        m.browse.row = 9;
        keys(&mut m, "d");
        assert!(matches!(&m.modal, Some(Modal::QueueSwitch(q)) if q.to.is_none()));
        keys(&mut m, "k");
        assert_eq!(m.modal, None);
        assert_eq!(m.queue.entries().len(), 4);
        keys(&mut m, "d");
        let effects = keys(&mut m, "d");
        assert!(m.queue.is_empty());
        assert_eq!(m.conn.id(), Some("conn-ask"), "no switch");
        assert_eq!(logs(&effects), [(Some(Tag::Unstaged), text::discarded(4))]);
    }

    // The history snapshot carries the connection's labels as the GUI
    // stores them: predefined and the project's own.
    #[test]
    fn the_history_context_names_the_connection_and_its_labels() {
        let mut m = staged(false);
        m.library.connections[0].label_ids = vec!["prod".into(), "label-x".into(), "gone".into()];
        m.library.labels.push(crate::state::panels::LabelItem {
            project_id: "project-a".into(),
            id: "label-x".into(),
            name: "Billing".into(),
            color: "#123456".into(),
        });
        let conn = m.library.connections[0].clone();
        let labels = history_labels(&m, &conn);
        assert_eq!(
            labels,
            [
                HistoryLabel {
                    id: "prod".into(),
                    name: "Production".into(),
                    predefined: true,
                    color: "#ef4444".into()
                },
                HistoryLabel {
                    id: "label-x".into(),
                    name: "Billing".into(),
                    predefined: false,
                    color: "#123456".into()
                },
            ]
        );
    }

    #[test]
    fn debug_shows_no_sql_values_or_typed_text() {
        let mut m = staged(false);
        keys(&mut m, "c");
        let call = applies(&enter(&mut m))[0].clone();
        let dialog = CommitDialog {
            typed: "prod-marker".into(),
            preview: false,
            from_core: Some((
                vec![Destructive {
                    sql: "DELETE sql-marker".into(),
                    reason: "r".into(),
                }],
                1,
            )),
        };
        let edit = ValueEdit {
            id: "stage-1".into(),
            text: "value-marker".into(),
            start: "start-marker".into(),
        };
        let label = HistoryLabel {
            id: "label-1".into(),
            name: "label-name-marker".into(),
            predefined: false,
            color: "#000000".into(),
        };
        let text = format!(
            "{call:?} {dialog:?} {edit:?} {:?} {label:?} {:?} {:?}",
            m.queue,
            kind_lines(&m),
            m.modal
        );
        for marker in ["marker", "3150", "New Co", "UPDATE", "prod-analytics"] {
            assert!(!text.contains(marker), "{marker}: {text}");
        }
        let _ = (Staging::Delete { key: vec![] }, invoices());
    }

    fn has_disconnect(effects: &[Effect]) -> bool {
        effects
            .iter()
            .any(|e| matches!(e, Effect::Disconnect { .. }))
    }

    // Review I1: no switch or reconnect while an apply runs.
    #[test]
    fn no_switch_or_reconnect_while_a_commit_runs() {
        let mut m = staged(false);
        keys(&mut m, "c");
        assert_eq!(applies(&enter(&mut m)).len(), 1);
        let mut effects = keys(&mut m, "1");
        effects.extend(enter(&mut m));
        effects.extend(enter(&mut m));
        assert!(
            logs(&effects)
                .iter()
                .any(|(_, t)| t == text::COMMIT_RUNNING),
            "{:?}",
            logs(&effects)
        );
        assert!(!has_disconnect(&effects));
        assert!(!matches!(m.modal, Some(Modal::Picker(_))));
        assert_eq!(m.conn.core_id(), Some("core-1"));
        // `k` on the switch question.
        m.modal = Some(Modal::QueueSwitch(QueueSwitch {
            from: "conn-saved".into(),
            to: Some("conn-ask".into()),
        }));
        let effects = keys(&mut m, "k");
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Error), text::COMMIT_RUNNING.to_string())]
        );
        assert!(!has_disconnect(&effects));
        assert_eq!(m.conn.core_id(), Some("core-1"));
        assert_eq!(m.queue.entries().len(), 4);
    }

    // Review M3: history rows already shown aren't added twice, and the
    // list keeps the newest 500 (the GUI's rule).
    #[test]
    fn history_rows_are_added_once_and_trimmed_to_500() {
        let row = |id: String| HistoryItem {
            id,
            when: "12:00:00".into(),
            sql: "UPDATE".into(),
            elapsed_ms: 1.0,
            rows: 1.0,
        };
        let mut m = staged(false);
        m.history_items = (0..499).map(|i| row(format!("old-{i}"))).collect();
        m.history_items.insert(0, row("hist-1".into()));
        commit_with(
            &mut m,
            Ok(Applied::Applied {
                mode: ApplyMode::Atomic,
                applied: 4,
                results: Vec::new(),
                failed: None,
                ddl: false,
                history: vec![
                    row("hist-1".into()),
                    row("hist-2".into()),
                    row("hist-3".into()),
                ],
            }),
        );
        let ids: Vec<&str> = m.history_items.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids.len(), HISTORY_KEEP);
        assert_eq!(&ids[..3], ["hist-3", "hist-2", "hist-1"]);
        assert_eq!(ids.iter().filter(|i| **i == "hist-1").count(), 1);
        assert_eq!(m.history.len, HISTORY_KEEP);
    }

    // Review M4: Enter checks the queue's connection again.
    #[test]
    fn enter_refuses_when_the_connection_changed_under_the_dialog() {
        let mut m = staged(false);
        keys(&mut m, "c");
        m.conn = Conn::Connected {
            id: "conn-ask".into(),
            core_id: "core-2".into(),
        };
        let effects = enter(&mut m);
        assert!(applies(&effects).is_empty());
        assert_eq!(
            logs(&effects),
            [(
                Some(Tag::Error),
                text::commit_elsewhere(4, "prod-analytics")
            )]
        );
        assert!(m.committing.is_none());
    }

    // Review M5: the queue's connection's engine, not panel 1's.
    #[test]
    fn the_queue_s_engine_names_the_values() {
        let mut m = staged(false);
        m.conn = Conn::Connected {
            id: "conn-file".into(),
            core_id: "core-9".into(),
        };
        assert_eq!(queue_engine(&m), "postgres");
        let text = crate::testing::snapshot::buffer_text(&crate::testing::snapshot::draw(&{
            let mut m = m.clone();
            m.focus_panel(Panel::Pending);
            m.pending_tab = 1;
            m
        }));
        assert!(text.contains("$1 = 3150.00"), "{text}");
        assert!(!text.contains("?1"), "{text}");
    }
}
