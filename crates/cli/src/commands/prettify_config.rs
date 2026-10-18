use std::io::Write;
use std::path::{Path, PathBuf};

use termy_core::config_core::{config_path, prettify_config_contents};

pub fn run() {
    let Some(path) = config_path() else {
        eprintln!("Could not determine config directory");
        std::process::exit(1);
    };

    if !path.exists() {
        println!("Config file does not exist yet: {}", path.display());
        println!("Nothing to prettify.");
        return;
    }

    match prettify_config_file(&path) {
        Ok(Outcome::Unchanged) => {
            println!("Config file is already prettified: {}", path.display());
        }
        Ok(Outcome::Rewritten { backup, prettified }) => {
            println!("Config file prettified: {}", path.display());
            println!("Backup of the previous file: {}", backup.display());
            println!();
            print!("{prettified}");
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug)]
enum Outcome {
    Unchanged,
    Rewritten { backup: PathBuf, prettified: String },
}

fn prettify_config_file(path: &Path) -> Result<Outcome, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("Failed to read config file: {error}"))?;

    let malformed = malformed_lines(&contents);
    if !malformed.is_empty() {
        let mut message = format!(
            "Refusing to prettify {}: these lines are not `key = value` or `[section]` and would be dropped:",
            path.display()
        );
        for (line_number, line) in malformed {
            message.push_str(&format!("\n  line {line_number}: {line}"));
        }
        return Err(message);
    }

    let prettified = prettify_config_contents(&contents);
    if prettified == contents {
        return Ok(Outcome::Unchanged);
    }

    // Prettifying drops comments, so keep the original next to the config.
    let mut backup_name = path.file_name().unwrap_or_default().to_os_string();
    backup_name.push(".bak");
    let backup = path.with_file_name(backup_name);
    write_atomically(&backup, &contents)
        .map_err(|error| format!("Failed to write backup {}: {error}", backup.display()))?;

    // Write through a symlinked config (e.g. into a dotfiles repo) instead of
    // replacing the link with a regular file.
    let target = dunce::canonicalize(path)
        .map_err(|error| format!("Failed to resolve config file: {error}"))?;
    write_atomically(&target, &prettified)
        .map_err(|error| format!("Failed to write config file: {error}"))?;

    Ok(Outcome::Rewritten { backup, prettified })
}

/// Lines `prettify_config_contents` silently discards that are not comments.
fn malformed_lines(contents: &str) -> Vec<(usize, &str)> {
    contents
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            let kept = trimmed.is_empty()
                || trimmed.starts_with('#')
                || (trimmed.starts_with('[') && trimmed.ends_with(']'))
                || trimmed.contains('=');
            (!kept).then_some((index + 1, trimmed))
        })
        .collect()
}

fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(contents.as_bytes())?;
    if let Ok(metadata) = std::fs::metadata(path) {
        temp.as_file().set_permissions(metadata.permissions())?;
    }
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Outcome, malformed_lines, prettify_config_file};

    #[test]
    fn malformed_lines_ignores_comments_sections_and_settings() {
        let contents = "# note\n\n[colors]\nforeground = #fff\noops\n  [broken\n";
        assert_eq!(malformed_lines(contents), vec![(5, "oops"), (6, "[broken")]);
    }

    #[test]
    fn already_prettified_config_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.txt");
        std::fs::write(&path, "theme = nord\n").unwrap();

        let outcome = prettify_config_file(&path).unwrap();

        assert!(matches!(outcome, Outcome::Unchanged));
        assert!(!temp.path().join("config.txt.bak").exists());
    }

    #[cfg(unix)]
    #[test]
    fn rewrite_keeps_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.txt");
        std::fs::write(&path, "font_size = 14\ntheme = nord\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        prettify_config_file(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
