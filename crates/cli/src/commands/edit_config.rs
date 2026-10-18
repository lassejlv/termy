#[cfg(any(target_os = "windows", test))]
use std::{ffi::OsStr, path::PathBuf};
use std::{ffi::OsString, path::Path, process::Command};
use termy_core::config_core::config_path;

struct EditorLauncher {
    program: OsString,
    args: Vec<OsString>,
}

impl EditorLauncher {
    fn new(program: impl Into<OsString>, args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

pub fn run() -> Result<(), String> {
    let Some(path) = config_path() else {
        return Err("Could not determine config directory".to_string());
    };

    if !path.exists() {
        if let Some(parent) = path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return Err(format!("Failed to create config directory: {e}"));
        }
        if let Err(e) = std::fs::write(&path, "") {
            return Err(format!("Failed to create config file: {e}"));
        }
    }

    println!("Opening {}", path.display());
    launch_editor(&path, std::env::var_os("EDITOR"))
}

fn launch_editor(path: &Path, editor: Option<OsString>) -> Result<(), String> {
    let launchers = editor_launchers(path, editor)?;
    try_launchers(&launchers, |launcher| {
        editor_command(launcher)
            .args(&launcher.args)
            .status()
            .map(|status| status.success())
    })
}

#[cfg(target_os = "windows")]
fn editor_command(launcher: &EditorLauncher) -> Command {
    // `Command::new` only finds `.exe` files on PATH, but editors such as VS
    // Code install a `code.cmd` shim. Given the resolved `.cmd`/`.bat` path,
    // std runs it through `cmd.exe /d /c` with batch-safe argument quoting.
    let program = resolve_windows_program(
        &launcher.program,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("PATHEXT").as_deref(),
    )
    .map_or_else(|| launcher.program.clone(), PathBuf::into_os_string);
    Command::new(program)
}

#[cfg(not(target_os = "windows"))]
fn editor_command(launcher: &EditorLauncher) -> Command {
    Command::new(&launcher.program)
}

/// Finds `program` the way `cmd.exe` does: on PATH (or as given when it has a
/// directory), trying each PATHEXT extension unless it already has one.
/// The current directory is never searched.
#[cfg(any(target_os = "windows", test))]
fn resolve_windows_program(
    program: &OsStr,
    path_env: Option<&OsStr>,
    pathext: Option<&OsStr>,
) -> Option<PathBuf> {
    let extensions: Vec<String> = pathext
        .and_then(OsStr::to_str)
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(str::trim)
        .filter(|extension| extension.starts_with('.') && extension.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    let program_path = Path::new(program);
    let has_runnable_extension = program_path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|known| known[1..].eq_ignore_ascii_case(extension))
        });
    // An extensionless file next to a shim (VS Code's `bin/code` is a shell
    // script) can't be run, so only accept the bare name when it is runnable.
    let find_in = |base: &Path| -> Option<PathBuf> {
        if has_runnable_extension && base.is_file() {
            return Some(base.to_path_buf());
        }
        extensions
            .iter()
            .map(|extension| {
                let mut candidate = base.as_os_str().to_os_string();
                candidate.push(extension);
                PathBuf::from(candidate)
            })
            .find(|candidate| candidate.is_file())
    };

    if program_path.is_absolute() || program_path.components().count() > 1 {
        return find_in(program_path);
    }
    std::env::split_paths(path_env?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| find_in(&dir.join(program_path)))
}

#[cfg(not(target_os = "windows"))]
fn split_editor_command(command: &str) -> Result<(String, Vec<String>), String> {
    let mut words = shlex::split(command)
        .ok_or("EDITOR has unmatched quotes or escapes")?
        .into_iter();
    let program = words.next().ok_or("EDITOR is empty")?;
    if program.is_empty() {
        return Err("EDITOR has an empty executable name".to_string());
    }
    Ok((program, words.collect()))
}

#[cfg(target_os = "windows")]
fn split_editor_command(command: &str) -> Result<(String, Vec<String>), String> {
    split_windows_editor_command(command)
}

/// Splits EDITOR with Windows rules: backslashes are path separators, not
/// escapes. The program is a double-quoted first token, else the longest
/// whitespace-separated prefix naming an existing file (an unquoted
/// `C:\Program Files\...` path), else the first word.
#[cfg(any(target_os = "windows", test))]
fn split_windows_editor_command(command: &str) -> Result<(String, Vec<String>), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("EDITOR is empty".to_string());
    }

    let (program, rest) = if let Some(quoted) = command.strip_prefix('"') {
        let end = quoted.find('"').ok_or("EDITOR has unmatched quotes")?;
        (&quoted[..end], &quoted[end + 1..])
    } else {
        let end = command
            .char_indices()
            .filter(|(_, c)| c.is_whitespace())
            .map(|(index, _)| index)
            .rev()
            .find(|&index| Path::new(&command[..index]).is_file())
            .or_else(|| command.find(char::is_whitespace))
            .unwrap_or(command.len());
        (&command[..end], &command[end..])
    };
    if program.is_empty() {
        return Err("EDITOR has an empty executable name".to_string());
    }

    Ok((program.to_string(), split_windows_arguments(rest)?))
}

/// Whitespace-separated words; double quotes group words and are removed.
#[cfg(any(target_os = "windows", test))]
fn split_windows_arguments(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for c in input.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if quoted {
        return Err("EDITOR has unmatched quotes".to_string());
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

fn editor_launchers(path: &Path, editor: Option<OsString>) -> Result<Vec<EditorLauncher>, String> {
    let path = path.as_os_str().to_os_string();
    let mut launchers = Vec::new();

    // Try $EDITOR first, then platform-specific fallbacks
    if let Some(editor) = editor {
        // Preserve literal executable paths, including spaces and non-UTF-8 paths.
        let launcher = if Path::new(&editor).is_file() {
            EditorLauncher::new(editor, vec![path.clone()])
        } else {
            let command = editor.to_str().ok_or("EDITOR is not valid UTF-8")?;
            let (program, arguments) = split_editor_command(command)?;
            let mut args: Vec<OsString> = arguments.into_iter().map(OsString::from).collect();
            args.push(path.clone());
            EditorLauncher::new(program, args)
        };
        launchers.push(launcher);
    }

    #[cfg(target_os = "macos")]
    launchers.push(EditorLauncher::new(
        "open",
        vec![OsString::from("-t"), path],
    ));

    #[cfg(target_os = "linux")]
    {
        launchers.push(EditorLauncher::new("xdg-open", vec![path.clone()]));
        for editor in ["nano", "vim", "vi"] {
            launchers.push(EditorLauncher::new(editor, vec![path.clone()]));
        }
    }

    #[cfg(target_os = "windows")]
    launchers.push(EditorLauncher::new("notepad", vec![path]));

    Ok(launchers)
}

fn try_launchers<F>(launchers: &[EditorLauncher], mut launch: F) -> Result<(), String>
where
    F: FnMut(&EditorLauncher) -> std::io::Result<bool>,
{
    let mut failures = Vec::with_capacity(launchers.len());
    for launcher in launchers {
        let name = launcher.program.to_string_lossy();
        match launch(launcher) {
            Ok(true) => return Ok(()),
            Ok(false) => failures.push(format!("{name} exited with an error")),
            Err(error) => failures.push(format!("failed to run {name}: {error}")),
        }
    }

    Err(format!(
        "Could not open the config file: {}",
        failures.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, io};

    // POSIX quoting; Windows splits EDITOR with its own rules (tested below).
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn editor_arguments_and_quoted_paths_are_preserved() {
        let path = Path::new("config with spaces.toml");
        for (command, program, args) in [
            (
                "code --wait",
                "code",
                vec!["--wait", "config with spaces.toml"],
            ),
            (
                "'/opt/My Editor/bin/editor' --wait",
                "/opt/My Editor/bin/editor",
                vec!["--wait", "config with spaces.toml"],
            ),
            (
                "editor '$HOME; echo nope'",
                "editor",
                vec!["$HOME; echo nope", "config with spaces.toml"],
            ),
        ] {
            let launchers = editor_launchers(path, Some(command.into())).unwrap();
            assert_eq!(launchers[0].program, OsString::from(program));
            assert_eq!(
                launchers[0].args,
                args.into_iter().map(OsString::from).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn literal_editor_path_with_spaces_still_works() {
        let temp = tempfile::tempdir().unwrap();
        let editor = temp.path().join("My Editor");
        std::fs::write(&editor, b"").unwrap();
        let launchers =
            editor_launchers(Path::new("config"), Some(editor.clone().into_os_string())).unwrap();
        assert_eq!(launchers[0].program, editor.into_os_string());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn malformed_editor_commands_are_reported() {
        for command in ["", "''", "editor 'unterminated"] {
            assert!(editor_launchers(Path::new("config"), Some(command.into())).is_err());
        }
    }

    #[test]
    fn windows_editor_split_keeps_backslashes_and_quotes() {
        for (command, program, args) in [
            (r"C:\tools\nvim.exe -p", r"C:\tools\nvim.exe", vec!["-p"]),
            ("code --wait", "code", vec!["--wait"]),
            (
                r#""C:\Program Files\My Editor\ed.exe" --wait "two words" C:\dir\"#,
                r"C:\Program Files\My Editor\ed.exe",
                vec!["--wait", "two words", r"C:\dir\"],
            ),
            ("  notepad  ", "notepad", vec![]),
        ] {
            let (actual_program, actual_args) = split_windows_editor_command(command).unwrap();
            assert_eq!(actual_program, program, "{command}");
            assert_eq!(actual_args, args, "{command}");
        }
    }

    #[test]
    fn windows_editor_split_finds_unquoted_path_with_spaces() {
        let temp = tempfile::tempdir().unwrap();
        let editor = temp.path().join("My Editor.exe");
        std::fs::write(&editor, b"").unwrap();
        let command = format!("{} --wait", editor.display());

        let (program, args) = split_windows_editor_command(&command).unwrap();
        assert_eq!(Path::new(&program), editor);
        assert_eq!(args, ["--wait"]);
    }

    #[test]
    fn windows_editor_split_reports_malformed_commands() {
        for command in [
            "",
            "   ",
            r#""C:\unterminated"#,
            r#""" --wait"#,
            r#"ed "open"#,
        ] {
            assert!(split_windows_editor_command(command).is_err(), "{command}");
        }
    }

    #[test]
    fn windows_program_resolution_prefers_pathext_shims() {
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // VS Code ships an extensionless shell script next to its `.cmd` shim.
        std::fs::write(bin.join("code"), b"#!/bin/sh").unwrap();
        std::fs::write(bin.join("code.cmd"), b"@echo off").unwrap();
        let path_env = std::env::join_paths([temp.path().join("empty"), bin.clone()]).unwrap();
        let pathext = OsString::from(".COM;.EXE;.BAT;.CMD");
        let resolve = |program: &str| {
            resolve_windows_program(
                OsStr::new(program),
                Some(path_env.as_os_str()),
                Some(pathext.as_os_str()),
            )
        };

        assert_eq!(resolve("code"), Some(bin.join("code.cmd")));
        assert_eq!(resolve("code.cmd"), Some(bin.join("code.cmd")));
        assert_eq!(resolve("missing"), None);
        let absolute = bin.join("code");
        assert_eq!(
            resolve(absolute.to_str().unwrap()),
            Some(bin.join("code.cmd"))
        );
    }

    #[test]
    fn launcher_failures_fall_through_until_one_succeeds() {
        let launchers = vec![
            EditorLauncher::new("preferred", Vec::new()),
            EditorLauncher::new("fallback", Vec::new()),
        ];
        let mut outcomes = VecDeque::from([
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            Ok(true),
        ]);
        let mut attempted = Vec::new();

        let result = try_launchers(&launchers, |launcher| {
            attempted.push(launcher.program.clone());
            outcomes.pop_front().expect("launcher outcome")
        });

        assert_eq!(result, Ok(()));
        assert_eq!(
            attempted,
            [OsString::from("preferred"), OsString::from("fallback")]
        );
    }

    #[test]
    fn nonzero_exit_is_reported_when_no_launcher_succeeds() {
        let launchers = vec![
            EditorLauncher::new("preferred", Vec::new()),
            EditorLauncher::new("fallback", Vec::new()),
        ];

        let error = try_launchers(&launchers, |_| Ok(false))
            .expect_err("all unsuccessful launchers should fail");

        assert_eq!(
            error,
            "Could not open the config file: preferred exited with an error; fallback exited with an error"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launchers_prefer_editor_then_open() {
        let path = Path::new("config.toml");
        let launchers = editor_launchers(path, Some(OsString::from("nvim"))).unwrap();

        assert_eq!(launchers.len(), 2);
        assert_eq!(launchers[0].program, OsString::from("nvim"));
        assert_eq!(launchers[0].args, [OsString::from("config.toml")]);
        assert_eq!(launchers[1].program, OsString::from("open"));
        assert_eq!(
            launchers[1].args,
            [OsString::from("-t"), OsString::from("config.toml")]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_launchers_use_open_when_editor_is_unset() {
        let path = Path::new("config.toml");
        let launchers = editor_launchers(path, None).unwrap();

        assert_eq!(launchers.len(), 1);
        assert_eq!(launchers[0].program, OsString::from("open"));
        assert_eq!(
            launchers[0].args,
            [OsString::from("-t"), OsString::from("config.toml")]
        );
    }
}
