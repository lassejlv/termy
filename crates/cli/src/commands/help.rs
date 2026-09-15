pub fn run() {
    println!("Usage:");
    println!();
    println!("  termy /path/to/project");
    println!("  termy --working-directory /path/to/project");
    println!();
    println!("If Termy is already running, those commands open a new tab.");
    println!("File managers use the same path via Open new Termy tab here.");
    println!();
    println!("Available commands:");
    println!();
    println!("  plugin            Install and manage plugins");
    println!("  -tui              Interactive TUI for all CLI features");
    println!("  -version          Show version information");
    println!("  -help             Show this help message");
    println!("  -list-fonts       List available monospace fonts");
    println!("  -list-keybinds    List all keybindings");
    println!("  -list-themes      List available themes");
    println!("  -list-colors      Show current theme colors");
    println!("  -list-actions     List available keybind actions");
    println!("  -edit-config      Open config file in editor");
    println!("  -show-config      Display current configuration");
    println!("  -validate-config  Validate configuration file");
    println!("  -prettify-config  Prettify config (removes comments, formats)");
    println!("  -update           Check for updates");
    println!("  -export-theme     Export current colors to a themes repo checkout");
    println!("  -validate-theme-repo");
    println!("                    Validate a themes repo checkout");
}
