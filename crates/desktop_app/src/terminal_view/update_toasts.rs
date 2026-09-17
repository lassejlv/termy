use super::TerminalView;
use crate::auto_update::UpdateState;

#[derive(Debug, Clone, PartialEq, Eq)]
enum UpdateToastEffect {
    None,
    DismissProgressToast,
    Enqueue {
        kind: crate::ui::toast::ToastKind,
        message: String,
    },
    StartOrUpdateProgress {
        message: String,
    },
    FinishProgressOrEnqueue {
        kind: crate::ui::toast::ToastKind,
        message: String,
    },
}

fn update_toast_effect(state: Option<&UpdateState>) -> UpdateToastEffect {
    match state {
        Some(UpdateState::Available { version, .. }) => UpdateToastEffect::Enqueue {
            kind: crate::ui::toast::ToastKind::Info,
            message: format!("Update v{version} available"),
        },
        Some(UpdateState::Downloaded { version, .. }) => UpdateToastEffect::StartOrUpdateProgress {
            message: format!("Installing v{version}"),
        },
        Some(UpdateState::Installing { version }) => UpdateToastEffect::StartOrUpdateProgress {
            message: format!("Installing v{version}"),
        },
        Some(UpdateState::InstallerLaunched { version }) => {
            UpdateToastEffect::FinishProgressOrEnqueue {
                kind: crate::ui::toast::ToastKind::Info,
                message: format!(
                    "Installer launched for v{version}; Termy will reopen when setup finishes"
                ),
            }
        }
        Some(UpdateState::Installed { version }) => UpdateToastEffect::FinishProgressOrEnqueue {
            kind: crate::ui::toast::ToastKind::Success,
            message: installed_update_toast_message(version),
        },
        Some(UpdateState::Error(message)) => UpdateToastEffect::FinishProgressOrEnqueue {
            kind: crate::ui::toast::ToastKind::Error,
            message: format!("Update failed: {message}"),
        },
        Some(UpdateState::UpToDate) => UpdateToastEffect::DismissProgressToast,
        _ => UpdateToastEffect::None,
    }
}

fn installed_update_toast_message(version: &str) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("v{version} installed \u{2014} reopen from /Applications")
    }
    #[cfg(target_os = "windows")]
    {
        format!("v{} installed \u{2014} restart to apply", version)
    }
    #[cfg(target_os = "linux")]
    {
        format!(
            "v{} installed to ~/.local/bin \u{2014} restart to apply",
            version
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        format!("v{} installed \u{2014} restart to apply", version)
    }
}

impl TerminalView {
    pub(super) fn sync_update_toasts(&mut self, state: Option<&UpdateState>) {
        let changed = self.last_notified_update_state.as_ref() != state;
        if !changed {
            return;
        }

        self.last_notified_update_state = state.cloned();

        match update_toast_effect(state) {
            UpdateToastEffect::None => {}
            UpdateToastEffect::DismissProgressToast => {
                if let Some(id) = self.update_check_toast_id.take() {
                    crate::ui::toast::dismiss_toast(id);
                }
            }
            UpdateToastEffect::Enqueue { kind, message } => {
                if let Some(id) = self.update_check_toast_id.take() {
                    crate::ui::toast::dismiss_toast(id);
                }
                crate::ui::toast::enqueue_toast(kind, message, None);
            }
            UpdateToastEffect::StartOrUpdateProgress { message } => {
                if let Some(id) = self.update_check_toast_id {
                    crate::ui::toast::update_toast(
                        id,
                        crate::ui::toast::ToastKind::Loading,
                        message,
                    );
                } else {
                    self.update_check_toast_id = Some(crate::ui::toast::loading(message));
                }
            }
            UpdateToastEffect::FinishProgressOrEnqueue { kind, message } => {
                if let Some(id) = self.update_check_toast_id.take() {
                    crate::ui::toast::update_toast(id, kind, message);
                } else {
                    crate::ui::toast::enqueue_toast(kind, message, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn available_update_state_enqueues_independent_stack_entry() {
        assert_eq!(
            update_toast_effect(Some(&UpdateState::Available {
                version: "0.1.79".to_string(),
                asset_name: "Termy-v0.1.79-macos-arm64.dmg".to_string(),
                url: "https://example.com".to_string(),
                checksum_asset_name: Some("checksums.txt".to_string()),
                checksum_url: Some("https://example.com/checksums.txt".to_string()),
                extension: "dmg".to_string(),
            })),
            UpdateToastEffect::Enqueue {
                kind: crate::ui::toast::ToastKind::Info,
                message: "Update v0.1.79 available".to_string(),
            }
        );
    }

    #[test]
    fn downloaded_update_state_starts_install_progress() {
        assert_eq!(
            update_toast_effect(Some(&UpdateState::Downloaded {
                version: "0.1.79".to_string(),
                installer_path: PathBuf::from("/tmp/termy"),
            })),
            UpdateToastEffect::StartOrUpdateProgress {
                message: "Installing v0.1.79".to_string(),
            }
        );
    }

    #[test]
    fn progress_update_states_keep_one_mutable_loading_toast() {
        assert_eq!(
            update_toast_effect(Some(&UpdateState::Installing {
                version: "0.1.79".to_string(),
            })),
            UpdateToastEffect::StartOrUpdateProgress {
                message: "Installing v0.1.79".to_string(),
            }
        );
        assert!(matches!(
            update_toast_effect(Some(&UpdateState::Installed {
                version: "0.1.79".to_string(),
            })),
            UpdateToastEffect::FinishProgressOrEnqueue {
                kind: crate::ui::toast::ToastKind::Success,
                ..
            }
        ));
    }

    #[test]
    fn installer_launched_state_finishes_progress_toast() {
        assert_eq!(
            update_toast_effect(Some(&UpdateState::InstallerLaunched {
                version: "0.1.79".to_string(),
            })),
            UpdateToastEffect::FinishProgressOrEnqueue {
                kind: crate::ui::toast::ToastKind::Info,
                message: "Installer launched for v0.1.79; Termy will reopen when setup finishes"
                    .to_string(),
            }
        );
    }

    #[test]
    fn up_to_date_dismisses_progress_toast_without_adding_stack_entry() {
        assert_eq!(
            update_toast_effect(Some(&UpdateState::UpToDate)),
            UpdateToastEffect::DismissProgressToast
        );
        assert_eq!(update_toast_effect(None), UpdateToastEffect::None);
    }
}
