use crate::CommandId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultKeybind {
    pub trigger: &'static str,
    pub action: CommandId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeybindPlatform {
    MacOs,
    Windows,
    Linux,
    Other,
}

impl KeybindPlatform {
    pub const ALL: [Self; 4] = [Self::MacOs, Self::Windows, Self::Linux, Self::Other];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }

    pub const fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Self::Other
        }
    }
}

pub fn default_keybinds_for_platform(platform: KeybindPlatform) -> Vec<DefaultKeybind> {
    let mut bindings = vec![
        DefaultKeybind {
            trigger: "secondary-q",
            action: CommandId::Quit,
        },
        DefaultKeybind {
            trigger: "secondary-,",
            action: CommandId::OpenSettings,
        },
        DefaultKeybind {
            trigger: "secondary-p",
            action: CommandId::ToggleCommandPalette,
        },
        DefaultKeybind {
            trigger: "secondary-t",
            action: CommandId::NewTab,
        },
        DefaultKeybind {
            trigger: "secondary-w",
            action: CommandId::ClosePaneOrTab,
        },
        DefaultKeybind {
            trigger: "secondary-b",
            action: CommandId::ToggleWorkspaceSidebar,
        },
        DefaultKeybind {
            trigger: "ctrl-tab",
            action: CommandId::CycleTabs,
        },
        DefaultKeybind {
            trigger: "secondary-1",
            action: CommandId::SwitchToTab1,
        },
        DefaultKeybind {
            trigger: "secondary-2",
            action: CommandId::SwitchToTab2,
        },
        DefaultKeybind {
            trigger: "secondary-3",
            action: CommandId::SwitchToTab3,
        },
        DefaultKeybind {
            trigger: "secondary-4",
            action: CommandId::SwitchToTab4,
        },
        DefaultKeybind {
            trigger: "secondary-5",
            action: CommandId::SwitchToTab5,
        },
        DefaultKeybind {
            trigger: "secondary-6",
            action: CommandId::SwitchToTab6,
        },
        DefaultKeybind {
            trigger: "secondary-7",
            action: CommandId::SwitchToTab7,
        },
        DefaultKeybind {
            trigger: "secondary-8",
            action: CommandId::SwitchToTab8,
        },
        DefaultKeybind {
            trigger: "secondary-9",
            action: CommandId::SwitchToTab9,
        },
        DefaultKeybind {
            trigger: "secondary-d",
            action: CommandId::SplitPaneVertical,
        },
        DefaultKeybind {
            trigger: "secondary-shift-d",
            action: CommandId::SplitPaneHorizontal,
        },
        DefaultKeybind {
            trigger: "secondary-o",
            action: CommandId::FocusPaneNext,
        },
        DefaultKeybind {
            trigger: "secondary-alt-left",
            action: CommandId::FocusPaneLeft,
        },
        DefaultKeybind {
            trigger: "secondary-alt-right",
            action: CommandId::FocusPaneRight,
        },
        DefaultKeybind {
            trigger: "secondary-alt-up",
            action: CommandId::FocusPaneUp,
        },
        DefaultKeybind {
            trigger: "secondary-alt-down",
            action: CommandId::FocusPaneDown,
        },
        DefaultKeybind {
            trigger: "secondary-alt-shift-left",
            action: CommandId::ResizePaneLeft,
        },
        DefaultKeybind {
            trigger: "secondary-alt-shift-right",
            action: CommandId::ResizePaneRight,
        },
        DefaultKeybind {
            trigger: "secondary-alt-shift-up",
            action: CommandId::ResizePaneUp,
        },
        DefaultKeybind {
            trigger: "secondary-alt-shift-down",
            action: CommandId::ResizePaneDown,
        },
        DefaultKeybind {
            trigger: "secondary-enter",
            action: CommandId::TogglePaneZoom,
        },
        DefaultKeybind {
            trigger: "secondary-=",
            action: CommandId::ZoomIn,
        },
        DefaultKeybind {
            trigger: "secondary-+",
            action: CommandId::ZoomIn,
        },
        DefaultKeybind {
            trigger: "secondary--",
            action: CommandId::ZoomOut,
        },
        DefaultKeybind {
            trigger: "secondary-0",
            action: CommandId::ZoomReset,
        },
        DefaultKeybind {
            trigger: "secondary-f",
            action: CommandId::OpenSearch,
        },
        DefaultKeybind {
            trigger: "secondary-g",
            action: CommandId::SearchNext,
        },
        DefaultKeybind {
            trigger: "secondary-shift-g",
            action: CommandId::SearchPrevious,
        },
        DefaultKeybind {
            trigger: "secondary-alt-i",
            action: CommandId::ToggleInspector,
        },
    ];

    if matches!(platform, KeybindPlatform::MacOs) {
        bindings.push(DefaultKeybind {
            trigger: "secondary-m",
            action: CommandId::MinimizeWindow,
        });
    }

    if matches!(platform, KeybindPlatform::MacOs | KeybindPlatform::Windows) {
        bindings.push(DefaultKeybind {
            trigger: "secondary-c",
            action: CommandId::Copy,
        });
        bindings.push(DefaultKeybind {
            trigger: "secondary-v",
            action: CommandId::Paste,
        });
        bindings.push(DefaultKeybind {
            trigger: "secondary-a",
            action: CommandId::SelectAll,
        });
    } else {
        bindings.push(DefaultKeybind {
            trigger: "ctrl-shift-c",
            action: CommandId::Copy,
        });
        bindings.push(DefaultKeybind {
            trigger: "ctrl-shift-v",
            action: CommandId::Paste,
        });
        bindings.push(DefaultKeybind {
            trigger: "ctrl-shift-a",
            action: CommandId::SelectAll,
        });
    }

    bindings
}

pub fn default_keybinds_for_current_platform() -> Vec<DefaultKeybind> {
    default_keybinds_for_platform(KeybindPlatform::current())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeybindLineRef<'a> {
    pub line_number: usize,
    pub value: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindDirective {
    Clear,
    Bind { trigger: String, action: CommandId },
    Unbind { trigger: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeybindWarning {
    pub line_number: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeybind {
    pub trigger: String,
    pub action: CommandId,
}

pub fn default_resolved_keybinds_for_platform(platform: KeybindPlatform) -> Vec<ResolvedKeybind> {
    default_keybinds_for_platform(platform)
        .into_iter()
        .map(|binding| ResolvedKeybind {
            trigger: binding.trigger.to_string(),
            action: binding.action,
        })
        .collect()
}

pub fn default_resolved_keybinds() -> Vec<ResolvedKeybind> {
    default_resolved_keybinds_for_platform(KeybindPlatform::current())
}

pub fn parse_keybind_directives(
    lines: &[KeybindLineRef<'_>],
) -> (Vec<KeybindDirective>, Vec<KeybindWarning>) {
    parse_keybind_directives_from_iter(lines.iter().copied())
}

pub fn parse_keybind_directives_from_iter<'a>(
    lines: impl IntoIterator<Item = KeybindLineRef<'a>>,
) -> (Vec<KeybindDirective>, Vec<KeybindWarning>) {
    let mut directives = Vec::new();
    let mut warnings = Vec::new();

    for line in lines {
        let value = line.value.trim();
        if value.is_empty() {
            warnings.push(KeybindWarning {
                line_number: line.line_number,
                message: "empty keybind value".to_string(),
            });
            continue;
        }

        if value.eq_ignore_ascii_case("clear") {
            directives.push(KeybindDirective::Clear);
            continue;
        }

        let Some((trigger_raw, action_raw)) = value.rsplit_once('=') else {
            warnings.push(KeybindWarning {
                line_number: line.line_number,
                message: "expected `keybind = <trigger>=<action>` or `keybind = clear`".to_string(),
            });
            continue;
        };

        let mut trigger = trigger_raw.trim().to_string();
        let action_raw = action_raw.trim();
        if trigger.is_empty() || action_raw.is_empty() {
            warnings.push(KeybindWarning {
                line_number: line.line_number,
                message: "keybind trigger and action must both be non-empty".to_string(),
            });
            continue;
        }

        if should_treat_trailing_dash_as_equal_key(&trigger) {
            trigger.push('=');
        }

        let trigger = match canonicalize_keybind_trigger(&trigger) {
            Ok(trigger) => trigger,
            Err(message) => {
                warnings.push(KeybindWarning {
                    line_number: line.line_number,
                    message,
                });
                continue;
            }
        };

        if action_raw.eq_ignore_ascii_case("unbind") {
            directives.push(KeybindDirective::Unbind { trigger });
            continue;
        }

        let Some(action) = CommandId::from_config_name(action_raw) else {
            warnings.push(KeybindWarning {
                line_number: line.line_number,
                message: format!(
                    "unknown keybind action `{}`; expected one of: {}",
                    action_raw,
                    CommandId::all_config_names().collect::<Vec<_>>().join(", ")
                ),
            });
            continue;
        };

        directives.push(KeybindDirective::Bind { trigger, action });
    }

    (directives, warnings)
}

fn should_treat_trailing_dash_as_equal_key(trigger: &str) -> bool {
    trigger.ends_with('-') && !trigger.ends_with("--")
}

pub fn canonicalize_keybind_trigger(trigger: &str) -> Result<String, String> {
    let mut normalized_parts = Vec::new();
    for component in trigger.split_whitespace() {
        normalized_parts.push(canonicalize_trigger_component(component)?);
    }

    if normalized_parts.is_empty() {
        return Err("empty keybind trigger".to_string());
    }

    Ok(normalized_parts.join(" "))
}

fn canonicalize_trigger_component(component: &str) -> Result<String, String> {
    let normalized = component.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("empty keybind trigger component".to_string());
    }

    let mut rest = normalized.as_str();
    let mut modifiers = Vec::new();

    while let Some((modifier, stripped)) = split_modifier_prefix(rest) {
        modifiers.push(modifier);
        rest = stripped;
    }

    if rest.is_empty() {
        return Err(format!(
            "invalid keybind trigger component `{component}`: missing key"
        ));
    }

    let key = rest;
    let mut canonical_modifiers = Vec::new();
    for modifier in ["ctrl", "alt", "shift", "cmd", "fn", "secondary"] {
        if modifiers.contains(&modifier) && !canonical_modifiers.contains(&modifier) {
            canonical_modifiers.push(modifier);
        }
    }

    if canonical_modifiers.is_empty() {
        Ok(key.to_string())
    } else {
        Ok(format!("{}-{}", canonical_modifiers.join("-"), key))
    }
}

fn split_modifier_prefix(input: &str) -> Option<(&'static str, &str)> {
    for (prefix, canonical) in [
        ("secondary-", "secondary"),
        ("control-", "ctrl"),
        ("ctrl-", "ctrl"),
        ("option-", "alt"),
        ("alt-", "alt"),
        ("shift-", "shift"),
        ("command-", "cmd"),
        ("super-", "cmd"),
        ("meta-", "cmd"),
        ("cmd-", "cmd"),
        ("function-", "fn"),
        ("fn-", "fn"),
    ] {
        if let Some(rest) = input.strip_prefix(prefix) {
            return Some((canonical, rest));
        }
    }

    None
}

pub fn resolve_keybinds(
    mut bindings: Vec<ResolvedKeybind>,
    directives: &[KeybindDirective],
) -> Vec<ResolvedKeybind> {
    for directive in directives {
        match directive {
            KeybindDirective::Clear => bindings.clear(),
            KeybindDirective::Unbind { trigger } => {
                bindings.retain(|binding| binding.trigger != *trigger);
            }
            KeybindDirective::Bind { trigger, action } => {
                bindings.retain(|binding| binding.trigger != *trigger);
                bindings.push(ResolvedKeybind {
                    trigger: trigger.clone(),
                    action: *action,
                });
            }
        }
    }

    bindings
}

#[cfg(test)]
mod tests {
    use super::{
        KeybindDirective, KeybindLineRef, KeybindPlatform, KeybindWarning, ResolvedKeybind,
        canonicalize_keybind_trigger, default_keybinds_for_current_platform,
        default_keybinds_for_platform, parse_keybind_directives, resolve_keybinds,
    };
    use crate::CommandId;

    fn resolved(trigger: &str, action: CommandId) -> ResolvedKeybind {
        ResolvedKeybind {
            trigger: trigger.to_string(),
            action,
        }
    }

    #[test]
    fn default_keybinds_include_open_settings_shortcut() {
        let defaults = default_keybinds_for_current_platform();
        assert!(
            defaults
                .iter()
                .any(|binding| binding.trigger == "secondary-,"
                    && binding.action == CommandId::OpenSettings)
        );
    }

    #[test]
    fn default_keybinds_close_pane_or_tab_on_secondary_w() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "secondary-w" && binding.action == CommandId::ClosePaneOrTab
                }),
                "missing secondary-w -> close_pane_or_tab on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_include_requested_pane_shortcuts() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "secondary-d"
                        && binding.action == CommandId::SplitPaneVertical
                }),
                "missing secondary-d -> split_pane_vertical on {}",
                platform.as_str()
            );
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "secondary-shift-d"
                        && binding.action == CommandId::SplitPaneHorizontal
                }),
                "missing secondary-shift-d -> split_pane_horizontal on {}",
                platform.as_str()
            );
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "secondary-o" && binding.action == CommandId::FocusPaneNext
                }),
                "missing secondary-o -> focus_pane_next on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_cycle_tabs_on_ctrl_tab() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "ctrl-tab" && binding.action == CommandId::CycleTabs
                }),
                "missing ctrl-tab -> cycle_tabs on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_do_not_include_secondary_shift_w_close_pane() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                !defaults.iter().any(|binding| {
                    binding.trigger == "secondary-shift-w" && binding.action == CommandId::ClosePane
                }),
                "unexpected secondary-shift-w -> close_pane on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_do_not_include_toggle_tab_bar_visibility() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                !defaults
                    .iter()
                    .any(|binding| binding.action == CommandId::ToggleTabBarVisibility),
                "unexpected default binding for toggle_tab_bar_visibility on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_toggle_workspace_sidebar_on_secondary_b() {
        for platform in KeybindPlatform::ALL {
            let defaults = default_keybinds_for_platform(platform);
            assert!(
                defaults.iter().any(|binding| {
                    binding.trigger == "secondary-b"
                        && binding.action == CommandId::ToggleWorkspaceSidebar
                }),
                "missing secondary-b -> toggle_workspace_sidebar on {}",
                platform.as_str()
            );
        }
    }

    #[test]
    fn default_keybinds_are_platform_explicit() {
        let mac = default_keybinds_for_platform(KeybindPlatform::MacOs);
        assert!(mac.iter().any(|binding| binding.trigger == "secondary-m"
            && binding.action == CommandId::MinimizeWindow));
        assert!(
            mac.iter().any(
                |binding| binding.trigger == "secondary-c" && binding.action == CommandId::Copy
            )
        );

        let linux = default_keybinds_for_platform(KeybindPlatform::Linux);
        assert!(
            linux
                .iter()
                .any(|binding| binding.trigger == "ctrl-shift-c"
                    && binding.action == CommandId::Copy)
        );
        assert!(!linux.iter().any(|binding| binding.trigger == "secondary-m"
            && binding.action == CommandId::MinimizeWindow));
    }

    #[test]
    fn canonicalize_trigger_normalizes_modifier_aliases_and_order() {
        assert_eq!(
            canonicalize_keybind_trigger("Shift-Control-C"),
            Ok("ctrl-shift-c".to_string())
        );
        assert_eq!(
            canonicalize_keybind_trigger("command-option-p"),
            Ok("alt-cmd-p".to_string())
        );
        assert_eq!(
            canonicalize_keybind_trigger("secondary-p ctrl-shift-g"),
            Ok("secondary-p ctrl-shift-g".to_string())
        );
    }

    #[test]
    fn parses_keybind_directives_and_reports_errors() {
        let lines = [
            KeybindLineRef {
                line_number: 1,
                value: "clear",
            },
            KeybindLineRef {
                line_number: 2,
                value: "Shift-control-P=toggle_command_palette",
            },
            KeybindLineRef {
                line_number: 3,
                value: "secondary-p=unbind",
            },
            KeybindLineRef {
                line_number: 4,
                value: "secondary-p=invalid_action",
            },
        ];

        let (directives, warnings) = parse_keybind_directives(&lines);

        assert_eq!(
            directives,
            vec![
                KeybindDirective::Clear,
                KeybindDirective::Bind {
                    trigger: "ctrl-shift-p".to_string(),
                    action: CommandId::ToggleCommandPalette,
                },
                KeybindDirective::Unbind {
                    trigger: "secondary-p".to_string(),
                }
            ]
        );
        assert_eq!(warnings.len(), 1);
        assert!(matches!(warnings[0], KeybindWarning { line_number: 4, .. }));
    }

    #[test]
    fn parses_clear_screen_keybind() {
        let lines = [KeybindLineRef {
            line_number: 1,
            value: "secondary-k=clear_screen",
        }];

        let (directives, warnings) = parse_keybind_directives(&lines);

        assert!(warnings.is_empty());
        assert!(matches!(
            directives.as_slice(),
            [KeybindDirective::Bind { trigger, action }]
                if trigger == "secondary-k" && action.config_name() == "clear_screen"
        ));
    }

    #[test]
    fn resolve_keybinds_applies_directives_in_order() {
        let defaults = vec![
            resolved("secondary-p", CommandId::ToggleCommandPalette),
            resolved("secondary-c", CommandId::Copy),
        ];
        let directives = vec![
            KeybindDirective::Bind {
                trigger: "secondary-p".to_string(),
                action: CommandId::NewTab,
            },
            KeybindDirective::Unbind {
                trigger: "secondary-c".to_string(),
            },
            KeybindDirective::Bind {
                trigger: "secondary-v".to_string(),
                action: CommandId::Paste,
            },
        ];

        assert_eq!(
            resolve_keybinds(defaults, &directives),
            vec![
                resolved("secondary-p", CommandId::NewTab),
                resolved("secondary-v", CommandId::Paste)
            ]
        );
    }
}
