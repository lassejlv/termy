use super::*;
use gpui::{AppContext, TestAppContext, WindowHandle, WindowOptions};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
};
use termy_core::multiplexer::SessionClient;

const PREFIX: &str = "terminal_view::multiplexer_session::tests::";

#[gpui::test]
fn multiplexer_is_off_by_default_and_config_changes_require_restart(cx: &mut TestAppContext) {
    let mut config = AppConfig::default();
    assert!(!config.multiplexer_enabled);
    cx.update(|cx| {
        crate::multiplexer::initialize(&config, cx).unwrap();
        assert!(!crate::multiplexer::enabled(cx));
        assert!(crate::multiplexer::claim_window(cx, true).is_none());
        config.multiplexer_enabled = true;
        crate::multiplexer::initialize(&config, cx).unwrap();
        assert!(!crate::multiplexer::enabled(cx));
    });
}

fn test_config() -> AppConfig {
    AppConfig {
        multiplexer_enabled: true,
        // The built-in host must take precedence over legacy tmux config.
        tmux_enabled: true,
        native_tab_persistence: false,
        native_layout_autosave: false,
        auto_update: false,
        shell: Some("/bin/sh".into()),
        warn_on_quit: false,
        warn_on_quit_with_running_process: false,
        ..AppConfig::default()
    }
}

fn window(cx: &mut TestAppContext, empty: bool) -> WindowHandle<TerminalView> {
    cx.update(|cx| {
        cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| TerminalView::new_for_window(window, cx, test_config(), empty))
        })
        .unwrap()
    })
}

fn update<R>(
    cx: &mut TestAppContext,
    handle: WindowHandle<TerminalView>,
    f: impl FnOnce(&mut TerminalView, &mut Context<TerminalView>) -> R,
) -> R {
    cx.update(|cx| handle.update(cx, |view, _, cx| f(view, cx)).unwrap())
}

#[track_caller]
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < end, "session did not reach expected state");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn launch(root: &Path, name: &str) -> TerminalLaunch {
    TerminalLaunch::Program {
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            r#"stty -echo
(while :; do printf x >> "$1/$2.ticks"; sleep 0.05; done) &
ticker=$!
trap 'kill "$ticker" 2>/dev/null' EXIT HUP TERM
printf 'MAIN\r\n\033[?1049h\033[2J\033[H\033[?2004hREADY:%s\r\n' "$2"
count=0
while IFS= read -r value; do
    count=$((count + 1))
    printf 'count=%s:%s\r\n' "$count" "$value"
done
"#
            .into(),
            "mux-test".into(),
            root.display().to_string(),
            name.into(),
        ],
    }
}

fn core_text(view: &TerminalView) -> String {
    let Terminal::Native(native) = view.active_terminal().unwrap() else {
        panic!("native runtime required");
    };
    native
        .terminal
        .lock()
        .unwrap()
        .render_read(true)
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect()
}

fn ids(client: &SessionClient) -> Vec<(String, Option<u32>)> {
    let mut ids: Vec<_> = client
        .list()
        .unwrap()
        .into_iter()
        .map(|pane| (pane.id, pane.child_pid))
        .collect();
    ids.sort();
    ids
}

// This is invoked as a separate OS process by the integration test below.
#[test]
fn host_process() {
    if let Ok(root) = std::env::var("TERMY_DESKTOP_MUX_ROOT") {
        termy_core::multiplexer::serve(Path::new(&root)).unwrap();
    }
}

#[gpui::test]
fn desktop_client_process(cx: &mut TestAppContext) {
    let Ok(root) = std::env::var("TERMY_DESKTOP_MUX_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let client = SessionClient::connect(root).unwrap();
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let phase = std::env::var("TERMY_DESKTOP_MUX_PHASE").unwrap();
    let first = window(cx, phase == "create");
    if phase == "create" {
        update(cx, first, |view, cx| {
            assert_eq!(view.runtime_kind(), RuntimeKind::Native);
            assert!(view.add_tab_with_launch(None, Some(&launch(root, "left")), cx));
            assert!(view.split_active_pane_vertical_with_launch(
                None,
                Some(&launch(root, "right")),
                cx
            ));
        });
        wait(|| {
            cx.run_until_parked();
            update(cx, first, |view, _| view.session.tabs[0].panes.len() == 2)
        });
        wait(|| update(cx, first, |view, _| core_text(view).contains("READY:right")));
        update(cx, first, |view, cx| {
            view.active_terminal().unwrap().write_input(b"before\n");
            assert!(view.toggle_pane_zoom(cx));
            view.session.tabs[0].manual_title = Some("server".into());
            view.session.tabs[0].pinned = true;
            view.refresh_tab_title(0);
            assert!(view.add_tab_with_launch(None, Some(&launch(root, "build")), cx));
            view.session.tabs[1].manual_title = Some("build".into());
            view.session.active_tab = 0;
            view.session.workspaces[0].name = "Work".into();
            view.session.workspaces[0].custom_named = true;
            view.add_workspace(cx);
            view.session.workspaces[1].name = "Docs".into();
            view.session.workspaces[1].custom_named = true;
            view.switch_workspace(0, cx);
        });
        wait(|| {
            update(cx, first, |view, _| {
                core_text(view).contains("count=1:before")
            })
        });
        let second = window(cx, true);
        update(cx, second, |view, cx| {
            assert!(view.add_tab_with_launch(None, Some(&launch(root, "window2")), cx));
            view.session.tabs[0].manual_title = Some("second window".into());
        });
        assert_eq!(ids(&client).len(), 5);
        fs::write(
            root.join("before.json"),
            serde_json::to_vec(&ids(&client)).unwrap(),
        )
        .unwrap();
        // Exercise the app-quit observer rather than calling detach directly.
        cx.update(|cx| cx.quit());
        cx.run_until_parked();
    } else if phase == "restore" {
        assert_eq!(cx.update(|cx| crate::multiplexer::pending_windows(cx)), 1);
        let second = window(cx, false);
        let before: Vec<(String, Option<u32>)> =
            serde_json::from_slice(&fs::read(root.join("before.json")).unwrap()).unwrap();
        assert_eq!(ids(&client), before);
        update(cx, first, |view, _| {
            assert_eq!(view.session.workspaces.len(), 2);
            assert_eq!(view.session.workspaces[0].name, "Work");
            assert_eq!(view.session.workspaces[1].name, "Docs");
            assert!(view.session.workspaces[1].pending_restore.is_some());
            assert_eq!(view.session.tabs.len(), 2);
            assert_eq!(view.session.active_tab, 0);
            assert_eq!(view.session.tabs[0].manual_title.as_deref(), Some("server"));
            assert!(view.session.tabs[0].pinned);
            assert_eq!(view.session.tabs[0].panes.len(), 1);
            assert_eq!(
                view.session.native_pane_zoom_snapshots[&view.session.tabs[0].id]
                    .other_panes
                    .len(),
                1
            );
            assert!(view.active_terminal().unwrap().alternate_screen_mode());
            assert!(core_text(view).contains("count=1:before"));
            view.active_terminal().unwrap().write_input(b"after\n");
        });
        wait(|| {
            update(cx, first, |view, _| {
                core_text(view).contains("count=2:after")
            })
        });
        update(cx, second, |view, _| {
            assert_eq!(
                view.session.tabs[0].manual_title.as_deref(),
                Some("second window")
            );
        });
        update(cx, first, |view, cx| {
            // Pending workspaces have no attached Terminal to drop.
            let docs = view.session.workspaces[1].id;
            assert!(view.delete_workspace_by_id(docs, cx));
            view.close_tab(1, cx);
            assert!(view.toggle_pane_zoom(cx));
            assert_eq!(view.session.tabs[0].panes.len(), 2);
            assert!(
                view.session
                    .native_pane_layout_trees
                    .contains_key(&view.session.tabs[0].id)
            );
        });
        assert_eq!(ids(&client).len(), 3);
        // Closing a window must save/detach it; closing the remaining app must
        // also retain its own processes. Release callbacks run during disposal.
        cx.update(|cx| {
            first
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        });
        cx.run_until_parked();
        assert_eq!(ids(&client).len(), 3);
        cx.update(|cx| cx.quit());
        cx.run_until_parked();
    } else if phase == "transfer" {
        let second = window(cx, false);
        let before = ids(&client);
        update(cx, first, |view, cx| {
            view.arm_window_tab_drag(0, gpui::point(px(1.0), px(1.0)), cx);
        });
        cx.update(|cx| {
            second
                .update(cx, |view, window, cx| {
                    assert!(view.finish_window_tab_drag(
                        &MouseUpEvent {
                            button: MouseButton::Left,
                            ..Default::default()
                        },
                        window,
                        cx
                    ));
                })
                .unwrap();
        });
        wait(|| {
            cx.run_until_parked();
            update(cx, second, |view, _| view.session.tabs.len() == 2)
        });
        assert_eq!(
            ids(&client),
            before,
            "moving a tab must preserve every process"
        );
        assert_eq!(cx.windows().len(), 1);
        update(cx, second, |view, cx| {
            assert_eq!(view.session.tabs[1].manual_title.as_deref(), Some("server"));
            assert!(view.close_active_pane(cx));
        });
        wait(|| {
            cx.run_until_parked();
            ids(&client).len() == 2
        });
        cx.update(|cx| cx.quit());
        cx.run_until_parked();
    } else {
        assert_eq!(cx.update(|cx| crate::multiplexer::pending_windows(cx)), 0);
        assert_eq!(ids(&client).len(), 2);
        update(cx, first, |view, cx| {
            assert_eq!(view.session.tabs.len(), 2);
            assert_eq!(view.session.tabs[1].manual_title.as_deref(), Some("server"));
            assert_eq!(view.session.tabs[1].panes.len(), 1);
            view.session.tabs[1].pinned = false;
            view.close_tab(1, cx);
        });
        assert_eq!(ids(&client).len(), 1);
        cx.update(|cx| {
            first
                .update(cx, |view, window, cx| {
                    view.request_active_tab_close(window, cx);
                })
                .unwrap();
        });
        cx.run_until_parked();
        assert!(
            ids(&client).is_empty(),
            "last-tab close must end its process instead of detaching"
        );
    }
}

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[gpui::test]
fn multiplexer_text_selection_survives_scrolling_and_output(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let _host = Host(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{PREFIX}host_process"), "--nocapture"])
            .env("TERMY_DESKTOP_MUX_ROOT", &root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait(|| SessionClient::connect(&root).is_ok());
    let client = SessionClient::connect(&root).unwrap();
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        let launch = TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "stty -echo; i=0; while [ $i -lt 100 ]; do printf 'line-%02d\\r\\n' \"$i\"; i=$((i+1)); done; printf READY; read go; printf '\\r\\nincoming-output\\r\\n'; read done".into()],
        };
        assert!(view.add_tab_with_launch(None, Some(&launch), cx));
    });
    wait(|| update(cx, handle, |view, _| core_text(view).contains("READY")));

    let cell_position = |view: &TerminalView, col: usize, row: usize| {
        let (padding_x, padding_y) = view.effective_terminal_padding();
        let size = view.layout_cell_size();
        point(
            px(view.workspace_sidebar_width() + padding_x) + size.width * (col as f32 + 0.5),
            px(view.terminal_content_top_inset() + padding_y) + size.height * (row as f32 + 0.5),
        )
    };
    cx.update(|cx| {
        handle
            .update(cx, |view, window, cx| {
                view.process_terminal_events(cx);
                let start = cell_position(view, 0, 1);
                let end = cell_position(view, 6, 1);
                view.handle_mouse_down(
                    &MouseDownEvent {
                        button: MouseButton::Left,
                        position: start,
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                view.handle_global_mouse_move_event(
                    &MouseMoveEvent {
                        pressed_button: Some(MouseButton::Left),
                        position: end,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                assert!(view.selection_dragging && view.has_selection());
                assert!(view.selected_text().unwrap().starts_with("line-"));
                let anchor = view.selection_anchor;
                view.handle_terminal_scroll_wheel(
                    &ScrollWheelEvent {
                        position: end,
                        delta: gpui::ScrollDelta::Lines(point(0.0, 1.0)),
                        touch_phase: TouchPhase::Moved,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.content_scroll_baseline,
                    view.active_terminal().unwrap().scroll_state().0
                );
                assert!(view.content_scroll_baseline > 0);
                view.process_terminal_events(cx);
                assert_eq!(
                    view.selection_anchor, anchor,
                    "user scrolling must not move the selection anchor"
                );
                assert!(view.selection_dragging && view.has_selection());
                assert!(view.handle_global_mouse_up_event(
                    &MouseUpEvent {
                        button: MouseButton::Left,
                        position: end,
                        click_count: 1,
                        ..Default::default()
                    },
                    cx
                ));
                assert!(!view.selection_dragging && view.has_selection());
            })
            .unwrap();
    });

    let (selected, history) = update(cx, handle, |view, _| {
        let selected = view.selected_text().unwrap();
        let terminal = view.active_terminal().unwrap();
        let history = terminal.scroll_state().1;
        terminal.write_input(b"go\n");
        (selected, history)
    });
    wait(|| {
        update(cx, handle, |view, _| {
            view.active_terminal().unwrap().scroll_state().1 > history
        })
    });
    update(cx, handle, |view, cx| {
        view.process_terminal_events(cx);
        assert!(view.has_selection());
        assert_eq!(view.selected_text().as_deref(), Some(selected.as_str()));
    });
    client.shutdown().unwrap();
}

#[gpui::test]
fn desktop_saves_preserve_cli_workspace_edits(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let _host = Host(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("{PREFIX}host_process"), "--nocapture"])
            .env("TERMY_DESKTOP_MUX_ROOT", &root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait(|| SessionClient::connect(&root).is_ok());
    let client = SessionClient::connect(&root).unwrap();
    let session = cx.update(|cx| {
        crate::multiplexer::install_for_test(client.clone(), cx).unwrap();
        crate::multiplexer::claim_window(cx, false).unwrap().0
    });
    let local = termy_core::session_model::StoredSession {
        active_workspace: 0,
        workspaces: vec![termy_core::session_model::StoredWorkspace {
            name: "Development".into(),
            pinned: false,
            active_tab: 0,
            tabs: Vec::new(),
        }],
    };
    session.save(local.clone()).unwrap();
    let saved: termy_core::session_model::StoredMultiplexer =
        serde_json::from_str(&client.layout().unwrap().unwrap()).unwrap();
    client
        .edit_workspace(
            &saved.windows[0].id,
            0,
            &local.workspaces[0],
            &termy_core::session_model::WorkspaceEdit::SetPinned { pinned: true },
        )
        .unwrap();
    session.save(local.clone()).unwrap();
    session.save(local.clone()).unwrap();
    let saved: termy_core::session_model::StoredMultiplexer =
        serde_json::from_str(&client.layout().unwrap().unwrap()).unwrap();
    assert!(saved.windows[0].session.workspaces[0].pinned);
    let mut changed = local;
    changed.workspaces[0].name = "Desktop renamed".into();
    session.save(changed).unwrap();
    let saved: termy_core::session_model::StoredMultiplexer =
        serde_json::from_str(&client.layout().unwrap().unwrap()).unwrap();
    assert_eq!(
        saved.windows[0].session.workspaces[0].name,
        "Desktop renamed"
    );
    assert!(saved.windows[0].session.workspaces[0].pinned);
}

#[test]
fn desktop_windows_restore_live_sessions_after_client_exit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let exe = std::env::current_exe().unwrap();
    let mut host = Host(
        Command::new(&exe)
            .args(["--exact", &format!("{PREFIX}host_process"), "--nocapture"])
            .env("TERMY_DESKTOP_MUX_ROOT", &root)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    wait(|| SessionClient::connect(&root).is_ok());
    let client = SessionClient::connect(&root).unwrap();
    for phase in ["create", "restore", "transfer", "close"] {
        let output = Command::new(&exe)
            .args([
                "--exact",
                &format!("{PREFIX}desktop_client_process"),
                "--nocapture",
            ])
            .env("TERMY_DESKTOP_MUX_ROOT", &root)
            .env("TERMY_DESKTOP_MUX_PHASE", phase)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{phase}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if phase != "close" {
            let ticks = fs::metadata(root.join("left.ticks")).unwrap().len();
            wait(|| fs::metadata(root.join("left.ticks")).unwrap().len() > ticks);
        }
    }
    assert!(client.list().unwrap().is_empty());
    client.shutdown().unwrap();
    assert!(host.0.wait().unwrap().success());
}
