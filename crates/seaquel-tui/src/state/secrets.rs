//! Passwords the user types. A typed password
//! lives in the model only while its connection does: it's sent to Core as
//! `SuppliedSecrets` for the connect, saved only through Core's
//! `connectionUpdate` when the user ticked "Save password" and the connect
//! succeeded, and dropped on disconnect. `Debug` never shows one: a
//! [`Secret`] prints `<password>`, and [`Typed`] names only which are set.

use std::fmt;

use seaquel_core::SuppliedSecrets;

/// A typed password.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

/// Overwritten when dropped (`zeroize`). Best effort: a `String` that grew
/// while typing may have left its old buffer behind, and Core's own copies
/// (`SuppliedSecrets`, the keychain call) aren't ours to wipe.
impl Drop for Secret {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.0.zeroize();
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<password>")
    }
}

impl Secret {
    pub fn new(value: impl Into<String>) -> Secret {
        Secret(value.into())
    }

    /// An empty one with room for a typical password, so typing seldom
    /// reallocates (and leaves a copy behind).
    pub fn with_room() -> Secret {
        Secret(String::with_capacity(128))
    }

    /// The text, for Core only.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn push(&mut self, c: char) {
        self.0.push(c);
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// One `•` per character, as the prompt shows it.
    pub fn masked(&self) -> String {
        "•".repeat(self.0.chars().count())
    }
}

/// Which secret a prompt asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SecretKind {
    /// The database password (`db:<id>`, `savePassword`).
    Db,
    /// The SSH password (`ssh:<id>`, `saveSshPassword`).
    Ssh,
    /// The SSH key's passphrase (`ssh-key:<id>`, `saveSshKeyPassphrase`).
    SshKey,
}

/// The secrets typed for the connection being connected or connected.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Typed {
    pub db: Option<Secret>,
    pub ssh: Option<Secret>,
    pub ssh_key: Option<Secret>,
}

impl fmt::Debug for Typed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Typed")
            .field("db", &self.db.is_some())
            .field("ssh", &self.ssh.is_some())
            .field("ssh_key", &self.ssh_key.is_some())
            .finish()
    }
}

impl Typed {
    fn slot(&mut self, kind: SecretKind) -> &mut Option<Secret> {
        match kind {
            SecretKind::Db => &mut self.db,
            SecretKind::Ssh => &mut self.ssh,
            SecretKind::SshKey => &mut self.ssh_key,
        }
    }

    pub fn get(&self, kind: SecretKind) -> Option<&Secret> {
        match kind {
            SecretKind::Db => self.db.as_ref(),
            SecretKind::Ssh => self.ssh.as_ref(),
            SecretKind::SshKey => self.ssh_key.as_ref(),
        }
    }

    pub fn set(&mut self, kind: SecretKind, secret: Secret) {
        *self.slot(kind) = Some(secret);
    }

    pub fn forget(&mut self, kind: SecretKind) {
        *self.slot(kind) = None;
    }

    pub fn is_empty(&self) -> bool {
        self.db.is_none() && self.ssh.is_none() && self.ssh_key.is_none()
    }

    /// What Core's connect takes: every typed secret wins over the store.
    pub fn supplied(&self) -> SuppliedSecrets {
        let text = |s: &Option<Secret>| s.as_ref().map(|s| s.expose().to_string());
        SuppliedSecrets {
            db: text(&self.db),
            ssh: text(&self.ssh),
            ssh_key: text(&self.ssh_key),
        }
    }

    /// Only the `kinds` given (what the user ticked to save).
    pub fn only(&self, kinds: &[SecretKind]) -> Typed {
        let mut out = Typed::default();
        for kind in kinds {
            if let Some(secret) = self.get(*kind) {
                out.set(*kind, secret.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_a_password() {
        let mut typed = Typed::default();
        typed.set(SecretKind::Db, Secret::new("hunter2-marker"));
        typed.set(SecretKind::SshKey, Secret::new("phrase-marker"));
        let text = format!("{typed:?} {:?}", typed.get(SecretKind::Db));
        assert!(!text.contains("marker"), "{text}");
        assert!(text.contains("<password>"), "{text}");
        assert!(
            text.contains("db: true") && text.contains("ssh: false"),
            "{text}"
        );
        let supplied = format!("{:?}", typed.supplied());
        assert!(!supplied.contains("marker"), "{supplied}");
    }

    #[test]
    fn typed_secrets_become_supplied_secrets() {
        let mut typed = Typed::default();
        assert!(typed.is_empty());
        typed.set(SecretKind::Db, Secret::new("pw"));
        typed.set(SecretKind::Ssh, Secret::new("sshpw"));
        let s = typed.supplied();
        assert_eq!(
            (s.db.as_deref(), s.ssh.as_deref(), s.ssh_key.as_deref()),
            (Some("pw"), Some("sshpw"), None)
        );
        let only = typed.only(&[SecretKind::Ssh]);
        assert_eq!(only.get(SecretKind::Db), None);
        assert_eq!(only.get(SecretKind::Ssh).map(Secret::expose), Some("sshpw"));
        typed.forget(SecretKind::Db);
        assert_eq!(typed.get(SecretKind::Db), None);
        assert!(!typed.is_empty());
    }

    #[test]
    fn the_prompt_masks_and_edits() {
        let mut s = Secret::default();
        for c in "pä$".chars() {
            s.push(c);
        }
        assert_eq!(s.masked(), "•••");
        s.pop();
        assert_eq!(s.expose(), "pä");
    }
}
