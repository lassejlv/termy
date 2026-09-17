use super::super::*;
use crate::terminal_ui::TmuxPaneState;
use std::path::Path;

impl TerminalView {
    pub(crate) fn fallback_title(&self) -> &str {
        let fallback = self.tab_title.fallback.trim();
        if fallback.is_empty() {
            DEFAULT_TAB_TITLE
        } else {
            fallback
        }
    }

    pub(crate) fn resolve_template(
        template: &str,
        cwd: Option<&str>,
        command: Option<&str>,
    ) -> String {
        const CWD_TOKEN: &str = "{cwd}";
        const COMMAND_TOKEN: &str = "{command}";

        let cwd = cwd.unwrap_or("");
        let command = command.unwrap_or("");
        let mut remaining = template;
        let mut resolved = String::with_capacity(template.len());

        while let Some(open_brace_idx) = remaining.find('{') {
            resolved.push_str(&remaining[..open_brace_idx]);
            remaining = &remaining[open_brace_idx..];
            if let Some(tail) = remaining.strip_prefix(CWD_TOKEN) {
                resolved.push_str(cwd);
                remaining = tail;
                continue;
            }
            if let Some(tail) = remaining.strip_prefix(COMMAND_TOKEN) {
                resolved.push_str(command);
                remaining = tail;
                continue;
            }

            resolved.push('{');
            remaining = &remaining['{'.len_utf8()..];
        }

        resolved.push_str(remaining);
        resolved
    }

    pub(crate) fn is_shell_command(command: &str) -> bool {
        let command = command.trim();
        if command.is_empty() {
            return false;
        }
        let Some(first_token) = command.split_whitespace().next() else {
            return false;
        };
        let token = Path::new(first_token)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(first_token)
            .trim_start_matches('-');
        if token.is_empty() {
            return false;
        }
        let mut normalized = token.to_ascii_lowercase();
        if let Some(stem) = normalized.strip_suffix(".exe") {
            normalized = stem.to_string();
        }

        matches!(
            normalized.as_str(),
            "bash"
                | "zsh"
                | "fish"
                | "sh"
                | "dash"
                | "ksh"
                | "tcsh"
                | "csh"
                | "nu"
                | "pwsh"
                | "powershell"
                | "cmd"
        )
    }

    fn display_cwd_for_tab_title(cwd: Option<&str>) -> Option<String> {
        let cwd = cwd.map(str::trim).filter(|cwd| !cwd.is_empty())?;
        Some(Self::display_working_directory_for_prompt(Path::new(cwd)))
    }

    fn resolve_prompt_title(template: &str, cwd: Option<&str>) -> String {
        let cwd = Self::display_cwd_for_tab_title(cwd);
        Self::resolve_template(template, cwd.as_deref(), None)
    }

    pub(crate) fn derive_tmux_shell_title(
        tab_title: &TabTitleConfig,
        pane: &TmuxPaneState,
    ) -> Option<String> {
        let cwd = pane.current_path.trim();
        let command = pane.current_command.trim();
        let resolved = if Self::is_shell_command(command) {
            Self::resolve_prompt_title(&tab_title.prompt_format, (!cwd.is_empty()).then_some(cwd))
        } else {
            Self::resolve_template(
                &tab_title.command_format,
                None,
                (!command.is_empty()).then_some(command),
            )
        };

        let resolved = resolved.trim();
        if resolved.is_empty() {
            return None;
        }

        Some(Self::truncate_tab_title(resolved))
    }

    pub(crate) fn should_seed_predicted_prompt_title(tab_title: &TabTitleConfig) -> bool {
        tab_title.priority.contains(&TabTitleSource::Explicit)
    }

    pub(crate) fn predicted_prompt_seed_title(
        tab_title: &TabTitleConfig,
        cwd: Option<&str>,
    ) -> Option<String> {
        if !Self::should_seed_predicted_prompt_title(tab_title) {
            return None;
        }

        let resolved = Self::resolve_prompt_title(&tab_title.prompt_format, cwd);
        let resolved = resolved.trim();
        if resolved.is_empty() {
            return None;
        }

        Some(Self::truncate_tab_title(resolved))
    }

    fn smart_mode_shell_fallback_enabled(tab_title: &TabTitleConfig) -> bool {
        tab_title.mode == termy_core::config_core::TabTitleMode::Smart
            && !tab_title.shell_integration
            && tab_title.priority.contains(&TabTitleSource::Explicit)
            && tab_title.priority.contains(&TabTitleSource::Shell)
    }

    /// Returns the title candidate for a single source in the priority list.
    ///
    /// The caller walks sources in priority order and uses the first non-empty
    /// candidate.  The `Explicit` source has two special deference modes that
    /// both yield to `shell_title` when available:
    ///
    /// - **Prediction**: the explicit title was pre-seeded at tab creation from
    ///   the cwd and has not yet been confirmed by a shell-integration event.
    /// - **Smart-mode shell fallback**: shell integration is disabled in smart
    ///   mode, so the shell's own title (set via terminal escape sequences) is
    ///   preferred once available.
    ///
    /// In both cases the explicit title is kept as a fallback so the tab is
    /// never blank.
    fn title_source_candidate<'a>(
        source: TabTitleSource,
        manual_title: Option<&'a str>,
        explicit_title: Option<&'a str>,
        explicit_title_is_prediction: bool,
        prediction_allows_shell: bool,
        shell_title: Option<&'a str>,
        fallback_title: &'a str,
        smart_mode_shell_fallback: bool,
    ) -> Option<&'a str> {
        match source {
            TabTitleSource::Manual => manual_title,
            // The explicit title is speculative—prefer a live shell title.
            TabTitleSource::Explicit if explicit_title_is_prediction && prediction_allows_shell => {
                shell_title.or(explicit_title)
            }
            TabTitleSource::Explicit if explicit_title_is_prediction => explicit_title,
            TabTitleSource::Explicit if smart_mode_shell_fallback => {
                // Smart mode seeds an explicit title before the shell emits a title.
                // When shell integration is disabled, prefer live shell titles once
                // available while keeping explicit as a fallback.
                shell_title.or(explicit_title)
            }
            TabTitleSource::Explicit => explicit_title,
            TabTitleSource::Shell => shell_title,
            TabTitleSource::Fallback => Some(fallback_title),
        }
    }

    fn parse_explicit_title(&self, title: &str) -> Option<ExplicitTitlePayload> {
        let prefix = self.tab_title.explicit_prefix.trim();
        if prefix.is_empty() {
            return None;
        }

        let payload = title.strip_prefix(prefix)?.trim();
        if payload.is_empty() {
            return None;
        }

        if let Some(prompt) = payload.strip_prefix("prompt:") {
            let prompt = prompt.trim();
            if prompt.is_empty() {
                return None;
            }
            return Some(ExplicitTitlePayload::Prompt {
                title: Self::resolve_prompt_title(&self.tab_title.prompt_format, Some(prompt)),
                cwd: prompt.to_string(),
            });
        }

        if let Some(command) = payload.strip_prefix("command:") {
            let command = command.trim();
            if command.is_empty() {
                return None;
            }
            return Some(ExplicitTitlePayload::Command {
                title: Self::resolve_template(&self.tab_title.command_format, None, Some(command)),
                command: command.to_string(),
            });
        }

        let explicit = payload.strip_prefix("title:").unwrap_or(payload).trim();
        if explicit.is_empty() {
            return None;
        }

        Some(ExplicitTitlePayload::Title(explicit.to_string()))
    }

    pub(crate) fn resolved_tab_title(&self, index: usize) -> String {
        let tab = &self.session.tabs[index];
        let fallback_title = self.fallback_title();
        let smart_mode_shell_fallback = Self::smart_mode_shell_fallback_enabled(&self.tab_title);
        let prediction_allows_shell = self.tab_title.priority.contains(&TabTitleSource::Shell);

        for source in &self.tab_title.priority {
            let candidate = Self::title_source_candidate(
                *source,
                tab.manual_title.as_deref(),
                tab.explicit_title.as_deref(),
                tab.explicit_title_is_prediction,
                prediction_allows_shell,
                tab.shell_title.as_deref(),
                fallback_title,
                smart_mode_shell_fallback,
            );

            if let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
                return Self::truncate_tab_title(candidate);
            }
        }

        Self::truncate_tab_title(fallback_title)
    }

    pub(crate) fn refresh_tab_title(&mut self, index: usize) -> bool {
        if index >= self.session.tabs.len() {
            return false;
        }

        let next = self.resolved_tab_title(index);
        if self.session.tabs[index].title == next {
            return false;
        }

        let previous = std::mem::replace(&mut self.session.tabs[index].title, next);
        let current_title = self.session.tabs[index].title.clone();
        self.invalidate_tab_title_width_cache_for_title(previous.as_str());
        self.invalidate_tab_title_width_cache_for_title(current_title.as_str());

        // Keep title-width behavior uniform across manual, shell, explicit, and fallback sources.
        self.session.tabs[index].sticky_title_width = 0.0;
        self.session.tabs[index].title_text_width = 0.0;
        self.mark_tab_strip_layout_dirty();
        true
    }

    pub(crate) fn apply_terminal_title(
        &mut self,
        index: usize,
        title: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let title = title.trim();
        if title.is_empty() || index >= self.session.tabs.len() {
            return false;
        }

        if let Some(explicit_payload) = self.parse_explicit_title(title) {
            return match explicit_payload {
                ExplicitTitlePayload::Prompt { title, cwd } => {
                    self.session.tabs[index].last_prompt_cwd = Some(cwd);
                    self.session.tabs[index].running_process = false;
                    self.session.tabs[index].current_command = None;
                    self.cancel_pending_command_title(index);
                    self.set_explicit_title(index, title)
                }
                ExplicitTitlePayload::Title(prompt_title) => {
                    self.session.tabs[index].current_command = None;
                    self.cancel_pending_command_title(index);
                    self.set_explicit_title(index, prompt_title)
                }
                ExplicitTitlePayload::Command { title, command } => {
                    self.session.tabs[index].running_process = true;
                    self.session.tabs[index].current_command = Some(command);
                    let tab_id = self.session.tabs[index].id;
                    self.schedule_delayed_command_title(tab_id, title, COMMAND_TITLE_DELAY_MS, cx);
                    false
                }
            };
        }

        let shell_title = Self::truncate_tab_title(title);
        if self.session.tabs[index].shell_title.as_deref() == Some(shell_title.as_str()) {
            return false;
        }

        self.session.tabs[index].shell_title = Some(shell_title);
        self.refresh_tab_title(index)
    }

    pub(crate) fn clear_terminal_titles(&mut self, index: usize) -> bool {
        if index >= self.session.tabs.len() {
            return false;
        }

        self.cancel_pending_command_title(index);
        let tab = &mut self.session.tabs[index];
        tab.running_process = false;
        tab.current_command = None;
        let had_shell = tab.shell_title.take().is_some();
        let had_explicit = tab.explicit_title.take().is_some();
        tab.explicit_title_is_prediction = false;
        if !had_shell && !had_explicit {
            return false;
        }

        self.refresh_tab_title(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TabTitleConfig, TabTitleSource};

    fn expected_home_relative_path(parts: &[&str]) -> String {
        if parts.is_empty() {
            return "~".to_string();
        }

        let separator = std::path::MAIN_SEPARATOR.to_string();
        format!("~{}{}", std::path::MAIN_SEPARATOR, parts.join(&separator))
    }

    fn pane_with(path: &str, command: &str) -> TmuxPaneState {
        TmuxPaneState {
            id: "%1".to_string(),
            window_id: "@1".to_string(),
            session_id: "$1".to_string(),
            is_active: true,
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            cursor_x: 0,
            cursor_y: 0,
            mouse_mode: TmuxPaneMouseMode::default(),
            current_path: path.to_string(),
            current_command: command.to_string(),
        }
    }

    #[test]
    fn predicted_prompt_seed_title_uses_cwd_template_when_explicit_is_enabled() {
        let config = TabTitleConfig::default();
        let title = TerminalView::predicted_prompt_seed_title(&config, Some("~/projects/termy"));
        assert_eq!(title.as_deref(), Some("~/projects/termy"));
    }

    #[test]
    fn predicted_prompt_seed_title_formats_absolute_home_path_relative() {
        let config = TabTitleConfig::default();
        let home = TerminalView::user_home_dir().expect("home dir");
        let cwd = home.join("projects").join("termy");
        let expected = expected_home_relative_path(&["projects", "termy"]);
        let title = TerminalView::predicted_prompt_seed_title(
            &config,
            Some(cwd.to_string_lossy().as_ref()),
        );

        assert_eq!(title.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn predicted_prompt_seed_title_skips_static_only_priority() {
        let config = TabTitleConfig {
            priority: vec![TabTitleSource::Manual, TabTitleSource::Fallback],
            ..Default::default()
        };

        let title = TerminalView::predicted_prompt_seed_title(&config, Some("~/projects/termy"));
        assert!(title.is_none());
    }

    #[test]
    fn predicted_prompt_seed_title_ignores_empty_resolved_output() {
        let config = TabTitleConfig {
            prompt_format: "{cwd}".to_string(),
            ..Default::default()
        };

        let title = TerminalView::predicted_prompt_seed_title(&config, None);
        assert!(title.is_none());
    }

    #[test]
    fn smart_mode_shell_fallback_enabled_when_shell_integration_is_off() {
        let config = TabTitleConfig {
            shell_integration: false,
            ..Default::default()
        };
        assert!(TerminalView::smart_mode_shell_fallback_enabled(&config));
    }

    #[test]
    fn smart_mode_shell_fallback_disabled_when_shell_integration_is_on() {
        let config = TabTitleConfig::default();
        assert!(!TerminalView::smart_mode_shell_fallback_enabled(&config));
    }

    #[test]
    fn smart_mode_shell_fallback_disabled_for_non_smart_mode() {
        let config = TabTitleConfig {
            mode: termy_core::config_core::TabTitleMode::Shell,
            shell_integration: false,
            ..Default::default()
        };
        assert!(!TerminalView::smart_mode_shell_fallback_enabled(&config));
    }

    #[test]
    fn title_source_candidate_prefers_shell_when_smart_shell_fallback_is_enabled() {
        let candidate = TerminalView::title_source_candidate(
            TabTitleSource::Explicit,
            None,
            Some("explicit"),
            false,
            false,
            Some("shell"),
            "fallback",
            true,
        );
        assert_eq!(candidate, Some("shell"));
    }

    #[test]
    fn title_source_candidate_uses_explicit_when_shell_is_unavailable() {
        let candidate = TerminalView::title_source_candidate(
            TabTitleSource::Explicit,
            None,
            Some("explicit"),
            false,
            false,
            None,
            "fallback",
            true,
        );
        assert_eq!(candidate, Some("explicit"));
    }

    #[test]
    fn title_source_candidate_prefers_shell_over_predicted_explicit_title() {
        let candidate = TerminalView::title_source_candidate(
            TabTitleSource::Explicit,
            None,
            Some("predicted"),
            true,
            true,
            Some("shell"),
            "fallback",
            false,
        );
        assert_eq!(candidate, Some("shell"));
    }

    #[test]
    fn title_source_candidate_keeps_predicted_explicit_when_shell_source_is_disabled() {
        let candidate = TerminalView::title_source_candidate(
            TabTitleSource::Explicit,
            None,
            Some("predicted"),
            true,
            false,
            Some("shell"),
            "fallback",
            false,
        );
        assert_eq!(candidate, Some("predicted"));
    }

    #[test]
    fn resolve_template_replaces_known_tokens_in_single_pass() {
        let resolved = TerminalView::resolve_template(
            "cwd={cwd} command={command}",
            Some("{command}"),
            Some("{cwd}"),
        );
        assert_eq!(resolved, "cwd={command} command={cwd}");
    }

    #[test]
    fn resolve_template_leaves_unknown_brace_tokens_unchanged() {
        let resolved =
            TerminalView::resolve_template("start {unknown} end", Some("cwd"), Some("cmd"));
        assert_eq!(resolved, "start {unknown} end");
    }

    #[test]
    fn resolve_prompt_title_formats_absolute_home_paths_relative() {
        let home = TerminalView::user_home_dir().expect("home dir");
        let cwd = home.join("projects").join("termy");
        let expected = expected_home_relative_path(&["projects", "termy"]);

        assert_eq!(
            TerminalView::resolve_prompt_title("{cwd}", Some(cwd.to_string_lossy().as_ref())),
            expected
        );
    }

    #[test]
    fn resolve_prompt_title_leaves_non_home_absolute_paths_unchanged() {
        assert_eq!(
            TerminalView::resolve_prompt_title("{cwd}", Some("/tmp/work")),
            "/tmp/work"
        );
    }

    #[test]
    fn is_shell_command_matches_fixed_shell_set_case_insensitively() {
        assert!(TerminalView::is_shell_command("zsh"));
        assert!(TerminalView::is_shell_command("PwSh"));
        assert!(TerminalView::is_shell_command("/bin/bash"));
        assert!(TerminalView::is_shell_command("-zsh"));
        assert!(TerminalView::is_shell_command("pwsh.exe"));
        assert!(TerminalView::is_shell_command("bash -l"));
        assert!(TerminalView::is_shell_command("cmd"));
        assert!(!TerminalView::is_shell_command("sleep"));
        assert!(!TerminalView::is_shell_command(""));
    }

    #[test]
    fn derive_tmux_shell_title_uses_prompt_format_for_shell_commands() {
        let tab_title = TabTitleConfig {
            prompt_format: "cwd:{cwd}".to_string(),
            ..Default::default()
        };
        let pane = pane_with("/tmp/work", "zsh");

        let title = TerminalView::derive_tmux_shell_title(&tab_title, &pane);
        assert_eq!(title.as_deref(), Some("cwd:/tmp/work"));
    }

    #[test]
    fn derive_tmux_shell_title_formats_home_paths_relative() {
        let tab_title = TabTitleConfig {
            prompt_format: "cwd:{cwd}".to_string(),
            ..Default::default()
        };
        let home = TerminalView::user_home_dir().expect("home dir");
        let pane = pane_with(
            home.join("work").join("project").to_string_lossy().as_ref(),
            "zsh",
        );
        let expected = format!("cwd:{}", expected_home_relative_path(&["work", "project"]));

        let title = TerminalView::derive_tmux_shell_title(&tab_title, &pane);
        assert_eq!(title.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn derive_tmux_shell_title_uses_command_format_for_non_shell_commands() {
        let tab_title = TabTitleConfig {
            command_format: "run:{command}".to_string(),
            ..Default::default()
        };
        let pane = pane_with("/tmp/work", "sleep");

        let title = TerminalView::derive_tmux_shell_title(&tab_title, &pane);
        assert_eq!(title.as_deref(), Some("run:sleep"));
    }

    #[test]
    fn derive_tmux_shell_title_returns_none_when_resolved_title_is_empty() {
        let tab_title = TabTitleConfig {
            command_format: " ".to_string(),
            ..Default::default()
        };
        let pane = pane_with("/tmp/work", "sleep");

        let title = TerminalView::derive_tmux_shell_title(&tab_title, &pane);
        assert!(title.is_none());
    }
}
