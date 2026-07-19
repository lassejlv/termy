use super::*;

impl SettingsWindow {
    pub(super) fn apply_editable_field(
        &mut self,
        field: EditableField,
        raw: &str,
    ) -> Result<(), String> {
        let value = raw.trim();
        match field {
            EditableField::Theme
            | EditableField::ThemeMode
            | EditableField::ThemeLight
            | EditableField::ThemeDark
            | EditableField::AppIcon
            | EditableField::BackgroundOpacity
            | EditableField::FontFamily
            | EditableField::UiFontFamily
            | EditableField::FontSize
            | EditableField::LineHeight
            | EditableField::PaddingX
            | EditableField::PaddingY => self.apply_appearance_field(field, value),
            EditableField::WindowsShell
            | EditableField::Shell
            | EditableField::Term
            | EditableField::Colorterm
            | EditableField::TmuxBinary
            | EditableField::TmuxCommandPrefix
            | EditableField::ScrollbackHistory
            | EditableField::InactiveTabScrollback
            | EditableField::ScrollMultiplier
            | EditableField::CursorStyle
            | EditableField::ScrollbarVisibility
            | EditableField::ScrollbarStyle
            | EditableField::PaneFocusEffect
            | EditableField::PaneFocusStrength => self.apply_terminal_field(field, value),
            EditableField::TabFallbackTitle
            | EditableField::TabTitlePriority
            | EditableField::TabTitleMode
            | EditableField::TabTitleExplicitPrefix
            | EditableField::TabTitlePromptFormat
            | EditableField::TabTitleCommandFormat
            | EditableField::TabCloseVisibility
            | EditableField::TabWidthMode
            | EditableField::TabBarPosition
            | EditableField::SidebarWidth => self.apply_tabs_field(field, value),
            EditableField::WorkingDirectory
            | EditableField::WorkingDirFallback
            | EditableField::WindowWidth
            | EditableField::WindowHeight => self.apply_advanced_field(field, value),
            EditableField::Color(id) => self.apply_color_field(id, value),
        }
    }

    pub(super) fn apply_appearance_field(
        &mut self,
        field: EditableField,
        value: &str,
    ) -> Result<(), String> {
        match field {
            EditableField::Theme => {
                if value.is_empty() {
                    return Err("Theme cannot be empty".to_string());
                }
                let message = crate::config::set_theme_in_config(value)?;
                let canonical_theme = message
                    .strip_prefix("Theme set to ")
                    .unwrap_or(value)
                    .to_string();
                self.config.theme = canonical_theme;
                Ok(())
            }
            EditableField::ThemeMode => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "manual" | "off" | "fixed" => termy_config_core::AppearanceMode::Manual,
                    "system" | "auto" | "sync" => termy_config_core::AppearanceMode::System,
                    _ => return Err("Theme mode must be manual or system".to_string()),
                };
                self.config.theme_mode = parsed;
                let canonical = match parsed {
                    termy_config_core::AppearanceMode::Manual => "manual",
                    termy_config_core::AppearanceMode::System => "system",
                };
                config::set_root_setting(termy_config_core::RootSettingId::ThemeMode, canonical)
            }
            EditableField::ThemeLight => {
                if value.is_empty() {
                    return Err("Light theme cannot be empty".to_string());
                }
                config::set_root_setting(termy_config_core::RootSettingId::ThemeLight, value)?;
                self.config.theme_light = value.to_string();
                Ok(())
            }
            EditableField::ThemeDark => {
                if value.is_empty() {
                    return Err("Dark theme cannot be empty".to_string());
                }
                config::set_root_setting(termy_config_core::RootSettingId::ThemeDark, value)?;
                self.config.theme_dark = value.to_string();
                Ok(())
            }
            EditableField::AppIcon => {
                let parsed = termy_config_core::AppIcon::from_str(value)
                    .ok_or_else(|| "App icon must be default or old".to_string())?;
                let canonical = match parsed {
                    termy_config_core::AppIcon::TermyDefault => "default",
                    termy_config_core::AppIcon::TermyOld => "old",
                };
                config::set_root_setting(termy_config_core::RootSettingId::AppIcon, canonical)?;
                self.config.app_icon = parsed;
                crate::app_icon::apply(parsed);
                Ok(())
            }
            EditableField::BackgroundOpacity => {
                let parsed = value
                    .trim_end_matches('%')
                    .parse::<f32>()
                    .map_err(|_| "Background opacity must be a number from 0 to 100".to_string())?;
                let opacity = (parsed / 100.0).clamp(0.0, 1.0);
                self.clear_background_opacity_preview();
                self.persist_background_opacity(opacity)?;
                Ok(())
            }
            EditableField::FontFamily => {
                if value.trim().is_empty() {
                    return Err("Font family cannot be empty".to_string());
                }
                let value = crate::font_families::canonical_available_font_family(
                    value,
                    &self.available_font_families,
                )
                .ok_or_else(|| "Font family is not installed".to_string())?;
                config::set_root_setting(termy_config_core::RootSettingId::FontFamily, &value)?;
                self.config.font_family = value;
                Ok(())
            }
            EditableField::UiFontFamily => {
                if value.trim().is_empty() {
                    return Err("UI font family cannot be empty".to_string());
                }
                let value = crate::font_families::canonical_available_font_family(
                    value,
                    &self.available_font_families,
                )
                .ok_or_else(|| "UI font family is not installed".to_string())?;
                config::set_root_setting(termy_config_core::RootSettingId::UiFontFamily, &value)?;
                self.config.ui_font_family = value;
                Ok(())
            }
            EditableField::FontSize => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Font size must be a positive number".to_string())?;
                if parsed <= 0.0 {
                    return Err("Font size must be greater than 0".to_string());
                }
                self.config.font_size = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::FontSize,
                    &format!("{parsed}"),
                )
            }
            EditableField::LineHeight => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Line height must be a number".to_string())?;
                if !parsed.is_finite() {
                    return Err("Line height must be finite".to_string());
                }
                if !(termy_config_core::MIN_LINE_HEIGHT..=termy_config_core::MAX_LINE_HEIGHT)
                    .contains(&parsed)
                {
                    return Err(format!(
                        "Line height must be between {} and {}",
                        termy_config_core::MIN_LINE_HEIGHT,
                        termy_config_core::MAX_LINE_HEIGHT
                    ));
                }
                config::set_root_setting(
                    termy_config_core::RootSettingId::LineHeight,
                    &format_line_height(parsed),
                )?;
                self.config.line_height = parsed;
                Ok(())
            }
            EditableField::PaddingX => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Horizontal padding must be a number".to_string())?;
                if parsed < 0.0 {
                    return Err("Horizontal padding cannot be negative".to_string());
                }
                self.config.padding_x = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaddingX,
                    &format!("{parsed}"),
                )
            }
            EditableField::PaddingY => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Vertical padding must be a number".to_string())?;
                if parsed < 0.0 {
                    return Err("Vertical padding cannot be negative".to_string());
                }
                self.config.padding_y = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaddingY,
                    &format!("{parsed}"),
                )
            }
            _ => unreachable!("invalid appearance field"),
        }
    }

    pub(super) fn apply_terminal_field(
        &mut self,
        field: EditableField,
        value: &str,
    ) -> Result<(), String> {
        match field {
            EditableField::WindowsShell => {
                let parsed = match value.trim().to_ascii_lowercase().as_str() {
                    "cmd" | "command_prompt" | "commandprompt" => {
                        termy_config_core::WindowsShell::Cmd
                    }
                    "powershell" | "power_shell" | "windows_powershell" | "ps" => {
                        termy_config_core::WindowsShell::PowerShell
                    }
                    "pwsh" | "powershell_core" | "powershellcore" | "powershell_7"
                    | "powershell-7" | "powershell7" | "power_shell_7" | "power_shell_core" => {
                        termy_config_core::WindowsShell::PowerShellCore
                    }
                    "git_bash" | "gitbash" | "git-bash" | "bash" => {
                        termy_config_core::WindowsShell::GitBash
                    }
                    _ => {
                        return Err(
                            "Windows shell must be cmd, powershell, pwsh, or git_bash".to_string()
                        );
                    }
                };
                self.config.windows_shell = parsed;
                let canonical = match parsed {
                    termy_config_core::WindowsShell::Cmd => "cmd",
                    termy_config_core::WindowsShell::PowerShell => "powershell",
                    termy_config_core::WindowsShell::PowerShellCore => "pwsh",
                    termy_config_core::WindowsShell::GitBash => "git_bash",
                };
                config::set_root_setting(termy_config_core::RootSettingId::WindowsShell, canonical)
            }
            EditableField::Shell => {
                if value.is_empty() {
                    self.config.shell = None;
                    config::set_root_setting(termy_config_core::RootSettingId::Shell, "none")
                } else {
                    self.config.shell = Some(value.to_string());
                    config::set_root_setting(termy_config_core::RootSettingId::Shell, value)
                }
            }
            EditableField::Term => {
                if value.is_empty() {
                    return Err("TERM cannot be empty".to_string());
                }
                self.config.term = value.to_string();
                config::set_root_setting(termy_config_core::RootSettingId::Term, value)
            }
            EditableField::Colorterm => {
                if value.is_empty() {
                    self.config.colorterm = None;
                    config::set_root_setting(termy_config_core::RootSettingId::Colorterm, "none")
                } else {
                    self.config.colorterm = Some(value.to_string());
                    config::set_root_setting(termy_config_core::RootSettingId::Colorterm, value)
                }
            }
            EditableField::TmuxBinary => {
                if value.is_empty() {
                    return Err("tmux binary cannot be empty".to_string());
                }
                self.config.tmux_binary = value.to_string();
                config::set_root_setting(termy_config_core::RootSettingId::TmuxBinary, value)
            }
            EditableField::TmuxCommandPrefix => {
                if value.is_empty() {
                    self.config.tmux_command_prefix = None;
                    config::set_root_setting(
                        termy_config_core::RootSettingId::TmuxCommandPrefix,
                        "none",
                    )
                } else {
                    self.config.tmux_command_prefix = Some(value.to_string());
                    config::set_root_setting(
                        termy_config_core::RootSettingId::TmuxCommandPrefix,
                        value,
                    )
                }
            }
            EditableField::ScrollbackHistory => {
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "Scrollback history must be a positive integer".to_string())?
                    .min(100_000);
                self.config.scrollback_history = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::ScrollbackHistory,
                    &parsed.to_string(),
                )
            }
            EditableField::InactiveTabScrollback => {
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| "Inactive tab scrollback must be a positive integer".to_string())?
                    .min(100_000);
                self.config.inactive_tab_scrollback = Some(parsed);
                config::set_root_setting(
                    termy_config_core::RootSettingId::InactiveTabScrollback,
                    &parsed.to_string(),
                )
            }
            EditableField::ScrollMultiplier => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Scroll multiplier must be a number".to_string())?;
                if !parsed.is_finite() {
                    return Err("Scroll multiplier must be finite".to_string());
                }
                let parsed = parsed.clamp(0.1, 1000.0);
                self.config.mouse_scroll_multiplier = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::MouseScrollMultiplier,
                    &parsed.to_string(),
                )
            }
            EditableField::CursorStyle => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "line" | "bar" | "beam" | "ibeam" => termy_config_core::CursorStyle::Line,
                    "block" | "box" => termy_config_core::CursorStyle::Block,
                    _ => return Err("Cursor style must be line or block".to_string()),
                };
                self.config.cursor_style = parsed;
                let canonical = match parsed {
                    termy_config_core::CursorStyle::Line => "line",
                    termy_config_core::CursorStyle::Block => "block",
                };
                config::set_root_setting(termy_config_core::RootSettingId::CursorStyle, canonical)
            }
            EditableField::ScrollbarVisibility => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "off" => termy_config_core::TerminalScrollbarVisibility::Off,
                    "always" => termy_config_core::TerminalScrollbarVisibility::Always,
                    "on_scroll" | "onscroll" => {
                        termy_config_core::TerminalScrollbarVisibility::OnScroll
                    }
                    _ => {
                        return Err(
                            "Scrollbar visibility must be off, always, or on_scroll".to_string()
                        );
                    }
                };
                self.config.terminal_scrollbar_visibility = parsed;
                let canonical = match parsed {
                    termy_config_core::TerminalScrollbarVisibility::Off => "off",
                    termy_config_core::TerminalScrollbarVisibility::Always => "always",
                    termy_config_core::TerminalScrollbarVisibility::OnScroll => "on_scroll",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::ScrollbarVisibility,
                    canonical,
                )
            }
            EditableField::ScrollbarStyle => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "neutral" => termy_config_core::TerminalScrollbarStyle::Neutral,
                    "muted_theme" | "mutedtheme" => {
                        termy_config_core::TerminalScrollbarStyle::MutedTheme
                    }
                    "theme" => termy_config_core::TerminalScrollbarStyle::Theme,
                    _ => {
                        return Err(
                            "Scrollbar style must be neutral, muted_theme, or theme".to_string()
                        );
                    }
                };
                self.config.terminal_scrollbar_style = parsed;
                let canonical = match parsed {
                    termy_config_core::TerminalScrollbarStyle::Neutral => "neutral",
                    termy_config_core::TerminalScrollbarStyle::MutedTheme => "muted_theme",
                    termy_config_core::TerminalScrollbarStyle::Theme => "theme",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::ScrollbarStyle,
                    canonical,
                )
            }
            EditableField::PaneFocusEffect => {
                let parsed =
                    match value.to_ascii_lowercase().as_str() {
                        "off" => termy_config_core::PaneFocusEffect::Off,
                        "soft_spotlight" | "softspotlight" | "soft-spotlight" => {
                            termy_config_core::PaneFocusEffect::SoftSpotlight
                        }
                        "cinematic" => termy_config_core::PaneFocusEffect::Cinematic,
                        "minimal" => termy_config_core::PaneFocusEffect::Minimal,
                        _ => return Err(
                            "Pane focus effect must be off, soft_spotlight, cinematic, or minimal"
                                .to_string(),
                        ),
                    };
                self.config.pane_focus_effect = parsed;
                let canonical = match parsed {
                    termy_config_core::PaneFocusEffect::Off => "off",
                    termy_config_core::PaneFocusEffect::SoftSpotlight => "soft_spotlight",
                    termy_config_core::PaneFocusEffect::Cinematic => "cinematic",
                    termy_config_core::PaneFocusEffect::Minimal => "minimal",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaneFocusEffect,
                    canonical,
                )
            }
            EditableField::PaneFocusStrength => {
                if value.is_empty() {
                    return Err("Pane focus strength cannot be empty".to_string());
                }
                let has_percent_suffix = value.ends_with('%');
                let normalized_input = value.trim_end_matches('%').trim();
                let parsed = normalized_input
                    .parse::<f32>()
                    .map_err(|_| "Pane focus strength must be a finite number".to_string())?;
                if !parsed.is_finite() {
                    return Err("Pane focus strength must be a finite number".to_string());
                }
                let normalized = if has_percent_suffix {
                    parsed / 100.0
                } else {
                    parsed
                }
                .clamp(0.0, 2.0);
                self.config.pane_focus_strength = normalized;
                config::set_root_setting(
                    termy_config_core::RootSettingId::PaneFocusStrength,
                    &format!("{normalized:.3}"),
                )
            }
            _ => unreachable!("invalid terminal field"),
        }
    }

    pub(super) fn apply_tabs_field(
        &mut self,
        field: EditableField,
        value: &str,
    ) -> Result<(), String> {
        match field {
            EditableField::TabFallbackTitle => {
                if value.is_empty() {
                    return Err("Fallback title cannot be empty".to_string());
                }
                self.config.tab_title.fallback = value.to_string();
                config::set_root_setting(termy_config_core::RootSettingId::TabTitleFallback, value)
            }
            EditableField::TabTitlePriority => {
                if value.is_empty() {
                    return Err("Title priority cannot be empty".to_string());
                }
                self.config.tab_title.priority = value
                    .split(',')
                    .filter_map(Self::parse_tab_title_source_token)
                    .fold(Vec::new(), |mut acc, source| {
                        if !acc.contains(&source) {
                            acc.push(source);
                        }
                        acc
                    });
                if self.config.tab_title.priority.is_empty() {
                    return Err("Title priority must contain valid sources".to_string());
                }
                config::set_root_setting(termy_config_core::RootSettingId::TabTitlePriority, value)
            }
            EditableField::TabTitleMode => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "smart" => termy_config_core::TabTitleMode::Smart,
                    "shell" => termy_config_core::TabTitleMode::Shell,
                    "explicit" => termy_config_core::TabTitleMode::Explicit,
                    "static" => termy_config_core::TabTitleMode::Static,
                    _ => {
                        return Err(
                            "Tab title mode must be smart, shell, explicit, or static".to_string()
                        );
                    }
                };
                self.config.tab_title.mode = parsed;
                let canonical = match parsed {
                    termy_config_core::TabTitleMode::Smart => "smart",
                    termy_config_core::TabTitleMode::Shell => "shell",
                    termy_config_core::TabTitleMode::Explicit => "explicit",
                    termy_config_core::TabTitleMode::Static => "static",
                };
                config::set_root_setting(termy_config_core::RootSettingId::TabTitleMode, canonical)
            }
            EditableField::TabTitleExplicitPrefix => {
                if value.is_empty() {
                    return Err("Explicit prefix cannot be empty".to_string());
                }
                self.config.tab_title.explicit_prefix = value.to_string();
                config::set_root_setting(
                    termy_config_core::RootSettingId::TabTitleExplicitPrefix,
                    value,
                )
            }
            EditableField::TabTitlePromptFormat => {
                if value.is_empty() {
                    return Err("Prompt format cannot be empty".to_string());
                }
                self.config.tab_title.prompt_format = value.to_string();
                config::set_root_setting(
                    termy_config_core::RootSettingId::TabTitlePromptFormat,
                    value,
                )
            }
            EditableField::TabTitleCommandFormat => {
                if value.is_empty() {
                    return Err("Command format cannot be empty".to_string());
                }
                self.config.tab_title.command_format = value.to_string();
                config::set_root_setting(
                    termy_config_core::RootSettingId::TabTitleCommandFormat,
                    value,
                )
            }
            EditableField::TabCloseVisibility => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "active_hover" | "activehover" | "active+hover" => {
                        termy_config_core::TabCloseVisibility::ActiveHover
                    }
                    "hover" => termy_config_core::TabCloseVisibility::Hover,
                    "always" => termy_config_core::TabCloseVisibility::Always,
                    _ => {
                        return Err(
                            "Tab close visibility must be active_hover, hover, or always"
                                .to_string(),
                        );
                    }
                };
                self.config.tab_close_visibility = parsed;
                let canonical = match parsed {
                    termy_config_core::TabCloseVisibility::ActiveHover => "active_hover",
                    termy_config_core::TabCloseVisibility::Hover => "hover",
                    termy_config_core::TabCloseVisibility::Always => "always",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::TabCloseVisibility,
                    canonical,
                )
            }
            EditableField::TabWidthMode => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "stable" => termy_config_core::TabWidthMode::Stable,
                    "active_grow" | "activegrow" | "active-grow" => {
                        termy_config_core::TabWidthMode::ActiveGrow
                    }
                    "active_grow_sticky" | "activegrowsticky" | "active-grow-sticky" => {
                        termy_config_core::TabWidthMode::ActiveGrowSticky
                    }
                    "uniform" | "fixed" | "equal" => termy_config_core::TabWidthMode::Uniform,
                    _ => {
                        return Err(
                            "Tab width mode must be uniform, stable, active_grow, or active_grow_sticky"
                                .to_string(),
                        );
                    }
                };
                self.config.tab_width_mode = parsed;
                let canonical = match parsed {
                    termy_config_core::TabWidthMode::Stable => "stable",
                    termy_config_core::TabWidthMode::ActiveGrow => "active_grow",
                    termy_config_core::TabWidthMode::ActiveGrowSticky => "active_grow_sticky",
                    termy_config_core::TabWidthMode::Uniform => "uniform",
                };
                config::set_root_setting(termy_config_core::RootSettingId::TabWidthMode, canonical)
            }
            EditableField::TabBarPosition => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "top" => termy_config_core::TabBarPosition::Top,
                    "right" => termy_config_core::TabBarPosition::Right,
                    _ => {
                        return Err("Tab bar position must be top or right".to_string());
                    }
                };
                self.config.tab_bar_position = parsed;
                let canonical = match parsed {
                    termy_config_core::TabBarPosition::Top => "top",
                    termy_config_core::TabBarPosition::Right => "right",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::TabBarPosition,
                    canonical,
                )
            }
            EditableField::SidebarWidth => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Sidebar width must be a positive number".to_string())?;
                if parsed <= 0.0 {
                    return Err("Sidebar width must be greater than 0".to_string());
                }
                let width = parsed.clamp(
                    termy_config_core::MIN_SIDEBAR_WIDTH,
                    termy_config_core::MAX_SIDEBAR_WIDTH,
                );
                self.config.sidebar_width = width;
                config::set_root_setting(
                    termy_config_core::RootSettingId::SidebarWidth,
                    &width.to_string(),
                )
            }
            _ => unreachable!("invalid tabs field"),
        }
    }

    pub(super) fn apply_advanced_field(
        &mut self,
        field: EditableField,
        value: &str,
    ) -> Result<(), String> {
        match field {
            EditableField::WorkingDirectory => {
                if value.is_empty() {
                    self.config.working_dir = None;
                    config::set_root_setting(termy_config_core::RootSettingId::WorkingDir, "none")
                } else {
                    self.config.working_dir = Some(value.to_string());
                    config::set_root_setting(termy_config_core::RootSettingId::WorkingDir, value)
                }
            }
            EditableField::WorkingDirFallback => {
                let parsed = match value.to_ascii_lowercase().as_str() {
                    "home" | "user" => termy_config_core::WorkingDirFallback::Home,
                    "process" | "cwd" => termy_config_core::WorkingDirFallback::Process,
                    _ => return Err("Working dir fallback must be home or process".to_string()),
                };
                self.config.working_dir_fallback = parsed;
                let canonical = match parsed {
                    termy_config_core::WorkingDirFallback::Home => "home",
                    termy_config_core::WorkingDirFallback::Process => "process",
                };
                config::set_root_setting(
                    termy_config_core::RootSettingId::WorkingDirFallback,
                    canonical,
                )
            }
            EditableField::WindowWidth => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Default width must be a positive number".to_string())?;
                if parsed <= 0.0 {
                    return Err("Default width must be greater than 0".to_string());
                }
                self.config.window_width = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::WindowWidth,
                    &parsed.to_string(),
                )
            }
            EditableField::WindowHeight => {
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| "Default height must be a positive number".to_string())?;
                if parsed <= 0.0 {
                    return Err("Default height must be greater than 0".to_string());
                }
                self.config.window_height = parsed;
                config::set_root_setting(
                    termy_config_core::RootSettingId::WindowHeight,
                    &parsed.to_string(),
                )
            }
            _ => unreachable!("invalid advanced field"),
        }
    }

    pub(super) fn apply_color_field(
        &mut self,
        id: termy_config_core::ColorSettingId,
        value: &str,
    ) -> Result<(), String> {
        if value.is_empty() {
            config::set_color_setting(id, None)?;
            self.set_custom_color_for_id(id, None);
        } else {
            let Some(parsed) = termy_config_core::Rgb8::from_hex(value) else {
                return Err("Color must be #RRGGBB".to_string());
            };
            let canonical = format!("#{:02x}{:02x}{:02x}", parsed.r, parsed.g, parsed.b);
            config::set_color_setting(id, Some(&canonical))?;
            self.set_custom_color_for_id(id, Some(parsed));
        }
        self.colors = TerminalColors::from_config(&self.config, self.system_appearance);
        Ok(())
    }
}
