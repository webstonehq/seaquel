//! Where an app's data lives.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::StorageError;

/// The environment variable that overrides the data dir.
pub const DATA_DIR_ENV: &str = "SEAQUEL_DATA_DIR";

/// The data dir for the app `identifier` (`app.seaquel.desktop`, or
/// `app.seaquel.desktop.dev` for dev builds).
///
/// `SEAQUEL_DATA_DIR`, when set and non-empty, is the data dir itself, and
/// `identifier` isn't appended. Otherwise it is the platform's data dir plus
/// `identifier`, which is what Tauri 2's `app_data_dir` resolves to:
///
/// - macOS: `~/Library/Application Support/<identifier>`
/// - Linux: `$XDG_DATA_HOME/<identifier>`, or `~/.local/share/<identifier>`
/// - Windows: `%APPDATA%\<identifier>`
///
/// Nothing is created.
pub fn data_dir(identifier: &str) -> Result<PathBuf, StorageError> {
    resolve(std::env::var_os(DATA_DIR_ENV), dirs::data_dir(), identifier)
}

fn resolve(
    env: Option<OsString>,
    platform: Option<PathBuf>,
    identifier: &str,
) -> Result<PathBuf, StorageError> {
    match env {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => platform
            .map(|base| base.join(identifier))
            .ok_or(StorageError::NoDataDir),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "app.seaquel.test";

    #[test]
    fn env_var_wins_and_is_the_dir_itself() {
        let dir = resolve(
            Some("/srv/seaquel".into()),
            Some("/home/me/.local/share".into()),
            ID,
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/srv/seaquel"));
    }

    #[test]
    fn empty_env_var_is_ignored() {
        let dir = resolve(Some("".into()), Some("/base".into()), ID).unwrap();
        assert_eq!(dir, PathBuf::from("/base").join(ID));
    }

    #[test]
    fn platform_dir_gets_the_identifier() {
        let dir = resolve(None, Some("/base".into()), ID).unwrap();
        assert_eq!(dir, PathBuf::from("/base").join(ID));
    }

    #[test]
    fn no_env_and_no_platform_dir_is_an_error() {
        let err = resolve(None, None, ID).unwrap_err();
        assert!(matches!(err, StorageError::NoDataDir));
        assert_eq!(err.code(), crate::NO_DATA_DIR);
    }

    /// The fallback, without whatever `SEAQUEL_DATA_DIR` the test run has.
    fn platform_default() -> PathBuf {
        resolve(None, dirs::data_dir(), ID).unwrap()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_path() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            platform_default(),
            home.join("Library/Application Support").join(ID)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_path() {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| dirs::home_dir().unwrap().join(".local/share"));
        assert_eq!(platform_default(), base.join(ID));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path() {
        let appdata = PathBuf::from(std::env::var_os("APPDATA").unwrap());
        assert_eq!(platform_default(), appdata.join(ID));
    }
}
