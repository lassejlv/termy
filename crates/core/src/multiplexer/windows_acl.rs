//! Keeps a Windows session directory private to its user (audit L4). It holds
//! the endpoint token, and a directory on a shared drive (`C:\mux`) would
//! otherwise inherit permissions that let other users read the token and run
//! commands as this user. See `discovery::Root` for what is checked.
use anyhow::{Context, bail, ensure};
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree},
    Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
        },
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorDacl,
        GetTokenInformation, IsWellKnownSid, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        TokenUser, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid, WinCreatorOwnerRightsSid,
        WinCreatorOwnerSid, WinLocalSystemSid,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        READ_CONTROL, WRITE_DAC,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
/// Accounts that may own the directory and its files, and may be granted
/// access to them, besides this user.
const TRUSTED: [WELL_KNOWN_SID_TYPE; 2] = [WinLocalSystemSid, WinBuiltinAdministratorsSid];

/// Memory that a security API allocated with `LocalAlloc`.
struct LocalMemory(*mut c_void);

impl Drop for LocalMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from an API documented to allocate it
            // with LocalAlloc, and nothing else frees it.
            unsafe { LocalFree(self.0) };
        }
    }
}

/// The SID of the user this process runs as.
struct UserSid {
    /// The `TOKEN_USER` structure and the SID it points into.
    buffer: Vec<u64>,
}

impl UserSid {
    fn current() -> anyhow::Result<Self> {
        let mut token: HANDLE = ptr::null_mut();
        // SAFETY: the pseudo handle of this process needs no closing, and
        // `token` is writable.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error()).context("open this process's token");
        }
        let mut length = 0u32;
        // SAFETY: a null buffer of length zero asks only for the size.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut length) };
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        // SAFETY: `buffer` is writable, 8-byte aligned and at least `length`
        // bytes long.
        let read = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 8) as u32,
                &mut length,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: `token` was opened above and is closed once.
        unsafe { CloseHandle(token) };
        if read == 0 {
            return Err(error).context("read this process's user");
        }
        Ok(Self { buffer })
    }

    fn sid(&self) -> PSID {
        // SAFETY: the buffer holds the TOKEN_USER that GetTokenInformation
        // wrote; its SID pointer points into the same buffer.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn to_sddl(&self) -> anyhow::Result<String> {
        let mut text = ptr::null_mut();
        // SAFETY: the SID is valid while `self` lives; `text` is writable.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error()).context("format the user's SID");
        }
        let text = LocalMemory(text.cast());
        let wide = text.0.cast::<u16>();
        // SAFETY: the API returned a NUL-terminated UTF-16 string.
        let length = (0..)
            .take_while(|&index| unsafe { *wide.add(index) } != 0)
            .count();
        // SAFETY: `length` units before the terminator are initialized.
        let units = unsafe { std::slice::from_raw_parts(wide, length) };
        Ok(String::from_utf16_lossy(units))
    }

    /// Whether `sid` is this user or a [`TRUSTED`] account.
    fn trusts(&self, sid: PSID) -> bool {
        // SAFETY: both SIDs are valid for these calls.
        let user = unsafe { EqualSid(sid, self.sid()) } != 0;
        user || TRUSTED
            .into_iter()
            // SAFETY: as above.
            .any(|known| unsafe { IsWellKnownSid(sid, known) } != 0)
    }
}

/// The owner and DACL of an open file or directory.
struct Security {
    owner: PSID,
    dacl: *mut ACL,
    /// Holds the memory `owner` and `dacl` point into.
    _descriptor: LocalMemory,
}

impl Security {
    fn of(file: &File) -> io::Result<Self> {
        let (mut owner, mut dacl, mut descriptor) =
            (ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        // SAFETY: the handle is open with READ_CONTROL; the requested
        // outputs are writable and the others are optional.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        Ok(Self {
            owner,
            dacl,
            _descriptor: LocalMemory(descriptor),
        })
    }

    /// Whether the DACL grants access only to this user, the trusted
    /// accounts and the owner placeholders. Deny entries are fine; a null
    /// DACL grants everyone everything.
    fn is_private(&self, user: &UserSid) -> io::Result<bool> {
        if self.dacl.is_null() {
            return Ok(false);
        }
        let mut size = ACL_SIZE_INFORMATION::default();
        // SAFETY: `dacl` is valid while `self` lives, and `size` is writable
        // with the given length.
        if unsafe {
            GetAclInformation(
                self.dacl,
                (&raw mut size).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        for index in 0..size.AceCount {
            let mut ace: *mut c_void = ptr::null_mut();
            // SAFETY: `index` is below the ACE count and `ace` is writable.
            if unsafe { GetAce(self.dacl, index, &mut ace) } == 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: every ACE starts with an ACE_HEADER.
            match unsafe { (*ace.cast::<ACE_HEADER>()).AceType } {
                ACCESS_DENIED_ACE_TYPE => {}
                ACCESS_ALLOWED_ACE_TYPE => {
                    // SAFETY: an access-allowed ACE has this layout, and its
                    // SID starts at `SidStart` and lies within the ACE.
                    let sid: PSID =
                        unsafe { (&raw mut (*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart).cast() };
                    let owner_placeholder = [WinCreatorOwnerSid, WinCreatorOwnerRightsSid]
                        .into_iter()
                        // SAFETY: the SID is valid for this call.
                        .any(|known| unsafe { IsWellKnownSid(sid, known) } != 0);
                    if !owner_placeholder && !user.trusts(sid) {
                        return Ok(false);
                    }
                }
                // Callback and object entries do not occur on plain files.
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}

fn file_information(file: &File) -> io::Result<BY_HANDLE_FILE_INFORMATION> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle is open and `information` is writable.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information)
}

/// The volume and file index of an open file, which identify it.
pub(crate) fn file_id(file: &File) -> Option<(u32, u64)> {
    let information = file_information(file).ok()?;
    Some((
        information.dwVolumeSerialNumber,
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow),
    ))
}

/// Opens the directory `path` itself, not what a junction or symbolic link
/// there points to, and refuses those. The handle does not share delete
/// access, so while it is open the directory cannot be renamed or removed.
pub(crate) fn open_directory(path: &Path) -> anyhow::Result<File> {
    let open = |access| {
        OpenOptions::new()
            .access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    };
    let access = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL;
    // Without the right to change its permissions, it can still be checked.
    let directory = match open(access | WRITE_DAC) {
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => open(access),
        result => result,
    }
    .with_context(|| format!("open {}", path.display()))?;
    let attributes = file_information(&directory)
        .with_context(|| format!("open {}", path.display()))?
        .dwFileAttributes;
    ensure!(
        attributes & FILE_ATTRIBUTE_DIRECTORY != 0
            && attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "invalid multiplexer directory: {}",
        path.display()
    );
    Ok(directory)
}

fn not_private(path: &Path) -> anyhow::Error {
    anyhow::anyhow!(
        "multiplexer directory must be private to the current user: {} grants access to other accounts; remove their access or choose another directory",
        path.display()
    )
}

/// Checks the session directory opened by [`open_directory`]: it must be
/// owned by this user, SYSTEM or the administrators. If its DACL grants
/// anyone else access, it is replaced with a protected one granting full
/// control to this user and SYSTEM only (as for a directory an older Termy
/// created, or one created under `C:\`), and the directory is refused if
/// that is not allowed.
pub(crate) fn secure_directory(directory: &File, path: &Path) -> anyhow::Result<()> {
    let user = UserSid::current()?;
    let security = Security::of(directory)
        .with_context(|| format!("read the permissions of {}", path.display()))?;
    ensure!(
        user.trusts(security.owner),
        "multiplexer directory {} belongs to another account; choose another directory",
        path.display()
    );
    if security.is_private(&user)? {
        return Ok(());
    }
    protect(directory, &user).map_err(|error| not_private(path).context(error))?;
    ensure!(
        Security::of(directory)?.is_private(&user)?,
        not_private(path)
    );
    Ok(())
}

/// Fails unless `file`, opened in the session directory without following
/// reparse points, is a plain file owned by this user, SYSTEM or the
/// administrators: a file another account left there could be read by it.
pub(crate) fn check_file(file: &File) -> anyhow::Result<()> {
    let attributes = file_information(file)?.dwFileAttributes;
    ensure!(
        attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "a link in the session directory"
    );
    let security = Security::of(file)?;
    ensure!(
        UserSid::current()?.trusts(security.owner),
        "a file in the session directory belongs to another account"
    );
    Ok(())
}

/// Replaces the DACL of `directory` with a protected one granting full
/// control to `user` and SYSTEM, inherited by everything inside.
fn protect(directory: &File, user: &UserSid) -> anyhow::Result<()> {
    let sddl = format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})", user.to_sddl()?);
    let descriptor = security_descriptor(&sddl)?;
    let dacl = descriptor_dacl(&descriptor)?;
    // SAFETY: the handle is open (with WRITE_DAC, or the call fails) and
    // `dacl` points into `descriptor`, which outlives the call. Owner, group
    // and SACL stay unchanged.
    let status = unsafe {
        SetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32)).context("set a private DACL");
    }
    Ok(())
}

fn security_descriptor(sddl: &str) -> anyhow::Result<LocalMemory> {
    let sddl: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated; `descriptor` is writable and the
    // size output is optional.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error()).context("build a security descriptor");
    }
    Ok(LocalMemory(descriptor))
}

fn descriptor_dacl(descriptor: &LocalMemory) -> anyhow::Result<*mut ACL> {
    let (mut present, mut defaulted) = (0, 0);
    let mut dacl: *mut ACL = ptr::null_mut();
    // SAFETY: the descriptor is valid; the outputs are writable.
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) }
        == 0
        || present == 0
    {
        bail!("the security descriptor has no DACL");
    }
    Ok(dacl)
}

/// Whether the DACL of the directory `path` grants no one else access.
#[cfg(test)]
pub(crate) fn is_private(path: &Path) -> bool {
    let user = UserSid::current().unwrap();
    Security::of(&open_directory(path).unwrap())
        .unwrap()
        .is_private(&user)
        .unwrap()
}

/// Replaces the DACL of `path` with the one in the SDDL string `sddl`.
#[cfg(test)]
pub(crate) fn set_dacl(path: &Path, sddl: &str) -> anyhow::Result<()> {
    let directory = open_directory(path)?;
    let descriptor = security_descriptor(sddl)?;
    let dacl = descriptor_dacl(&descriptor)?;
    // SAFETY: as in `protect`.
    let status = unsafe {
        SetSecurityInfo(
            directory.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            dacl,
            ptr::null(),
        )
    };
    ensure!(
        status == ERROR_SUCCESS,
        "set the DACL of {}",
        path.display()
    );
    Ok(())
}
