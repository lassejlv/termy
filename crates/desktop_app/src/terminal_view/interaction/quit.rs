use super::*;
use gpui::PromptLevel;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseRequestTarget {
    Application,
    WindowClose,
    TabClose { tab_id: TabId },
    WorkspaceDelete { workspace_id: u64 },
}

impl TerminalView {
    fn close_request_is_blocked(
        &mut self,
        target: CloseRequestTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        match target {
            CloseRequestTarget::TabClose { tab_id } => {
                let blocked = self
                    .tab_index_by_id(tab_id)
                    .and_then(|index| self.session.tabs.get(index))
                    .is_some_and(|tab| tab.pinned);
                if blocked {
                    self.notify_pinned_tab_close_blocked(cx);
                }
                blocked
            }
            CloseRequestTarget::WorkspaceDelete { workspace_id } => {
                let Some(blocker) = self.workspace_delete_blocker(workspace_id) else {
                    return false;
                };
                match blocker {
                    workspaces::WorkspaceDeleteBlocker::Missing => {}
                    workspaces::WorkspaceDeleteBlocker::LastWorkspace => {
                        crate::ui::toast::info("The last workspace cannot be deleted");
                        self.notify_overlay(cx);
                    }
                    workspaces::WorkspaceDeleteBlocker::PinnedWorkspace => {
                        crate::ui::toast::info(
                            "Pinned workspaces must be unpinned before deleting",
                        );
                        self.notify_overlay(cx);
                    }
                    workspaces::WorkspaceDeleteBlocker::PinnedTab => {
                        self.notify_pinned_tab_close_blocked(cx);
                    }
                }
                true
            }
            CloseRequestTarget::Application | CloseRequestTarget::WindowClose => false,
        }
    }

    fn notify_pinned_tab_close_blocked(&mut self, cx: &mut Context<Self>) {
        crate::ui::toast::info("Pinned tabs must be unpinned before closing");
        self.notify_overlay(cx);
    }

    fn should_prompt_for_close_target(
        target: CloseRequestTarget,
        warn_on_quit: bool,
        warn_on_quit_with_running_process: bool,
        busy_tab_count: usize,
    ) -> bool {
        if busy_tab_count > 0 {
            return warn_on_quit_with_running_process;
        }

        matches!(
            target,
            CloseRequestTarget::Application | CloseRequestTarget::WindowClose
        ) && warn_on_quit
    }

    fn should_force_close_when_prompt_in_flight(target: CloseRequestTarget) -> bool {
        matches!(
            target,
            CloseRequestTarget::Application | CloseRequestTarget::WindowClose
        )
    }

    fn tab_close_request_target(tab_count: usize, tab_id: TabId) -> CloseRequestTarget {
        if tab_count <= 1 {
            CloseRequestTarget::WindowClose
        } else {
            CloseRequestTarget::TabClose { tab_id }
        }
    }

    pub(in super::super) fn execute_quit_command_action(
        &mut self,
        action: CommandAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::Quit => {
                self.request_application_quit(window, cx);
                true
            }
            CommandAction::RestartApp => {
                match self.restart_application_with_persist() {
                    Ok(()) => {
                        self.allow_quit_without_prompt = true;
                        cx.quit();
                    }
                    Err(error) => {
                        crate::ui::toast::error(format!("Restart failed: {error}"));
                        self.notify_overlay(cx);
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub(in super::super) fn restart_application_with_persist(&self) -> Result<(), String> {
        self.sync_persisted_native_workspace();
        self.restart_application()
    }

    fn restart_application(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;

        #[cfg(target_os = "macos")]
        {
            let app_bundle = exe
                .ancestors()
                .find(|path| {
                    path.extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
                })
                .map(PathBuf::from);

            if let Some(app_bundle) = app_bundle {
                let status = Command::new("open")
                    .arg("-n")
                    .arg(&app_bundle)
                    .status()
                    .map_err(|e| format!("failed to launch app bundle: {e}"))?;
                if status.success() {
                    return Ok(());
                }
                return Err(format!("open returned non-success status: {status}"));
            }
        }

        Command::new(&exe)
            .spawn()
            .map_err(|e| format!("failed to spawn executable: {e}"))?;
        Ok(())
    }

    pub(in super::super) fn tab_is_busy(tab: &TerminalTab) -> bool {
        tab.running_process
            || tab
                .panes
                .iter()
                .any(|pane| pane.terminal().alternate_screen_mode())
    }

    fn tab_title_for_warning(
        &self,
        index: usize,
        tab: &TerminalTab,
        fallback_title: &str,
    ) -> String {
        let title = tab.title.trim();
        if title.is_empty() {
            format!("{fallback_title} {}", index + 1)
        } else {
            title.to_string()
        }
    }

    fn busy_tab_titles_for_close_target(&self, target: CloseRequestTarget) -> Vec<String> {
        let fallback_title = self.fallback_title();
        match target {
            CloseRequestTarget::Application | CloseRequestTarget::WindowClose => {
                let mut titles = self
                    .session
                    .tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, tab)| Self::tab_is_busy(tab))
                    .map(|(index, tab)| self.tab_title_for_warning(index, tab, fallback_title))
                    .collect::<Vec<_>>();
                titles.extend(self.stashed_busy_workspace_tab_titles());
                titles
            }
            CloseRequestTarget::TabClose { tab_id } => self
                .session
                .tabs
                .iter()
                .enumerate()
                .find(|(_, tab)| tab.id == tab_id && Self::tab_is_busy(tab))
                .map(|(index, tab)| vec![self.tab_title_for_warning(index, tab, fallback_title)])
                .unwrap_or_default(),
            CloseRequestTarget::WorkspaceDelete { workspace_id } => {
                let Some(workspace_index) = self
                    .session
                    .workspaces
                    .iter()
                    .position(|entry| entry.id == workspace_id)
                else {
                    return Vec::new();
                };
                let tabs = if workspace_index == self.session.active_workspace {
                    self.session.tabs.as_slice()
                } else {
                    self.session.workspaces[workspace_index].tabs.as_slice()
                };
                tabs.iter()
                    .enumerate()
                    .filter(|(_, tab)| Self::tab_is_busy(tab))
                    .map(|(index, tab)| self.tab_title_for_warning(index, tab, fallback_title))
                    .collect()
            }
        }
    }

    fn close_warning_title(target: CloseRequestTarget) -> &'static str {
        match target {
            CloseRequestTarget::Application => "Quit Termy?",
            CloseRequestTarget::WindowClose => "Close Window?",
            CloseRequestTarget::TabClose { .. } => "Close Tab?",
            CloseRequestTarget::WorkspaceDelete { .. } => "Delete Workspace?",
        }
    }

    fn close_warning_buttons(target: CloseRequestTarget) -> &'static [&'static str] {
        match target {
            CloseRequestTarget::Application => &["Quit", "Cancel"],
            CloseRequestTarget::WindowClose => &["Close Window", "Cancel"],
            CloseRequestTarget::TabClose { .. } => &["Close Tab", "Cancel"],
            CloseRequestTarget::WorkspaceDelete { .. } => &["Delete Workspace", "Cancel"],
        }
    }

    fn close_warning_final_prompt(target: CloseRequestTarget) -> &'static str {
        match target {
            CloseRequestTarget::Application => "Quit anyway?",
            CloseRequestTarget::WindowClose => "Close this window anyway?",
            CloseRequestTarget::TabClose { .. } => "Close it anyway?",
            CloseRequestTarget::WorkspaceDelete { .. } => "Delete this workspace anyway?",
        }
    }

    fn close_warning_detail(target: CloseRequestTarget, busy_titles: &[String]) -> Option<String> {
        if busy_titles.is_empty() {
            return None;
        }

        if matches!(target, CloseRequestTarget::TabClose { .. }) {
            let mut detail =
                "This tab is running a command or fullscreen terminal app:\n".to_string();

            if let Some(title) = busy_titles.first() {
                detail.push_str("- ");
                detail.push_str(title);
                detail.push('\n');
            }

            detail.push_str("\nClose this tab anyway?");
            return Some(detail);
        }

        let count = busy_titles.len();
        let mut detail = format!(
            "{} tab{} {} running a command or fullscreen terminal app:\n",
            count,
            if count == 1 { "" } else { "s" },
            if count == 1 { "has" } else { "have" },
        );

        for title in busy_titles {
            detail.push_str("- ");
            detail.push_str(title);
            detail.push('\n');
        }

        detail.push('\n');
        detail.push_str(Self::close_warning_final_prompt(target));
        Some(detail)
    }

    fn close_tab_by_id(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        if let Some(index) = self.tab_index_by_id(tab_id) {
            self.close_tab(index, cx);
        }
    }

    fn follow_through_close_request(
        &mut self,
        target: CloseRequestTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        match target {
            CloseRequestTarget::Application => {
                self.sync_persisted_native_workspace();
                for handle in cx.windows() {
                    if handle != self.window_handle
                        && let Some(handle) = handle.downcast::<Self>()
                    {
                        let _ = handle.update(cx, |view, _, _| {
                            view.sync_persisted_native_workspace();
                        });
                    }
                }
                self.allow_quit_without_prompt = true;
                cx.quit();
                false
            }
            CloseRequestTarget::WindowClose => {
                self.sync_persisted_native_workspace();
                true
            }
            CloseRequestTarget::TabClose { tab_id } => {
                self.close_tab_by_id(tab_id, cx);
                false
            }
            CloseRequestTarget::WorkspaceDelete { workspace_id } => {
                if !self.close_request_is_blocked(target, cx) {
                    let _ = self.delete_workspace_by_id(workspace_id, cx);
                }
                false
            }
        }
    }

    fn request_close(
        &mut self,
        target: CloseRequestTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.quit_prompt_in_flight {
            if Self::should_force_close_when_prompt_in_flight(target) {
                // If the quit confirm prompt is unresponsive, allow a second
                // close request to follow through.
                match target {
                    CloseRequestTarget::Application => {
                        self.allow_quit_without_prompt = true;
                        cx.quit();
                    }
                    CloseRequestTarget::WindowClose => {
                        self.allow_quit_without_prompt = true;
                        return true;
                    }
                    CloseRequestTarget::TabClose { .. }
                    | CloseRequestTarget::WorkspaceDelete { .. } => {}
                }
            }
            return false;
        }

        if self.close_request_is_blocked(target, cx) {
            return false;
        }

        let mut busy_titles = self.busy_tab_titles_for_close_target(target);
        if target == CloseRequestTarget::Application {
            for handle in cx.windows() {
                if handle != self.window_handle
                    && let Some(handle) = handle.downcast::<Self>()
                    && let Ok(titles) = handle.update(cx, |view, _, _| {
                        view.busy_tab_titles_for_close_target(target)
                    })
                {
                    busy_titles.extend(titles);
                }
            }
        }
        if !Self::should_prompt_for_close_target(
            target,
            self.warn_on_quit,
            self.warn_on_quit_with_running_process,
            busy_titles.len(),
        ) {
            return self.follow_through_close_request(target, cx);
        }

        self.quit_prompt_in_flight = true;
        let detail = Self::close_warning_detail(target, &busy_titles);
        let prompt = window.prompt(
            PromptLevel::Warning,
            Self::close_warning_title(target),
            detail.as_deref(),
            Self::close_warning_buttons(target),
            cx,
        );
        let window_handle = window.window_handle();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let confirmed = matches!(prompt.await, Ok(0));
            let _ = cx.update(|cx| {
                let mut follow_through = false;
                if this
                    .update(cx, |view, _| {
                        view.quit_prompt_in_flight = false;
                        if confirmed {
                            if matches!(
                                target,
                                CloseRequestTarget::Application | CloseRequestTarget::WindowClose
                            ) {
                                view.allow_quit_without_prompt = true;
                            }
                            follow_through = true;
                        }
                    })
                    .is_err()
                {
                    return;
                }

                if !follow_through {
                    return;
                }

                match target {
                    CloseRequestTarget::Application => {
                        let _ = this.update(cx, |view, cx| {
                            view.follow_through_close_request(target, cx);
                        });
                    }
                    CloseRequestTarget::WindowClose => {
                        let _ = this.update(cx, |view, cx| {
                            view.follow_through_close_request(target, cx);
                        });
                        let _ = window_handle.update(cx, |_, window, _| window.remove_window());
                    }
                    CloseRequestTarget::TabClose { tab_id } => {
                        let _ = this.update(cx, |view, cx| view.close_tab_by_id(tab_id, cx));
                    }
                    CloseRequestTarget::WorkspaceDelete { workspace_id } => {
                        let _ = this.update(cx, |view, cx| {
                            if !view.close_request_is_blocked(target, cx) {
                                let _ = view.delete_workspace_by_id(workspace_id, cx);
                            }
                        });
                    }
                }
            });
        })
        .detach();

        false
    }

    pub(crate) fn handle_window_should_close_request(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.allow_quit_without_prompt {
            self.sync_persisted_native_workspace();
            self.finish_benchmark_session();
            self.allow_quit_without_prompt = false;
            return true;
        }

        self.request_close(CloseRequestTarget::WindowClose, window, cx)
    }

    pub(in super::super) fn request_application_quit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.request_close(CloseRequestTarget::Application, window, cx);
    }

    pub(in super::super) fn request_active_tab_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.active_tab >= self.session.tabs.len() {
            return;
        }
        let tab_id = self.session.tabs[self.session.active_tab].id;
        let target = Self::tab_close_request_target(self.effective_tab_count_for_close(), tab_id);
        let _ = self.request_close(target, window, cx);
    }

    /// Tab count used to pick a close target: stashed workspaces keep the
    /// window alive, so closing the visible strip's last tab is still a tab
    /// close (the workspace folds) rather than a window close.
    fn effective_tab_count_for_close(&self) -> usize {
        if self.has_other_workspaces() {
            self.session.tabs.len().saturating_add(1)
        } else {
            self.session.tabs.len()
        }
    }

    pub(in super::super) fn request_tab_close_by_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.session.tabs.len() {
            return;
        }
        let tab_id = self.session.tabs[index].id;
        let target = Self::tab_close_request_target(self.effective_tab_count_for_close(), tab_id);
        let _ = self.request_close(target, window, cx);
    }

    pub(in super::super) fn request_workspace_delete_by_id(
        &mut self,
        workspace_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.request_close(
            CloseRequestTarget::WorkspaceDelete { workspace_id },
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{CloseRequestTarget, TerminalView};

    #[test]
    fn prompt_in_flight_force_close_policy_allows_app_and_window_targets() {
        assert!(TerminalView::should_force_close_when_prompt_in_flight(
            CloseRequestTarget::Application
        ));
        assert!(TerminalView::should_force_close_when_prompt_in_flight(
            CloseRequestTarget::WindowClose
        ));
        assert!(!TerminalView::should_force_close_when_prompt_in_flight(
            CloseRequestTarget::TabClose { tab_id: 1 }
        ));
        assert!(!TerminalView::should_force_close_when_prompt_in_flight(
            CloseRequestTarget::WorkspaceDelete { workspace_id: 1 }
        ));
    }

    #[test]
    fn tab_close_request_targets_window_for_last_tab() {
        assert_eq!(
            TerminalView::tab_close_request_target(1, 7),
            CloseRequestTarget::WindowClose
        );
        assert_eq!(
            TerminalView::tab_close_request_target(2, 7),
            CloseRequestTarget::TabClose { tab_id: 7 }
        );
    }

    #[test]
    fn always_warn_on_quit_only_prompts_for_app_or_window_close_when_not_busy() {
        assert!(TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::Application,
            true,
            false,
            0,
        ));
        assert!(TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::WindowClose,
            true,
            false,
            0,
        ));
        assert!(!TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::TabClose { tab_id: 1 },
            true,
            false,
            0,
        ));
        assert!(!TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::WorkspaceDelete { workspace_id: 1 },
            true,
            false,
            0,
        ));
    }

    #[test]
    fn running_process_warning_only_prompts_when_busy() {
        assert!(TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::Application,
            false,
            true,
            1,
        ));
        assert!(!TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::Application,
            false,
            true,
            0,
        ));
        assert!(TerminalView::should_prompt_for_close_target(
            CloseRequestTarget::WorkspaceDelete { workspace_id: 7 },
            false,
            true,
            1,
        ));
    }

    #[test]
    fn close_warning_detail_is_absent_for_always_warn_without_busy_tabs() {
        assert_eq!(
            TerminalView::close_warning_detail(CloseRequestTarget::Application, &[]),
            None
        );
    }

    #[test]
    fn workspace_delete_warning_uses_destructive_workspace_copy() {
        let target = CloseRequestTarget::WorkspaceDelete { workspace_id: 7 };
        assert_eq!(
            TerminalView::close_warning_title(target),
            "Delete Workspace?"
        );
        assert_eq!(
            TerminalView::close_warning_buttons(target),
            &["Delete Workspace", "Cancel"]
        );
        assert_eq!(
            TerminalView::close_warning_final_prompt(target),
            "Delete this workspace anyway?"
        );
    }

    #[test]
    fn window_close_warning_describes_only_the_current_window() {
        let target = CloseRequestTarget::WindowClose;
        assert_eq!(TerminalView::close_warning_title(target), "Close Window?");
        assert_eq!(
            TerminalView::close_warning_buttons(target),
            &["Close Window", "Cancel"]
        );
        assert_eq!(
            TerminalView::close_warning_final_prompt(target),
            "Close this window anyway?"
        );
    }
}
