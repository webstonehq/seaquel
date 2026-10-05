//! The staged queue, the TUI's own: a port of the GUI's
//! documented rules (`PendingChangesManager`), not its code. Task 4 builds
//! the edit half: staging cell edits, Set default, row deletes and inserts,
//! the per-cell replacement rules, undo, and each entry's plan from Core.
//! Task 5 adds unstaging, panel 4's order, the replan before a commit,
//! the applied entries leaving and the failed one marked; the commit
//! itself is `commit.rs`.
//!
//! - **One connection.** While it holds anything (entries, or steps undo
//!   could bring back) the queue belongs to one saved connection
//!   ([`Queue::bind`]); another's stagings are refused until it's
//!   committed or cleared.
//! - **One entry per cell.** A repeated edit of a cell replaces its staged
//!   one (an update or a Set default); an edit back to the value as loaded
//!   unstages it.
//! - **Deletes toggle** per row; an insert is its own entry, its values
//!   edited in place (a column left out gets its default).
//! - **Each entry keeps its [`Change`]** (built by [`Entry::change`]), the
//!   row as loaded (shared, for Task 5's diff) and its plan. Every staging
//!   gives the entry a new `seq`, and a plan answering an older one is
//!   dropped: a plan that lands after a later edit of the same cell never
//!   overwrites it.
//! - **Refusals.** Core refusing the edit itself (`NOT_EDITABLE`,
//!   `INVALID_ARGUMENT`) takes its staging back; any other failure keeps
//!   the entry, unplanned, to be planned again at commit.
//! - **Undo** (`u`) takes back the last staging action. Each step keeps
//!   only the entries it changed, as they were (its inverse), so a refusal
//!   taking its own step out leaves every other step intact.
//!
//! Entries hold keys and values, so `Debug` shows ids, kinds and counts
//! only.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use seaquel_core::domain::edits::{Change, Edit, PlannedChange, TableTarget};
use seaquel_core::Value;

use super::app::Staged;
use super::dialogs::CallError;

/// `[column, value]` pairs, as Core's edits take keys and inserts.
pub type RowValues = Vec<(String, Value)>;

/// How many staging actions `u` can take back.
pub const UNDO_DEPTH: usize = 1_000;

/// The codes that refuse an edit itself: its staging is taken back.
pub const REFUSALS: [&str; 2] = ["NOT_EDITABLE", "INVALID_ARGUMENT"];

/// What an entry stages.
#[derive(Clone, PartialEq)]
pub enum Staging {
    Update {
        key: RowValues,
        column: String,
        value: Value,
    },
    SetDefault {
        key: RowValues,
        column: String,
    },
    Delete {
        key: RowValues,
    },
    /// The values typed so far, in the grid's column order.
    Insert {
        values: RowValues,
    },
}

impl Staging {
    fn kind(&self) -> &'static str {
        match self {
            Staging::Update { .. } => "update",
            Staging::SetDefault { .. } => "setDefault",
            Staging::Delete { .. } => "delete",
            Staging::Insert { .. } => "insert",
        }
    }

    /// The row a keyed entry is about.
    pub fn key(&self) -> Option<&RowValues> {
        match self {
            Staging::Update { key, .. }
            | Staging::SetDefault { key, .. }
            | Staging::Delete { key } => Some(key),
            Staging::Insert { .. } => None,
        }
    }
}

/// An entry's plan (`PlannedChange`'s `Debug` shows no SQL or values).
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// An insert with no value yet: nothing to plan.
    Empty,
    /// Not planned: its connection wasn't open, or Core couldn't plan it
    /// for a reason other than the edit. Planned again at commit.
    Unplanned,
    /// Asked; Core hasn't answered.
    Planning,
    Planned(PlannedChange),
}

/// One staged change.
#[derive(Clone, PartialEq)]
pub struct Entry {
    /// `stage-<n>`: the [`Change`]'s id.
    pub id: String,
    pub target: TableTarget,
    pub staging: Staging,
    /// The row as loaded (empty for an insert), for the diff.
    pub row: Arc<RowValues>,
    /// Moves on with every staging of this entry.
    pub seq: u64,
    pub plan: Plan,
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("kind", &self.staging.kind())
            .field("seq", &self.seq)
            .field(
                "plan",
                &match self.plan {
                    Plan::Empty => "empty",
                    Plan::Unplanned => "unplanned",
                    Plan::Planning => "planning",
                    Plan::Planned(_) => "planned",
                },
            )
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// The edit Core plans and applies; `None` for an insert with no value.
    pub fn edit(&self) -> Option<Edit> {
        let target = self.target.clone();
        Some(match &self.staging {
            Staging::Update { key, column, value } => Edit::UpdateCell {
                target,
                key: key.clone(),
                column: column.clone(),
                value: value.clone(),
            },
            Staging::SetDefault { key, column } => Edit::SetDefault {
                target,
                key: key.clone(),
                column: column.clone(),
            },
            Staging::Delete { key } => Edit::DeleteRow {
                target,
                key: key.clone(),
            },
            Staging::Insert { values } if values.is_empty() => return None,
            Staging::Insert { values } => Edit::InsertRow {
                target,
                values: values.clone(),
            },
        })
    }

    /// The queue entry as `apply_changes` takes it back.
    pub fn change(&self) -> Option<Change> {
        self.edit().map(|edit| Change::Edit {
            id: self.id.clone(),
            edit,
        })
    }
}

/// A plan to ask Core for: entry `id` as of `seq`.
#[derive(Clone, PartialEq)]
pub struct PlanRequest {
    pub id: String,
    pub seq: u64,
    pub edit: Edit,
}

impl fmt::Debug for PlanRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanRequest")
            .field("id", &self.id)
            .field("seq", &self.seq)
            .field("edit", &self.edit)
            .finish()
    }
}

/// What a staging action did.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Staged (or restaged): plan it.
    Staged(PlanRequest),
    /// An entry left the queue (or an insert went back to empty).
    Unstaged,
    /// Nothing changed.
    Nothing,
}

/// What a plan's answer did.
#[derive(Debug, Clone, PartialEq)]
pub enum Planned {
    /// Kept: the entry as planned.
    Kept(Entry),
    /// For an older staging, or an entry no longer there: dropped.
    Stale,
    /// Core refused the edit: the staging was taken back.
    Refused(Entry, CallError),
    /// Core couldn't plan it (not a refusal of the edit): kept, unplanned.
    Failed(Entry, CallError),
}

/// One undo step: what it was, and the entries it changed as they were
/// before it (`None`: it added that entry), with their positions.
#[derive(Clone)]
struct Step {
    label: String,
    before: Vec<(usize, String, Option<Entry>)>,
    /// The entry and seq it staged, so a refused plan can take its own
    /// step back.
    staged: Option<(String, u64)>,
}

/// The queue.
#[derive(Clone, Default)]
pub struct Queue {
    entries: Vec<Entry>,
    steps: Vec<Step>,
    next_id: u64,
    next_seq: u64,
    connection: Option<String>,
    generation: u64,
    /// Cell and row lookups into `entries` (by [`cell_index`] and
    /// [`row_index`]), rebuilt after every change.
    cells: HashMap<String, usize>,
    deletes: HashMap<String, usize>,
    /// The entry the last apply stopped at, and why: cleared by
    /// the next staging action.
    failure: Option<(String, CallError)>,
    /// An apply lost its connection before it answered:
    /// some of the queue may have been applied.
    /// Cleared by an apply that answers, or by emptying the queue.
    interrupted: bool,
}

impl fmt::Debug for Queue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Queue")
            .field("entries", &self.entries)
            .field("undo", &self.steps.len())
            .field("bound", &self.connection().is_some())
            .finish()
    }
}

/// A row's lookup key: the table and the key's values (`Debug` tells NULL
/// from the text "NULL" and a number from its text).
fn row_index(target: &TableTarget, key: &RowValues) -> String {
    let mut out = format!("{}\u{0}{}", target.schema, target.table);
    for (column, value) in key {
        out.push('\u{0}');
        out.push_str(column);
        out.push('=');
        out.push_str(&format!("{value:?}"));
    }
    out
}

fn cell_index(target: &TableTarget, key: &RowValues, column: &str) -> String {
    format!("{}\u{1}{column}", row_index(target, key))
}

impl Queue {
    /// The saved connection the queue belongs to, while it holds anything
    /// (an entry, or a step undo could bring one back with).
    pub fn connection(&self) -> Option<&str> {
        if self.entries.is_empty() && self.steps.is_empty() {
            None
        } else {
            self.connection.as_deref()
        }
    }

    /// Ties the queue to `connection` before a staging; refused (naming the
    /// queue's connection) while it holds another's.
    pub fn bind(&mut self, connection: &str) -> Result<(), String> {
        match self.connection() {
            Some(bound) if bound != connection => Err(bound.to_string()),
            _ => {
                self.connection = Some(connection.to_string());
                Ok(())
            }
        }
    }

    /// Moves on with every change, so caches know to refresh.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.id == id)
    }

    /// After every change: the lookups and the generation.
    fn changed(&mut self) {
        self.generation += 1;
        self.cells.clear();
        self.deletes.clear();
        for (i, e) in self.entries.iter().enumerate() {
            match &e.staging {
                Staging::Update { key, column, .. } | Staging::SetDefault { key, column } => {
                    self.cells.insert(cell_index(&e.target, key, column), i);
                }
                Staging::Delete { key } => {
                    self.deletes.insert(row_index(&e.target, key), i);
                }
                Staging::Insert { .. } => {}
            }
        }
    }

    /// Staged changes by kind: an update or Set default is an update.
    pub fn counts(&self) -> Staged {
        let mut staged = Staged::default();
        for entry in &self.entries {
            match entry.staging {
                Staging::Update { .. } | Staging::SetDefault { .. } => staged.updates += 1,
                Staging::Delete { .. } => staged.deletes += 1,
                Staging::Insert { .. } => staged.inserts += 1,
            }
        }
        staged
    }

    /// The staged update or Set default of a cell.
    pub fn cell(&self, target: &TableTarget, key: &RowValues, column: &str) -> Option<&Entry> {
        self.cells
            .get(&cell_index(target, key, column))
            .map(|&i| &self.entries[i])
    }

    /// Whether a row is staged for delete.
    pub fn deleted(&self, target: &TableTarget, key: &RowValues) -> bool {
        self.deletes.contains_key(&row_index(target, key))
    }

    /// A table's staged inserts, in the order they were added.
    pub fn inserts<'a>(&'a self, target: &'a TableTarget) -> impl Iterator<Item = &'a Entry> {
        self.entries
            .iter()
            .filter(move |e| e.target == *target && matches!(e.staging, Staging::Insert { .. }))
    }

    /// A table's staged changes, counted.
    pub fn table_counts(&self, target: &TableTarget) -> Staged {
        let mut staged = Staged::default();
        for entry in self.entries.iter().filter(|e| e.target == *target) {
            match entry.staging {
                Staging::Update { .. } | Staging::SetDefault { .. } => staged.updates += 1,
                Staging::Delete { .. } => staged.deletes += 1,
                Staging::Insert { .. } => staged.inserts += 1,
            }
        }
        staged
    }

    fn seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("stage-{}", self.next_id)
    }

    /// Starts a step: `id`'s entry as it is now (or its absence).
    fn step(&mut self, label: String, id: &str) {
        self.failure = None;
        if self.steps.len() == UNDO_DEPTH {
            self.steps.remove(0);
        }
        let at = self.position(id);
        let before = at.map(|i| self.entries[i].clone());
        self.steps.push(Step {
            label,
            before: vec![(at.unwrap_or(self.entries.len()), id.to_string(), before)],
            staged: None,
        });
    }

    /// The entry at `index` was (re)staged: a new seq, planning, and the
    /// last step remembers it.
    fn restage(&mut self, index: usize) -> Outcome {
        let seq = self.seq();
        let entry = &mut self.entries[index];
        entry.seq = seq;
        let Some(edit) = entry.edit() else {
            entry.plan = Plan::Empty;
            self.changed();
            return Outcome::Unstaged;
        };
        entry.plan = Plan::Planning;
        let request = PlanRequest {
            id: entry.id.clone(),
            seq,
            edit,
        };
        if let Some(step) = self.steps.last_mut() {
            step.staged = Some((request.id.clone(), seq));
        }
        self.changed();
        Outcome::Staged(request)
    }

    fn push(
        &mut self,
        target: &TableTarget,
        staging: Staging,
        row: &RowValues,
        label: String,
    ) -> Outcome {
        let id = self.new_id();
        self.step(label, &id);
        self.entries.push(Entry {
            id,
            target: target.clone(),
            staging,
            row: Arc::new(row.clone()),
            seq: 0,
            plan: Plan::Empty,
        });
        self.restage(self.entries.len() - 1)
    }

    fn remove(&mut self, index: usize, label: String) -> Outcome {
        let id = self.entries[index].id.clone();
        self.step(label, &id);
        self.entries.remove(index);
        self.changed();
        Outcome::Unstaged
    }

    fn cell_position(&self, target: &TableTarget, key: &RowValues, column: &str) -> Option<usize> {
        self.cells.get(&cell_index(target, key, column)).copied()
    }

    /// A cell edited to `value`. `original` is the cell as loaded: an edit
    /// back to it unstages the cell's update (or Set default).
    #[allow(clippy::too_many_arguments)]
    pub fn edit_cell(
        &mut self,
        target: &TableTarget,
        key: &RowValues,
        row: &RowValues,
        column: &str,
        value: Value,
        original: &Value,
        label: String,
    ) -> Outcome {
        let back = same_cell(&value, original);
        match self.cell_position(target, key, column) {
            None if back => Outcome::Nothing,
            None => self.push(
                target,
                Staging::Update {
                    key: key.clone(),
                    column: column.to_string(),
                    value,
                },
                row,
                label,
            ),
            Some(i) => {
                if let Staging::Update { value: staged, .. } = &self.entries[i].staging {
                    if same_value(staged, &value) {
                        return Outcome::Nothing;
                    }
                }
                if back {
                    return self.remove(i, label);
                }
                let id = self.entries[i].id.clone();
                self.step(label, &id);
                self.entries[i].staging = Staging::Update {
                    key: key.clone(),
                    column: column.to_string(),
                    value,
                };
                self.restage(i)
            }
        }
    }

    /// Set default on a cell: replaces the cell's staged update.
    pub fn set_default(
        &mut self,
        target: &TableTarget,
        key: &RowValues,
        row: &RowValues,
        column: &str,
        label: String,
    ) -> Outcome {
        let staging = Staging::SetDefault {
            key: key.clone(),
            column: column.to_string(),
        };
        match self.cell_position(target, key, column) {
            Some(i) if self.entries[i].staging == staging => Outcome::Nothing,
            Some(i) => {
                let id = self.entries[i].id.clone();
                self.step(label, &id);
                self.entries[i].staging = staging;
                self.restage(i)
            }
            None => self.push(target, staging, row, label),
        }
    }

    /// Stages a row's delete, or unstages it when it's staged.
    pub fn toggle_delete(
        &mut self,
        target: &TableTarget,
        key: &RowValues,
        row: &RowValues,
        label: String,
    ) -> Outcome {
        match self.deletes.get(&row_index(target, key)).copied() {
            Some(i) => self.remove(i, label),
            None => self.push(target, Staging::Delete { key: key.clone() }, row, label),
        }
    }

    /// A blank insert row; its id.
    pub fn add_insert(&mut self, target: &TableTarget, label: String) -> String {
        let id = self.new_id();
        self.step(label, &id);
        self.entries.push(Entry {
            id: id.clone(),
            target: target.clone(),
            staging: Staging::Insert { values: Vec::new() },
            row: Arc::new(Vec::new()),
            seq: 0,
            plan: Plan::Empty,
        });
        self.changed();
        id
    }

    /// Sets (or with `None` clears, back to its default) a column of insert
    /// `id`. `order` is the grid's column order, which the values keep.
    pub fn edit_insert(
        &mut self,
        id: &str,
        column: &str,
        value: Option<Value>,
        order: &[String],
        label: String,
    ) -> Outcome {
        let Some(i) = self.position(id) else {
            return Outcome::Nothing;
        };
        let Staging::Insert { values } = &self.entries[i].staging else {
            return Outcome::Nothing;
        };
        let current = values.iter().find(|(c, _)| c == column).map(|(_, v)| v);
        let unchanged = match (current, &value) {
            (None, None) => true,
            (Some(a), Some(b)) => same_value(a, b),
            _ => false,
        };
        if unchanged {
            return Outcome::Nothing;
        }
        let mut values = values.clone();
        values.retain(|(c, _)| c != column);
        if let Some(value) = value {
            values.push((column.to_string(), value));
        }
        let position = |c: &str| order.iter().position(|o| o == c).unwrap_or(usize::MAX);
        values.sort_by_key(|(c, _)| position(c));
        self.step(label, id);
        self.entries[i].staging = Staging::Insert { values };
        self.restage(i)
    }

    /// Drops insert `id`.
    pub fn drop_insert(&mut self, id: &str, label: String) -> Outcome {
        match self.position(id) {
            Some(i) => self.remove(i, label),
            None => Outcome::Nothing,
        }
    }

    /// Puts an entry back as `before` had it (or takes it out).
    fn restore(&mut self, at: usize, id: &str, before: Option<Entry>) {
        match (self.position(id), before) {
            (Some(i), Some(entry)) => self.entries[i] = entry,
            (Some(i), None) => {
                self.entries.remove(i);
            }
            (None, Some(entry)) => {
                let at = at.min(self.entries.len());
                self.entries.insert(at, entry);
            }
            (None, None) => {}
        }
    }

    /// Takes back the last staging action. Its label, and the restored
    /// entries whose plan was still out, asked again under a new seq (the
    /// caller plans them, or [`Queue::unplan`]s them).
    pub fn undo(&mut self) -> Option<(String, Vec<PlanRequest>)> {
        let step = self.steps.pop()?;
        self.failure = None;
        let mut requests = Vec::new();
        for (at, id, before) in step.before.into_iter().rev() {
            self.restore(at, &id, before);
            if let Some(i) = self.position(&id) {
                if matches!(self.entries[i].plan, Plan::Planning) {
                    let seq = self.seq();
                    let entry = &mut self.entries[i];
                    entry.seq = seq;
                    if let Some(edit) = entry.edit() {
                        requests.push(PlanRequest {
                            id: entry.id.clone(),
                            seq,
                            edit,
                        });
                    }
                }
            }
        }
        self.changed();
        Some((step.label, requests))
    }

    /// Entries asked to be planned with no connection to plan them on:
    /// unplanned (planned again at commit), and their seq moved on.
    pub fn unplan(&mut self, requests: &[PlanRequest]) {
        for request in requests {
            let seq = self.seq();
            if let Some(i) = self.position(&request.id) {
                if self.entries[i].seq == request.seq {
                    self.entries[i].plan = Plan::Unplanned;
                    self.entries[i].seq = seq;
                }
            }
        }
        self.changed();
    }

    /// Core's plan for entry `id` as of `seq`.
    pub fn planned(
        &mut self,
        id: &str,
        seq: u64,
        result: Result<PlannedChange, CallError>,
    ) -> Planned {
        let Some(i) = self.entries.iter().position(|e| e.id == id && e.seq == seq) else {
            return Planned::Stale;
        };
        let outcome = match result {
            Ok(plan) => {
                self.entries[i].plan = Plan::Planned(plan);
                Planned::Kept(self.entries[i].clone())
            }
            Err(e) if REFUSALS.contains(&e.code.as_str()) => {
                let refused = self.entries[i].clone();
                // Take back the step that staged it, and only that step:
                // the entry as it found it (or none).
                let at = self
                    .steps
                    .iter()
                    .rposition(|s| s.staged.as_ref() == Some(&(id.to_string(), seq)));
                match at {
                    Some(at) => {
                        let step = self.steps.remove(at);
                        for (pos, sid, before) in step.before {
                            self.restore(pos, &sid, before);
                        }
                    }
                    None => {
                        self.entries.remove(i);
                    }
                }
                Planned::Refused(refused, e)
            }
            Err(e) => {
                self.entries[i].plan = Plan::Unplanned;
                Planned::Failed(self.entries[i].clone(), e)
            }
        };
        self.changed();
        outcome
    }

    /// Empties the queue, its undo history and its connection (Task 5's
    /// commit and discard, and the hook for a switch of connection).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.steps.clear();
        self.connection = None;
        self.failure = None;
        self.interrupted = false;
        self.changed();
    }

    /// Panel 4's `space`: takes entry `id` out, as an undo step labelled
    /// `label` (prototype `unstage`).
    pub fn unstage(&mut self, id: &str, label: String) -> Outcome {
        match self.position(id) {
            Some(i) => self.remove(i, label),
            None => Outcome::Nothing,
        }
    }

    /// The entries in panel 4's order: grouped by table, the tables in the
    /// order their first entry was staged, each table's entries in staging
    /// order. Indices into [`Queue::entries`].
    pub fn display_order(&self) -> Vec<usize> {
        let mut tables: Vec<&TableTarget> = Vec::new();
        for e in &self.entries {
            if !tables.contains(&&e.target) {
                tables.push(&e.target);
            }
        }
        tables
            .into_iter()
            .flat_map(|t| {
                self.entries
                    .iter()
                    .enumerate()
                    .filter(move |(_, e)| e.target == *t)
                    .map(|(i, _)| i)
            })
            .collect()
    }

    /// Before the commit dialog: the entries left [`Plan::Unplanned`] are
    /// asked again under a new seq (now `Planning`).
    pub fn replan(&mut self) -> Vec<PlanRequest> {
        let mut requests = Vec::new();
        for i in 0..self.entries.len() {
            if self.entries[i].plan != Plan::Unplanned {
                continue;
            }
            let seq = self.seq();
            let entry = &mut self.entries[i];
            let Some(edit) = entry.edit() else {
                continue;
            };
            entry.seq = seq;
            entry.plan = Plan::Planning;
            requests.push(PlanRequest {
                id: entry.id.clone(),
                seq,
                edit,
            });
        }
        if !requests.is_empty() {
            self.changed();
        }
        requests
    }

    /// Whether every entry with a change to send is planned.
    pub fn all_planned(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e.plan, Plan::Planned(_) | Plan::Empty))
    }

    /// The changes a commit sends, in queue order (an insert with no value
    /// yet is left out, and stays).
    pub fn changes(&self) -> Vec<Change> {
        self.entries.iter().filter_map(Entry::change).collect()
    }

    /// After an apply: the applied entries leave, and with them the undo
    /// history (nothing staged before the apply can come back over what
    /// the database now holds).
    pub fn remove_applied(&mut self, ids: &[String]) {
        let before = self.entries.len();
        self.entries.retain(|e| !ids.contains(&e.id));
        if self.entries.len() != before {
            self.steps.clear();
        }
        self.changed();
    }

    /// Marks the entry an apply stopped at with its error.
    pub fn mark_failed(&mut self, id: &str, error: CallError) {
        self.failure = Some((id.to_string(), error));
        self.changed();
    }

    /// An apply lost its connection: the queue may be partly applied (the
    /// GUI's `pendingChangesInterrupted`).
    pub fn mark_interrupted(&mut self, interrupted: bool) {
        self.interrupted = interrupted;
        self.changed();
    }

    /// Whether the last apply ended without saying what ran.
    pub fn interrupted(&self) -> bool {
        self.interrupted
    }

    /// The marked entry's error, if `id` is it.
    pub fn failure(&self, id: &str) -> Option<&CallError> {
        self.failure
            .as_ref()
            .filter(|(failed, _)| failed == id)
            .map(|(_, e)| e)
    }
}

/// Two values the same for staging.
fn same_value(a: &Value, b: &Value) -> bool {
    a == b
}

/// A typed value equal to the cell as loaded: NULL for NULL, bytes for the
/// same bytes, else the same text as the cell shows (`cellText`).
pub fn same_cell(typed: &Value, loaded: &Value) -> bool {
    match (typed, loaded) {
        (Value::Null, Value::Null) => true,
        (Value::Null, _) | (_, Value::Null) => false,
        (Value::Text(t), loaded) => *t == super::grid::cell_text(loaded),
        (typed, loaded) => typed == loaded,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invoices() -> TableTarget {
        TableTarget {
            schema: "public".into(),
            table: "invoices".into(),
        }
    }

    fn key(id: i64) -> RowValues {
        vec![("id".into(), Value::Int(id))]
    }

    fn row(id: i64, total: &str) -> RowValues {
        vec![
            ("id".into(), Value::Int(id)),
            ("total".into(), Value::Decimal(total.into())),
        ]
    }

    fn text(s: &str) -> Value {
        Value::Text(s.into())
    }

    fn edit(q: &mut Queue, id: i64, value: Value, original: &str) -> Outcome {
        q.edit_cell(
            &invoices(),
            &key(id),
            &row(id, original),
            "total",
            value,
            &Value::Decimal(original.into()),
            format!("edit total · id {id}"),
        )
    }

    fn planned_change() -> PlannedChange {
        use seaquel_core::sql::statements::QueryType;
        PlannedChange {
            sql: "UPDATE".into(),
            params: Vec::new(),
            query_type: QueryType::Update,
            dml: true,
            summary: None,
        }
    }

    // Prototype `applyEdit`; GUI: a repeated edit of one cell replaces the
    // queued one (`PendingChangesManager`).
    #[test]
    fn a_repeated_edit_of_a_cell_replaces_it_and_one_back_to_the_original_unstages() {
        let mut q = Queue::default();
        let Outcome::Staged(first) = edit(&mut q, 48109, text("3150.00"), "2975.00") else {
            panic!("staged")
        };
        assert_eq!(q.counts().updates, 1);
        let Outcome::Staged(second) = edit(&mut q, 48109, text("3200.00"), "2975.00") else {
            panic!("restaged")
        };
        assert_eq!(q.entries().len(), 1, "one entry per cell");
        assert_eq!(first.id, second.id);
        assert!(second.seq > first.seq);
        assert_eq!(
            edit(&mut q, 48109, text("3200.00"), "2975.00"),
            Outcome::Nothing
        );
        assert_eq!(
            edit(&mut q, 48109, text("2975.00"), "2975.00"),
            Outcome::Unstaged
        );
        assert!(q.is_empty());
        assert_eq!(
            edit(&mut q, 48109, text("2975.00"), "2975.00"),
            Outcome::Nothing,
            "no edit, nothing staged"
        );
    }

    #[test]
    fn null_is_its_own_value() {
        assert!(same_cell(&Value::Null, &Value::Null));
        assert!(!same_cell(&text("NULL"), &Value::Null));
        assert!(!same_cell(&Value::Null, &text("")));
        assert!(same_cell(&text("12"), &Value::Int(12)));
        assert!(same_cell(
            &text("4210.00"),
            &Value::Decimal("4210.00".into())
        ));
        assert!(!same_cell(&text("4210"), &Value::Decimal("4210.00".into())));
    }

    // A plan that lands after a later edit of the same cell is dropped
    // (GUI's per-cell sequence).
    #[test]
    fn a_late_plan_is_dropped() {
        let mut q = Queue::default();
        let Outcome::Staged(first) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        let Outcome::Staged(second) = edit(&mut q, 1, text("6"), "4") else {
            panic!()
        };
        assert_eq!(
            q.planned(&first.id, first.seq, Ok(planned_change())),
            Planned::Stale
        );
        assert_eq!(q.entries()[0].plan, Plan::Planning);
        assert!(matches!(
            q.planned(&second.id, second.seq, Ok(planned_change())),
            Planned::Kept(_)
        ));
        assert!(matches!(q.entries()[0].plan, Plan::Planned(_)));
    }

    // Prototype `toggleDel`.
    #[test]
    fn delete_toggles() {
        let mut q = Queue::default();
        let t = invoices();
        assert!(matches!(
            q.toggle_delete(&t, &key(48106), &row(48106, "540.00"), "delete".into()),
            Outcome::Staged(_)
        ));
        assert!(q.deleted(&t, &key(48106)));
        assert_eq!(q.counts().deletes, 1);
        assert_eq!(
            q.toggle_delete(&t, &key(48106), &row(48106, "540.00"), "delete".into()),
            Outcome::Unstaged
        );
        assert!(!q.deleted(&t, &key(48106)));
    }

    #[test]
    fn set_default_replaces_the_cell_s_update() {
        let mut q = Queue::default();
        let t = invoices();
        edit(&mut q, 1, text("5"), "4");
        assert!(matches!(
            q.set_default(&t, &key(1), &row(1, "4"), "total", "default".into()),
            Outcome::Staged(_)
        ));
        assert_eq!(q.entries().len(), 1);
        assert!(matches!(q.entries()[0].staging, Staging::SetDefault { .. }));
        assert_eq!(
            q.set_default(&t, &key(1), &row(1, "4"), "total", "default".into()),
            Outcome::Nothing
        );
    }

    #[test]
    fn an_insert_plans_once_it_has_a_value_and_keeps_the_grid_s_order() {
        let mut q = Queue::default();
        let t = invoices();
        let id = q.add_insert(&t, "insert".into());
        assert_eq!(q.counts().inserts, 1);
        assert_eq!(q.entry(&id).unwrap().edit(), None, "nothing to plan yet");
        let order = ["id".to_string(), "customer".into(), "total".into()];
        let Outcome::Staged(request) =
            q.edit_insert(&id, "total", Some(text("10")), &order, "edit".into())
        else {
            panic!()
        };
        q.edit_insert(&id, "customer", Some(text("Acme")), &order, "edit".into());
        let Some(Edit::InsertRow { values, .. }) = q.entry(&id).unwrap().edit() else {
            panic!()
        };
        let columns: Vec<_> = values.iter().map(|(c, _)| c.as_str()).collect();
        assert_eq!(columns, ["customer", "total"]);
        assert!(matches!(request.edit, Edit::InsertRow { .. }));
        // Back to its default: the value goes.
        q.edit_insert(&id, "customer", None, &order, "default".into());
        q.edit_insert(&id, "total", None, &order, "default".into());
        assert_eq!(q.entry(&id).unwrap().plan, Plan::Empty);
        assert_eq!(q.drop_insert(&id, "drop".into()), Outcome::Unstaged);
        assert!(q.is_empty());
    }

    // Prototype `undo`: each action kind.
    #[test]
    fn undo_takes_back_each_action_in_turn() {
        let mut q = Queue::default();
        let t = invoices();
        edit(&mut q, 1, text("5"), "4");
        q.toggle_delete(&t, &key(2), &row(2, "9"), "delete id 2".into());
        let id = q.add_insert(&t, "insert".into());
        q.edit_insert(
            &id,
            "total",
            Some(text("1")),
            &["total".into()],
            "edit".into(),
        );
        edit(&mut q, 1, text("4"), "4");
        assert_eq!(
            q.entries().len(),
            2,
            "edit back unstaged, insert and delete kept"
        );

        let labels: Vec<String> = std::iter::from_fn(|| q.undo().map(|(l, _)| l)).collect();
        assert_eq!(
            labels,
            [
                "edit total · id 1",
                "edit",
                "insert",
                "delete id 2",
                "edit total · id 1"
            ]
        );
        assert!(q.is_empty());
        assert!(q.undo().is_none());
    }

    #[test]
    fn undo_plans_again_what_was_still_planning() {
        let mut q = Queue::default();
        let Outcome::Staged(first) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        edit(&mut q, 1, text("6"), "4");
        // The first plan arrives late and is dropped; undo brings the first
        // value back, and its plan must be asked for again.
        assert_eq!(
            q.planned(&first.id, first.seq, Ok(planned_change())),
            Planned::Stale
        );
        let (_, requests) = q.undo().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].seq > first.seq);
        assert!(matches!(
            q.planned(&requests[0].id, requests[0].seq, Ok(planned_change())),
            Planned::Kept(_)
        ));
    }

    // A refused plan (`NOT_EDITABLE`) takes its staging back.
    #[test]
    fn a_refused_plan_takes_its_staging_back() {
        let mut q = Queue::default();
        let t = invoices();
        let Outcome::Staged(first) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        q.planned(&first.id, first.seq, Ok(planned_change()));
        let Outcome::Staged(default) =
            q.set_default(&t, &key(1), &row(1, "4"), "total", "d".into())
        else {
            panic!()
        };
        let refused = q.planned(
            &default.id,
            default.seq,
            Err(CallError::new("NOT_EDITABLE", "no default")),
        );
        assert!(matches!(refused, Planned::Refused(_, _)));
        assert!(
            matches!(q.entries()[0].staging, Staging::Update { .. }),
            "the update before it is back"
        );
        // The step went with it: one undo left, the first edit's.
        assert_eq!(q.undo().unwrap().0, "edit total · id 1");
        assert!(q.is_empty());

        let Outcome::Staged(delete) = q.toggle_delete(&t, &key(3), &row(3, "1"), "delete".into())
        else {
            panic!()
        };
        q.planned(
            &delete.id,
            delete.seq,
            Err(CallError::new("NOT_EDITABLE", "no primary key")),
        );
        assert!(q.is_empty());
        assert!(q.undo().is_none());
    }

    fn refuse(code: &str) -> Result<PlannedChange, CallError> {
        Err(CallError::new(code, format!("{code} message")))
    }

    // I4: only refusals of the edit itself take it back.
    #[test]
    fn only_a_refusal_of_the_edit_takes_it_back() {
        for (code, reverted) in [
            ("NOT_EDITABLE", true),
            ("INVALID_ARGUMENT", true),
            ("CONNECTION_NOT_FOUND", false),
            ("QUERY_ERROR", false),
        ] {
            let mut q = Queue::default();
            let Outcome::Staged(r) = edit(&mut q, 1, text("5"), "4") else {
                panic!()
            };
            match q.planned(&r.id, r.seq, refuse(code)) {
                Planned::Refused(..) => assert!(reverted, "{code}"),
                Planned::Failed(..) => {
                    assert!(!reverted, "{code}");
                    assert_eq!(q.entries()[0].plan, Plan::Unplanned, "kept, to plan again");
                }
                other => panic!("{code}: {other:?}"),
            }
            assert_eq!(q.is_empty(), reverted, "{code}");
        }
    }

    // M4: a refusal's staging is gone for good, and undo still takes back
    // the stagings around it, valid ones kept until then.
    #[test]
    fn undo_after_a_refusal_neither_resurrects_it_nor_loses_others() {
        let mut q = Queue::default();
        let t = invoices();
        let Outcome::Staged(a) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        q.planned(&a.id, a.seq, Ok(planned_change()));
        let Outcome::Staged(b) = q.toggle_delete(&t, &key(2), &row(2, "9"), "delete id 2".into())
        else {
            panic!()
        };
        // C is staged before B's answer arrives.
        let Outcome::Staged(c) = edit(&mut q, 3, text("8"), "7") else {
            panic!()
        };
        assert!(matches!(
            q.planned(&b.id, b.seq, refuse("NOT_EDITABLE")),
            Planned::Refused(..)
        ));
        q.planned(&c.id, c.seq, Ok(planned_change()));
        assert_eq!(q.entries().len(), 2);
        assert_eq!(q.undo().unwrap().0, "edit total · id 3");
        assert_eq!(q.entries().len(), 1);
        assert!(!q.deleted(&t, &key(2)), "the refused delete stays gone");
        assert_eq!(q.entries()[0].id, a.id, "the first edit is kept");
        assert_eq!(q.undo().unwrap().0, "edit total · id 1");
        assert!(q.is_empty());
        assert!(q.undo().is_none());
    }

    // M4: a refused re-edit brings back the earlier valid one.
    #[test]
    fn a_refused_re_edit_keeps_the_earlier_one() {
        let mut q = Queue::default();
        let Outcome::Staged(a) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        q.planned(&a.id, a.seq, Ok(planned_change()));
        let Outcome::Staged(again) = edit(&mut q, 1, text("x"), "4") else {
            panic!()
        };
        q.planned(&again.id, again.seq, refuse("INVALID_ARGUMENT"));
        let Staging::Update { value, .. } = &q.entries()[0].staging else {
            panic!()
        };
        assert_eq!(*value, text("5"));
        assert!(matches!(q.entries()[0].plan, Plan::Planned(_)));
    }

    // I3: the queue belongs to one saved connection while it holds anything.
    #[test]
    fn the_queue_is_bound_to_one_connection() {
        let mut q = Queue::default();
        assert_eq!(q.connection(), None);
        q.bind("conn-a").unwrap();
        assert_eq!(q.connection(), None, "nothing staged yet: free");
        q.bind("conn-b").unwrap();
        edit(&mut q, 1, text("5"), "4");
        assert_eq!(q.connection(), Some("conn-b"));
        assert!(q.bind("conn-a").is_err());
        assert!(q.bind("conn-b").is_ok());
        let before = q.generation();
        edit(&mut q, 1, text("4"), "4");
        assert!(q.generation() > before, "every change moves the generation");
        assert!(q.is_empty());
        assert_eq!(
            q.connection(),
            Some("conn-b"),
            "undo can still bring it back"
        );
        q.clear();
        assert_eq!(q.connection(), None);
    }

    #[test]
    fn unplanned_entries_wait_for_their_connection() {
        let mut q = Queue::default();
        let Outcome::Staged(a) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        edit(&mut q, 1, text("6"), "4");
        q.planned(&a.id, a.seq, Ok(planned_change()));
        let (_, requests) = q.undo().unwrap();
        q.unplan(&requests);
        assert_eq!(q.entries()[0].plan, Plan::Unplanned);
        // Its old answer can't land on it.
        assert_eq!(
            q.planned(&requests[0].id, requests[0].seq, Ok(planned_change())),
            Planned::Stale
        );
    }

    #[test]
    fn debug_shows_no_keys_or_values() {
        let mut q = Queue::default();
        edit(&mut q, 48109, text("secret-marker"), "2975.00");
        let text = format!("{q:?}");
        assert!(
            !text.contains("secret-marker") && !text.contains("48109"),
            "{text}"
        );
    }

    fn items() -> TableTarget {
        TableTarget {
            schema: "public".into(),
            table: "items".into(),
        }
    }

    fn plan_all(q: &mut Queue) {
        let pending: Vec<(String, u64)> = q
            .entries()
            .iter()
            .filter(|e| e.plan == Plan::Planning)
            .map(|e| (e.id.clone(), e.seq))
            .collect();
        for (id, seq) in pending {
            q.planned(&id, seq, Ok(planned_change()));
        }
    }

    // Prototype `unstage` and `undo` (`undel`, `edit` with `prev`); GUI:
    // removing a queued change (`PendingChangesManager.remove`).
    #[test]
    fn unstage_takes_an_entry_out_and_undo_brings_it_back() {
        let mut q = Queue::default();
        let t = invoices();
        edit(&mut q, 48109, text("3150.00"), "2975.00");
        q.toggle_delete(
            &t,
            &key(48106),
            &row(48106, "540.00"),
            "delete id 48106".into(),
        );
        plan_all(&mut q);
        let id = q.entries()[1].id.clone();
        assert_eq!(
            q.unstage(&id, "unstage delete id 48106".into()),
            Outcome::Unstaged
        );
        assert!(!q.deleted(&t, &key(48106)));
        assert_eq!(q.counts().total(), 1);
        assert_eq!(q.unstage("stage-nope", "x".into()), Outcome::Nothing);
        let (label, requests) = q.undo().unwrap();
        assert_eq!(label, "unstage delete id 48106");
        assert!(requests.is_empty(), "it was planned: nothing to ask again");
        assert!(q.deleted(&t, &key(48106)));
        assert_eq!(q.entries()[1].id, id, "back in its place");
        assert!(matches!(q.entries()[1].plan, Plan::Planned(_)));
    }

    // Design 1c: panel 4 groups by table, tables in the order first staged.
    #[test]
    fn panel_four_groups_entries_by_table() {
        let mut q = Queue::default();
        edit(&mut q, 48109, text("1"), "2");
        q.toggle_delete(&items(), &key(12), &row(12, "1"), "d".into());
        q.toggle_delete(&invoices(), &key(48106), &row(48106, "1"), "d".into());
        q.add_insert(&items(), "i".into());
        let order = q.display_order();
        let tables: Vec<&str> = order
            .iter()
            .map(|&i| q.entries()[i].target.table.as_str())
            .collect();
        assert_eq!(tables, ["invoices", "invoices", "items", "items"]);
        assert_eq!(order, [0, 2, 1, 3]);
    }

    // Entries left unplanned are planned again before the
    // commit dialog shows their SQL; an empty insert isn't sent.
    #[test]
    fn replan_asks_again_for_what_is_unplanned_and_changes_skip_empty_inserts() {
        let mut q = Queue::default();
        let Outcome::Staged(a) = edit(&mut q, 1, text("5"), "4") else {
            panic!()
        };
        q.planned(&a.id, a.seq, refuse("QUERY_ERROR"));
        assert_eq!(q.entries()[0].plan, Plan::Unplanned);
        q.add_insert(&invoices(), "i".into());
        assert!(!q.all_planned());
        let requests = q.replan();
        assert_eq!(requests.len(), 1, "the empty insert has nothing to plan");
        assert_eq!(q.entries()[0].plan, Plan::Planning);
        assert!(requests[0].seq > a.seq);
        assert!(q.replan().is_empty(), "already asked");
        q.planned(&requests[0].id, requests[0].seq, Ok(planned_change()));
        assert!(q.all_planned(), "the empty insert doesn't hold it up");
        let changes = q.changes();
        assert_eq!(changes.len(), 1);
        assert!(matches!(&changes[0], Change::Edit { id, .. } if *id == a.id));
    }

    // GUI `PendingChangesManager.apply`: applied entries leave; the failed
    // one is marked until the next staging action.
    #[test]
    fn applied_entries_leave_and_a_failure_is_marked_until_the_next_staging() {
        let mut q = Queue::default();
        edit(&mut q, 1, text("5"), "4");
        edit(&mut q, 2, text("6"), "4");
        edit(&mut q, 3, text("7"), "4");
        let ids: Vec<String> = q.entries().iter().map(|e| e.id.clone()).collect();
        q.mark_failed(&ids[1], CallError::new("NO_ROWS_AFFECTED", "gone"));
        assert_eq!(
            q.failure(&ids[1]).map(|e| e.code.as_str()),
            Some("NO_ROWS_AFFECTED")
        );
        assert_eq!(q.failure(&ids[0]), None);
        q.remove_applied(&ids[..1]);
        assert_eq!(q.entries().len(), 2);
        assert!(q.undo().is_none(), "no undo across an apply");
        assert!(q.failure(&ids[1]).is_some(), "still marked");
        edit(&mut q, 9, text("1"), "2");
        assert_eq!(q.failure(&ids[1]), None, "a staging clears the mark");
        q.mark_failed(&ids[2], CallError::new("X", "y"));
        q.clear();
        assert_eq!(q.failure(&ids[2]), None);
    }
}
