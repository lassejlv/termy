use std::{
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use crate::config_core::{
    AppConfig, ConfigDiagnostic, CursorStyle, SHELL_DECIDE_THEME_ID, SystemAppearance,
    WindowsShell, WorkingDirFallback, resolve_active_theme,
};
use crate::protocol::TerminalQueryColors;
use crate::runtime::{
    TerminalCursorStyle, TerminalRuntimeConfig, WindowsShell as RuntimeWindowsShell,
    WorkingDirFallback as RuntimeWorkingDirFallback,
};
use crate::themes::{ThemeColors, normalize_theme_id, parse_theme_colors_json};
use crate::{TerminalColor, TermyColor};

#[derive(Debug, Clone)]
pub struct LoadedTermyConfig {
    pub path: Option<PathBuf>,
    pub app_config: AppConfig,
    pub runtime_config: TerminalRuntimeConfig,
    pub diagnostics: Vec<ConfigDiagnostic>,
    pub loaded_from_disk: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedThemeColors {
    pub active_theme: String,
    pub ansi: [TermyColor; 16],
    pub foreground: TermyColor,
    pub background: TermyColor,
    pub cursor: TermyColor,
}

#[derive(Debug)]
pub enum TermyConfigError {
    Read { path: PathBuf, source: io::Error },
}

impl fmt::Display for TermyConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
        }
    }
}

impl Error for TermyConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
        }
    }
}

pub fn load_config_from_default_path() -> Result<LoadedTermyConfig, TermyConfigError> {
    let Some(path) = crate::config_core::config_path() else {
        return Ok(default_loaded_config(None));
    };

    match load_config_from_path(&path) {
        Ok(config) => Ok(config),
        Err(TermyConfigError::Read { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(default_loaded_config(Some(path)))
        }
        Err(error) => Err(error),
    }
}

pub fn load_config_from_path(
    path: impl AsRef<Path>,
) -> Result<LoadedTermyConfig, TermyConfigError> {
    let path = path.as_ref().to_path_buf();
    let contents = fs::read_to_string(&path).map_err(|source| TermyConfigError::Read {
        path: path.clone(),
        source,
    })?;
    Ok(load_config_from_contents_with_path(
        &contents,
        Some(path),
        true,
    ))
}

pub fn load_config_from_contents(contents: &str) -> LoadedTermyConfig {
    load_config_from_contents_with_path(contents, None, false)
}

pub fn runtime_config_from_app_config(config: &AppConfig) -> TerminalRuntimeConfig {
    runtime_config_from_app_config_with_theme(config, None, SystemAppearance::Dark)
}

pub fn runtime_config_from_app_config_with_theme(
    config: &AppConfig,
    config_path: Option<&Path>,
    system_appearance: SystemAppearance,
) -> TerminalRuntimeConfig {
    runtime_config_from_app_config_with_query_colors(
        config,
        terminal_query_colors_from_resolved_theme(&resolve_theme_colors_from_app_config(
            config,
            config_path,
            system_appearance,
        )),
    )
}

pub fn runtime_config_from_app_config_with_query_colors(
    config: &AppConfig,
    query_colors: TerminalQueryColors,
) -> TerminalRuntimeConfig {
    let working_dir_fallback = match config.working_dir_fallback {
        WorkingDirFallback::Home => RuntimeWorkingDirFallback::Home,
        WorkingDirFallback::Process => RuntimeWorkingDirFallback::Process,
    };

    TerminalRuntimeConfig {
        shell: config.shell.clone(),
        windows_shell: match config.windows_shell {
            WindowsShell::Cmd => RuntimeWindowsShell::Cmd,
            WindowsShell::PowerShell => RuntimeWindowsShell::PowerShell,
            WindowsShell::PowerShellCore => RuntimeWindowsShell::PowerShellCore,
            WindowsShell::GitBash => RuntimeWindowsShell::GitBash,
        },
        term: config.term.clone(),
        colorterm: config.colorterm.clone(),
        environment: Default::default(),
        query_colors,
        working_dir_fallback,
        scrollback_history: config.scrollback_history,
        default_cursor_style: match config.cursor_style {
            CursorStyle::Line => TerminalCursorStyle::Line,
            CursorStyle::Block => TerminalCursorStyle::Block,
        },
    }
}

fn load_config_from_contents_with_path(
    contents: &str,
    path: Option<PathBuf>,
    loaded_from_disk: bool,
) -> LoadedTermyConfig {
    let report = AppConfig::from_contents_with_report(contents);
    let runtime_config = runtime_config_from_app_config_with_theme(
        &report.config,
        path.as_deref(),
        SystemAppearance::Dark,
    );
    LoadedTermyConfig {
        path,
        runtime_config,
        app_config: report.config,
        diagnostics: report.diagnostics,
        loaded_from_disk,
    }
}

fn default_loaded_config(path: Option<PathBuf>) -> LoadedTermyConfig {
    let app_config = AppConfig::default();
    let runtime_config = runtime_config_from_app_config_with_theme(
        &app_config,
        path.as_deref(),
        SystemAppearance::Dark,
    );
    LoadedTermyConfig {
        path,
        runtime_config,
        app_config,
        diagnostics: Vec::new(),
        loaded_from_disk: false,
    }
}

pub fn resolve_theme_colors_from_app_config(
    config: &AppConfig,
    config_path: Option<&Path>,
    system_appearance: SystemAppearance,
) -> ResolvedThemeColors {
    let active_theme = resolve_active_theme(config, system_appearance).to_string();
    let mut colors = if active_theme.eq_ignore_ascii_case(SHELL_DECIDE_THEME_ID) {
        terminal_default_theme_colors()
    } else {
        load_installed_theme_colors(&active_theme, config_path)
            .or_else(|| builtin_theme_colors(&active_theme))
            .unwrap_or_else(|| match (config.theme_mode, system_appearance) {
                (crate::config_core::AppearanceMode::System, SystemAppearance::Light) => {
                    crate::themes::termy_light()
                }
                _ => crate::themes::termy(),
            })
    };
    apply_custom_colors(&mut colors, &config.colors);

    ResolvedThemeColors {
        active_theme,
        ansi: colors.ansi.map(term_color_from_rgb),
        foreground: term_color_from_rgb(colors.foreground),
        background: term_color_from_rgb(colors.background),
        cursor: term_color_from_rgb(colors.cursor),
    }
}

pub fn terminal_query_colors_from_resolved_theme(
    colors: &ResolvedThemeColors,
) -> TerminalQueryColors {
    TerminalQueryColors {
        ansi: colors.ansi.map(terminal_color_from_term_color),
        foreground: terminal_color_from_term_color(colors.foreground),
        background: terminal_color_from_term_color(colors.background),
        cursor: None,
    }
}

fn load_installed_theme_colors(theme_id: &str, config_path: Option<&Path>) -> Option<ThemeColors> {
    let normalized = normalize_theme_id(theme_id);
    if normalized.is_empty() {
        return None;
    }

    let owned_config_path;
    let config_path = if let Some(path) = config_path {
        path
    } else {
        owned_config_path = crate::config_core::config_path()?;
        owned_config_path.as_path()
    };
    let theme_path = config_path
        .parent()?
        .join("themes")
        .join(format!("{normalized}.json"));
    let contents = std::fs::read_to_string(theme_path).ok()?;
    parse_theme_colors_json(&contents).ok()
}

fn builtin_theme_colors(theme_id: &str) -> Option<ThemeColors> {
    match normalize_theme_id(theme_id).as_str() {
        "termy" => Some(crate::themes::termy()),
        "termy-light" | "termylight" => Some(crate::themes::termy_light()),
        "tokyo-night" | "tokyonight" => Some(crate::themes::tokyo_night()),
        "catppuccin-mocha" | "catppuccin" | "catppuccinmocha" => {
            Some(crate::themes::catppuccin_mocha())
        }
        "dracula" => Some(crate::themes::dracula()),
        "gruvbox-dark" | "gruvbox" | "gruvboxdark" => Some(crate::themes::gruvbox_dark()),
        "nord" => Some(crate::themes::nord()),
        "solarized-dark" | "solarized" | "solarizeddark" => Some(crate::themes::solarized_dark()),
        "one-dark" | "one" | "onedark" => Some(crate::themes::one_dark()),
        "monokai" => Some(crate::themes::monokai()),
        "material-dark" | "material" | "materialdark" => Some(crate::themes::material_dark()),
        "palenight" => Some(crate::themes::palenight()),
        "tomorrow-night" | "tomorrow" | "tomorrownight" => Some(crate::themes::tomorrow_night()),
        "oceanic-next" | "oceanic" | "oceanicnext" => Some(crate::themes::oceanic_next()),
        _ => crate::themes::resolve_theme(theme_id),
    }
}

fn apply_custom_colors(colors: &mut ThemeColors, custom: &crate::config_core::CustomColors) {
    if let Some(color) = custom.foreground {
        colors.foreground = theme_rgb_from_config_rgb(color);
    }
    if let Some(color) = custom.background {
        colors.background = theme_rgb_from_config_rgb(color);
    }
    if let Some(color) = custom.cursor {
        colors.cursor = theme_rgb_from_config_rgb(color);
    }
    for (index, color) in custom.ansi.iter().enumerate() {
        if let Some(color) = color {
            colors.ansi[index] = theme_rgb_from_config_rgb(*color);
        }
    }
}

fn terminal_default_theme_colors() -> ThemeColors {
    let defaults = TerminalQueryColors::default();
    ThemeColors {
        ansi: defaults.ansi.map(theme_rgb_from_terminal_color),
        foreground: theme_rgb_from_terminal_color(defaults.foreground),
        background: theme_rgb_from_terminal_color(defaults.background),
        cursor: theme_rgb_from_terminal_color(defaults.foreground),
    }
}

fn theme_rgb_from_terminal_color(color: TerminalColor) -> crate::themes::Rgb8 {
    crate::themes::Rgb8::new(color.r, color.g, color.b)
}

fn theme_rgb_from_config_rgb(color: crate::config_core::Rgb8) -> crate::themes::Rgb8 {
    crate::themes::Rgb8::new(color.r, color.g, color.b)
}

fn term_color_from_rgb(color: crate::themes::Rgb8) -> TermyColor {
    TermyColor {
        r: color.r,
        g: color.g,
        b: color.b,
        a: 255,
    }
}

fn terminal_color_from_term_color(color: TermyColor) -> TerminalColor {
    TerminalColor {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_runtime_fields_from_contents() {
        let loaded = load_config_from_contents(
            "theme = nord\nshell = /bin/zsh\nterm = screen-256color\ncolorterm = none\nscrollback = 42\ncursor_style = line\n",
        );

        assert_eq!(loaded.runtime_config.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(loaded.runtime_config.term, "screen-256color");
        assert_eq!(loaded.runtime_config.colorterm, None);
        assert_eq!(loaded.runtime_config.scrollback_history, 42);
        assert_eq!(loaded.runtime_config.query_colors.background.r, 0x2e);
        assert_eq!(loaded.runtime_config.query_colors.background.g, 0x34);
        assert_eq!(loaded.runtime_config.query_colors.background.b, 0x40);
        assert_eq!(
            loaded.runtime_config.default_cursor_style,
            TerminalCursorStyle::Line
        );
        assert!(!loaded.loaded_from_disk);
    }

    #[test]
    fn resolves_custom_theme_colors_from_contents() {
        let loaded = load_config_from_contents(
            "theme = nord\n[colors]\nforeground = #ffffff\nbackground = #010203\ngreen = #040506\n",
        );
        let colors =
            resolve_theme_colors_from_app_config(&loaded.app_config, None, SystemAppearance::Dark);

        assert_eq!(colors.active_theme, "nord");
        assert_eq!(
            colors.foreground,
            TermyColor {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }
        );
        assert_eq!(
            colors.background,
            TermyColor {
                r: 1,
                g: 2,
                b: 3,
                a: 255,
            }
        );
        assert_eq!(
            colors.ansi[2],
            TermyColor {
                r: 4,
                g: 5,
                b: 6,
                a: 255,
            }
        );
    }

    #[test]
    fn missing_system_light_theme_uses_light_builtin_fallback() {
        let tempdir = tempdir().expect("tempdir");
        let config_path = tempdir.path().join("config.txt");
        let config = AppConfig {
            theme_mode: crate::config_core::AppearanceMode::System,
            theme_light: "missing-light-theme".to_string(),
            ..AppConfig::default()
        };

        let colors = resolve_theme_colors_from_app_config(
            &config,
            Some(&config_path),
            SystemAppearance::Light,
        );

        assert_eq!(
            colors.background,
            TermyColor {
                r: 0xFA,
                g: 0xFA,
                b: 0xF9,
                a: 0xFF,
            }
        );
    }

    #[test]
    fn loads_explicit_path_with_diagnostics() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("config.txt");
        fs::write(&path, "unknown_key = true\n").expect("write config");

        let loaded = load_config_from_path(&path).expect("load config");

        assert_eq!(loaded.path.as_deref(), Some(path.as_path()));
        assert!(loaded.loaded_from_disk);
        assert_eq!(loaded.diagnostics.len(), 1);
    }

    #[test]
    fn missing_explicit_path_is_an_error() {
        let tempdir = tempdir().expect("tempdir");
        let path = tempdir.path().join("missing.txt");

        let error = load_config_from_path(&path).expect_err("missing path should fail");

        match error {
            TermyConfigError::Read {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
        }
    }
}
