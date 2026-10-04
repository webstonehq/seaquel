//! The TUI's own state file (Q4 A, Decision 4): `<data_dir>/tui/state.json`
//! with the last project, the last connection per project, the panel tabs,
//! the theme, and the open query tabs with their text (Task 6). Ids only,
//! never a name (a saved query's tab keeps its id; its name is read from
//! the library). Not `seaquel.db`: a TUI blob in
//! `window_state` would be copied into new GUI windows.
//!
//! Written atomically (a temp file in the same folder, renamed over it,
//! then the folder fsynced), 0600 in a 0700 folder that may not be a
//! symlink, 500 ms after a change (on a blocking thread) and at exit. Each TUI
//! process overwrites it (the last one wins). A file that can't be read or
//! parsed is ignored with a log line naming the error's kind only.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::state::picker::{Remembered, RememberedTab};

/// The file's shape. Unknown fields (a newer version's) are ignored.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct File {
    version: u32,
    last_project: Option<String>,
    last_connection: BTreeMap<String, String>,
    tables_tab: Option<String>,
    saved_tab: Option<String>,
    theme: Option<String>,
    query_tabs: Vec<FileTab>,
    query_active: usize,
}

/// A query tab (Task 6).
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct FileTab {
    saved_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_hash: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    omitted: bool,
}

/// The most the file may be when read (review M1): a bigger one is ignored.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// The file's format version.
pub const VERSION: u32 = 1;

/// `<data_dir>/tui/state.json`.
pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("tui").join("state.json")
}

/// Why the file wasn't read.
#[derive(Debug)]
pub enum ReadError {
    Io(io::ErrorKind),
    /// Not JSON, or not the state file's shape.
    Corrupt,
    /// Over [`MAX_FILE_BYTES`].
    TooLarge,
}

/// Reads the file; a missing one is the default.
pub fn read(data_dir: &Path) -> Result<Remembered, ReadError> {
    match std::fs::metadata(path(data_dir)) {
        Ok(meta) if meta.len() > MAX_FILE_BYTES => return Err(ReadError::TooLarge),
        _ => {}
    }
    let text = match std::fs::read_to_string(path(data_dir)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Remembered::default()),
        Err(e) => return Err(ReadError::Io(e.kind())),
    };
    let file: File = serde_json::from_str(&text).map_err(|_| ReadError::Corrupt)?;
    Ok(Remembered {
        last_project: file.last_project,
        last_connection: file.last_connection,
        tables_tab: file.tables_tab,
        saved_tab: file.saved_tab,
        theme: file.theme,
        query_tabs: file
            .query_tabs
            .into_iter()
            .map(|t| RememberedTab {
                saved_id: t.saved_id,
                text: t.text,
                stored_hash: t.stored_hash,
                omitted: t.omitted,
            })
            .collect(),
        query_active: file.query_active,
    })
}

/// [`read`], with any problem logged and the default used.
pub fn load(data_dir: &Path) -> Remembered {
    match read(data_dir) {
        Ok(remembered) => remembered,
        Err(e) => {
            log::warn!(activity = "tui.state_file", error = format!("{e:?}").as_str(); "Ignoring the state file");
            Remembered::default()
        }
    }
}

/// A number for a save, taken when the save is asked for: the file ends up
/// holding the save with the highest one (review I3).
pub fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// The last `seq` written per data dir; holding it is the turn to write.
static WRITTEN: Mutex<BTreeMap<PathBuf, u64>> = Mutex::new(BTreeMap::new());

/// Writes the file atomically unless a save with a higher `seq` already
/// wrote it (then `Ok(false)`). Saves of one data dir take turns, so the
/// exit save waits for a write in flight (review I3).
pub fn save_latest(data_dir: &Path, remembered: &Remembered, seq: u64) -> io::Result<bool> {
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    let key = data_dir.to_path_buf();
    if written.get(&key).is_some_and(|last| *last >= seq) {
        return Ok(false);
    }
    write(data_dir, remembered)?;
    written.insert(key, seq);
    Ok(true)
}

/// Writes the file atomically (as the newest save).
pub fn save(data_dir: &Path, remembered: &Remembered) -> io::Result<()> {
    save_latest(data_dir, remembered, next_seq()).map(|_| ())
}

/// The write itself.
fn write(data_dir: &Path, remembered: &Remembered) -> io::Result<()> {
    if !data_dir.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the data dir doesn't exist",
        ));
    }
    let file = File {
        version: VERSION,
        last_project: remembered.last_project.clone(),
        last_connection: remembered.last_connection.clone(),
        tables_tab: remembered.tables_tab.clone(),
        saved_tab: remembered.saved_tab.clone(),
        theme: remembered.theme.clone(),
        query_tabs: remembered
            .query_tabs
            .iter()
            .map(|t| FileTab {
                saved_id: t.saved_id.clone(),
                text: t.text.clone(),
                stored_hash: t.stored_hash.clone(),
                omitted: t.omitted,
            })
            .collect(),
        query_active: remembered.query_active,
    };
    let text = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let target = path(data_dir);
    let folder = target.parent().expect("state.json has a folder");
    make_private_dir(folder)?;
    // A name of its own per write (review I3).
    static TEMP: AtomicU64 = AtomicU64::new(0);
    let temp = folder.join(format!(
        ".state.json.{}.{}.tmp",
        std::process::id(),
        TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut out = private_file(&temp)?;
        out.write_all(text.as_bytes())?;
        out.sync_all()?;
        std::fs::rename(&temp, &target)?;
        sync_dir(folder)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// `tui/`, created 0700 if missing (only it: the data dir must exist).
/// A symlink is refused; an existing folder is narrowed to 0700.
pub(crate) fn make_private_dir(folder: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(folder) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the state folder isn't a plain folder",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(folder)?;
        }
        Err(e) => return Err(e),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Makes the rename durable (Unix; elsewhere the rename is what there is).
fn sync_dir(folder: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(folder)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = folder;
    Ok(())
}

/// A new file, 0600, never through a symlink.
fn private_file(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Remembered {
        Remembered {
            last_project: Some("project-a".into()),
            last_connection: [
                ("project-a".to_string(), "conn-1".to_string()),
                ("project-b".to_string(), "conn-2".to_string()),
            ]
            .into(),
            tables_tab: Some("views".into()),
            saved_tab: None,
            theme: Some("light".into()),
            query_tabs: vec![
                RememberedTab {
                    saved_id: Some("saved-1".into()),
                    text: Some("SELECT 1 -- edited".into()),
                    stored_hash: Some(crate::state::picker::text_hash("SELECT 1")),
                    omitted: false,
                },
                RememberedTab {
                    saved_id: None,
                    text: Some("SELECT '😀'".into()),
                    stored_hash: None,
                    omitted: false,
                },
                RememberedTab {
                    saved_id: Some("saved-2".into()),
                    text: None,
                    stored_hash: Some(crate::state::picker::text_hash("SELECT 2")),
                    omitted: false,
                },
                RememberedTab {
                    saved_id: None,
                    text: None,
                    stored_hash: None,
                    omitted: true,
                },
            ],
            query_active: 1,
        }
    }

    #[test]
    fn it_round_trips_and_a_missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path()).unwrap(), Remembered::default());
        save(dir.path(), &sample()).unwrap();
        assert_eq!(read(dir.path()).unwrap(), sample());
        // Overwritten whole.
        save(dir.path(), &Remembered::default()).unwrap();
        assert_eq!(load(dir.path()), Remembered::default());
    }

    #[test]
    fn it_is_written_atomically_and_privately() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &sample()).unwrap();
        save(dir.path(), &sample()).unwrap();
        let folder = dir.path().join("tui");
        let names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["state.json"], "no temp file is left");
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(text.contains("\"version\": 1"), "{text}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path(dir.path())), 0o600);
            assert_eq!(mode(&folder), 0o700);
        }
    }

    /// Review I3: saves asked for in order but written by many threads at
    /// once leave a whole file holding the latest, and no temp file.
    #[test]
    fn concurrent_saves_leave_the_latest() {
        let dir = tempfile::tempdir().unwrap();
        let states: Vec<(u64, Remembered)> = (0..24)
            .map(|i| {
                let mut r = sample();
                r.last_project = Some(format!("project-{i}"));
                r.query_tabs[1].text = Some("x".repeat(64 * 1024 + i));
                (next_seq(), r)
            })
            .collect();
        let latest = states.last().unwrap().1.clone();
        std::thread::scope(|scope| {
            // The newest first, so older ones try to write after it.
            for (seq, r) in states.iter().rev() {
                let dir = dir.path();
                scope.spawn(move || save_latest(dir, r, *seq).unwrap());
            }
        });
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).expect("whole JSON");
        assert_eq!(read(dir.path()).unwrap(), latest);
        let names: Vec<String> = std::fs::read_dir(dir.path().join("tui"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["state.json"]);
        // An older save asked for after the newest wrote is skipped.
        assert!(!save_latest(dir.path(), &sample(), states[0].0).unwrap());
        assert_eq!(read(dir.path()).unwrap(), latest);
    }

    // Review M1: a file past 32 MiB isn't read; an unchanged saved tab
    // keeps only a hash.
    #[test]
    fn a_huge_file_is_ignored_and_unchanged_tabs_keep_no_text() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &sample()).unwrap();
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(!text.contains("SELECT 2"), "{text}");
        assert!(text.contains("storedHash"), "{text}");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path(dir.path()))
            .unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert!(matches!(read(dir.path()), Err(ReadError::TooLarge)));
        assert_eq!(load(dir.path()), Remembered::default());
    }

    #[test]
    fn a_corrupt_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tui")).unwrap();
        for bad in ["not json", "[1, 2]", r#"{"version": 1, "lastProject": 7}"#] {
            std::fs::write(path(dir.path()), bad).unwrap();
            assert!(matches!(read(dir.path()), Err(ReadError::Corrupt)), "{bad}");
            assert_eq!(load(dir.path()), Remembered::default());
        }
        // A newer version's extra fields are fine; its own fields are read.
        std::fs::write(
            path(dir.path()),
            r#"{"version": 2, "lastProject": "p", "tabs": []}"#,
        )
        .unwrap();
        assert_eq!(read(dir.path()).unwrap().last_project.as_deref(), Some("p"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_folder_is_refused_and_a_wide_one_narrowed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("tui")).unwrap();
        assert!(save(dir.path(), &sample()).is_err());
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);

        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("tui");
        std::fs::create_dir(&folder).unwrap();
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        save(dir.path(), &sample()).unwrap();
        let mode = std::fs::metadata(&folder).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[tokio::test]
    async fn the_runner_writes_it_off_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let (mut runner, _inbox) =
            crate::runtime::effects::Runner::new(None, Some(dir.path().to_path_buf()));
        runner.perform(crate::state::app::Effect::SaveState(sample()));
        // Written on a blocking thread, not by `perform` itself.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while read(dir.path()).unwrap() != sample() {
            assert!(std::time::Instant::now() < deadline, "never written");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn a_save_without_a_data_dir_fails_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert!(save(&missing, &sample()).is_err());
        assert!(!missing.exists(), "the data dir isn't created");
    }
}
