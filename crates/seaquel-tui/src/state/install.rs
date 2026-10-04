//! The DuckDB helper's install dialog (the DuckDB helper plan, Q5 A,
//! Decision 12, Task 7). A connect Core answers with `ENGINE_NOT_INSTALLED`
//! opens it, keeping the connect it belongs to ([`Pending`], with the
//! secrets typed for it). It looks up the download's size first (one
//! request for the release metadata), asks, then shows the download's
//! progress; Esc at any step returns to the picker (a download in flight is
//! dropped, which leaves nothing behind). A finished install connects again
//! with the same pending connect; a failure is worded here with a Retry.
//!
//! Pure like the rest of the model: the lookup is [`Effect::CheckDuckdb`],
//! the download [`Effect::InstallDuckdb`] (dropped by
//! [`Effect::CancelInstall`]), their answers [`Msg::DuckdbOffer`],
//! [`Msg::InstallProgress`] and [`Msg::Installed`].
//!
//! [`Msg::DuckdbOffer`]: super::app::Msg::DuckdbOffer
//! [`Msg::InstallProgress`]: super::app::Msg::InstallProgress
//! [`Msg::Installed`]: super::app::Msg::Installed

use std::fmt;

use super::app::{Effect, Modal, Model};
use super::dialogs::{CallError, Pending};
use super::keymap::BarContext;
use super::log::Tag;
use super::picker;
use super::text;

/// What the release offers: the download's size, and whether the helper
/// already there sits in a folder that isn't private (`Unsafe`), which an
/// install fixes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    /// Compressed bytes.
    pub size: u64,
    pub repair: bool,
}

/// Which step a Retry repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The size lookup.
    Check,
    /// The download.
    Download,
}

/// A failed lookup or download, worded.
#[derive(Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub title: &'static str,
    /// What to do about it.
    pub hint: &'static str,
    /// Core's message (no URL or path: `release_asset` keeps them out).
    pub message: String,
    /// The step `r` repeats; `None` when a retry can't help
    /// (`NOT_SUPPORTED`: no download for this platform).
    pub retry: Option<Step>,
}

impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Failure")
            .field("code", &self.code)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stage {
    /// Reading the release metadata for the size.
    Checking,
    /// "DuckDB support is a separate download of … Download now?"
    Ask(Offer),
    /// Compressed bytes received of `total`.
    Downloading {
        bytes: u64,
        total: u64,
    },
    Failed(Failure),
}

/// The install dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallDialog {
    /// The connect that found DuckDB support missing.
    pub pending: Pending,
    /// The lookup's or download's op: a late answer to an older one (or
    /// to one given up) is dropped.
    pub op: u64,
    pub stage: Stage,
}

impl InstallDialog {
    pub fn bar_context(&self) -> BarContext {
        match self.stage {
            Stage::Checking => BarContext::InstallChecking,
            Stage::Ask(_) => BarContext::InstallAsk,
            Stage::Downloading { .. } => BarContext::InstallDownloading,
            Stage::Failed(Failure { retry: None, .. }) => BarContext::InstallFailedFinal,
            Stage::Failed(_) => BarContext::InstallFailed,
        }
    }
}

/// A size as the dialog says it: `11.7 MB`, `640 KB`, `12 bytes`
/// (decimal units, as release pages show them).
pub fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1_000 {
        format!("{} KB", bytes.div_ceil(1_000))
    } else {
        format!("{bytes} bytes")
    }
}

/// A failed lookup or download as the dialog words it.
pub fn failure(error: &CallError, retry: Step) -> Failure {
    let (title, hint) = text::install_failure(&error.code);
    Failure {
        code: error.code.clone(),
        title,
        hint,
        message: error.message.clone(),
        // No download for this platform: trying again can't change that.
        retry: (error.code != "NOT_SUPPORTED").then_some(retry),
    }
}

fn next_op(model: &mut Model) -> u64 {
    let op = model.next_install;
    model.next_install += 1;
    op
}

fn dialog(model: &mut Model) -> Option<&mut InstallDialog> {
    match &mut model.modal {
        Some(Modal::InstallDuckdb(d)) => Some(d),
        _ => None,
    }
}

/// The dialog, when `op` is its current one.
fn current(model: &mut Model, op: u64) -> Option<&mut InstallDialog> {
    dialog(model).filter(|d| d.op == op)
}

/// `ENGINE_NOT_INSTALLED` from a connect: the dialog opens on `pending`
/// and looks up the size.
pub fn open(model: &mut Model, pending: Pending) -> Vec<Effect> {
    let op = next_op(model);
    model.modal = Some(Modal::InstallDuckdb(InstallDialog {
        pending,
        op,
        stage: Stage::Checking,
    }));
    vec![Effect::CheckDuckdb { op }]
}

/// The size lookup answered.
pub fn on_offer(model: &mut Model, op: u64, result: Result<Offer, CallError>) -> Vec<Effect> {
    let Some(d) = current(model, op) else {
        return Vec::new();
    };
    if d.stage != Stage::Checking {
        return Vec::new();
    }
    match result {
        Ok(offer) => {
            d.stage = Stage::Ask(offer);
            Vec::new()
        }
        Err(e) => {
            d.stage = Stage::Failed(failure(&e, Step::Check));
            vec![log_error(text::failed_line("duckdb lookup", &e.code))]
        }
    }
}

/// Enter on the question: download.
pub fn download(model: &mut Model) -> Vec<Effect> {
    let op = next_op(model);
    let Some(d) = dialog(model) else {
        return Vec::new();
    };
    let Stage::Ask(offer) = d.stage else {
        return Vec::new();
    };
    d.op = op;
    d.stage = Stage::Downloading {
        bytes: 0,
        total: offer.size,
    };
    vec![
        Effect::InstallDuckdb { op },
        log(text::install_started_line(&size_text(offer.size))),
    ]
}

/// The download's progress.
pub fn on_progress(model: &mut Model, op: u64, bytes: u64, total: u64) {
    if let Some(d) = current(model, op) {
        if let Stage::Downloading { .. } = d.stage {
            d.stage = Stage::Downloading { bytes, total };
        }
    }
}

/// The download ended: connect again, or say why it didn't install.
pub fn on_installed(model: &mut Model, op: u64, result: Result<(), CallError>) -> Vec<Effect> {
    let Some(d) = current(model, op) else {
        return Vec::new();
    };
    if !matches!(d.stage, Stage::Downloading { .. }) {
        return Vec::new();
    }
    match result {
        Ok(()) => {
            let Some(Modal::InstallDuckdb(d)) = model.modal.take() else {
                return Vec::new();
            };
            let mut pending = d.pending;
            pending.after_install = true;
            let mut effects = vec![log(text::INSTALLED_LINE.to_string())];
            effects.extend(super::connect::proceed(model, pending));
            effects
        }
        Err(e) => {
            d.stage = Stage::Failed(failure(&e, Step::Download));
            vec![log_error(text::failed_line("duckdb install", &e.code))]
        }
    }
}

/// `r` after a failure: the failed step again.
pub fn retry(model: &mut Model) -> Vec<Effect> {
    let op = next_op(model);
    let Some(d) = dialog(model) else {
        return Vec::new();
    };
    let Stage::Failed(f) = &d.stage else {
        return Vec::new();
    };
    match f.retry {
        None => Vec::new(),
        Some(Step::Check) => {
            d.op = op;
            d.stage = Stage::Checking;
            vec![Effect::CheckDuckdb { op }]
        }
        Some(Step::Download) => {
            // The install reads the size again: the first progress
            // message brings it.
            d.op = op;
            d.stage = Stage::Downloading { bytes: 0, total: 0 };
            vec![Effect::InstallDuckdb { op }]
        }
    }
}

/// Esc: back to the picker, on the connection's project with it selected,
/// dropping the lookup or download in flight.
pub fn stop(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::InstallDuckdb(d)) = model.modal.take() else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    if matches!(d.stage, Stage::Checking | Stage::Downloading { .. }) {
        effects.push(Effect::CancelInstall { op: d.op });
    }
    if matches!(d.stage, Stage::Downloading { .. }) {
        effects.push(log(text::INSTALL_STOPPED_LINE.to_string()));
    }
    let id = &d.pending.connection_id;
    model.modal = Some(match model.library.connection(id) {
        Some(row) => {
            let project_id = row.project_id.clone();
            let mut p = picker::connections_stage(&model.library, &project_id, &model.remembered);
            if let Some(i) = model
                .library
                .connections_of(&project_id)
                .position(|c| &c.id == id)
            {
                p.selected = i;
            }
            Modal::Picker(p)
        }
        None => Modal::Picker(picker::open_picker(
            &model.library,
            model.project.as_deref(),
            &model.remembered,
        )),
    });
    effects
}

fn log(text: String) -> Effect {
    Model::log_effect(None, text)
}

fn log_error(text: String) -> Effect {
    Model::log_effect(Some(Tag::Error), text)
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyCode;

    use super::*;
    use crate::state::app::{update, Attempt, Conn, ConnectCall, Msg, Stamp};
    use crate::state::panels::ConnItem;
    use crate::state::picker::{Picker, Stage as PickerStage};
    use crate::state::secrets::{Secret, SecretKind, Typed};
    use crate::testing::fixtures::{library, model};
    use crate::testing::keys::{key, press};

    /// The fixture library plus a DuckDB file connection in project-b.
    fn ready() -> Model {
        let mut m = model();
        let mut lib = library();
        let file = lib
            .connections
            .iter()
            .find(|c| c.id == "conn-file")
            .unwrap()
            .clone();
        lib.connections.push(ConnItem {
            id: "conn-duck".into(),
            name: "warehouse".into(),
            engine: "duckdb".into(),
            database: "/data/warehouse.duckdb".into(),
            ..file
        });
        update(&mut m, Msg::Library(Ok(lib)));
        m.project = Some("project-b".into());
        m
    }

    /// A pending connect with a typed secret, so the reconnect can be
    /// seen carrying it (DuckDB itself never asks for one).
    fn pending() -> Pending {
        let mut typed = Typed::default();
        typed.set(SecretKind::Db, Secret::new("typed-pw"));
        Pending {
            connection_id: "conn-duck".into(),
            typed,
            ..Pending::default()
        }
    }

    /// `pending` connecting as `attempt`, then Core's `code`.
    fn refused(m: &mut Model, pending: Pending, code: &str) -> Vec<Effect> {
        m.conn = Conn::Connecting(Attempt {
            attempt: 7,
            pending,
        });
        update(
            m,
            Msg::Connected {
                attempt: 7,
                result: Err(CallError::new(
                    code,
                    "DuckDB support for Seaquel 2026.10.4 isn't installed: the DuckDB helper isn't there",
                )),
                stamp: Stamp::default(),
            },
        )
    }

    fn dialog(m: &Model) -> &InstallDialog {
        match &m.modal {
            Some(Modal::InstallDuckdb(d)) => d,
            other => panic!("not the install dialog: {other:?}"),
        }
    }

    fn check_op(effects: &[Effect]) -> u64 {
        effects
            .iter()
            .find_map(|e| match e {
                Effect::CheckDuckdb { op } => Some(*op),
                _ => None,
            })
            .expect("looks the size up")
    }

    fn install_op(effects: &[Effect]) -> Option<u64> {
        effects.iter().find_map(|e| match e {
            Effect::InstallDuckdb { op } => Some(*op),
            _ => None,
        })
    }

    fn connect_call(effects: &[Effect]) -> Option<&ConnectCall> {
        effects.iter().find_map(|e| match e {
            Effect::Connect(call) => Some(call),
            _ => None,
        })
    }

    /// The dialog at the question, with `size`; its op.
    fn asking(m: &mut Model, size: u64) -> u64 {
        let op = check_op(&refused(m, pending(), "ENGINE_NOT_INSTALLED"));
        update(
            m,
            Msg::DuckdbOffer {
                op,
                result: Ok(Offer {
                    size,
                    repair: false,
                }),
            },
        );
        op
    }

    /// The dialog downloading; the download's op.
    fn downloading(m: &mut Model) -> u64 {
        asking(m, 11_700_000);
        install_op(&update(m, press(KeyCode::Enter))).expect("downloads")
    }

    #[test]
    fn not_installed_opens_the_dialog_with_the_pending_connect() {
        let mut m = ready();
        let effects = refused(&mut m, pending(), "ENGINE_NOT_INSTALLED");
        let op = check_op(&effects);
        let d = dialog(&m);
        assert_eq!(d.stage, Stage::Checking);
        assert_eq!(d.op, op);
        assert_eq!(d.pending, pending(), "the connect and its secrets kept");
        assert_eq!(m.bar_context(), BarContext::InstallChecking);
        assert_eq!(
            m.conn,
            Conn::Failed {
                id: "conn-duck".into()
            }
        );
        assert!(connect_call(&effects).is_none());
        // Nothing is downloaded before the user says so.
        assert!(install_op(&effects).is_none());
    }

    #[test]
    fn the_size_is_asked_about_then_enter_downloads() {
        let mut m = ready();
        let op = asking(&mut m, 11_700_000);
        assert_eq!(
            dialog(&m).stage,
            Stage::Ask(Offer {
                size: 11_700_000,
                repair: false
            })
        );
        assert_eq!(m.bar_context(), BarContext::InstallAsk);
        // Global keys don't reach past the dialog.
        assert!(update(&mut m, key('q')).is_empty());
        assert!(matches!(m.modal, Some(Modal::InstallDuckdb(_))));
        let effects = update(&mut m, press(KeyCode::Enter));
        let download = install_op(&effects).expect("downloads");
        assert_ne!(download, op, "a new op for the download");
        let d = dialog(&m);
        assert_eq!(d.op, download);
        assert_eq!(
            d.stage,
            Stage::Downloading {
                bytes: 0,
                total: 11_700_000
            }
        );
        assert_eq!(m.bar_context(), BarContext::InstallDownloading);
        // Enter again starts nothing.
        assert!(install_op(&update(&mut m, press(KeyCode::Enter))).is_none());
    }

    #[test]
    fn a_late_or_stale_offer_is_dropped() {
        let mut m = ready();
        let op = check_op(&refused(&mut m, pending(), "ENGINE_NOT_INSTALLED"));
        update(
            &mut m,
            Msg::DuckdbOffer {
                op: op + 100,
                result: Ok(Offer {
                    size: 1,
                    repair: false,
                }),
            },
        );
        assert_eq!(dialog(&m).stage, Stage::Checking);
        // With the dialog gone, an answer opens nothing.
        update(&mut m, press(KeyCode::Esc));
        update(
            &mut m,
            Msg::DuckdbOffer {
                op,
                result: Ok(Offer {
                    size: 1,
                    repair: false,
                }),
            },
        );
        assert!(!matches!(m.modal, Some(Modal::InstallDuckdb(_))));
    }

    #[test]
    fn progress_moves_the_bar_and_ignores_another_op() {
        let mut m = ready();
        let op = downloading(&mut m);
        update(
            &mut m,
            Msg::InstallProgress {
                op,
                bytes: 4_000_000,
                total: 11_700_000,
            },
        );
        assert_eq!(
            dialog(&m).stage,
            Stage::Downloading {
                bytes: 4_000_000,
                total: 11_700_000
            }
        );
        update(
            &mut m,
            Msg::InstallProgress {
                op: op + 1,
                bytes: 9_000_000,
                total: 11_700_000,
            },
        );
        assert_eq!(
            dialog(&m).stage,
            Stage::Downloading {
                bytes: 4_000_000,
                total: 11_700_000
            }
        );
    }

    #[test]
    fn esc_cancels_and_returns_to_the_picker_on_the_connection() {
        for at in ["checking", "asking", "downloading", "failed"] {
            let mut m = ready();
            let op = match at {
                "checking" => check_op(&refused(&mut m, pending(), "ENGINE_NOT_INSTALLED")),
                "asking" => asking(&mut m, 5),
                "downloading" => downloading(&mut m),
                _ => {
                    let op = downloading(&mut m);
                    update(
                        &mut m,
                        Msg::Installed {
                            op,
                            result: Err(CallError::new("FILE_ERROR", "StorageFull")),
                        },
                    );
                    op
                }
            };
            let effects = update(&mut m, press(KeyCode::Esc));
            let cancels = effects.contains(&Effect::CancelInstall { op });
            // Only a call in flight is dropped.
            assert_eq!(cancels, matches!(at, "checking" | "downloading"), "{at}");
            let duck = m
                .library
                .connections_of("project-b")
                .position(|c| c.id == "conn-duck")
                .unwrap();
            assert_eq!(
                m.modal,
                Some(Modal::Picker(Picker {
                    stage: PickerStage::Connections {
                        project_id: "project-b".into()
                    },
                    selected: duck,
                })),
                "{at}"
            );
            // A late end of the dropped download connects nothing.
            let effects = update(&mut m, Msg::Installed { op, result: Ok(()) });
            assert!(connect_call(&effects).is_none(), "{at}");
            assert!(matches!(m.modal, Some(Modal::Picker(_))), "{at}");
        }
    }

    #[test]
    fn success_connects_again_with_the_same_typed_secrets() {
        let mut m = ready();
        let op = downloading(&mut m);
        let effects = update(&mut m, Msg::Installed { op, result: Ok(()) });
        let call = connect_call(&effects).expect("connects again");
        assert_eq!(call.connection_id, "conn-duck");
        assert_eq!(
            call.secrets.db.as_ref().map(Secret::expose),
            Some("typed-pw")
        );
        assert_eq!(m.modal, None);
        let Conn::Connecting(attempt) = &m.conn else {
            panic!("{:?}", m.conn)
        };
        assert!(attempt.pending.after_install);
        assert!(effects.iter().any(|e| matches!(e, Effect::Log(_))));
    }

    /// Right after an install, `ENGINE_NOT_INSTALLED` again (a file that
    /// won't start) is a problem to read, not a second download.
    #[test]
    fn not_installed_right_after_an_install_is_a_problem() {
        let mut m = ready();
        let effects = refused(
            &mut m,
            Pending {
                after_install: true,
                ..pending()
            },
            "ENGINE_NOT_INSTALLED",
        );
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::CheckDuckdb { .. })));
        let Some(Modal::Problem(p)) = &m.modal else {
            panic!("{:?}", m.modal)
        };
        assert_eq!(p.code, "ENGINE_NOT_INSTALLED");
        assert_eq!(p.title, text::PROBLEM_TITLE_NOT_INSTALLED);
        assert!(p.message.contains("isn't there"), "{}", p.message);
    }

    #[test]
    fn each_failure_is_worded_and_retries_its_step() {
        let codes = [
            "NETWORK_ERROR",
            "RELEASE_NOT_FOUND",
            "ASSET_NOT_FOUND",
            "DIGEST_MISMATCH",
            "SIZE_MISMATCH",
            "GZIP_ERROR",
            "FILE_ERROR",
            "UNSAFE_FOLDER",
        ];
        let mut titles = Vec::new();
        for code in codes {
            let mut m = ready();
            let op = downloading(&mut m);
            update(
                &mut m,
                Msg::Installed {
                    op,
                    result: Err(CallError::new(code, "core says")),
                },
            );
            let Stage::Failed(f) = &dialog(&m).stage else {
                panic!("{code}: {:?}", dialog(&m).stage)
            };
            assert_eq!(f.code, code);
            assert!(!f.title.is_empty() && !f.hint.is_empty(), "{code}");
            assert_eq!(f.message, "core says");
            assert_eq!(f.retry, Some(Step::Download));
            titles.push((code, f.title));
            assert_eq!(m.bar_context(), BarContext::InstallFailed);
            // `r` downloads again, under a new op.
            let again = install_op(&update(&mut m, key('r'))).expect("retries the download");
            assert!(again > op, "{code}");
            assert!(matches!(
                dialog(&m).stage,
                Stage::Downloading { bytes: 0, .. }
            ));
        }
        // The kinds read differently: no network, not published, damaged,
        // disk, folder.
        let title = |code| titles.iter().find(|(c, _)| *c == code).unwrap().1;
        assert_ne!(title("NETWORK_ERROR"), title("RELEASE_NOT_FOUND"));
        assert_eq!(title("RELEASE_NOT_FOUND"), title("ASSET_NOT_FOUND"));
        assert_eq!(title("DIGEST_MISMATCH"), title("SIZE_MISMATCH"));
        assert_eq!(title("DIGEST_MISMATCH"), title("GZIP_ERROR"));
        assert_ne!(title("DIGEST_MISMATCH"), title("FILE_ERROR"));
        assert_ne!(title("FILE_ERROR"), title("UNSAFE_FOLDER"));
        assert_ne!(title("NETWORK_ERROR"), title("UNSAFE_FOLDER"));
        // Something unforeseen still gets a title.
        let other = failure(&CallError::new("SOMETHING_NEW", "x"), Step::Check);
        assert!(!other.title.is_empty());
    }

    /// Offline the lookup fails: the dialog says so and Retry looks again.
    #[test]
    fn offline_the_lookup_says_so_and_retry_looks_again() {
        let mut m = ready();
        let op = check_op(&refused(&mut m, pending(), "ENGINE_NOT_INSTALLED"));
        update(
            &mut m,
            Msg::DuckdbOffer {
                op,
                result: Err(CallError::new("NETWORK_ERROR", "connect")),
            },
        );
        let Stage::Failed(f) = &dialog(&m).stage else {
            panic!("{:?}", dialog(&m).stage)
        };
        assert_eq!(f.retry, Some(Step::Check));
        assert_eq!(
            f.title,
            failure(&CallError::new("NETWORK_ERROR", ""), Step::Check).title
        );
        let effects = update(&mut m, key('r'));
        let again = check_op(&effects);
        assert!(again > op);
        assert!(install_op(&effects).is_none());
        assert_eq!(dialog(&m).stage, Stage::Checking);
    }

    /// Review nit: no download for this platform isn't fixed by trying
    /// again, so the failure offers only Close.
    #[test]
    fn not_supported_offers_no_retry() {
        let mut m = ready();
        let op = check_op(&refused(&mut m, pending(), "ENGINE_NOT_INSTALLED"));
        update(
            &mut m,
            Msg::DuckdbOffer {
                op,
                result: Err(CallError::new("NOT_SUPPORTED", "no download here")),
            },
        );
        let Stage::Failed(f) = &dialog(&m).stage else {
            panic!("{:?}", dialog(&m).stage)
        };
        assert_eq!(f.retry, None);
        assert_eq!(m.bar_context(), BarContext::InstallFailedFinal);
        assert!(update(&mut m, key('r')).is_empty());
        assert!(matches!(dialog(&m).stage, Stage::Failed(_)));
        update(&mut m, press(KeyCode::Esc));
        assert!(matches!(m.modal, Some(Modal::Picker(_))));
    }

    /// Review M2: an `Unsafe` helper's offer says an install makes the
    /// folder private, and Enter still downloads.
    #[test]
    fn a_loose_folder_s_offer_is_a_repair() {
        let mut m = ready();
        let op = check_op(&refused(&mut m, pending(), "ENGINE_NOT_INSTALLED"));
        let offer = Offer {
            size: 11_700_000,
            repair: true,
        };
        update(
            &mut m,
            Msg::DuckdbOffer {
                op,
                result: Ok(offer),
            },
        );
        assert_eq!(dialog(&m).stage, Stage::Ask(offer));
        assert_eq!(m.bar_context(), BarContext::InstallAsk);
        assert!(install_op(&update(&mut m, press(KeyCode::Enter))).is_some());
    }

    /// Review M1: the connection was removed (by the app, say) while its
    /// helper downloaded: nothing to connect, so the picker opens and the
    /// command log says why.
    #[test]
    fn a_connection_removed_during_the_install_opens_the_picker_and_says_so() {
        let mut m = ready();
        let op = downloading(&mut m);
        m.library.connections.retain(|c| c.id != "conn-duck");
        let effects = update(&mut m, Msg::Installed { op, result: Ok(()) });
        assert!(connect_call(&effects).is_none());
        assert!(matches!(m.modal, Some(Modal::Picker(_))), "{:?}", m.modal);
        assert!(
            effects.contains(&Model::log_effect(
                Some(Tag::Error),
                text::CONNECTION_REMOVED.to_string()
            )),
            "{effects:?}"
        );
    }

    #[test]
    fn sizes_read_as_release_pages_show_them() {
        assert_eq!(size_text(11_700_000), "11.7 MB");
        assert_eq!(size_text(1_000_000), "1.0 MB");
        assert_eq!(size_text(640_000), "640 KB");
        assert_eq!(size_text(1_500), "2 KB");
        assert_eq!(size_text(12), "12 bytes");
    }

    #[test]
    fn debug_shows_no_message_or_secret() {
        let mut m = ready();
        let op = downloading(&mut m);
        update(
            &mut m,
            Msg::Installed {
                op,
                result: Err(CallError::new("FILE_ERROR", "msg-marker")),
            },
        );
        let text = format!("{:?}", m.modal);
        assert!(
            !text.contains("marker") && !text.contains("typed-pw"),
            "{text}"
        );
    }
}
