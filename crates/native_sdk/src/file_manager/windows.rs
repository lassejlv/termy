use super::{OPEN_TAB_HERE_LABEL, explorer_open_tab_command};
use std::path::Path;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegSetValueExW,
};
use windows::core::{PCWSTR, w};

const VERB: &str = "TermyOpenTab";

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
    let key = create_key(&format!(r"{subkey}"))?;
    set_sz(key, PCWSTR::null(), label)?;
    set_sz(key, w!("Icon"), icon)?;
    close_key(key);

    let command_key = create_key(&format!(r"{subkey}\command"))?;
    set_sz(command_key, PCWSTR::null(), command)?;
    close_key(command_key);
    Ok(())
}

fn create_key(path: &str) -> Result<windows::Win32::System::Registry::HKEY, String> {
    let wide = wide(path);
    let mut key = windows::Win32::System::Registry::HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(wide.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
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
    Ok(key)
}

fn set_sz(
    key: windows::Win32::System::Registry::HKEY,
    name: PCWSTR,
    value: &str,
) -> Result<(), String> {
    let wide = wide(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len().saturating_mul(2))
    };
    let status = unsafe { RegSetValueExW(key, name, Some(0), REG_SZ, Some(bytes)) };
    if status != ERROR_SUCCESS {
        return Err(format!("failed to write Explorer verb value: {:?}", status));
    }
    Ok(())
}

fn close_key(key: windows::Win32::System::Registry::HKEY) {
    unsafe {
        let _ = RegCloseKey(key);
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
