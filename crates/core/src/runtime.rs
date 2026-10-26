use crate::frame::{
    TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalViewportMetadata, TermyFrame,
    TermyFrameUpdate,
};
use crate::keyboard::TerminalKeyboardMode;
use crate::kitty_graphics::KittyGraphicsRenderPlacement;
#[cfg(unix)]
use crate::locale::{Utf8LocaleOverridePlan, preferred_utf8_locale, utf8_locale_override_plan};
use crate::mouse_protocol::TerminalMouseMode;
use crate::path_env::normalized_path_env;
use crate::protocol::{TerminalClipboardLocation, TerminalQueryColors, TerminalReplyHost};
use crate::search::{TermySearchMatch, TermySearchOptions, TermySharedSearchMatch};
use crate::shell_integration::ProgressState;
use flume::Sender;
#[cfg(not(target_os = "windows"))]
use std::path::Path;
use std::{collections::HashMap, env, ffi::OsString, path::PathBuf, sync::Arc};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct TabTitleShellIntegration {
    pub enabled: bool,
    pub explicit_prefix: String,
}

const DEFAULT_TERM: &str = "xterm-256color";
const DEFAULT_COLORTERM: &str = "truecolor";
const TERMY_TERM_PROGRAM: &str = "termy";
const GHOSTTY_COMPAT_TERM_PROGRAM: &str = "ghostty";
const GHOSTTY_COMPAT_TERM_PROGRAM_VERSION: &str = "1.2.0";

/// Value advertised to child processes through `TERM_PROGRAM`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TermProgram {
    Termy,
    /// Claude Code and similar CLIs gate terminal progress escape sequences on
    /// known terminal identities. Termy supports Ghostty's OSC progress
    /// protocol, so this advertises that compatibility while keeping TERM
    /// conservative for terminfo.
    #[default]
    Ghostty,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkingDirFallback {
    Home,
    Process,
}

#[allow(clippy::derivable_impls)]
impl Default for WorkingDirFallback {
    fn default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Home
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Process
        }
    }
}

const DEFAULT_SCROLLBACK_HISTORY: usize = 1000;

/// Upper clamp on scrollback lines, enforced at the point the value is applied
/// to the live grid. The config-file parser (`config_core`) already bounds this
/// at parse time, but the runtime/FFI setters (`with_scrollback_history`,
/// `set_scrollback_history`) and directly-constructed `TerminalRuntimeConfig`s
/// bypass that parser, so the core must self-defend: each pane eagerly grows its
/// scrollback toward this cap, so an unbounded value plus hostile output is an
/// unbounded memory leak. Kept in parity with `config_core`'s constant of the
/// same name.
pub const MAX_TERMINAL_SCROLLBACK_HISTORY: usize = 20_000;

/// Upper clamp on terminal dimensions. Real displays never approach this (an 8K
/// display at a 4px font is ~1900 columns); it exists only to stop a buggy or
/// hostile embedder from requesting a multi-gigabyte grid — `u16::MAX` on both
/// axes is ~4.3 billion cells. The axis limits and engine's total-cell limit
/// bound both the grid and snapshots allocated from it.
const MAX_TERMINAL_COLS: u16 = 4096;
const MAX_TERMINAL_ROWS: u16 = 4096;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowsShell {
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
    /// PowerShell 7 (`pwsh.exe`) when it is installed, else Windows
    /// PowerShell, else the `COMSPEC` command processor. Declared last so
    /// serialized indices of the explicit presets stay stable.
    #[default]
    Auto,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCursorStyle {
    Line,
    Block,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub col: usize,
    pub row: usize,
    pub style: TerminalCursorStyle,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalOptions {
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

impl Default for TerminalOptions {
    fn default() -> Self {
        Self {
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct TerminalRuntimeConfig {
    pub shell: Option<String>,
    pub windows_shell: WindowsShell,
    pub term: String,
    pub colorterm: Option<String>,
    #[serde(default)]
    pub term_program: TermProgram,
    pub environment: HashMap<String, String>,
    pub query_colors: TerminalQueryColors,
    pub working_dir_fallback: WorkingDirFallback,
    pub scrollback_history: usize,
    pub default_cursor_style: TerminalCursorStyle,
}

/// Selects what owns a newly-created PTY.
///
/// `ShellCommand` preserves the existing shell-evaluated startup-command API.
/// Structured tools such as OpenSSH must use `Program`, which sends each
/// argument directly to the child without routing through a shell.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum TerminalLaunch {
    ShellCommand(String),
    Program { program: String, args: Vec<String> },
}

impl Default for TerminalRuntimeConfig {
    fn default() -> Self {
        Self {
            shell: None,
            windows_shell: WindowsShell::default(),
            term: DEFAULT_TERM.to_string(),
            colorterm: Some(DEFAULT_COLORTERM.to_string()),
            term_program: TermProgram::default(),
            environment: HashMap::new(),
            query_colors: TerminalQueryColors::default(),
            working_dir_fallback: WorkingDirFallback::default(),
            scrollback_history: DEFAULT_SCROLLBACK_HISTORY,
            default_cursor_style: TerminalCursorStyle::Block,
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn resolved_shell_program(&self) -> String {
        default_shell_launch(self).program
    }
}

impl TerminalOptions {
    pub fn with_scrollback_history(self, scrollback_history: usize) -> Self {
        Self {
            scrollback_history,
            ..self
        }
    }
}

impl TerminalRuntimeConfig {
    pub fn term_options(&self) -> TerminalOptions {
        TerminalOptions {
            scrollback_history: self.scrollback_history,
            default_cursor_style: self.default_cursor_style,
        }
    }
}

fn login_shell_args(shell_path: &str) -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let _ = shell_path;
        Vec::new()
    }

    // On macOS, terminals conventionally launch login shells so that the user's
    // PATH and environment (set up in ~/.bash_profile, ~/.zprofile, etc.) are
    // available.  Pass both -i (interactive) and -l (login).
    #[cfg(target_os = "macos")]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string(), "-l".to_string()],
        _ => Vec::new(),
    }

    // On Linux (and other non-macOS Unix), the user is already in a login
    // session, so sourcing all login scripts on every terminal open adds
    // unnecessary startup latency.  Launch an interactive non-login shell
    // instead, matching conventional Linux terminal behavior.
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    match Path::new(shell_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        Some("bash" | "zsh" | "fish") => vec!["-i".to_string()],
        _ => Vec::new(),
    }
}

/// The executable and argument vector selected for a terminal PTY.
///
/// All terminal engines must use [`resolve_terminal_launch`] instead of
/// independently interpreting [`TerminalRuntimeConfig`] or [`TerminalLaunch`].
/// This keeps platform shell selection, login-shell arguments, and startup
/// command handling identical across engines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTerminalLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[cfg(target_os = "windows")]
mod windows_shell;
#[cfg(all(target_os = "windows", test))]
use windows_shell::{
    WindowsShellFamily, inherited_prompt_is_custom, windows_cmd_path, windows_git_bash_path,
};
#[cfg(target_os = "windows")]
use windows_shell::{
    apply_windows_shell_integration, warn_cmd_unc_working_directory, windows_shell_family,
    windows_shell_launch, windows_startup_command_launch, windows_startup_command_shell,
};

/// Where a terminal child's inherited variables come from. Launch decisions
/// that read the environment (the default shell, PATH, the locale) read the
/// same source, so a child never mixes two environments.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ChildEnvironment<'a> {
    /// This process's own environment.
    Process,
    /// Given variables instead, for example a multiplexer client's.
    Explicit(&'a [(String, String)]),
}

impl ChildEnvironment<'_> {
    pub(crate) fn var_os(self, name: &str) -> Option<OsString> {
        match self {
            Self::Process => env::var_os(name),
            // The last entry wins, as when the variables are applied.
            Self::Explicit(variables) => variables
                .iter()
                .rev()
                .find(|(key, _)| environment_names_match(key, name))
                .map(|(_, value)| OsString::from(value)),
        }
    }

    #[cfg(unix)]
    fn var(self, name: &str) -> Option<String> {
        self.var_os(name)?.into_string().ok()
    }
}

/// Windows environment names are case-insensitive.
fn environment_names_match(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

/// The whole environment of a child that starts from `base` instead of this
/// process's environment: `base` without the variables of the terminal that
/// launched it, then `overrides`. Entries no child environment can hold
/// (Windows' per-drive `=C:` entries) are left out.
pub(crate) fn explicit_child_environment(
    base: &[(String, String)],
    overrides: HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = Vec::with_capacity(base.len() + overrides.len());
    // Walk backwards so a repeated name keeps its last value, as
    // `ChildEnvironment::var_os` reads it.
    for (name, value) in base.iter().rev() {
        let usable = !name.is_empty()
            && !name.contains(['=', '\0'])
            && !value.contains('\0')
            && !crate::terminal_engine::transport::belongs_to_parent_terminal(name.as_ref());
        let replaced = overrides
            .keys()
            .chain(environment.iter().map(|(known, _)| known))
            .any(|known| environment_names_match(known, name));
        if usable && !replaced {
            environment.push((name.clone(), value.clone()));
        }
    }
    environment.extend(overrides);
    // A stable order keeps the child environment block reproducible.
    environment.sort_unstable();
    environment
}

#[cfg(test)]
fn resolve_shell_path(configured_shell: Option<&str>) -> String {
    resolve_shell_path_in(configured_shell, ChildEnvironment::Process)
}

#[cfg(any(not(target_os = "windows"), test))]
fn resolve_shell_path_in(configured_shell: Option<&str>, source: ChildEnvironment) -> String {
    if let Some(shell) = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
    {
        return shell.to_string();
    }

    if let Some(shell) = source
        .var_os("SHELL")
        .and_then(|shell| shell.into_string().ok())
        && !shell.trim().is_empty()
    {
        return shell;
    }

    #[cfg(target_os = "windows")]
    {
        windows_cmd_path()
    }

    #[cfg(target_os = "macos")]
    {
        "/bin/zsh".to_string()
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        "/bin/bash".to_string()
    }
}

fn configured_shell_launch(configured_shell: Option<&str>) -> Option<ResolvedTerminalLaunch> {
    let shell_path = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())?;
    Some(ResolvedTerminalLaunch {
        program: shell_path.to_string(),
        args: login_shell_args(shell_path),
    })
}

fn default_shell_launch(runtime_config: &TerminalRuntimeConfig) -> ResolvedTerminalLaunch {
    default_shell_launch_in(runtime_config, ChildEnvironment::Process)
}

fn default_shell_launch_in(
    runtime_config: &TerminalRuntimeConfig,
    source: ChildEnvironment,
) -> ResolvedTerminalLaunch {
    if let Some(launch) = configured_shell_launch(runtime_config.shell.as_deref()) {
        return launch;
    }

    #[cfg(target_os = "windows")]
    {
        windows_shell_launch(runtime_config.windows_shell, source)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell_path = resolve_shell_path_in(None, source);
        ResolvedTerminalLaunch {
            program: shell_path.clone(),
            args: login_shell_args(&shell_path),
        }
    }
}

pub fn resolve_terminal_launch(
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
) -> anyhow::Result<ResolvedTerminalLaunch> {
    resolve_terminal_launch_in(runtime_config, launch, ChildEnvironment::Process)
}

/// [`resolve_terminal_launch`] for a child whose environment is `source`.
pub(crate) fn resolve_terminal_launch_in(
    runtime_config: &TerminalRuntimeConfig,
    launch: Option<&TerminalLaunch>,
    source: ChildEnvironment,
) -> anyhow::Result<ResolvedTerminalLaunch> {
    if let Some(TerminalLaunch::Program { program, args }) = launch {
        anyhow::ensure!(
            !program.trim().is_empty(),
            "terminal program cannot be empty"
        );
        anyhow::ensure!(
            !program.contains('\0') && !args.iter().any(|arg| arg.contains('\0')),
            "terminal program and arguments cannot contain NUL bytes"
        );
        return Ok(ResolvedTerminalLaunch {
            program: program.clone(),
            args: args.clone(),
        });
    }

    if let Some(command) = launch.and_then(|launch| match launch {
        TerminalLaunch::ShellCommand(command) => {
            Some(command.trim()).filter(|command| !command.is_empty())
        }
        TerminalLaunch::Program { .. } => None,
    }) {
        #[cfg(unix)]
        {
            return Ok(ResolvedTerminalLaunch {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), command.to_string()],
            });
        }

        #[cfg(target_os = "windows")]
        {
            // A configured shell runs the command itself when it takes a
            // known command flag; other shells fall back to COMSPEC.
            if let Some(shell) = runtime_config
                .shell
                .as_deref()
                .map(str::trim)
                .filter(|shell| !shell.is_empty())
            {
                return Ok(windows_startup_command_launch(
                    windows_shell_family(shell),
                    shell.to_string(),
                    command,
                    source,
                ));
            }

            return Ok(windows_startup_command_shell(
                runtime_config.windows_shell,
                command,
                source,
            ));
        }
    }

    Ok(default_shell_launch_in(runtime_config, source))
}

fn user_home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(user_profile) = env::var("USERPROFILE")
            && !user_profile.trim().is_empty()
        {
            return Some(PathBuf::from(user_profile));
        }

        if let (Ok(home_drive), Ok(home_path)) = (env::var("HOMEDRIVE"), env::var("HOMEPATH"))
            && !home_drive.trim().is_empty()
            && !home_path.trim().is_empty()
        {
            return Some(PathBuf::from(format!("{home_drive}{home_path}")));
        }
    }

    if let Ok(home) = env::var("HOME")
        && !home.trim().is_empty()
    {
        return Some(PathBuf::from(home));
    }

    None
}

/// Build the child-process environment shared by native terminal engines.
///
/// Keeping this at the engine boundary prevents experimental backends from
/// drifting on terminal identity, PATH normalization, shell integration, or
/// Unix UTF-8 locale repair.
pub fn terminal_environment_overrides(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
) -> HashMap<String, String> {
    terminal_environment_overrides_in(shell_integration, runtime_config, ChildEnvironment::Process)
}

/// [`terminal_environment_overrides`] for a child whose environment is `source`.
pub(crate) fn terminal_environment_overrides_in(
    shell_integration: Option<&TabTitleShellIntegration>,
    runtime_config: &TerminalRuntimeConfig,
    source: ChildEnvironment,
) -> HashMap<String, String> {
    let mut env_overrides = HashMap::new();

    if let Some(path) = normalized_path_env(
        source
            .var_os("PATH")
            .or_else(|| source.var_os("Path"))
            .as_deref(),
    ) {
        env_overrides.insert("PATH".to_string(), path);
    }

    let term = runtime_config.term.trim();
    let term = if term.is_empty() { DEFAULT_TERM } else { term };
    env_overrides.insert("TERM".to_string(), term.to_string());

    if let Some(colorterm) = runtime_config
        .colorterm
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        env_overrides.insert("COLORTERM".to_string(), colorterm.to_string());
    }

    match runtime_config.term_program {
        TermProgram::Ghostty => {
            env_overrides.insert(
                "TERM_PROGRAM".to_string(),
                GHOSTTY_COMPAT_TERM_PROGRAM.to_string(),
            );
            env_overrides.insert(
                "TERM_PROGRAM_VERSION".to_string(),
                GHOSTTY_COMPAT_TERM_PROGRAM_VERSION.to_string(),
            );
        }
        // No TERM_PROGRAM_VERSION: the engine only knows its own crate
        // version, not the app's, and an inherited one is scrubbed.
        TermProgram::Termy => {
            env_overrides.insert("TERM_PROGRAM".to_string(), TERMY_TERM_PROGRAM.to_string());
        }
    }
    env_overrides.insert(
        "TERMY_TERM_PROGRAM".to_string(),
        TERMY_TERM_PROGRAM.to_string(),
    );

    // Sorted so that names colliding case-insensitively on Windows (`PATH`
    // and `Path`) resolve the same way on every launch.
    let mut user_environment = runtime_config.environment.iter().collect::<Vec<_>>();
    user_environment.sort_unstable_by(|left, right| left.0.cmp(right.0));
    for (key, value) in user_environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        // Windows names are case-insensitive: a user `Path` replaces the
        // normalized `PATH` instead of racing it into the child block.
        #[cfg(target_os = "windows")]
        env_overrides.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        env_overrides.insert(key.to_string(), value.clone());
    }

    // Locale overrides are intentionally Unix-only. POSIX shells use libc locale
    // (`LC_*`/`LANG`) for wcwidth/prompt width, while native Windows shells
    // (`cmd.exe`/PowerShell) do not use this locale contract.
    #[cfg(unix)]
    {
        apply_utf8_locale_overrides(&mut env_overrides, source);
    }

    let shell_integration_enabled = shell_integration.is_some_and(|cfg| cfg.enabled);
    env_overrides.insert(
        "TERMY_SHELL_INTEGRATION".to_string(),
        if shell_integration_enabled { "1" } else { "0" }.to_string(),
    );

    if shell_integration_enabled {
        let prefix = shell_integration
            .and_then(|cfg| {
                let trimmed = cfg.explicit_prefix.trim();
                (!trimmed.is_empty()).then_some(trimmed)
            })
            .unwrap_or("termy:tab:");
        env_overrides.insert("TERMY_TAB_TITLE_PREFIX".to_string(), prefix.to_string());
    }

    env_overrides
}

#[cfg(unix)]
fn apply_utf8_locale_overrides(
    env_overrides: &mut HashMap<String, String>,
    source: ChildEnvironment,
) {
    let lc_all = source.var("LC_ALL");
    let lc_ctype = source.var("LC_CTYPE");
    let lang = source.var("LANG");
    let target_utf8_locale =
        preferred_utf8_locale(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref());

    // zsh prompt width calculations rely on libc wcwidth + locale. If the shell
    // starts in C/POSIX/non-UTF-8 locale, multibyte prompt glyphs (e.g. U+276F)
    // can be counted by byte-length, drifting completion rendering.
    match utf8_locale_override_plan(lc_all.as_deref(), lc_ctype.as_deref(), lang.as_deref()) {
        Utf8LocaleOverridePlan::None => {}
        Utf8LocaleOverridePlan::LcCtypeOnly => {
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
        Utf8LocaleOverridePlan::LcAllAndLcCtype => {
            env_overrides.insert("LC_ALL".to_string(), target_utf8_locale.clone());
            env_overrides.insert("LC_CTYPE".to_string(), target_utf8_locale);
        }
    }
}

pub fn resolve_working_directory_path(configured: Option<&str>) -> Option<std::path::PathBuf> {
    let configured = configured?;
    // An absolute directory is taken as it is, so a name with surrounding
    // spaces that a client already resolved survives (`termy mux create`).
    let path = Path::new(configured);
    if path.is_absolute() && path.is_dir() {
        return Some(path.to_owned());
    }
    let configured = configured.trim();
    if configured.is_empty() {
        return None;
    }

    let path = if configured == "~" {
        user_home_dir()?
    } else if let Some(relative) = configured
        .strip_prefix("~/")
        .or_else(|| configured.strip_prefix("~\\"))
    {
        user_home_dir()?.join(relative)
    } else {
        PathBuf::from(configured)
    };

    if path.is_dir() { Some(path) } else { None }
}

pub fn resolve_launch_working_directory(
    configured: Option<&str>,
    fallback: WorkingDirFallback,
) -> Option<PathBuf> {
    resolve_working_directory_path(configured)
        .or_else(|| default_working_directory_with_fallback(fallback))
}

pub fn normalize_working_directory_candidate(candidate: Option<&str>) -> Option<String> {
    let candidate = candidate?.trim();
    if candidate.is_empty() || candidate.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }

    Some(resolve_working_directory_path(Some(candidate)).map_or_else(
        || candidate.to_string(),
        |path| path.to_string_lossy().into_owned(),
    ))
}

fn default_working_directory_with_fallback(fallback: WorkingDirFallback) -> Option<PathBuf> {
    if fallback == WorkingDirFallback::Home
        && let Some(home) = user_home_dir()
        && home.is_dir()
    {
        return Some(home);
    }

    env::current_dir().ok()
}

/// Events sent from the terminal to the view
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum TerminalEvent {
    /// Terminal content has changed, needs redraw
    Wakeup,
    /// Terminal title changed
    Title(String),
    /// Terminal title reset
    ResetTitle,
    /// Bell character received
    Bell,
    /// Terminal exited
    Exit,
    /// Keys or a paste were dropped: the program isn't reading its input and
    /// the write backlog is full. Coalesced to one event per drain.
    InputDropped,
    /// OSC 52 clipboard store request
    ClipboardStore(String),

    // Shell integration events (OSC 133)
    /// OSC 133;A - Shell prompt start
    ShellPromptStart,
    /// OSC 133;B - Command input start
    ShellCommandStart,
    /// OSC 133;C - Command executing
    ShellCommandExecuting,
    /// OSC 133;D - Command finished with optional exit code
    ShellCommandFinished(Option<i32>),

    // Progress indicator (OSC 9;4)
    /// Progress state change from OSC 9;4
    Progress(ProgressState),

    // Working directory (OSC 7)
    /// Working directory changed
    WorkingDirectory(String),

    /// Coalesced OSC 7501 snapshot, with inherited apps resolved.
    ProgramStatus(Vec<crate::ProgramStatusRecord>),
}

/// Host-provided callback used to schedule terminal event draining.
///
/// The callback is intentionally payload-free: hosts that multiplex several
/// terminals can capture their own stable terminal identifier, while the FFI
/// host can keep using its existing one-terminal wake channel.
#[derive(Clone)]
pub struct TerminalWakeupNotifier {
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl TerminalWakeupNotifier {
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Arc::new(notify),
        }
    }

    pub fn notify(&self) {
        (self.notify)();
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalDirtySpan {
    pub row: usize,
    pub left_col: usize,
    pub right_col: usize,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum TerminalDamageSnapshot {
    Full,
    Partial(Vec<TerminalDirtySpan>),
}

/// Visits viewport cells as `(row, display_offset, line, col, cell)`.
pub(crate) type DamageCellVisitor<'v> =
    dyn FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell) + 'v;

/// Viewport cells read under the lock that took their damage; see
/// [`Terminal::with_render_damage`].
pub struct TerminalDamageCells<'a> {
    display_offset: usize,
    line_bounds: (i32, i32),
    visit: &'a mut dyn FnMut(&[TerminalDirtySpan], &mut DamageCellVisitor<'_>) -> bool,
}

impl<'a> TerminalDamageCells<'a> {
    pub(crate) fn new(
        display_offset: usize,
        line_bounds: (i32, i32),
        visit: &'a mut dyn FnMut(&[TerminalDirtySpan], &mut DamageCellVisitor<'_>) -> bool,
    ) -> Self {
        Self {
            display_offset,
            line_bounds,
            visit,
        }
    }

    pub fn display_offset(&self) -> usize {
        self.display_offset
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.line_bounds
    }

    /// Visit the cells covered by `spans`, span by span. Returns false,
    /// without visiting, when a span lies outside the viewport.
    pub fn visit(
        &mut self,
        spans: &[TerminalDirtySpan],
        mut visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        (self.visit)(spans, &mut visitor)
    }
}

pub use crate::terminal_types::TerminalSize;

impl TerminalSize {
    /// Clamp the cell dimensions into the supported range. Applied at every
    /// entry point that sizes the grid (`Terminal::new`, `new_display`,
    /// `resize`) so a buggy or hostile embedder cannot request a grid large
    /// enough to exhaust memory. The pixel cell metrics are left untouched.
    /// Columns/rows are floored at 1 so downstream grid math never sees a zero
    /// dimension.
    pub(crate) fn clamped(self) -> Self {
        let cols = self.cols.clamp(1, MAX_TERMINAL_COLS);
        let rows = usize::from(self.rows.clamp(1, MAX_TERMINAL_ROWS))
            .min(crate::terminal_engine::Size::MAX_CELLS / usize::from(cols))
            as u16;
        Self { cols, rows, ..self }
    }
}

/// The terminal state wrapper
pub struct Terminal {
    backend: engine_backend::Backend,
}

mod custom_backend;
mod engine_backend;

impl Terminal {
    /// Attach a renderer to a terminal owned by the built-in session host.
    pub fn from_remote(transport: Arc<dyn crate::remote::RemoteTransport>) -> Self {
        Self {
            backend: engine_backend::Backend::Remote(Box::new(crate::remote::RemoteBackend::new(
                transport,
            ))),
        }
    }

    /// The active engine name for diagnostics. Do not branch application
    /// behavior on this value; construction policy remains owned by core.
    pub fn engine_label(&self) -> &'static str {
        self.backend.engine_label()
    }

    /// Create a new terminal with the given size.
    pub fn new(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new(
            size,
            configured_working_dir,
            event_wakeup_tx,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(|backend| Self { backend })
    }

    /// Create a terminal whose wakeups are routed through a host callback.
    pub fn new_with_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            startup_command,
        )
        .map(|backend| Self { backend })
    }

    /// Create a terminal whose child is selected with a typed launch contract.
    pub fn new_with_launch_and_wakeup_notifier(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
            None,
            ChildEnvironment::Process,
        )
        .map(|backend| Self { backend })
    }

    /// Like [`Self::new_with_launch_and_wakeup_notifier`], but the child starts
    /// from `environment` instead of this process's environment. Termy's own
    /// terminal variables and the configured ones still apply on top, and the
    /// default shell, PATH and locale are read from `environment`.
    pub fn new_with_launch_in_environment(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
        environment: &[(String, String)],
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
            None,
            ChildEnvironment::Explicit(environment),
        )
        .map(|backend| Self { backend })
    }

    /// Like [`Self::new_with_launch_and_wakeup_notifier`], replaying a saved
    /// buffer before the child starts. Unlike [`Self::hydrate_output`] after
    /// spawning, the restored text can't interleave with the new session's
    /// first output or swallow its replies.
    pub fn new_with_launch_restoring_output(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
        restored_output: &[u8],
    ) -> anyhow::Result<Self> {
        engine_backend::Backend::new_with_launch_and_wakeup_notifier(
            size,
            configured_working_dir,
            wakeup_notifier,
            tab_title_shell_integration,
            runtime_config,
            launch,
            Some(restored_output),
            ChildEnvironment::Process,
        )
        .map(|backend| Self { backend })
    }

    /// Create a display-only terminal with no PTY or child process.
    pub fn new_display(size: TerminalSize, runtime_config: Option<&TerminalRuntimeConfig>) -> Self {
        Self {
            backend: engine_backend::Backend::new_display(size, runtime_config),
        }
    }

    /// Create a display-only terminal whose committed output wakes the host.
    pub fn new_display_with_wakeup_notifier(
        size: TerminalSize,
        runtime_config: Option<&TerminalRuntimeConfig>,
        wakeup_notifier: Option<TerminalWakeupNotifier>,
    ) -> Self {
        Self {
            backend: engine_backend::Backend::new_display_with_wakeup_notifier(
                size,
                runtime_config,
                wakeup_notifier,
            ),
        }
    }

    pub fn feed_output(&self, bytes: &[u8]) {
        self.backend.feed_output(bytes);
    }

    /// For display terminals: feed output and return the protocol replies
    /// queued so far, in order, instead of waiting for `drain_events`.
    pub fn feed_output_with_replies(&self, bytes: &[u8]) -> Vec<u8> {
        self.backend.feed_output_with_replies(bytes)
    }

    /// Disable protocol replies for a display terminal that mirrors another
    /// terminal's output (which already answered): queries are parsed and
    /// change no state, and replies are dropped.
    pub fn set_protocol_replies_enabled(&self, enabled: bool) {
        self.backend.set_protocol_replies_enabled(enabled);
    }

    /// The engine's full state; see `terminal_engine::Engine::encode_snapshot`.
    /// `None` for multiplexer-backed terminals.
    pub fn encode_snapshot(&self) -> Option<Vec<u8>> {
        self.backend.encode_snapshot()
    }

    /// Replace the engine state with a snapshot, keeping this terminal's
    /// configured colors, cursor style and cell size. `None` for
    /// multiplexer-backed terminals.
    pub fn restore_snapshot(
        &self,
        bytes: &[u8],
    ) -> Option<Result<(), crate::terminal_engine::SnapshotError>> {
        self.backend.restore_snapshot(bytes)
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.backend.child_pid()
    }

    /// Ends the child and waits for it, where dropping the terminal would
    /// leave a program that ignores SIGHUP to a timer: for a process about to
    /// exit. Remote and display terminals have no child here.
    pub(crate) fn terminate_and_wait(&self) {
        self.backend.terminate_and_wait();
    }

    /// Suspends render wakeups for a hidden terminal. Title changes and OSC 7501
    /// program status still wake the host so tab chrome can follow them.
    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.backend.set_wakeup_enabled(enabled);
    }

    pub fn write(&self, input: &[u8]) {
        self.backend.write(input);
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        self.backend.write_owned(input);
    }

    /// [`Self::write_owned`], returning whether the input was queued for the
    /// program: `WouldBlock` when the input backlog is full, `BrokenPipe`
    /// once the PTY is closed. Input that does not fit is reported to views
    /// as [`TerminalEvent::InputDropped`] either way. A session-host client
    /// only queues the input for the host and reports success.
    pub(crate) fn try_write_owned(&self, input: Vec<u8>) -> std::io::Result<()> {
        self.backend.try_write_owned(input)
    }

    /// The cursor's line, numbered from the first line output ever scrolled
    /// into history; see [`crate::remote::RemoteReply::Written`].
    pub(crate) fn input_anchor(&self) -> u64 {
        self.backend.input_anchor()
    }

    /// Write a mouse-motion report. Motion may fill only half of the input
    /// backlog, so a program that stops reading cannot starve later keys;
    /// excess reports are dropped and the next one carries the newer position.
    pub fn write_mouse_motion(&self, input: Vec<u8>) {
        self.backend.write_mouse_motion(input);
    }

    pub fn hydrate_output(&self, bytes: &[u8]) {
        self.backend.hydrate_output(bytes);
    }

    pub fn resize(&mut self, new_size: TerminalSize) {
        self.backend.resize(new_size);
    }

    pub fn nudge_resize(&self) {
        self.backend.nudge_resize();
    }

    pub fn size(&self) -> TerminalSize {
        self.backend.size()
    }

    /// The size the latest `resize` asked for. Local terminals apply it
    /// immediately; a session-host terminal reports its previous size from
    /// `size` until the host publishes a frame at the new size. Compare
    /// against this when deciding whether a resize is still needed.
    pub fn requested_size(&self) -> TerminalSize {
        self.backend.requested_size()
    }

    /// `true` when another client resized this session-host pane after a
    /// frame showed the size this terminal last asked for, or the size it was
    /// created or attached at. `requested_size` then follows the host again.
    /// Always `false` for local terminals.
    pub fn host_size_overridden(&self) -> bool {
        self.backend.host_size_overridden()
    }

    /// `true` once the session host confirmed that it stopped sending frames
    /// to this terminal after `set_wakeup_enabled(false)`; it sends events
    /// only until render wakeups are enabled again. Always `false` for local
    /// terminals.
    pub fn frames_paused(&self) -> bool {
        self.backend.frames_paused()
    }

    pub fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.backend.kitty_graphics_placements()
    }

    pub fn kitty_graphics_revision(&self) -> u64 {
        self.backend.kitty_graphics_revision()
    }

    pub fn kitty_graphics_snapshot(&self) -> (u64, Vec<KittyGraphicsRenderPlacement>) {
        self.backend.kitty_graphics_snapshot()
    }

    pub fn kitty_clipboard_paste_events_enabled(&self) -> bool {
        self.backend.kitty_clipboard_paste_events_enabled()
    }

    /// Build a paste notification for a local terminal with an external transport,
    /// retaining its single-use permission grant for the corresponding read.
    /// Remote terminals use `send_kitty_clipboard_paste_event` through their host.
    pub fn kitty_clipboard_paste_notification(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> Option<Vec<u8>> {
        self.backend
            .kitty_clipboard_paste_notification(location, available_formats)
    }

    pub fn send_kitty_clipboard_paste_event(
        &self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> bool {
        self.backend
            .send_kitty_clipboard_paste_event(location, available_formats)
    }

    pub fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        self.backend.drain_events(host)
    }

    /// Answers a clipboard request the reply host deferred (see
    /// [`TerminalReplyHost::defer_clipboard_request`]) and sends the program
    /// every reply now due, in request order. Unknown ids are ignored.
    pub fn complete_clipboard_request(&self, id: u64, reply: crate::TerminalClipboardReply) {
        self.backend.complete_clipboard_request(id, Some(reply));
    }

    /// Denies a deferred clipboard request, for a host that will not answer
    /// it: the program gets the same reply as for a refused request.
    pub fn deny_clipboard_request(&self, id: u64) {
        self.backend.complete_clipboard_request(id, None);
    }

    pub fn set_query_colors(&mut self, query_colors: TerminalQueryColors) {
        self.backend.set_query_colors(query_colors);
    }

    pub fn palette(&self) -> crate::TerminalPalette {
        self.backend.palette()
    }

    pub fn snapshot(&self) -> TermyFrame {
        self.backend.snapshot()
    }

    pub fn frame_update(&self, force_full: bool) -> TermyFrameUpdate {
        self.backend.frame_update(force_full)
    }

    pub fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        self.backend.take_render_damage_snapshot()
    }

    /// Take render damage and read its dirty cells under one backend lock.
    ///
    /// Taking damage and then calling
    /// [`Self::visit_viewport_ranges_locked_at_generation`] fails whenever
    /// output lands in between, which forces a full read. Here output waits
    /// until `read` returns. `read` runs under the backend lock and must not
    /// call back into this terminal.
    pub fn with_render_damage<R>(
        &self,
        read: impl FnOnce(TerminalRenderDamageSnapshot, TerminalDamageCells<'_>) -> R,
    ) -> R {
        self.backend.with_render_damage(read)
    }

    pub fn render_read(&self, force_full: bool) -> TerminalRenderRead {
        self.backend.render_read(force_full)
    }

    /// Visit a coherent viewport snapshot, allowing reentrant terminal reads.
    pub fn visit_viewport_cells(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.backend.visit_viewport_cells(visitor)
    }

    /// Visit a snapshot of viewport ranges only if the requested generation is current.
    /// The callback may call back into this terminal.
    pub fn visit_viewport_ranges_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        self.backend
            .visit_viewport_ranges_at_generation(generation, spans, visitor)
    }

    /// Visit viewport cells without collecting an intermediate snapshot.
    ///
    /// The callback runs under the local backend lock and must not call back
    /// into this terminal. Use [`Self::visit_viewport_cells`] for reentrant callbacks.
    pub fn visit_viewport_cells_locked(
        &self,
        visitor: impl FnMut(usize, i32, usize, &crate::TerminalRenderCell),
    ) -> TerminalViewportMetadata {
        self.backend.visit_viewport_cells_locked(visitor)
    }

    /// Visit current-generation viewport ranges without an intermediate snapshot.
    ///
    /// The callback runs under the local backend lock and must not call back into
    /// this terminal. Use [`Self::visit_viewport_ranges_at_generation`] for reentrant callbacks.
    pub fn visit_viewport_ranges_locked_at_generation(
        &self,
        generation: u64,
        spans: &[TerminalDirtySpan],
        visitor: impl FnMut(usize, usize, i32, usize, &crate::TerminalRenderCell),
    ) -> bool {
        self.backend
            .visit_viewport_ranges_locked_at_generation(generation, spans, visitor)
    }

    pub fn line_bounds(&self) -> (i32, i32) {
        self.backend.line_bounds()
    }

    /// Visit a requested inclusive buffer-line range from one coherent backend state.
    ///
    /// The callback runs under the backend lock and must not call back into this terminal.
    pub fn visit_line_cells(
        &self,
        requested_first: i32,
        requested_last: i32,
        visitor: impl FnMut((i32, i32, usize), i32, usize, &crate::TerminalRenderCell),
    ) -> (i32, i32, usize) {
        self.backend
            .visit_line_cells(requested_first, requested_last, visitor)
    }

    pub fn search(&self, query: &str) -> Vec<TermySearchMatch> {
        self.backend.search(query)
    }

    pub fn search_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySearchMatch> {
        self.backend.search_with_options(query, options)
    }

    pub fn search_shared(&self, query: &str) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared(query)
    }

    pub fn search_shared_with_options(
        &self,
        query: &str,
        options: TermySearchOptions,
    ) -> Vec<TermySharedSearchMatch> {
        self.backend.search_shared_with_options(query, options)
    }

    pub fn hyperlink_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedLink> {
        self.backend.hyperlink_at(row, col)
    }

    pub fn link_at(&self, row: usize, col: usize) -> Option<crate::links::DetectedViewportLink> {
        self.backend.link_at(row, col)
    }

    /// [`Self::hyperlink_at`] for a row of the frame `view` describes, which
    /// output may have moved since (audit L19).
    pub(crate) fn hyperlink_at_view(
        &self,
        row: usize,
        col: usize,
        view: crate::remote::ViewportAnchor,
    ) -> Option<crate::links::DetectedLink> {
        self.backend.hyperlink_at_view(row, col, view)
    }

    /// [`Self::link_at`] for a row of the frame `view` describes.
    pub(crate) fn link_at_view(
        &self,
        row: usize,
        col: usize,
        view: crate::remote::ViewportAnchor,
    ) -> Option<crate::links::DetectedViewportLink> {
        self.backend.link_at_view(row, col, view)
    }

    /// Buffer lines as text; see [`crate::remote::RemoteCommand::LineTexts`].
    pub(crate) fn line_texts(
        &self,
        first: i32,
        last: i32,
        anchor: Option<u64>,
        max_bytes: usize,
    ) -> crate::remote::LineTexts {
        self.backend.line_texts(first, last, anchor, max_bytes)
    }

    pub fn take_damage_snapshot(&self) -> TerminalDamageSnapshot {
        self.backend.take_damage_snapshot()
    }

    pub fn scroll_display(&self, delta_lines: i32) -> bool {
        self.backend.scroll_display(delta_lines)
    }

    pub fn scroll_to_bottom(&self) -> bool {
        self.backend.scroll_to_bottom()
    }

    pub fn clear_scrollback(&self) -> bool {
        self.backend.clear_scrollback()
    }

    pub fn scroll_state(&self) -> (usize, usize) {
        self.backend.scroll_state()
    }

    pub fn cursor_state(&self) -> Option<TerminalCursorState> {
        self.backend.cursor_state()
    }

    pub fn cursor_position(&self) -> (usize, usize) {
        self.backend.cursor_position()
    }

    pub fn has_pending_events(&self) -> bool {
        self.backend.has_pending_events()
    }

    pub fn set_term_options(&self, options: TerminalOptions) {
        self.backend.set_term_options(options);
    }

    pub fn set_scrollback_history(&self, scrollback_history: usize) {
        self.backend.set_scrollback_history(scrollback_history);
    }

    pub fn bracketed_paste_mode(&self) -> bool {
        self.backend.bracketed_paste_mode()
    }

    pub fn mouse_mode(&self) -> TerminalMouseMode {
        self.backend.mouse_mode()
    }

    pub fn keyboard_mode(&self) -> TerminalKeyboardMode {
        self.backend.keyboard_mode()
    }

    pub fn alternate_screen_mode(&self) -> bool {
        self.backend.alternate_screen_mode()
    }

    /// The state a session host publishes, read in one consistent pass.
    pub(crate) fn remote_state(&self) -> crate::remote::RemoteState {
        self.backend.remote_state()
    }
}

#[cfg(test)]
mod tests;
