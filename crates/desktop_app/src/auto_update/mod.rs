use gpui_kit::{App, AppContext, AsyncApp, Entity, Global, WeakEntity};

mod engine;
#[cfg(target_os = "macos")]
pub mod sparkle;

pub use engine::{InstallOutcome, ReleaseInfo, UpdateCheck, UpdateState};

use engine::{
    cache_installer_path, check_for_updates, do_install, download_installer,
    verify_installer_checksum,
};

/// The one updater shared by every window, so the app checks, downloads and
/// installs each release once.
struct GlobalAutoUpdater(Entity<AutoUpdater>);
impl Global for GlobalAutoUpdater {}

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

    /// macOS updates go through [`sparkle`] instead.
    pub fn supported_on_current_platform() -> bool {
        cfg!(target_os = "windows")
    }

    /// Returns the app-wide updater, creating it on first use. `None` when
    /// this platform has no in-app installer (macOS uses Sparkle).
    pub fn global(cx: &mut App) -> Option<Entity<Self>> {
        if !Self::supported_on_current_platform() {
            return None;
        }
        if let Some(global) = cx.try_global::<GlobalAutoUpdater>() {
            return Some(global.0.clone());
        }
        let updater = cx.new(|_| Self::new(env!("CARGO_PKG_VERSION")));
        cx.set_global(GlobalAutoUpdater(updater.clone()));
        Some(updater)
    }

    /// A new check would only repeat or interrupt work already under way.
    fn busy(state: &UpdateState) -> bool {
        matches!(
            state,
            UpdateState::Checking
                | UpdateState::Downloading { .. }
                | UpdateState::Downloaded { .. }
                | UpdateState::Installing { .. }
                | UpdateState::InstallerLaunched { .. }
        )
    }

    /// Checks once per app run; later calls (e.g. from each new window's
    /// startup check) reuse the earlier result.
    pub fn check_if_idle(entity: WeakEntity<Self>, cx: &mut App) {
        let Some(this) = entity.upgrade() else { return };
        if this.read(cx).state == UpdateState::Idle {
            Self::check(entity, cx);
        }
    }

    pub fn check(entity: WeakEntity<Self>, cx: &mut App) {
        let Some(this) = entity.upgrade() else { return };
        if Self::busy(&this.read(cx).state) {
            return;
        }
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
            cx.update(|cx| {
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
                cx.update(|cx| {
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
            cx.update(|cx| {
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
            cx.update(|cx| {
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

    pub fn dismiss(&mut self, cx: &mut gpui_kit::Context<Self>) {
        self.state = UpdateState::Idle;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoUpdater, UpdateState};

    #[test]
    fn checks_do_not_restart_work_in_progress() {
        assert!(!AutoUpdater::busy(&UpdateState::Idle));
        assert!(!AutoUpdater::busy(&UpdateState::UpToDate));
        assert!(!AutoUpdater::busy(&UpdateState::Error(
            "offline".to_string()
        )));
        assert!(AutoUpdater::busy(&UpdateState::Checking));
        assert!(AutoUpdater::busy(&UpdateState::Downloading {
            version: "1.0.0".to_string(),
            downloaded: 1,
            total: 2,
        }));
        assert!(AutoUpdater::busy(&UpdateState::Installing {
            version: "1.0.0".to_string(),
        }));
    }
}
