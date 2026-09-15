//! macOS's terminal preference is the Shell role for Unix executables.
//! Keep other file associations and URL handlers untouched.
use core_foundation::{
    base::TCFType,
    bundle::CFBundle,
    string::{CFString, CFStringRef},
    url::{CFURL, CFURLRef},
};

const BUNDLE_ID: &str = "com.lassevestergaard.termy";
const SHELL_ROLE: u32 = 1 << 3;

// Launch Services exposes the Shell role explicitly, unlike NSWorkspace's
// general-purpose default application API.
#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn LSCopyDefaultRoleHandlerForContentType(content_type: CFStringRef, role: u32) -> CFStringRef;
    fn LSSetDefaultRoleHandlerForContentType(
        content_type: CFStringRef,
        role: u32,
        bundle_id: CFStringRef,
    ) -> i32;
    fn LSRegisterURL(url: CFURLRef, update: u8) -> i32;
}

pub(crate) fn is_default() -> bool {
    let content_type = CFString::new("public.unix-executable");
    // SAFETY: the input is a live CFString; Copy returns an owned nullable string.
    let handler = unsafe {
        LSCopyDefaultRoleHandlerForContentType(content_type.as_concrete_TypeRef(), SHELL_ROLE)
    };
    if handler.is_null() {
        return false;
    }
    // SAFETY: the non-null result follows Core Foundation's Copy ownership rule.
    let handler = unsafe { CFString::wrap_under_create_rule(handler) };
    handler.to_string().eq_ignore_ascii_case(BUNDLE_ID)
}

fn application_bundle_url() -> Result<CFURL, String> {
    let bundle = CFBundle::main_bundle();
    let info = bundle.info_dictionary();
    let identifier = info
        .find(CFString::new("CFBundleIdentifier"))
        .and_then(|value| value.downcast::<CFString>());
    if identifier.as_ref().map(ToString::to_string).as_deref() != Some(BUNDLE_ID) {
        return Err("Open the installed Termy.app to set it as your default terminal.".into());
    }
    bundle
        .bundle_url()
        .ok_or_else(|| "Could not locate the Termy app bundle.".into())
}

pub(crate) fn set_default() -> Result<(), String> {
    let url = application_bundle_url()?;
    // SAFETY: the bundle URL remains live throughout registration.
    let status = unsafe { LSRegisterURL(url.as_concrete_TypeRef(), 1) };
    if status != 0 {
        return Err(format!(
            "Could not register Termy with macOS (error {status})."
        ));
    }
    let content_type = CFString::new("public.unix-executable");
    let bundle_id = CFString::new(BUNDLE_ID);
    // SAFETY: both CFStrings remain live throughout the call.
    let status = unsafe {
        LSSetDefaultRoleHandlerForContentType(
            content_type.as_concrete_TypeRef(),
            SHELL_ROLE,
            bundle_id.as_concrete_TypeRef(),
        )
    };
    if status != 0 {
        return Err(format!(
            "Could not change the default terminal (macOS error {status})."
        ));
    }
    if !is_default() {
        return Err("macOS did not apply the default terminal change. Try again.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_terminal_requires_installed_app_bundle() {
        // This test binary is not an app bundle. Exercise the guard without
        // registering an application or changing the user's system preference.
        let error = super::application_bundle_url().unwrap_err();
        assert!(error.contains("Open the installed Termy.app"));
    }
}
