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
use std::{collections::HashMap, env, path::PathBuf, sync::Arc};

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
    #[default]
    Cmd,
    PowerShell,
    PowerShellCore,
    GitBash,
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
fn windows_cmd_path() -> String {
    if let Ok(comspec) = env::var("COMSPEC")
        && !comspec.trim().is_empty()
    {
        return comspec;
    }
    "C:\\Windows\\System32\\cmd.exe".to_string()
}

#[cfg(target_os = "windows")]
fn windows_git_bash_path() -> String {
    let mut candidates = Vec::new();
    if let Ok(program_files) = env::var("ProgramFiles")
        && !program_files.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files).join("Git\\bin\\bash.exe"));
    }
    if let Ok(program_files_x86) = env::var("ProgramFiles(x86)")
        && !program_files_x86.trim().is_empty()
    {
        candidates.push(PathBuf::from(program_files_x86).join("Git\\bin\\bash.exe"));
    }
    if let Ok(local_app_data) = env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        candidates.push(PathBuf::from(local_app_data).join("Programs\\Git\\bin\\bash.exe"));
    }

    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .map_or_else(|| "bash.exe".to_string(), |path| path.display().to_string())
}

#[cfg(any(not(target_os = "windows"), test))]
fn resolve_shell_path(configured_shell: Option<&str>) -> String {
    if let Some(shell) = configured_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty())
    {
        return shell.to_string();
    }

    if let Ok(shell) = env::var("SHELL")
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

#[cfg(target_os = "windows")]
fn windows_shell_launch(windows_shell: WindowsShell) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: Vec::new(),
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec!["-NoLogo".to_string()],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["--login".to_string(), "-i".to_string()],
        },
    }
}

#[cfg(target_os = "windows")]
fn windows_startup_command_shell(
    windows_shell: WindowsShell,
    command: &str,
) -> ResolvedTerminalLaunch {
    match windows_shell {
        WindowsShell::Cmd => ResolvedTerminalLaunch {
            program: windows_cmd_path(),
            args: vec!["/C".to_string(), command.to_string()],
        },
        WindowsShell::PowerShell => ResolvedTerminalLaunch {
            program: "powershell.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::PowerShellCore => ResolvedTerminalLaunch {
            program: "pwsh.exe".to_string(),
            args: vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        },
        WindowsShell::GitBash => ResolvedTerminalLaunch {
            program: windows_git_bash_path(),
            args: vec!["-lc".to_string(), command.to_string()],
        },
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
    if let Some(launch) = configured_shell_launch(runtime_config.shell.as_deref()) {
        return launch;
    }

    #[cfg(target_os = "windows")]
    {
        windows_shell_launch(runtime_config.windows_shell)
    }

    #[cfg(not(target_os = "windows"))]
    {
        let shell_path = resolve_shell_path(None);
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
            if runtime_config
                .shell
                .as_deref()
                .map(str::trim)
                .is_some_and(|shell| !shell.is_empty())
            {
                return Ok(ResolvedTerminalLaunch {
                    program: "cmd.exe".to_string(),
                    args: vec!["/C".to_string(), command.to_string()],
                });
            }

            return Ok(windows_startup_command_shell(
                runtime_config.windows_shell,
                command,
            ));
        }
    }

    Ok(default_shell_launch(runtime_config))
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
    let mut env_overrides = HashMap::new();

    if let Some(path) = normalized_path_env(
        env::var_os("PATH")
            .or_else(|| env::var_os("Path"))
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

    // Claude Code and similar CLIs gate terminal progress escape sequences on
    // known terminal identities. Termy supports Ghostty's OSC progress
    // protocol, so advertise that compatibility to child processes while
    // keeping TERM conservative for terminfo.
    env_overrides.insert(
        "TERM_PROGRAM".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM.to_string(),
    );
    env_overrides.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        GHOSTTY_COMPAT_TERM_PROGRAM_VERSION.to_string(),
    );
    env_overrides.insert(
        "TERMY_TERM_PROGRAM".to_string(),
        TERMY_TERM_PROGRAM.to_string(),
    );

    for (key, value) in &runtime_config.environment {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        env_overrides.insert(key.to_string(), value.clone());
    }

    // Locale overrides are intentionally Unix-only. POSIX shells use libc locale
    // (`LC_*`/`LANG`) for wcwidth/prompt width, while native Windows shells
    // (`cmd.exe`/PowerShell) do not use this locale contract.
    #[cfg(unix)]
    {
        apply_utf8_locale_overrides(&mut env_overrides);
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
fn apply_utf8_locale_overrides(env_overrides: &mut HashMap<String, String>) {
    let lc_all = env::var("LC_ALL").ok();
    let lc_ctype = env::var("LC_CTYPE").ok();
    let lang = env::var("LANG").ok();
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
    let configured = configured?.trim();
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
    #[allow(dead_code)]
    Title(String),
    /// Terminal title reset
    ResetTitle,
    /// Bell character received
    Bell,
    /// Terminal exited
    Exit,
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

pub use crate::terminal_types::TerminalSize;

impl TerminalSize {
    /// Clamp the cell dimensions into the supported range. Applied at every
    /// entry point that sizes the grid (`Terminal::new`, `new_display`,
    /// `resize`) so a buggy or hostile embedder cannot request a grid large
    /// enough to exhaust memory. The pixel cell metrics are left untouched.
    /// Columns/rows are floored at 1 so downstream grid math never sees a zero
    /// dimension.
    fn clamped(self) -> Self {
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

    pub fn child_pid(&self) -> Option<u32> {
        self.backend.child_pid()
    }

    pub fn set_wakeup_enabled(&self, enabled: bool) {
        self.backend.set_wakeup_enabled(enabled);
    }

    pub fn write(&self, input: &[u8]) {
        self.backend.write(input);
    }

    pub fn write_owned(&self, input: Vec<u8>) {
        self.backend.write_owned(input);
    }

    pub fn hydrate_output(&self, bytes: &[u8]) {
        self.backend.hydrate_output(bytes);
    }

    #[allow(dead_code)]
    pub fn write_str(&self, input: &str) {
        self.backend.write_str(input);
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

    pub(crate) fn render_read_with_screen(&self, force_full: bool) -> (TerminalRenderRead, bool) {
        self.backend.render_read_with_screen(force_full)
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

    #[allow(dead_code)]
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
}

#[cfg(test)]
mod tests;
