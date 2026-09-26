//! The credential chain libgit2 asks when a remote wants authentication.
//!
//! The order is the one `src-tauri`'s `git.rs` had:
//!
//! 1. when SSH keys are allowed:
//!    1. the SSH agent, only when the URL names a user;
//!    2. the given key file (its result is final, even a failure);
//!    3. `<home>/.ssh/id_ed25519`, then `<home>/.ssh/id_rsa`, if they exist;
//! 2. when a plaintext user and password are allowed and both are given,
//!    those;
//! 3. libgit2's default credential.
//!
//! libgit2 asks again, without limit, each time the remote turns a
//! credential down (over SSH it keeps the repo locked meanwhile), and the
//! Tauri command answered with the same credential every time. Now each
//! call goes on down the chain past what was already handed out: after a
//! rejected agent the given key, after a rejected `id_ed25519` the `id_rsa`,
//! and so on. It answers with an error once the chain is used up, when the
//! given key was already tried (its result stays final), or after
//! [`MAX_ATTEMPTS`] credentials.
//!
//! [`resolve`] walks it over a [`CredentialMaker`], so tests can check the
//! order with a fake instead of real keys and a real agent.

use std::path::Path;

use git2::{Cred, CredentialType, RemoteCallbacks};

use crate::GitCredentials;

/// The most credentials one operation hands libgit2: enough for the agent,
/// both default keys and a user/password.
pub(crate) const MAX_ATTEMPTS: usize = 4;

/// One link of the chain, to remember what was already handed out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Agent,
    GivenKey,
    DefaultKey(&'static str),
    UserPass,
    Fallback,
}

/// What one operation's credential callback already handed out.
#[derive(Debug, Default)]
pub(crate) struct Attempts {
    tried: Vec<Step>,
}

impl Attempts {
    fn tried(&self, step: Step) -> bool {
        self.tried.contains(&step)
    }

    fn hand_out<C>(&mut self, step: Step, cred: C) -> Result<C, git2::Error> {
        self.tried.push(step);
        Ok(cred)
    }
}

fn exhausted(why: &str) -> git2::Error {
    git2::Error::from_str(&format!("authentication failed: {why}"))
}

/// Builds the credential objects [`resolve`] tries. [`Git2Creds`] makes real
/// ones; tests use a fake that records the calls.
pub(crate) trait CredentialMaker {
    type Cred;
    fn agent(&mut self, username: &str) -> Result<Self::Cred, git2::Error>;
    fn ssh_key(
        &mut self,
        username: &str,
        key: &Path,
        passphrase: Option<&str>,
    ) -> Result<Self::Cred, git2::Error>;
    fn userpass(&mut self, username: &str, password: &str) -> Result<Self::Cred, git2::Error>;
    fn fallback(&mut self) -> Result<Self::Cred, git2::Error>;
}

/// The chain from the module docs, skipping what `attempts` says was already
/// handed out. `home` is where the default keys are looked for; `None`
/// skips them.
pub(crate) fn resolve<M: CredentialMaker>(
    maker: &mut M,
    attempts: &mut Attempts,
    creds: Option<&GitCredentials>,
    home: Option<&Path>,
    username_from_url: Option<&str>,
    allowed: CredentialType,
) -> Result<M::Cred, git2::Error> {
    if attempts.tried.len() >= MAX_ATTEMPTS {
        return Err(exhausted("too many attempts"));
    }

    if allowed.contains(CredentialType::SSH_KEY) {
        if let Some(username) = username_from_url {
            if !attempts.tried(Step::Agent) {
                if let Ok(cred) = maker.agent(username) {
                    return attempts.hand_out(Step::Agent, cred);
                }
            }
        }

        let username = username_from_url.unwrap_or("git");
        if let Some(key_path) = creds.and_then(|c| c.ssh_key_path.as_deref()) {
            if attempts.tried(Step::GivenKey) {
                return Err(exhausted("the SSH key was rejected"));
            }
            let passphrase = creds.and_then(|c| c.ssh_passphrase.as_deref());
            let cred = maker.ssh_key(username, Path::new(key_path), passphrase)?;
            return attempts.hand_out(Step::GivenKey, cred);
        }

        if let Some(home) = home {
            for name in ["id_ed25519", "id_rsa"] {
                let step = Step::DefaultKey(name);
                let key = home.join(".ssh").join(name);
                if !attempts.tried(step) && key.exists() {
                    if let Ok(cred) = maker.ssh_key(username, &key, None) {
                        return attempts.hand_out(step, cred);
                    }
                }
            }
        }
    }

    if allowed.contains(CredentialType::USER_PASS_PLAINTEXT) {
        if let Some(GitCredentials {
            username: Some(username),
            password: Some(password),
            ..
        }) = creds
        {
            if attempts.tried(Step::UserPass) {
                return Err(exhausted("the username or password was rejected"));
            }
            let cred = maker.userpass(username, password)?;
            return attempts.hand_out(Step::UserPass, cred);
        }
    }

    if attempts.tried(Step::Fallback) {
        return Err(exhausted("no more credentials to try"));
    }
    let cred = maker.fallback()?;
    attempts.hand_out(Step::Fallback, cred)
}

/// Real libgit2 credentials.
struct Git2Creds;

impl CredentialMaker for Git2Creds {
    type Cred = Cred;

    fn agent(&mut self, username: &str) -> Result<Cred, git2::Error> {
        Cred::ssh_key_from_agent(username)
    }

    fn ssh_key(
        &mut self,
        username: &str,
        key: &Path,
        passphrase: Option<&str>,
    ) -> Result<Cred, git2::Error> {
        Cred::ssh_key(username, None, key, passphrase)
    }

    fn userpass(&mut self, username: &str, password: &str) -> Result<Cred, git2::Error> {
        Cred::userpass_plaintext(username, password)
    }

    fn fallback(&mut self) -> Result<Cred, git2::Error> {
        Cred::default()
    }
}

/// Remote callbacks that answer credential requests with the chain.
pub(crate) fn callbacks<'a>(
    creds: Option<GitCredentials>,
    home: Option<std::path::PathBuf>,
) -> RemoteCallbacks<'a> {
    let mut callbacks = RemoteCallbacks::new();
    let mut attempts = Attempts::default();
    callbacks.credentials(move |_url, username_from_url, allowed| {
        resolve(
            &mut Git2Creds,
            &mut attempts,
            creds.as_ref(),
            home.as_deref(),
            username_from_url,
            allowed,
        )
    });
    callbacks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records each attempt as a string; `agent_ok` and `key_ok` pick
    /// whether those succeed.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        agent_ok: bool,
        key_ok: bool,
    }

    fn fail() -> git2::Error {
        git2::Error::from_str("fake failure")
    }

    impl CredentialMaker for Fake {
        type Cred = String;

        fn agent(&mut self, username: &str) -> Result<String, git2::Error> {
            let call = format!("agent:{username}");
            self.calls.push(call.clone());
            self.agent_ok.then_some(call).ok_or_else(fail)
        }

        fn ssh_key(
            &mut self,
            username: &str,
            key: &Path,
            passphrase: Option<&str>,
        ) -> Result<String, git2::Error> {
            let name = key.file_name().unwrap().to_string_lossy();
            let call = format!("key:{username}:{name}:{}", passphrase.unwrap_or("-"));
            self.calls.push(call.clone());
            self.key_ok.then_some(call).ok_or_else(fail)
        }

        fn userpass(&mut self, username: &str, password: &str) -> Result<String, git2::Error> {
            let call = format!("userpass:{username}:{password}");
            self.calls.push(call.clone());
            Ok(call)
        }

        fn fallback(&mut self) -> Result<String, git2::Error> {
            self.calls.push("default".into());
            Ok("default".into())
        }
    }

    const SSH: CredentialType = CredentialType::SSH_KEY;

    fn all() -> CredentialType {
        CredentialType::SSH_KEY | CredentialType::USER_PASS_PLAINTEXT
    }

    /// A temp home with the given default keys under `.ssh`.
    fn home_with(keys: &[&str]) -> tempfile::TempDir {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".ssh")).unwrap();
        for key in keys {
            std::fs::write(home.path().join(".ssh").join(key), "fake key").unwrap();
        }
        home
    }

    fn key_creds() -> GitCredentials {
        GitCredentials {
            ssh_key_path: Some("/keys/deploy_key".into()),
            ssh_passphrase: Some("pp".into()),
            ..Default::default()
        }
    }

    fn user_creds() -> GitCredentials {
        GitCredentials {
            username: Some("alice".into()),
            password: Some("token".into()),
            ..Default::default()
        }
    }

    #[test]
    fn agent_first_when_the_url_has_a_user() {
        let mut fake = Fake {
            agent_ok: true,
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&key_creds()),
            None,
            Some("git"),
            SSH,
        )
        .unwrap();
        assert_eq!(got, "agent:git");
        assert_eq!(fake.calls, ["agent:git"]);
    }

    #[test]
    fn no_agent_without_a_user_in_the_url() {
        let mut fake = Fake {
            agent_ok: true,
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&key_creds()),
            None,
            None,
            SSH,
        )
        .unwrap();
        assert_eq!(got, "key:git:deploy_key:pp");
        assert_eq!(fake.calls, ["key:git:deploy_key:pp"]);
    }

    #[test]
    fn given_key_after_a_failed_agent_and_its_failure_is_final() {
        let home = home_with(&["id_ed25519", "id_rsa"]);
        let mut fake = Fake::default(); // agent and keys fail
        let err = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&GitCredentials {
                ssh_key_path: Some("/keys/deploy_key".into()),
                ..user_creds()
            }),
            Some(home.path()),
            Some("git"),
            all(),
        )
        .unwrap_err();
        assert_eq!(err.message(), "fake failure");
        // Neither the default keys nor the user/password are tried.
        assert_eq!(fake.calls, ["agent:git", "key:git:deploy_key:-"]);
    }

    #[test]
    fn default_keys_in_the_passed_home_ed25519_first() {
        let home = home_with(&["id_ed25519", "id_rsa"]);
        let mut fake = Fake {
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            None,
            Some(home.path()),
            Some("git"),
            SSH,
        )
        .unwrap();
        assert_eq!(got, "key:git:id_ed25519:-");
        assert_eq!(fake.calls, ["agent:git", "key:git:id_ed25519:-"]);
    }

    #[test]
    fn rsa_when_there_is_no_ed25519() {
        let home = home_with(&["id_rsa"]);
        let mut fake = Fake {
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            None,
            Some(home.path()),
            None,
            SSH,
        )
        .unwrap();
        assert_eq!(got, "key:git:id_rsa:-");
    }

    #[test]
    fn failed_default_keys_fall_through_to_user_and_password() {
        let home = home_with(&["id_ed25519", "id_rsa"]);
        let mut fake = Fake::default();
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&user_creds()),
            Some(home.path()),
            Some("git"),
            all(),
        )
        .unwrap();
        assert_eq!(got, "userpass:alice:token");
        assert_eq!(
            fake.calls,
            [
                "agent:git",
                "key:git:id_ed25519:-",
                "key:git:id_rsa:-",
                "userpass:alice:token"
            ]
        );
    }

    #[test]
    fn missing_default_keys_are_skipped() {
        let home = home_with(&[]);
        let mut fake = Fake {
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&user_creds()),
            Some(home.path()),
            None,
            all(),
        )
        .unwrap();
        assert_eq!(got, "userpass:alice:token");
        assert_eq!(fake.calls, ["userpass:alice:token"]);
    }

    #[test]
    fn no_home_skips_the_default_keys() {
        let mut fake = Fake {
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(&mut fake, &mut Attempts::default(), None, None, None, SSH).unwrap();
        assert_eq!(got, "default");
        assert_eq!(fake.calls, ["default"]);
    }

    #[test]
    fn https_only_goes_straight_to_user_and_password() {
        let home = home_with(&["id_ed25519"]);
        let mut fake = Fake {
            agent_ok: true,
            key_ok: true,
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&GitCredentials {
                ssh_key_path: Some("/keys/deploy_key".into()),
                ..user_creds()
            }),
            Some(home.path()),
            Some("alice"),
            CredentialType::USER_PASS_PLAINTEXT,
        )
        .unwrap();
        assert_eq!(got, "userpass:alice:token");
        assert_eq!(fake.calls, ["userpass:alice:token"]);
    }

    #[test]
    fn user_without_password_falls_back_to_default() {
        let mut fake = Fake::default();
        let creds = GitCredentials {
            username: Some("alice".into()),
            ..Default::default()
        };
        let got = resolve(
            &mut fake,
            &mut Attempts::default(),
            Some(&creds),
            None,
            None,
            CredentialType::USER_PASS_PLAINTEXT,
        )
        .unwrap();
        assert_eq!(got, "default");
    }

    // ── Repeated asks: libgit2 calls again after each rejection ──

    /// Asks the chain `times` times with one `Attempts`, as one operation's
    /// callback does, and gives each answer (`Err` as "err: <message>").
    fn ask_repeatedly(
        fake: &mut Fake,
        creds: Option<&GitCredentials>,
        home: Option<&Path>,
        user: Option<&str>,
        allowed: CredentialType,
        times: usize,
    ) -> Vec<String> {
        let mut attempts = Attempts::default();
        (0..times)
            .map(|_| {
                resolve(fake, &mut attempts, creds, home, user, allowed)
                    .unwrap_or_else(|e| format!("err: {}", e.message()))
            })
            .collect()
    }

    #[test]
    fn a_rejected_credential_is_never_handed_out_again() {
        let home = home_with(&["id_ed25519", "id_rsa"]);
        let mut fake = Fake {
            agent_ok: true,
            key_ok: true,
            ..Default::default()
        };
        let answers = ask_repeatedly(
            &mut fake,
            Some(&user_creds()),
            Some(home.path()),
            Some("git"),
            all(),
            6,
        );
        assert_eq!(
            answers,
            [
                "agent:git",
                "key:git:id_ed25519:-",
                "key:git:id_rsa:-",
                "userpass:alice:token",
                "err: authentication failed: too many attempts",
                "err: authentication failed: too many attempts",
            ]
        );
    }

    #[test]
    fn a_rejected_given_key_is_final() {
        let home = home_with(&["id_ed25519"]);
        let mut fake = Fake {
            key_ok: true,
            ..Default::default()
        };
        let answers = ask_repeatedly(
            &mut fake,
            Some(&GitCredentials {
                ssh_key_path: Some("/keys/deploy_key".into()),
                ..user_creds()
            }),
            Some(home.path()),
            None,
            all(),
            3,
        );
        assert_eq!(
            answers,
            [
                "key:git:deploy_key:-",
                "err: authentication failed: the SSH key was rejected",
                "err: authentication failed: the SSH key was rejected",
            ]
        );
    }

    #[test]
    fn a_rejected_password_is_not_sent_again() {
        let mut fake = Fake::default();
        let answers = ask_repeatedly(
            &mut fake,
            Some(&user_creds()),
            None,
            Some("alice"),
            CredentialType::USER_PASS_PLAINTEXT,
            3,
        );
        assert_eq!(
            answers,
            [
                "userpass:alice:token",
                "err: authentication failed: the username or password was rejected",
                "err: authentication failed: the username or password was rejected",
            ]
        );
        assert_eq!(fake.calls, ["userpass:alice:token"]);
    }

    #[test]
    fn the_default_credential_is_handed_out_once() {
        let mut fake = Fake::default();
        let answers = ask_repeatedly(&mut fake, None, None, None, SSH, 3);
        assert_eq!(
            answers,
            [
                "default",
                "err: authentication failed: no more credentials to try",
                "err: authentication failed: no more credentials to try",
            ]
        );
    }
}
