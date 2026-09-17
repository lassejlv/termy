use crate::commands::CommandAction;
use crate::config::AppConfig;
use gpui::App;
#[cfg(debug_assertions)]
use gpui::Keystroke;
use log::warn;
use termy_core::command_core::{
    CommandId, KeybindDirective, KeybindLineRef, KeybindWarning, ResolvedKeybind,
    canonicalize_keybind_trigger, default_resolved_keybinds, parse_keybind_directives_from_iter,
    resolve_keybinds,
};
use termy_core::config_core::KeybindConfigLine;

const GLOBAL_KEYBIND_WARNING_LINE_NUMBER: usize = 0;
const PLUGIN_KEYBIND_PREFIX: &str = "plugin:";

pub fn install_keybindings(cx: &mut App, config: &AppConfig, tmux_enabled: bool) {
    let (command_lines, mut plugin_bindings, mut warnings) = partition_plugin_keybinds(config);
    let (mut resolved, command_warnings) =
        resolve_command_keybinds_for_lines(config, tmux_enabled, &command_lines);
    warnings.extend(command_warnings);
    let (task_bindings, task_warnings) = resolve_task_keybinds_for_config(config);
    warnings.extend(task_warnings);
    remove_command_keybinds_shadowed_by_tasks(&mut resolved, &task_bindings);
    remove_plugin_keybinds_shadowed_by_tasks(&mut plugin_bindings, &task_bindings);
    report_warnings(&warnings);
    let resolved = reorder_resolved_keybinds_for_menu_display(resolved);

    for binding in &resolved {
        debug_assert_trigger_is_valid_for_gpui(&binding.trigger);
    }
    for binding in &task_bindings {
        debug_assert_trigger_is_valid_for_gpui(&binding.trigger);
    }
    for binding in &plugin_bindings {
        debug_assert_trigger_is_valid_for_gpui(&binding.trigger);
    }

    cx.clear_key_bindings();
    // Keep menu shortcut glyphs in sync with custom keybinds by rebuilding menus
    // only after the canonical keymap bindings are reinstalled.
    cx.bind_keys(resolved.iter().map(|binding| {
        CommandAction::from_command_id(binding.action).to_key_binding(&binding.trigger)
    }));
    cx.bind_keys(task_bindings.into_iter().map(|binding| {
        let ResolvedTaskKeybind {
            trigger, task_name, ..
        } = binding;
        crate::commands::RunNamedTask { task_name }.into_key_binding(&trigger)
    }));
    cx.bind_keys(plugin_bindings.into_iter().map(|binding| {
        crate::commands::RunPluginCommand {
            plugin_id: binding.plugin_id,
            command_id: binding.command_id,
        }
        .into_key_binding(&binding.trigger)
    }));
    cx.bind_keys(crate::commands::inline_input_keybindings());
    cx.set_menus(crate::menus::app_menus(
        !termy_core::cli_install_core::is_cli_installed(),
        tmux_enabled,
        config.simple_mode,
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedTaskKeybind {
    line_number: usize,
    trigger: String,
    task_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPluginKeybind {
    trigger: String,
    plugin_id: String,
    command_id: String,
}

fn parse_plugin_keybind_action(action: &str) -> Result<Option<(String, String)>, String> {
    let Some(target) = action.strip_prefix(PLUGIN_KEYBIND_PREFIX) else {
        return Ok(None);
    };
    let Some((plugin_id, command_id)) = target.split_once('/') else {
        return Err("plugin keybind action must use `plugin:<plugin-id>/<command-id>`".to_string());
    };
    if !termy_core::plugin_runtime::valid_plugin_id(plugin_id) {
        return Err(format!("invalid plugin ID `{plugin_id}` in keybind action"));
    }
    if !termy_core::plugin_runtime::valid_plugin_id(command_id) {
        return Err(format!(
            "invalid plugin command ID `{command_id}` in keybind action"
        ));
    }
    Ok(Some((plugin_id.to_string(), command_id.to_string())))
}

fn canonical_trigger_for_assignment(
    line_number: usize,
    trigger: &str,
) -> Result<String, KeybindWarning> {
    let rewritten = format!("{trigger}=unbind");
    let (directives, mut warnings) = parse_keybind_directives_from_iter([KeybindLineRef {
        line_number,
        value: rewritten.as_str(),
    }]);
    if let Some(warning) = warnings.pop() {
        return Err(warning);
    }
    match directives.into_iter().next() {
        Some(KeybindDirective::Unbind { trigger }) => Ok(trigger),
        _ => Err(KeybindWarning {
            line_number,
            message: "invalid keybind trigger".to_string(),
        }),
    }
}

fn partition_plugin_keybinds(
    config: &AppConfig,
) -> (
    Vec<KeybindConfigLine>,
    Vec<ResolvedPluginKeybind>,
    Vec<KeybindWarning>,
) {
    let mut command_lines = Vec::with_capacity(config.keybind_lines.len());
    let mut plugin_bindings: Vec<ResolvedPluginKeybind> = Vec::new();
    let mut warnings = Vec::new();

    for line in &config.keybind_lines {
        let value = line.value.trim();
        if value.eq_ignore_ascii_case("clear") {
            plugin_bindings.clear();
            command_lines.push(line.clone());
            continue;
        }

        let Some((trigger_raw, action_raw)) = value.rsplit_once('=') else {
            command_lines.push(line.clone());
            continue;
        };
        let action_raw = action_raw.trim();
        match parse_plugin_keybind_action(action_raw) {
            Ok(Some((plugin_id, command_id))) => {
                let trigger = match canonical_trigger_for_assignment(line.line_number, trigger_raw)
                {
                    Ok(trigger) => trigger,
                    Err(warning) => {
                        warnings.push(warning);
                        continue;
                    }
                };
                plugin_bindings.retain(|binding| binding.trigger != trigger);
                plugin_bindings.push(ResolvedPluginKeybind {
                    trigger: trigger.clone(),
                    plugin_id,
                    command_id,
                });
                command_lines.push(KeybindConfigLine {
                    line_number: line.line_number,
                    value: format!("{trigger}=unbind"),
                });
            }
            Err(message) => warnings.push(KeybindWarning {
                line_number: line.line_number,
                message,
            }),
            Ok(None) => {
                if (action_raw.eq_ignore_ascii_case("unbind")
                    || CommandId::from_config_name(action_raw).is_some())
                    && let Ok(trigger) =
                        canonical_trigger_for_assignment(line.line_number, trigger_raw)
                {
                    plugin_bindings.retain(|binding| binding.trigger != trigger);
                }
                command_lines.push(line.clone());
            }
        }
    }

    (command_lines, plugin_bindings, warnings)
}

pub(crate) fn plugin_keybind_lines_for_settings(
    config: &AppConfig,
    replaced_trigger: Option<&str>,
) -> Vec<String> {
    let (_command_lines, plugin_bindings, _warnings) = partition_plugin_keybinds(config);
    plugin_bindings
        .into_iter()
        .filter(|binding| replaced_trigger != Some(binding.trigger.as_str()))
        .map(|binding| {
            format!(
                "{}=plugin:{}/{}",
                binding.trigger, binding.plugin_id, binding.command_id
            )
        })
        .collect()
}

fn resolve_task_keybinds_for_config(
    config: &AppConfig,
) -> (Vec<ResolvedTaskKeybind>, Vec<KeybindWarning>) {
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();

    for task in &config.tasks {
        let Some(keybind) = task.keybind.as_ref() else {
            continue;
        };
        match canonicalize_keybind_trigger(&keybind.value) {
            Ok(trigger) => candidates.push(ResolvedTaskKeybind {
                line_number: keybind.line_number,
                trigger,
                task_name: task.name.clone(),
            }),
            Err(message) => warnings.push(KeybindWarning {
                line_number: keybind.line_number,
                message: format!("invalid keybind for task \"{}\": {message}", task.name),
            }),
        }
    }

    candidates.sort_by_key(|binding| binding.line_number);
    let mut resolved: Vec<ResolvedTaskKeybind> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if let Some(index) = resolved
            .iter()
            .position(|binding| binding.trigger == candidate.trigger)
        {
            let overridden = resolved.remove(index);
            warnings.push(KeybindWarning {
                line_number: overridden.line_number,
                message: format!(
                    "task \"{}\" keybind `{}` is overridden by task \"{}\" on line {}",
                    overridden.task_name,
                    overridden.trigger,
                    candidate.task_name,
                    candidate.line_number
                ),
            });
        }
        resolved.push(candidate);
    }

    (resolved, warnings)
}

fn remove_command_keybinds_shadowed_by_tasks(
    command_bindings: &mut Vec<ResolvedKeybind>,
    task_bindings: &[ResolvedTaskKeybind],
) {
    command_bindings.retain(|binding| {
        !task_bindings
            .iter()
            .any(|task_binding| task_binding.trigger == binding.trigger)
    });
}

fn remove_plugin_keybinds_shadowed_by_tasks(
    plugin_bindings: &mut Vec<ResolvedPluginKeybind>,
    task_bindings: &[ResolvedTaskKeybind],
) {
    plugin_bindings.retain(|binding| {
        !task_bindings
            .iter()
            .any(|task_binding| task_binding.trigger == binding.trigger)
    });
}

fn reorder_resolved_keybinds_for_menu_display(
    resolved: Vec<ResolvedKeybind>,
) -> Vec<ResolvedKeybind> {
    use std::collections::HashMap;

    if resolved.len() < 2 {
        return resolved;
    }

    let mut last_index_by_action = HashMap::new();
    for (index, binding) in resolved.iter().enumerate() {
        last_index_by_action.insert(binding.action, index);
    }

    let mut primary = Vec::new();
    let mut secondary = Vec::new();
    for (index, binding) in resolved.into_iter().enumerate() {
        if last_index_by_action.get(&binding.action) == Some(&index) {
            primary.push(binding);
        } else {
            secondary.push(binding);
        }
    }

    primary.extend(secondary);
    primary
}

#[cfg(test)]
fn resolve_keybinds_for_config(
    config: &AppConfig,
    tmux_enabled: bool,
) -> (
    Vec<termy_core::command_core::ResolvedKeybind>,
    Vec<KeybindWarning>,
) {
    let (command_lines, _plugin_bindings, mut warnings) = partition_plugin_keybinds(config);
    let (resolved, command_warnings) =
        resolve_command_keybinds_for_lines(config, tmux_enabled, &command_lines);
    warnings.extend(command_warnings);
    (resolved, warnings)
}

fn resolve_command_keybinds_for_lines(
    config: &AppConfig,
    tmux_enabled: bool,
    keybind_lines: &[KeybindConfigLine],
) -> (
    Vec<termy_core::command_core::ResolvedKeybind>,
    Vec<KeybindWarning>,
) {
    let (directives, mut warnings) =
        parse_keybind_directives_from_iter(keybind_lines.iter().map(|line| KeybindLineRef {
            line_number: line.line_number,
            value: line.value.as_str(),
        }));

    let resolved = resolve_keybinds(default_resolved_keybinds(), &directives);
    let mut suppressed_tmux_only_bindings = 0usize;
    let mut suppressed_simple_mode_bindings = 0usize;
    let resolved = if tmux_enabled {
        resolved
    } else {
        // Native mode intentionally suppresses tmux-only actions. Emit an explicit
        // warning so this behavior is visible instead of silently dropping bindings.
        resolved
            .into_iter()
            .filter(|binding| {
                if binding.action.is_tmux_only() {
                    suppressed_tmux_only_bindings += 1;
                    return false;
                }
                true
            })
            .collect()
    };
    let resolved = if config.simple_mode {
        resolved
            .into_iter()
            .filter(|binding| {
                if binding.action == CommandId::ToggleCommandPalette {
                    suppressed_simple_mode_bindings += 1;
                    return false;
                }
                true
            })
            .collect()
    } else {
        resolved
    };

    if suppressed_tmux_only_bindings > 0 {
        warnings.push(KeybindWarning {
            line_number: GLOBAL_KEYBIND_WARNING_LINE_NUMBER,
            message: format!(
                "{suppressed_tmux_only_bindings} tmux-only keybind(s) ignored while tmux is disabled"
            ),
        });
    }
    if suppressed_simple_mode_bindings > 0 {
        warnings.push(KeybindWarning {
            line_number: GLOBAL_KEYBIND_WARNING_LINE_NUMBER,
            message: format!(
                "{suppressed_simple_mode_bindings} command palette keybind(s) ignored while Simple Mode is enabled"
            ),
        });
    }

    (resolved, warnings)
}

fn report_warnings(warnings: &[KeybindWarning]) {
    if warnings.is_empty() {
        return;
    }

    let mut keybind_warning_count = 0usize;
    for warning in warnings {
        if warning.line_number == GLOBAL_KEYBIND_WARNING_LINE_NUMBER {
            warn!("Ignoring keybind: {}", warning.message);
            continue;
        }

        keybind_warning_count += 1;
        warn!(
            "Ignoring keybind at config line {}: {}",
            warning.line_number, warning.message
        );
    }

    if keybind_warning_count > 0 {
        let noun = if keybind_warning_count == 1 {
            "line"
        } else {
            "lines"
        };
        crate::ui::toast::warning(format!("Ignored {keybind_warning_count} keybind {noun}"));
    }
}

#[cfg(debug_assertions)]
fn debug_assert_trigger_is_valid_for_gpui(trigger: &str) {
    for component in trigger.split_whitespace() {
        debug_assert!(
            Keystroke::parse(component).is_ok(),
            "command_core emitted unsupported GPUI keybind trigger component `{component}` from `{trigger}`"
        );
    }
}

#[cfg(not(debug_assertions))]
fn debug_assert_trigger_is_valid_for_gpui(_trigger: &str) {}

#[cfg(test)]
mod tests {
    use super::{
        partition_plugin_keybinds, remove_command_keybinds_shadowed_by_tasks,
        remove_plugin_keybinds_shadowed_by_tasks, reorder_resolved_keybinds_for_menu_display,
        resolve_keybinds_for_config, resolve_task_keybinds_for_config,
    };
    use crate::config::AppConfig;
    use termy_core::command_core::{
        CommandId, KeybindLineRef, ResolvedKeybind, default_resolved_keybinds,
        parse_keybind_directives_from_iter, resolve_keybinds,
    };
    use termy_core::config_core::KeybindConfigLine;

    fn fixture_keybind_lines() -> Vec<KeybindConfigLine> {
        vec![
            KeybindConfigLine {
                line_number: 1,
                value: "Secondary-P=toggle_command_palette".to_string(),
            },
            KeybindConfigLine {
                line_number: 2,
                value: "Control-Shift-C=copy".to_string(),
            },
            KeybindConfigLine {
                line_number: 3,
                value: "cmd-=zoom_in".to_string(),
            },
            KeybindConfigLine {
                line_number: 4,
                value: "secondary-p=unbind".to_string(),
            },
        ]
    }

    #[test]
    fn resolved_keybinds_match_command_core_for_same_fixture() {
        let config = AppConfig {
            keybind_lines: fixture_keybind_lines(),
            ..Default::default()
        };

        let (resolved_from_gui, warnings) = resolve_keybinds_for_config(&config, true);
        assert!(warnings.is_empty());

        let (directives, core_warnings) =
            parse_keybind_directives_from_iter(config.keybind_lines.iter().map(|line| {
                KeybindLineRef {
                    line_number: line.line_number,
                    value: line.value.as_str(),
                }
            }));
        assert!(core_warnings.is_empty());

        let resolved_from_core = resolve_keybinds(default_resolved_keybinds(), &directives);
        assert_eq!(resolved_from_gui, resolved_from_core);
    }

    #[test]
    fn resolved_keybinds_use_canonicalized_triggers() {
        let config = AppConfig {
            keybind_lines: fixture_keybind_lines(),
            ..Default::default()
        };

        let (resolved, warnings) = resolve_keybinds_for_config(&config, true);
        assert!(warnings.is_empty());
        assert!(
            resolved
                .iter()
                .any(|binding| binding.trigger == "ctrl-shift-c"
                    && binding.action.config_name() == "copy")
        );
        assert!(
            resolved
                .iter()
                .any(|binding| binding.trigger == "cmd-="
                    && binding.action.config_name() == "zoom_in")
        );
        assert!(
            resolved
                .iter()
                .all(|binding| !(binding.trigger == "secondary-p"
                    && binding.action.config_name() == "toggle_command_palette"))
        );
    }

    #[test]
    fn reorder_keybinds_promotes_latest_binding_per_action() {
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

        let reordered = reorder_resolved_keybinds_for_menu_display(resolved);
        let first_copy = reordered
            .iter()
            .find(|binding| binding.action == CommandId::Copy)
            .expect("missing copy binding");
        let first_paste = reordered
            .iter()
            .find(|binding| binding.action == CommandId::Paste)
            .expect("missing paste binding");

        assert_eq!(first_copy.trigger, "ctrl-shift-c");
        assert_eq!(first_paste.trigger, "secondary-v");
        assert!(
            reordered
                .iter()
                .any(|binding| binding.trigger == "secondary-c")
        );
    }

    #[test]
    fn reorder_keybinds_keeps_single_action_bindings_unchanged() {
        let resolved = vec![ResolvedKeybind {
            trigger: "secondary-t".to_string(),
            action: CommandId::NewTab,
        }];

        let reordered = reorder_resolved_keybinds_for_menu_display(resolved.clone());
        assert_eq!(reordered, resolved);
    }

    #[test]
    fn native_mode_keeps_resize_default_keybindings() {
        let config = AppConfig::default();

        let (tmux_resolved, tmux_warnings) = resolve_keybinds_for_config(&config, true);
        assert!(tmux_warnings.is_empty());
        assert!(
            tmux_resolved
                .iter()
                .any(|binding| binding.action == CommandId::ResizePaneLeft)
        );

        let (native_resolved, native_warnings) = resolve_keybinds_for_config(&config, false);
        assert!(native_warnings.is_empty());
        assert!(
            native_resolved
                .iter()
                .any(|binding| binding.action == CommandId::ResizePaneLeft)
        );
    }

    #[test]
    fn tmux_mode_keeps_focus_next_pane_default_keybind() {
        let config = AppConfig::default();
        let (resolved, warnings) = resolve_keybinds_for_config(&config, true);

        assert!(warnings.is_empty());
        assert!(resolved.iter().any(|binding| {
            binding.trigger == "secondary-o" && binding.action == CommandId::FocusPaneNext
        }));
    }

    #[test]
    fn simple_mode_suppresses_command_palette_keybinds() {
        let config = AppConfig {
            simple_mode: true,
            keybind_lines: vec![KeybindConfigLine {
                line_number: 10,
                value: "cmd-shift-p=toggle_command_palette".to_string(),
            }],
            ..Default::default()
        };

        let (resolved, warnings) = resolve_keybinds_for_config(&config, true);

        assert!(!warnings.is_empty());
        assert!(
            resolved
                .iter()
                .all(|binding| binding.action != CommandId::ToggleCommandPalette)
        );
        assert!(
            resolved
                .iter()
                .any(|binding| binding.action == CommandId::OpenSettings)
        );
    }

    #[test]
    fn keybind_resolution_keeps_non_tmux_actions_when_tmux_disabled() {
        let config = AppConfig {
            keybind_lines: vec![
                KeybindConfigLine {
                    line_number: 1,
                    value: "clear".to_string(),
                },
                KeybindConfigLine {
                    line_number: 10,
                    value: "secondary-d=split_pane_vertical".to_string(),
                },
            ],
            ..Default::default()
        };

        let (resolved, warnings) = resolve_keybinds_for_config(&config, false);
        assert!(warnings.is_empty());
        assert_eq!(
            resolved,
            vec![ResolvedKeybind {
                trigger: "secondary-d".to_string(),
                action: CommandId::SplitPaneVertical,
            }]
        );
    }

    #[test]
    fn task_keybinds_are_canonicalized_and_target_named_tasks() {
        let config = AppConfig::from_contents(
            "task.build.command = cargo build\n\
             task.build.keybind = Shift-Secondary-B\n",
        );

        let (resolved, warnings) = resolve_task_keybinds_for_config(&config);

        assert!(warnings.is_empty());
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].trigger, "shift-secondary-b");
        assert_eq!(resolved[0].task_name, "build");
    }

    #[test]
    fn later_task_keybind_wins_conflicts_by_config_line() {
        let config = AppConfig::from_contents(
            "task.zed.command = cargo test\n\
             task.zed.keybind = secondary-shift-t\n\
             task.alpha.command = cargo build\n\
             task.alpha.keybind = secondary-shift-t\n",
        );

        let (resolved, warnings) = resolve_task_keybinds_for_config(&config);

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].task_name, "alpha");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line_number, 2);
    }

    #[test]
    fn task_keybinds_shadow_command_bindings_with_the_same_trigger() {
        let config = AppConfig::from_contents(
            "task.build.command = cargo build\n\
             task.build.keybind = secondary-t\n",
        );
        let (task_bindings, warnings) = resolve_task_keybinds_for_config(&config);
        assert!(warnings.is_empty());

        let mut command_bindings = default_resolved_keybinds();
        remove_command_keybinds_shadowed_by_tasks(&mut command_bindings, &task_bindings);

        assert!(
            command_bindings
                .iter()
                .all(|binding| binding.trigger != "secondary-t")
        );
    }

    #[test]
    fn plugin_keybind_replaces_a_default_command_binding() {
        let config = AppConfig {
            keybind_lines: vec![KeybindConfigLine {
                line_number: 7,
                value: "Shift-Secondary-T=plugin:git-tools/status".to_string(),
            }],
            ..Default::default()
        };

        let (command_lines, plugin_bindings, warnings) = partition_plugin_keybinds(&config);
        assert!(warnings.is_empty());
        assert_eq!(
            plugin_bindings,
            vec![super::ResolvedPluginKeybind {
                trigger: "shift-secondary-t".to_string(),
                plugin_id: "git-tools".to_string(),
                command_id: "status".to_string(),
            }]
        );

        let command_config = AppConfig {
            keybind_lines: command_lines,
            ..config
        };
        let (commands, command_warnings) = resolve_keybinds_for_config(&command_config, true);
        assert!(command_warnings.is_empty());
        assert!(
            commands
                .iter()
                .all(|binding| binding.trigger != "shift-secondary-t")
        );
    }

    #[test]
    fn later_builtin_or_unbind_directives_replace_plugin_keybinds() {
        let config = AppConfig {
            keybind_lines: vec![
                KeybindConfigLine {
                    line_number: 1,
                    value: "secondary-g=plugin:git-tools/status".to_string(),
                },
                KeybindConfigLine {
                    line_number: 2,
                    value: "secondary-g=search_next".to_string(),
                },
                KeybindConfigLine {
                    line_number: 3,
                    value: "secondary-h=plugin:git-tools/history".to_string(),
                },
                KeybindConfigLine {
                    line_number: 4,
                    value: "secondary-h=unbind".to_string(),
                },
            ],
            ..Default::default()
        };

        let (_command_lines, plugin_bindings, warnings) = partition_plugin_keybinds(&config);
        assert!(warnings.is_empty());
        assert!(plugin_bindings.is_empty());
    }

    #[test]
    fn malformed_plugin_keybind_is_ignored_with_a_targeted_warning() {
        let config = AppConfig {
            keybind_lines: vec![KeybindConfigLine {
                line_number: 12,
                value: "secondary-g=plugin:git-tools".to_string(),
            }],
            ..Default::default()
        };

        let (_command_lines, plugin_bindings, warnings) = partition_plugin_keybinds(&config);
        assert!(plugin_bindings.is_empty());
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line_number, 12);
        assert!(
            warnings[0]
                .message
                .contains("plugin:<plugin-id>/<command-id>")
        );
    }

    #[test]
    fn task_keybinds_shadow_plugin_bindings_with_the_same_trigger() {
        let config = AppConfig::from_contents(
            "keybind = secondary-b=plugin:git-tools/branches\n\
             task.build.command = cargo build\n\
             task.build.keybind = secondary-b\n",
        );
        let (_command_lines, mut plugin_bindings, plugin_warnings) =
            partition_plugin_keybinds(&config);
        let (task_bindings, task_warnings) = resolve_task_keybinds_for_config(&config);
        assert!(plugin_warnings.is_empty());
        assert!(task_warnings.is_empty());

        remove_plugin_keybinds_shadowed_by_tasks(&mut plugin_bindings, &task_bindings);
        assert!(plugin_bindings.is_empty());
    }

    #[test]
    fn settings_preserve_only_effective_plugin_keybinds() {
        let config = AppConfig::from_contents(
            "keybind = secondary-a=plugin:git-tools/first\n\
             keybind = clear\n\
             keybind = secondary-b=plugin:git-tools/second\n\
             keybind = secondary-c=plugin:git-tools/third\n",
        );

        assert_eq!(
            super::plugin_keybind_lines_for_settings(&config, Some("secondary-c")),
            vec!["secondary-b=plugin:git-tools/second"]
        );
    }
}
