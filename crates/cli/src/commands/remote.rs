//! `termy remote …` runs `termy-remote`, the Swift agent that serves Termy
//! Next's sessions to the Termy iPhone app through the end-to-end encrypted
//! relay (see `next/Sources/TermyRemoteAgent`).

use std::ffi::OsString;
use std::path::PathBuf;

const BINARY: &str = "termy-remote";

/// Where `termy-remote` may be: `TERMY_REMOTE_BIN`, next to this binary,
/// inside an installed Termy Next, then `PATH`.
fn candidates() -> Vec<PathBuf> {
    let mut paths = vec![];
    if let Some(path) = std::env::var_os("TERMY_REMOTE_BIN") {
        paths.push(PathBuf::from(path));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        paths.push(dir.join(BINARY));
    }
    let app = PathBuf::from("Termy Next.app/Contents/Helpers").join(BINARY);
    paths.push(PathBuf::from("/Applications").join(&app));
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Applications").join(&app));
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|dir| dir.join(BINARY)));
    }
    paths
}

pub fn run(args: Vec<OsString>) {
    let Some(binary) = candidates().into_iter().find(|path| path.is_file()) else {
        eprintln!(
            "termy remote needs termy-remote, which ships with Termy Next. Install Termy Next, or set TERMY_REMOTE_BIN."
        );
        std::process::exit(1);
    };
    let mut command = std::process::Command::new(&binary);
    command.args(&args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("couldn't run {}: {error}", binary.display());
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("couldn't run {}: {error}", binary.display());
                std::process::exit(1);
            }
        }
    }
}
