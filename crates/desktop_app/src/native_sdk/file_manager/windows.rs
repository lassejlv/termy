use super::{
    OPEN_TAB_HERE_LABEL, explorer_command_executable, explorer_open_tab_command,
    is_cargo_build_output,
};
use std::path::Path;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, w};

const VERB: &str = "TermyOpenTab";

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// The verb keys, relative to `HKCU` here and `HKLM` for the installer.
fn verb_subkeys() -> [String; 3] {
    [
        format!(r"Software\Classes\Directory\shell\{VERB}"),
        format!(r"Software\Classes\Directory\Background\shell\{VERB}"),
        format!(r"Software\Classes\Drive\shell\{VERB}"),
    ]
}

pub(super) fn register(executable: &Path) -> Result<(), String> {
    remove_stale_user_verbs();
    if machine_verbs_installed() {
        // The installer's machine-wide verbs already launch the installed
        // copy. Per-user ones would shadow them and outlive an uninstall.
        return unregister();
    }
    if is_cargo_build_output(executable) {
        log::debug!(
            "Skipping Explorer integration for development build {}",
            executable.display()
        );
        return Ok(());
    }

    let command = explorer_open_tab_command(executable);
    let icon = executable.to_string_lossy().into_owned();
    for subkey in verb_subkeys() {
        write_verb(&subkey, OPEN_TAB_HERE_LABEL, &icon, &command)?;
    }
    Ok(())
}

/// Deletes the per-user verbs. Missing keys are not an error.
pub(super) fn unregister() -> Result<(), String> {
    let errors: Vec<String> = verb_subkeys()
        .iter()
        .filter_map(|subkey| delete_user_key(subkey).err())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn machine_verbs_installed() -> bool {
    let subkey = wide(&verb_subkeys()[0]);
    let mut key = HKEY::default();
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status != ERROR_SUCCESS {
        return false;
    }
    drop(RegistryKey(key));
    true
}

/// Removes verbs that launch nothing (a moved or deleted portable copy) or a
/// Cargo build that earlier versions registered.
fn remove_stale_user_verbs() {
    for subkey in verb_subkeys() {
        let Some(command) = read_user_default_value(&format!(r"{subkey}\command")) else {
            continue;
        };
        let stale = explorer_command_executable(&command)
            .is_none_or(|path| !path.is_file() || is_cargo_build_output(&path));
        if stale && let Err(error) = delete_user_key(&subkey) {
            log::warn!("Could not remove stale Explorer verb: {error}");
        }
    }
}

fn read_user_default_value(subkey: &str) -> Option<String> {
    let subkey = wide(subkey);
    let mut byte_len = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS || byte_len == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (byte_len as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR::null(),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let value = &buffer[..(byte_len as usize / 2).min(buffer.len())];
    let end = value
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(value.len());
    Some(String::from_utf16_lossy(&value[..end]))
}

fn delete_user_key(subkey: &str) -> Result<(), String> {
    let wide = wide(subkey);
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr())) };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    Err(format!(
        "failed to delete Explorer verb {subkey}: {:?}",
        status
    ))
}

fn write_verb(subkey: &str, label: &str, icon: &str, command: &str) -> Result<(), String> {
    let key = create_key(subkey)?;
    set_sz(&key, PCWSTR::null(), label)?;
    set_sz(&key, w!("Icon"), icon)?;

    let command_key = create_key(&format!(r"{subkey}\command"))?;
    set_sz(&command_key, PCWSTR::null(), command)?;
    Ok(())
}

fn create_key(path: &str) -> Result<RegistryKey, String> {
    let wide = wide(path);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide.as_ptr()),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!(
            "failed to create Explorer verb {path}: {:?}",
            status
        ));
    }
    Ok(RegistryKey(key))
}

fn set_sz(key: &RegistryKey, name: PCWSTR, value: &str) -> Result<(), String> {
    let wide = wide(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len().saturating_mul(2))
    };
    let mut existing = vec![0u8; bytes.len()];
    let mut existing_len = existing.len() as u32;
    let mut existing_type = Default::default();
    let read_status = unsafe {
        RegQueryValueExW(
            key.0,
            name,
            None,
            Some(&mut existing_type),
            Some(existing.as_mut_ptr()),
            Some(&mut existing_len),
        )
    };
    if read_status == ERROR_SUCCESS
        && existing_type == REG_SZ
        && existing_len as usize == bytes.len()
        && existing == bytes
    {
        return Ok(());
    }
    let status = unsafe { RegSetValueExW(key.0, name, None, REG_SZ, Some(bytes)) };
    if status != ERROR_SUCCESS {
        return Err(format!("failed to write Explorer verb value: {:?}", status));
    }
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
