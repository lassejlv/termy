use anyhow::Context;
use clap::{Args as ClapArgs, Subcommand, builder::NonEmptyStringValueParser, value_parser};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};
use termy_core::multiplexer::{
    InputCommand, PaneInfo, PaneLaunch, SessionClient, connect_or_start, is_exited_pane,
    is_host_unavailable, is_missing_pane, is_version_mismatch,
};
use termy_core::remote::{InputRejection, LineText, LineTexts};
use termy_core::session_model::StoredMultiplexer;
use termy_core::{
    TabTitleShellIntegration, TerminalKeyboardMode, TerminalRuntimeConfig, TerminalSize,
    TermyKeystroke, TermyModifiers,
};

/// The version of the JSON every `termy mux` command prints. 2 changed
/// `capture`, `wait`, `list` and the error shape; see the CLI docs.
const SCHEMA_VERSION: u32 = 2;
/// The most input one `send` or `key` writes.
const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// How often `wait` reads the pane again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(ClapArgs)]
pub struct Args {
    /// Multiplexer directory; defaults to $TERMY_MUX_DIR inside a multiplexer
    /// pane, otherwise to the desktop config's multiplexer directory
    #[arg(long, global = true)]
    session_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the background host without creating a terminal
    Start,
    /// Report whether a host runs, without starting one
    Status,
    /// List stable pane IDs, child PIDs, titles, directories, and exit state
    List {
        /// Also report each pane's size and shell-integration command state
        #[arg(long)]
        verbose: bool,
    },
    /// Create a persistent terminal (starts the host if necessary)
    Create {
        #[arg(long, default_value_t = 80, value_parser = value_parser!(u16).range(1..=400))]
        cols: u16,
        #[arg(long, default_value_t = 24, value_parser = value_parser!(u16).range(1..=200))]
        rows: u16,
        /// Directory to start in, relative to this command's directory, which
        /// is the default
        #[arg(long)]
        working_directory: Option<String>,
        /// Program to run instead of the configured shell
        #[arg(long)]
        shell: Option<String>,
        /// Saved window ID; defaults to the first window's active workspace
        #[arg(long)]
        window: Option<String>,
        /// Zero-based workspace index in the selected window; defaults to its
        /// active workspace
        #[arg(long, requires = "window")]
        workspace: Option<usize>,
    },
    /// Send input bytes literally, without shell escaping or interpretation
    Send {
        id: String,
        #[arg(
            required_unless_present = "stdin",
            conflicts_with = "stdin",
            allow_hyphen_values = true
        )]
        text: Option<String>,
        /// Read the input from standard input: any bytes, at most 1 MiB
        #[arg(long)]
        stdin: bool,
        /// Append Enter after the supplied text
        #[arg(long)]
        enter: bool,
        /// Then wait until the output from the line the input was typed on
        /// contains this text, as `wait --after` with the returned anchor
        #[arg(long, allow_hyphen_values = true, value_parser = NonEmptyStringValueParser::new())]
        wait: Option<String>,
        /// How long --wait waits
        #[arg(long, default_value_t = 5000, requires = "wait", value_parser = value_parser!(u64).range(1..=3_600_000))]
        timeout_ms: u64,
    },
    /// Send keys, encoded the same on every platform
    Key {
        id: String,
        /// Key names or characters, each optionally with modifiers
        /// (`ctrl+c`, `alt+left`, `shift+tab`), sent in order
        #[arg(required = true)]
        keys: Vec<String>,
        /// Hold Control for every key
        #[arg(long)]
        control: bool,
        /// Hold Alt for every key
        #[arg(long)]
        alt: bool,
        /// Hold Shift for every key
        #[arg(long)]
        shift: bool,
    },
    /// Capture screen or history text, dimensions, and cursor without taking
    /// terminal ownership
    Capture {
        id: String,
        /// First line: 0 is the top screen row, negative lines are history
        /// (-1 is the newest); defaults to 0
        #[arg(long, allow_negative_numbers = true)]
        start: Option<i32>,
        /// Last line, numbered like --start; defaults to the bottom screen row
        #[arg(long, allow_negative_numbers = true)]
        end: Option<i32>,
    },
    /// Wait until output contains text, or until a command finishes
    Wait {
        id: String,
        #[arg(
            required_unless_present = "command_done",
            conflicts_with = "command_done",
            allow_hyphen_values = true,
            value_parser = NonEmptyStringValueParser::new()
        )]
        text: Option<String>,
        /// Wait until the command started by the latest input finishes and
        /// report its exit code; needs shell integration
        #[arg(long)]
        command_done: bool,
        /// First line to search, numbered from the screen as the wait
        /// starts: 0 is its top row, negative lines are history
        #[arg(long, default_value_t = 0, allow_negative_numbers = true)]
        start: i32,
        /// Search only output from the line where `send` or `key` typed the
        /// input that returned this anchor
        #[arg(long, conflicts_with_all = ["start", "command_done"])]
        after: Option<u64>,
        #[arg(long, default_value_t = 5000, value_parser = value_parser!(u64).range(1..=3_600_000))]
        timeout_ms: u64,
    },
    /// Resize a persistent terminal that is alone in its tab
    Resize {
        id: String,
        #[arg(value_parser = value_parser!(u16).range(1..=400))]
        cols: u16,
        #[arg(value_parser = value_parser!(u16).range(1..=200))]
        rows: u16,
    },
    /// Split a saved pane; horizontal creates left/right panes, vertical top/bottom
    Split {
        id: String,
        #[arg(long, default_value = "horizontal", value_parser = ["horizontal", "vertical"])]
        axis: String,
    },
    /// Close one terminal and terminate its process
    Close { id: String },
    /// Read the live window/workspace/tab layout
    Layout,
    /// Create, select, reorder, or delete empty workspaces in a saved window
    Window {
        id: String,
        #[command(subcommand)]
        edit: WindowCommand,
    },
    /// Edit the saved tab containing a stable pane ID
    Tab {
        id: String,
        #[command(subcommand)]
        edit: TabCommand,
    },
    /// Rename or pin a saved workspace using its window ID and zero-based index
    Workspace {
        window: String,
        index: usize,
        #[command(subcommand)]
        edit: WorkspaceCommand,
    },
    /// Stop the host and terminate every terminal it owns
    Shutdown {
        /// Stop the host's process without asking it, for a host that no
        /// longer answers (SIGTERM, then SIGKILL; on Windows, terminate it)
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    Rename { name: String },
    Pin,
    Unpin,
    SelectTab { index: usize },
    MoveTab { from: usize, to: usize },
}

#[derive(Subcommand)]
enum WindowCommand {
    #[command(name = "create-workspace")]
    Create { name: String },
    #[command(name = "select-workspace")]
    Select { index: usize },
    #[command(name = "move-workspace")]
    Move { from: usize, to: usize },
    #[command(name = "delete-empty-workspace")]
    DeleteEmpty { index: usize },
}

#[derive(Subcommand)]
enum TabCommand {
    Rename {
        title: String,
    },
    ResetTitle,
    Pin,
    Unpin,
    Zoom,
    Unzoom,
    Focus {
        pane_id: String,
    },
    /// Move a divider by signed cells (positive moves right/down)
    ResizeDivider {
        #[arg(value_parser=["left","right","top","bottom"])]
        edge: String,
        #[arg(allow_hyphen_values = true)]
        delta: i16,
    },
}

/// What a failure reports as `error.code` (audit L13). Scripts tell
/// failures apart by it; the exit status is 2 for invalid arguments and 1
/// for everything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Code {
    /// The command line is invalid: a range, an empty or unknown value, or
    /// a working directory that is not a directory.
    InvalidArgument,
    /// No pane, window or workspace has that ID or index (any more).
    NotFound,
    /// The pane's program has exited.
    Exited,
    /// The pane's input backlog is full: its program stopped reading.
    InputDropped,
    /// `wait` gave up.
    Timeout,
    /// `wait` gave up, and output it should have searched left the
    /// scrollback before it could be read.
    OutputLost,
    /// The pane's state does not allow the command.
    Unsupported,
    /// No host runs, or it cannot be reached.
    HostUnavailable,
    /// A host of another Termy version runs; `termy mux shutdown` stops it.
    VersionMismatch,
    Failed,
}

impl Code {
    fn name(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Exited => "exited",
            Self::InputDropped => "input_dropped",
            Self::Timeout => "timeout",
            Self::OutputLost => "output_lost",
            Self::Unsupported => "unsupported",
            Self::HostUnavailable => "host_unavailable",
            Self::VersionMismatch => "version_mismatch",
            Self::Failed => "failed",
        }
    }

    fn exit_status(self) -> i32 {
        if self == Self::InvalidArgument { 2 } else { 1 }
    }

    /// The code of a failure: the one it was raised with, else what its
    /// cause tells.
    fn of(error: &anyhow::Error) -> Self {
        for cause in error.chain() {
            if let Some(failure) = cause.downcast_ref::<Failure>() {
                return failure.code;
            }
            if let Some(rejection) = cause.downcast_ref::<InputRejection>() {
                return match rejection {
                    InputRejection::Exited => Self::Exited,
                    InputRejection::BacklogFull => Self::InputDropped,
                    InputRejection::Failed(_) => Self::Failed,
                };
            }
        }
        if is_version_mismatch(error) {
            Self::VersionMismatch
        } else if is_host_unavailable(error) {
            Self::HostUnavailable
        } else if is_exited_pane(error) {
            Self::Exited
        } else if is_missing_pane(error) {
            Self::NotFound
        } else {
            Self::Failed
        }
    }
}

#[derive(Debug)]
struct Failure {
    code: Code,
    message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

fn fail(code: Code, message: impl Into<String>) -> anyhow::Error {
    Failure {
        code,
        message: message.into(),
    }
    .into()
}

pub fn run(args: Args) {
    match execute(args) {
        Ok(result) => println!(
            "{}",
            json!({"schema_version":SCHEMA_VERSION,"ok":true,"result":result})
        ),
        Err(error) => exit_with(Code::of(&error), &format!("{error:#}")),
    }
}

fn exit_with(code: Code, message: &str) -> ! {
    eprintln!(
        "{}",
        json!({"schema_version":SCHEMA_VERSION,"ok":false,
            "error":{"code":code.name(),"message":message}})
    );
    std::process::exit(code.exit_status());
}

/// Reports a `termy mux` command line that does not parse like any other
/// failure: a JSON error with code `invalid_argument`, exit status 2, also
/// for a missing subcommand. Help and version requests print as usual.
pub fn parse_error(error: clap::Error) -> ! {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
    ) {
        error.exit();
    }
    let rendered = error.render().to_string();
    let message = rendered.split("\n\n").next().unwrap_or_default().trim();
    exit_with(
        Code::InvalidArgument,
        message.strip_prefix("error: ").unwrap_or(message),
    );
}

/// Steps that failed after a layout change was committed (audit L7): the
/// command succeeded, so scripts must not retry it.
fn with_warnings(mut result: Value, warnings: Vec<String>) -> Value {
    if !warnings.is_empty() {
        result["warnings"] = json!(warnings);
    }
    result
}

/// The configuration a new terminal starts with, as the desktop applies it:
/// shell, terminal identity, scrollback, cursor style and shell integration
/// (audit B26). Invalid config lines are warnings, and a config that cannot
/// be read gives the defaults and a warning. The CLI cannot see the system
/// appearance, so a theme that follows it resolves as dark.
fn user_settings() -> (TerminalRuntimeConfig, TabTitleShellIntegration, Vec<String>) {
    match termy_core::load_config_from_default_path() {
        Ok(loaded) => {
            let path = loaded
                .path
                .as_deref()
                .map_or_else(|| "config".into(), |path| path.display().to_string());
            let warnings = loaded
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    format!(
                        "{path} line {}: {}",
                        diagnostic.line_number, diagnostic.message
                    )
                })
                .collect();
            (
                loaded.runtime_config,
                TabTitleShellIntegration {
                    enabled: loaded.app_config.tab_title.shell_integration,
                    explicit_prefix: loaded.app_config.tab_title.explicit_prefix,
                },
                warnings,
            )
        }
        Err(error) => {
            let defaults = termy_core::AppConfig::default();
            (
                termy_core::runtime_config_from_app_config(&defaults),
                TabTitleShellIntegration {
                    enabled: defaults.tab_title.shell_integration,
                    explicit_prefix: defaults.tab_title.explicit_prefix,
                },
                vec![format!("Using the default settings: {error}")],
            )
        }
    }
}

/// `requested`, resolved against this command's directory (`~` against its
/// home), or this command's directory itself, made absolute as a desktop tab
/// would (symbolic links stay). Fails unless it is a directory, rather than
/// letting the pane start elsewhere (audit B27). The host uses an absolute
/// directory as given.
fn working_directory(requested: Option<&str>) -> anyhow::Result<String> {
    let current = std::env::current_dir().context("cannot read the current directory")?;
    let path = match requested {
        None => current,
        Some(home) if home == "~" || home.starts_with("~/") || home.starts_with("~\\") => {
            termy_core::resolve_working_directory_path(Some(home)).ok_or_else(|| {
                fail(
                    Code::InvalidArgument,
                    format!("working directory {home} is not a directory"),
                )
            })?
        }
        Some(path) => current.join(path),
    };
    let path = std::path::absolute(&path).unwrap_or(path);
    if !path.is_dir() {
        return Err(fail(
            Code::InvalidArgument,
            format!("working directory {} is not a directory", path.display()),
        ));
    }
    Ok(path.to_string_lossy().into_owned())
}

fn layout(client: &SessionClient) -> anyhow::Result<StoredMultiplexer> {
    StoredMultiplexer::parse(client.layout()?.as_deref()).map_err(anyhow::Error::msg)
}

/// The live pane `id`. A pane closed after its program exited, as a
/// desktop window closes it, fails with `exited` for a while.
fn pane(client: &SessionClient, id: &str) -> anyhow::Result<PaneInfo> {
    if let Some(pane) = client.list()?.into_iter().find(|pane| pane.id == id) {
        return Ok(pane);
    }
    match client.size(id) {
        Err(error) if is_exited_pane(&error) => Err(fail(
            Code::Exited,
            format!("pane {id} exited and was closed"),
        )),
        _ => Err(fail(Code::NotFound, format!("no pane has ID {id}"))),
    }
}

fn find_tab<'a>(
    layout: &'a StoredMultiplexer,
    id: &str,
) -> anyhow::Result<&'a termy_core::session_model::StoredTab> {
    layout
        .windows
        .iter()
        .flat_map(|window| &window.session.workspaces)
        .flat_map(|workspace| &workspace.tabs)
        .find(|tab| {
            tab.panes
                .iter()
                .any(|pane| pane.session_id.as_deref() == Some(id))
        })
        .ok_or_else(|| fail(Code::NotFound, format!("pane {id} is not in a saved tab")))
}

fn execute(args: Args) -> anyhow::Result<Value> {
    // Inside a pane, the host that runs it, even when this process resolves
    // another config directory.
    let root = args
        .session_dir
        .or_else(|| {
            std::env::var_os("TERMY_MUX_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
        .or_else(termy_core::multiplexer::default_root)
        .context("cannot determine session directory; pass --session-dir")?;
    match args.command {
        Command::Shutdown { force } => {
            if force {
                termy_core::multiplexer::kill_host(&root)?;
            } else {
                // Also stops a host left running by another Termy version.
                termy_core::multiplexer::shutdown_host(&root)?;
            }
            return Ok(json!({"stopped":true}));
        }
        Command::Status => return status(root),
        Command::Create {
            cols,
            rows,
            working_directory: directory,
            shell,
            window,
            workspace,
        } => {
            // Arguments are checked before a background process starts.
            let (mut config, shell_integration, warnings) = user_settings();
            if shell.is_some() {
                config.shell = shell;
            }
            let launch = PaneLaunch {
                size: TerminalSize {
                    cols,
                    rows,
                    ..TerminalSize::default()
                },
                working_directory: Some(working_directory(directory.as_deref())?),
                config,
                launch: None,
                shell_integration: Some(shell_integration),
                environment: Some(PaneLaunch::client_environment()),
            };
            let client = connect_or_start(&root, &std::env::current_exe()?)?;
            let id = match window {
                Some(window_id) => {
                    let index = workspace_index(&client, &window_id, workspace)?;
                    // The CLI has no snapshot of its own to protect: the
                    // workspace only has to exist (audit B17).
                    client.create_tab_in_workspace(launch, &window_id, index)?
                }
                None => client.create_tab(launch, None)?,
            };
            return Ok(with_warnings(json!({"id":id}), warnings));
        }
        _ => {}
    }
    let client = if matches!(args.command, Command::Start) {
        connect_or_start(&root, &std::env::current_exe()?)?
    } else {
        SessionClient::connect(&root).map_err(|error| {
            if is_version_mismatch(&error) {
                error
            } else {
                error.context("cannot connect to the multiplexer; use mux start")
            }
        })?
    };
    match args.command {
        // Every host this client can reach supports conditional layout edits.
        Command::Start => Ok(json!({"session_dir":root,"conditional_layout_updates":true})),
        Command::List { verbose } => list(&client, verbose),
        Command::Send {
            id,
            text,
            // Clap allows it exactly when there is no text.
            stdin: _,
            enter,
            wait,
            timeout_ms,
        } => {
            let mut bytes = if let Some(text) = text {
                text.into_bytes()
            } else {
                // Bytes, not text: input need not be UTF-8 (audit L12).
                let mut bytes = Vec::new();
                std::io::stdin()
                    .lock()
                    .take(MAX_INPUT_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .context("cannot read standard input")?;
                bytes
            };
            if bytes.len() > MAX_INPUT_BYTES {
                return Err(fail(Code::InvalidArgument, "input exceeds 1 MiB"));
            }
            if enter {
                bytes.push(b'\r');
            }
            let length = bytes.len();
            let anchor = client.write(&id, bytes)?;
            let mut result = json!({"id":id,"bytes":length,"anchor":anchor});
            if let Some(text) = wait {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                let found = wait_for_text(
                    &client,
                    &id,
                    &text,
                    Origin::After(anchor),
                    deadline,
                    timeout_ms,
                )
                .context("the input was sent")?;
                result["wait"] = found;
            }
            Ok(result)
        }
        Command::Key {
            id,
            keys,
            control,
            alt,
            shift,
        } => {
            let held = TermyModifiers {
                control,
                alt,
                shift,
                ..TermyModifiers::default()
            };
            let keyboard_mode = client.snapshot(&id)?.keyboard_mode;
            let mut bytes = Vec::new();
            for key in &keys {
                bytes.extend(encode_key(key, held, keyboard_mode)?);
            }
            // One write, so no other input lands between the keys.
            let length = bytes.len();
            let anchor = client.write(&id, bytes)?;
            Ok(json!({"id":id,"keys":keys.len(),"bytes":length,"anchor":anchor}))
        }
        Command::Capture { id, start, end } => capture(&client, &id, start, end),
        Command::Wait {
            id,
            text,
            command_done,
            start,
            after,
            timeout_ms,
        } => {
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            if let Some(text) = text {
                let origin = after.map_or(Origin::Screen(start), Origin::After);
                wait_for_text(&client, &id, &text, origin, deadline, timeout_ms)
            } else {
                debug_assert!(command_done);
                wait_for_command(&client, &id, deadline, timeout_ms)
            }
        }
        Command::Resize { id, cols, rows } => resize(&client, &id, cols, rows),
        Command::Close { id } => {
            // Only an ID the host or the layout knows: a typo must not report
            // that a pane was closed (audit L14).
            let live = client.list()?.iter().any(|pane| pane.id == id);
            if !live && !layout(&client)?.session_ids().any(|saved| saved == id) {
                return Err(fail(Code::NotFound, format!("no pane has ID {id}")));
            }
            let warnings = client.close_saved_pane(&id)?;
            Ok(with_warnings(json!({"id":id}), warnings))
        }
        Command::Layout => match client.layout()? {
            Some(layout) => Ok(serde_json::from_str(&layout)?),
            None => Ok(Value::Null),
        },
        Command::Tab { id, edit } => {
            use termy_core::session_model::{PaneResizeEdge, TabEdit};
            let layout = layout(&client)?;
            let expected = find_tab(&layout, &id)?;
            let edit = match edit {
                TabCommand::Rename { title } => TabEdit::Rename { title: Some(title) },
                TabCommand::ResetTitle => TabEdit::Rename { title: None },
                TabCommand::Pin => TabEdit::SetPinned { pinned: true },
                TabCommand::Unpin => TabEdit::SetPinned { pinned: false },
                TabCommand::Zoom => TabEdit::SetZoomed { zoomed: true },
                TabCommand::Unzoom => TabEdit::SetZoomed { zoomed: false },
                TabCommand::Focus { pane_id } => TabEdit::FocusPane { id: pane_id },
                TabCommand::ResizeDivider { edge, delta } => TabEdit::ResizeDivider {
                    id: id.clone(),
                    delta,
                    edge: match edge.as_str() {
                        "left" => PaneResizeEdge::Left,
                        "right" => PaneResizeEdge::Right,
                        "top" => PaneResizeEdge::Top,
                        _ => PaneResizeEdge::Bottom,
                    },
                },
            };
            let (layout, warnings) = client.edit_tab(&id, expected, &edit)?;
            Ok(with_warnings(serde_json::to_value(layout)?, warnings))
        }
        Command::Split { id, axis } => {
            let layout = layout(&client)?;
            let tab = find_tab(&layout, &id)?;
            let source = pane(&client, &id)?;
            // The source pane's shell and settings (audit B26), in its
            // current directory, with this command's environment.
            let settings = client.launch_config(&id)?;
            let axis = if axis == "horizontal" {
                termy_core::session_model::PaneResizeAxis::Horizontal
            } else {
                termy_core::session_model::PaneResizeAxis::Vertical
            };
            let (new_id, warnings) = client.split_pane(
                &id,
                axis,
                tab,
                PaneLaunch {
                    // The split pane's saved size replaces it.
                    size: TerminalSize::default(),
                    working_directory: PaneLaunch::client_working_directory(
                        source.working_directory.as_deref(),
                        settings.config.working_dir_fallback,
                    ),
                    config: settings.config,
                    launch: None,
                    shell_integration: settings.shell_integration,
                    environment: Some(PaneLaunch::client_environment()),
                },
            )?;
            Ok(with_warnings(
                json!({"id":new_id,"source_id":id,"axis":axis}),
                warnings,
            ))
        }
        Command::Window { id, edit } => {
            use termy_core::session_model::WindowEdit;
            let layout = layout(&client)?;
            let expected = &layout
                .windows
                .iter()
                .find(|window| window.id == id)
                .ok_or_else(|| fail(Code::NotFound, format!("no window has ID {id}")))?
                .session;
            let edit = match edit {
                WindowCommand::Create { name } => WindowEdit::CreateWorkspace { name },
                WindowCommand::Select { index } => WindowEdit::SelectWorkspace { index },
                WindowCommand::Move { from, to } => WindowEdit::MoveWorkspace { from, to },
                WindowCommand::DeleteEmpty { index } => WindowEdit::DeleteEmptyWorkspace { index },
            };
            Ok(serde_json::to_value(
                client.edit_window(&id, expected, &edit)?,
            )?)
        }
        Command::Workspace {
            window,
            index,
            edit,
        } => {
            use termy_core::session_model::WorkspaceEdit;
            let layout = layout(&client)?;
            let expected = layout
                .windows
                .iter()
                .find(|item| item.id == window)
                .and_then(|window| window.session.workspaces.get(index))
                .ok_or_else(|| {
                    fail(
                        Code::NotFound,
                        format!("window {window} has no workspace {index}"),
                    )
                })?;
            let edit = match edit {
                WorkspaceCommand::Rename { name } => WorkspaceEdit::Rename { name },
                WorkspaceCommand::Pin => WorkspaceEdit::SetPinned { pinned: true },
                WorkspaceCommand::Unpin => WorkspaceEdit::SetPinned { pinned: false },
                WorkspaceCommand::SelectTab { index } => WorkspaceEdit::SelectTab { index },
                WorkspaceCommand::MoveTab { from, to } => WorkspaceEdit::MoveTab { from, to },
            };
            Ok(serde_json::to_value(
                client.edit_workspace(&window, index, expected, &edit)?,
            )?)
        }
        Command::Shutdown { .. } | Command::Status | Command::Create { .. } => {
            unreachable!("handled before connecting")
        }
    }
}

/// Whether a host runs, without starting one or creating its directory.
fn status(root: PathBuf) -> anyhow::Result<Value> {
    if !termy_core::multiplexer::host_is_running(&root) {
        return Ok(json!({"session_dir":root,"running":false}));
    }
    match SessionClient::connect(&root) {
        Ok(client) => Ok(
            json!({"session_dir":root,"running":true,"compatible":true,"panes":client.list()?.len()}),
        ),
        Err(error) if is_version_mismatch(&error) => {
            Ok(json!({"session_dir":root,"running":true,"compatible":false}))
        }
        Err(error) => Err(error),
    }
}

/// The workspace `create --window` targets: `requested`, or the window's
/// active one (audit L15).
fn workspace_index(
    client: &SessionClient,
    window_id: &str,
    requested: Option<usize>,
) -> anyhow::Result<usize> {
    let layout = layout(client)?;
    let session = &layout
        .windows
        .iter()
        .find(|window| window.id == window_id)
        .ok_or_else(|| fail(Code::NotFound, format!("no window has ID {window_id}")))?
        .session;
    let count = session.workspaces.len();
    match requested {
        Some(index) if index < count => Ok(index),
        Some(index) => Err(fail(
            Code::NotFound,
            format!("window {window_id} has no workspace {index}"),
        )),
        None if count == 0 => Err(fail(
            Code::NotFound,
            format!("window {window_id} has no workspace"),
        )),
        None => Ok(session.active_workspace.min(count - 1)),
    }
}

fn list(client: &SessionClient, verbose: bool) -> anyhow::Result<Value> {
    let panes = client.list()?;
    Ok(panes
        .into_iter()
        .map(|pane| {
            let mut entry = json!({"id":pane.id,"child_pid":pane.child_pid,"title":pane.title,
                "working_directory":pane.working_directory,"exited":pane.exited});
            if verbose {
                // A pane closed meanwhile has no size.
                let size = client.size(&pane.id).ok();
                entry["cols"] = json!(size.map(|size| size.cols));
                entry["rows"] = json!(size.map(|size| size.rows));
                entry["shell"] = json!(pane.shell.map(|shell| json!({
                    "running":shell.running,
                    "commands_started":shell.commands_started,
                    "commands_finished":shell.commands_finished,
                    "exit_code":shell.exit_code,
                })));
            }
            entry
        })
        .collect())
}

/// Resizes a pane alone in its tab, keeping its saved size in step and its
/// cell metrics (audit L8). A pane that shares its tab follows the tab's
/// dividers instead, so resizing it alone is refused.
fn resize(client: &SessionClient, id: &str, cols: u16, rows: u16) -> anyhow::Result<Value> {
    let current = client.size(id)?;
    let saved = layout(client)?;
    if let Ok(tab) = find_tab(&saved, id)
        && tab.panes.len() > 1
    {
        return Err(fail(
            Code::Unsupported,
            format!(
                "pane {id} shares its tab with other panes; move a divider with `termy mux tab {id} resize-divider` instead"
            ),
        ));
    }
    client.resize(
        id,
        TerminalSize {
            cols,
            rows,
            ..current
        },
    )?;
    // The resize is done; a stale saved size only affects later splits.
    let warnings = client
        .update_layout(|layout| {
            for window in &mut layout.windows {
                for workspace in &mut window.session.workspaces {
                    for tab in &mut workspace.tabs {
                        if let [pane] = tab.panes.as_mut_slice()
                            && pane.session_id.as_deref() == Some(id)
                        {
                            pane.width = cols;
                            pane.height = rows;
                        }
                    }
                }
            }
            Ok(())
        })
        .err()
        .map(|error| vec![format!("Saving the new size failed: {error:#}")])
        .unwrap_or_default();
    Ok(with_warnings(
        json!({"id":id,"cols":cols,"rows":rows}),
        warnings,
    ))
}

/// Rows of text joined into lines: a soft-wrapped row continues on the next
/// one without a line break, so text longer than the pane is found whole.
struct Joined {
    text: String,
    /// Where each row starts in `text`, with its line number.
    rows: Vec<(usize, i32)>,
}

fn join<'a>(first: i32, lines: impl IntoIterator<Item = &'a LineText>, cols: usize) -> Joined {
    let mut joined = Joined {
        text: String::new(),
        rows: Vec::new(),
    };
    let mut continues = false;
    for (line, row) in (first..).zip(lines) {
        if !joined.rows.is_empty() && !continues {
            joined.text.push('\n');
        }
        joined.rows.push((joined.text.len(), line));
        if row.wrapped {
            // Blanks before the wrap are part of the line.
            joined.text.extend(row.cells(cols));
        } else {
            joined.text.push_str(&row.text);
        }
        continues = row.wrapped;
    }
    joined
}

/// Text of the pane's lines, read from the host's engine rather than a
/// view's viewport: no client's scroll position changes it (audit B18).
fn capture(
    client: &SessionClient,
    id: &str,
    start: Option<i32>,
    end: Option<i32>,
) -> anyhow::Result<Value> {
    let start = start.unwrap_or(0);
    let end = end.unwrap_or(i32::MAX);
    if start > end {
        return Err(fail(Code::InvalidArgument, "--start is after --end"));
    }
    let state = client.snapshot(id)?;
    let texts = client.line_texts(id, start, end)?;
    let joined = join(texts.first, &texts.lines, texts.bounds.2);
    let last = (!texts.lines.is_empty()).then(|| texts.first + texts.lines.len() as i32 - 1);
    Ok(
        json!({"id":id,"cols":state.size.cols,"rows":state.size.rows,
        "start":last.map(|_| texts.first),"end":last,
        "history_size":texts.bounds.0.unsigned_abs(),"text":joined.text,
        "cursor":{"row":state.cursor_position.1,"col":state.cursor_position.0},
        "alternate_screen":state.alternate_screen}),
    )
}

/// Where `wait` starts searching.
#[derive(Clone, Copy)]
enum Origin {
    /// This line of the screen as the wait starts: 0 is its top row,
    /// negative lines are history.
    Screen(i32),
    /// The line input was typed on, as the anchor `send` or `key` returned.
    After(u64),
}

/// Finds a marker in a pane's output, read again on every poll. Lines are
/// numbered from `anchor` (see [`LineTexts::lines_scrolled`]) however far
/// output scrolled; rows that scrolled into history no longer change, so
/// only the last few of them are searched again, for a marker that starts
/// there and ends in newer rows.
struct Search<'a> {
    marker: &'a str,
    anchor: u64,
    /// The first line searched.
    origin: i32,
    /// The screen's width and height the lines were read at.
    size: (usize, u16),
    /// History rows kept for matches that continue into later rows.
    kept: Vec<LineText>,
    /// The line number of the first row of `kept`.
    kept_first: i32,
    /// History dropped lines before they could be searched (audit B18).
    lost: bool,
}

/// Where a marker was found.
#[derive(Debug, PartialEq, Eq)]
struct Found {
    /// The line the match starts on, numbered from the current screen.
    line: i32,
    /// The lines that contain the match, joined like `capture` joins them.
    text: String,
}

impl<'a> Search<'a> {
    /// `texts` are the lines read from `origin` on. With `existed`, line
    /// `origin` was in the buffer before, so history dropped it if `texts`
    /// starts later.
    fn new(marker: &'a str, origin: i32, texts: &LineTexts, existed: bool) -> Self {
        Self {
            marker,
            anchor: texts.lines_scrolled,
            origin,
            size: (texts.bounds.2, texts.rows),
            kept: Vec::new(),
            kept_first: texts.first,
            lost: existed && texts.first > origin,
        }
    }

    /// The first line to read next.
    fn next_line(&self) -> i32 {
        self.kept_first + self.kept.len() as i32
    }

    /// Whether `texts` were read at another size than the search began at.
    /// Lines then reflowed, which moved them by an amount that no counter
    /// tracks.
    fn resized(&self, texts: &LineTexts) -> bool {
        (texts.bounds.2, texts.rows) != self.size
    }

    /// The first line to search again after a resize, numbered from the
    /// current screen: as many history lines before it as output added since
    /// the search began, from the same origin.
    fn restart_line(&self, texts: &LineTexts) -> i32 {
        let moved = texts.scrolled as i64 - self.anchor as i64;
        (i64::from(self.origin) - moved).clamp(i32::MIN.into(), 0) as i32
    }

    /// Searches the kept rows and `texts`, the lines read from
    /// [`Self::next_line`] on, then keeps what scrolled into history.
    fn scan(&mut self, texts: LineTexts) -> Option<Found> {
        if texts.first != self.next_line() {
            // History dropped the lines between the last read and this one.
            self.lost |= texts.first > self.next_line();
            self.kept.clear();
            self.kept_first = texts.first;
        }
        // The line number of the current screen's top row.
        let screen_top = (texts.scrolled as i64 - self.anchor as i64) as i32;
        let cols = texts.bounds.2;
        let joined = join(self.kept_first, self.kept.iter().chain(&texts.lines), cols);
        if let Some(offset) = joined.text.find(self.marker) {
            let row = joined
                .rows
                .iter()
                .rev()
                .find(|(start, _)| *start <= offset)
                .map_or(self.kept_first, |(_, line)| *line);
            let end = offset + self.marker.len();
            let from = joined.text[..offset]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            let to = joined.text[end..]
                .find('\n')
                .map_or(joined.text.len(), |index| end + index);
            return Some(Found {
                line: row - screen_top,
                text: joined.text[from..to].to_owned(),
            });
        }
        let frozen = usize::try_from(screen_top - texts.first)
            .unwrap_or(0)
            .min(texts.lines.len());
        self.kept.extend(texts.lines.into_iter().take(frozen));
        // Keep the fewest last rows whose joined text is as long as the
        // marker: a match starting before them would lie within them, and
        // was not there.
        let mut length = 0;
        let keep = self
            .kept
            .iter()
            .rev()
            .take_while(|row| {
                let more = length < self.marker.len();
                length += if row.wrapped {
                    row.cells(cols).map(str::len).sum()
                } else {
                    row.text.len() + 1
                };
                more
            })
            .count();
        let dropped = self.kept.len() - keep;
        self.kept.drain(..dropped);
        self.kept_first += dropped as i32;
        None
    }
}

/// Waits until the pane's output from `origin` on contains `marker`, also
/// once it scrolled into history. Fails fast when the pane exits or closes
/// (audit L10).
fn wait_for_text(
    client: &SessionClient,
    id: &str,
    marker: &str,
    origin: Origin,
    deadline: Instant,
    timeout_ms: u64,
) -> anyhow::Result<Value> {
    let (texts, mut search) = match origin {
        Origin::Screen(start) => {
            let texts = client.line_texts(id, start, i32::MAX)?;
            let search = Search::new(marker, start, &texts, false);
            (texts, search)
        }
        Origin::After(anchor) => {
            let texts = client.line_texts_anchored(id, 0, i32::MAX, Some(anchor))?;
            let search = Search::new(marker, 0, &texts, true);
            (texts, search)
        }
    };
    let mut texts = Some(texts);
    let mut exited = false;
    loop {
        let mut read = match texts.take() {
            Some(texts) => texts,
            None => {
                client.line_texts_anchored(id, search.next_line(), i32::MAX, Some(search.anchor))?
            }
        };
        if search.resized(&read) {
            // Search the current screen again, and the history output added
            // since the search began.
            let first = search.restart_line(&read);
            read = client.line_texts(id, first, i32::MAX)?;
            let lost = search.lost;
            search = Search::new(marker, first, &read, false);
            search.lost = lost;
        }
        if let Some(found) = search.scan(read) {
            return Ok(json!({"id":id,"line":found.line,"text":found.text,
                "truncated":search.lost}));
        }
        let lost = if search.lost {
            "; output scrolled out of the scrollback before it could be searched"
        } else {
            ""
        };
        if exited {
            return Err(fail(
                Code::Exited,
                format!("pane {id} exited before its output contained {marker:?}{lost}"),
            ));
        }
        // Read once more after an exit, for output printed just before it.
        exited = pane(client, id)
            .map_err(|error| {
                if Code::of(&error) == Code::NotFound {
                    fail(
                        Code::NotFound,
                        format!("pane {id} was closed while waiting"),
                    )
                } else {
                    error
                }
            })?
            .exited;
        if exited {
            continue;
        }
        if Instant::now() >= deadline {
            return Err(fail(
                if search.lost {
                    Code::OutputLost
                } else {
                    Code::Timeout
                },
                format!("timed out after {timeout_ms} ms waiting for {marker:?}{lost}"),
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Waits until the command that the pane's latest input started has
/// finished, from its OSC 133 shell-integration reports (audit I14; see
/// [`InputCommand`] for which finish counts).
fn wait_for_command(
    client: &SessionClient,
    id: &str,
    deadline: Instant,
    timeout_ms: u64,
) -> anyhow::Result<Value> {
    loop {
        let info = pane(client, id)?;
        let command = info.shell.and_then(|shell| shell.input_command);
        if let Some(InputCommand::Finished(exit_code)) = command {
            return Ok(json!({"id":id,"exit_code":exit_code}));
        }
        if info.exited {
            return Err(fail(
                Code::Exited,
                format!("pane {id} exited before its command finished"),
            ));
        }
        if Instant::now() >= deadline {
            let hint = match (info.shell, command) {
                (None, _) => {
                    "; the pane reported no shell integration (OSC 133), which --command-done needs"
                }
                (Some(_), None) => "; no input reached the pane",
                _ => "",
            };
            return Err(fail(
                Code::Timeout,
                format!("timed out after {timeout_ms} ms waiting for the command to finish{hint}"),
            ));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Key names `key` accepts, with their aliases, in lowercase; any case is
/// accepted. A single character is sent as itself.
const KEY_NAMES: &[(&str, &[&str])] = &[
    ("enter", &["enter", "return", "ret", "cr"]),
    ("tab", &["tab"]),
    ("escape", &["escape", "esc"]),
    ("backspace", &["backspace", "bs", "bksp"]),
    ("space", &["space", "spc"]),
    ("up", &["up"]),
    ("down", &["down"]),
    ("left", &["left"]),
    ("right", &["right"]),
    ("home", &["home"]),
    ("end", &["end"]),
    ("pageup", &["pageup", "pgup", "page-up", "page_up"]),
    (
        "pagedown",
        &["pagedown", "pgdn", "pgdown", "page-down", "page_down"],
    ),
    ("insert", &["insert", "ins"]),
    ("delete", &["delete", "del"]),
    ("f1", &["f1"]),
    ("f2", &["f2"]),
    ("f3", &["f3"]),
    ("f4", &["f4"]),
    ("f5", &["f5"]),
    ("f6", &["f6"]),
    ("f7", &["f7"]),
    ("f8", &["f8"]),
    ("f9", &["f9"]),
    ("f10", &["f10"]),
    ("f11", &["f11"]),
    ("f12", &["f12"]),
];

/// The bytes of one `key` argument, such as `enter`, `q` or `ctrl+alt+x`,
/// with `held` added to its modifiers. Encoded as on Linux whatever the
/// platform (audit B25).
fn encode_key(
    spec: &str,
    held: TermyModifiers,
    keyboard_mode: TerminalKeyboardMode,
) -> anyhow::Result<Vec<u8>> {
    let invalid = |message: String| fail(Code::InvalidArgument, message);
    // `+` itself is a key: `+`, `ctrl++`.
    let (prefix, name) = match spec.strip_suffix("++") {
        Some(prefix) => (Some(prefix), "+"),
        None if spec.len() > 1 => match spec.rsplit_once('+') {
            Some((prefix, name)) => (Some(prefix), name),
            None => (None, spec),
        },
        None => (None, spec),
    };
    let mut modifiers = held;
    for modifier in prefix.into_iter().flat_map(|prefix| prefix.split('+')) {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers.control = true,
            "alt" | "meta" | "option" | "opt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            _ => {
                return Err(invalid(format!(
                    "unknown modifier {modifier:?} in key {spec:?}; use ctrl, alt or shift"
                )));
            }
        }
    }
    let mut characters = name.chars();
    let (key, key_char) = if let (Some(character), None) = (characters.next(), characters.next()) {
        if character.is_ascii_uppercase() {
            modifiers.shift = true;
        }
        let key = character.to_ascii_lowercase().to_string();
        // The text the key types; Control types none.
        let text = if modifiers.shift {
            character.to_uppercase().collect()
        } else {
            character.to_string()
        };
        (key, (!modifiers.control).then_some(text))
    } else {
        let lowercase = name.to_ascii_lowercase();
        let key = KEY_NAMES
            .iter()
            .find(|(_, aliases)| aliases.contains(&lowercase.as_str()))
            .map(|(key, _)| *key)
            .ok_or_else(|| {
                invalid(format!(
                    "unknown key {name:?}; use a single character or one of: {}",
                    KEY_NAMES
                        .iter()
                        .flat_map(|(_, aliases)| aliases.iter())
                        .copied()
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        (key.to_owned(), None)
    };
    let stroke = TermyKeystroke {
        key,
        key_char,
        modifiers,
    };
    termy_core::portable_keystroke_to_input(
        &stroke,
        termy_core::TerminalKeyEventKind::Press,
        keyboard_mode,
    )
    .ok_or_else(|| invalid(format!("key {spec:?} has no terminal encoding")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(spec: &str) -> Vec<u8> {
        encode_key(
            spec,
            TermyModifiers::default(),
            TerminalKeyboardMode::default(),
        )
        .unwrap()
    }

    /// Audit B25: every name and chord has one encoding, on every platform.
    #[test]
    fn keys_encode_the_same_on_every_platform() {
        for (spec, expected) in [
            ("enter", &b"\r"[..]),
            ("Return", b"\r"),
            ("ESC", b"\x1b"),
            ("bs", b"\x7f"),
            ("Tab", b"\t"),
            ("shift+tab", b"\x1b[Z"),
            ("space", b" "),
            ("PgUp", b"\x1b[5~"),
            ("pgdn", b"\x1b[6~"),
            ("del", b"\x1b[3~"),
            ("up", b"\x1b[A"),
            ("f5", b"\x1b[15~"),
            ("c", b"c"),
            ("C", b"C"),
            ("shift+c", b"C"),
            ("ctrl+c", b"\x03"),
            ("CTRL+C", b"\x03"),
            ("ctrl+left", b"\x1b[1;5D"),
            ("alt+left", b"\x1b[1;3D"),
            ("ctrl+backspace", b"\x7f"),
            ("alt+b", b"\x1bb"),
            ("alt+B", b"\x1bB"),
            ("alt+enter", b"\x1b\r"),
            ("ctrl+alt+x", b"\x1b\x18"),
            ("+", b"+"),
            ("alt++", b"\x1b+"),
            ("-", b"-"),
            ("é", "é".as_bytes()),
        ] {
            assert_eq!(bytes(spec), expected, "{spec}");
        }
    }

    #[test]
    fn held_modifiers_apply_to_every_key() {
        let control = TermyModifiers {
            control: true,
            ..TermyModifiers::default()
        };
        let encode = |spec| encode_key(spec, control, TerminalKeyboardMode::default()).unwrap();
        assert_eq!(encode("c"), b"\x03");
        assert_eq!(encode("alt+x"), b"\x1b\x18");
        let shift = TermyModifiers {
            shift: true,
            ..TermyModifiers::default()
        };
        assert_eq!(
            encode_key("a", shift, TerminalKeyboardMode::default()).unwrap(),
            b"A"
        );
    }

    #[test]
    fn keys_follow_the_kitty_keyboard_protocol_when_enabled() {
        let disambiguate =
            TerminalKeyboardMode::from_flags(false, true, false, false, false, false);
        let encode = |spec| encode_key(spec, TermyModifiers::default(), disambiguate).unwrap();
        assert_eq!(encode("ctrl+c"), b"\x1b[99;5u");
        assert_eq!(encode("alt+b"), b"\x1b[98;3u");
        assert_eq!(encode("esc"), b"\x1b[27u");
    }

    #[test]
    fn unknown_keys_and_modifiers_are_invalid_arguments() {
        for spec in ["fn", "hyper+a", "ctrl+", "f13"] {
            let error = encode_key(
                spec,
                TermyModifiers::default(),
                TerminalKeyboardMode::default(),
            )
            .unwrap_err();
            assert_eq!(Code::of(&error), Code::InvalidArgument, "{spec}: {error}");
        }
    }

    fn row(text: &str, wrapped: bool) -> LineText {
        LineText {
            text: text.into(),
            columns: vec![1; text.len()],
            wrapped,
        }
    }

    /// Lines numbered as when 10 lines had scrolled, read from a 4-row screen
    /// after `scrolled` had.
    fn texts(first: i32, scrolled: u64, rows: Vec<LineText>) -> LineTexts {
        let moved = scrolled as i32 - 10;
        LineTexts {
            bounds: (-100 + moved, 3 + moved, 6),
            first,
            lines_scrolled: 10,
            scrolled,
            rows: 4,
            lines: rows,
        }
    }

    #[test]
    fn soft_wrapped_rows_join_into_one_line() {
        let joined = join(
            -1,
            &[row("ab cd", true), row("ef", false), row("gh", false)],
            6,
        );
        // The blank at the wrap belongs to the line.
        assert_eq!(joined.text, "ab cd ef\ngh");
        assert_eq!(joined.rows, [(0, -1), (6, 0), (9, 1)]);
    }

    /// Audit B18: a marker wider than the pane is found across its wrap.
    #[test]
    fn markers_longer_than_a_row_match_across_wraps() {
        let start = texts(0, 10, vec![row("$ run", false), row("", false)]);
        let mut search = Search::new("READY_NOW", 0, &start, false);
        assert_eq!(search.scan(start.clone()), None);
        let found = search
            .scan(texts(
                0,
                10,
                vec![row("$ run", false), row("READY_", true), row("NOW", false)],
            ))
            .unwrap();
        assert_eq!(
            found,
            Found {
                line: 1,
                text: "READY_NOW".into()
            }
        );
    }

    /// Audit B18: output that scrolled into history between two reads is
    /// still searched, and kept rows join the rows read after them.
    #[test]
    fn markers_that_scrolled_into_history_between_reads_match() {
        let start = texts(0, 10, vec![row("$ ", false)]);
        let mut search = Search::new("MARK_ER", 0, &start, false);
        assert_eq!(search.scan(start.clone()), None);
        assert_eq!(search.next_line(), 0);
        // Ten lines scrolled: the marker is now history (line 2 is -8).
        let mut rows = vec![row("$ ", false), row("x", false), row("MARK_ER", false)];
        rows.extend((0..12).map(|_| row("y", false)));
        let found = search.scan(texts(0, 20, rows)).unwrap();
        assert_eq!(
            found,
            Found {
                line: -8,
                text: "MARK_ER".into()
            }
        );

        // A marker split across the frozen rows and a later read.
        let start = texts(0, 10, vec![row("", false)]);
        let mut search = Search::new("SPLIT_MARK", 0, &start, false);
        let mut rows: Vec<_> = (0..10).map(|_| row("z", false)).collect();
        rows.push(row("SPLIT_", true));
        assert_eq!(search.scan(texts(0, 21, rows)), None);
        // Only the rows the marker could still continue from are kept, and
        // the next read starts after them.
        assert_eq!(search.kept_first, 8);
        assert_eq!(search.next_line(), 11);
        let found = search
            .scan(texts(11, 21, vec![row("MARK", false)]))
            .unwrap();
        assert_eq!(
            found,
            Found {
                line: -1,
                text: "SPLIT_MARK".into()
            }
        );
    }

    /// A wide character wrapped to the next row leaves a spacer that is no
    /// text, so the joined line has no gap.
    #[test]
    fn wide_characters_join_across_a_wrap() {
        let wrapped = LineText {
            text: "ab日".into(),
            columns: vec![1, 1, 3, 0, 0],
            wrapped: true,
        };
        let rest = LineText {
            text: "本語".into(),
            columns: vec![3, 0, 3, 0],
            wrapped: false,
        };
        assert_eq!(join(0, &[wrapped, rest], 5).text, "ab日本語");
    }

    /// Audit B18: output that left the scrollback before it was read is
    /// reported, both between reads and before the first one of `--after`.
    #[test]
    fn output_evicted_before_it_was_read_is_reported() {
        let start = texts(0, 10, vec![row("$ ", false)]);
        let mut search = Search::new("NEVER", 0, &start, false);
        assert_eq!(search.scan(start.clone()), None);
        assert!(!search.lost);
        // Lines 0..5 scrolled away and were dropped before this read.
        assert_eq!(search.scan(texts(5, 40, vec![row("y", false)])), None);
        assert!(search.lost);

        let evicted = texts(3, 40, vec![row("y", false)]);
        assert!(Search::new("NEVER", 0, &evicted, true).lost);
        // History that never held the lines a screen origin asks for.
        assert!(!Search::new("NEVER", -50, &evicted, false).lost);
    }

    /// Audit B18: a resize reflows lines, so the search restarts from the
    /// current screen with the history output added since it began.
    #[test]
    fn a_resize_restarts_the_search() {
        let start = texts(-2, 10, vec![row("$ ", false)]);
        let search = Search::new("X", -2, &start, false);
        let mut later = texts(0, 25, vec![row("y", false)]);
        assert!(!search.resized(&later));
        later.rows = 5;
        assert!(search.resized(&later));
        assert_eq!(search.restart_line(&later), -17);
    }
}
