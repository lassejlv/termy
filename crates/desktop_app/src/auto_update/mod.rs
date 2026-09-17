use gpui::{App, AsyncApp, WeakEntity};

mod engine;

pub use engine::{InstallOutcome, ReleaseInfo, UpdateCheck, UpdateState};

use engine::{
    cache_installer_path, check_for_updates, do_install, download_installer,
    verify_installer_checksum,
};

pub struct AutoUpdater {
    current_version: &'static str,
    pub state: UpdateState,
}

impl AutoUpdater {
    pub fn new(current_version: &'static str) -> Self {
        Self {
            current_version,
            state: UpdateState::Idle,
        }
    }

    pub fn supported_on_current_platform() -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }

    pub fn check(entity: WeakEntity<Self>, cx: &mut App) {
        let Some(this) = entity.upgrade() else { return };
        this.update(cx, |this, cx| {
            this.state = UpdateState::Checking;
            cx.notify();
        });

        let current_version = this.read(cx).current_version.to_string();
        let bg = cx
            .background_executor()
            .spawn(async move { check_for_updates(&current_version) });

        let weak = entity;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = bg.await;
            let _ = cx.update(|cx| {
                let Some(this) = weak.upgrade() else { return };
                this.update(cx, |this, cx| {
                    match result {
                        Ok(UpdateCheck::UpdateAvailable(info)) => {
                            this.state = UpdateState::Available {
                                version: info.version,
                                asset_name: info.asset_name,
                                url: info.download_url,
                                checksum_asset_name: info.checksum_asset_name,
                                checksum_url: info.checksum_url,
                                extension: info.extension,
                            };
                        }
                        Ok(UpdateCheck::UpToDate) => {
                            this.state = UpdateState::UpToDate;
                        }
                        Err(e) => {
                            log::warn!("Update check failed: {e}");
                            this.state = UpdateState::Error(format!("{e}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn install(entity: WeakEntity<Self>, cx: &mut App) {
        let Some(this) = entity.upgrade() else { return };

        let (version, asset_name, url, checksum_asset_name, checksum_url, extension) = {
            let read = this.read(cx);
            match &read.state {
                UpdateState::Available {
                    version,
                    asset_name,
                    url,
                    checksum_asset_name,
                    checksum_url,
                    extension,
                } => (
                    version.clone(),
                    asset_name.clone(),
                    url.clone(),
                    checksum_asset_name.clone(),
                    checksum_url.clone(),
                    extension.clone(),
                ),
                _ => return,
            }
        };

        this.update(cx, |this, cx| {
            this.state = UpdateState::Downloading {
                version: version.clone(),
                downloaded: 0,
                total: 0,
            };
            cx.notify();
        });

        let (progress_tx, progress_rx) = flume::bounded::<(u64, u64)>(4);
        let dest = cache_installer_path(&version, &extension);
        let dl_version = version.clone();
        let bg = cx.background_executor().spawn(async move {
            let path = download_installer(&url, &dest, progress_tx)?;
            verify_installer_checksum(
                &path,
                &asset_name,
                checksum_asset_name.as_deref(),
                checksum_url.as_deref(),
            )?;
            Ok::<std::path::PathBuf, anyhow::Error>(path)
        });

        let weak_progress = entity.clone();
        let progress_version = version.clone();
        cx.spawn(async move |cx: &mut AsyncApp| {
            while let Ok((downloaded, total)) = progress_rx.recv_async().await {
                let Some(this) = weak_progress.upgrade() else {
                    break;
                };
                let ver = progress_version.clone();
                let _ = cx.update(|cx| {
                    this.update(cx, |this, cx| {
                        if !matches!(
                            &this.state,
                            UpdateState::Downloading { version, .. } if version == &ver
                        ) {
                            return;
                        }
                        this.state = UpdateState::Downloading {
                            version: ver.clone(),
                            downloaded,
                            total,
                        };
                        cx.notify();
                    });
                });
            }
        })
        .detach();

        let weak_done = entity;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = bg.await;
            let _ = cx.update(|cx| {
                let Some(this) = weak_done.upgrade() else {
                    return;
                };
                match result {
                    Ok(path) => {
                        Self::start_install(this.downgrade(), dl_version, path, cx);
                    }
                    Err(e) => {
                        this.update(cx, |this, cx| {
                            this.state =
                                UpdateState::Error(format!("Download or verification failed: {e}"));
                            cx.notify();
                        });
                    }
                }
            });
        })
        .detach();
    }

    fn start_install(
        entity: WeakEntity<Self>,
        version: String,
        installer_path: std::path::PathBuf,
        cx: &mut App,
    ) {
        let Some(this) = entity.upgrade() else { return };

        this.update(cx, |this, cx| {
            this.state = UpdateState::Installing {
                version: version.clone(),
            };
            cx.notify();
        });

        let bg = cx
            .background_executor()
            .spawn(async move { do_install(&installer_path) });

        let weak = entity;
        cx.spawn(async move |cx: &mut AsyncApp| {
            let result = bg.await;
            let _ = cx.update(|cx| {
                let Some(this) = weak.upgrade() else { return };
                this.update(cx, |this, cx| {
                    match result {
                        Ok(InstallOutcome::Installed) => {
                            this.state = UpdateState::Installed { version };
                        }
                        Ok(InstallOutcome::InstallerLaunched) => {
                            this.state = UpdateState::InstallerLaunched { version };
                        }
                        Err(e) => {
                            this.state = UpdateState::Error(format!("Install failed: {e}"));
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn dismiss(&mut self, cx: &mut gpui::Context<Self>) {
        self.state = UpdateState::Idle;
        cx.notify();
    }
}
