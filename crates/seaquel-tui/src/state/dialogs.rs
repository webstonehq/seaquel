//! The connect dialogs (Decision 16): the password prompt with its "Save
//! password" box, the SSH host-key trust prompt, a failed connect (with a
//! retry when a typed password could fix it) and a notice. Each carries the
//! connect it belongs to ([`Pending`]): the connection, the secrets typed so
//! far, which to save and an approved host key.
//!
//! Core's error codes are worded here ([`problem`]), never in the view.

use std::collections::BTreeSet;
use std::fmt;

use super::secrets::{Secret, SecretKind, Typed};
use super::text;

/// An error from a Core call: its code and message. The message can name a
/// host or a file, so `Debug` shows the code only.
#[derive(Clone, PartialEq, Eq)]
pub struct CallError {
    pub code: String,
    pub message: String,
}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CallError({})", self.code)
    }
}

impl CallError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> CallError {
        CallError {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A connect on its way: what the prompts gathered so far.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    pub connection_id: String,
    pub typed: Typed,
    /// The typed secrets to save once the connect succeeds.
    pub save: BTreeSet<SecretKind>,
    /// The host-key fingerprint the user trusted.
    pub trust: Option<String>,
}

/// "Password for …" with a masked input and a "Save password" box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordPrompt {
    pub pending: Pending,
    pub kind: SecretKind,
    pub input: Secret,
    pub save: bool,
    /// Whether "Save password" can be ticked: not while the secret store
    /// isn't available (probe F4).
    pub can_save: bool,
    /// Why it's asked again (a refused password), if it is.
    pub reason: Option<&'static str>,
}

/// An unknown SSH host key: trust it and connect, or don't.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustPrompt {
    pub pending: Pending,
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
}

impl fmt::Debug for TrustPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrustPrompt")
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

/// A failed connect, worded.
#[derive(Clone, PartialEq, Eq)]
pub struct Problem {
    pub code: String,
    pub title: &'static str,
    pub message: String,
    /// `r` asks for this secret and connects again.
    pub retry: Option<(SecretKind, Pending)>,
}

impl fmt::Debug for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Problem")
            .field("code", &self.code)
            .field("retry", &self.retry.as_ref().map(|(k, _)| k))
            .finish_non_exhaustive()
    }
}

/// A message to acknowledge.
#[derive(Clone, PartialEq, Eq)]
pub struct Notice(pub String);

impl fmt::Debug for Notice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Notice")
    }
}

/// The `SHA256:…` fingerprint in an `UNKNOWN_HOST_KEY` message: what
/// follows the last `Fingerprint: ` (Core's label), the base64 after
/// `SHA256:` only, since Core may wrap the SSH layer's message (a `)`).
/// An earlier `SHA256:` (in a host name) never counts. (A structured field
/// is a follow-up.)
pub fn fingerprint(message: &str) -> Option<String> {
    let label = "Fingerprint: SHA256:";
    let at = message.rfind(label)? + label.len();
    let rest = &message[at..];
    let len = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
        .unwrap_or(rest.len());
    (len > 0).then(|| format!("SHA256:{}", &rest[..len]))
}

/// A failed connect as the problem dialog words it: Core's message under a
/// title that says what to do.
pub fn problem(error: &CallError, retry: Option<(SecretKind, Pending)>) -> Problem {
    let title = match error.code.as_str() {
        "SECRET_UNREADABLE"
        | "SECRET_STORE_ERROR"
        | "SECRET_STORE_UNAVAILABLE"
        | "NO_SECRET_STORE" => text::problem_title_keychain(text::Store::here()),
        "HOST_KEY_MISMATCH" => text::PROBLEM_TITLE_HOST_KEY,
        "ENGINE_NOT_AVAILABLE" => text::PROBLEM_TITLE_ENGINE,
        "CONNECTION_NOT_FOUND" => text::PROBLEM_TITLE_GONE,
        _ => text::PROBLEM_TITLE_CONNECT,
    };
    Problem {
        code: error.code.clone(),
        title,
        message: error.message.clone(),
        retry,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_comes_from_core_s_message() {
        let message = "The host key for 127.0.0.1:2222 is not in known_hosts.\n\
                       Fingerprint: SHA256:abcDEF123+/xyz";
        assert_eq!(
            fingerprint(message).as_deref(),
            Some("SHA256:abcDEF123+/xyz")
        );
        assert_eq!(fingerprint("no fingerprint here"), None);
        // Core wraps the SSH layer's message: what follows isn't part of it.
        assert_eq!(
            fingerprint("SSH tunnel failed (Fingerprint: SHA256:xBaw+q/E=)").as_deref(),
            Some("SHA256:xBaw+q/E=")
        );
        assert_eq!(fingerprint("Fingerprint: SHA256:"), None);
        // Only what follows "Fingerprint: " counts: an earlier SHA256: (a
        // host or user name could hold one) doesn't.
        assert_eq!(
            fingerprint("host SHA256:decoy is not in known_hosts.\nFingerprint: SHA256:real")
                .as_deref(),
            Some("SHA256:real")
        );
        assert_eq!(fingerprint("SHA256:alone, no label"), None);
        assert_eq!(fingerprint("Fingerprint: MD5:aa"), None);
    }

    #[test]
    fn each_code_is_worded() {
        let words = |code: &str| problem(&CallError::new(code, "core says"), None);
        let unreadable = words("SECRET_UNREADABLE");
        assert_eq!(
            unreadable.title,
            text::problem_title_keychain(text::Store::here())
        );
        assert!(unreadable.message.contains("core says"));
        let mismatch = words("HOST_KEY_MISMATCH");
        assert_eq!(mismatch.title, text::PROBLEM_TITLE_HOST_KEY);
        assert!(mismatch.retry.is_none());
        let engine = words("ENGINE_NOT_AVAILABLE");
        assert_eq!(engine.title, text::PROBLEM_TITLE_ENGINE);
        let other = words("CONNECTION_ERROR");
        assert_eq!(other.title, text::PROBLEM_TITLE_CONNECT);
        assert_eq!(other.code, "CONNECTION_ERROR");
        let gone = words("CONNECTION_NOT_FOUND");
        assert_eq!(gone.title, text::PROBLEM_TITLE_GONE);
    }

    #[test]
    fn debug_shows_no_message_host_or_password() {
        let mut pending = Pending {
            connection_id: "conn-1".into(),
            ..Pending::default()
        };
        pending.typed.set(SecretKind::Db, Secret::new("pw-marker"));
        let p = problem(
            &CallError::new("CONNECTION_ERROR", "db.host-marker refused"),
            Some((SecretKind::Db, pending.clone())),
        );
        let t = TrustPrompt {
            pending: pending.clone(),
            host: "bastion-marker".into(),
            port: 22,
            fingerprint: "SHA256:x".into(),
        };
        let prompt = PasswordPrompt {
            pending,
            kind: SecretKind::Db,
            input: Secret::new("typed-marker"),
            save: true,
            can_save: true,
            reason: None,
        };
        let text = format!(
            "{p:?} {t:?} {prompt:?} {:?} {:?}",
            Notice("notice-marker".into()),
            CallError::new("X", "msg-marker")
        );
        assert!(!text.contains("marker"), "{text}");
    }
}
