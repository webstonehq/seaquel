//! Ask AI (Q10 A, Decision 20; design 1d) in `update`: a popup over the
//! query tab. Ctrl+K (or `:ask`) opens it; the title says what the
//! connection shares (Core's sharing rule, read with the library); `@`
//! completes table, saved-query and dashboard names; Enter sends Core's
//! `ai_generate` with the request **as typed** (Core resolves the mentions)
//! and the editor's text as the existing query.
//!
//! **The answer is only inserted.** Enter inserts it at the cursor. Ctrl+R
//! inserts it as a statement of its own and runs **only that statement**
//! (`db.run` with the cursor inside it), and only when it is one statement
//! that Core's read-only token check (`seaquel_core::sql::read_only`)
//! passes and the cursor isn't inside a statement of the user's; otherwise
//! it is inserted, not run, and the popup says why (Q10 A). The gate is
//! that token check: the run itself is an ordinary editor run, not a
//! read-only transaction (Decision 25; a `db.run` read-only flag is a
//! Follow-up). Tab refines (the
//! shown SQL becomes the existing query), Ctrl+S saves it as a new saved
//! query, Esc closes (or, while waiting, drops the request).
//!
//! **Selection:** the editor has no visual selection in 7a (Q7 A's Normal
//! mode has none), so "replacing the selection" has nothing to replace and
//! the answer goes in at the cursor (`Editor::insert_str`, which would
//! replace a selection if the textarea held one).
//!
//! No prompt, SQL or key reaches `Debug` or a log line: the types here
//! print sizes and codes only. The key never reaches the TUI at all: Core
//! reads it from the keychain (or the test hook's store) for each call.

use std::fmt;
use std::time::Instant;

use seaquel_core::sql::read_only::read_only_error;
use seaquel_core::sql::scan::{split_statements, statement_at, tokens};

use super::app::{Effect, Modal, Model};
use super::completion::{Item, ItemKind};
use super::dialogs::CallError;
use super::keymap::BarContext;
use super::log::Tag;
use super::panels::TableKind;
use super::query::{self, RunKind, SaveAs, SqlText};
use super::text;

/// Where the popup is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The request is being typed.
    Prompt,
    /// `ai_generate` is out (since the tick it was sent at).
    Waiting { since: Option<Instant> },
    /// The answer is shown.
    Answer,
}

/// The generated SQL.
#[derive(Clone, PartialEq, Eq)]
pub struct Answer {
    pub sql: String,
    pub elapsed_ms: u64,
    /// The connection's model, as the library names it.
    pub model: Option<String>,
    /// Ctrl+R put it in the editor without running it.
    pub inserted: bool,
}

impl fmt::Debug for Answer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Answer")
            .field("sql_bytes", &self.sql.len())
            .field("elapsed_ms", &self.elapsed_ms)
            .field("inserted", &self.inserted)
            .finish_non_exhaustive()
    }
}

/// The `@` list: what the word after `@` matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    /// The byte offset of the `@` in the prompt.
    pub start: usize,
    pub selected: usize,
}

/// The popup.
#[derive(Clone, PartialEq, Eq)]
pub struct Ask {
    /// The query tab it inserts into.
    pub tab: u64,
    /// The saved connection whose provider, model and sharing apply.
    pub connection_id: String,
    pub project_id: String,
    /// The request as typed.
    pub prompt: String,
    pub mention: Option<Mention>,
    pub stage: Stage,
    pub answer: Option<Answer>,
    /// Tab: the next request refines the shown SQL.
    pub refining: bool,
    /// Why the last request failed, worded.
    pub error: Option<String>,
    /// What happened to the answer (inserted, not run; saved; stopped).
    pub note: Option<String>,
    /// The project's dashboards' names, for `@`.
    pub dashboards: Vec<String>,
    /// The request in flight (a late answer to another is dropped).
    pub op: u64,
}

impl fmt::Debug for Ask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ask")
            .field("tab", &self.tab)
            .field("connection_id", &self.connection_id)
            .field("prompt_bytes", &self.prompt.len())
            .field("stage", &self.stage)
            .field("answer", &self.answer)
            .field("refining", &self.refining)
            .field("error", &self.error.is_some())
            .field("op", &self.op)
            .finish_non_exhaustive()
    }
}

impl Ask {
    /// The key bar (and keymap chain) the popup's stage needs.
    pub fn bar_context(&self) -> BarContext {
        match self.stage {
            Stage::Waiting { .. } => BarContext::AskWaiting,
            Stage::Answer if self.answer.as_ref().is_some_and(|a| a.inserted) => {
                BarContext::AskDone
            }
            Stage::Answer => BarContext::AskAnswer,
            Stage::Prompt if self.mention.is_some() => BarContext::AskMention,
            Stage::Prompt => BarContext::AskPrompt,
        }
    }
}

/// A project's dashboards' names (Ask AI's `@` list): `Debug` shows how
/// many.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Names(pub Vec<String>);

impl fmt::Debug for Names {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Names({})", self.0.len())
    }
}

/// `ai_generate` for the runtime.
#[derive(Clone, PartialEq, Eq)]
pub struct GenerateCall {
    pub op: u64,
    /// The saved connection.
    pub connection_id: String,
    pub request: String,
    /// The editor's text, or the SQL being refined.
    pub existing: String,
}

impl fmt::Debug for GenerateCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenerateCall")
            .field("op", &self.op)
            .field("connection_id", &self.connection_id)
            .field("request_bytes", &self.request.len())
            .field("existing_bytes", &self.existing.len())
            .finish()
    }
}

/// How many items the `@` list holds at most.
const MENTION_MAX: usize = 50;

/// How long the request may grow, in bytes.
const PROMPT_MAX: usize = 16 * 1024;

fn ask_ref(model: &Model) -> Option<&Ask> {
    match &model.modal {
        Some(Modal::Ask(a)) => Some(a),
        _ => None,
    }
}

fn ask_mut(model: &mut Model) -> Option<&mut Ask> {
    match &mut model.modal {
        Some(Modal::Ask(a)) => Some(a),
        _ => None,
    }
}

/// A space as Core's mention scanner reads one (JavaScript's `\s`).
fn is_space(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Ctrl+K, `:ask`: the popup over the active query tab.
pub fn open(model: &mut Model) -> Vec<Effect> {
    let Some(tab) = model.query.active_mut() else {
        return vec![Model::log_effect(Some(Tag::Error), text::ASK_NO_TAB)];
    };
    tab.editor.completion = None;
    let tab = tab.id;
    let Some(conn) = model.conn.id().and_then(|id| model.library.connection(id)) else {
        return vec![Model::log_effect(
            Some(Tag::Error),
            text::ASK_NEEDS_CONNECTION,
        )];
    };
    let (connection_id, project_id) = (conn.id.clone(), conn.project_id.clone());
    model.modal = Some(Modal::Ask(Ask {
        tab,
        connection_id,
        project_id: project_id.clone(),
        prompt: String::new(),
        mention: None,
        stage: Stage::Prompt,
        answer: None,
        refining: false,
        error: None,
        note: None,
        dashboards: Vec::new(),
        op: 0,
    }));
    vec![Effect::LoadMentions { project_id }]
}

/// The title's sharing line: Core's rule for the connection, as the
/// library read it, or that AI is turned off.
pub fn sharing_line(model: &Model) -> String {
    if model.library.ai_off {
        return text::ASK_AI_OFF.to_string();
    }
    let id = ask_ref(model)
        .map(|a| a.connection_id.as_str())
        .or_else(|| model.conn.id());
    let ai = id
        .and_then(|id| model.library.connection(id))
        .map(|c| c.ai.clone())
        .unwrap_or_default();
    text::ask_sharing(ai.schema, ai.data)
}

/// The mention a name is written as, the way Core's scanner reads it back
/// (`@name`, or `@"name"` for one with a space); `None` for a name it
/// can't (one starting with `"`, or with a space and a `"`).
fn mention_token(name: &str) -> Option<String> {
    // `@"…` is read as a quoted mention, so a name that starts with `"` (a
    // DuckDB catalog part, `"fx.we""ird".main`) can't be written (M5).
    if name.is_empty() || name.starts_with('"') {
        return None;
    }
    if !name.chars().any(is_space) {
        return Some(format!("@{name}"));
    }
    (!name.contains('"')).then(|| format!("@\"{name}\""))
}

/// What the word after `@` matches, from the start of a name: tables
/// (schema-qualified, matched by either name), the project's saved
/// queries, then its dashboards.
pub fn mention_items(model: &Model) -> Vec<Item> {
    let Some(a) = ask_ref(model) else {
        return Vec::new();
    };
    let Some(mention) = &a.mention else {
        return Vec::new();
    };
    let typed = a.prompt[mention.start + 1..]
        .trim_start_matches('"')
        .to_lowercase();
    let starts = |name: &str| name.to_lowercase().starts_with(&typed);
    let mut items = Vec::new();
    let mut push = |label: String, detail: &str, kind: ItemKind| {
        if items.len() < MENTION_MAX && mention_token(&label).is_some() {
            items.push(Item {
                label,
                detail: detail.to_string(),
                kind,
            });
        }
    };
    for t in &model.schema {
        let qualified = format!("{}.{}", t.schema, t.name);
        if starts(&t.name) || starts(&qualified) {
            let detail = match t.kind {
                TableKind::Table => "table",
                TableKind::View => "view",
                TableKind::MaterializedView => "matview",
            };
            push(qualified, detail, ItemKind::Table);
        }
    }
    for s in model.saved_items.iter().filter(|s| starts(&s.name)) {
        push(s.name.clone(), "saved query", ItemKind::SavedQuery);
    }
    for d in a.dashboards.iter().filter(|d| starts(d)) {
        push(d.clone(), "dashboard", ItemKind::Dashboard);
    }
    items
}

/// A bracketed paste into the request: on one line (each line break a
/// space), as typed.
pub fn paste(model: &mut Model, pasted: &str) {
    let flat: String = pasted
        .replace("\r\n", " ")
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    for c in flat.chars() {
        type_char(model, c);
    }
}

/// Whether the request takes printable keys now.
pub fn typing(model: &Model) -> bool {
    ask_ref(model).is_some_and(|a| a.stage == Stage::Prompt)
}

pub fn type_char(model: &mut Model, c: char) {
    let Some(a) = ask_mut(model).filter(|a| a.stage == Stage::Prompt) else {
        return;
    };
    if a.prompt.len() + c.len_utf8() > PROMPT_MAX {
        return;
    }
    a.prompt.push(c);
    a.error = None;
    a.note = None;
    if let Some(mention) = &mut a.mention {
        mention.selected = 0;
        let word = &a.prompt[mention.start + 1..];
        // `@"…"` ends at its closing quote; a bare name at a space.
        let ends = if word.starts_with('"') {
            word.len() > 1 && c == '"'
        } else {
            is_space(c)
        };
        if ends {
            a.mention = None;
        }
    }
}

pub fn backspace(model: &mut Model) {
    let Some(a) = ask_mut(model).filter(|a| a.stage == Stage::Prompt) else {
        return;
    };
    a.prompt.pop();
    a.error = None;
    if let Some(mention) = &mut a.mention {
        mention.selected = 0;
        if a.prompt.len() <= mention.start {
            a.mention = None;
        }
    }
}

/// `@`: typed, and at a word's start (not in an address) the list opens.
pub fn mention(model: &mut Model) {
    let Some(a) = ask_mut(model).filter(|a| a.stage == Stage::Prompt) else {
        return;
    };
    if a.prompt.len() + 1 > PROMPT_MAX {
        return;
    }
    let word_start = a
        .prompt
        .chars()
        .last()
        .is_none_or(|c| is_space(c) || "(,".contains(c));
    a.prompt.push('@');
    a.error = None;
    a.note = None;
    a.mention = word_start.then(|| Mention {
        start: a.prompt.len() - 1,
        selected: 0,
    });
}

pub fn mention_step(model: &mut Model, forward: bool) {
    let n = mention_items(model).len().max(1);
    if let Some(mention) = ask_mut(model).and_then(|a| a.mention.as_mut()) {
        mention.selected = (mention.selected + if forward { 1 } else { n - 1 }) % n;
    }
}

/// Tab or Enter in the list: the name, written so Core reads it back, and
/// a space; with nothing listed the list just closes.
pub fn mention_accept(model: &mut Model) {
    let items = mention_items(model);
    let Some(a) = ask_mut(model) else {
        return;
    };
    let Some(mention) = a.mention.take() else {
        return;
    };
    let Some(token) = items
        .get(mention.selected)
        .and_then(|item| mention_token(&item.label))
    else {
        return;
    };
    if mention.start + token.len() + 1 > PROMPT_MAX {
        return;
    }
    a.prompt.truncate(mention.start);
    a.prompt.push_str(&token);
    a.prompt.push(' ');
}

pub fn mention_close(model: &mut Model) {
    if let Some(a) = ask_mut(model) {
        a.mention = None;
    }
}

/// Enter: `ai_generate` with the request as typed, and the editor's text
/// (or, refining, the shown SQL) as the existing query.
pub fn send(model: &mut Model) -> Vec<Effect> {
    let Some(tab) = ask_ref(model)
        .filter(|a| a.stage == Stage::Prompt)
        .map(|a| a.tab)
    else {
        return Vec::new();
    };
    let editor_text = model
        .query
        .tab(tab)
        .map(|t| t.editor.text())
        .unwrap_or_default();
    let op = query::next_op(model);
    let now = model.now;
    let Some(a) = ask_mut(model) else {
        return Vec::new();
    };
    if a.prompt.trim().is_empty() {
        a.error = Some(text::ASK_EMPTY.to_string());
        return Vec::new();
    }
    let existing = if a.refining {
        a.answer.as_ref().map(|x| x.sql.clone()).unwrap_or_default()
    } else {
        editor_text
    };
    a.op = op;
    a.stage = Stage::Waiting { since: now };
    a.mention = None;
    a.error = None;
    a.note = None;
    vec![Effect::Generate(GenerateCall {
        op,
        connection_id: a.connection_id.clone(),
        request: a.prompt.clone(),
        existing,
    })]
}

/// Esc while waiting: the request is dropped (its future, and with it the
/// HTTP request); a late answer is ignored.
pub fn stop(model: &mut Model) -> Vec<Effect> {
    let Some(a) = ask_mut(model).filter(|a| matches!(a.stage, Stage::Waiting { .. })) else {
        return Vec::new();
    };
    a.stage = Stage::Prompt;
    a.note = Some(text::ASK_STOPPED.to_string());
    vec![Effect::CancelGenerate]
}

/// Whether a request is out (the keychain box may show for it).
pub fn waiting(model: &Model) -> bool {
    ask_ref(model).is_some_and(|a| matches!(a.stage, Stage::Waiting { .. }))
}

/// Esc in the keychain box while a request waits on it: the request is
/// dropped, and the keychain call it waited on isn't the next one's.
pub fn give_up_keychain(model: &mut Model) -> Vec<Effect> {
    let store = model.store;
    let Some(a) = ask_mut(model).filter(|a| matches!(a.stage, Stage::Waiting { .. })) else {
        return Vec::new();
    };
    a.stage = Stage::Prompt;
    a.error = Some(text::ask_keychain_gave_up(store).to_string());
    model.keychain_stale = true;
    vec![
        Effect::CancelGenerate,
        Model::log_effect(
            Some(Tag::Error),
            text::failed_line("ask", "SECRET_UNREADABLE"),
        ),
    ]
}

/// The editor of the tab the popup inserts into, shown and focused.
fn into_tab(model: &mut Model, tab: u64) -> bool {
    let Some(i) = model.query.tabs.iter().position(|t| t.id == tab) else {
        return false;
    };
    model.query.active = i;
    query::focus_editor(model);
    true
}

/// Enter on the answer: in at the cursor, the popup closed. An answer
/// Ctrl+R already inserted isn't inserted again.
pub fn insert(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::Ask(a)) = model.modal.take() else {
        return Vec::new();
    };
    let Some(answer) = a.answer.filter(|x| !x.inserted) else {
        return Vec::new();
    };
    if into_tab(model, a.tab) {
        if let Some(t) = model.query.tab_mut(a.tab) {
            t.editor.completion = None;
            t.editor.insert_str(&answer.sql);
        }
    }
    Vec::new()
}

/// Why Ctrl+R won't run `sql` here, if it won't: not connected to the
/// popup's connection, more than one statement, or a statement the
/// read-only check refuses.
fn why_not_run(model: &Model, a: &Ask, sql: &str) -> Option<String> {
    let connected =
        model.conn.core_id().is_some() && model.conn.id() == Some(a.connection_id.as_str());
    let Some(engine) = query::engine(model).filter(|_| connected) else {
        return Some(text::ASK_NOT_CONNECTED.to_string());
    };
    if split_statements(sql, engine).len() != 1 {
        return Some(text::ASK_NOT_RUN_SEVERAL.to_string());
    }
    // The read-only check passes: nothing stops the run.
    read_only_error(sql, engine)?;
    let verb = tokens(sql, engine)
        .first()
        .map(|t| t.text(sql).to_ascii_uppercase())
        .unwrap_or_default();
    Some(if matches!(verb.as_str(), "SELECT" | "WITH") {
        text::ask_not_run("")
    } else {
        text::ask_not_run(&verb)
    })
}

/// Newlines to add so `n` existing ones make a blank line.
fn blank_line(n: usize) -> &'static str {
    match n {
        0 => "\n\n",
        1 => "\n",
        _ => "",
    }
}

/// `sql` without its `;`s and surrounding spaces.
fn bare(sql: &str) -> &str {
    sql.trim_matches(|c: char| c.is_whitespace() || c == ';')
}

/// Ctrl+R on the answer (Q10 A): a statement of its own at the cursor (a
/// blank line around it, its own `;`), the cursor inside it, and `db.run`
/// of the statement at the cursor, so Core runs exactly it. Only when it is
/// one statement Core's read-only token check passes, the cursor isn't
/// inside a statement of the user's, and it reads back as that statement;
/// otherwise it goes in as it came, isn't run, and the popup says why. The
/// gate is the token check: the run is an ordinary editor run (`db.run`),
/// not a read-only transaction (Decision 25).
pub fn insert_and_run(model: &mut Model) -> Vec<Effect> {
    let Some(a) = ask_ref(model) else {
        return Vec::new();
    };
    let Some(answer) = a.answer.as_ref().filter(|x| !x.inserted) else {
        return Vec::new();
    };
    let sql = answer.sql.clone();
    let tab = a.tab;
    let mut why = why_not_run(model, a, &sql);
    let engine = query::engine(model);
    let Some(t) = model.query.tab_mut(tab) else {
        return Vec::new();
    };
    t.editor.completion = None;
    let full = t.editor.text();
    let (before, after) = full.split_at(t.editor.cursor_byte());
    // Inside the user's statement (code after the last `;` before the
    // cursor): it stays as it was, so the answer only goes in (M6).
    let inside = engine.is_some_and(|engine| {
        tokens(before, engine)
            .last()
            .is_some_and(|tok| tok.text(before) != ";")
    });
    if why.is_none() && inside {
        why = Some(text::ASK_NOT_RUN_ALONE.to_string());
    }
    let (Some(engine), None) = (engine, &why) else {
        t.editor.insert_str(&sql);
        mark_inserted(model, why.unwrap_or_default());
        return Vec::new();
    };
    let body = bare(&sql).to_string();
    let mut head = String::new();
    if !before.trim().is_empty() {
        let trailing = before.len() - before.trim_end_matches('\n').len();
        head.push_str(blank_line(trailing));
    }
    head.push_str(&body);
    let mut tail = String::from(";");
    if !tokens(after, engine).is_empty() {
        let leading = after.len() - after.trim_start_matches('\n').len();
        tail.push_str(blank_line(leading));
    }
    // The cursor goes back to the end of the statement's text.
    let (row, col) = t.editor.cursor();
    let lines = head.matches('\n').count();
    let end_col = match head.rfind('\n') {
        Some(i) => head[i + 1..].chars().count(),
        None => col + head.chars().count(),
    };
    t.editor.insert_str(&format!("{head}{tail}"));
    t.editor.jump(row + lines, end_col);
    // It must read back as that statement, or nothing runs.
    let full = t.editor.text();
    let own = statement_at(&full, t.editor.cursor_byte(), engine)
        .is_some_and(|s| bare(&full[s.text]) == body);
    if !own {
        mark_inserted(model, text::ASK_NOT_RUN_ALONE.to_string());
        return Vec::new();
    }
    model.modal = None;
    into_tab(model, tab);
    query::run(model, RunKind::Current, false)
}

/// The answer went in without running: the popup says why, and Enter only
/// closes now.
fn mark_inserted(model: &mut Model, why: String) {
    if let Some(a) = ask_mut(model) {
        a.note = Some(why);
        if let Some(answer) = a.answer.as_mut() {
            answer.inserted = true;
        }
    }
}

/// Tab on the answer: a new request with the shown SQL as the existing
/// query.
pub fn refine(model: &mut Model) {
    if let Some(a) = ask_mut(model).filter(|a| a.stage == Stage::Answer) {
        a.stage = Stage::Prompt;
        a.refining = true;
        a.prompt.clear();
        a.error = None;
        a.note = None;
    }
}

/// Ctrl+S on the answer: its name, then `savedQueryCreate` of the SQL (not
/// the tab's text) in panel 3's project; Esc or the save comes back here.
pub fn save(model: &mut Model) -> Vec<Effect> {
    if model.project.is_none() {
        if let Some(a) = ask_mut(model) {
            a.note = Some(text::NEEDS_PROJECT.to_string());
        }
        return Vec::new();
    }
    let Some(Modal::Ask(a)) = model.modal.take() else {
        return Vec::new();
    };
    let Some(sql) = a.answer.as_ref().map(|x| x.sql.clone()) else {
        model.modal = Some(Modal::Ask(a));
        return Vec::new();
    };
    model.modal = Some(Modal::SaveAs(SaveAs {
        tab: a.tab,
        name: String::new(),
        error: None,
        text: Some(SqlText(sql)),
        back: Some(Box::new(a)),
    }));
    Vec::new()
}

/// `ai_generate` answered: the SQL, or Core's refusal worded (the log
/// names the code only).
pub fn on_generated(
    model: &mut Model,
    op: u64,
    result: Result<SqlText, CallError>,
    elapsed_ms: u64,
) -> Vec<Effect> {
    let model_name = ask_ref(model)
        .and_then(|a| model.library.connection(&a.connection_id))
        .and_then(|c| c.ai.model.clone());
    let Some(a) = ask_mut(model).filter(|a| a.op == op && matches!(a.stage, Stage::Waiting { .. }))
    else {
        return Vec::new();
    };
    a.stage = Stage::Prompt;
    match result {
        Ok(SqlText(sql)) if sql.trim().is_empty() => {
            a.error = Some(text::ASK_NO_SQL.to_string());
            Vec::new()
        }
        Ok(SqlText(sql)) => {
            a.answer = Some(Answer {
                sql,
                elapsed_ms,
                model: model_name,
                inserted: false,
            });
            a.stage = Stage::Answer;
            a.refining = false;
            Vec::new()
        }
        Err(e) => {
            a.error = Some(text::ask_error(&e.code, &e.message));
            vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("ask", &e.code),
            )]
        }
    }
}

/// Ctrl+S's save answered (`Msg::QuerySaved` with `detached`): the tab
/// stays as it is; panel 3 gains the row.
pub fn on_saved(
    model: &mut Model,
    tab: u64,
    text: SqlText,
    result: Result<super::panels::SavedItem, CallError>,
    taken_by: Option<String>,
) -> Vec<Effect> {
    match result {
        Ok(item) => {
            let mut effects = vec![Model::log_effect(None, super::text::saved_line(&item.name))];
            if item.shared {
                effects.push(Model::log_effect(None, super::text::SAVED_SHARED));
            }
            if let Some(a) = ask_mut(model) {
                a.note = Some(super::text::ask_saved(&item.name));
            }
            if model.project.is_some() {
                match model.saved_items.iter_mut().find(|s| s.id == item.id) {
                    Some(row) => *row = item,
                    None => model.saved_items.push(item),
                }
                model.refresh_lists();
            }
            effects
        }
        Err(e) if e.code == "NAME_TAKEN" => {
            let holder = taken_by
                .as_deref()
                .and_then(|id| model.saved_items.iter().find(|s| s.id == id))
                .map(|s| s.name.as_str());
            let message = super::text::name_taken(holder);
            let back = match model.modal.take() {
                Some(Modal::Ask(a)) => Some(Box::new(a)),
                None => None,
                other => {
                    model.modal = other;
                    return Vec::new();
                }
            };
            model.modal = Some(Modal::SaveAs(SaveAs {
                tab,
                name: String::new(),
                error: Some(message),
                text: Some(text),
                back,
            }));
            Vec::new()
        }
        Err(e) => {
            let message = super::text::apply_failed(&e.code, &e.message);
            match ask_mut(model) {
                Some(a) => a.error = Some(message),
                None => {
                    if model.modal.is_none() {
                        model.modal = Some(Modal::Notice(super::dialogs::Notice(message)));
                    }
                }
            }
            vec![Model::log_effect(
                Some(Tag::Error),
                super::text::failed_line("save", &e.code),
            )]
        }
    }
}

/// The project's dashboards arrived (a failed read lists none).
pub fn on_mentions(model: &mut Model, project_id: &str, result: Result<Names, CallError>) {
    if let (Some(a), Ok(Names(names))) = (ask_mut(model), result) {
        if a.project_id == project_id {
            a.dashboards = names;
        }
    }
}

#[cfg(test)]
mod tests;
