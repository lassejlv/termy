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

    fn check_for_updates_action(&mut self, cx: &mut Context<Self>) {
        let Some(updater) = self.ensure_auto_updater(cx) else {
            crate::ui::toast::info("Auto updates are only available on macOS and Windows");
            self.notify_overlay(cx);
            return;
        };

        AutoUpdater::check(updater.downgrade(), cx);
        self.update_check_toast_id = Some(crate::ui::toast::loading("Checking for updates"));
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
