//! The one-time move of secrets out of stored connection strings (phase 5d,
//! Decision 12a), run when a writable workspace opens.
//!
//! Before phase 5a the TypeScript stripped only a URL's user-info password,
//! so a row saved then and not touched since can still hold a secret in
//! `connections.connection_string`. For each such row
//! (`connections::with_secret_in_string`):
//!
//! - **With a secret store (desktop).** The database password the driver
//!   reads (`db`) and a TablePlus `+ssh` URL's SSH password (`ssh`) move to
//!   `db:<id>` and `ssh:<id>`, outside any write transaction, so a keychain
//!   prompt never holds the write lock:
//!   - no entry: the value is written; if that fails the row is left as it
//!     is (string and flags), and the next open tries again;
//!   - an entry equal to the value (a crash between an earlier keychain
//!     write and its row update): nothing to write;
//!   - a different entry: it's kept, never overwritten, the flag is left,
//!     and the connection is listed in the notice.
//!
//!   Then, in one write transaction that reads the string again (and skips
//!   the row if another window saved it since), the stripped string and the
//!   save flags (`save_password` for a stored or equal `db`,
//!   `save_ssh_password` for `ssh`) are written in one update, and a row
//!   that lost something (`unmovable`, or a kept different entry) joins the
//!   notice list in the same transaction. If the row is left for the next
//!   open (a later keychain call failed, or the row changed), the entries
//!   this pass wrote are taken back.
//! - **Without a store (web).** Every listed row is stripped and listed,
//!   each in its own transaction that reads the string again first.
//!
//! [`STRING_SECRETS_UPGRADED_KEY`] is written once a pass leaves nothing to
//! retry; until then each open lists again, which finds only the rows left
//! behind. [`STRING_SECRETS_NOTICE_KEY`] holds the listed ids (a JSON list,
//! deduplicated), which the GUI reads and clears.
//!
//! **Scrubbing.** SQLite keeps deleted bytes until the space is reused, so
//! each row update runs with `secure_delete`, and a pass that stripped a row
//! records two more steps with it: a `VACUUM` (for older copies in pages the
//! update never touched) and a `wal_checkpoint(TRUNCATE)`. Each has its own
//! key ([`STRING_SECRETS_VACUUM_KEY`], [`STRING_SECRETS_CHECKPOINT_KEY`]),
//! cleared when it succeeds, and a failed or busy one runs again on the
//! next open (a rebuild at most [`MAX_VACUUM_ATTEMPTS`] times), apart
//! from the upgraded flag. The duration and file sizes are
//! logged. Each row changed emits a
//! `connection` event with no origin. Nothing of a string or a secret is
//! logged, put in an error or an event: ids, engines' counts and codes only.

use log::{debug, info, warn};
use seaquel_storage::{app_state, connections, split_connection_string_secret};
use seaquel_workspace::library::{DB_SECRET, SSH_SECRET};

use crate::changes::WriteOrigin;
use crate::{CoreError, Executor, StoredKind, Workspace};

/// Set (to `"1"`) once the upgrade has nothing left to do. Core's own key.
pub const STRING_SECRETS_UPGRADED_KEY: &str = "connectionStringSecretsUpgraded";
/// The ids of the connections that lost a secret they couldn't keep, as a
/// JSON list, for the GUI's one-time notice, which clears it.
pub const STRING_SECRETS_NOTICE_KEY: &str = "connectionStringSecretsNotice";
/// Set with a row's strip, cleared once a `VACUUM` has rebuilt the file:
/// until then each open tries again (best effort, apart from the upgraded
/// flag).
///
/// Its value counts the failed rebuilds; after [`MAX_VACUUM_ATTEMPTS`] the
/// rebuild isn't tried again (the checkpoint still is).
pub const STRING_SECRETS_VACUUM_KEY: &str = "connectionStringSecretsVacuum";
/// How many failed `VACUUM`s the scrub tries before giving up on it.
pub const MAX_VACUUM_ATTEMPTS: u32 = 3;
/// Set with a row's strip, cleared once `wal_checkpoint(TRUNCATE)` has
/// moved every page into the file and emptied the WAL; a busy checkpoint
/// is tried again on the next open.
pub const STRING_SECRETS_CHECKPOINT_KEY: &str = "connectionStringSecretsCheckpoint";

/// What happened to one row.
enum Outcome {
    /// Stripped (and listed or not).
    Done,
    /// Nothing to do after all (gone, or no secret any more).
    Unchanged,
    /// Left for the next open: a keychain call failed, or the row changed.
    Retry,
}

impl Workspace {
    /// Runs the upgrade (see the module docs), then any scrub a pass left
    /// pending. A read-only workspace (the CLI) never runs it. Failures are
    /// logged by code and left for the next open; the workspace opens
    /// either way. `executor` only times the scrub for the log.
    #[doc(hidden)]
    pub async fn upgrade_string_secrets(&self, executor: Option<&dyn Executor>) {
        if self.storage().is_read_only() {
            return;
        }
        if let Err(e) = self.try_upgrade_string_secrets().await {
            warn!(activity = "workspace.upgradeStringSecrets", code = e.code.as_str(); "Moving secrets out of stored connection strings failed");
        }
        if let Err(e) = self.scrub_string_secrets(executor).await {
            warn!(activity = "workspace.upgradeStringSecrets", code = e.code.as_str(); "Scrubbing the file after stripping failed");
        }
    }

    async fn try_upgrade_string_secrets(&self) -> Result<(), CoreError> {
        let st = self.storage();
        if app_state::get(st, STRING_SECRETS_UPGRADED_KEY)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let rows = connections::with_secret_in_string(st).await?;
        let (mut done, mut retry) = (0usize, 0usize);
        for row in &rows {
            match self.upgrade_row(&row.id).await {
                Ok(Outcome::Done) => done += 1,
                Ok(Outcome::Unchanged) => {}
                Ok(Outcome::Retry) => retry += 1,
                Err(e) => {
                    warn!(activity = "workspace.upgradeStringSecrets", code = e.code.as_str(); "A connection's string couldn't be upgraded");
                    retry += 1;
                }
            }
        }
        if retry == 0 {
            self.set_key(STRING_SECRETS_UPGRADED_KEY, Some("1")).await?;
        }
        if !rows.is_empty() {
            info!(activity = "workspace.upgradeStringSecrets", rows = rows.len(), done = done, retry = retry; "Moved secrets out of stored connection strings");
        }
        Ok(())
    }

    /// Writes one of Core's own `app_state` keys, and announces it.
    async fn set_key(&self, key: &str, value: Option<&str>) -> Result<(), CoreError> {
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        app_state::set_in(&mut tx, key, value).await?;
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::Storage,
            None,
            Some(vec![key.to_string()]),
            &WriteOrigin::none(),
        );
        Ok(())
    }

    /// The rebuild and checkpoint a pass that stripped rows asked for
    /// (their keys), so the old strings don't linger in free space, old
    /// pages or the WAL. The row updates already ran with `secure_delete`.
    /// Both hold the write lock, so this stays on the open path rather than
    /// running behind the app's first writes.
    async fn scrub_string_secrets(&self, executor: Option<&dyn Executor>) -> Result<(), CoreError> {
        let st = self.storage();
        // The value counts failed rebuilds; past the cap it isn't tried again.
        let failed_vacuums = app_state::get(st, STRING_SECRETS_VACUUM_KEY)
            .await?
            .map(|v| v.trim().parse::<u32>().unwrap_or(0));
        let vacuum = failed_vacuums.is_some_and(|n| n < MAX_VACUUM_ATTEMPTS);
        if failed_vacuums.is_some_and(|n| n >= MAX_VACUUM_ATTEMPTS) {
            debug!(activity = "workspace.upgradeStringSecrets", attempts = MAX_VACUUM_ATTEMPTS; "The rebuild failed too often; not trying again");
        }
        let checkpoint = app_state::get(st, STRING_SECRETS_CHECKPOINT_KEY)
            .await?
            .is_some();
        if !vacuum && !checkpoint {
            return Ok(());
        }
        let started = executor.map(Executor::monotonic);
        let bytes_before = file_bytes(st);
        let mut vacuumed = failed_vacuums.is_none();
        if vacuum {
            // The attempt is counted before it runs, so one that fails or
            // never returns (a crash) counts too; a successful one clears
            // the key. If even the count can't be written (the file is
            // locked), the rebuild would fail the same way: it isn't tried.
            let attempts = failed_vacuums.unwrap_or(0).saturating_add(1);
            match self
                .set_key(STRING_SECRETS_VACUUM_KEY, Some(&attempts.to_string()))
                .await
            {
                Err(e) => {
                    warn!(activity = "workspace.upgradeStringSecrets", code = e.code.as_str(); "Couldn't record a rebuild attempt; the next open tries again");
                }
                Ok(()) => match st.vacuum().await {
                    Ok(()) => {
                        self.set_key(STRING_SECRETS_VACUUM_KEY, None).await?;
                        vacuumed = true;
                    }
                    Err(e) => {
                        warn!(activity = "workspace.upgradeStringSecrets", code = e.code(), attempts = attempts; "Rebuilding the file failed");
                    }
                },
            }
        }
        // Always, even after a failed rebuild: the stripped pages still go
        // into the file and the WAL empties.
        let checkpointed = match st.checkpoint().await {
            Ok(true) => true,
            Ok(false) => {
                warn!(activity = "workspace.upgradeStringSecrets"; "The WAL checkpoint was busy; the next open tries again");
                false
            }
            Err(e) => {
                warn!(activity = "workspace.upgradeStringSecrets", code = e.code(); "The WAL checkpoint failed; the next open tries again");
                false
            }
        };
        if checkpointed && checkpoint {
            self.set_key(STRING_SECRETS_CHECKPOINT_KEY, None).await?;
        }
        let ms = match (started, executor) {
            (Some(start), Some(ex)) => ex.monotonic().saturating_sub(start).as_millis() as u64,
            _ => 0,
        };
        info!(activity = "workspace.upgradeStringSecrets", vacuumed = vacuumed, checkpointed = checkpointed, duration_ms = ms, bytes_before = bytes_before, bytes_after = file_bytes(st); "Scrubbed the file after stripping");
        Ok(())
    }

    async fn upgrade_row(&self, id: &str) -> Result<Outcome, CoreError> {
        let st = self.storage();
        // 1. The row, on the pool (no write lock held).
        let Some(row) = connections::get(st, id).await? else {
            return Ok(Outcome::Unchanged);
        };
        let Some(string) = row.connection_string.clone() else {
            return Ok(Outcome::Unchanged);
        };
        let Some(split) = split_connection_string_secret(&string) else {
            return Ok(Outcome::Unchanged);
        };

        // 2. The keychain, outside any transaction.
        let mut listed = split.unmovable;
        let (mut save_password, mut save_ssh_password) = (false, false);
        // The entries this pass wrote: taken back if the row isn't updated.
        let mut written: Vec<(&str, &str)> = Vec::new();
        for (prefix, value) in [
            (DB_SECRET, split.db.as_deref()),
            (SSH_SECRET, split.ssh.as_deref()),
        ] {
            match self.keychain_step(id, value, prefix).await {
                step @ (Step::Written | Step::Matched) => {
                    if matches!(step, Step::Written) {
                        written.push((prefix, value.unwrap_or_default()));
                    }
                    if prefix == DB_SECRET {
                        save_password = true;
                    } else {
                        save_ssh_password = true;
                    }
                }
                Step::KeptOther | Step::NoStore => listed = true,
                Step::Nothing => {}
                Step::Failed => {
                    self.take_back(id, &written).await;
                    return Ok(Outcome::Retry);
                }
            }
        }

        // 3. One write transaction: read again, then strip, flag and list.
        let updated = self
            .strip_row(
                id,
                &string,
                &split.stripped,
                save_password,
                save_ssh_password,
                listed,
            )
            .await;
        if !matches!(updated, Ok(Outcome::Done)) {
            self.take_back(id, &written).await;
        }
        updated
    }

    async fn strip_row(
        &self,
        id: &str,
        string: &str,
        stripped: &str,
        save_password: bool,
        save_ssh_password: bool,
        listed: bool,
    ) -> Result<Outcome, CoreError> {
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let Some(mut current) = connections::get(&mut tx, id).await? else {
            return Ok(Outcome::Unchanged);
        };
        if current.connection_string.as_deref() != Some(string) {
            // Another window saved the row since step 1; Core's writes strip
            // it, and the next open lists it again if they didn't.
            return Ok(Outcome::Retry);
        }
        // The old string's bytes are zeroed where the update frees them; the
        // connection goes back to the pool with it off.
        tx.secure_delete(true).await?;
        current.connection_string = Some(stripped.to_string()).filter(|s| !s.is_empty());
        current.save_password |= save_password;
        current.save_ssh_password |= save_ssh_password;
        connections::update(&mut tx, &current).await?;
        tx.secure_delete(false).await?;
        if listed {
            let notice = app_state::get(&mut tx, STRING_SECRETS_NOTICE_KEY).await?;
            let mut ids: Vec<String> = notice
                .as_deref()
                .and_then(|n| serde_json::from_str(n).ok())
                .unwrap_or_default();
            if !ids.iter().any(|i| i == id) {
                ids.push(id.to_string());
                let json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
                app_state::set_in(&mut tx, STRING_SECRETS_NOTICE_KEY, Some(&json)).await?;
            }
        }
        // The scrub this strip asks for, recorded with it. A rebuild already
        // pending keeps its failed-attempt count.
        if app_state::get(&mut tx, STRING_SECRETS_VACUUM_KEY)
            .await?
            .is_none()
        {
            app_state::set_in(&mut tx, STRING_SECRETS_VACUUM_KEY, Some("0")).await?;
        }
        app_state::set_in(&mut tx, STRING_SECRETS_CHECKPOINT_KEY, Some("1")).await?;
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::Connection,
            Some(current.project_id),
            Some(vec![id.to_string()]),
            &WriteOrigin::none(),
        );
        Ok(Outcome::Done)
    }

    /// Deletes the keychain entries this pass wrote for a row it then left
    /// for the next open, unless something else has written them since.
    #[cfg(feature = "secrets")]
    async fn take_back(&self, id: &str, written: &[(&str, &str)]) {
        let Some(store) = self.secrets() else {
            return;
        };
        for (prefix, value) in written {
            let key = format!("{prefix}{id}");
            match store.get(&key).await {
                Ok(Some(now)) if now == *value => {
                    if let Err(e) = store.delete(&key).await {
                        warn!(activity = "workspace.upgradeStringSecrets", code = e.code(); "Taking back a keychain entry failed");
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(activity = "workspace.upgradeStringSecrets", code = e.code(); "Taking back a keychain entry failed");
                }
            }
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn take_back(&self, _id: &str, _written: &[(&str, &str)]) {}

    /// Moves one secret (`value`, under `prefix` + `id`) to the keychain,
    /// never overwriting an entry that's there.
    #[cfg(feature = "secrets")]
    async fn keychain_step(&self, id: &str, value: Option<&str>, prefix: &str) -> Step {
        let Some(value) = value else {
            return Step::Nothing;
        };
        let Some(store) = self.secrets() else {
            return Step::NoStore;
        };
        let key = format!("{prefix}{id}");
        match store.get(&key).await {
            Err(e) => {
                warn!(activity = "workspace.upgradeStringSecrets", code = e.code(); "Reading the keychain failed");
                Step::Failed
            }
            Ok(Some(existing)) if existing == value => Step::Matched,
            Ok(Some(_)) => Step::KeptOther,
            Ok(None) => match store.set(&key, value).await {
                Ok(()) => Step::Written,
                Err(e) => {
                    warn!(activity = "workspace.upgradeStringSecrets", code = e.code(); "Writing the keychain failed");
                    Step::Failed
                }
            },
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn keychain_step(&self, _id: &str, value: Option<&str>, _prefix: &str) -> Step {
        match value {
            Some(_) => Step::NoStore,
            None => Step::Nothing,
        }
    }
}

/// The metadata file's size and its WAL's, for the log (sizes only).
#[cfg(not(target_arch = "wasm32"))]
fn file_bytes(st: &seaquel_storage::Storage) -> u64 {
    let path = st.path();
    let wal = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
    ));
    [path, wal.as_path()]
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// The browser's metadata file has no file system behind it (phase 8): its
/// size is the in-memory database's, read through a snapshot. Only the
/// scrub after a strip logs it, which a browser file made fresh never
/// needs; 0 when the snapshot can't be taken.
#[cfg(target_arch = "wasm32")]
fn file_bytes(st: &seaquel_storage::Storage) -> u64 {
    st.snapshot().map_or(0, |image| image.len() as u64)
}

/// What the keychain step did with one secret.
#[cfg_attr(not(feature = "secrets"), allow(dead_code))]
enum Step {
    /// The row had none of this kind.
    Nothing,
    /// This pass wrote it to the keychain.
    Written,
    /// The keychain already held the same value.
    Matched,
    /// A different entry was there and was kept.
    KeptOther,
    /// A keychain call failed: leave the row for the next open.
    Failed,
    /// No store (web): the secret is lost by stripping.
    NoStore,
}
