//! Importing connections from TablePlus and DBeaver (phase 5e):
//! the other tool's entries to import candidates, and the duplicate check.
//!
//! A port of `services/tableplus-import.ts`, `services/dbeaver-import.ts` and
//! `services/connection-import.ts`, pinned by `tests/fixtures/imports`. Pure:
//! no I/O and no panics on any input, and it builds for wasm32. Core finds and
//! reads the files (TablePlus's plist decoded to JSON with the `plist` crate,
//! as `src-tauri` did) and hands the result here.
//!
//! What changed from the TypeScript (`changes.json`):
//! - Every candidate has a `key` the create call names it by: TablePlus's
//!   `id:<ID>`, or `pos:<n>` (its place in the plist's list) for an entry with
//!   no `ID` or one whose `ID` another entry has; DBeaver's connection key.
//! - `duplicateOf` names the saved connection with the same type, host,
//!   port, database and user, in place of `isDuplicate`/`selected`.
//! - `problem`: `invalidPort` (port 0 on the wire) for a port that isn't a
//!   number or is outside 0–65535, and for TablePlus `noId` and
//!   `duplicateId`, which come first. Those entries used to be dropped or
//!   imported with `NaN`. Then two that `connectionCreate` would refuse:
//!   TablePlus `invalidSshPort` (tunnel port 0 on the wire) and DBeaver
//!   `noName` (a missing or blank name).
//! - A file that doesn't parse is an [`ImportError`] instead of an empty
//!   list.
//!
//! Everything else is today's mapping, with JavaScript's coercions where the
//! TypeScript relied on them: `String()` of plist numbers and booleans,
//! `Number()` of `tLSMode`, `parseInt` of ports, JavaScript truthiness for
//! DBeaver's defaults, and `Object.entries` order for DBeaver's connections.

use std::collections::HashMap;
use std::fmt;

use seaquel_types::names::is_js_space;
use seaquel_types::storage::{PersistedConnection, SshTunnelConfig};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;

/// Which tool's file an import reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ImportSource {
    Tableplus,
    Dbeaver,
}

/// Why a candidate can't be imported. `importsCreate` refuses its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ImportProblem {
    /// The port isn't a number (`parseInt` gave `NaN`) or is outside
    /// 0–65535. The candidate's `port` is 0.
    InvalidPort,
    /// TablePlus: the entry has no `ID` (or one whose text is empty).
    NoId,
    /// TablePlus: another entry has the same `ID`.
    DuplicateId,
    /// TablePlus: the SSH tunnel's port isn't a whole number from 0 to
    /// 65535 (`parseInt` keeps `-5` or `70000`). The tunnel's `port` is 0.
    InvalidSshPort,
    /// DBeaver: the connection has no name, or only whitespace.
    NoName,
}

impl ImportProblem {
    /// The wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPort => "invalidPort",
            Self::NoId => "noId",
            Self::DuplicateId => "duplicateId",
            Self::InvalidSshPort => "invalidSshPort",
            Self::NoName => "noName",
        }
    }
}

/// One connection found in the other tool's file, as the import dialog
/// lists it.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ImportCandidate {
    /// What `importsCreate` names it by: TablePlus `id:<ID>` or `pos:<n>`,
    /// DBeaver the connection's key.
    pub key: String,
    pub name: String,
    #[serde(rename = "type")]
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"postgres\" | \"mysql\" | \"sqlite\" | \"mariadb\" | \"mssql\" | \"duckdb\"")
    )]
    pub ty: String,
    pub host: String,
    /// 0 with [`ImportProblem::InvalidPort`].
    pub port: u16,
    pub database_name: String,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssl_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_tunnel: Option<SshTunnelConfig>,
    /// The project's saved connection with the same type, host, port,
    /// database and user ([`mark_duplicates`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<ImportProblem>,
    /// The port parsed and was in range. An invalid port is 0 on the wire
    /// but matches no saved connection, as today's `NaN` matched none, even
    /// when an id problem is the one reported.
    #[serde(skip)]
    #[cfg_attr(feature = "ts", ts(skip))]
    port_ok: bool,
}

/// Type, port and which parts are present: never the key, name, host,
/// database or user.
impl fmt::Debug for ImportCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportCandidate")
            .field("ty", &self.ty)
            .field("port", &self.port)
            .field("ssl_mode", &self.ssl_mode.is_some())
            .field("ssh_tunnel", &self.ssh_tunnel.is_some())
            .field("duplicate", &self.duplicate_of.is_some())
            .field("problem", &self.problem)
            .finish()
    }
}

/// `importsCandidates`' answer: `{found: false}` when there's no file,
/// `{found: true, unreadable}` when it can't be read or parsed (a message
/// naming no path), else `{found: true, candidates}`.
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ImportCandidates {
    pub found: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unreadable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<ImportCandidate>>,
}

impl fmt::Debug for ImportCandidates {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportCandidates")
            .field("found", &self.found)
            .field("unreadable", &self.unreadable.is_some())
            .field("candidates", &self.candidates.as_ref().map(Vec::len))
            .finish()
    }
}

impl ImportCandidates {
    /// No file at the place looked in.
    pub fn not_found() -> Self {
        Self {
            found: false,
            unreadable: None,
            candidates: None,
        }
    }

    /// The file is there but can't be read. `message` must name no path.
    pub fn unreadable(message: impl Into<String>) -> Self {
        Self {
            found: true,
            unreadable: Some(message.into()),
            candidates: None,
        }
    }

    /// A reader's result: the candidates, or the error's message.
    pub fn from_result(result: Result<Vec<ImportCandidate>, ImportError>) -> Self {
        match result {
            Ok(candidates) => Self {
                found: true,
                unreadable: None,
                candidates: Some(candidates),
            },
            Err(e) => Self::unreadable(e.to_string()),
        }
    }
}

/// Why a file that was found can't be read as the tool's connections. The
/// messages name no path and quote nothing from the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// DBeaver: not JSON (an empty file included).
    NotJson,
    /// TablePlus: the plist isn't a list of connections.
    NotAList,
    /// DBeaver: JSON, but not an object.
    NotAnObject,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotJson => "The file isn't valid JSON.",
            Self::NotAList => "The file isn't a list of TablePlus connections.",
            Self::NotAnObject => "The file isn't a DBeaver data sources file.",
        })
    }
}

impl std::error::Error for ImportError {}

/// A saved connection as the duplicate check sees it (`ConnectionIdentity`
/// in the TypeScript, plus its id).
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionIdentity {
    pub id: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub host: String,
    pub port: f64,
    pub database_name: String,
    pub username: String,
}

/// The id and type only.
impl fmt::Debug for ConnectionIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionIdentity")
            .field("id", &self.id)
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

impl ConnectionIdentity {
    /// A stored connection.
    pub fn of(c: &PersistedConnection) -> Self {
        Self {
            id: c.id.clone(),
            ty: c.ty.clone(),
            host: c.host.clone(),
            port: c.port,
            database_name: c.database_name.clone(),
            username: c.username.clone(),
        }
    }

    /// A candidate just imported as `id`, so the next ones in the same call
    /// are checked against it too (the TypeScript re-read the page's
    /// connections before each create).
    pub fn of_candidate(id: impl Into<String>, c: &ImportCandidate) -> Self {
        Self {
            id: id.into(),
            ty: c.ty.clone(),
            host: c.host.clone(),
            port: if c.port_ok {
                f64::from(c.port)
            } else {
                f64::NAN
            },
            database_name: c.database_name.clone(),
            username: c.username.clone(),
        }
    }
}

/// Sets each candidate's `duplicateOf` to the first of `existing` with the
/// same type, host, port, database and user (`isAlreadySaved`), and clears
/// it otherwise. A candidate whose port was invalid matches nothing.
pub fn mark_duplicates(candidates: &mut [ImportCandidate], existing: &[ConnectionIdentity]) {
    for c in candidates {
        c.duplicate_of = if c.port_ok {
            existing
                .iter()
                .find(|e| {
                    e.ty == c.ty
                        && e.host == c.host
                        && e.port == f64::from(c.port)
                        && e.database_name == c.database_name
                        && e.username == c.username
                })
                .map(|e| e.id.clone())
        } else {
            None
        };
    }
}

/// The first of `keys` whose candidate has a problem, which `importsCreate`
/// refuses (`INVALID_ARGUMENT`, naming the key). A key no candidate has
/// isn't a problem here: the create reports it as its own outcome.
pub fn refused_key<'a>(
    candidates: &[ImportCandidate],
    keys: &'a [String],
) -> Option<(&'a str, ImportProblem)> {
    let problems: HashMap<&str, ImportProblem> = candidates
        .iter()
        .filter_map(|c| Some((c.key.as_str(), c.problem?)))
        .collect();
    keys.iter()
        .find_map(|k| problems.get(k.as_str()).map(|p| (k.as_str(), *p)))
}

/// Where each tool keeps its connections, relative to the home directory,
/// on `os` (`std::env::consts::OS`), as `src-tauri` read them. TablePlus
/// only runs on macOS.
pub fn default_path(source: ImportSource, os: &str) -> Option<&'static str> {
    match (source, os) {
        (ImportSource::Tableplus, "macos") => {
            Some("Library/Application Support/com.tinyapp.TablePlus/Data/Connections.plist")
        }
        (ImportSource::Dbeaver, "macos") => {
            Some("Library/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
        }
        (ImportSource::Dbeaver, "windows") => {
            Some("AppData/Roaming/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
        }
        (ImportSource::Dbeaver, "linux") => {
            Some(".local/share/DBeaverData/workspace6/General/.dbeaver/data-sources.json")
        }
        _ => None,
    }
}

/// A Seaquel type's default port, as the import mappers had it.
fn default_port(ty: &str) -> &'static str {
    match ty {
        "postgres" => "5432",
        "mysql" | "mariadb" => "3306",
        "mssql" => "1433",
        _ => "0",
    }
}

/// The port and its problem: `parseInt` of `text`, which must be a whole
/// number from 0 to 65535 (`-0` is 0).
fn read_port(parsed: Option<f64>) -> (u16, bool) {
    match parsed {
        Some(v) if (0.0..=65535.0).contains(&v) => (v as u16, true),
        _ => (0, false),
    }
}

// ---------------------------------------------------------------------------
// TablePlus

/// The candidates in TablePlus's `Connections.plist`, decoded to JSON:
/// every dict in the list whose `Driver` Seaquel supports, in list order,
/// without `duplicateOf` (see [`mark_duplicates`]). Anything but a list is
/// [`ImportError::NotAList`].
pub fn tableplus_candidates(entries: &Value) -> Result<Vec<ImportCandidate>, ImportError> {
    let Value::Array(list) = entries else {
        return Err(ImportError::NotAList);
    };
    // Each dict's `ID` as text; an empty one is no ID.
    let ids: Vec<Option<String>> = list
        .iter()
        .map(|e| {
            e.as_object()
                .map(|o| o.get("ID").map(js_str).unwrap_or_default())
        })
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for id in ids.iter().flatten().filter(|id| !id.is_empty()) {
        *counts.entry(id.as_str()).or_default() += 1;
    }
    let mut out = Vec::new();
    for (pos, entry) in list.iter().enumerate() {
        let (Some(entry), Some(id)) = (entry.as_object(), &ids[pos]) else {
            continue;
        };
        let id_problem = if id.is_empty() {
            Some(ImportProblem::NoId)
        } else if counts.get(id.as_str()).copied().unwrap_or(0) > 1 {
            Some(ImportProblem::DuplicateId)
        } else {
            None
        };
        let key = match id_problem {
            Some(_) => format!("pos:{pos}"),
            None => format!("id:{id}"),
        };
        if let Some(mut c) = tableplus_entry(entry) {
            c.key = key;
            if id_problem.is_some() {
                c.problem = id_problem;
            }
            out.push(c);
        }
    }
    Ok(out)
}

/// `mapToImportable` over `toTablePlusConnection`; `None` for a driver
/// Seaquel doesn't support. The key is the caller's.
fn tableplus_entry(entry: &serde_json::Map<String, Value>) -> Option<ImportCandidate> {
    let s = |k: &str| entry.get(k).map(js_str).unwrap_or_default();
    let ty = match s("Driver").as_str() {
        "PostgreSQL" => "postgres",
        "MySQL" => "mysql",
        "MariaDB" => "mariadb",
        "SQLite" => "sqlite",
        "SQL Server" => "mssql",
        _ => return None,
    };
    let file_based = ty == "sqlite";
    let host = or(s("DatabaseHost"), "localhost");
    let parsed = parse_int(&or(s("DatabasePort"), default_port(ty)));
    let (port, port_ok) = read_port(parsed);
    let database_name = if file_based {
        or(s("DatabasePath"), &s("DatabaseName"))
    } else {
        s("DatabaseName")
    };
    // `Number(entry.tLSMode)`, absent for `null`/missing and `NaN`, then
    // `String()` of it looked up in `{0: prefer, 1: disable, 2: require}`.
    let tls = match entry.get("tLSMode") {
        None | Some(Value::Null) => None,
        Some(v) => Some(js_to_number(v)).filter(|n| !n.is_nan()),
    };
    let ssl_mode = match (ty, tls) {
        ("postgres" | "mysql" | "mariadb", Some(n)) => tls_mode(n).map(str::to_string),
        _ => None,
    };
    let ssh_host = s("ServerAddress");
    let over_ssh =
        !file_based && entry.get("isOverSSH") == Some(&Value::Bool(true)) && !ssh_host.is_empty();
    // `parseInt(sshPort, 10) || 22`: `NaN` and 0 are 22. Anything else must
    // be a port `connectionCreate` takes, else `invalidSshPort` and 0.
    let (ssh_port, ssh_port_ok) = read_port(Some(
        parse_int(&s("ServerPort"))
            .filter(|p| *p != 0.0)
            .unwrap_or(22.0),
    ));
    let ssh_port_ok = ssh_port_ok || !over_ssh;
    let ssh_tunnel = over_ssh.then(|| SshTunnelConfig {
        enabled: true,
        host: ssh_host,
        port: f64::from(ssh_port),
        username: s("ServerUser"),
        auth_method: if entry.get("isUsePrivateKey") == Some(&Value::Bool(true)) {
            "key"
        } else {
            "password"
        }
        .to_string(),
        key_path: None,
    });
    // The name falls back to today's `${host}:${port}`, `NaN` included.
    let name = or(
        s("ConnectionName"),
        &format!("{host}:{}", js_number_text(parsed.unwrap_or(f64::NAN))),
    );
    Some(ImportCandidate {
        key: String::new(),
        name,
        ty: ty.to_string(),
        host,
        port,
        database_name,
        username: s("DatabaseUser"),
        ssl_mode,
        ssh_tunnel,
        duplicate_of: None,
        problem: if !port_ok {
            Some(ImportProblem::InvalidPort)
        } else if !ssh_port_ok {
            Some(ImportProblem::InvalidSshPort)
        } else {
            None
        },
        port_ok,
    })
}

/// TablePlus's TLS modes, as `tablePlusTlsModeToSslMode` has them: only the
/// values confirmed against its UI. `String(-0)` is `"0"`.
fn tls_mode(n: f64) -> Option<&'static str> {
    if n == 0.0 {
        Some("prefer")
    } else if n == 1.0 {
        Some("disable")
    } else if n == 2.0 {
        Some("require")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// DBeaver

/// The candidates in DBeaver's `data-sources.json`: each connection whose
/// `provider` Seaquel supports (compared in lower case), in `Object.entries`
/// order, without `duplicateOf`. Text that isn't JSON (an empty file, a BOM)
/// is [`ImportError::NotJson`] and JSON that isn't an object
/// [`ImportError::NotAnObject`]; no `connections` is no candidates.
pub fn dbeaver_candidates(json: &[u8]) -> Result<Vec<ImportCandidate>, ImportError> {
    let JsValue::Object(top) =
        serde_json::from_slice::<JsValue>(json).map_err(|_| ImportError::NotJson)?
    else {
        return Err(ImportError::NotAnObject);
    };
    let Some((_, connections)) = top.into_iter().find(|(k, _)| k == "connections") else {
        return Ok(Vec::new());
    };
    // `Object.entries` of an object, or of an array by index; a scalar gives
    // nothing (a string's characters spread into no `provider`). An object
    // or array that doesn't decode (a key with a lone surrogate escape,
    // which `serde_json` can't hold) is unreadable, not an empty list.
    let entries: Entries = match JsValue::of(&connections).ok_or(ImportError::NotJson)? {
        JsValue::Object(e) => e,
        JsValue::Array(items) => items
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i.to_string(), v))
            .collect(),
        JsValue::Other => Vec::new(),
    };
    Ok(entries
        .into_iter()
        .filter_map(|(key, raw)| dbeaver_entry(key, &raw))
        .collect())
}

/// `mapToImportable` for one connection; `None` for a provider Seaquel
/// doesn't support (or that isn't text, where the TypeScript threw), and
/// for an entry that isn't an object or doesn't decode one level deep.
///
/// Only `provider`, `name` and `configuration`'s `host`, `port`, `database`
/// and `user` are decoded, each on its own, so a number past f64's range or
/// deep nesting anywhere else keeps the connection, as in JavaScript.
fn dbeaver_entry(key: String, raw: &RawValue) -> Option<ImportCandidate> {
    let Some(JsValue::Object(fields)) = JsValue::of(raw) else {
        return None;
    };
    let provider = field(&fields, "provider")?.as_str()?.to_lowercase();
    let ty = match provider.as_str() {
        "postgresql" | "postgres" => "postgres",
        "mysql" => "mysql",
        "mariadb" => "mariadb",
        "sqlite" => "sqlite",
        "mssql" | "sqlserver" => "mssql",
        "duckdb" => "duckdb",
        _ => return None,
    };
    // `configuration || {}`; a configuration that isn't an object (or that
    // doesn't decode) has no fields.
    let config: Entries = fields
        .iter()
        .find(|(name, _)| name == "configuration")
        .and_then(|(_, v)| match JsValue::of(v) {
            Some(JsValue::Object(e)) => Some(e),
            _ => None,
        })
        .unwrap_or_default();
    // `config.x || fallback`, then the value as text.
    let or_text = |k: &str, fallback: &str| match field(&config, k) {
        Some(v) if js_truthy(&v) => js_string(&v, 0),
        _ => fallback.to_string(),
    };
    let host = or_text("host", "localhost");
    let (port, port_ok) = read_port(parse_int(&or_text("port", default_port(ty))));
    let name = match field(&fields, "name") {
        None | Some(Value::Null) => String::new(),
        Some(v) => js_string(&v, 0),
    };
    let problem = if !port_ok {
        Some(ImportProblem::InvalidPort)
    } else if name.trim_matches(is_js_space).is_empty() {
        Some(ImportProblem::NoName)
    } else {
        None
    };
    Some(ImportCandidate {
        key,
        name,
        ty: ty.to_string(),
        host,
        port,
        database_name: or_text("database", ""),
        username: or_text("user", ""),
        ssl_mode: None,
        ssh_tunnel: None,
        duplicate_of: None,
        problem,
        port_ok,
    })
}

/// A field's value as `JSON.parse` gives it, decoded on its own. A number
/// past f64's range is `±Infinity` there, so it is read as that text, which
/// is what `String()`, truthiness and `parseInt` see. A value that can't be
/// decoded at all (a lone surrogate escape, nesting past serde's limit) is
/// absent.
fn field(entries: &Entries, name: &str) -> Option<Value> {
    let raw = entries.iter().find(|(k, _)| k == name)?.1.get();
    match serde_json::from_str::<Value>(raw) {
        Ok(v) => Some(v),
        Err(_) if raw.starts_with('-') && is_number_literal(raw) => {
            Some(Value::String("-Infinity".into()))
        }
        Err(_) if is_number_literal(raw) => Some(Value::String("Infinity".into())),
        Err(_) => None,
    }
}

fn is_number_literal(raw: &str) -> bool {
    raw.bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
}

type Entries = Vec<(String, Box<RawValue>)>;

/// A JSON value as `JSON.parse` and `Object.entries` see it, one level deep:
/// an object's keys in `Object.entries` order (array-index keys first,
/// ascending, then the rest as written) with a repeated key in its first
/// place and its last value, an array's items, or something else. The
/// values stay raw until read, so only the top level and `connections` are
/// parsed this way.
enum JsValue {
    Object(Entries),
    Array(Vec<Box<RawValue>>),
    Other,
}

impl JsValue {
    /// A raw value read one level deep: `Other` for a scalar, `None` for an
    /// object or array that doesn't decode (a key `serde_json` can't hold).
    fn of(raw: &RawValue) -> Option<Self> {
        match raw.get().as_bytes().first() {
            Some(b'{' | b'[') => serde_json::from_str(raw.get()).ok(),
            _ => Some(Self::Other),
        }
    }
}

impl<'de> Deserialize<'de> for JsValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = JsValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<JsValue, A::Error> {
                let mut entries: Entries = Vec::new();
                let mut at: HashMap<String, usize> = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value = map.next_value::<Box<RawValue>>()?;
                    match at.get(&key) {
                        Some(&i) => entries[i].1 = value,
                        None => {
                            at.insert(key.clone(), entries.len());
                            entries.push((key, value));
                        }
                    }
                }
                // Stable, so the other keys keep their order.
                entries.sort_by_key(|(k, _)| array_index(k).map_or(u64::MAX, u64::from));
                Ok(JsValue::Object(entries))
            }
            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<JsValue, A::Error> {
                let mut items = Vec::new();
                while let Some(v) = seq.next_element::<Box<RawValue>>()? {
                    items.push(v);
                }
                Ok(JsValue::Array(items))
            }
            fn visit_bool<E>(self, _: bool) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
            fn visit_i64<E>(self, _: i64) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
            fn visit_u64<E>(self, _: u64) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
            fn visit_f64<E>(self, _: f64) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
            fn visit_str<E>(self, _: &str) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
            fn visit_unit<E>(self) -> Result<JsValue, E> {
                Ok(JsValue::Other)
            }
        }
        d.deserialize_any(V)
    }
}

/// An array index (`"0"`, `"17"`, at most 2^32 − 2): the keys
/// `Object.entries` lists first, in ascending order.
fn array_index(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.is_empty() || b.len() > 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if b.len() > 1 && b[0] == b'0' {
        return None;
    }
    key.parse::<u64>()
        .ok()
        .filter(|n| *n <= u64::from(u32::MAX - 1))
        .map(|n| n as u32)
}

// ---------------------------------------------------------------------------
// JavaScript's coercions

/// `parseInt(text, 10)`: leading JavaScript whitespace, a sign, then the
/// ASCII digits up to the first other character. `None` is `NaN`. Past
/// about 309 digits it is infinite, as in JavaScript; `"-0"` is `-0`.
pub fn parse_int(text: &str) -> Option<f64> {
    let s = text.trim_start_matches(is_js_space);
    let (negative, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = rest
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    let digits = &rest[..end];
    if digits.is_empty() {
        return None;
    }
    let value = digits.parse::<f64>().unwrap_or(f64::INFINITY);
    Some(if negative { -value } else { value })
}

/// `String(n)` for a JavaScript number.
fn js_number_text(n: f64) -> String {
    ryu_js::Buffer::new().format(n).to_string()
}

/// `toTablePlusConnection`'s `str`: text, a number's or a boolean's text,
/// and `""` for anything else.
fn js_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => js_number_text(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// `a || b` on text.
fn or(a: String, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else {
        a
    }
}

/// JavaScript truthiness of a JSON value.
fn js_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// How deep `js_string` follows nested arrays before it gives up (`""`).
const MAX_JS_STRING_DEPTH: usize = 32;

/// `String(v)`. An array is its items joined with `,` (`null` as `""`), an
/// object `[object Object]`.
fn js_string(v: &Value, depth: usize) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Array(items) => {
            if depth >= MAX_JS_STRING_DEPTH {
                return String::new();
            }
            let parts: Vec<String> = items
                .iter()
                .map(|i| match i {
                    Value::Null => String::new(),
                    other => js_string(other, depth + 1),
                })
                .collect();
            parts.join(",")
        }
        Value::Object(_) => "[object Object]".to_string(),
        other => js_str(other),
    }
}

/// `Number(v)`. An array goes through its text: empty is 0, one item is
/// that item's text read as a number, more than one has a `,` and is `NaN`.
fn js_to_number(v: &Value) -> f64 {
    let mut v = v;
    // Nested one-item arrays, without recursion.
    loop {
        match v {
            Value::Null => return 0.0,
            Value::Bool(b) => return if *b { 1.0 } else { 0.0 },
            Value::Number(n) => return n.as_f64().unwrap_or(f64::NAN),
            Value::String(s) => return string_to_number(s),
            Value::Object(_) => return f64::NAN,
            Value::Array(items) => match items.as_slice() {
                [] => return 0.0,
                [Value::Null] => return 0.0,
                // `String(true)` is "true", which isn't a number.
                [Value::Bool(_)] | [Value::Object(_)] => return f64::NAN,
                [item] => v = item,
                _ => return f64::NAN,
            },
        }
    }
}

/// JavaScript's `StringToNumber`: trimmed; empty is 0; `Infinity` with an
/// optional sign; `0x`, `0o` and `0b` integers; else a decimal literal, and
/// anything more is `NaN`.
fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(is_js_space);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let radix = match t.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0o" | "0O") => 8,
        Some("0b" | "0B") => 2,
        _ => 10,
    };
    if radix != 10 {
        let digits = &t[2..];
        if digits.is_empty() {
            return f64::NAN;
        }
        let mut value = 0.0_f64;
        for c in digits.chars() {
            match c.to_digit(radix) {
                Some(d) => value = value * f64::from(radix) + f64::from(d),
                None => return f64::NAN,
            }
        }
        return value;
    }
    // Rust's float parser also takes `inf` and `nan`, which JavaScript
    // doesn't: only a decimal literal's characters get through.
    if !t
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_like_javascript() {
        assert_eq!(js_number_text(5432.0), "5432");
        assert_eq!(js_number_text(-0.0), "0");
        assert_eq!(js_number_text(1.5), "1.5");
        assert_eq!(js_number_text(1e21), "1e+21");
        assert_eq!(js_number_text(1e-7), "1e-7");
        assert_eq!(js_number_text(f64::NAN), "NaN");
        assert_eq!(js_number_text(f64::INFINITY), "Infinity");
    }

    #[test]
    fn strings_read_as_numbers_like_javascript() {
        for (s, want) in [
            ("", 0.0),
            (" 2 ", 2.0),
            ("\u{a0}1\n", 1.0),
            ("0x1F", 31.0),
            ("0b11", 3.0),
            ("0o7", 7.0),
            ("-2", -2.0),
            ("+2", 2.0),
            (".5", 0.5),
            ("2.", 2.0),
            ("1e1", 10.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            assert_eq!(string_to_number(s), want, "{s:?}");
        }
        for s in [
            "on", "inf", "NaN", "infinity", "1e", ".", "0x", "-0x1", "1 2", "2px", "e5", "--1",
        ] {
            assert!(string_to_number(s).is_nan(), "{s:?}");
        }
    }

    #[test]
    fn array_indexes_are_javascripts() {
        assert_eq!(array_index("0"), Some(0));
        assert_eq!(array_index("4294967294"), Some(4_294_967_294));
        for k in [
            "",
            "01",
            "-1",
            "1.0",
            "4294967295",
            "99999999999",
            " 1",
            "a",
        ] {
            assert_eq!(array_index(k), None, "{k:?}");
        }
    }
}
