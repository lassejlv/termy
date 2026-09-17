use termy_core::command_core::{
    KeybindDirective, KeybindLineRef, parse_keybind_directives_from_iter,
};
use termy_core::config_core::{AppConfig, ConfigDiagnosticKind, config_path};

pub fn run() {
    let Some(path) = config_path() else {
        eprintln!("Could not determine config directory");
        std::process::exit(1);
    };

    println!("Config file: {}", path.display());

    if !path.exists() {
        println!("Status: File does not exist (using defaults)");
        println!("Result: Valid");
        return;
    }

    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            println!("Status: Failed to read file");
            println!("Error: {e}");
            std::process::exit(1);
        }
    };

    let ValidationReport { errors, warnings } = validate_contents(&contents);

    // Print results
    if errors.is_empty() && warnings.is_empty() {
        println!("Status: Valid");
    } else {
        if !errors.is_empty() {
            println!();
            println!("Errors:");
            for error in &errors {
                println!("  {error}");
            }
        }

        if !warnings.is_empty() {
            println!();
            println!("Warnings:");
            for warning in &warnings {
                println!("  {warning}");
            }
        }

        println!();
        if errors.is_empty() {
            println!("Result: Valid (with warnings)");
        } else {
            println!("Result: Invalid");
            std::process::exit(1);
        }
    }
}

pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn validate_contents(contents: &str) -> ValidationReport {
    let report = AppConfig::from_contents_with_report(contents);
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    for diagnostic in report.diagnostics {
        let message = format!("Line {}: {}", diagnostic.line_number, diagnostic.message);
        match diagnostic.kind {
            ConfigDiagnosticKind::InvalidSyntax | ConfigDiagnosticKind::InvalidValue => {
                errors.push(message);
            }
            ConfigDiagnosticKind::UnknownSection
            | ConfigDiagnosticKind::UnknownRootKey
            | ConfigDiagnosticKind::UnknownColorKey
            | ConfigDiagnosticKind::DuplicateRootKey => {
                warnings.push(message);
            }
        }
    }

    warnings.extend(tmux_disabled_keybind_warnings(&report.config));

    ValidationReport { errors, warnings }
}

fn tmux_disabled_keybind_warnings(config: &AppConfig) -> Vec<String> {
    if config.tmux_enabled {
        return Vec::new();
    }

    let mut warnings = Vec::new();

    // Parse each keybind line independently so warnings keep the exact source line number.
    for line in &config.keybind_lines {
        let (directives, _parse_warnings) =
            parse_keybind_directives_from_iter(std::iter::once(KeybindLineRef {
                line_number: line.line_number,
                value: line.value.as_str(),
            }));

        for directive in directives {
            if let KeybindDirective::Bind { action, .. } = directive
                && action.is_tmux_only()
            {
                warnings.push(format!(
                    "Line {}: keybind action '{}' requires tmux_enabled=true (restart required)",
                    line.line_number,
                    action.config_name()
                ));
            }
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::validate_contents;

    #[test]
    fn mixed_case_root_keys_are_validated_case_insensitively() {
        let report = validate_contents(
            "Theme = termy\n\
             FoNt_SiZe = 13\n\
             CuRsOr_BlInK = true\n",
        );

        assert!(
            report.errors.is_empty(),
            "unexpected errors: {:?}",
            report.errors
        );
        assert!(
            report.warnings.is_empty(),
            "unexpected warnings: {:?}",
            report.warnings
        );
    }

    #[test]
    fn mixed_case_theme_key_parses_like_runtime_parser() {
        let report = validate_contents("THEME = custom-theme\n");

        assert!(report.errors.is_empty());
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn boolean_aliases_and_positive_font_size_follow_parser_rules() {
        let report = validate_contents(
            "cursor_blink = yes\n\
             background_blur = 0\n\
             font_size = 0\n",
        );

        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].contains("font_size"));
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn resize_keybind_does_not_warn_when_tmux_is_disabled() {
        let report = validate_contents(
            "tmux_enabled = false\n\
             keybind = secondary-alt-shift-left=resize_pane_left\n\
             keybind = secondary-c=copy\n",
        );

        assert!(
            report.errors.is_empty(),
            "unexpected errors: {:?}",
            report.errors
        );
        assert!(
            report.warnings.is_empty(),
            "unexpected warnings: {:?}",
            report.warnings
        );
        assert!(
            !report
                .warnings
                .iter()
                .any(|warning| warning.contains("copy")),
            "non-tmux keybind should not warn: {:?}",
            report.warnings
        );
    }

    #[test]
    fn resize_keybind_does_not_warn_when_tmux_is_enabled() {
        let report = validate_contents(
            "tmux_enabled = true\n\
             keybind = secondary-alt-shift-left=resize_pane_left\n",
        );

        assert!(
            report.errors.is_empty(),
            "unexpected errors: {:?}",
            report.errors
        );
        assert!(
            report.warnings.is_empty(),
            "unexpected warnings: {:?}",
            report.warnings
        );
    }
}
