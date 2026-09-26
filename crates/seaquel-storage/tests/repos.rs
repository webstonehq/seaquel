//! Replays every case in `tests/fixtures/repos/*.json` (what the TypeScript
//! repositories did, frozen in phase 3 Task 2) against the Rust queries: the
//! same calls with the same arguments must return the same objects (compared
//! as JSON values) and leave the same rows, storage classes included. See
//! `tests/fixtures/README.md` for the format.
//!
//! Then the password stripping of Decision 13.1: `connections::save` and
//! the `strip_connection_string_passwords` data step.

mod common;

use std::collections::BTreeSet;
use std::path::Path;

use seaquel_storage::{
    ai_chats, app_state, connection_overrides, connections, dashboard_versions, dashboards,
    import_state, license, onboarding, project_state, projects, query_history, query_versions,
    saved_queries, shared_repos, strip_connection_string_password, themes, tutorial,
    user_credentials, vault_state, Storage, StorageError, StorageOptions, DATA_STEPS_TABLE,
};
use seaquel_types::storage::{DashboardVersionsPrune, QueryVersionPromote, QueryVersionsPrune};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use sqlx::sqlite::SqliteValueRef;
use sqlx::{Decode, Row, TypeInfo, ValueRef};

// ---------------------------------------------------------------------------
// Fixture format
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FixtureFile {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    steps: Vec<Step>,
    rows: serde_json::Map<String, Value>,
}

#[derive(Deserialize)]
struct Step {
    call: Option<String>,
    sql: Option<String>,
    #[serde(default)]
    args: Vec<Box<RawValue>>,
    /// `Some(Value::Null)` for a recorded `null`, `None` when the step has no
    /// `result` key (the call returns nothing).
    #[serde(default, deserialize_with = "present")]
    result: Option<Value>,
    #[serde(default)]
    error: Option<Value>,
}

fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

#[derive(Deserialize)]
struct TableRows {
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    types: Vec<Vec<String>>,
}

/// The recorder's placeholder for a random workflow id.
const MASKED_WORKFLOW_ID: &str = "workflow-<random-uuid>";

// ---------------------------------------------------------------------------
// Arguments: the recorded JSON as the TypeScript client would send it
// ---------------------------------------------------------------------------

/// The argument's JSON text as `JSON.stringify` would write it: the fixture's
/// indentation removed (key order kept), and the recorder's `{"$date": x}`
/// for a JS `Date` replaced by the ISO string it crosses the wire as.
fn wire_text(raw: &RawValue) -> String {
    let mut out = String::with_capacity(raw.get().len());
    let (mut in_string, mut escaped) = (false, false);
    for c in raw.get().chars() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            out.push(c);
        } else if !c.is_whitespace() {
            out.push(c);
        }
    }
    // `{"$date":"…"}` or `{"$date":null}`; the value never holds a brace.
    while let Some(start) = out.find(r#"{"$date":"#) {
        let end = start + out[start..].find('}').expect("closing brace of $date");
        let inner = out[start + r#"{"$date":"#.len()..end].to_string();
        out.replace_range(start..=end, &inner);
    }
    out
}

struct Args(Vec<String>);

impl Args {
    fn get<T: DeserializeOwned>(&self, i: usize) -> T {
        serde_json::from_str(&self.0[i])
            .unwrap_or_else(|e| panic!("argument {i} ({}): {e}", self.0[i]))
    }
    fn str(&self, i: usize) -> String {
        self.get(i)
    }
    fn opt_str(&self, i: usize) -> Option<String> {
        self.get(i)
    }
    fn raw(&self, i: usize) -> Box<RawValue> {
        self.get(i)
    }
}

fn to_json<T: Serialize>(v: T) -> Option<Value> {
    Some(serde_json::to_value(v).expect("result serializes"))
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// Runs one recorded repository call. `Ok(None)` for a call that returns
/// nothing.
async fn call(st: &Storage, name: &str, a: &Args) -> Result<Option<Value>, StorageError> {
    Ok(match name {
        "projectsRepo.loadAll" => to_json(projects::load_all(st).await?),
        "projectsRepo.save" => {
            projects::save(st, &a.get(0)).await?;
            None
        }
        "projectsRepo.saveAll" => {
            projects::save_all(st, &a.get::<Vec<_>>(0)).await?;
            None
        }
        "projectsRepo.remove" => {
            projects::remove(st, &a.str(0)).await?;
            None
        }

        "appStateRepo.get" => to_json(app_state::get(st, &a.str(0)).await?),
        "appStateRepo.set" => {
            app_state::set(st, &a.str(0), a.opt_str(1).as_deref()).await?;
            None
        }

        "connectionsRepo.loadAll" => to_json(connections::load_all(st).await?),
        "connectionsRepo.save" => {
            connections::save(st, &a.get(0)).await?;
            None
        }
        "connectionsRepo.remove" => {
            connections::remove(st, &a.str(0)).await?;
            None
        }

        "connectionOverridesRepo.load" => to_json(connection_overrides::load(st, &a.str(0)).await?),
        "connectionOverridesRepo.loadAll" => to_json(connection_overrides::load_all(st).await?),
        "connectionOverridesRepo.save" => {
            connection_overrides::save(st, &a.get(0)).await?;
            None
        }
        "connectionOverridesRepo.remove" => {
            connection_overrides::remove(st, &a.str(0)).await?;
            None
        }

        "projectStateRepo.load" => to_json(project_state::load(st, &a.str(0)).await?),
        "projectStateRepo.save" => {
            project_state::save(st, &a.get(0)).await?;
            None
        }
        "projectStateRepo.remove" => {
            project_state::remove(st, &a.str(0)).await?;
            None
        }

        "savedQueriesRepo.loadByProject" => {
            to_json(saved_queries::load_by_project(st, &a.str(0)).await?)
        }
        "savedQueriesRepo.saveAll" => {
            saved_queries::save_all(st, &a.str(0), &a.get::<Vec<_>>(1)).await?;
            None
        }
        "savedQueriesRepo.removeByProject" => {
            saved_queries::remove_by_project(st, &a.str(0)).await?;
            None
        }

        "queryVersionsRepo.loadByQuery" => {
            to_json(query_versions::load_by_query(st, &a.str(0)).await?)
        }
        "queryVersionsRepo.loadByProject" => {
            to_json(query_versions::load_by_project(st, &a.str(0)).await?)
        }
        "queryVersionsRepo.insert" => {
            query_versions::insert(st, &a.get(0)).await?;
            None
        }

        "queryHistoryRepo.loadByConnection" => {
            to_json(query_history::load_by_connection(st, &a.str(0)).await?)
        }
        "queryHistoryRepo.replaceAll" => {
            query_history::replace_all(st, &a.str(0), &a.get::<Vec<_>>(1)).await?;
            None
        }
        "queryHistoryRepo.removeByConnection" => {
            query_history::remove_by_connection(st, &a.str(0)).await?;
            None
        }

        "sharedReposRepo.loadAll" => to_json(shared_repos::load_all(st).await?),
        "sharedReposRepo.saveAll" => {
            shared_repos::save_all(st, &a.get::<Vec<Box<RawValue>>>(0), a.opt_str(1).as_deref())
                .await?;
            None
        }

        "themeRepo.loadPreferences" => to_json(themes::load_preferences(st).await?),
        "themeRepo.savePreferences" => {
            themes::save_preferences(st, &a.str(0), &a.str(1)).await?;
            None
        }
        "themeRepo.loadUserThemes" => to_json(themes::load_user_themes(st).await?),
        "themeRepo.saveUserThemes" => {
            themes::save_user_themes(st, &a.get::<Vec<Box<RawValue>>>(0)).await?;
            None
        }

        "licenseRepo.load" => to_json(license::load(st).await?),
        "licenseRepo.save" => {
            license::save(st, &a.raw(0)).await?;
            None
        }
        "onboardingRepo.load" => to_json(onboarding::load(st).await?),
        "onboardingRepo.save" => {
            onboarding::save(st, &a.raw(0)).await?;
            None
        }

        "tutorialRepo.loadAll" => to_json(tutorial::load_all(st).await?),
        "tutorialRepo.save" => {
            tutorial::save(st, &a.str(0), &a.str(1), a.opt_str(2).as_deref()).await?;
            None
        }
        "tutorialRepo.removeLesson" => {
            tutorial::remove_lesson(st, &a.str(0)).await?;
            None
        }
        "tutorialRepo.removeAll" => {
            tutorial::remove_all(st).await?;
            None
        }

        "importStateRepo.load" => to_json(import_state::load(st, &a.str(0)).await?),
        "importStateRepo.save" => {
            import_state::save(st, &a.str(0), a.get(1), a.opt_str(2).as_deref()).await?;
            None
        }

        "dashboardsRepo.loadByProject" => {
            to_json(dashboards::load_by_project(st, &a.str(0)).await?)
        }
        "dashboardsRepo.save" => {
            dashboards::save(st, &a.get(0)).await?;
            None
        }
        "dashboardsRepo.remove" => {
            dashboards::remove(st, &a.str(0)).await?;
            None
        }
        "dashboardsRepo.removeByProject" => {
            dashboards::remove_by_project(st, &a.str(0)).await?;
            None
        }

        "dashboardVersionsRepo.loadByDashboard" => {
            to_json(dashboard_versions::load_by_dashboard(st, &a.str(0)).await?)
        }
        "dashboardVersionsRepo.loadByProject" => {
            to_json(dashboard_versions::load_by_project(st, &a.str(0)).await?)
        }
        "dashboardVersionsRepo.insert" => {
            dashboard_versions::insert(st, &a.get(0)).await?;
            None
        }

        "aiChatsRepo.loadByConnection" => {
            to_json(ai_chats::load_by_connection(st, &a.str(0)).await?)
        }
        "aiChatsRepo.saveChat" => {
            ai_chats::save_chat(st, &a.get(0)).await?;
            None
        }
        "aiChatsRepo.removeChat" => {
            ai_chats::remove_chat(st, &a.str(0)).await?;
            None
        }
        "aiChatsRepo.removeByConnection" => {
            ai_chats::remove_by_connection(st, &a.str(0)).await?;
            None
        }
        "aiChatsRepo.loadMessages" => to_json(ai_chats::load_messages(st, &a.str(0)).await?),
        "aiChatsRepo.replaceAllMessages" => {
            ai_chats::replace_all_messages(st, &a.str(0), &a.get::<Vec<_>>(1)).await?;
            None
        }

        "vaultStateRepo.load" => to_json(vault_state::load(st).await?),
        "vaultStateRepo.save" => {
            vault_state::save(st, &a.get(0)).await?;
            None
        }
        "vaultStateRepo.reset" => {
            vault_state::reset(st).await?;
            None
        }

        "userCredentialsRepo.load" => {
            to_json(user_credentials::load(st, &a.str(0), &a.str(1)).await?)
        }
        "userCredentialsRepo.save" => {
            user_credentials::save(st, &a.get(0)).await?;
            None
        }
        "userCredentialsRepo.remove" => {
            user_credentials::remove(st, &a.str(0), &a.str(1)).await?;
            None
        }
        "userCredentialsRepo.removeAllForKey" => {
            user_credentials::remove_all_for_key(st, &a.str(0)).await?;
            None
        }

        other => panic!("no Rust call for {other}"),
    })
}

// ---------------------------------------------------------------------------
// Pruning: TypeScript computes it, storage runs it (README, quirk 12)
// ---------------------------------------------------------------------------

/// The recorded result of the next `load` of the same parent after step `i`.
fn next_load<'a>(steps: &'a [Step], i: usize, load: &str, parent: &str) -> &'a [Value] {
    steps[i + 1..]
        .iter()
        .find(|s| {
            s.call.as_deref() == Some(load)
                && serde_json::from_str::<String>(s.args[0].get()).unwrap() == parent
        })
        .and_then(|s| s.result.as_ref())
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("no {load}({parent}) after step {i}"))
        .as_slice()
}

fn ids(versions: &[Value]) -> BTreeSet<String> {
    versions
        .iter()
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect()
}

/// Replays `queryVersionsRepo.pruneOldVersions(queryId, keepCount)` as the
/// split runs it: the versions missing from the next recorded load are
/// deleted, and the survivor whose snapshot went from null to text is
/// promoted with that text.
async fn prune_query_versions(st: &Storage, steps: &[Step], i: usize) -> Result<(), StorageError> {
    let query_id: String = serde_json::from_str(steps[i].args[0].get()).unwrap();
    let before = to_json(query_versions::load_by_query(st, &query_id).await?).unwrap();
    let before = before.as_array().unwrap();
    let after = next_load(steps, i, "queryVersionsRepo.loadByQuery", &query_id);
    let kept = ids(after);
    let delete_ids: Vec<String> = ids(before).difference(&kept).cloned().collect();
    let promote = after.iter().find_map(|v| {
        let was = before.iter().find(|b| b["id"] == v["id"]).unwrap();
        (was["snapshot"].is_null() && !v["snapshot"].is_null()).then(|| QueryVersionPromote {
            id: v["id"].as_str().unwrap().to_string(),
            snapshot: v["snapshot"].as_str().unwrap().to_string(),
        })
    });
    query_versions::prune(
        st,
        &QueryVersionsPrune {
            saved_query_id: query_id,
            delete_ids,
            promote,
        },
    )
    .await
}

async fn prune_dashboard_versions(
    st: &Storage,
    steps: &[Step],
    i: usize,
) -> Result<(), StorageError> {
    let dashboard_id: String = serde_json::from_str(steps[i].args[0].get()).unwrap();
    let before = to_json(dashboard_versions::load_by_dashboard(st, &dashboard_id).await?).unwrap();
    let after = next_load(
        steps,
        i,
        "dashboardVersionsRepo.loadByDashboard",
        &dashboard_id,
    );
    let delete_ids = ids(before.as_array().unwrap())
        .difference(&ids(after))
        .cloned()
        .collect();
    dashboard_versions::prune(
        st,
        &DashboardVersionsPrune {
            dashboard_id,
            delete_ids,
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// What the TypeScript client does to a loaded value before the recorder saw it
// ---------------------------------------------------------------------------

/// `lastConnected` crosses as the stored text, and the client builds a `Date`
/// from it. The recorder wrote that `Date` as `{"$date": toISOString()}`, or
/// `{"$date": null}` for an Invalid Date. This covers the forms the fixtures
/// hold (the recording ran with `TZ=UTC`).
fn js_date(text: &str) -> Value {
    fn digits(s: &str) -> Option<u32> {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    }
    let parse = || -> Option<String> {
        let s = text.strip_suffix('Z').unwrap_or(text);
        let (date, time) = s.split_once(['T', ' ']).unwrap_or((s, "00:00:00"));
        let d: Vec<_> = date.split('-').collect();
        let (y, mo, da) = (digits(d.first()?)?, digits(d.get(1)?)?, digits(d.get(2)?)?);
        let (hms, ms) = time.split_once('.').unwrap_or((time, "0"));
        let t: Vec<_> = hms.split(':').collect();
        let (h, mi, se) = (digits(t.first()?)?, digits(t.get(1)?)?, digits(t.get(2)?)?);
        let ms = digits(&format!("{ms:0<3}")[..3])?;
        (d.len() == 3 && t.len() == 3 && (1..=12).contains(&mo) && (1..=31).contains(&da))
            .then(|| format!("{y:04}-{mo:02}-{da:02}T{h:02}:{mi:02}:{se:02}.{ms:03}Z"))
    };
    json!({ "$date": parse() })
}

/// A value after `fromStorable` and the recorder's `toStorable`, or `None`
/// when `fromStorable` throws (the client drops that saved workflow).
fn storable_round_trip(v: &Value) -> Option<Value> {
    /// What `fromStorable` returns: plain JSON, or a decoded tag.
    enum Js {
        Plain(Value),
        Tag(&'static str, Value),
        Array(Vec<Js>),
        Object(Vec<(String, Js)>),
    }
    fn from_storable(v: &Value) -> Option<Js> {
        Some(match v {
            Value::Array(items) => {
                Js::Array(items.iter().map(from_storable).collect::<Option<_>>()?)
            }
            Value::Object(o) => {
                let kind = o.get("$sq").and_then(Value::as_str);
                match (kind, o.get("v")) {
                    (Some("bigint"), Some(Value::String(s))) => {
                        let n: i128 = s.trim().parse().ok()?;
                        Js::Tag("bigint", json!(n.to_string()))
                    }
                    (Some("float"), Some(f)) if ["NaN", "inf", "-inf"].contains(&f.as_str()?) => {
                        Js::Tag("float", f.clone())
                    }
                    (Some("decimal"), Some(d)) => Js::Tag("decimal", d.clone()),
                    (Some("bytes"), Some(b)) => Js::Tag("bytes", b.clone()),
                    // `json` returns `v` untouched: no recursion into it.
                    (Some("json"), Some(inner)) => Js::Plain(inner.clone()),
                    (Some("bigint" | "float"), Some(_)) => return None,
                    _ => Js::Object(
                        o.iter()
                            .map(|(k, v)| Some((k.clone(), from_storable(v)?)))
                            .collect::<Option<_>>()?,
                    ),
                }
            }
            other => Js::Plain(other.clone()),
        })
    }
    fn to_storable_plain(v: &Value) -> Value {
        match v {
            Value::Array(items) => Value::Array(items.iter().map(to_storable_plain).collect()),
            Value::Object(o) if o.contains_key("$sq") => json!({ "$sq": "json", "v": v }),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), to_storable_plain(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    fn to_storable(v: &Js) -> Value {
        match v {
            Js::Plain(p) => to_storable_plain(p),
            Js::Tag(kind, v) => json!({ "$sq": kind, "v": v }),
            Js::Array(items) => Value::Array(items.iter().map(to_storable).collect()),
            Js::Object(fields) => {
                let object: serde_json::Map<_, _> = fields
                    .iter()
                    .map(|(k, v)| (k.clone(), to_storable(v)))
                    .collect();
                // A plain object with a `$sq` key is wrapped so it can't pass
                // for a tag.
                if object.contains_key("$sq") {
                    json!({ "$sq": "json", "v": object })
                } else {
                    Value::Object(object)
                }
            }
        }
    }
    from_storable(v).map(|js| to_storable(&js))
}

/// The loaded value as the TypeScript client hands it on.
fn client_view(call: &str, mut v: Value) -> Value {
    match call {
        "connectionsRepo.loadAll" => {
            for c in v.as_array_mut().unwrap() {
                if let Some(Value::String(text)) = c.get("lastConnected") {
                    let date = js_date(text);
                    c["lastConnected"] = date;
                }
            }
        }
        "projectStateRepo.load" if !v.is_null() => {
            let workflows = v["savedWorkflows"].as_array().unwrap();
            v["savedWorkflows"] = workflows.iter().filter_map(storable_round_trip).collect();
        }
        _ => {}
    }
    v
}

// ---------------------------------------------------------------------------
// Stored rows
// ---------------------------------------------------------------------------

fn cell(v: SqliteValueRef<'_>) -> (Value, String) {
    // A NULL's type_info is the column's declared type.
    if v.is_null() {
        return (Value::Null, "null".to_string());
    }
    let kind = v.type_info().name().to_ascii_lowercase();
    let value = match kind.as_str() {
        "integer" => json!(<i64 as Decode<sqlx::Sqlite>>::decode(v).unwrap()),
        "real" => json!(<f64 as Decode<sqlx::Sqlite>>::decode(v).unwrap()),
        "text" => json!(<String as Decode<sqlx::Sqlite>>::decode(v).unwrap()),
        other => panic!("unexpected storage class {other}"),
    };
    (value, kind)
}

/// Stored values match when equal, numbers by value (a REAL 12.0 is recorded
/// as `12`), and a masked workflow id matches any `workflow-<uuid v4>`.
fn stored_eq(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(a), Value::String(b)) if b == MASKED_WORKFLOW_ID => a
            .strip_prefix("workflow-")
            .and_then(|id| uuid::Uuid::parse_str(id).ok())
            .is_some_and(|id| id.get_version_num() == 4),
        _ => actual == expected,
    }
}

async fn check_rows(st: &Storage, case: &str, tables: &serde_json::Map<String, Value>) {
    for (table, spec) in tables {
        let spec: TableRows = serde_json::from_value(spec.clone()).unwrap();
        let n = spec.columns.len();
        let order: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
        let sql = format!(
            "SELECT {} FROM {table} ORDER BY {}",
            spec.columns.join(", "),
            order.join(", ")
        );
        let rows = sqlx::query(&sql).fetch_all(st.pool()).await.unwrap();
        let (mut values, mut types) = (Vec::new(), Vec::new());
        for row in &rows {
            let cells: Vec<_> = (0..n).map(|i| cell(row.try_get_raw(i).unwrap())).collect();
            values.push(cells.iter().map(|c| c.0.clone()).collect::<Vec<_>>());
            types.push(cells.into_iter().map(|c| c.1).collect::<Vec<_>>());
        }
        assert_eq!(types, spec.types, "{case}: storage classes in {table}");
        let same = values.len() == spec.rows.len()
            && values
                .iter()
                .zip(&spec.rows)
                .all(|(a, e)| a.len() == e.len() && a.iter().zip(e).all(|(a, e)| stored_eq(a, e)));
        assert!(
            same,
            "{case}: rows in {table}\n  got:      {}\n  expected: {}",
            json!(values),
            json!(spec.rows)
        );
    }
}

// ---------------------------------------------------------------------------
// The replay
// ---------------------------------------------------------------------------

async fn open(dir: &Path) -> Storage {
    Storage::open(dir.join("seaquel.db"), StorageOptions::default())
        .await
        .unwrap()
}

/// A recorded error names its category: better-sqlite3's `code` for a
/// constraint, and no code for the vault's bare `JSON.parse`, which storage
/// reports as a value it can't decode.
fn assert_error_kind(what: &str, e: &StorageError, expected: &Value) {
    use sqlx::error::ErrorKind;
    let kind = match expected.get("code").and_then(Value::as_str) {
        Some("SQLITE_CONSTRAINT_FOREIGNKEY") => Some(ErrorKind::ForeignKeyViolation),
        Some("SQLITE_CONSTRAINT_UNIQUE" | "SQLITE_CONSTRAINT_PRIMARYKEY") => {
            Some(ErrorKind::UniqueViolation)
        }
        Some("SQLITE_CONSTRAINT_CHECK") => Some(ErrorKind::CheckViolation),
        Some("SQLITE_CONSTRAINT_NOTNULL") => Some(ErrorKind::NotNullViolation),
        Some(other) => panic!("{what}: no mapping for {other}"),
        None => None,
    };
    match (kind, e) {
        (Some(kind), StorageError::Sqlx(sqlx::Error::Database(db))) if db.kind() == kind => {}
        (None, StorageError::Sqlx(sqlx::Error::Decode(_))) => {}
        _ => panic!("{what}: failed with {e:?}, expected {expected}"),
    }
}

async fn replay_case(case: &Case) {
    let dir = tempfile::tempdir().unwrap();
    let st = open(dir.path()).await;
    for (i, step) in case.steps.iter().enumerate() {
        let what = format!("{} step {i}", case.name);
        if let Some(sql) = &step.sql {
            sqlx::raw_sql(sql).execute(st.pool()).await.unwrap();
            continue;
        }
        let name = step.call.as_deref().expect("a call or sql step");
        let args = Args(step.args.iter().map(|a| wire_text(a)).collect());
        let outcome = match name {
            "queryVersionsRepo.pruneOldVersions" => prune_query_versions(&st, &case.steps, i)
                .await
                .map(|()| None),
            "dashboardVersionsRepo.pruneOldVersions" => {
                prune_dashboard_versions(&st, &case.steps, i)
                    .await
                    .map(|()| None)
            }
            _ => call(&st, name, &args).await,
        };
        match (outcome, &step.error, &step.result) {
            (Err(e), Some(expected), _) => assert_error_kind(&what, &e, expected),
            (Err(e), None, _) => panic!("{what}: {name} failed: {e}"),
            (Ok(_), Some(err), _) => panic!("{what}: {name} succeeded, expected {err}"),
            (Ok(got), None, expected) => {
                let got = got.map(|v| client_view(name, v));
                assert_eq!(
                    got.as_ref(),
                    expected.as_ref(),
                    "{what}: {name} returned\n  {}\nexpected\n  {}",
                    json!(got),
                    json!(expected)
                );
            }
        }
    }
    check_rows(&st, &case.name, &case.rows).await;
    st.close().await;
}

async fn replay(file: &str) {
    let fixture: FixtureFile =
        serde_json::from_str(&common::fixture(&format!("repos/{file}"))).unwrap();
    assert!(!fixture.cases.is_empty());
    for case in &fixture.cases {
        replay_case(case).await;
    }
}

macro_rules! replay_tests {
    ($($test:ident => $file:literal,)*) => {$(
        #[tokio::test]
        async fn $test() {
            replay($file).await;
        }
    )*};
}

replay_tests! {
    replays_ai_chats => "ai-chats.json",
    replays_app_state => "app-state.json",
    replays_connection_overrides => "connection-overrides.json",
    replays_connections => "connections.json",
    replays_dashboard_versions => "dashboard-versions.json",
    replays_dashboards => "dashboards.json",
    replays_import_state => "import-state.json",
    replays_license => "license.json",
    replays_onboarding => "onboarding.json",
    replays_project_state => "project-state.json",
    replays_projects => "projects.json",
    replays_query_history => "query-history.json",
    replays_query_versions => "query-versions.json",
    replays_saved_queries => "saved-queries.json",
    replays_shared_repos => "shared-repos.json",
    replays_themes => "themes.json",
    replays_tutorial => "tutorial.json",
    replays_user_credentials => "user-credentials.json",
    replays_vault_state => "vault-state.json",
}

/// Every fixture file has a test above, and every case in them is 76.
#[test]
fn every_fixture_file_is_replayed() {
    let dir = common::fixture_path("repos");
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    let this = include_str!("repos.rs");
    let mut cases = 0;
    for f in &files {
        assert!(this.contains(&format!("=> \"{f}\"")), "{f} isn't replayed");
        let fixture: FixtureFile =
            serde_json::from_str(&common::fixture(&format!("repos/{f}"))).unwrap();
        cases += fixture.cases.len();
    }
    assert_eq!((files.len(), cases), (19, 76));
}

// ---------------------------------------------------------------------------
// Pruning edge cases the fixtures don't reach
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_prune_only_touches_its_own_parent() {
    let dir = tempfile::tempdir().unwrap();
    let st = open(dir.path()).await;
    let project = r#"{"id":"p","name":"P","createdAt":"c","updatedAt":"u","customLabels":[]}"#;
    projects::save(&st, &serde_json::from_str(project).unwrap())
        .await
        .unwrap();
    let query = |id: &str| {
        serde_json::from_value(json!({"id": id, "name": id, "query": "x", "projectId": "p",
            "createdAt": "c", "updatedAt": "u"}))
        .unwrap()
    };
    saved_queries::save_all(&st, "p", &[query("a"), query("b")])
        .await
        .unwrap();
    for (id, q) in [("a1", "a"), ("b1", "b")] {
        let v = json!({"id": id, "queryId": q, "version": 1, "snapshot": null, "diff": "d",
            "createdAt": "c"});
        query_versions::insert(&st, &serde_json::from_value(v).unwrap())
            .await
            .unwrap();
    }
    // An id of another query is neither deleted nor promoted.
    query_versions::prune(
        &st,
        &QueryVersionsPrune {
            saved_query_id: "a".into(),
            delete_ids: vec!["b1".into()],
            promote: Some(QueryVersionPromote {
                id: "b1".into(),
                snapshot: "text".into(),
            }),
        },
    )
    .await
    .unwrap();
    let b = query_versions::load_by_query(&st, "b").await.unwrap();
    assert_eq!((b.len(), b[0].snapshot.as_deref()), (1, None));
    // A promotion that fails rolls the prune's deletes back.
    let v = json!({"id": "a2", "queryId": "a", "version": 2, "snapshot": null, "diff": "d",
        "createdAt": "c"});
    query_versions::insert(&st, &serde_json::from_value(v).unwrap())
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TRIGGER no_promotion BEFORE UPDATE ON query_versions \
         BEGIN SELECT RAISE(ABORT, 'no promotion'); END",
    )
    .execute(st.pool())
    .await
    .unwrap();
    let failed = query_versions::prune(
        &st,
        &QueryVersionsPrune {
            saved_query_id: "a".into(),
            delete_ids: vec!["a1".into()],
            promote: Some(QueryVersionPromote {
                id: "a2".into(),
                snapshot: "text".into(),
            }),
        },
    )
    .await;
    assert!(failed.is_err());
    let a = query_versions::load_by_query(&st, "a").await.unwrap();
    let ids: Vec<_> = a.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(ids, ["a1", "a2"]);
    assert_eq!(a[1].snapshot, None);
    st.close().await;
}

// ---------------------------------------------------------------------------
// Password stripping (Decision 13.1)
// ---------------------------------------------------------------------------

fn connection(id: &str, connection_string: &str) -> seaquel_types::storage::PersistedConnection {
    serde_json::from_value(json!({
        "id": id, "name": id, "type": "mssql", "host": "h", "port": 1433,
        "databaseName": "d", "username": "u", "projectId": "p", "labelIds": [],
        "connectionString": connection_string,
    }))
    .unwrap()
}

async fn stored_connection_strings(st: &Storage) -> Vec<(String, Option<String>)> {
    sqlx::query_as("SELECT id, connection_string FROM connections ORDER BY id")
        .fetch_all(st.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn save_strips_passwords_in_both_formats() {
    let dir = tempfile::tempdir().unwrap();
    let st = open(dir.path()).await;
    let project = r#"{"id":"p","name":"P","createdAt":"c","updatedAt":"u","customLabels":[]}"#;
    projects::save(&st, &serde_json::from_str(project).unwrap())
        .await
        .unwrap();

    let cases = [
        (
            "ado",
            "Server=tcp:h,1433;Database=d;User Id=sa;Password=hunter2;Encrypt=true",
            Some("Server=tcp:h,1433;Database=d;User Id=sa;Encrypt=true"),
        ),
        (
            "pg-url",
            "postgres://me:hunter2@db:5432/app",
            Some("postgresql://me@db:5432/app"),
        ),
        (
            "mysql-url",
            "mysql://root:hunter2@db/shop?ssl-mode=REQUIRED",
            Some("mysql://root@db/shop?ssl-mode=REQUIRED"),
        ),
        (
            "clean-url",
            "mysql://админ@db.example.com:3306/données",
            Some("mysql://админ@db.example.com:3306/données"),
        ),
        ("sqlite", "sqlite:///tmp/a.db", Some("sqlite:///tmp/a.db")),
    ];
    for (id, input, _) in &cases {
        connections::save(&st, &connection(id, input))
            .await
            .unwrap();
    }
    let stored = stored_connection_strings(&st).await;
    for (id, _, expected) in &cases {
        let got = stored.iter().find(|(i, _)| i == id).unwrap();
        assert_eq!(got.1.as_deref(), *expected, "{id}");
    }
    let loaded = connections::load_all(&st).await.unwrap();
    assert!(loaded.iter().all(|c| !c
        .connection_string
        .as_deref()
        .unwrap_or("")
        .contains("hunter2")));

    // A row an older build left with a password is cleaned when it's next
    // saved.
    sqlx::query(
        "UPDATE connections SET connection_string = 'Data Source=h;PWD={a;b}' WHERE id = 'ado'",
    )
    .execute(st.pool())
    .await
    .unwrap();
    let mut old = connections::load_all(&st)
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == "ado")
        .unwrap();
    old.name = "renamed".into();
    connections::save(&st, &old).await.unwrap();
    let stored = stored_connection_strings(&st).await;
    let ado = stored.iter().find(|(i, _)| i == "ado").unwrap();
    assert_eq!(ado.1.as_deref(), Some("Data Source=h"));
    st.close().await;
}

/// Strings that key=value stripping handles, with what's left of them.
const KEY_VALUE_CASES: &[(&str, &str)] = &[
    ("Server=h;Password=secret;", "Server=h;"),
    ("Server=h;Password=secret", "Server=h"),
    ("Password=secret;Server=h", "Server=h"),
    ("Password=secret", ""),
    (
        "Server=h;password=a;PWD=b;pwd=c;Database=d",
        "Server=h;Database=d",
    ),
    (
        "Server=h; Password = secret ;Database=d",
        "Server=h;Database=d",
    ),
    ("Server=h;\tpAsSwOrD=x", "Server=h"),
    (
        "Server=h;Password='se;cret';Database=d",
        "Server=h;Database=d",
    ),
    (
        "Server=h;Password=\"se;cr\"\"et\";Database=d",
        "Server=h;Database=d",
    ),
    (
        "Server=h;Password='it''s;x';Database=d",
        "Server=h;Database=d",
    ),
    (
        "Driver={ODBC Driver 18};Server=h;PWD={p;w}}d};UID=sa",
        "Driver={ODBC Driver 18};Server=h;UID=sa",
    ),
    (
        "Server=h;Password=  'quoted' trailing;Database=d",
        "Server=h;Database=d",
    ),
    ("Server=h;Password=it's;Database=d", "Server=h;Database=d"),
    (
        "Server=h;Application Name='a;Password=x';Database=d",
        "Server=h;Application Name='a;Password=x';Database=d",
    ),
    (
        "Server=h;PasswordHint=x;Pwd2=y",
        "Server=h;PasswordHint=x;Pwd2=y",
    ),
    ("Server=h;;Password=x;;", "Server=h;;;"),
    (
        "jdbc:sqlserver://h:1433;databaseName=d;password=secret",
        "jdbc:sqlserver://h:1433;databaseName=d",
    ),
    ("Server=h;Password='unterminated;Database=d", "Server=h"),
    ("Server=h;Database=d", "Server=h;Database=d"),
];

#[test]
fn key_value_strings_lose_their_password() {
    for (input, expected) in KEY_VALUE_CASES {
        assert_eq!(
            strip_connection_string_password(input),
            *expected,
            "{input}"
        );
    }
}

/// `new URL`'s output for these strings, from running today's
/// `stripPasswordFromConnectionString` in Node (22). Strings without a
/// password are left alone rather than normalised (see the function's doc).
const URL_CASES: &[(&str, &str)] = &[
    (
        "postgres://user:secret@localhost:5432/app",
        "postgresql://user@localhost:5432/app",
    ),
    (
        "postgresql://user:secret@localhost:5432/app",
        "postgresql://user@localhost:5432/app",
    ),
    (
        "postgresql://user:secret@localhost:5432/app?sslmode=require",
        "postgresql://user@localhost:5432/app?sslmode=require",
    ),
    (
        "mysql://root:p%40ss@db.example.com:3306/shop",
        "mysql://root@db.example.com:3306/shop",
    ),
    (
        "mysql://root:p@ss@db.example.com/shop",
        "mysql://root@db.example.com/shop",
    ),
    ("mariadb://u:pw@h/db", "mariadb://u@h/db"),
    ("postgres://:onlypw@host/db", "postgresql://host/db"),
    (
        "mysql://админ:пароль@db.example.com:3306/données",
        "mysql://%D0%B0%D0%B4%D0%BC%D0%B8%D0%BD@db.example.com:3306/donn%C3%A9es",
    ),
    (
        "POSTGRES://User:Secret@HOST:5432/App",
        "postgresql://User@HOST:5432/App",
    ),
    (
        "postgres://u:s@[::1]:5432/db",
        "postgresql://u@[::1]:5432/db",
    ),
    (
        "sqlserver://sa:Secret1@localhost:1433/master",
        "sqlserver://sa@localhost:1433/master",
    ),
    (
        "mssql://sa:Secret1@localhost:1433?encrypt=true",
        "mssql://sa@localhost:1433?encrypt=true",
    ),
    ("postgres://u:p%20w@h/db#frag", "postgresql://u@h/db#frag"),
    ("http://u:p@example.com", "http://u@example.com/"),
    ("postgres://u:p@h:5432", "postgresql://u@h:5432"),
    // A `;` in the user info is part of the password; the URL stays whole.
    ("postgres://u:p;password=x@h/db", "postgresql://u@h/db"),
    // No password: unchanged (Node would normalise these).
    ("postgres://user:@host/db", "postgres://user:@host/db"),
    ("postgres://user@host/db", "postgres://user@host/db"),
    (
        "mysql://админ@db.example.com:3306/données",
        "mysql://админ@db.example.com:3306/données",
    ),
    ("duckdb:///tmp/x.duckdb", "duckdb:///tmp/x.duckdb"),
    ("not a url", "not a url"),
    ("", ""),
    // SQLite strings are never touched.
    ("sqlite:///tmp/a.db", "sqlite:///tmp/a.db"),
    ("sqlite::memory:", "sqlite::memory:"),
    ("sqlite:x.db;Password=y", "sqlite:x.db;Password=y"),
];

#[test]
fn url_strings_match_the_typescript() {
    for (input, expected) in URL_CASES {
        assert_eq!(
            strip_connection_string_password(input),
            *expected,
            "{input}"
        );
    }
}

/// URLs with a `password` or `pwd` query parameter (any case, the name
/// percent-decoded), which libpq and MySQL both accept.
const QUERY_PARAMETER_CASES: &[(&str, &str)] = &[
    ("postgres://u@h/db?password=secret", "postgresql://u@h/db"),
    (
        "mysql://u@h/db?sslmode=require&password=s",
        "mysql://u@h/db?sslmode=require",
    ),
    (
        "postgres://u@h/db?PWD=x&application_name=a",
        "postgresql://u@h/db?application_name=a",
    ),
    (
        "postgres://u:p@h/db?pass%77ord=x&a=1",
        "postgresql://u@h/db?a=1",
    ),
    ("mysql://u@h/db?Password=a&pwd=b", "mysql://u@h/db"),
    // In a URL with an authority, pairs after the authority go too.
    ("postgres://u:p@h/db;Password=x", "postgresql://u@h/db"),
    (
        "postgres://u@h/db?sslmode=require;pwd=x",
        "postgres://u@h/db?sslmode=require",
    ),
    (
        "mysql://u@h/db?passwordless=1&a=password",
        "mysql://u@h/db?passwordless=1&a=password",
    ),
];

#[test]
fn url_query_passwords_are_removed() {
    for (input, expected) in QUERY_PARAMETER_CASES {
        assert_eq!(
            strip_connection_string_password(input),
            *expected,
            "{input}"
        );
    }
}

/// Stripping twice gives what stripping once gave, including the two
/// strings the review's fuzzer found, where removing a pair exposed a URL
/// password, and a sweep of generated strings.
#[test]
fn stripping_is_idempotent() {
    let mut inputs: Vec<String> = [
        "PWD=🚀?;postgresql://u:p@h\"postgres://",
        "password==;passwordpostgresql://u:p@h#",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (input, _) in KEY_VALUE_CASES
        .iter()
        .chain(URL_CASES)
        .chain(QUERY_PARAMETER_CASES)
    {
        inputs.push(input.to_string());
    }
    let atoms = [
        "a",
        "=",
        ";",
        "'",
        "\"",
        "{",
        "}",
        " ",
        "password",
        "PWD",
        "é",
        "🚀",
        "x",
        "sqlite:",
        ":",
        "@",
        "/",
        "postgres://",
        "postgresql://",
        "mysql://",
        "u:p@h",
        "?",
        "#",
        "%",
        "[",
        "]",
        "&",
    ];
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..50_000 {
        let n = (next() % 12) as usize;
        inputs.push(
            (0..n)
                .map(|_| atoms[(next() % atoms.len() as u64) as usize])
                .collect(),
        );
    }
    for input in &inputs {
        let once = strip_connection_string_password(input);
        assert_eq!(strip_connection_string_password(&once), once, "{input:?}");
    }
}

/// A file with today's schema, the way the TypeScript left it, holding the
/// project `p` and a connection per `(id, connection_string)`; `None` as the
/// id stores NULL.
async fn old_file(path: &Path, rows: &[(Option<String>, Option<String>)]) {
    common::load_fixture(path, "schemas/current.sql").await;
    let mut conn = common::raw_connect(path).await;
    let mut tx = sqlx::Connection::begin(&mut conn).await.unwrap();
    sqlx::query(
        "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'P', 'c', 'u')",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    for (id, cs) in rows {
        sqlx::query(
            "INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
             username, connection_string) VALUES (?, 'p', 'n', 'mssql', 'h', 1, 'd', 'u', ?)",
        )
        .bind(id)
        .bind(cs)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    sqlx::Connection::close(conn).await.unwrap();
}

async fn strings_by_rowid(st: &Storage) -> Vec<Option<String>> {
    sqlx::query_scalar("SELECT connection_string FROM connections ORDER BY rowid")
        .fetch_all(st.pool())
        .await
        .unwrap()
}

/// The `strip_connection_string_passwords` data step cleans the rows
/// already on disk the first time the Rust build opens a file: each comes
/// out as `strip_connection_string_password` leaves it, a NULL id included,
/// and strings without a password keep their exact text.
#[tokio::test]
async fn data_step_cleans_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");

    let mut cases: Vec<(String, String)> = KEY_VALUE_CASES
        .iter()
        .chain(QUERY_PARAMETER_CASES)
        .map(|(input, expected)| (input.to_string(), expected.to_string()))
        .collect();
    for s in [
        "mysql://root@db/shop?ssl-mode=REQUIRED",
        "mysql://админ@db.example.com:3306/données",
        "postgres://user@host/db?application_name=Password",
        "sqlite:x.db;Password=y",
        "",
    ] {
        cases.push((s.to_string(), s.to_string()));
    }
    cases.push((
        "postgres://me:hunter2@db:5432/app".into(),
        "postgresql://me@db:5432/app".into(),
    ));
    let mut rows: Vec<(Option<String>, Option<String>)> = cases
        .iter()
        .enumerate()
        .map(|(i, (input, _))| (Some(format!("c{i:03}")), Some(input.clone())))
        .collect();
    rows.push((None, Some("Server=h;Password=x".into())));
    rows.push((Some("null".into()), None));
    old_file(&path, &rows).await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let stored = strings_by_rowid(&st).await;
    for ((input, expected), got) in cases.iter().zip(&stored) {
        assert_eq!(got.as_deref(), Some(expected.as_str()), "{input}");
        assert_eq!(
            strip_connection_string_password(input),
            *expected,
            "{input}"
        );
    }
    assert_eq!(stored[cases.len()].as_deref(), Some("Server=h"), "NULL id");
    assert_eq!(stored[cases.len() + 1], None);
    st.close().await;
}

/// The step runs once per file: it's recorded, and a password written after
/// it (by hand, since save strips) is still there on the next open.
#[tokio::test]
async fn data_step_runs_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    old_file(
        &path,
        &[(Some("c".into()), Some("Server=h;Password=x".into()))],
    )
    .await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert_eq!(strings_by_rowid(&st).await, [Some("Server=h".to_string())]);
    let steps: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT name FROM {DATA_STEPS_TABLE} WHERE applied_at IS NOT NULL"
    ))
    .fetch_all(st.pool())
    .await
    .unwrap();
    assert_eq!(steps, ["strip_connection_string_passwords"]);
    sqlx::query("UPDATE connections SET connection_string = 'Password=x'")
        .execute(st.pool())
        .await
        .unwrap();
    st.close().await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert_eq!(
        strings_by_rowid(&st).await,
        [Some("Password=x".to_string())]
    );
    st.close().await;
}

/// The step is linear: 10,000 rows and one 200 KB string take well under
/// the time the review's CTE version needed for the long string alone
/// (280 s).
// A native test measuring its own wall time; the wasm32 rule behind
// `disallowed_types` doesn't apply.
#[allow(clippy::disallowed_types)]
#[tokio::test]
async fn data_step_is_fast() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    let long = format!(
        "Server=h;Application Name={};Password=secret;Database=d",
        "x".repeat(200_000)
    );
    let mut rows: Vec<(Option<String>, Option<String>)> = (0..10_000)
        .map(|i| {
            let cs = format!("Server=h{i};Application Name=app;Password=secret{i};Database=d");
            (Some(format!("c{i:05}")), Some(cs))
        })
        .collect();
    rows.push((Some("long".into()), Some(long.clone())));
    old_file(&path, &rows).await;

    let started = std::time::Instant::now();
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let elapsed = started.elapsed();
    let stored = strings_by_rowid(&st).await;
    assert!(stored
        .iter()
        .all(|s| !s.as_deref().unwrap().contains("secret")));
    assert_eq!(
        stored[10_000].as_deref(),
        Some(strip_connection_string_password(&long).as_str())
    );
    assert!(elapsed.as_secs() < 10, "open took {elapsed:?}");
    st.close().await;
}

/// A step that fails doesn't stop `open`: it's rolled back and not
/// recorded, and the next open (here, with the cause gone) runs it.
#[tokio::test]
async fn a_failed_data_step_is_retried_on_the_next_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    old_file(
        &path,
        &[
            (Some("a".into()), Some("Server=h;Password=x".into())),
            (Some("b".into()), Some("Server=h;Pwd=y".into())),
        ],
    )
    .await;
    common::exec_file(
        &path,
        "CREATE TRIGGER no_cleanup BEFORE UPDATE ON connections WHEN new.id = 'b' \
         BEGIN SELECT RAISE(ABORT, 'no cleanup'); END;",
    )
    .await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    // Row `a` was updated before `b` failed; the rollback undid it.
    assert_eq!(
        strings_by_rowid(&st).await,
        [
            Some("Server=h;Password=x".to_string()),
            Some("Server=h;Pwd=y".to_string())
        ]
    );
    let recorded: Vec<String> = sqlx::query_scalar(&format!("SELECT name FROM {DATA_STEPS_TABLE}"))
        .fetch_all(st.pool())
        .await
        .unwrap_or_default();
    assert!(recorded.is_empty(), "{recorded:?}");
    sqlx::query("DROP TRIGGER no_cleanup")
        .execute(st.pool())
        .await
        .unwrap();
    st.close().await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert_eq!(
        strings_by_rowid(&st).await,
        [Some("Server=h".to_string()), Some("Server=h".to_string())]
    );
    st.close().await;
}

/// Edge bytes: a NUL inside a value is kept (the pairs around it are cut as
/// usual), and TEXT that isn't valid UTF-8 is skipped, byte for byte.
#[tokio::test]
async fn data_step_keeps_nul_bytes_and_skips_invalid_utf8() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    old_file(
        &path,
        &[(
            Some("nul".into()),
            Some("Server=h\0x;Password=y\0z;Db=\0".into()),
        )],
    )
    .await;
    // `Server=h;Password=` then 0xFF 0xFE: invalid UTF-8, stored as TEXT.
    common::exec_file(
        &path,
        "INSERT INTO connections (id, project_id, name, type, host, port, database_name, \
         username, connection_string) VALUES ('bad', 'p', 'n', 'mssql', 'h', 1, 'd', 'u', \
         CAST(x'5365727665723d683b50617373776f72643dfffe' AS TEXT));",
    )
    .await;

    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, typeof(connection_string), hex(connection_string) FROM connections \
         ORDER BY rowid",
    )
    .fetch_all(st.pool())
    .await
    .unwrap();
    let hex = |s: &str| s.bytes().map(|b| format!("{b:02X}")).collect::<String>();
    assert_eq!(
        rows,
        [
            (
                "nul".to_string(),
                "text".to_string(),
                hex("Server=h\0x;Db=\0")
            ),
            (
                "bad".to_string(),
                "text".to_string(),
                "5365727665723D683B50617373776F72643DFFFE".to_string()
            ),
        ]
    );
    st.close().await;
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// A label id listed twice is saved once instead of failing the primary key
/// and rolling the whole save back.
#[tokio::test]
async fn repeated_label_ids_are_saved_once() {
    let dir = tempfile::tempdir().unwrap();
    let st = open(dir.path()).await;
    let project = r#"{"id":"p","name":"P","createdAt":"c","updatedAt":"u","customLabels":[]}"#;
    projects::save(&st, &serde_json::from_str(project).unwrap())
        .await
        .unwrap();
    let mut c = connection("c", "Server=h");
    c.label_ids = vec!["b".into(), "a".into(), "b".into()];
    connections::save(&st, &c).await.unwrap();
    let loaded = connections::load_all(&st).await.unwrap();
    // Loaded in the primary key's order, as always.
    assert_eq!(loaded[0].label_ids, ["a", "b"]);
    let inserted: Vec<String> =
        sqlx::query_scalar("SELECT label_id FROM connection_labels ORDER BY rowid")
            .fetch_all(st.pool())
            .await
            .unwrap();
    assert_eq!(inserted, ["b", "a"]);
    st.close().await;
}
