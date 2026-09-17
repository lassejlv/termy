use anyhow::{Context, ensure};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};
use termy_core::multiplexer::{PaneLaunch, SessionClient, connect_or_start};
use termy_core::{Terminal, TerminalRuntimeConfig, TerminalSize};

#[derive(ClapArgs)]
pub struct Args {
    /// Multiplexer directory; defaults to the desktop config's multiplexer directory
    #[arg(long, global = true)]
    session_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the background host without creating a terminal
    Start,
    /// List stable pane IDs, child PIDs, titles, and exit state
    List,
    /// Create a persistent terminal (starts the host if necessary)
    Create {
        #[arg(long, default_value_t = 80)]
        cols: u16,
        #[arg(long, default_value_t = 24)]
        rows: u16,
        #[arg(long)]
        working_directory: Option<String>,
        #[arg(long)]
        shell: Option<String>,
        /// Saved window ID; defaults to the first window's active workspace
        #[arg(long)]
        window: Option<String>,
        /// Zero-based workspace index in the selected window
        #[arg(long, requires = "window")]
        workspace: Option<usize>,
    },
    /// Send literal UTF-8 input without shell escaping or interpretation
    Send {
        id: String,
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        text: Option<String>,
        #[arg(long)]
        stdin: bool,
        /// Append Enter after the supplied text
        #[arg(long)]
        enter: bool,
    },
    /// Send a terminal key through the desktop key encoder
    Key {
        id: String,
        key: String,
        #[arg(long)]
        control: bool,
        #[arg(long)]
        alt: bool,
        #[arg(long)]
        shift: bool,
    },
    /// Capture visible text, dimensions, and cursor without taking terminal ownership
    Capture { id: String },
    /// Wait until visible terminal output contains text; exit 1 on timeout
    Wait {
        id: String,
        text: String,
        #[arg(long, default_value_t = 5000)]
        timeout_ms: u64,
    },
    /// Resize a persistent terminal
    Resize { id: String, cols: u16, rows: u16 },
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
    Shutdown,
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

pub fn run(args: Args) {
    match execute(args) {
        Ok(result) => println!("{}", json!({"schema_version":1,"ok":true,"result":result})),
        Err(error) => {
            eprintln!(
                "{}",
                json!({"schema_version":1,"ok":false,"error":{"message":format!("{error:#}")}})
            );
            std::process::exit(1);
        }
    }
}

fn dimensions(cols: u16, rows: u16) -> anyhow::Result<TerminalSize> {
    ensure!(
        (1..=400).contains(&cols) && (1..=200).contains(&rows),
        "size must be between 1x1 and 400x200"
    );
    Ok(TerminalSize {
        cols,
        rows,
        cell_width: 9.0,
        cell_height: 18.0,
    })
}

fn capture(id: &str, terminal: &Terminal) -> Value {
    let render = terminal.render_read(true);
    let lines: Vec<_> = render
        .cells
        .chunks(usize::from(render.metadata.cols).max(1))
        .map(|row| {
            row.iter()
                .filter(|cell| !cell.wide_character_spacer)
                .map(|cell| if cell.hidden { " " } else { cell.text.as_str() })
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    json!({"id":id,"cols":render.metadata.cols,"rows":render.metadata.rows,
        "text":lines.join("\n"),"cursor":render.metadata.cursor.map(|cursor| json!({"row":cursor.row,"col":cursor.col})),
        "display_offset":render.metadata.display_offset,"history_size":render.metadata.history_size})
}

fn execute(args: Args) -> anyhow::Result<Value> {
    let root = args
        .session_dir
        .or_else(|| {
            termy_core::config_core::config_path()?
                .parent()
                .map(|path| path.join("multiplexer"))
        })
        .context("cannot determine session directory; pass --session-dir")?;
    // Validate before starting a background process.
    if let Command::Create { cols, rows, .. } | Command::Resize { cols, rows, .. } = &args.command {
        dimensions(*cols, *rows)?;
    }
    let client = if matches!(args.command, Command::Start | Command::Create { .. }) {
        connect_or_start(&root, &std::env::current_exe()?)?
    } else {
        SessionClient::connect(&root).context("cannot connect to the multiplexer; use mux start")?
    };
    match args.command {
        Command::Start => Ok(
            json!({"session_dir":root,"conditional_layout_updates":client.supports_conditional_layout_updates()}),
        ),
        Command::List => Ok(serde_json::to_value(client.list()?)?),
        Command::Create {
            cols,
            rows,
            working_directory,
            shell,
            window,
            workspace,
        } => {
            let target = if let Some(window_id) = window {
                let layout: termy_core::session_model::StoredMultiplexer =
                    serde_json::from_str(&client.layout()?.context("No saved session layout")?)?;
                let index = workspace.unwrap_or(0);
                let expected = layout
                    .windows
                    .iter()
                    .find(|window| window.id == window_id)
                    .and_then(|window| window.session.workspaces.get(index))
                    .context("Workspace does not exist")?
                    .clone();
                Some(termy_core::session_model::WorkspaceTarget {
                    window_id,
                    index,
                    expected,
                })
            } else {
                None
            };
            let (id, terminal) = client.create_tab(
                PaneLaunch {
                    size: dimensions(cols, rows)?,
                    working_directory,
                    config: TerminalRuntimeConfig {
                        shell,
                        ..Default::default()
                    },
                    launch: None,
                    shell_integration: None,
                },
                None,
                target,
            )?;
            drop(terminal);
            Ok(json!({"id":id}))
        }
        Command::Send {
            id,
            text,
            stdin,
            enter,
        } => {
            let mut text = text.unwrap_or_default();
            if stdin {
                std::io::stdin()
                    .take(1024 * 1024 + 1)
                    .read_to_string(&mut text)?;
            }
            ensure!(text.len() <= 1024 * 1024, "input exceeds 1 MiB");
            if enter {
                text.push('\r');
            }
            let bytes = text.len();
            client.write(&id, text.into_bytes())?;
            Ok(json!({"id":id,"bytes":bytes}))
        }
        Command::Key {
            id,
            key,
            control,
            alt,
            shift,
        } => {
            let terminal = client.attach(&id, None)?;
            let stroke = termy_core::TermyKeystroke {
                key,
                key_char: None,
                modifiers: termy_core::TermyModifiers {
                    control,
                    alt,
                    shift,
                    platform: false,
                    function: false,
                },
            };
            let bytes = termy_core::keystroke_to_input(
                &stroke,
                termy_core::TerminalKeyEventKind::Press,
                terminal.keyboard_mode(),
                true,
            )
            .context("key has no terminal encoding")?;
            client.write(&id, bytes)?;
            Ok(json!({"id":id}))
        }
        Command::Capture { id } => Ok(capture(&id, &client.attach(&id, None)?)),
        Command::Wait {
            id,
            text,
            timeout_ms,
        } => {
            ensure!(
                (1..=3_600_000).contains(&timeout_ms),
                "timeout must be between 1 and 3600000 ms"
            );
            let terminal = client.attach(&id, None)?;
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                let output = capture(&id, &terminal);
                if output["text"]
                    .as_str()
                    .is_some_and(|value| value.contains(&text))
                {
                    return Ok(output);
                }
                ensure!(
                    Instant::now() < deadline,
                    "timed out waiting for terminal output"
                );
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        Command::Resize { id, cols, rows } => {
            client.resize(&id, dimensions(cols, rows)?)?;
            Ok(json!({"id":id,"cols":cols,"rows":rows}))
        }
        Command::Close { id } => {
            client.close_saved_pane(&id)?;
            Ok(json!({"id":id}))
        }
        Command::Layout => match client.layout()? {
            Some(layout) => Ok(serde_json::from_str(&layout)?),
            None => Ok(Value::Null),
        },
        Command::Tab { id, edit } => {
            use termy_core::session_model::{PaneResizeEdge, TabEdit};
            let layout: termy_core::session_model::StoredMultiplexer =
                serde_json::from_str(&client.layout()?.context("No saved session layout")?)?;
            let expected = layout
                .windows
                .iter()
                .flat_map(|w| &w.session.workspaces)
                .flat_map(|w| &w.tabs)
                .find(|tab| {
                    tab.panes
                        .iter()
                        .any(|pane| pane.session_id.as_deref() == Some(&id))
                })
                .context("Pane is not in a saved tab")?;
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
            Ok(serde_json::to_value(
                client.edit_tab(&id, expected, &edit)?,
            )?)
        }
        Command::Split { id, axis } => {
            let layout: termy_core::session_model::StoredMultiplexer =
                serde_json::from_str(&client.layout()?.context("No saved session layout")?)?;
            let tab = layout
                .windows
                .iter()
                .flat_map(|w| &w.session.workspaces)
                .flat_map(|w| &w.tabs)
                .find(|tab| {
                    tab.panes
                        .iter()
                        .any(|pane| pane.session_id.as_deref() == Some(&id))
                })
                .context("Pane is not in a saved tab")?;
            let working_directory = client
                .list()?
                .into_iter()
                .find(|pane| pane.id == id)
                .and_then(|pane| pane.working_directory);
            let axis = if axis == "horizontal" {
                termy_core::session_model::PaneResizeAxis::Horizontal
            } else {
                termy_core::session_model::PaneResizeAxis::Vertical
            };
            let (new_id, terminal) = client.split_pane(
                &id,
                axis,
                tab,
                PaneLaunch {
                    size: dimensions(80, 24)?,
                    working_directory,
                    config: TerminalRuntimeConfig::default(),
                    launch: None,
                    shell_integration: None,
                },
            )?;
            drop(terminal);
            Ok(json!({"id":new_id,"source_id":id,"axis":axis}))
        }
        Command::Window { id, edit } => {
            use termy_core::session_model::WindowEdit;
            let layout: termy_core::session_model::StoredMultiplexer =
                serde_json::from_str(&client.layout()?.context("No saved session layout")?)?;
            let expected = &layout
                .windows
                .iter()
                .find(|window| window.id == id)
                .context("Window does not exist")?
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
            let layout: termy_core::session_model::StoredMultiplexer =
                serde_json::from_str(&client.layout()?.context("No saved session layout")?)?;
            let expected = layout
                .windows
                .iter()
                .find(|item| item.id == window)
                .and_then(|window| window.session.workspaces.get(index))
                .context("Workspace does not exist")?;
            let edit = match edit {
                WorkspaceCommand::Rename { name } => {
                    termy_core::session_model::WorkspaceEdit::Rename { name }
                }
                WorkspaceCommand::Pin => {
                    termy_core::session_model::WorkspaceEdit::SetPinned { pinned: true }
                }
                WorkspaceCommand::Unpin => {
                    termy_core::session_model::WorkspaceEdit::SetPinned { pinned: false }
                }
                WorkspaceCommand::SelectTab { index } => {
                    termy_core::session_model::WorkspaceEdit::SelectTab { index }
                }
                WorkspaceCommand::MoveTab { from, to } => {
                    termy_core::session_model::WorkspaceEdit::MoveTab { from, to }
                }
            };
            Ok(serde_json::to_value(
                client.edit_workspace(&window, index, expected, &edit)?,
            )?)
        }
        Command::Shutdown => {
            client.shutdown()?;
            Ok(json!({"stopped":true}))
        }
    }
}
