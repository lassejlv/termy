use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(name = "termy")]
#[command(about = "Termy terminal emulator CLI", long_about = None)]
#[command(version)]
struct Cli {
    /// Open Termy with this working directory
    #[arg(
        long = "working-directory",
        value_name = "PATH",
        conflicts_with = "path"
    )]
    working_directory: Option<PathBuf>,

    /// Open a separate terminal window
    #[arg(long, conflicts_with = "new_tab")]
    new_window: bool,

    /// Open a tab in the running terminal window
    #[arg(long, conflicts_with = "new_window")]
    new_tab: bool,

    /// Open Termy with this working directory
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    #[command(subcommand)]
    action: Option<Action>,
}

#[derive(Subcommand)]
enum Action {
    /// Install and manage plugins
    Plugin {
        #[command(subcommand)]
        command: PluginCommand,
    },

    /// Show version information
    #[command(name = "-version")]
    Version,

    /// Show help and available actions
    #[command(name = "-help")]
    Help,

    /// List available monospace fonts
    #[command(name = "-list-fonts")]
    ListFonts,

    /// List all keybindings
    #[command(name = "-list-keybinds")]
    ListKeybinds,

    /// List available themes
    #[command(name = "-list-themes")]
    ListThemes,

    /// Show current theme colors
    #[command(name = "-list-colors")]
    ListColors,

    /// List available keybind actions
    #[command(name = "-list-actions")]
    ListActions,

    /// Open config file in editor
    #[command(name = "-edit-config")]
    EditConfig,

    /// Display current configuration
    #[command(name = "-show-config")]
    ShowConfig,

    /// Validate configuration file
    #[command(name = "-validate-config")]
    ValidateConfig,

    /// Prettify configuration file (removes comments, formats consistently)
    #[command(name = "-prettify-config")]
    PrettifyConfig,

    /// Interactive TUI for all CLI features
    #[command(name = "-tui")]
    Tui,

    /// Check for updates
    #[command(name = "-update")]
    Update,

    /// Export the current resolved theme into a Termy themes repo checkout
    #[command(name = "-export-theme")]
    ExportTheme {
        /// Local path to the termy-org/themes checkout
        #[arg(long)]
        repo: PathBuf,
        /// Theme slug, normalized to Termy's theme id format
        #[arg(long)]
        slug: String,
        /// Display name for the theme
        #[arg(long)]
        name: String,
        /// Semver version, for example 1.0.0
        #[arg(long)]
        version: String,
        /// Theme description
        #[arg(long, default_value = "")]
        description: String,
        /// Overwrite an existing files/<version>.json
        #[arg(long)]
        force: bool,
    },

    /// Validate a Termy themes repo checkout
    #[command(name = "-validate-theme-repo")]
    ValidateThemeRepo {
        /// Local path to the termy-org/themes checkout
        #[arg(long)]
        repo: PathBuf,
    },
}

#[derive(Subcommand)]
enum PluginCommand {
    /// Install a plugin from a local directory or GitHub repository
    #[command(visible_alias = "install")]
    Add {
        /// Local plugin directory, GitHub repository, or /tree/<ref>/<path> URL
        #[arg(value_name = "SOURCE")]
        source: String,
        /// Git branch, tag, or commit to install from GitHub
        #[arg(long = "ref", value_name = "REF")]
        reference: Option<String>,
        /// Plugin directory inside a GitHub repository
        #[arg(long, value_name = "PATH")]
        path: Option<String>,
        /// Accept the trusted-code warning without prompting
        #[arg(long)]
        yes: bool,
    },

    /// Create a plugin scaffold
    Init {
        /// Directory to initialize
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
        /// Stable lowercase plugin ID; defaults to the directory name
        #[arg(long)]
        id: Option<String>,
        /// Display name; defaults to a title made from the plugin ID
        #[arg(long)]
        name: Option<String>,
    },

    /// Install a local plugin and sync source changes until stopped
    Dev {
        /// Local plugin development directory
        #[arg(value_name = "PATH", default_value = ".")]
        path: PathBuf,
    },

    /// List installed plugins and their source revisions
    List,

    /// Show one plugin or the complete plugin inventory
    Status {
        /// Installed plugin ID; omit to show every plugin
        id: Option<String>,
    },

    /// Enable an installed plugin
    Enable {
        /// Installed plugin ID
        id: String,
    },

    /// Disable an installed plugin without removing it
    Disable {
        /// Installed plugin ID
        id: String,
    },

    /// Update an installed GitHub plugin
    Update {
        /// Installed plugin ID
        id: String,
        /// Accept the trusted-code warning without prompting
        #[arg(long)]
        yes: bool,
    },

    /// Remove an installed plugin
    #[command(visible_alias = "uninstall")]
    Remove {
        /// Installed plugin ID
        id: String,
        /// Remove without prompting
        #[arg(long)]
        yes: bool,
    },
}

fn main() {
    let cli = Cli::parse();

    let path = cli.working_directory.or(cli.path);
    if path.is_some() || cli.new_window || cli.new_tab {
        commands::open::run(path, cli.new_window, cli.new_tab);
        return;
    }

    match cli.action {
        Some(Action::Plugin { command }) => commands::plugins::run(command),
        Some(Action::Version) => commands::version::run(),
        Some(Action::Help) => commands::help::run(),
        Some(Action::ListFonts) => commands::list_fonts::run(),
        Some(Action::ListKeybinds) => commands::list_keybinds::run(),
        Some(Action::ListThemes) => commands::list_themes::run(),
        Some(Action::ListColors) => commands::list_colors::run(),
        Some(Action::ListActions) => commands::list_actions::run(),
        Some(Action::EditConfig) => {
            if let Err(error) = commands::edit_config::run() {
                eprintln!("Error: {error}");
                std::process::exit(1);
            }
        }
        Some(Action::ShowConfig) => commands::show_config::run(),
        Some(Action::ValidateConfig) => commands::validate_config::run(),
        Some(Action::PrettifyConfig) => commands::prettify_config::run(),
        Some(Action::Tui) => commands::tui::run(),
        Some(Action::Update) => commands::update::run(),
        Some(Action::ExportTheme {
            repo,
            slug,
            name,
            version,
            description,
            force,
        }) => commands::theme_repo::export_theme(repo, slug, name, version, description, force),
        Some(Action::ValidateThemeRepo { repo }) => commands::theme_repo::validate_theme_repo(repo),
        None => {
            // No subcommand: show help
            commands::help::run();
        }
    }
}
