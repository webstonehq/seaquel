//! The Win32 side of [`super`]: a file's or folder's owner and DACL, read
//! and set through one handle opened without following a reparse point
//! ([`Node`]), and the private DACL an install sets.
//!
//! None of this runs off Windows. Its tests (`tests` below) run on CI's
//! Windows runner.

use std::ffi::c_void;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr::{self, null, null_mut};

use windows_sys::core::PWSTR;
use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GENERIC_ALL, GENERIC_EXECUTE,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, GetSecurityInfo, SetSecurityInfo,
    SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, AddAccessAllowedAceEx, AddAccessDeniedAceEx, GetAce, GetAclInformation,
    GetLengthSid, GetSecurityDescriptorControl, GetTokenInformation, InitializeAcl, TokenUser,
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PRESENT,
    SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileAttributeTagInfo, GetFileInformationByHandleEx, DELETE, FILE_ALL_ACCESS, FILE_APPEND_DATA,
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    FILE_WRITE_DATA, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::{flags, is_link, rights, Ace, AceKind, Entry, Security, SYSTEM};

// The pure rule's numbers are `winnt.h`'s.
const _: () = {
    assert!(rights::FILE_WRITE_DATA == FILE_WRITE_DATA);
    assert!(rights::FILE_APPEND_DATA == FILE_APPEND_DATA);
    assert!(rights::FILE_DELETE_CHILD == FILE_DELETE_CHILD);
    assert!(rights::DELETE == DELETE);
    assert!(rights::WRITE_DAC == WRITE_DAC);
    assert!(rights::WRITE_OWNER == WRITE_OWNER);
    assert!(rights::GENERIC_ALL == GENERIC_ALL);
    assert!(rights::GENERIC_WRITE == GENERIC_WRITE);
    assert!(rights::GENERIC_READ == GENERIC_READ);
    assert!(rights::GENERIC_EXECUTE == GENERIC_EXECUTE);
    assert!(rights::FILE_ALL_ACCESS == FILE_ALL_ACCESS);
    assert!(rights::FILE_GENERIC_READ == FILE_GENERIC_READ);
    assert!(rights::FILE_GENERIC_WRITE == FILE_GENERIC_WRITE);
    assert!(rights::FILE_GENERIC_EXECUTE == FILE_GENERIC_EXECUTE);
    assert!(super::FILE_ATTRIBUTE_REPARSE_POINT == FILE_ATTRIBUTE_REPARSE_POINT);
    assert!(flags::OBJECT_INHERIT as u32 == OBJECT_INHERIT_ACE);
    assert!(flags::CONTAINER_INHERIT as u32 == CONTAINER_INHERIT_ACE);
    assert!(flags::INHERIT_ONLY as u32 == INHERIT_ONLY_ACE);
};

/// Why a DACL couldn't be set.
#[derive(Debug)]
pub enum AclError {
    /// The path is a link (a name-surrogate reparse point): nothing was
    /// set, and nothing behind it was touched.
    Link,
    /// A file where a folder was expected, or the other way round.
    WrongKind,
    /// Windows refused; the error's kind and code only (no path).
    Io(io::Error),
}

impl fmt::Display for AclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AclError::Link => f.write_str("a link"),
            AclError::WrongKind => f.write_str("not the kind expected"),
            AclError::Io(e) => write!(f, "{:?}", e.kind()),
        }
    }
}

impl std::error::Error for AclError {}

impl From<io::Error> for AclError {
    fn from(e: io::Error) -> Self {
        AclError::Io(e)
    }
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

fn win32(status: u32) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

/// Memory Windows allocated for us, freed with `LocalFree`.
struct Local(HLOCAL);

impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from an API that says to free it with
            // `LocalFree`, and is freed once.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// A handle closed with `CloseHandle`.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: an open handle this value owns, closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn handle(file: &File) -> HANDLE {
    file.as_raw_handle() as HANDLE
}

/// `S-1-5-…` for a SID.
fn sid_string(sid: PSID) -> io::Result<String> {
    let mut text: PWSTR = null_mut();
    // SAFETY: `sid` points at a SID the caller holds; `text` receives a
    // string Windows allocates, freed by `Local`.
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last_error());
    }
    let _free = Local(text.cast());
    // SAFETY: a NUL-terminated UTF-16 string, valid until `_free` drops.
    let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
    let units = unsafe { std::slice::from_raw_parts(text, len) };
    Ok(String::from_utf16_lossy(units))
}

/// Runs `f` with this process's user SID (the token's user, the same when
/// elevated).
fn with_user_sid<R>(f: impl FnOnce(PSID) -> io::Result<R>) -> io::Result<R> {
    let mut token: HANDLE = null_mut();
    // SAFETY: the pseudo-handle of this process; `token` receives a handle
    // closed by `Owned`.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = Owned(token);
    let mut len = 0u32;
    // SAFETY: a size query (no buffer); it fails with
    // ERROR_INSUFFICIENT_BUFFER and sets `len`.
    let sized = unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut len) };
    if sized == 0 {
        let e = last_error();
        if e.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(e);
        }
    }
    // u64s, so the TOKEN_USER at its start is aligned.
    let mut buffer = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: `buffer` holds at least `len` bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 8) as u32,
            &mut len,
        )
    } == 0
    {
        return Err(last_error());
    }
    // SAFETY: the call filled a TOKEN_USER whose SID points into `buffer`,
    // which outlives `f`.
    let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    f(sid)
}

/// This process's user's SID, as text (`S-1-5-21-…`).
pub fn current_user() -> io::Result<String> {
    with_user_sid(sid_string)
}

/// The owner, the DACL and whether it is protected, through `file`.
fn read_security(file: &File) -> io::Result<Security> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: an open handle with READ_CONTROL; `owner` and `dacl` point
    // into `descriptor`, which Windows allocates and `Local` frees after
    // the last use below.
    win32(unsafe {
        GetSecurityInfo(
            handle(file),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    })?;
    let descriptor = Local(descriptor);
    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: the descriptor just read.
    if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
        return Err(last_error());
    }
    let owner = if owner.is_null() {
        None
    } else {
        Some(sid_string(owner)?)
    };
    let dacl = if dacl.is_null() || control & SE_DACL_PRESENT == 0 {
        None
    } else {
        Some(read_aces(dacl)?)
    };
    Ok(Security {
        owner,
        dacl,
        protected: control & SE_DACL_PROTECTED != 0,
    })
}

/// The entries of `acl`.
fn read_aces(acl: *const ACL) -> io::Result<Vec<Ace>> {
    let mut info = ACL_SIZE_INFORMATION {
        AceCount: 0,
        AclBytesInUse: 0,
        AclBytesFree: 0,
    };
    // SAFETY: a valid ACL from GetSecurityInfo; `info` is the size class's
    // struct.
    if unsafe {
        GetAclInformation(
            acl,
            ptr::addr_of_mut!(info).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last_error());
    }
    let mut aces = Vec::with_capacity(info.AceCount as usize);
    for i in 0..info.AceCount {
        let mut ace: *mut c_void = null_mut();
        // SAFETY: `i` is below the ACL's count.
        if unsafe { GetAce(acl, i, &mut ace) } == 0 {
            return Err(last_error());
        }
        // SAFETY: every entry starts with an ACE_HEADER.
        let header = unsafe { ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        let kind = AceKind::of(header.AceType);
        let (mask, sid) = match kind {
            AceKind::Other => (0, String::new()),
            AceKind::Allow | AceKind::AllowCallback | AceKind::Deny | AceKind::DenyCallback => {
                // Allowed, denied and their callback forms share
                // ACCESS_ALLOWED_ACE's start: the header, the mask, then
                // the SID. Too short to hold that is a broken entry.
                if (header.AceSize as usize) < size_of::<ACCESS_ALLOWED_ACE>() + 4 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "an access entry is too short",
                    ));
                }
                let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                // SAFETY: the entry is at least that long (checked above).
                let mask = unsafe { ptr::read_unaligned(ptr::addr_of!((*allowed).Mask)) };
                let sid = unsafe { ptr::addr_of_mut!((*allowed).SidStart) }.cast::<c_void>();
                (mask, sid_string(sid)?)
            }
        };
        aces.push(Ace {
            kind,
            flags: header.AceFlags,
            mask,
            sid,
        });
    }
    Ok(aces)
}

/// A SID from its text, freed when dropped.
struct OwnedSid(Local);

impl OwnedSid {
    fn parse(text: &str) -> io::Result<OwnedSid> {
        let wide: Vec<u16> = std::ffi::OsStr::new(text)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut sid: PSID = null_mut();
        // SAFETY: a NUL-terminated string; `sid` receives memory freed by
        // `Local`.
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
            return Err(last_error());
        }
        Ok(OwnedSid(Local(sid)))
    }

    fn as_psid(&self) -> PSID {
        self.0 .0
    }
}

/// A file or folder opened as it is: a reparse point is opened, not
/// followed, and everything is read and set through this one handle, so
/// what is judged is what is changed. It shares read, write and delete, so
/// a running helper or an antivirus scan doesn't stop it.
pub struct Node {
    file: File,
    dir: bool,
    link: bool,
}

impl Node {
    /// Opens `path`. `dir`: a folder is expected, and is opened with
    /// `FILE_LIST_DIRECTORY | FILE_TRAVERSE` too, which a DACL set through
    /// the handle needs to reach the folder's existing children. `write`:
    /// with `WRITE_DAC`, to set the DACL.
    pub fn open(path: &Path, dir: bool, write: bool) -> io::Result<Node> {
        let mut access = READ_CONTROL | FILE_READ_ATTRIBUTES;
        if dir {
            access |= FILE_LIST_DIRECTORY | FILE_TRAVERSE;
        }
        if write {
            access |= WRITE_DAC;
        }
        let file = OpenOptions::new()
            .access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let mut info = FILE_ATTRIBUTE_TAG_INFO {
            FileAttributes: 0,
            ReparseTag: 0,
        };
        // SAFETY: an open handle; `info` is the class's struct.
        if unsafe {
            GetFileInformationByHandleEx(
                handle(&file),
                FileAttributeTagInfo,
                ptr::addr_of_mut!(info).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(last_error());
        }
        Ok(Node {
            file,
            dir: info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            link: is_link(info.FileAttributes, info.ReparseTag),
        })
    }

    /// Its owner, DACL, kind and whether it is a link.
    pub fn entry(&self) -> io::Result<Entry> {
        Ok(Entry {
            security: read_security(&self.file)?,
            dir: self.dir,
            link: self.link,
        })
    }

    /// Sets the DACL to exactly `aces`, in that order, protected (nothing
    /// inherited from the parent). Only allow and deny entries can be
    /// written. A link is refused ([`AclError::Link`]). The owner isn't
    /// changed.
    pub fn set_protected(&self, aces: &[Ace]) -> Result<(), AclError> {
        if self.link {
            return Err(AclError::Link);
        }
        let sids = aces
            .iter()
            .map(|a| OwnedSid::parse(&a.sid))
            .collect::<io::Result<Vec<_>>>()?;
        let entry = size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>();
        let mut len = size_of::<ACL>();
        for sid in &sids {
            // SAFETY: a SID ConvertStringSidToSidW made.
            len += entry + unsafe { GetLengthSid(sid.as_psid()) } as usize;
        }
        let mut acl = vec![0u32; len.div_ceil(4)];
        let acl_ptr: *mut ACL = acl.as_mut_ptr().cast();
        // SAFETY: `acl` is at least `len` bytes, aligned; every entry fits
        // by construction; the SIDs outlive the calls.
        unsafe {
            if InitializeAcl(acl_ptr, (acl.len() * 4) as u32, ACL_REVISION) == 0 {
                return Err(last_error().into());
            }
            for (ace, sid) in aces.iter().zip(&sids) {
                let flags = u32::from(ace.flags & !flags::INHERITED);
                let added = match ace.kind {
                    AceKind::Allow => {
                        AddAccessAllowedAceEx(acl_ptr, ACL_REVISION, flags, ace.mask, sid.as_psid())
                    }
                    AceKind::Deny => {
                        AddAccessDeniedAceEx(acl_ptr, ACL_REVISION, flags, ace.mask, sid.as_psid())
                    }
                    _ => {
                        return Err(AclError::Io(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "only allow and deny entries can be written",
                        )))
                    }
                };
                if added == 0 {
                    return Err(last_error().into());
                }
            }
            win32(SetSecurityInfo(
                handle(&self.file),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl_ptr,
                null(),
            ))?;
        }
        Ok(())
    }

    /// The private DACL: full control for this user and SYSTEM only,
    /// protected, inherited by a folder's children (`OI|CI`).
    pub fn make_private(&self) -> Result<(), AclError> {
        let inherit = if self.dir {
            flags::OBJECT_INHERIT | flags::CONTAINER_INHERIT
        } else {
            0
        };
        let user = current_user()?;
        self.set_protected(&[
            Ace::allow(&user, rights::FILE_ALL_ACCESS, inherit),
            Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, inherit),
        ])
    }
}

/// `path` as it is: a link (symlink, junction) is described, not followed.
pub fn inspect(path: &Path) -> io::Result<Entry> {
    Node::open(path, false, false)?.entry()
}

/// Sets `path`'s DACL to the private one ([`Node::make_private`]). `dir`:
/// a folder is expected; the other kind is [`AclError::WrongKind`] and a
/// link [`AclError::Link`], with nothing set.
pub fn make_private(path: &Path, dir: bool) -> Result<(), AclError> {
    let node = Node::open(path, dir, true)?;
    if node.link {
        return Err(AclError::Link);
    }
    if node.dir != dir {
        return Err(AclError::WrongKind);
    }
    node.make_private()
}

#[cfg(test)]
mod tests {
    use super::super::{Level, Problem, EVERYONE, SYSTEM};
    use super::*;
    use std::process::Command;

    fn icacls(path: &Path, args: &[&str]) {
        let out = Command::new("icacls")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "icacls {args:?}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    /// A local account's SID (`S-1-5-21-…`) or an Entra ID one
    /// (`S-1-12-1-…`).
    #[test]
    fn the_user_is_a_user_sid() {
        let user = current_user().unwrap();
        assert!(
            user.starts_with("S-1-5-") || user.starts_with("S-1-12-1-"),
            "{user}"
        );
        assert_ne!(user, SYSTEM);
    }

    /// What an install sets, read back: protected, the user and SYSTEM
    /// with full control, inherited by a folder's children and not by a
    /// file's (it has none).
    #[test]
    fn make_private_sets_a_protected_dacl_of_the_user_and_system() {
        let user = current_user().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("file.exe");
        std::fs::write(&file, b"x").unwrap();
        for (path, is_dir) in [(&folder, true), (&file, false)] {
            let before = inspect(path).unwrap();
            assert!(!before.security.protected, "inherited from %TEMP%");
            make_private(path, is_dir).unwrap();
            let after = inspect(path).unwrap();
            assert_eq!(after.dir, is_dir);
            assert!(!after.link);
            assert!(after.security.protected);
            assert!(after.security.is_private(&user), "{:?}", after.security);
            let inherit = if is_dir {
                flags::OBJECT_INHERIT | flags::CONTAINER_INHERIT
            } else {
                0
            };
            let mut aces = after.security.dacl.unwrap();
            aces.sort_by(|a, b| a.sid.cmp(&b.sid));
            let mut want = vec![
                Ace::allow(&user, rights::FILE_ALL_ACCESS, inherit),
                Ace::allow(SYSTEM, rights::FILE_ALL_ACCESS, inherit),
            ];
            want.sort_by(|a, b| a.sid.cmp(&b.sid));
            assert_eq!(aces, want);
        }
    }

    /// A folder's new DACL reaches its existing children. A file
    /// inside that inherited `%TEMP%`'s entries (Administrators among them)
    /// inherits the user's and SYSTEM's afterwards, and nothing else.
    #[test]
    fn a_folders_new_dacl_reaches_its_children() {
        let user = current_user().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let child = folder.join("child.txt");
        std::fs::write(&child, b"x").unwrap();
        let before = inspect(&child).unwrap().security;
        assert!(!before.protected);
        assert!(
            before
                .dacl
                .iter()
                .flatten()
                .any(|a| a.sid != user && a.sid != SYSTEM),
            "the child inherits more than the user and SYSTEM first: {before:?}"
        );
        make_private(&folder, true).unwrap();
        let after = inspect(&child).unwrap().security;
        assert!(!after.protected, "the child's own setting is kept");
        let aces = after.dacl.unwrap();
        assert!(!aces.is_empty());
        for ace in &aces {
            assert!(ace.flags & flags::INHERITED != 0, "{ace:?}");
            assert!(ace.sid == user || ace.sid == SYSTEM, "{ace:?}");
        }
    }

    /// `Everyone:(M)` on a folder is read as writable by
    /// another principal; making it private again clears it.
    #[test]
    fn everyone_modify_is_read_and_repaired() {
        let user = current_user().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let problem = || {
            inspect(&folder)
                .unwrap()
                .problem(&user, true, Level::Private)
        };
        assert_eq!(problem(), None);
        icacls(&folder, &["/grant", &format!("*{EVERYONE}:(M)")]);
        assert_eq!(problem(), Some(Problem::OthersCanWrite));
        make_private(&folder, true).unwrap();
        assert_eq!(problem(), None);
    }

    /// The root's repair through one handle: the entries written are read
    /// back as given, protected, and Administrators' entry stays.
    #[test]
    fn set_protected_writes_the_entries_given() {
        let user = current_user().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("root");
        std::fs::create_dir(&folder).unwrap();
        icacls(&folder, &["/grant", "*S-1-5-11:(OI)(CI)(M)"]);
        let node = Node::open(&folder, true, true).unwrap();
        let entry = node.entry().unwrap();
        assert_eq!(
            entry.problem(&user, true, Level::Root),
            Some(Problem::OthersCanWrite)
        );
        let repaired = entry.security.root_repair(&user).unwrap();
        node.set_protected(&repaired).unwrap();
        let after = node.entry().unwrap();
        assert!(after.security.protected);
        assert_eq!(after.security.dacl.as_ref(), Some(&repaired));
        assert_eq!(after.problem(&user, true, Level::Root), None);
        assert!(repaired
            .iter()
            .any(|a| a.sid == super::super::ADMINISTRATORS));
    }

    /// A junction is a link: described as one, never followed, and refused
    /// by `make_private` with its own error.
    #[test]
    fn a_junction_is_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let junction = dir.path().join("junction");
        let out = Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .output()
            .unwrap();
        assert!(out.status.success(), "mklink /J");
        let user = current_user().unwrap();
        assert_eq!(
            inspect(&junction)
                .unwrap()
                .problem(&user, true, Level::Private),
            Some(Problem::Link)
        );
        assert!(matches!(make_private(&junction, true), Err(AclError::Link)));
        assert!(!inspect(&real).unwrap().security.protected, "not followed");
    }

    #[test]
    fn the_wrong_kind_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            make_private(&file, true),
            Err(AclError::WrongKind)
        ));
        assert!(matches!(
            make_private(dir.path(), false),
            Err(AclError::WrongKind)
        ));
    }
}
