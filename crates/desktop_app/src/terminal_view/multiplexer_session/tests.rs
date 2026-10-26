use super::*;
use gpui_kit::{AppContext, TestAppContext, WindowHandle, WindowOptions};
use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
};
use termy_core::multiplexer::SessionClient;

mod layout;

const PREFIX: &str = "terminal_view::multiplexer_session::tests::";

#[gpui_kit::test]
fn multiplexer_is_off_by_default_and_config_changes_require_restart(cx: &mut TestAppContext) {
    let mut config = AppConfig::default();
    assert!(!config.multiplexer_enabled);
    cx.update(|cx| {
        crate::multiplexer::initialize(&config, cx);
        assert!(!crate::multiplexer::enabled(cx));
        assert!(crate::multiplexer::claim_window(cx, true).is_none());
        config.multiplexer_enabled = true;
        crate::multiplexer::initialize(&config, cx);
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
    // These integration windows receive wakeups from real PTY/IPC threads.
    // Tell GPUI's test scheduler that this is intentionally non-deterministic I/O.
    cx.executor().allow_parking();
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

#[gpui_kit::test]
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
        // TestAppContext::quit runs shutdown observers; the test platform
        // implementation of App::quit is a no-op.
        cx.quit();
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
        cx.quit();
        cx.run_until_parked();
    } else if phase == "transfer" {
        let second = window(cx, false);
        let before = ids(&client);
        update(cx, first, |view, cx| {
            view.arm_window_tab_drag(0, gpui_kit::point(px(1.0), px(1.0)), cx);
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
            cx.executor().advance_clock(Duration::from_millis(100));
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
        cx.quit();
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

#[gpui_kit::test]
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
    // Lay the new tab out at the window's size, and wait until the host shows
    // that size, before selecting. The resize goes to the host on the input
    // connection and the scroll below on a request connection of its own, so
    // a resize still on its way can land after the scroll: the taller screen
    // pulls history back onto it, under the selection and the history count
    // the output is checked against.
    cx.update(|cx| {
        handle
            .update(cx, |view, window, cx| {
                view.last_terminal_resize_signature = None;
                view.last_resize_applied_at = None;
                let cell_size = view.calculate_cell_size(window, cx);
                view.sync_terminal_size(window, cell_size, cx);
            })
            .unwrap();
    });
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| {
            let terminal = view.active_terminal().unwrap();
            terminal.size() == terminal.requested_size()
        })
    });

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
                        delta: gpui_kit::ScrollDelta::Lines(point(0.0, 1.0)),
                        touch_phase: TouchPhase::Moved,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                assert_eq!(
                    view.content_scroll_baseline.display_offset,
                    view.active_terminal().unwrap().scroll_state().0
                );
                assert!(view.content_scroll_baseline.display_offset > 0);
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

    // Settle pending layout before taking the scroll baseline. Real PTY/IPC
    // wakeups also need the test UI executor to run while waiting for output.
    cx.run_until_parked();
    let (selected, history) = update(cx, handle, |view, _| {
        let selected = view.selected_text().unwrap();
        let terminal = view.active_terminal().unwrap();
        let history = terminal.scroll_state().1;
        terminal.write_input(b"go\n");
        (selected, history)
    });
    wait(|| {
        cx.run_until_parked();
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

#[gpui_kit::test]
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
        zoom_steps: 0,
        workspaces: vec![termy_core::session_model::StoredWorkspace {
            name: "Development".into(),
            pinned: false,
            active_tab: 0,
            tabs: Vec::new(),
        }],
    };
    session.save(local.clone(), session.base()).unwrap();
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
    session.save(local.clone(), session.base()).unwrap();
    session.save(local.clone(), session.base()).unwrap();
    let saved: termy_core::session_model::StoredMultiplexer =
        serde_json::from_str(&client.layout().unwrap().unwrap()).unwrap();
    assert!(saved.windows[0].session.workspaces[0].pinned);
    let mut changed = local;
    changed.workspaces[0].name = "Desktop renamed".into();
    session.save(changed, session.base()).unwrap();
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

/// Start a host in a separate process and install a client for this app.
fn start_host(cx: &mut TestAppContext) -> (tempfile::TempDir, Host, SessionClient) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let host = Host(
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
    (temp, host, client)
}

fn shell(script: &str) -> TerminalLaunch {
    TerminalLaunch::Program {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), format!("stty -echo; {script}")],
    }
}

fn terminal_text(terminal: &Terminal) -> String {
    let Terminal::Native(native) = terminal else {
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

#[gpui_kit::test]
fn inactive_tab_cap_leaves_host_history_alone(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        view.inactive_tab_scrollback = Some(2);
        let history = shell(
            "i=0; while [ $i -lt 200 ]; do printf 'line-%03d\\r\\n' $i; i=$((i+1)); done; \
             printf READY; read go; printf '\\r\\nDONE\\r\\n'; read done",
        );
        assert!(view.add_tab_with_launch(None, Some(&history), cx));
    });
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| {
            core_text(view).contains("READY")
                && view.active_terminal().unwrap().scroll_state().1 > 100
        })
    });
    update(cx, handle, |view, cx| {
        // Opening a second tab parks the first one as inactive.
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        assert_eq!(view.session.active_tab, 1);
        view.switch_tab(0, cx);
        // Input queues behind any option change, so once DONE is drawn the
        // host has applied whatever this view sent while the tab was hidden.
        view.active_terminal().unwrap().write_input(b"go\n");
    });
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| core_text(view).contains("DONE"))
    });
    update(cx, handle, |view, _| {
        assert!(
            view.active_terminal().unwrap().scroll_state().1 > 100,
            "the inactive-tab cap must not trim history in the shared host"
        );
    });
    client.shutdown().unwrap();
}

/// Audit B20: a pane this view created at exactly its own size, and never
/// resized, still gets that size back after another client resizes it.
#[gpui_kit::test]
fn desktop_reasserts_its_size_after_another_client_resizes_a_pane(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let handle = window(cx, true);
    let new_tab = |cx: &mut TestAppContext| {
        update(cx, handle, |view, cx| {
            assert!(view.add_tab_with_launch(None, Some(&shell("printf READY; read done")), cx));
        });
        wait(|| {
            cx.run_until_parked();
            update(cx, handle, |view, _| core_text(view).contains("READY"))
        });
    };
    // Runs one layout pass and reports whether it resized a terminal. A
    // `full` pass ignores that nothing changed in the window's layout.
    let sync = |cx: &mut TestAppContext, full: bool| {
        cx.update(|cx| {
            handle
                .update(cx, |view, window, cx| {
                    if full {
                        view.last_terminal_resize_signature = None;
                    }
                    view.last_resize_applied_at = None;
                    let cell_size = view.calculate_cell_size(window, cx);
                    view.sync_terminal_size(window, cell_size, cx);
                    view.last_resize_applied_at.is_some()
                })
                .unwrap()
        })
    };
    let host_size = |cx: &mut TestAppContext| {
        update(cx, handle, |view, _| {
            let size = view.active_terminal().unwrap().size();
            (size.cols, size.rows)
        })
    };
    // Two tabs first, so the tab strip no longer changes the layout.
    new_tab(cx);
    new_tab(cx);
    sync(cx, true);
    let desktop = update(cx, handle, |view, _| {
        let size = view.active_terminal().unwrap().requested_size();
        (size.cols, size.rows)
    });
    assert_ne!(desktop, (40, 10));
    wait(|| {
        cx.run_until_parked();
        host_size(cx) == desktop
    });

    // New Tab: the pane starts at the view's size, so no resize is ever sent.
    new_tab(cx);
    assert_eq!(host_size(cx), desktop);
    assert!(
        !sync(cx, true),
        "the new pane was not created at the view's size"
    );
    assert!(!sync(cx, false));

    let id = update(cx, handle, |view, _| {
        view.active_terminal()
            .unwrap()
            .session_id()
            .unwrap()
            .to_owned()
    });
    client
        .resize(
            &id,
            TerminalSize {
                cols: 40,
                rows: 10,
                ..TerminalSize::default()
            },
        )
        .unwrap();
    wait(|| {
        cx.run_until_parked();
        host_size(cx) == (40, 10)
    });
    assert!(
        sync(cx, false),
        "an unchanged layout must re-assert the size"
    );
    wait(|| {
        cx.run_until_parked();
        host_size(cx) == desktop
    });
    assert!(!sync(cx, false));
    client.shutdown().unwrap();
}

#[gpui_kit::test]
fn a_new_host_pane_does_not_replay_a_saved_buffer_above_its_prompt(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let handle = window(cx, true);
    let terminal = update(cx, handle, |view, _| {
        Terminal::new_native_restoring(
            TerminalSize::default(),
            None,
            Some(&view.native_terminal_wakeup_router),
            Some(&view.tab_shell_integration),
            Some(&view.terminal_runtime),
            view.multiplexer_client(),
            b"SAVED-BUFFER\r\n",
        )
        .unwrap()
    });
    terminal.write_input(b"echo MARK-$((40+2))\n");
    wait(|| terminal_text(&terminal).contains("MARK-42"));
    assert!(!terminal_text(&terminal).contains("SAVED-BUFFER"));
    drop(terminal);
    client.shutdown().unwrap();
}

#[gpui_kit::test]
fn a_closed_window_can_be_reopened_while_others_stay_open(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let first = window(cx, true);
    update(cx, first, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
    });
    let second = window(cx, true);
    update(cx, second, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        view.session.tabs[0].manual_title = Some("second window".into());
    });
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 0);
    cx.update(|cx| {
        second
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert_eq!(ids(&client).len(), 2, "closing a window detaches its panes");
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 1);

    assert!(cx.update(|cx| crate::multiplexer::queue_detached_window(cx)));
    let reopened = window(cx, false);
    update(cx, reopened, |view, _| {
        assert_eq!(view.session.tabs.len(), 1);
        assert_eq!(
            view.session.tabs[0].manual_title.as_deref(),
            Some("second window")
        );
    });
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 0);
    assert!(!cx.update(|cx| crate::multiplexer::queue_detached_window(cx)));
    assert_eq!(ids(&client).len(), 2);
    client.shutdown().unwrap();
}

fn layout_window_count(client: &SessionClient) -> usize {
    client.layout().unwrap().map_or(0, |json| {
        serde_json::from_str::<termy_core::session_model::StoredMultiplexer>(&json)
            .unwrap()
            .windows
            .len()
    })
}

/// Audit I6: with the close warning on, closing one of several windows asks
/// whether to detach it (its processes keep running for Reopen Detached
/// Window) or to terminate its processes.
#[gpui_kit::test]
fn closing_one_of_several_windows_offers_detach_or_terminate(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_prompt_builder(crate::linux_prompt::render_prompt));
    let (_temp, _host, client) = start_host(cx);
    let first = window(cx, true);
    update(cx, first, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        view.sync_persisted_native_workspace();
    });
    let close_with = |cx: &mut TestAppContext, handle: WindowHandle<TerminalView>, keys: &str| {
        cx.update(|cx| {
            handle
                .update(cx, |view, window, cx| {
                    view.warn_on_quit = true;
                    view.sync_persisted_native_workspace();
                    assert!(!view.handle_window_should_close_request(window, cx));
                    assert!(view.quit_prompt_in_flight);
                })
                .unwrap();
        });
        cx.simulate_keystrokes(handle.into(), keys);
        cx.run_until_parked();
    };

    let second = window(cx, true);
    update(cx, second, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
    });
    close_with(cx, second, "enter");
    assert_eq!(ids(&client).len(), 2, "Detach keeps the processes running");
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 1);

    assert!(cx.update(|cx| crate::multiplexer::queue_detached_window(cx)));
    let reopened = window(cx, false);
    update(cx, reopened, |view, _| {
        assert_eq!(view.session.tabs.len(), 1);
    });
    assert_eq!(layout_window_count(&client), 2);
    close_with(cx, reopened, "tab enter");
    wait(|| {
        cx.run_until_parked();
        ids(&client).len() == 1
    });
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 0);
    assert_eq!(layout_window_count(&client), 1, "nothing is left to reopen");
    client.shutdown().unwrap();
}

/// Audit I6: a detached window whose processes all ended is not offered for
/// reopening, so it can never come back empty and be saved over.
#[gpui_kit::test]
fn a_detached_window_whose_processes_ended_is_not_reopened(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let first = window(cx, true);
    update(cx, first, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
    });
    let second = window(cx, true);
    let id = update(cx, second, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        view.sync_persisted_native_workspace();
        view.active_terminal()
            .unwrap()
            .session_id()
            .unwrap()
            .to_owned()
    });
    cx.update(|cx| {
        second
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 1);

    client.write(&id, b"go\n".to_vec()).unwrap();
    wait(|| {
        client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == id && pane.exited)
    });
    assert_eq!(cx.update(|cx| crate::multiplexer::detached_windows(cx)), 0);
    assert!(!cx.update(|cx| crate::multiplexer::queue_detached_window(cx)));
    client.shutdown().unwrap();
}

/// Audit B23: a user who turned the multiplexer on before the import marker
/// existed gets it with the first host save, so a later host restart (for
/// example after a reboot) does not import their old local tabs again.
#[gpui_kit::test]
fn the_first_host_save_marks_the_local_session_as_imported(cx: &mut TestAppContext) {
    let (_temp, _host, client) = start_host(cx);
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        crate::workspace_store::WorkspaceStore::open(&dir.path().join("workspaces.db")).unwrap(),
    );
    store
        .save_session(&super::import_tests::local_session())
        .unwrap();
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        view.owns_persisted_session = true;
        view.native_tab_persistence = true;
        assert!(view.workspace_store.set(Some(Arc::clone(&store))).is_ok());
        assert!(view.multiplexer_import_pending());
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        view.sync_persisted_native_workspace();
    });
    wait(|| store.imported_into_multiplexer().unwrap());
    update(cx, handle, |view, _| {
        assert!(!view.multiplexer_import_pending(), "recorded once per run");
    });
    client.shutdown().unwrap();
}

/// A program that starts a host the way Termy's own executable does: this
/// test binary runs `host_process` for `--multiplexer-host <root>`.
fn host_launcher(dir: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let launcher = dir.join("termy-host");
    let script = format!(
        "#!/bin/sh\nTERMY_DESKTOP_MUX_ROOT=\"$2\" exec '{}' --exact '{PREFIX}host_process' --nocapture\n",
        std::env::current_exe().unwrap().display()
    );
    fs::write(&launcher, script).unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
    launcher
}

use crate::multiplexer::HOST_LOSS_REPORTED;

fn kill_host(root: &Path) {
    termy_core::multiplexer::kill_host(root).unwrap();
    wait(|| !termy_core::multiplexer::host_is_running(root));
}

/// Stops whatever host runs in its directory, also when a test fails.
struct HostDirectory(std::path::PathBuf);
impl Drop for HostDirectory {
    fn drop(&mut self) {
        if termy_core::multiplexer::host_is_running(&self.0) {
            let _ = termy_core::multiplexer::kill_host(&self.0);
        }
    }
}

/// Audit B12, review finding 2: when the host dies, its tabs stay open,
/// marked as ended, and Termy keeps running; a new tab and a new window then
/// start a new host instead of failing.
#[gpui_kit::test]
fn new_tabs_and_windows_start_a_new_host_after_it_died(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    let _host = HostDirectory(root.clone());
    let client =
        termy_core::multiplexer::connect_or_start(&root, &host_launcher(temp.path())).unwrap();
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
    });
    let old = ids(&client);
    assert_eq!(old.len(), 1);

    // The window learns that the host is gone, and keeps its tab.
    kill_host(&root);
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, cx| {
            view.process_terminal_events(cx);
            view.active_terminal().unwrap().session_host_lost()
        })
    });
    cx.run_until_parked();
    update(cx, handle, |view, cx| {
        view.process_terminal_events(cx);
        assert_eq!(view.session.tabs.len(), 1, "the ended tab stays");
    });
    assert!(HOST_LOSS_REPORTED.load(Ordering::Acquire));
    assert!(!termy_core::multiplexer::host_is_running(&root));

    update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read done")), cx));
        assert_eq!(view.session.tabs.len(), 2);
    });
    let new = ids(&client);
    assert_eq!(new.len(), 1);
    assert_ne!(new, old);
    update(cx, handle, |view, _| {
        let session = view
            .active_terminal()
            .unwrap()
            .session_id()
            .map(str::to_owned);
        assert_eq!(session, Some(new[0].0.clone()));
    });

    kill_host(&root);
    let second = window(cx, false);
    update(cx, second, |view, _| {
        assert_eq!(view.session.tabs.len(), 1);
        assert!(view.active_terminal().unwrap().session_id().is_some());
    });
    assert_eq!(ids(&client).len(), 1);
    // Both windows are still open.
    cx.run_until_parked();
    update(cx, handle, |view, cx| {
        view.process_terminal_events(cx);
        assert_eq!(view.session.tabs.len(), 2);
    });
    client.shutdown().unwrap();
}

/// Audit B12: when no host can be started either, a new window still opens,
/// with a regular terminal.
#[gpui_kit::test]
fn a_new_window_without_a_host_opens_a_regular_terminal(cx: &mut TestAppContext) {
    let (_temp, host, client) = start_host(cx);
    drop(host);
    let handle = window(cx, false);
    update(cx, handle, |view, _| {
        assert_eq!(view.session.tabs.len(), 1);
        assert!(view.active_terminal().unwrap().session_id().is_none());
    });
    assert!(client.list().is_err());
}
