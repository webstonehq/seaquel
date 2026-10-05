//! Windows file security for the DuckDB helper:
//! the rule that says whether a file or
//! folder is safe to start a program from, and on Windows the calls that
//! read and set it.
//!
//! The rule is pure, over a description of the security descriptor
//! ([`Security`]: the owner's SID and the DACL's entries), so it is tested
//! on every platform. A file or folder is safe when:
//! - it isn't a link: a name-surrogate reparse point (symlink, junction;
//!   [`is_link`]). Other reparse points (WOF compression, dedup, cloud
//!   placeholders) are the file itself;
//! - its owner is the user, Administrators (an elevated process can make
//!   that owner for the user's own files) or SYSTEM (an installer or a
//!   management agent; SYSTEM can do anything anyway);
//! - it has a DACL (a NULL DACL grants everyone everything);
//! - no entry that applies to it grants a right of its [`Level`] to anyone
//!   but the user, SYSTEM or Administrators. Below the app's folder
//!   ([`Level::Private`]) that is [`rights::WRITE`]: write or add a file,
//!   append or add a folder, delete, delete a child, `WRITE_DAC`,
//!   `WRITE_OWNER`, or the generic write or all. On the app's folder
//!   ([`Level::Root`]) it is [`rights::REPLACE`], what replaces or renames
//!   `bin` or the folder itself (delete a child, delete, `WRITE_DAC`,
//!   `WRITE_OWNER`, generic all): others may add files there, as Unix only
//!   takes group and world write off that folder. Deny entries are ignored
//!   (an allow to another principal is refused even when a deny precedes
//!   it), inherit-only entries are skipped (they apply to children, which
//!   are checked themselves), and an entry of a type this rule doesn't know
//!   refuses.
//!
//! Writing extended attributes or basic attributes isn't counted: neither
//! changes what runs.
//!
//! What an install sets below the app's folder (`make_private`, Windows)
//! is a protected DACL (inheritance from the parent off) granting full
//! control to the user and SYSTEM only, inherited by a folder's children.
//! The app's folder is only repaired ([`Security::root_repair`]): its own
//! entries kept, the replacing rights taken out of others' allows.
//!
//! Nothing here logs, and no error carries a path or a SID.

/// The SID of `NT AUTHORITY\SYSTEM`.
pub const SYSTEM: &str = "S-1-5-18";
/// The SID of `BUILTIN\Administrators`.
pub const ADMINISTRATORS: &str = "S-1-5-32-544";
/// The SID of `Everyone` (tests).
pub const EVERYONE: &str = "S-1-1-0";

/// Access rights in an entry's mask (`winnt.h`'s values; checked against
/// `windows-sys` on Windows).
pub mod rights {
    /// `FILE_WRITE_DATA`; `FILE_ADD_FILE` on a folder.
    pub const FILE_WRITE_DATA: u32 = 0x0002;
    /// `FILE_APPEND_DATA`; `FILE_ADD_SUBDIRECTORY` on a folder.
    pub const FILE_APPEND_DATA: u32 = 0x0004;
    pub const FILE_DELETE_CHILD: u32 = 0x0040;
    pub const DELETE: u32 = 0x0001_0000;
    pub const WRITE_DAC: u32 = 0x0004_0000;
    pub const WRITE_OWNER: u32 = 0x0008_0000;
    pub const GENERIC_ALL: u32 = 0x1000_0000;
    pub const GENERIC_WRITE: u32 = 0x4000_0000;
    /// Full control.
    pub const FILE_ALL_ACCESS: u32 = 0x001F_01FF;
    /// `GENERIC_READ` and `GENERIC_EXECUTE`.
    pub const GENERIC_READ: u32 = 0x8000_0000;
    pub const GENERIC_EXECUTE: u32 = 0x2000_0000;
    /// What the generic rights mean for a file or folder.
    pub const FILE_GENERIC_READ: u32 = 0x0012_0089;
    pub const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
    pub const FILE_GENERIC_EXECUTE: u32 = 0x0012_00A0;
    /// On the app's folder (`<identifier>`), what lets another principal
    /// replace or rename `bin`, or the folder itself: the root's rule,
    /// Unix's "no group or world write" on a folder whose children are
    /// private.
    pub const REPLACE: u32 = FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;
    /// What only the user, SYSTEM and Administrators may be granted.
    pub const WRITE: u32 = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_DELETE_CHILD
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_ALL
        | GENERIC_WRITE;
}

/// An entry's flags (`AceFlags`).
pub mod flags {
    pub const OBJECT_INHERIT: u8 = 0x01;
    pub const CONTAINER_INHERIT: u8 = 0x02;
    pub const INHERIT_ONLY: u8 = 0x08;
    pub const INHERITED: u8 = 0x10;
}

/// What an entry does, from its `AceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AceKind {
    /// `ACCESS_ALLOWED_ACE_TYPE` (0).
    Allow,
    /// `ACCESS_ALLOWED_CALLBACK_ACE_TYPE` (9): a conditional allow. It may
    /// apply, so the rule counts it as an allow; a repair can't rewrite it.
    AllowCallback,
    /// `ACCESS_DENIED_ACE_TYPE` (1).
    Deny,
    /// `ACCESS_DENIED_CALLBACK_ACE_TYPE` (10).
    DenyCallback,
    /// Any other type (object entries, audit entries): refused unless
    /// inherit-only.
    Other,
}

impl AceKind {
    /// The kind of an `AceType` byte.
    pub fn of(ace_type: u8) -> AceKind {
        match ace_type {
            0 => AceKind::Allow,
            9 => AceKind::AllowCallback,
            1 => AceKind::Deny,
            10 => AceKind::DenyCallback,
            _ => AceKind::Other,
        }
    }

    fn allows(self) -> bool {
        matches!(self, AceKind::Allow | AceKind::AllowCallback)
    }

    fn denies(self) -> bool {
        matches!(self, AceKind::Deny | AceKind::DenyCallback)
    }
}

/// How strict the rule is for a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// `bin` and below, and the file: no write of any kind by others
    /// ([`rights::WRITE`]).
    Private,
    /// The app's folder: others may add files and folders, not replace or
    /// rename what is there ([`rights::REPLACE`]).
    Root,
}

/// `FILE_ATTRIBUTE_REPARSE_POINT`.
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
/// The reparse tag bit that marks a name surrogate: a symlink or a junction
/// (std's `is_symlink` test, junctions included).
pub const REPARSE_NAME_SURROGATE: u32 = 0x2000_0000;

/// Whether a file with these attributes and reparse tag is a link. Only
/// name surrogates count; other reparse points (WOF compression, dedup,
/// cloud placeholders) are the file itself.
pub fn is_link(attributes: u32, reparse_tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 && reparse_tag & REPARSE_NAME_SURROGATE != 0
}

/// A mask with its generic rights replaced by what they mean for files.
pub fn expand_generic(mask: u32) -> u32 {
    let mut out = mask
        & !(rights::GENERIC_ALL
            | rights::GENERIC_WRITE
            | rights::GENERIC_READ
            | rights::GENERIC_EXECUTE);
    if mask & rights::GENERIC_ALL != 0 {
        out |= rights::FILE_ALL_ACCESS;
    }
    if mask & rights::GENERIC_WRITE != 0 {
        out |= rights::FILE_GENERIC_WRITE;
    }
    if mask & rights::GENERIC_READ != 0 {
        out |= rights::FILE_GENERIC_READ;
    }
    if mask & rights::GENERIC_EXECUTE != 0 {
        out |= rights::FILE_GENERIC_EXECUTE;
    }
    out
}

/// One entry of a DACL. `sid` is the string form (`S-1-5-…`); empty for
/// [`AceKind::Other`], whose layout isn't read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ace {
    pub kind: AceKind,
    pub flags: u8,
    pub mask: u32,
    pub sid: String,
}

impl Ace {
    /// An allow entry (tests and descriptions).
    pub fn allow(sid: &str, mask: u32, flags: u8) -> Ace {
        Ace {
            kind: AceKind::Allow,
            flags,
            mask,
            sid: sid.to_string(),
        }
    }
}

/// A file's or folder's owner and DACL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Security {
    /// The owner's SID; `None` when the descriptor has none.
    pub owner: Option<String>,
    /// `None` is a NULL DACL (everyone may do anything).
    pub dacl: Option<Vec<Ace>>,
    /// `SE_DACL_PROTECTED`: the parent's inheritable entries don't apply.
    pub protected: bool,
}

/// Why a file or folder isn't safe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A symlink, junction or other reparse point.
    Link,
    /// A file where a folder belongs, or the other way round.
    WrongKind,
    /// Owned by someone but the user and Administrators.
    Owner,
    /// A NULL DACL.
    NoDacl,
    /// An entry grants write to another principal.
    OthersCanWrite,
    /// An entry of a type the rule doesn't know.
    UnknownEntry,
}

impl Problem {
    /// A few words for an error message (no path, no SID).
    pub fn describe(self) -> &'static str {
        match self {
            Problem::Link => "a link",
            Problem::WrongKind => "not the kind expected",
            Problem::Owner => "owned by another user",
            Problem::NoDacl => "open to everyone",
            Problem::OthersCanWrite => "writable by another user",
            Problem::UnknownEntry => "an access entry that can't be judged",
        }
    }
}

impl Security {
    /// [`Security::problem_at`] for [`Level::Private`].
    pub fn problem(&self, user: &str) -> Option<Problem> {
        self.problem_at(user, Level::Private)
    }

    /// The root's repair: the same entries, in canonical order (denies
    /// first) and all explicit, with [`rights::REPLACE`] taken out of every
    /// allow to another principal (generic rights expanded first, so read
    /// and add stay) and allows left with nothing dropped. Set protected,
    /// so the parent's entries don't come back. `None` when an entry can't
    /// be rewritten as it is (a callback entry, an unknown type).
    pub fn root_repair(&self, user: &str) -> Option<Vec<Ace>> {
        let inherit = flags::OBJECT_INHERIT | flags::CONTAINER_INHERIT;
        let Some(dacl) = &self.dacl else {
            return Some(vec![
                Ace::allow(user, rights::FILE_ALL_ACCESS, inherit),
                Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, inherit),
            ]);
        };
        let (mut denies, mut allows) = (Vec::new(), Vec::new());
        for ace in dacl {
            let explicit = ace.flags & !flags::INHERITED;
            match ace.kind {
                AceKind::Deny => denies.push(Ace {
                    flags: explicit,
                    ..ace.clone()
                }),
                AceKind::Allow => {
                    let mask = if trusted(&ace.sid, user) {
                        ace.mask
                    } else {
                        expand_generic(ace.mask) & !rights::REPLACE
                    };
                    if mask != 0 {
                        allows.push(Ace {
                            flags: explicit,
                            mask,
                            ..ace.clone()
                        });
                    }
                }
                AceKind::AllowCallback | AceKind::DenyCallback | AceKind::Other => return None,
            }
        }
        denies.extend(allows);
        Some(denies)
    }

    /// Why this isn't safe to start a program from for `user` (a SID) at
    /// `level`, or `None` when it is (the rule in the module's
    /// documentation).
    pub fn problem_at(&self, user: &str, level: Level) -> Option<Problem> {
        match self.owner.as_deref() {
            Some(owner) if trusted(owner, user) => {}
            _ => return Some(Problem::Owner),
        }
        let unsafe_rights = match level {
            Level::Private => rights::WRITE,
            Level::Root => rights::REPLACE,
        };
        let Some(dacl) = &self.dacl else {
            return Some(Problem::NoDacl);
        };
        for ace in dacl.iter().filter(|a| applies(a)) {
            if ace.kind == AceKind::Other {
                return Some(Problem::UnknownEntry);
            }
            if ace.kind.allows() && !trusted(&ace.sid, user) && ace.mask & unsafe_rights != 0 {
                return Some(Problem::OthersCanWrite);
            }
        }
        None
    }

    /// Exactly what [`make_private`] sets: protected, and every entry that
    /// applies grants only the user or SYSTEM. An install sets the DACL
    /// again when this is false.
    pub fn is_private(&self, user: &str) -> bool {
        self.protected
            && self.problem(user).is_none()
            && self.dacl.iter().flatten().filter(|a| applies(a)).all(|a| {
                a.kind.denies() || (a.kind == AceKind::Allow && (a.sid == user || a.sid == SYSTEM))
            })
    }
}

/// The user, SYSTEM or Administrators.
fn trusted(sid: &str, user: &str) -> bool {
    sid == user || sid == SYSTEM || sid == ADMINISTRATORS
}

/// Whether an entry applies to the object itself (not inherit-only).
fn applies(ace: &Ace) -> bool {
    ace.flags & flags::INHERIT_ONLY == 0
}

/// A file or folder as found, without following a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub security: Security,
    pub dir: bool,
    /// A symlink, junction or other reparse point.
    pub link: bool,
}

impl Entry {
    /// [`Security::problem_at`], after the entry is no link and is a
    /// folder when `dir` (a file otherwise).
    pub fn problem(&self, user: &str, dir: bool, level: Level) -> Option<Problem> {
        if self.link {
            Some(Problem::Link)
        } else if self.dir != dir {
            Some(Problem::WrongKind)
        } else {
            self.security.problem_at(user, level)
        }
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{current_user, inspect, make_private, AclError, Node};

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "S-1-5-21-1-2-3-1001";
    const OTHER: &str = "S-1-5-21-1-2-3-1002";
    const USERS: &str = "S-1-5-32-545";
    const AUTHENTICATED: &str = "S-1-5-11";
    const CREATOR_OWNER: &str = "S-1-3-0";
    const OI_CI: u8 = flags::OBJECT_INHERIT | flags::CONTAINER_INHERIT;

    fn sec(owner: &str, aces: Vec<Ace>) -> Security {
        Security {
            owner: Some(owner.to_string()),
            dacl: Some(aces),
            protected: false,
        }
    }

    /// `%LOCALAPPDATA%`'s default, inherited by the app's folder.
    fn profile_default() -> Vec<Ace> {
        vec![
            Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, OI_CI | flags::INHERITED),
            Ace::allow(
                ADMINISTRATORS,
                rights::FILE_ALL_ACCESS,
                OI_CI | flags::INHERITED,
            ),
            Ace::allow(USER, rights::FILE_ALL_ACCESS, OI_CI | flags::INHERITED),
        ]
    }

    /// What an install sets.
    fn private() -> Security {
        Security {
            owner: Some(USER.to_string()),
            dacl: Some(vec![
                Ace::allow(USER, rights::FILE_ALL_ACCESS, OI_CI),
                Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, OI_CI),
            ]),
            protected: true,
        }
    }

    #[test]
    fn the_profiles_default_and_the_installs_dacl_are_safe() {
        assert_eq!(sec(USER, profile_default()).problem(USER), None);
        assert_eq!(private().problem(USER), None);
    }

    /// A folder given `Everyone:(M)`. Modify is read, write,
    /// append, execute and delete.
    #[test]
    fn everyone_modify_is_refused() {
        let modify = 0x0013_01BF;
        let mut aces = profile_default();
        aces.push(Ace::allow(EVERYONE, modify, 0));
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
    }

    /// Each right that counts as writing, alone, granted to another principal.
    #[test]
    fn every_write_right_to_another_principal_is_refused() {
        for right in [
            rights::FILE_WRITE_DATA,
            rights::FILE_APPEND_DATA,
            rights::FILE_DELETE_CHILD,
            rights::DELETE,
            rights::WRITE_DAC,
            rights::WRITE_OWNER,
            rights::GENERIC_ALL,
            rights::GENERIC_WRITE,
        ] {
            for who in [OTHER, USERS, AUTHENTICATED, EVERYONE] {
                let mut aces = profile_default();
                aces.push(Ace::allow(who, right, 0));
                assert_eq!(
                    sec(USER, aces).problem(USER),
                    Some(Problem::OthersCanWrite),
                    "{right:#x} to {who}"
                );
            }
        }
    }

    /// Reading and running, and writing attributes, aren't writes.
    #[test]
    fn read_and_execute_for_others_is_fine() {
        let read_execute = 0x0012_00A9;
        let attributes = 0x0100 | 0x0010; // FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA
        let mut aces = profile_default();
        aces.push(Ace::allow(USERS, read_execute | attributes, OI_CI));
        assert_eq!(sec(USER, aces).problem(USER), None);
    }

    /// The trusted three may write; the rule is about everyone else.
    #[test]
    fn the_user_system_and_administrators_may_write() {
        for who in [USER, SYSTEM, ADMINISTRATORS] {
            let aces = vec![Ace::allow(who, rights::WRITE, 0)];
            assert_eq!(sec(USER, aces).problem(USER), None, "{who}");
        }
    }

    /// Another user's SID differs from this one's in the last part only.
    #[test]
    fn the_user_is_matched_exactly() {
        let aces = vec![Ace::allow(OTHER, rights::FILE_ALL_ACCESS, 0)];
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
        let aces = vec![Ace::allow("S-1-5-21-1-2-3-10011", rights::WRITE_DAC, 0)];
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
    }

    /// The owner can always change the DACL, so it must be the user,
    /// Administrators (what an elevated copy produces) or SYSTEM (an
    /// installer or a management agent; SYSTEM can do anything anyway),
    /// never anyone else.
    #[test]
    fn the_owner_must_be_the_user_or_administrators() {
        assert_eq!(sec(ADMINISTRATORS, profile_default()).problem(USER), None);
        assert_eq!(sec(SYSTEM, profile_default()).problem(USER), None);
        for owner in [OTHER, USERS, EVERYONE] {
            assert_eq!(
                sec(owner, profile_default()).problem(USER),
                Some(Problem::Owner),
                "{owner}"
            );
        }
        let no_owner = Security {
            owner: None,
            ..sec(USER, profile_default())
        };
        assert_eq!(no_owner.problem(USER), Some(Problem::Owner));
    }

    #[test]
    fn a_null_dacl_is_refused() {
        let null = Security {
            owner: Some(USER.to_string()),
            dacl: None,
            protected: true,
        };
        assert_eq!(null.problem(USER), Some(Problem::NoDacl));
        // An empty DACL grants nothing: safe (if useless).
        assert_eq!(sec(USER, vec![]).problem(USER), None);
    }

    /// Inherit-only entries apply to children (checked on their own), so
    /// `C:\ProgramData`'s `CREATOR OWNER:(OI)(CI)(IO)(F)` doesn't refuse
    /// the folder itself. An entry that applies to both does.
    #[test]
    fn inherit_only_entries_are_for_the_children() {
        let mut aces = profile_default();
        aces.push(Ace::allow(
            CREATOR_OWNER,
            rights::FILE_ALL_ACCESS,
            OI_CI | flags::INHERIT_ONLY,
        ));
        aces.push(Ace::allow(
            EVERYONE,
            rights::GENERIC_ALL,
            OI_CI | flags::INHERIT_ONLY,
        ));
        assert_eq!(sec(USER, aces.clone()).problem(USER), None);
        aces.push(Ace::allow(
            USERS,
            rights::FILE_APPEND_DATA,
            flags::CONTAINER_INHERIT,
        ));
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
    }

    /// A deny doesn't make an allow to another principal acceptable (the
    /// rule doesn't evaluate access), and a deny alone is fine.
    #[test]
    fn deny_entries_neither_refuse_nor_excuse() {
        let deny = Ace {
            kind: AceKind::Deny,
            flags: 0,
            mask: rights::WRITE,
            sid: EVERYONE.to_string(),
        };
        let mut aces = vec![deny.clone()];
        aces.extend(profile_default());
        assert_eq!(sec(USER, aces.clone()).problem(USER), None);
        aces.insert(1, Ace::allow(EVERYONE, rights::FILE_WRITE_DATA, 0));
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
    }

    #[test]
    fn an_unknown_entry_refuses_unless_inherit_only() {
        let other = Ace {
            kind: AceKind::Other,
            flags: 0,
            mask: 0,
            sid: String::new(),
        };
        let mut aces = profile_default();
        aces.push(other.clone());
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::UnknownEntry));
        let mut aces = profile_default();
        aces.push(Ace {
            flags: flags::INHERIT_ONLY,
            ..other
        });
        assert_eq!(sec(USER, aces).problem(USER), None);
    }

    #[test]
    fn ace_types_are_read() {
        assert_eq!(AceKind::of(0), AceKind::Allow);
        assert_eq!(AceKind::of(9), AceKind::AllowCallback);
        assert_eq!(AceKind::of(1), AceKind::Deny);
        assert_eq!(AceKind::of(10), AceKind::DenyCallback);
        for t in [2, 3, 5, 6, 11, 17, 0x13] {
            assert_eq!(AceKind::of(t), AceKind::Other, "{t}");
        }
    }

    /// What decides whether an install sets the DACL again.
    #[test]
    fn private_is_exactly_what_an_install_sets() {
        assert!(private().is_private(USER));
        // Inherited from the profile: safe, but not what an install sets.
        assert!(!sec(USER, profile_default()).is_private(USER));
        let mut unprotected = private();
        unprotected.protected = false;
        assert!(!unprotected.is_private(USER));
        let mut admins = private();
        admins
            .dacl
            .as_mut()
            .unwrap()
            .push(Ace::allow(ADMINISTRATORS, rights::FILE_ALL_ACCESS, 0));
        assert!(!admins.is_private(USER), "only the user and SYSTEM");
        let mut loose = private();
        loose
            .dacl
            .as_mut()
            .unwrap()
            .push(Ace::allow(EVERYONE, rights::FILE_WRITE_DATA, 0));
        assert!(!loose.is_private(USER));
        let mut admin_owner = private();
        admin_owner.owner = Some(ADMINISTRATORS.to_string());
        assert!(
            admin_owner.is_private(USER),
            "the owner is judged by problem()"
        );
        let mut other_owner = private();
        other_owner.owner = Some(OTHER.to_string());
        assert!(!other_owner.is_private(USER));
        let null = Security {
            dacl: None,
            ..private()
        };
        assert!(!null.is_private(USER));
    }

    #[test]
    fn links_and_the_wrong_kind_are_refused_first() {
        let folder = Entry {
            security: private(),
            dir: true,
            link: false,
        };
        assert_eq!(folder.problem(USER, true, Level::Private), None);
        assert_eq!(
            folder.problem(USER, false, Level::Private),
            Some(Problem::WrongKind)
        );
        let file = Entry {
            dir: false,
            ..folder.clone()
        };
        assert_eq!(file.problem(USER, false, Level::Private), None);
        assert_eq!(
            file.problem(USER, true, Level::Private),
            Some(Problem::WrongKind)
        );
        let junction = Entry {
            link: true,
            ..folder.clone()
        };
        assert_eq!(
            junction.problem(USER, true, Level::Private),
            Some(Problem::Link)
        );
        let loose = Entry {
            security: sec(OTHER, profile_default()),
            ..folder
        };
        assert_eq!(
            loose.problem(USER, true, Level::Private),
            Some(Problem::Owner)
        );
    }

    /// A conditional allow counts as an allow.
    #[test]
    fn a_callback_allow_to_another_principal_is_refused() {
        let mut aces = profile_default();
        aces.push(Ace {
            kind: AceKind::AllowCallback,
            flags: 0,
            mask: rights::FILE_WRITE_DATA,
            sid: EVERYONE.to_string(),
        });
        assert_eq!(sec(USER, aces).problem(USER), Some(Problem::OthersCanWrite));
    }

    /// Modify (`(M)`): read, write, append, execute and delete.
    const MODIFY: u32 = 0x0013_01BF;
    /// Write and add (`(W)`, `(WD,AD)` and friends): no delete.
    const WRITE_ADD: u32 =
        rights::FILE_GENERIC_READ | rights::FILE_GENERIC_WRITE | rights::FILE_GENERIC_EXECUTE;

    /// The root (decision 3): another principal may add files and folders
    /// to the app's folder, as Unix's root rule allows anything but group
    /// and world write; replacing or renaming what is there may not.
    #[test]
    fn the_root_refuses_only_what_replaces_or_renames() {
        let mut aces = profile_default();
        aces.push(Ace::allow(
            AUTHENTICATED,
            WRITE_ADD,
            OI_CI | flags::INHERITED,
        ));
        let adds = sec(USER, aces);
        assert_eq!(adds.problem_at(USER, Level::Root), None);
        assert_eq!(
            adds.problem_at(USER, Level::Private),
            Some(Problem::OthersCanWrite)
        );
        for right in [
            rights::FILE_DELETE_CHILD,
            rights::DELETE,
            rights::WRITE_DAC,
            rights::WRITE_OWNER,
            rights::GENERIC_ALL,
        ] {
            let mut aces = profile_default();
            aces.push(Ace::allow(AUTHENTICATED, WRITE_ADD | right, 0));
            assert_eq!(
                sec(USER, aces).problem_at(USER, Level::Root),
                Some(Problem::OthersCanWrite),
                "{right:#x}"
            );
        }
        // `(M)` holds DELETE: the folder itself could be renamed away.
        let mut aces = profile_default();
        aces.push(Ace::allow(AUTHENTICATED, MODIFY, OI_CI | flags::INHERITED));
        assert_eq!(
            sec(USER, aces).problem_at(USER, Level::Root),
            Some(Problem::OthersCanWrite)
        );
        // The owner and a NULL DACL are judged as at any level.
        assert_eq!(
            sec(OTHER, profile_default()).problem_at(USER, Level::Root),
            Some(Problem::Owner)
        );
    }

    /// The root's repair masks the replacing rights out of others' allows
    /// and keeps everything else: the trusted entries, the others' read and
    /// add, denies (first), inheritance flags; inherited entries become
    /// explicit, since the result is set protected.
    #[test]
    fn the_roots_repair_keeps_the_other_entries() {
        let deny = Ace {
            kind: AceKind::Deny,
            flags: OI_CI | flags::INHERITED,
            mask: rights::WRITE_DAC,
            sid: USERS.to_string(),
        };
        let mut aces = profile_default();
        aces.push(Ace::allow(AUTHENTICATED, MODIFY, OI_CI | flags::INHERITED));
        aces.push(Ace::allow(EVERYONE, rights::GENERIC_ALL, 0));
        aces.push(Ace::allow(USERS, rights::FILE_DELETE_CHILD, 0));
        aces.push(deny.clone());
        let loose = sec(USER, aces);
        let repaired = loose.root_repair(USER).expect("rewritable");
        let explicit = |a: Ace| Ace {
            flags: a.flags & !flags::INHERITED,
            ..a
        };
        let mut want = vec![explicit(deny)];
        want.extend(profile_default().into_iter().map(explicit));
        want.push(Ace::allow(AUTHENTICATED, MODIFY & !rights::DELETE, OI_CI));
        want.push(Ace::allow(
            EVERYONE,
            rights::FILE_ALL_ACCESS & !rights::REPLACE,
            0,
        ));
        assert_eq!(repaired, want, "the delete-child-only entry goes");
        let after = Security {
            dacl: Some(repaired),
            protected: true,
            ..loose
        };
        assert_eq!(after.problem_at(USER, Level::Root), None);
        // Nothing to change: the same entries, explicit.
        let fine = sec(USER, profile_default());
        assert_eq!(
            fine.root_repair(USER).unwrap(),
            profile_default()
                .into_iter()
                .map(explicit)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_root_with_entries_it_cant_rewrite_has_no_repair() {
        for kind in [
            AceKind::AllowCallback,
            AceKind::DenyCallback,
            AceKind::Other,
        ] {
            let mut aces = profile_default();
            aces.push(Ace {
                kind,
                flags: 0,
                mask: rights::FILE_WRITE_DATA,
                sid: EVERYONE.to_string(),
            });
            assert_eq!(sec(USER, aces).root_repair(USER), None, "{kind:?}");
        }
        let null = Security {
            dacl: None,
            ..sec(USER, vec![])
        };
        // Nothing to keep (an empty DACL would lock everyone out): the
        // user and SYSTEM, as on the levels below.
        assert_eq!(
            null.root_repair(USER),
            Some(vec![
                Ace::allow(USER, rights::FILE_ALL_ACCESS, OI_CI),
                Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, OI_CI),
            ])
        );
    }

    #[test]
    fn generic_rights_expand_to_file_rights() {
        assert_eq!(expand_generic(rights::GENERIC_ALL), rights::FILE_ALL_ACCESS);
        assert_eq!(
            expand_generic(rights::GENERIC_READ | rights::GENERIC_EXECUTE | rights::DELETE),
            rights::FILE_GENERIC_READ | rights::FILE_GENERIC_EXECUTE | rights::DELETE
        );
        assert_eq!(
            expand_generic(rights::GENERIC_WRITE),
            rights::FILE_GENERIC_WRITE
        );
        assert_eq!(expand_generic(0x1F), 0x1F);
    }

    /// Only name surrogates (symlinks, junctions) are links.
    #[test]
    fn only_name_surrogate_reparse_points_are_links() {
        const REPARSE: u32 = FILE_ATTRIBUTE_REPARSE_POINT;
        for tag in [
            0xA000_000C, // IO_REPARSE_TAG_SYMLINK
            0xA000_0003, // IO_REPARSE_TAG_MOUNT_POINT (a junction)
            0xA000_001D, // IO_REPARSE_TAG_LX_SYMLINK
        ] {
            assert!(is_link(REPARSE | 0x10, tag), "{tag:#x}");
        }
        for tag in [
            0x8000_0017, // IO_REPARSE_TAG_WOF
            0x8000_0013, // IO_REPARSE_TAG_DEDUP
            0x9000_001A, // IO_REPARSE_TAG_CLOUD_6
            0x9000_601A, // a cloud tag with its flag bits
            0x8000_001B, // IO_REPARSE_TAG_APPEXECLINK
        ] {
            assert!(!is_link(REPARSE, tag), "{tag:#x}");
        }
        assert!(!is_link(0x20, 0xA000_000C), "no reparse attribute, no link");
    }
}
