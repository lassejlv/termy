use super::{OPEN_TAB_HERE_LABEL, explorer_open_tab_command};
use std::path::Path;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegQueryValueExW, RegSetValueExW,
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

pub(super) fn register(executable: &Path) -> Result<(), String> {
    let command = explorer_open_tab_command(executable);
    let icon = executable.to_string_lossy().into_owned();
    for subkey in [
        format!(r"Software\Classes\Directory\shell\{VERB}"),
        format!(r"Software\Classes\Directory\Background\shell\{VERB}"),
        format!(r"Software\Classes\Drive\shell\{VERB}"),
    ] {
        write_verb(&subkey, OPEN_TAB_HERE_LABEL, &icon, &command)?;
    }
    Ok(())
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
