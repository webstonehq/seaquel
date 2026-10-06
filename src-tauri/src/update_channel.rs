//! The desktop updater's channel: which feed it asks, read from the
//! `updateChannel` setting (see `docs/plans/2026-10-05-beta-update-channel-design.md`).

use semver::Version;
use tauri::Url;

/// Must match `plugins.updater.endpoints` in `tauri.conf.json`
/// (`the_stable_feed_matches_the_config`).
const STABLE_ENDPOINT: &str =
    "https://seaquel.app/updates/check/{{target}}/{{arch}}/{{current_version}}";
const BETA_ENDPOINT: &str =
    "https://seaquel.app/updates/check/beta/{{target}}/{{arch}}/{{current_version}}";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateChannel {
    Stable,
    Beta,
}

impl UpdateChannel {
    /// The stored setting, or the default for this build when it is unset
    /// or unreadable: a pre-release build (`2026.10.0-beta.2`) follows beta.
    pub(crate) fn resolve(stored: Option<&str>, version: &Version) -> Self {
        match stored {
            Some("beta") => UpdateChannel::Beta,
            Some("stable") => UpdateChannel::Stable,
            _ if !version.pre.is_empty() => UpdateChannel::Beta,
            _ => UpdateChannel::Stable,
        }
    }

    /// The feed to ask. `Url` percent-encodes the `{{…}}` placeholders; the
    /// updater substitutes the encoded form too.
    pub(crate) fn endpoint(self) -> Url {
        let s = match self {
            UpdateChannel::Stable => STABLE_ENDPOINT,
            UpdateChannel::Beta => BETA_ENDPOINT,
        };
        Url::parse(s).expect("the updater endpoints are valid URLs")
    }
}

/// This build's real version. Windows builds rewrite `tauri.conf.json`'s
/// version for MSI (`2026.10.0-beta.3` becomes `26.10.0-3`, see
/// `release.yml`), so `package_info().version` is wrong there; the crate's
/// own version isn't rewritten.
pub(crate) fn app_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the crate version is semver")
}

/// Whether the feed offers an update: only a newer version, never a
/// downgrade (a beta user who switches to stable keeps the beta until
/// stable passes it) and never the version that runs.
pub(crate) fn is_newer(remote: &Version, current: &Version) -> bool {
    remote > current
}

/// A downloaded update, with the channel and version it came as.
pub(crate) struct PendingDownload {
    pub(crate) channel: UpdateChannel,
    pub(crate) version: String,
    pub(crate) bytes: Vec<u8>,
}

impl PendingDownload {
    /// Whether it may be installed now: only as the version it was
    /// downloaded as, and only on the channel that downloaded it.
    pub(crate) fn matches(&self, channel: UpdateChannel, version: &str) -> bool {
        self.channel == channel && self.version == version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn the_stored_channel_wins() {
        assert_eq!(
            UpdateChannel::resolve(Some("beta"), &v("2026.9.2")),
            UpdateChannel::Beta
        );
        assert_eq!(
            UpdateChannel::resolve(Some("stable"), &v("2026.10.0-beta.1")),
            UpdateChannel::Stable
        );
    }

    #[test]
    fn unset_follows_the_build() {
        assert_eq!(
            UpdateChannel::resolve(None, &v("2026.9.2")),
            UpdateChannel::Stable
        );
        assert_eq!(
            UpdateChannel::resolve(None, &v("2026.10.0-beta.2")),
            UpdateChannel::Beta
        );
        // A value Core would refuse reads as unset.
        assert_eq!(
            UpdateChannel::resolve(Some("nightly"), &v("2026.9.2")),
            UpdateChannel::Stable
        );
    }

    #[test]
    fn each_channel_has_its_feed() {
        let stable = UpdateChannel::Stable.endpoint();
        assert_eq!(stable.host_str(), Some("seaquel.app"));
        assert_eq!(
            stable.path(),
            "/updates/check/%7B%7Btarget%7D%7D/%7B%7Barch%7D%7D/%7B%7Bcurrent_version%7D%7D"
        );
        let beta = UpdateChannel::Beta.endpoint();
        assert_eq!(beta.host_str(), Some("seaquel.app"));
        assert_eq!(
            beta.path(),
            format!(
                "/updates/check/beta{}",
                &stable.path()["/updates/check".len()..]
            )
        );
    }

    #[test]
    fn the_stable_feed_matches_the_config() {
        assert!(include_str!("../tauri.conf.json").contains(STABLE_ENDPOINT));
    }

    #[test]
    fn only_a_newer_version_is_an_update() {
        assert!(is_newer(&v("2026.10.0"), &v("2026.10.0-beta.3")));
        assert!(is_newer(&v("2026.10.0-beta.4"), &v("2026.10.0-beta.3")));
        // No downgrade from a beta to an older stable.
        assert!(!is_newer(&v("2026.9.3"), &v("2026.10.0-beta.3")));
        // Not the version that runs.
        assert!(!is_newer(&v("2026.9.3"), &v("2026.9.3")));
        // What a Windows build's rewritten version would have let through.
        assert!(is_newer(&v("2026.9.3"), &v("26.10.0-3")));
    }

    #[test]
    fn the_app_version_is_the_crate_version() {
        assert_eq!(app_version().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn a_pending_update_installs_only_as_downloaded() {
        use UpdateChannel::{Beta, Stable};
        let pending = PendingDownload {
            channel: Beta,
            version: "2026.10.0-beta.2".into(),
            bytes: Vec::new(),
        };
        assert!(pending.matches(Beta, "2026.10.0-beta.2"));
        assert!(!pending.matches(Beta, "2026.10.0-beta.3"));
        assert!(!pending.matches(Stable, "2026.10.0-beta.2"));
    }
}
