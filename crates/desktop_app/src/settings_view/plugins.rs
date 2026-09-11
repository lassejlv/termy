use super::*;
use termy_plugin_runtime::{PluginInventory, PluginSetting};

const PLUGIN_SETTING_CONTROL_WIDTH: f32 = 228.0;
const PLUGIN_SETTING_CONTROL_HEIGHT: f32 = 28.0;

#[derive(Clone, Debug)]
pub(super) struct ActivePluginSettingInput {
    plugin_id: String,
    key: String,
    pub(super) state: TextInputState,
    selecting: bool,
}

struct PluginSettingsSnapshot {
    plugins: Vec<InstalledPlugin>,
    settings: BTreeMap<String, Vec<PluginSettingState>>,
    inventory_errors: Vec<String>,
    bun_path: Option<PathBuf>,
    bun_error: Option<String>,
}

impl SettingsWindow {
    fn load_plugin_settings_snapshot(runtime: &PluginRuntime) -> PluginSettingsSnapshot {
        let refresh = runtime.refresh_if_changed();
        let (plugins, inventory_errors) = match runtime.installed_plugins() {
            Ok(PluginInventory { plugins, errors }) => (plugins, errors),
            Err(error) => (Vec::new(), vec![error]),
        };
        let (bun_path, bun_error) = match runtime.bun_path() {
            Ok(path) => (path, None),
            Err(error) => (None, Some(error)),
        };
        let settings = runtime.plugin_settings_snapshot();
        runtime.suspend_if_eventless();
        let mut inventory_errors = inventory_errors;
        inventory_errors.extend(refresh.errors);
        inventory_errors.extend(settings.errors);
        PluginSettingsSnapshot {
            plugins,
            settings: settings.plugins,
            inventory_errors,
            bun_path,
            bun_error,
        }
    }

    fn apply_plugin_settings_snapshot(&mut self, snapshot: PluginSettingsSnapshot) {
        self.installed_plugins = snapshot.plugins;
        self.plugin_settings = snapshot.settings;
        self.plugin_inventory_errors = snapshot.inventory_errors;
        self.plugin_bun_path = snapshot.bun_path;
        self.plugin_bun_error = snapshot.bun_error;
        self.plugin_operation_in_flight = false;
    }

    pub(super) fn refresh_plugin_settings(&mut self, cx: &mut Context<Self>) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let snapshot =
                smol::unblock(move || Self::load_plugin_settings_snapshot(&runtime)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn set_plugin_setting_from_settings(
        &mut self,
        plugin_id: String,
        key: String,
        value: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        self.active_plugin_setting_select = None;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (result, snapshot) = smol::unblock(move || {
                let result = runtime.set_plugin_setting(&plugin_id, &key, value);
                let snapshot = Self::load_plugin_settings_snapshot(&runtime);
                (result, snapshot)
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    match result {
                        Ok(()) => crate::ui::toast::success("Plugin setting saved"),
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn reset_plugin_setting_from_settings(
        &mut self,
        plugin_id: String,
        key: String,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        self.plugin_setting_input = None;
        self.active_plugin_setting_select = None;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (result, snapshot) = smol::unblock(move || {
                let result = runtime.reset_plugin_setting(&plugin_id, &key);
                let snapshot = Self::load_plugin_settings_snapshot(&runtime);
                (result, snapshot)
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    match result {
                        Ok(()) => crate::ui::toast::success("Plugin setting reset"),
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn begin_plugin_setting_input(
        &mut self,
        plugin_id: String,
        key: String,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.blur_sidebar_search();
        self.active_input = None;
        self.active_plugin_setting_select = None;
        self.plugin_setting_input = Some(ActivePluginSettingInput {
            plugin_id,
            key,
            state: TextInputState::new(value),
            selecting: false,
        });
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn handle_plugin_setting_input_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.plugin_setting_input.as_mut() else {
            return;
        };
        cx.stop_propagation();
        let index = input.state.character_index_for_point(event.position);
        if event.modifiers.shift {
            input.state.select_to_utf16(index);
        } else if event.click_count >= 3 {
            input.state.select_all();
        } else if event.click_count == 2 {
            input.state.select_token_at_utf16(index);
        } else {
            input.state.set_cursor_utf16(index);
        }
        input.selecting = event.click_count == 1;
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn handle_plugin_setting_input_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.plugin_setting_input.as_mut() else {
            return;
        };
        if !input.selecting || !event.dragging() {
            return;
        }
        let index = input.state.character_index_for_point(event.position);
        input.state.select_to_utf16(index);
        cx.notify();
    }

    fn handle_plugin_setting_input_mouse_up(&mut self, cx: &mut Context<Self>) {
        if let Some(input) = self.plugin_setting_input.as_mut() {
            input.selecting = false;
            cx.notify();
        }
    }

    fn commit_plugin_setting_input(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.plugin_setting_input.take() else {
            return;
        };
        self.set_plugin_setting_from_settings(
            input.plugin_id,
            input.key,
            serde_json::Value::String(input.state.text().to_string()),
            cx,
        );
    }

    pub(super) fn handle_plugin_setting_input_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("a")
        {
            if let Some(input) = self.plugin_setting_input.as_mut() {
                input.state.select_all();
            }
            cx.notify();
            return;
        }
        if Self::cmd_only(event.keystroke.modifiers)
            && event.keystroke.key.eq_ignore_ascii_case("v")
        {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let text = text.replace(['\n', '\r'], "");
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.replace_text_in_range(None, &text);
                }
            }
            cx.notify();
            return;
        }
        match event.keystroke.key.as_str() {
            "enter" => self.commit_plugin_setting_input(cx),
            "escape" => {
                self.plugin_setting_input = None;
                cx.notify();
            }
            "backspace" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.delete_backward();
                }
                cx.notify();
            }
            "delete" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.delete_forward();
                }
                cx.notify();
            }
            "left" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.move_left();
                }
                cx.notify();
            }
            "right" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.move_right();
                }
                cx.notify();
            }
            "home" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.move_to_start();
                }
                cx.notify();
            }
            "end" => {
                if let Some(input) = self.plugin_setting_input.as_mut() {
                    input.state.move_to_end();
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn install_plugin_from_folder(&mut self, cx: &mut Context<Self>) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let folder = rfd::AsyncFileDialog::new()
                .set_title("Install Termy Plugin")
                .pick_folder()
                .await;
            let Some(folder) = folder else {
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.plugin_operation_in_flight = false;
                        cx.notify();
                    })
                });
                return;
            };

            let path = folder.path().to_path_buf();
            let folder_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("this plugin");
            let message = format!(
                "Install \"{folder_name}\"? Plugins are trusted local code and run with your user permissions."
            );
            if !termy_native_sdk::confirm("Install Plugin", &message) {
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.plugin_operation_in_flight = false;
                        cx.notify();
                    })
                });
                return;
            }

            let (result, snapshot) = smol::unblock(move || {
                let result = runtime.install_from_directory(&path);
                let snapshot = Self::load_plugin_settings_snapshot(&runtime);
                (result, snapshot)
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    match result {
                        Ok(plugin) => crate::ui::toast::success(format!(
                            "Installed {}. Open the command menu to use it.",
                            plugin.name
                        )),
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn set_plugin_enabled_from_settings(
        &mut self,
        id: String,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (result, snapshot) = smol::unblock(move || {
                let result = runtime.set_plugin_enabled(&id, enabled);
                let snapshot = Self::load_plugin_settings_snapshot(&runtime);
                (result, snapshot)
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    match result {
                        Ok(()) if enabled => crate::ui::toast::success("Plugin enabled"),
                        Ok(()) => crate::ui::toast::success("Plugin disabled"),
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn confirm_uninstall_plugin(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        if self.plugin_operation_in_flight {
            return;
        }
        self.plugin_operation_in_flight = true;
        cx.notify();
        let runtime = self.plugin_runtime.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let message = format!(
                "Uninstall \"{name}\"? Its copied plugin directory will be permanently removed."
            );
            if !termy_native_sdk::confirm("Uninstall Plugin", &message) {
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.plugin_operation_in_flight = false;
                        cx.notify();
                    })
                });
                return;
            }

            let (result, snapshot) = smol::unblock(move || {
                let result = runtime.uninstall_plugin(&id);
                let snapshot = Self::load_plugin_settings_snapshot(&runtime);
                (result, snapshot)
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.apply_plugin_settings_snapshot(snapshot);
                    match result {
                        Ok(()) => crate::ui::toast::success("Plugin uninstalled"),
                        Err(error) => crate::ui::toast::error(error),
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn open_plugins_directory(&mut self) {
        if let Err(error) = self.plugin_runtime.installed_plugins() {
            crate::ui::toast::error(error);
            return;
        }
        let Some(path) = self.plugin_runtime.plugins_directory() else {
            crate::ui::toast::error("Termy config path is unavailable");
            return;
        };

        #[cfg(target_os = "macos")]
        let result = Command::new("open")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        #[cfg(target_os = "linux")]
        let result = Command::new("xdg-open")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        #[cfg(target_os = "windows")]
        let result = Command::new("explorer")
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        if let Err(error) = result {
            crate::ui::toast::error(format!(
                "Failed to open plugin folder {}: {error}",
                path.display()
            ));
        }
    }

    fn render_plugin_setting_reset_button(
        &self,
        plugin_id: String,
        key: String,
        visible: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover = self.bg_hover();
        let text = self.text_muted();
        let text_hover = self.text_primary();
        div()
            .id(SharedString::from(format!(
                "plugin-setting-reset-{plugin_id}-{key}"
            )))
            .w(px(40.0))
            .h(px(24.0))
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(text)
            .when(!visible, |button| button.opacity(0.0))
            .when(visible, |button| {
                button
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover).text_color(text_hover))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.reset_plugin_setting_from_settings(plugin_id.clone(), key.clone(), cx);
                    }))
            })
            .child("Reset")
            .into_any_element()
    }

    fn render_plugin_text_setting_control(
        &self,
        plugin_id: String,
        key: String,
        value: String,
        placeholder: Option<String>,
        secret: bool,
        configured: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .plugin_setting_input
            .as_ref()
            .is_some_and(|input| input.plugin_id == plugin_id && input.key == key);
        let text_color = self.text_secondary();
        let idle_border = self.card_border_color();
        let accent = self.accent();
        let input_bg = self.bg_input();
        let font = Font {
            family: self.config.ui_font_family.clone().into(),
            ..gpui::font("")
        };
        let display = if secret {
            if configured {
                "••••••••".to_string()
            } else {
                placeholder.unwrap_or_else(|| "Not configured".to_string())
            }
        } else if value.is_empty() {
            placeholder.unwrap_or_else(|| "Empty".to_string())
        } else {
            value.clone()
        };
        let value_element = if active {
            if secret {
                let mut hidden = text_color;
                hidden.a = 0.0;
                let mut hidden_selection = self.accent_with_alpha(0.3);
                hidden_selection.a = 0.0;
                let masked = self
                    .plugin_setting_input
                    .as_ref()
                    .map_or_else(String::new, |input| {
                        if input.state.text().is_empty() {
                            String::new()
                        } else {
                            "•".repeat(input.state.text().chars().count().min(12))
                        }
                    });
                div()
                    .relative()
                    .size_full()
                    .child(TextInputElement::new(
                        cx.entity(),
                        self.focus_handle.clone(),
                        font,
                        px(SETTINGS_INPUT_TEXT_SIZE),
                        hidden.into(),
                        hidden_selection.into(),
                        TextInputAlignment::Left,
                    ))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .flex()
                            .items_center()
                            .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                            .text_color(text_color)
                            .child(masked),
                    )
                    .into_any_element()
            } else {
                TextInputElement::new(
                    cx.entity(),
                    self.focus_handle.clone(),
                    font,
                    px(SETTINGS_INPUT_TEXT_SIZE),
                    text_color.into(),
                    self.accent_with_alpha(0.3).into(),
                    TextInputAlignment::Left,
                )
                .into_any_element()
            }
        } else {
            div()
                .h_full()
                .flex()
                .items_center()
                .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                .text_color(if value.is_empty() || (secret && !configured) {
                    self.text_muted()
                } else {
                    text_color
                })
                .child(display)
                .into_any_element()
        };
        let begin_plugin = plugin_id.clone();
        let begin_key = key.clone();
        let initial_value = if secret { String::new() } else { value };
        div()
            .id(SharedString::from(format!(
                "plugin-setting-input-{plugin_id}-{key}"
            )))
            .w(px(PLUGIN_SETTING_CONTROL_WIDTH))
            .h(px(PLUGIN_SETTING_CONTROL_HEIGHT))
            .px_2()
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .bg(input_bg)
            .border_1()
            .border_color(if active { accent } else { idle_border })
            .overflow_hidden()
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let already_active = view.plugin_setting_input.as_ref().is_some_and(|input| {
                        input.plugin_id == begin_plugin && input.key == begin_key
                    });
                    if !already_active {
                        view.begin_plugin_setting_input(
                            begin_plugin.clone(),
                            begin_key.clone(),
                            initial_value.clone(),
                            window,
                            cx,
                        );
                    }
                    view.handle_plugin_setting_input_mouse_down(event, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|view, event: &MouseMoveEvent, _, cx| {
                view.handle_plugin_setting_input_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.handle_plugin_setting_input_mouse_up(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.handle_plugin_setting_input_mouse_up(cx)),
            )
            .child(value_element)
            .into_any_element()
    }

    fn render_plugin_select_setting_control(
        &self,
        plugin_id: String,
        key: String,
        value: String,
        options: Vec<termy_plugin_runtime::PluginSettingOption>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self
            .active_plugin_setting_select
            .as_ref()
            .is_some_and(|active| active.0 == plugin_id && active.1 == key);
        let label = options
            .iter()
            .find(|option| option.value == value)
            .map_or_else(|| value.clone(), |option| option.label.clone());
        let bg = self.bg_input();
        let border = if open {
            self.accent()
        } else {
            self.card_border_color()
        };
        let hover = self.bg_hover();
        let text = self.text_secondary();
        // The elevated color is a translucent foreground tint. Making that tint
        // opaque turns light themes into a white sheet, so use the theme's
        // actual background color for an opaque menu surface instead.
        let mut popup_bg = self.colors.background;
        popup_bg.a = 1.0;
        let popup_border = self.border_color();
        let toggle_plugin = plugin_id.clone();
        let toggle_key = key.clone();
        let mut menu = div()
            .id(SharedString::from(format!(
                "plugin-setting-options-{plugin_id}-{key}"
            )))
            .occlude()
            .absolute()
            .top(px(PLUGIN_SETTING_CONTROL_HEIGHT + 2.0))
            .left_0()
            .right_0()
            .py_1()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .bg(popup_bg)
            .border_1()
            .border_color(popup_border);
        for (index, option) in options.into_iter().enumerate() {
            let option_plugin = plugin_id.clone();
            let option_key = key.clone();
            let option_value = option.value;
            menu = menu.child(
                div()
                    .id(SharedString::from(format!(
                        "plugin-setting-option-{plugin_id}-{key}-{index}"
                    )))
                    .px_2()
                    .py(px(3.0))
                    .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                    .text_color(text)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.set_plugin_setting_from_settings(
                                option_plugin.clone(),
                                option_key.clone(),
                                serde_json::Value::String(option_value.clone()),
                                cx,
                            );
                        }),
                    )
                    .child(option.label),
            );
        }
        div()
            .id(SharedString::from(format!(
                "plugin-setting-select-{plugin_id}-{key}"
            )))
            .w(px(PLUGIN_SETTING_CONTROL_WIDTH))
            .h(px(PLUGIN_SETTING_CONTROL_HEIGHT))
            .relative()
            .px_2()
            .rounded(px(SETTINGS_INPUT_RADIUS))
            .bg(bg)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
            .text_color(text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, _, cx| {
                    cx.stop_propagation();
                    let next = (view.active_plugin_setting_select.as_ref()
                        != Some(&(toggle_plugin.clone(), toggle_key.clone())))
                    .then(|| (toggle_plugin.clone(), toggle_key.clone()));
                    view.active_plugin_setting_select = next;
                    view.plugin_setting_input = None;
                    cx.notify();
                }),
            )
            .child(label)
            .child(
                svg()
                    .path(SharedString::from(if open {
                        "icons/settings/chevron-up.svg"
                    } else {
                        "icons/settings/chevron-down.svg"
                    }))
                    .size(px(12.0))
                    .text_color(self.text_muted()),
            )
            .when(open, |control| {
                control.child(deferred(menu).with_priority(20))
            })
            .into_any_element()
    }

    fn render_plugin_setting_row(
        &self,
        plugin_id: String,
        setting: PluginSettingState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = setting.definition.id().to_string();
        let title = setting.definition.title().to_string();
        let description = setting.definition.description().map_or_else(
            || match &setting.definition {
                PluginSetting::Toggle { .. } => "Enable or disable this behavior.".to_string(),
                PluginSetting::Text { .. } => "Text used by this plugin.".to_string(),
                PluginSetting::Select { .. } => "Choose how this plugin behaves.".to_string(),
                PluginSetting::Secret { .. } => {
                    "Stored securely in the operating-system credential store.".to_string()
                }
            },
            str::to_string,
        );
        let control = match &setting.definition {
            PluginSetting::Toggle { .. } => {
                let checked = setting
                    .value
                    .as_ref()
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let toggle_plugin = plugin_id.clone();
                let toggle_key = key.clone();
                self.render_switch(
                    SharedString::from(format!("plugin-setting-toggle-{plugin_id}-{key}")),
                    checked,
                    cx,
                    move |view, _, cx| {
                        view.set_plugin_setting_from_settings(
                            toggle_plugin.clone(),
                            toggle_key.clone(),
                            serde_json::Value::Bool(!checked),
                            cx,
                        );
                    },
                )
                .into_any_element()
            }
            PluginSetting::Text { placeholder, .. } => self.render_plugin_text_setting_control(
                plugin_id.clone(),
                key.clone(),
                setting
                    .value
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                placeholder.clone(),
                false,
                setting.configured,
                cx,
            ),
            PluginSetting::Secret { placeholder, .. } => self.render_plugin_text_setting_control(
                plugin_id.clone(),
                key.clone(),
                String::new(),
                placeholder.clone(),
                true,
                setting.configured,
                cx,
            ),
            PluginSetting::Select { options, .. } => self.render_plugin_select_setting_control(
                plugin_id.clone(),
                key.clone(),
                setting
                    .value
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                options.clone(),
                cx,
            ),
        };
        let reset = self.render_plugin_setting_reset_button(plugin_id, key, setting.configured, cx);
        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .flex()
            .items_center()
            .gap_4()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_primary())
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .line_height(px(17.0))
                            .text_color(self.text_muted())
                            .child(description),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(control)
                    .child(reset),
            )
            .into_any_element()
    }

    pub(super) fn render_plugins_section(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut installed_rows = self
            .installed_plugins
            .clone()
            .into_iter()
            .map(|plugin| self.render_installed_plugin_row(plugin, cx))
            .collect::<Vec<_>>();
        if installed_rows.is_empty() {
            installed_rows.push(self.render_empty_plugins_row());
        }

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "Plugins",
                "Install and manage plugins",
                SettingsSection::Plugins,
                cx,
            ))
            .child(self.render_settings_group(
                "Runtime",
                vec![
                    self.render_plugin_runtime_row(),
                    self.render_plugin_actions_row(cx),
                ],
            ))
            .child(self.render_settings_group("Installed plugins", installed_rows));

        for (plugin_id, settings) in self.plugin_settings.clone() {
            if settings.is_empty() {
                continue;
            }
            let plugin_name = self
                .installed_plugins
                .iter()
                .find(|plugin| plugin.id == plugin_id)
                .map_or_else(|| plugin_id.clone(), |plugin| plugin.name.clone());
            let rows = settings
                .into_iter()
                .map(|setting| self.render_plugin_setting_row(plugin_id.clone(), setting, cx))
                .collect();
            section = section.child(self.render_settings_group(
                SharedString::from(format!("{plugin_name} settings")),
                rows,
            ));
        }

        if !self.plugin_inventory_errors.is_empty() || self.plugin_bun_error.is_some() {
            section = section.child(self.render_plugin_errors());
        }

        section.child(self.render_plugin_trust_notice())
    }

    fn render_plugin_runtime_row(&self) -> AnyElement {
        let ready = self.plugin_bun_path.is_some();
        let status = if ready { "Ready" } else { "Bun not found" };
        let detail = self.plugin_bun_path.as_ref().map_or_else(
            || "Install Bun to load plugins".to_string(),
            |path| path.display().to_string(),
        );
        let status_color = if ready {
            self.accent()
        } else {
            self.colors.ansi[3]
        };

        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_primary())
                            .child("Bun runtime"),
                    )
                    .child(div().text_xs().text_color(self.text_muted()).child(detail)),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(status_color)
                    .child(status),
            )
            .into_any_element()
    }

    fn render_plugin_actions_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.plugin_operation_in_flight;
        let border = self.border_color();
        let hover = self.bg_hover();
        let text_primary = self.text_primary();
        let text_secondary = self.text_secondary();
        let muted = self.text_muted();
        let button_bg = self.bg_input();
        let accent = self.accent();
        let accent_hover = self.accent_with_alpha(0.85);
        let button_text = self.contrasting_text_for_fill(accent, self.bg_card());

        let install_button = div()
            .id("plugin-install-folder")
            .h(px(28.0))
            .px_3()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .bg(accent)
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(button_text)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(busy, |button| button.opacity(0.55))
            .when(!busy, |button| {
                button
                    .hover(move |style| style.bg(accent_hover))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.install_plugin_from_folder(cx);
                    }))
            })
            .child(if busy {
                "Working..."
            } else {
                "Install from folder"
            });

        let open_button = div()
            .id("plugin-open-folder")
            .h(px(28.0))
            .px_3()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .border_1()
            .border_color(border)
            .bg(button_bg)
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(text_secondary)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |style| style.bg(hover).text_color(text_primary))
            .child("Open folder")
            .on_click(cx.listener(|view, _, _, _| view.open_plugins_directory()));

        let refresh_button = div()
            .id("plugin-refresh")
            .h(px(28.0))
            .px_3()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(muted)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(busy, |button| button.opacity(0.55))
            .when(!busy, |button| {
                button
                    .hover(move |style| style.bg(hover).text_color(text_primary))
                    .on_click(cx.listener(|view, _, _, cx| view.refresh_plugin_settings(cx)))
            })
            .child("Refresh");

        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.text_primary())
                            .child("Local plugins"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("Choose a folder containing plugin.json and plugin.ts"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(refresh_button)
                    .child(open_button)
                    .child(install_button),
            )
            .into_any_element()
    }

    fn render_empty_plugins_row(&self) -> AnyElement {
        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(20.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(self.text_primary())
                    .child("No plugins installed"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(self.text_muted())
                    .child("Install a local plugin folder to get started."),
            )
            .into_any_element()
    }

    fn render_installed_plugin_row(
        &self,
        plugin: InstalledPlugin,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.plugin_operation_in_flight;
        let border = self.border_color();
        let button_bg = self.bg_input();
        let hover = self.bg_hover();
        let text_primary = self.text_primary();
        let text_secondary = self.text_secondary();
        let muted = self.text_muted();
        let has_error = plugin.error.is_some();
        let enabled = plugin.enabled && !has_error;
        let status = if has_error {
            "Invalid"
        } else if plugin.enabled {
            "Enabled"
        } else {
            "Disabled"
        };
        let status_color = if has_error {
            self.colors.ansi[1]
        } else if enabled {
            self.accent()
        } else {
            muted
        };
        let detail = plugin.error.clone().unwrap_or_else(|| {
            plugin.version.as_ref().map_or_else(
                || plugin.id.clone(),
                |version| format!("{} · v{version}", plugin.id),
            )
        });
        let toggle_id = plugin.id.clone();
        let uninstall_id = plugin.id.clone();
        let uninstall_name = plugin.name.clone();

        let toggle_button = div()
            .id(SharedString::from(format!("plugin-toggle-{}", plugin.id)))
            .h(px(27.0))
            .px_3()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .border_1()
            .border_color(border)
            .bg(button_bg)
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(text_secondary)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(busy || has_error, |button| button.opacity(0.55))
            .when(!busy && !has_error, |button| {
                button
                    .hover(move |style| style.bg(hover).text_color(text_primary))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.set_plugin_enabled_from_settings(
                            toggle_id.clone(),
                            !plugin.enabled,
                            cx,
                        );
                    }))
            })
            .child(if plugin.enabled { "Disable" } else { "Enable" });

        let uninstall_button = div()
            .id(SharedString::from(format!(
                "plugin-uninstall-{}",
                plugin.id
            )))
            .h(px(27.0))
            .px_3()
            .rounded(px(SETTINGS_BUTTON_RADIUS))
            .text_xs()
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(muted)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .when(busy, |button| button.opacity(0.55))
            .when(!busy, |button| {
                button
                    .hover(move |style| style.bg(hover).text_color(text_primary))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.confirm_uninstall_plugin(
                            uninstall_id.clone(),
                            uninstall_name.clone(),
                            cx,
                        );
                    }))
            })
            .child("Uninstall");

        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(text_primary)
                            .child(plugin.name),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(if has_error { status_color } else { muted })
                            .overflow_x_hidden()
                            .child(detail),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(status_color)
                            .child(status),
                    )
                    .child(toggle_button)
                    .child(uninstall_button),
            )
            .into_any_element()
    }

    fn render_plugin_errors(&self) -> AnyElement {
        let mut messages = self.plugin_inventory_errors.clone();
        if let Some(error) = self.plugin_bun_error.clone() {
            messages.push(error);
        }
        let error_color = self.colors.ansi[1];
        let mut content = div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .rounded(px(SETTINGS_CARD_RADIUS))
            .bg(self.bg_elevated())
            .border_1()
            .border_color(self.card_border_color())
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(error_color)
                    .child("Plugin runtime needs attention"),
            );
        for message in messages {
            content = content.child(div().text_xs().text_color(self.text_muted()).child(message));
        }
        content.into_any_element()
    }

    fn render_plugin_trust_notice(&self) -> AnyElement {
        div()
            .w_full()
            .px(px(CARD_ROW_PADDING_X))
            .py(px(CARD_ROW_PADDING_Y))
            .rounded(px(SETTINGS_CARD_RADIUS))
            .bg(self.bg_elevated())
            .border_1()
            .border_color(self.card_border_color())
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(self.text_primary())
                    .child("Trusted local code"),
            )
            .child(div().text_xs().text_color(self.text_muted()).child(
                "Plugins run through Bun with your user permissions. Install only code you trust.",
            ))
            .into_any_element()
    }
}
