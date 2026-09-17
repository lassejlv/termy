use super::*;

#[gpui::test]
fn new_tab_inherits_cwd_after_shell_cd_with_foreground_app(cx: &mut gpui::TestAppContext) {
    let root = tempfile::tempdir().unwrap();
    let initial = root.path().join("initial");
    let project = root.path().join("project with spaces");
    let ready = root.path().join("ready");
    std::fs::create_dir(&initial).unwrap();
    std::fs::create_dir(&project).unwrap();
    let initial = initial.canonicalize().unwrap();
    let project = project.canonicalize().unwrap();
    let config = AppConfig {
        tmux_enabled: false,
        shell: Some("/bin/sh".into()),
        ..Default::default()
    };
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TerminalView::new_for_window(window, cx, config, true);
        view.owns_persisted_session = false;
        let terminal = Terminal::new_native_with_launch(
            TerminalSize::default(),
            initial.to_str(),
            None,
            None,
            None,
            Some(&TerminalLaunch::Program {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "read go; cd \"$1\" || exit; sleep 30 & app=$!; touch \"$2\"; wait \"$app\""
                        .into(),
                    "termy-cwd-test".into(),
                    project.to_string_lossy().into_owned(),
                    ready.to_string_lossy().into_owned(),
                ],
            }),
            None,
        )
        .unwrap();
        view.session
            .tabs
            .push(TerminalView::create_native_tab(1, terminal, 80, 24, None));
        view
    });
    view.update_in(cx, |view, _, cx| {
        // Prime the old process-cwd cache before the shell changes directories.
        assert_eq!(
            view.preferred_working_dir_for_new_session(None, cx)
                .as_deref(),
            initial.to_str()
        );
        view.session.tabs[0]
            .active_terminal()
            .unwrap()
            .write_input(b"go\n");
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(
            Instant::now() < deadline,
            "foreground process did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    view.update_in(cx, |view, _, cx| {
        let tab = &mut view.session.tabs[0];
        tab.running_process = true;
        tab.shell_title = Some("lazygit".into());
        assert_eq!(
            view.preferred_working_dir_for_new_session(None, cx)
                .as_deref(),
            project.to_str()
        );
        assert!(view.add_tab_with_working_dir(None, cx));
        let pid = view.session.tabs[view.session.active_tab]
            .active_terminal()
            .unwrap()
            .child_pid()
            .unwrap();
        assert_eq!(
            TerminalView::working_dir_for_child_pid_blocking(pid).as_deref(),
            project.to_str()
        );
    });
}

#[gpui::test]
// #388: exercised by just test-tmux-integration on macOS and Linux CI.
#[ignore = "requires tmux >= 3.3"]
fn new_tmux_tab_inherits_live_pane_cwd_with_foreground_app(cx: &mut gpui::TestAppContext) {
    use crate::terminal_ui::{TmuxClient, TmuxLaunchTarget, TmuxRuntimeConfig, TmuxSocketTarget};
    struct Server(String);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = Command::new("tmux")
                .args(["-L", &self.0, "kill-server"])
                .output();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let initial = root.path().canonicalize().unwrap();
    let project = initial.join("project with spaces");
    std::fs::create_dir(&project).unwrap();
    let server = Server(format!("termy-388-{}", std::process::id()));
    assert!(
        Command::new("tmux")
            .args([
                "-L",
                &server.0,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "cwd-test",
                "-c"
            ])
            .arg(&initial)
            .arg("/bin/sh")
            .status()
            .unwrap()
            .success()
    );
    let config = TmuxRuntimeConfig {
        launch: TmuxLaunchTarget::Session {
            name: "cwd-test".into(),
            socket: TmuxSocketTarget::Named(server.0.clone()),
        },
        ..Default::default()
    };
    let client = TmuxClient::new(config.clone(), 80, 24, None, None).unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| {
        let mut view = TerminalView::new_for_window(
            window,
            cx,
            AppConfig {
                tmux_enabled: false,
                ..Default::default()
            },
            true,
        );
        view.owns_persisted_session = false;
        view.runtime = RuntimeState::Tmux(TmuxRuntime::new(config, client, None, 80, 24));
        assert!(view.refresh_tmux_snapshot());
        view
    });
    view.update_in(cx, |view, _, cx| {
        assert_eq!(
            view.preferred_working_dir_for_new_session(None, cx)
                .as_deref(),
            initial.to_str()
        );
        let pane = view.session.tabs[view.session.active_tab]
            .active_pane_id()
            .unwrap()
            .to_string();
        view.tmux_runtime()
            .client
            .send_input(
                &pane,
                format!("cd '{}'; sleep 30\n", project.display()).as_bytes(),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = view.tmux_runtime().client.refresh_snapshot().unwrap();
            if snapshot
                .windows
                .iter()
                .flat_map(|window| &window.panes)
                .any(|p| {
                    p.id == pane
                        && p.current_command == "sleep"
                        && p.current_path == project.to_str().unwrap()
                })
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "foreground process did not start"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // Deliberately leave the UI's last prompt and runtime snapshot stale.
        let tab = &mut view.session.tabs[view.session.active_tab];
        tab.last_prompt_cwd = Some(initial.to_string_lossy().into_owned());
        tab.shell_title = Some("lazygit".into());
        tab.running_process = true;
        assert_eq!(
            view.preferred_working_dir_for_new_session(initial.to_str(), cx)
                .as_deref(),
            initial.to_str()
        );
        assert_eq!(
            view.preferred_working_dir_for_new_session(None, cx)
                .as_deref(),
            project.to_str()
        );
        assert!(view.tmux_add_tab(None, cx));
        let snapshot = view.tmux_runtime().client.refresh_snapshot().unwrap();
        let new_pane = view.session.tabs[view.session.active_tab]
            .active_pane_id()
            .unwrap();
        assert_ne!(new_pane, pane);
        assert_eq!(
            TerminalView::tmux_pane_working_dir(&snapshot, new_pane).as_deref(),
            project.to_str()
        );
    });
}
