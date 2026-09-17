use crate::config::AppConfig;

#[cfg(target_os = "macos")]
static APPLIED_ICON: std::sync::Mutex<Option<termy_core::config_core::AppIcon>> =
    std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
const TERMY_DEFAULT_ICON_PNG: &[u8] = include_bytes!("../../../assets/termy_icon@1024px.png");
#[cfg(target_os = "macos")]
const TERMY_OLD_ICON_PNG: &[u8] = include_bytes!("../../../assets/termy_old_icon.png");

pub(crate) fn apply_from_config(config: &AppConfig) {
    apply(config.app_icon);
}

pub(crate) fn apply_at_startup(config: &AppConfig) {
    #[cfg(target_os = "macos")]
    if config.app_icon == termy_core::config_core::AppIcon::TermyDefault
        && std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.ancestors()
                    .find(|path| path.extension().is_some_and(|ext| ext == "app"))
                    .map(std::path::Path::to_path_buf)
            })
            .is_some_and(|bundle| can_use_bundled_icon(&bundle))
    {
        // Launch Services already supplied this icon. Replacing it decodes a
        // second full-size image and synchronously talks to the Dock again.
        *APPLIED_ICON
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(config.app_icon);
        return;
    }
    apply_from_config(config);
}

#[cfg(target_os = "macos")]
fn can_use_bundled_icon(bundle: &std::path::Path) -> bool {
    bundle.join("Contents/Resources/termy.icns").is_file()
        // Finder stores a directory's custom icon in this hidden resource file.
        // Keep the existing reset path when the user selected the old icon.
        && !bundle.join("Icon\r").exists()
}

pub(crate) fn apply(icon: termy_core::config_core::AppIcon) {
    #[cfg(target_os = "macos")]
    {
        let mut applied = APPLIED_ICON
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *applied == Some(icon) {
            return;
        }
        let icon_bytes = match icon {
            termy_core::config_core::AppIcon::TermyDefault => TERMY_DEFAULT_ICON_PNG,
            termy_core::config_core::AppIcon::TermyOld => TERMY_OLD_ICON_PNG,
        };

        crate::native_sdk::set_dock_icon_from_png(icon_bytes);
        crate::launch_probe::record_stage("dock_icon_applied");

        let persisted = match icon {
            termy_core::config_core::AppIcon::TermyDefault => {
                crate::native_sdk::clear_current_app_bundle_file_icon()
            }
            termy_core::config_core::AppIcon::TermyOld => {
                crate::native_sdk::set_current_app_bundle_file_icon_from_png(icon_bytes)
            }
        };

        if !persisted
            && std::env::current_exe().ok().is_some_and(|path| {
                path.ancestors()
                    .any(|p| p.extension().and_then(|ext| ext.to_str()) == Some("app"))
            })
        {
            log::warn!("Failed to persist selected Termy app icon on the app bundle");
        }
        *applied = Some(icon);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = icon;
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn startup_icon_reuse_requires_packaged_default_without_custom_finder_icon() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("Termy.app");
        let resources = bundle.join("Contents/Resources");
        std::fs::create_dir_all(&resources).unwrap();
        assert!(!can_use_bundled_icon(&bundle));
        std::fs::write(resources.join("termy.icns"), b"fixture").unwrap();
        assert!(can_use_bundled_icon(&bundle));
        std::fs::write(bundle.join("Icon\r"), b"custom").unwrap();
        assert!(!can_use_bundled_icon(&bundle));
    }
}
