//! Desktop update-banner presentation models.

use crate::auto_update::UpdateState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateBannerTone {
    Info,
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateBannerAction {
    Install,
    Restart,
    ViewReleaseNotes,
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateButtonStyle {
    Primary,
    Secondary,
    Ghost,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateBannerButton {
    pub label: &'static str,
    pub action: UpdateBannerAction,
    pub style: UpdateButtonStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateProgress {
    Determinate {
        percent: u8,
        caption: Option<String>,
    },
    Indeterminate {
        caption: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateBannerModel {
    pub badge: &'static str,
    pub message: String,
    pub detail: Option<String>,
    pub version: Option<String>,
    pub progress: Option<UpdateProgress>,
    pub tone: UpdateBannerTone,
    pub buttons: Vec<UpdateBannerButton>,
}

impl UpdateBannerModel {
    pub fn from_state(state: &UpdateState) -> Option<Self> {
        match state {
            UpdateState::Available { version, .. } => Some(Self {
                badge: "Update",
                message: format!("Version {version} is ready"),
                detail: Some("Install now to get the latest Termy.".to_string()),
                version: Some(version.clone()),
                progress: None,
                tone: UpdateBannerTone::Info,
                buttons: vec![
                    UpdateBannerButton {
                        label: "Install",
                        action: UpdateBannerAction::Install,
                        style: UpdateButtonStyle::Primary,
                    },
                    UpdateBannerButton {
                        label: "Later",
                        action: UpdateBannerAction::Dismiss,
                        style: UpdateButtonStyle::Ghost,
                    },
                ],
            }),
            UpdateState::Downloading {
                version,
                downloaded,
                total,
            } => {
                let progress = if *total > 0 {
                    let percent =
                        ((*downloaded as f64 / *total as f64) * 100.0).clamp(0.0, 100.0) as u8;
                    UpdateProgress::Determinate {
                        percent,
                        caption: Some(format!(
                            "{} of {}",
                            format_bytes(*downloaded),
                            format_bytes(*total)
                        )),
                    }
                } else {
                    UpdateProgress::Indeterminate {
                        caption: Some(format!("{} so far", format_bytes(*downloaded))),
                    }
                };

                Some(Self {
                    badge: "Downloading",
                    message: format!("Fetching version {version}"),
                    detail: Some("Keeping Termy current.".to_string()),
                    version: Some(version.clone()),
                    progress: Some(progress),
                    tone: UpdateBannerTone::Info,
                    buttons: vec![],
                })
            }
            UpdateState::Downloaded { version, .. } => Some(Self {
                badge: "Downloaded",
                message: format!("Version {version} is ready to install"),
                detail: Some("Starting the installer…".to_string()),
                version: Some(version.clone()),
                progress: Some(UpdateProgress::Determinate {
                    percent: 100,
                    caption: Some("Download complete".to_string()),
                }),
                tone: UpdateBannerTone::Success,
                buttons: vec![],
            }),
            UpdateState::Installing { version } => Some(Self {
                badge: "Installing",
                message: format!("Installing version {version}"),
                detail: Some("Finishing the last update steps…".to_string()),
                version: Some(version.clone()),
                progress: Some(UpdateProgress::Indeterminate {
                    caption: Some("This usually takes a moment".to_string()),
                }),
                tone: UpdateBannerTone::Info,
                buttons: vec![],
            }),
            UpdateState::InstallerLaunched { version } => Some(Self {
                badge: "Installer",
                message: format!("Version {version} installer launched"),
                detail: Some("Termy will quit and reopen when setup finishes.".to_string()),
                version: Some(version.clone()),
                progress: None,
                tone: UpdateBannerTone::Info,
                buttons: vec![],
            }),
            UpdateState::Installed { version } => Some(Self {
                badge: "Installed",
                message: format!("Version {version} is installed"),
                detail: Some("Restart Termy to start using it.".to_string()),
                version: Some(version.clone()),
                progress: None,
                tone: UpdateBannerTone::Success,
                buttons: vec![
                    UpdateBannerButton {
                        label: "Restart",
                        action: UpdateBannerAction::Restart,
                        style: UpdateButtonStyle::Primary,
                    },
                    UpdateBannerButton {
                        label: "View release notes",
                        action: UpdateBannerAction::ViewReleaseNotes,
                        style: UpdateButtonStyle::Secondary,
                    },
                    UpdateBannerButton {
                        label: "Dismiss",
                        action: UpdateBannerAction::Dismiss,
                        style: UpdateButtonStyle::Ghost,
                    },
                ],
            }),
            UpdateState::Error(message) => Some(Self {
                badge: "Failed",
                message: "Update failed".to_string(),
                detail: Some(message.clone()),
                version: None,
                progress: None,
                tone: UpdateBannerTone::Error,
                buttons: vec![UpdateBannerButton {
                    label: "Dismiss",
                    action: UpdateBannerAction::Dismiss,
                    style: UpdateButtonStyle::Ghost,
                }],
            }),
            UpdateState::Idle | UpdateState::Checking | UpdateState::UpToDate => None,
        }
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = bytes as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.0} KB", n / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn downloaded_state_has_no_manual_install_action() {
        let model = UpdateBannerModel::from_state(&UpdateState::Downloaded {
            version: "1.2.3".to_string(),
            installer_path: PathBuf::from("/tmp/termy-installer.dmg"),
        })
        .expect("downloaded state should render an update banner");

        assert_eq!(model.badge, "Downloaded");
        assert_eq!(
            model.progress,
            Some(UpdateProgress::Determinate {
                percent: 100,
                caption: Some("Download complete".to_string()),
            })
        );
        assert!(model.buttons.is_empty());
    }

    #[test]
    fn installed_state_exposes_restart_and_release_notes() {
        let model = UpdateBannerModel::from_state(&UpdateState::Installed {
            version: "1.2.3".to_string(),
        })
        .expect("installed state should render an update banner");

        let actions: Vec<_> = model.buttons.iter().map(|button| button.action).collect();
        assert_eq!(
            actions,
            vec![
                UpdateBannerAction::Restart,
                UpdateBannerAction::ViewReleaseNotes,
                UpdateBannerAction::Dismiss,
            ]
        );
        assert_eq!(model.version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn installer_launched_state_has_no_manual_action() {
        let model = UpdateBannerModel::from_state(&UpdateState::InstallerLaunched {
            version: "1.2.3".to_string(),
        })
        .expect("installer launched state should render an update banner");

        assert_eq!(model.badge, "Installer");
        assert_eq!(
            model.detail.as_deref(),
            Some("Termy will quit and reopen when setup finishes.")
        );
        assert!(model.buttons.is_empty());
    }

    #[test]
    fn downloading_state_reports_byte_progress() {
        let model = UpdateBannerModel::from_state(&UpdateState::Downloading {
            version: "1.2.3".to_string(),
            downloaded: 5 * 1024 * 1024,
            total: 10 * 1024 * 1024,
        })
        .expect("downloading state should render an update banner");

        assert_eq!(
            model.progress,
            Some(UpdateProgress::Determinate {
                percent: 50,
                caption: Some("5.0 MB of 10.0 MB".to_string()),
            })
        );
    }

    #[test]
    fn installing_state_uses_indeterminate_progress() {
        let model = UpdateBannerModel::from_state(&UpdateState::Installing {
            version: "1.2.3".to_string(),
        })
        .expect("installing state should render an update banner");

        assert!(matches!(
            model.progress,
            Some(UpdateProgress::Indeterminate { .. })
        ));
    }

    #[test]
    fn format_bytes_uses_readable_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
