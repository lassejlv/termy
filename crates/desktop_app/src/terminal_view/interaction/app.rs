use super::*;

impl TerminalView {
    pub(in super::super) fn execute_app_system_command_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::OpenConfig => {
                self.open_config_action(cx);
                true
            }
            CommandAction::PrettifyConfig => {
                self.prettify_config_action(cx);
                true
            }
            CommandAction::ImportColors => {
                self.import_colors_action(cx);
                true
            }
            CommandAction::AppInfo => {
                self.app_info_action(cx);
                true
            }
            CommandAction::OpenSettings => {
                self.open_settings_action(cx);
                true
            }
            CommandAction::NewWindow => {
                // Errors are already logged and surfaced as a toast.
                let _ = crate::open_main_window_with_runtime_config_overrides(cx, None);
                true
            }
            CommandAction::ReopenDetachedWindow => {
                self.reopen_detached_window_action(cx);
                true
            }
            CommandAction::CheckForUpdates => {
                self.check_for_updates_action(cx);
                true
            }
            CommandAction::ViewReleaseNotes => {
                self.view_release_notes_action(cx);
                true
            }
            _ => false,
        }
    }

    /// Closing one of several windows detaches its terminals; they keep
    /// running in the session host. Reopen the first such window.
    fn reopen_detached_window_action(&mut self, cx: &mut Context<Self>) {
        if !crate::multiplexer::queue_detached_window(cx) {
            crate::ui::toast::info(if self.multiplexer.is_some() {
                "No detached windows to reopen"
            } else {
                "Reopening detached windows needs the built-in multiplexer"
            });
            self.notify_overlay(cx);
            return;
        }
        // Errors are already logged and surfaced as a toast.
        let _ = crate::open_main_window_with_runtime_config_overrides(cx, None);
    }

    fn open_config_action(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = crate::app_actions::open_config_file() {
            log::error!("Failed to open config file from command action: {error}");
            crate::ui::toast::error(error);
            self.notify_overlay(cx);
        }
    }

    fn prettify_config_action(&mut self, cx: &mut Context<Self>) {
        match config::prettify_config_file() {
            Ok(_) => {
                self.reload_config(cx);
                cx.notify();
            }
            Err(error) => {
                log::error!("Failed to prettify config file from command action: {error}");
                crate::ui::toast::error(error);
                self.notify_overlay(cx);
            }
        }
    }

    fn import_colors_action(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let file = rfd::AsyncFileDialog::new()
                .add_filter("JSON", &["json"])
                .set_title("Import Colors")
                .pick_file()
                .await;

            let Some(file) = file else {
                return;
            };

            let path = file.path().to_path_buf();
            let result = config::import_colors_from_json(&path);

            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| match result {
                    Ok(msg) => {
                        crate::ui::toast::success(msg);
                        view.reload_config(cx);
                        cx.notify();
                    }
                    Err(err) => {
                        crate::ui::toast::error(err);
                        view.notify_overlay(cx);
                    }
                })
            });
        })
        .detach();
    }

    fn app_info_action(&mut self, cx: &mut Context<Self>) {
        let config_path = self.config_path.as_ref().map_or_else(
            || "unknown".to_string(),
            |path| path.to_string_lossy().into_owned(),
        );
        let message = format!(
            "Termy v{} | {}-{} | config: {}",
            crate::APP_VERSION,
            std::env::consts::OS,
            std::env::consts::ARCH,
            config_path
        );
        crate::ui::toast::info(message);
        self.notify_overlay(cx);
    }

    fn open_settings_action(&mut self, cx: &mut Context<Self>) {
        if self.simple_mode {
            self.open_config_action(cx);
            return;
        }

        if let Err(error) = crate::app_actions::open_settings_window(cx) {
            log::error!("{error}");
            crate::ui::toast::error(error);
            self.notify_overlay(cx);
        }
    }

    #[cfg(target_os = "macos")]
    fn check_for_updates_action(&mut self, cx: &mut Context<Self>) {
        // Sparkle shows its own checking, result and install UI.
        if !crate::auto_update::sparkle::check_for_updates(cx) {
            crate::ui::toast::info("Updates are not available in this build of Termy");
            self.notify_overlay(cx);
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn check_for_updates_action(&mut self, cx: &mut Context<Self>) {
        let Some(updater) = self.ensure_auto_updater(cx) else {
            crate::ui::toast::info("Auto updates are only available on macOS and Windows");
            self.notify_overlay(cx);
            return;
        };

        AutoUpdater::check(updater.downgrade(), cx);
        super::super::update_toasts::UpdateToasts::show_checking(cx);
        self.notify_overlay(cx);
    }

    fn view_release_notes_action(&mut self, cx: &mut Context<Self>) {
        self.open_release_notes(crate::APP_VERSION.to_string(), cx);
    }

    pub(in super::super) fn browse_release_notes_action(&mut self, cx: &mut Context<Self>) {
        if self.simple_mode {
            self.open_release_notes(crate::APP_VERSION.to_string(), cx);
            return;
        }
        self.open_command_palette_in_mode(CommandPaletteMode::Releases, cx);
    }
}
