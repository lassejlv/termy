use super::*;
use std::collections::HashMap;

impl SettingsWindow {
    fn bindable_actions() -> Vec<CommandId> {
        termy_command_core::command_specs()
            .iter()
            .map(|spec| spec.id)
            .collect()
    }

    fn action_title_from_config_name(config_name: &str) -> String {
        config_name
            .split('_')
            .filter(|segment| !segment.is_empty())
            .map(|segment| {
                let mut chars = segment.chars();
                match chars.next() {
                    Some(first) => {
                        let mut title = String::with_capacity(segment.len());
                        title.push(first.to_ascii_uppercase());
                        title.extend(chars);
                        title
                    }
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn collapse_resolved_keybinds_to_single_binding(
        resolved: &[termy_command_core::ResolvedKeybind],
    ) -> HashMap<CommandId, String> {
        let mut bindings = HashMap::with_capacity(resolved.len());
        for binding in resolved {
            bindings.insert(binding.action, binding.trigger.clone());
        }
        bindings
    }

    fn effective_action_bindings_from_lines(
        lines: &[termy_config_core::KeybindConfigLine],
    ) -> HashMap<CommandId, String> {
        let (directives, _warnings) =
            termy_command_core::parse_keybind_directives_from_iter(lines.iter().map(|line| {
                termy_command_core::KeybindLineRef {
                    line_number: line.line_number,
                    value: line.value.as_str(),
                }
            }));
        let resolved = termy_command_core::resolve_keybinds(
            termy_command_core::default_resolved_keybinds(),
            &directives,
        );
        Self::collapse_resolved_keybinds_to_single_binding(&resolved)
    }

    fn effective_action_bindings(&self) -> HashMap<CommandId, String> {
        Self::effective_action_bindings_from_lines(&self.config.keybind_lines)
    }

    fn serialize_structured_keybind_lines(bindings: &HashMap<CommandId, String>) -> Vec<String> {
        let mut entries = bindings
            .iter()
            .map(|(action, trigger)| (action.config_name(), trigger.clone()))
            .collect::<Vec<_>>();
        entries
            .sort_unstable_by(|left, right| left.0.cmp(right.0).then_with(|| left.1.cmp(&right.1)));

        let mut lines = Vec::with_capacity(entries.len() + 1);
        lines.push("clear".to_string());
        for (action_name, trigger) in entries {
            lines.push(format!("{trigger}={action_name}"));
        }
        lines
    }

    fn persist_action_bindings(
        &mut self,
        bindings: &HashMap<CommandId, String>,
        replaced_plugin_trigger: Option<&str>,
    ) -> Result<(), String> {
        let mut lines = Self::serialize_structured_keybind_lines(bindings);
        lines.extend(crate::keybindings::plugin_keybind_lines_for_settings(
            &self.config,
            replaced_plugin_trigger,
        ));
        config::set_keybind_lines(&lines)?;
        self.config.keybind_lines = lines
            .into_iter()
            .enumerate()
            .map(|(index, value)| termy_config_core::KeybindConfigLine {
                line_number: index + 1,
                value,
            })
            .collect();
        Ok(())
    }

    pub(super) fn reset_keybinds_to_defaults(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = config::set_keybind_lines(&[]) {
            crate::ui::toast::error(error);
            return;
        }

        self.config.keybind_lines.clear();
        self.capturing_action = None;
        cx.notify();
    }

    fn clear_action_binding(&mut self, action: CommandId, cx: &mut Context<Self>) {
        let mut bindings = self.effective_action_bindings();
        bindings.remove(&action);
        if let Err(error) = self.persist_action_bindings(&bindings, None) {
            crate::ui::toast::error(error);
            return;
        }
        self.capturing_action = None;
        cx.notify();
    }

    fn assign_action_binding(&mut self, action: CommandId, trigger: &str, cx: &mut Context<Self>) {
        let mut bindings = self.effective_action_bindings();
        Self::apply_assignment_with_conflict_resolution(&mut bindings, action, trigger);

        if let Err(error) = self.persist_action_bindings(&bindings, Some(trigger)) {
            crate::ui::toast::error(error);
            return;
        }

        self.capturing_action = None;
        cx.notify();
    }

    fn apply_assignment_with_conflict_resolution(
        bindings: &mut HashMap<CommandId, String>,
        action: CommandId,
        trigger: &str,
    ) {
        bindings.retain(|existing_action, existing_trigger| {
            *existing_action == action || existing_trigger != trigger
        });
        bindings.insert(action, trigger.to_string());
    }

    fn begin_action_binding_capture(
        &mut self,
        action: CommandId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.blur_sidebar_search();
        self.active_input = None;
        self.capturing_action = Some(action);
        self.focus_handle.focus(window);
        cx.notify();
    }

    fn has_no_modifiers(modifiers: gpui::Modifiers) -> bool {
        !modifiers.control
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.platform
            && !modifiers.function
    }

    fn is_modifier_only_key(key: &str) -> bool {
        matches!(
            key,
            "shift"
                | "control"
                | "ctrl"
                | "alt"
                | "option"
                | "command"
                | "cmd"
                | "super"
                | "meta"
                | "fn"
                | "function"
                | "secondary"
        )
    }

    fn canonicalize_captured_trigger(
        key: &str,
        modifiers: gpui::Modifiers,
    ) -> Result<Option<String>, String> {
        let normalized_key = key.trim().to_ascii_lowercase();
        if normalized_key.is_empty() || Self::is_modifier_only_key(&normalized_key) {
            return Ok(None);
        }

        let mut parts = Vec::new();
        let secondary = modifiers.secondary();
        if secondary {
            parts.push("secondary");
        }
        if modifiers.control && !secondary {
            parts.push("ctrl");
        }
        if modifiers.alt {
            parts.push("alt");
        }
        if modifiers.shift {
            parts.push("shift");
        }
        if modifiers.platform && !secondary {
            parts.push("cmd");
        }
        if modifiers.function {
            parts.push("fn");
        }

        let raw = if parts.is_empty() {
            normalized_key
        } else {
            format!("{}-{}", parts.join("-"), normalized_key)
        };

        termy_command_core::canonicalize_keybind_trigger(&raw).map(Some)
    }

    fn secondary_display_label() -> &'static str {
        if cfg!(target_os = "macos") {
            "CMD"
        } else {
            "CTRL"
        }
    }

    fn modifier_display_label(modifier: &str) -> String {
        match modifier {
            "secondary" => Self::secondary_display_label().to_string(),
            "ctrl" => "CTRL".to_string(),
            "alt" => {
                if cfg!(target_os = "macos") {
                    "OPT".to_string()
                } else {
                    "ALT".to_string()
                }
            }
            "shift" => "SHIFT".to_string(),
            "cmd" => "CMD".to_string(),
            "fn" => "FN".to_string(),
            other => other.to_ascii_uppercase(),
        }
    }

    fn key_display_label(key: &str) -> String {
        match key {
            "space" => "SPACE".to_string(),
            "enter" => "ENTER".to_string(),
            "escape" => "ESC".to_string(),
            "tab" => "TAB".to_string(),
            "backspace" => "BACKSPACE".to_string(),
            "delete" => "DELETE".to_string(),
            "home" => "HOME".to_string(),
            "end" => "END".to_string(),
            "pageup" => "PAGE UP".to_string(),
            "pagedown" => "PAGE DOWN".to_string(),
            "left" => "LEFT".to_string(),
            "right" => "RIGHT".to_string(),
            "up" => "UP".to_string(),
            "down" => "DOWN".to_string(),
            other if other.len() == 1 => other.to_ascii_uppercase(),
            other => other.to_ascii_uppercase(),
        }
    }

    fn display_trigger_for_os(trigger: &str) -> String {
        let displayed = trigger
            .split_whitespace()
            .filter(|component| !component.is_empty())
            .filter_map(|component| {
                let parts = component
                    .split('-')
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                let (key, modifiers) = parts.split_last()?;
                let key = Self::key_display_label(key);
                if modifiers.is_empty() {
                    return Some(key);
                }
                let modifiers = modifiers
                    .iter()
                    .map(|modifier| Self::modifier_display_label(modifier))
                    .collect::<Vec<_>>()
                    .join(" + ");
                Some(format!("{modifiers} + {key}"))
            })
            .collect::<Vec<_>>();

        if displayed.is_empty() {
            trigger.to_string()
        } else {
            displayed.join(" then ")
        }
    }

    pub(super) fn handle_keybind_capture(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(action) = self.capturing_action else {
            return;
        };

        let key = event.keystroke.key.as_str();
        let no_modifiers = Self::has_no_modifiers(event.keystroke.modifiers);
        if no_modifiers && key.eq_ignore_ascii_case("escape") {
            self.capturing_action = None;
            cx.notify();
            return;
        }

        if no_modifiers
            && (key.eq_ignore_ascii_case("backspace") || key.eq_ignore_ascii_case("delete"))
        {
            self.clear_action_binding(action, cx);
            return;
        }

        match Self::canonicalize_captured_trigger(key, event.keystroke.modifiers) {
            Ok(Some(trigger)) => self.assign_action_binding(action, &trigger, cx),
            Ok(None) => {}
            Err(error) => crate::ui::toast::error(format!("Invalid key combo: {error}")),
        }
    }

    fn is_optional_tab_shortcut(action: CommandId) -> bool {
        matches!(
            action,
            CommandId::SwitchTabLeft
                | CommandId::SwitchTabRight
                | CommandId::SwitchToTab1
                | CommandId::SwitchToTab2
                | CommandId::SwitchToTab3
                | CommandId::SwitchToTab4
                | CommandId::SwitchToTab5
                | CommandId::SwitchToTab6
                | CommandId::SwitchToTab7
                | CommandId::SwitchToTab8
                | CommandId::SwitchToTab9
        )
    }

    pub(super) fn render_keybinding_row(
        &self,
        action: CommandId,
        action_bindings: &HashMap<CommandId, String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let config_name = action.config_name();
        let action_title = if action == CommandId::CycleTabs {
            "Switch tabs".to_string()
        } else {
            Self::action_title_from_config_name(config_name)
        };
        let is_capturing = self.capturing_action == Some(action);
        let binding_display = if is_capturing {
            "Press shortcut…".to_string()
        } else {
            action_bindings.get(&action).map_or_else(
                || "Unbound".to_string(),
                |trigger| Self::display_trigger_for_os(trigger),
            )
        };
        let hover_bg = self.bg_hover();
        let accent = self.accent();
        let focus_ring = self.input_focus_ring();
        let description = (action == CommandId::CycleTabs)
            .then_some("Move to the next tab, wrapping after the last");
        div()
            .id(SharedString::from(format!("keybind-row-{config_name}")))
            .debug_selector(move || format!("keybind-row-{config_name}"))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .py(px(CARD_ROW_PADDING_Y))
            .px(px(CARD_ROW_PADDING_X))
            .when(is_capturing, |s| s.bg(self.accent_with_alpha(0.06)))
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
                            .child(action_title),
                    )
                    .children(description.map(|description| {
                        div()
                            .text_xs()
                            .line_height(px(16.0))
                            .text_color(self.text_muted())
                            .child(description)
                    })),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id(SharedString::from(format!("keybind-bind-{config_name}")))
                            .debug_selector(move || format!("keybind-bind-{config_name}"))
                            .w(px(SETTINGS_CONTROL_WIDTH))
                            .h(px(SETTINGS_CONTROL_HEIGHT))
                            .px(px(SETTINGS_CONTROL_INNER_PADDING))
                            .flex()
                            .items_center()
                            .rounded(px(SETTINGS_INPUT_RADIUS))
                            .bg(self.bg_input())
                            .border_1()
                            .border_color(if is_capturing {
                                accent
                            } else {
                                self.card_border_color()
                            })
                            .when(is_capturing, |s| {
                                s.shadow(vec![Self::focus_ring_shadow(focus_ring)])
                            })
                            .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                            .text_color(self.text_secondary())
                            .cursor_pointer()
                            .hover(move |s| s.bg(hover_bg))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                if view.capturing_action == Some(action) {
                                    view.capturing_action = None;
                                    cx.notify();
                                } else {
                                    view.begin_action_binding_capture(action, window, cx);
                                }
                            }))
                            .child(binding_display),
                    )
                    .child(self.render_setting_action_button(
                        SharedString::from(format!("keybind-clear-{config_name}")),
                        "icons/close.svg",
                        "Clear shortcut",
                        action_bindings.contains_key(&action),
                        cx,
                        move |view, _, cx| view.clear_action_binding(action, cx),
                    )),
            )
            .into_any_element()
    }

    pub(super) fn render_keybindings_section(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let action_bindings = self.effective_action_bindings();
        let actions = Self::bindable_actions();
        let mut tab_rows =
            vec![self.render_keybinding_row(CommandId::CycleTabs, &action_bindings, cx)];
        let hover_bg = self.bg_hover();
        tab_rows.push(
            div()
                .id("more-tab-shortcuts")
                .debug_selector(|| "more-tab-shortcuts".into())
                .px(px(CARD_ROW_PADDING_X))
                .py(px(CARD_ROW_PADDING_Y))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .text_size(px(SETTINGS_INPUT_TEXT_SIZE))
                .text_color(self.text_secondary())
                .hover(move |s| s.bg(hover_bg))
                .child(
                    svg()
                        .path(if self.show_more_tab_shortcuts {
                            "icons/settings/chevron-up.svg"
                        } else {
                            "icons/settings/chevron-down.svg"
                        })
                        .size(px(14.0))
                        .text_color(self.text_secondary()),
                )
                .child(if self.show_more_tab_shortcuts {
                    "Hide extra tab shortcuts"
                } else {
                    "More tab shortcuts"
                })
                .on_click(cx.listener(|view, _, _, cx| {
                    view.show_more_tab_shortcuts = !view.show_more_tab_shortcuts;
                    view.capturing_action = None;
                    cx.notify();
                }))
                .into_any_element(),
        );
        if self.show_more_tab_shortcuts {
            tab_rows.extend(
                actions
                    .iter()
                    .copied()
                    .filter(|action| Self::is_optional_tab_shortcut(*action))
                    .map(|action| self.render_keybinding_row(action, &action_bindings, cx)),
            );
        }
        let rows = actions
            .into_iter()
            .filter(|action| {
                *action != CommandId::CycleTabs && !Self::is_optional_tab_shortcut(*action)
            })
            .map(|action| self.render_keybinding_row(action, &action_bindings, cx))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap(px(CARD_GAP))
            .child(self.render_section_header(
                "Keyboard shortcuts",
                "Click a shortcut to record. Escape cancels; Backspace clears.",
                SettingsSection::Keybindings,
                cx,
            ))
            .child(self.wrap_setting_with_scroll_anchor(
                "keybind",
                self.render_settings_group("Tab switching", tab_rows),
            ))
            .child(self.render_settings_group("Other shortcuts", rows))
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsWindow;
    use std::collections::HashMap;
    use termy_command_core::{CommandId, ResolvedKeybind, command_specs};

    #[gpui::test]
    fn tab_shortcuts_expand_and_capture_cancels_without_changing_bindings(
        cx: &mut gpui::TestAppContext,
    ) {
        let (settings, cx) = cx.add_window_view(|window, cx| {
            let mut view = SettingsWindow::new(window, cx);
            view.active_section = super::SettingsSection::Keybindings;
            view.blur_sidebar_search();
            view.config.keybind_lines = vec![termy_config_core::KeybindConfigLine {
                line_number: 1,
                value: "alt-x=switch_to_tab_1".into(),
            }];
            view.focus_handle.focus(window);
            view
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("keybind-row-cycle_tabs").is_some());
        assert!(cx.debug_bounds("keybind-row-switch_to_tab_1").is_none());
        let other_shortcuts_y = cx.debug_bounds("keybind-row-new_tab").unwrap().origin.y;
        let more = cx
            .debug_bounds("more-tab-shortcuts")
            .expect("tab shortcut disclosure");
        cx.simulate_click(more.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.debug_bounds("keybind-row-switch_to_tab_1").is_some());
        settings.read_with(cx, |view, _| {
            assert_eq!(
                view.effective_action_bindings()
                    .get(&CommandId::SwitchToTab1)
                    .map(String::as_str),
                Some("alt-x")
            );
        });
        let record = cx
            .debug_bounds("keybind-bind-cycle_tabs")
            .expect("main tab switch shortcut");
        assert_eq!(record.size.width, gpui::px(super::SETTINGS_CONTROL_WIDTH));
        assert_eq!(record.size.height, gpui::px(super::SETTINGS_CONTROL_HEIGHT));
        cx.simulate_click(record.center(), gpui::Modifiers::default());
        settings.read_with(cx, |view, _| {
            assert_eq!(view.capturing_action, Some(CommandId::CycleTabs))
        });
        cx.simulate_keystrokes("escape");
        settings.read_with(cx, |view, _| {
            assert!(view.capturing_action.is_none());
            assert_eq!(view.config.keybind_lines.len(), 1);
            assert_eq!(view.config.keybind_lines[0].value, "alt-x=switch_to_tab_1");
        });
        let more = cx.debug_bounds("more-tab-shortcuts").unwrap();
        cx.simulate_click(more.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        settings.read_with(cx, |view, _| assert!(!view.show_more_tab_shortcuts));
        assert_eq!(
            cx.debug_bounds("keybind-row-new_tab").unwrap().origin.y,
            other_shortcuts_y
        );
        cx.simulate_resize(gpui::size(gpui::px(760.0), gpui::px(560.0)));
        cx.run_until_parked();
        let record = cx.debug_bounds("keybind-bind-cycle_tabs").unwrap();
        assert_eq!(record.size.width, gpui::px(super::SETTINGS_CONTROL_WIDTH));
        assert!(record.right() < gpui::px(760.0));
    }

    #[test]
    fn main_tab_switch_assignment_preserves_individual_tab_shortcuts() {
        let mut bindings = SettingsWindow::effective_action_bindings_from_lines(&[
            termy_config_core::KeybindConfigLine {
                line_number: 1,
                value: "alt-x=switch_to_tab_1".into(),
            },
        ]);
        SettingsWindow::apply_assignment_with_conflict_resolution(
            &mut bindings,
            CommandId::CycleTabs,
            "alt-tab",
        );
        let lines = SettingsWindow::serialize_structured_keybind_lines(&bindings)
            .into_iter()
            .enumerate()
            .map(|(index, value)| termy_config_core::KeybindConfigLine {
                line_number: index + 1,
                value,
            })
            .collect::<Vec<_>>();
        let reloaded = SettingsWindow::effective_action_bindings_from_lines(&lines);
        assert_eq!(
            reloaded.get(&CommandId::CycleTabs).map(String::as_str),
            Some("alt-tab")
        );
        assert_eq!(
            reloaded.get(&CommandId::SwitchToTab1).map(String::as_str),
            Some("alt-x")
        );
        assert!(!SettingsWindow::is_optional_tab_shortcut(
            CommandId::CycleTabs
        ));
        assert_eq!(
            SettingsWindow::bindable_actions()
                .into_iter()
                .filter(|action| SettingsWindow::is_optional_tab_shortcut(*action))
                .count(),
            11
        );
    }

    #[test]
    fn bindable_actions_match_command_catalog() {
        let actions = SettingsWindow::bindable_actions();
        let expected = command_specs()
            .iter()
            .map(|spec| spec.id)
            .collect::<Vec<_>>();
        assert_eq!(actions, expected);
    }

    #[test]
    fn collapse_resolved_keybinds_keeps_last_binding_per_action() {
        let resolved = vec![
            ResolvedKeybind {
                trigger: "secondary-c".to_string(),
                action: CommandId::Copy,
            },
            ResolvedKeybind {
                trigger: "secondary-v".to_string(),
                action: CommandId::Paste,
            },
            ResolvedKeybind {
                trigger: "ctrl-shift-c".to_string(),
                action: CommandId::Copy,
            },
        ];

        let collapsed = SettingsWindow::collapse_resolved_keybinds_to_single_binding(&resolved);
        assert_eq!(
            collapsed.get(&CommandId::Copy),
            Some(&"ctrl-shift-c".to_string())
        );
        assert_eq!(
            collapsed.get(&CommandId::Paste),
            Some(&"secondary-v".to_string())
        );
        assert_eq!(collapsed.len(), 2);
    }

    #[test]
    fn serialize_structured_keybind_lines_is_deterministic_and_includes_clear() {
        let mut bindings = HashMap::new();
        bindings.insert(CommandId::Paste, "secondary-v".to_string());
        bindings.insert(CommandId::Copy, "secondary-c".to_string());

        let lines = SettingsWindow::serialize_structured_keybind_lines(&bindings);
        assert_eq!(lines[0], "clear");
        assert_eq!(lines[1], "secondary-c=copy");
        assert_eq!(lines[2], "secondary-v=paste");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn assignment_conflict_moves_trigger_to_new_action() {
        let mut bindings = HashMap::new();
        bindings.insert(CommandId::Copy, "secondary-c".to_string());
        bindings.insert(CommandId::Paste, "secondary-v".to_string());

        SettingsWindow::apply_assignment_with_conflict_resolution(
            &mut bindings,
            CommandId::Paste,
            "secondary-c",
        );

        assert_eq!(
            bindings.get(&CommandId::Paste),
            Some(&"secondary-c".to_string())
        );
        assert!(!bindings.contains_key(&CommandId::Copy));
    }

    #[test]
    fn canonicalize_captured_trigger_supports_modifier_combos() {
        let modifiers = gpui::Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        };
        let trigger = SettingsWindow::canonicalize_captured_trigger("C", modifiers)
            .expect("should canonicalize")
            .expect("should produce trigger");
        assert_eq!(trigger, "alt-shift-c");
    }

    #[test]
    fn canonicalize_captured_trigger_ignores_modifier_only_keys() {
        let modifiers = gpui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let trigger = SettingsWindow::canonicalize_captured_trigger("shift", modifiers)
            .expect("modifier-only should not fail");
        assert!(trigger.is_none());
    }

    #[test]
    fn display_trigger_for_os_maps_secondary_to_native_modifier_label() {
        let rendered = SettingsWindow::display_trigger_for_os("secondary-n");
        #[cfg(target_os = "macos")]
        assert_eq!(rendered, "CMD + N");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(rendered, "CTRL + N");
    }
}
