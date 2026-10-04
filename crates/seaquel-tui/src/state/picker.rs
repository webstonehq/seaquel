//! Choosing what to connect to (Decision 10, panel 1): the picker (a
//! project, then one of its connections), how `--project` and
//! `--connection` skip it (an id, else an exact, case-sensitive name, as
//! `seaquel-cli mcp` resolves them), and what the TUI remembers between
//! runs (Q4 A: the state file, `runtime/state_file.rs`).

use std::collections::BTreeMap;

use super::panels::{ConnItem, Library, ProjectItem};

/// The picker's step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    Projects,
    Connections { project_id: String },
}

/// The picker dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picker {
    pub stage: Stage,
    pub selected: usize,
}

/// What the TUI remembers between runs (Q4 A). Ids only, no names; the
/// open query tabs keep their text (Q4 A's "open query tabs with their
/// text").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Remembered {
    pub last_project: Option<String>,
    /// Per project, the connection last connected.
    pub last_connection: BTreeMap<String, String>,
    /// Panel 2's and 3's tabs (`views`, `history`) when not the first.
    pub tables_tab: Option<String>,
    pub saved_tab: Option<String>,
    /// `--theme`, once given: `dark` or `light`.
    pub theme: Option<String>,
    /// The open query tabs (Task 6) and the active one.
    pub query_tabs: Vec<RememberedTab>,
    pub query_active: usize,
}

/// A tab's text kept longer than this isn't written to the state file
/// (review M1): the tab is remembered with a notice instead.
pub const MAX_REMEMBERED_TEXT: usize = 1024 * 1024;

/// A query tab as the state file keeps it (review M1): the saved query it
/// came from (by id; its name and stored text are read from the library),
/// a hash of the stored text, and the text only when it differs from the
/// stored one and is at most [`MAX_REMEMBERED_TEXT`]. `Debug` shows no
/// text.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RememberedTab {
    pub saved_id: Option<String>,
    /// `None`: an unchanged saved query (its text is the library's), or a
    /// text too long to keep (`omitted`).
    pub text: Option<String>,
    /// [`text_hash`] of the saved query's text as the tab last knew it.
    pub stored_hash: Option<String>,
    /// The text was over [`MAX_REMEMBERED_TEXT`] and wasn't kept.
    pub omitted: bool,
}

impl std::fmt::Debug for RememberedTab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RememberedTab")
            .field("saved_id", &self.saved_id)
            .field("bytes", &self.text.as_ref().map(String::len))
            .field("omitted", &self.omitted)
            .finish_non_exhaustive()
    }
}

/// A stable hash of a text (64-bit FNV-1a, hex): the state file compares
/// stored texts by it instead of keeping them.
pub fn text_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// How the TUI starts: the project panel 3 shows, a connection to connect
/// to at once, or the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub project: Option<String>,
    pub connect: Option<String>,
    pub picker: Option<Picker>,
}

/// Resolves `--project` and `--connection` against the library; with
/// neither, the picker opens on the projects with the remembered one
/// selected. Errors are worded for stderr, as the CLI's are.
pub fn resolve_start(
    library: &Library,
    project: Option<&str>,
    connection: Option<&str>,
    remembered: &Remembered,
) -> Result<Start, String> {
    let project = match project {
        Some(wanted) => Some(find_project(library, wanted)?),
        None => None,
    };
    if let Some(wanted) = connection {
        let candidates: Vec<&ConnItem> = match project {
            Some(p) => library.connections_of(&p.id).collect(),
            None => library.connections.iter().collect(),
        };
        let conn = match lookup(&candidates, wanted, |c| &c.id, |c| &c.name) {
            Found::One(c) => c,
            Found::None => {
                return Err(match project {
                    Some(p) if library.connection(wanted).is_some() => format!(
                        "--connection {wanted:?} isn't in project {:?} ({})",
                        p.name, p.id
                    ),
                    _ => {
                        format!("--connection {wanted:?}: no saved connection has this name or id")
                    }
                })
            }
            Found::Many(many) => {
                return Err(format!(
                    "--connection {wanted:?}: {} saved connections have this name: {}. Pass \
                     --connection with the id instead",
                    many.len(),
                    many.iter()
                        .map(|c| c.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            }
        };
        return Ok(Start {
            project: Some(conn.project_id.clone()),
            connect: Some(conn.id.clone()),
            picker: None,
        });
    }
    if let Some(p) = project {
        return Ok(Start {
            project: Some(p.id.clone()),
            connect: None,
            picker: Some(connections_stage(library, &p.id, remembered)),
        });
    }
    let picker = open_picker(library, None, remembered);
    Ok(Start {
        project: selected_project(library, &picker).map(|p| p.id.clone()),
        connect: None,
        picker: Some(picker),
    })
}

/// What [`lookup`] found.
enum Found<'a, T> {
    One(&'a T),
    None,
    Many(Vec<&'a T>),
}

/// The item whose id is `wanted`, else the one whose name is: exact and
/// case-sensitive, as `seaquel-cli mcp` matches.
fn lookup<'a, T>(
    items: &[&'a T],
    wanted: &str,
    id: impl Fn(&T) -> &str,
    name: impl Fn(&T) -> &str,
) -> Found<'a, T> {
    if let Some(item) = items.iter().find(|i| id(i) == wanted) {
        return Found::One(item);
    }
    let named: Vec<&T> = items
        .iter()
        .copied()
        .filter(|i| name(i) == wanted)
        .collect();
    match named.len() {
        0 => Found::None,
        1 => Found::One(named[0]),
        _ => Found::Many(named),
    }
}

fn find_project<'a>(library: &'a Library, wanted: &str) -> Result<&'a ProjectItem, String> {
    let all: Vec<&ProjectItem> = library.projects.iter().collect();
    match lookup(&all, wanted, |p| &p.id, |p| &p.name) {
        Found::One(p) => Ok(p),
        Found::None => Err(format!(
            "--project {wanted:?}: no project has this name or id"
        )),
        Found::Many(many) => Err(format!(
            "--project {wanted:?}: {} projects have this name: {}. Pass --project with the id \
             instead",
            many.len(),
            many.iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The picker as it opens from panel 1: on the projects, the current (else
/// the remembered) one selected.
pub fn open_picker(library: &Library, current: Option<&str>, remembered: &Remembered) -> Picker {
    let wanted = current.or(remembered.last_project.as_deref());
    let selected = wanted
        .and_then(|id| library.projects.iter().position(|p| p.id == id))
        .unwrap_or(0);
    Picker {
        stage: Stage::Projects,
        selected,
    }
}

/// The picker on `project_id`'s connections, the remembered one selected.
pub fn connections_stage(library: &Library, project_id: &str, remembered: &Remembered) -> Picker {
    let selected = remembered
        .last_connection
        .get(project_id)
        .and_then(|id| library.connections_of(project_id).position(|c| &c.id == id))
        .unwrap_or(0);
    Picker {
        stage: Stage::Connections {
            project_id: project_id.to_string(),
        },
        selected,
    }
}

/// What the picker lists now.
pub fn picker_len(library: &Library, picker: &Picker) -> usize {
    match &picker.stage {
        Stage::Projects => library.projects.len(),
        Stage::Connections { project_id } => library.connections_of(project_id).count(),
    }
}

/// The selected project, at the projects step.
pub fn selected_project<'a>(library: &'a Library, picker: &Picker) -> Option<&'a ProjectItem> {
    match picker.stage {
        Stage::Projects => library.projects.get(picker.selected),
        Stage::Connections { .. } => None,
    }
}

/// The selected connection, at the connections step.
pub fn selected_connection<'a>(library: &'a Library, picker: &'a Picker) -> Option<&'a ConnItem> {
    match &picker.stage {
        Stage::Projects => None,
        Stage::Connections { project_id } => {
            library.connections_of(project_id).nth(picker.selected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: &str, project: &str, name: &str) -> ConnItem {
        ConnItem {
            id: id.into(),
            project_id: project.into(),
            name: name.into(),
            engine: "sqlite".into(),
            host: String::new(),
            port: None,
            database: "x.db".into(),
            save_password: false,
            save_ssh_password: false,
            save_ssh_key_passphrase: false,
            tunnel: None,
            label_ids: Vec::new(),
            ai: Default::default(),
        }
    }

    fn library() -> Library {
        Library {
            projects: vec![
                ProjectItem {
                    id: "project-a".into(),
                    name: "Alpha".into(),
                },
                ProjectItem {
                    id: "project-b".into(),
                    name: "Beta".into(),
                },
                ProjectItem {
                    id: "project-c".into(),
                    name: "Beta".into(),
                },
            ],
            connections: vec![
                conn("conn-1", "project-a", "local"),
                conn("conn-2", "project-b", "staging"),
                conn("conn-3", "project-b", "prod"),
                conn("conn-4", "project-a", "prod"),
            ],
            labels: Vec::new(),
            ai_off: false,
        }
    }

    #[test]
    fn with_no_arguments_the_picker_opens_on_the_remembered_project() {
        let lib = library();
        let start = resolve_start(&lib, None, None, &Remembered::default()).unwrap();
        assert_eq!(
            start,
            Start {
                project: Some("project-a".into()),
                connect: None,
                picker: Some(Picker {
                    stage: Stage::Projects,
                    selected: 0
                }),
            }
        );
        let remembered = Remembered {
            last_project: Some("project-b".into()),
            ..Remembered::default()
        };
        let start = resolve_start(&lib, None, None, &remembered).unwrap();
        assert_eq!(start.project.as_deref(), Some("project-b"));
        assert_eq!(start.picker.unwrap().selected, 1);

        // A remembered project that's gone falls back to the first.
        let gone = Remembered {
            last_project: Some("project-gone".into()),
            ..Remembered::default()
        };
        let start = resolve_start(&lib, None, None, &gone).unwrap();
        assert_eq!(start.project.as_deref(), Some("project-a"));

        let empty = resolve_start(&Library::default(), None, None, &gone).unwrap();
        assert_eq!(empty.project, None);
        assert!(empty.picker.is_some());
    }

    #[test]
    fn connection_by_id_else_exact_name_skips_the_picker() {
        let lib = library();
        let r = Remembered::default();
        for wanted in ["conn-2", "staging"] {
            let start = resolve_start(&lib, None, Some(wanted), &r).unwrap();
            assert_eq!(start.connect.as_deref(), Some("conn-2"), "{wanted}");
            assert_eq!(start.project.as_deref(), Some("project-b"));
            assert_eq!(start.picker, None);
        }
        let err = resolve_start(&lib, None, Some("Staging"), &r).unwrap_err();
        assert_eq!(
            err,
            "--connection \"Staging\": no saved connection has this name or id"
        );
        let err = resolve_start(&lib, None, Some("prod"), &r).unwrap_err();
        assert!(err.contains("2 saved connections have this name"), "{err}");
        assert!(err.contains("conn-3") && err.contains("conn-4"), "{err}");
        // Within a project, the name is the project's.
        let start = resolve_start(&lib, Some("Alpha"), Some("prod"), &r).unwrap();
        assert_eq!(start.connect.as_deref(), Some("conn-4"));
        let err = resolve_start(&lib, Some("project-a"), Some("conn-2"), &r).unwrap_err();
        assert!(err.contains("isn't in project"), "{err}");
    }

    #[test]
    fn project_alone_opens_its_connections_with_the_last_one_selected() {
        let lib = library();
        let remembered = Remembered {
            last_connection: [("project-b".to_string(), "conn-3".to_string())].into(),
            ..Remembered::default()
        };
        let start = resolve_start(&lib, Some("project-b"), None, &remembered).unwrap();
        assert_eq!(start.project.as_deref(), Some("project-b"));
        assert_eq!(
            start.picker,
            Some(Picker {
                stage: Stage::Connections {
                    project_id: "project-b".into()
                },
                selected: 1,
            })
        );
        let err = resolve_start(&lib, Some("Beta"), None, &remembered).unwrap_err();
        assert!(err.contains("2 projects have this name"), "{err}");
        let err = resolve_start(&lib, Some("Gamma"), None, &remembered).unwrap_err();
        assert_eq!(err, "--project \"Gamma\": no project has this name or id");
    }

    #[test]
    fn the_picker_preselects_what_was_used_last() {
        let lib = library();
        let remembered = Remembered {
            last_project: Some("project-b".into()),
            last_connection: [("project-a".to_string(), "conn-4".to_string())].into(),
            ..Remembered::default()
        };
        assert_eq!(open_picker(&lib, None, &remembered).selected, 1);
        assert_eq!(
            open_picker(&lib, Some("project-c"), &remembered).selected,
            2
        );
        let p = connections_stage(&lib, "project-a", &remembered);
        assert_eq!(p.selected, 1);
        assert_eq!(selected_connection(&lib, &p).unwrap().id, "conn-4");
        assert_eq!(picker_len(&lib, &p), 2);
        let p = connections_stage(&lib, "project-c", &remembered);
        assert_eq!((p.selected, picker_len(&lib, &p)), (0, 0));
        assert!(selected_connection(&lib, &p).is_none());
    }
}
