use termy_core::release_core::{UpdateCheck, check_for_updates};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run() {
    println!("Checking for updates...");
    println!();

    match check_for_updates(CURRENT_VERSION) {
        Ok(UpdateCheck::UpToDate) => {
            println!("You're up to date! (v{CURRENT_VERSION})");
        }
        Ok(UpdateCheck::UpdateAvailable(release)) => {
            println!("Update available!");
            println!();
            println!("  Current version: v{CURRENT_VERSION}");
            println!("  Latest version:  v{}", release.version);
            println!();
            println!("Download at: {}", release.release_url);
            println!();
            println!("Or update via the Termy app: Command Palette > Check for Updates");
        }
        Err(error) => {
            eprintln!("Failed to check for updates: {error}");
        }
    }
}
