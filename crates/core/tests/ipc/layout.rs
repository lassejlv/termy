//! The session layout the host owns: revisioned writes, closes, splits and resizes.

use super::*;

fn saved_pane(id: &str, left: u16, width: u16) -> session_model::StoredPane {
    session_model::StoredPane {
        session_id: Some(id.into()),
        left,
        top: 0,
        width,
        height: 24,
        buffer: None,
        zoom_steps: 0,
    }
}

fn saved_tab(panes: Vec<session_model::StoredPane>) -> session_model::StoredTab {
    session_model::StoredTab {
        zoomed: false,
        presentation: None,
        pinned: false,
        manual_title: None,
        active_pane: 0,
        layout_tree_json: None,
        panes,
    }
}

/// One window, one workspace holding `tabs`, with the last tab active.
fn saved_layout(tabs: Vec<session_model::StoredTab>) -> String {
    use session_model::*;
    serde_json::to_string(&StoredMultiplexer {
        version: STORED_MULTIPLEXER_VERSION,
        windows: vec![StoredWindow {
            id: "window".into(),
            session: StoredSession {
                workspaces: vec![StoredWorkspace {
                    name: "Work".into(),
                    pinned: false,
                    active_tab: tabs.len().saturating_sub(1),
                    tabs,
                }],
                active_workspace: 0,
                zoom_steps: 0,
            },
        }],
    })
    .unwrap()
}

fn saved_tabs(client: &SessionClient) -> Vec<Vec<(String, u16)>> {
    let layout =
        session_model::StoredMultiplexer::parse(client.layout().unwrap().as_deref()).unwrap();
    layout.windows[0].session.workspaces[0]
        .tabs
        .iter()
        .map(|tab| {
            tab.panes
                .iter()
                .map(|pane| (pane.session_id.clone().unwrap(), pane.width))
                .collect()
        })
        .collect()
}

/// Audit I4: the host versions the layout, rejects layouts it cannot read and
/// tells watchers about every accepted write.
#[test]
fn layout_writes_are_revisioned_validated_and_announced() {
    let host = Host::new();
    let mut watch = host.client.watch_layout().unwrap();
    assert_eq!(watch.recv().unwrap(), 0);
    assert_eq!(host.client.layout_revision().unwrap(), (0, None));
    let initial = saved_layout(Vec::new());
    assert_eq!(
        host.client
            .compare_and_set_layout(0, initial.clone())
            .unwrap(),
        Some(1)
    );
    assert_eq!(watch.recv().unwrap(), 1);
    assert_eq!(
        host.client.compare_and_set_layout(0, initial).unwrap(),
        None,
        "a stale revision must not overwrite"
    );

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writers: Vec<_> = ["desktop", "CLI"]
        .into_iter()
        .map(|name| {
            let client = SessionClient::connect(&host.root).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let mut tab = saved_tab(vec![saved_pane(name, 0, 80)]);
                tab.manual_title = Some(name.into());
                barrier.wait();
                let written = client
                    .compare_and_set_layout(1, saved_layout(vec![tab]))
                    .unwrap();
                (name, written)
            })
        })
        .collect();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|(_, written)| written.is_some())
            .count(),
        1
    );
    let winner = results
        .iter()
        .find(|(_, written)| written.is_some())
        .unwrap()
        .0;
    assert_eq!(saved_tabs(&host.client), [[(winner.to_owned(), 80)]]);
    assert_eq!(watch.recv().unwrap(), 2);

    for invalid in [
        "legacy write",
        r#"{"tabs":[]}"#,
        r#"{"version":9,"windows":[]}"#,
    ] {
        assert!(host.client.set_layout(invalid.into()).is_err(), "{invalid}");
        assert!(
            host.client
                .compare_and_set_layout(2, invalid.into())
                .is_err()
        );
    }
    assert_eq!(host.client.layout_revision().unwrap().0, 2);
    // An unconditional write still gets a revision and a notification.
    assert_eq!(host.client.set_layout(saved_layout(Vec::new())).unwrap(), 3);
    assert_eq!(watch.recv().unwrap(), 3);
}

/// Audit I4 and B2: the host prunes panes it closes and panes that exit while
/// no client shows them. An attached client still gets `Exit` and decides.
#[test]
fn closed_and_exited_panes_leave_the_layout() {
    let host = Host::new();
    let (kept, kept_terminal) = host.client.create(shell("read line"), None).unwrap();
    let (closed, closed_terminal) = host.client.create(shell("read line"), None).unwrap();
    // Never attached, so no client shows it when it exits.
    let exited = host
        .client
        .create_detached(shell("read line; exit 0"))
        .unwrap();
    let (watched, watched_terminal) = host
        .client
        .create(shell("read line; exit 0"), None)
        .unwrap();
    host.client
        .set_layout(saved_layout(vec![
            saved_tab(vec![saved_pane(&watched, 0, 80)]),
            saved_tab(vec![saved_pane(&kept, 0, 40), saved_pane(&closed, 40, 40)]),
            saved_tab(vec![saved_pane(&exited, 0, 80)]),
        ]))
        .unwrap();
    let mut watch = host.client.watch_layout().unwrap();
    let revision = watch.recv().unwrap();

    drop(closed_terminal);
    host.client.close(&closed).unwrap();
    assert!(watch.recv().unwrap() > revision);
    assert_eq!(
        saved_tabs(&host.client),
        [
            vec![(watched.clone(), 80)],
            vec![(kept.clone(), 80)],
            vec![(exited.clone(), 80)]
        ]
    );

    // Detached: the host removes the pane once its shell exits.
    host.client.write(&exited, b"\n".to_vec()).unwrap();
    wait_until(|| saved_tabs(&host.client).len() == 2);
    let layout =
        session_model::StoredMultiplexer::parse(host.client.layout().unwrap().as_deref()).unwrap();
    assert_eq!(
        layout.windows[0].session.workspaces[0].active_tab, 1,
        "the remaining neighbour of the closed active tab is selected"
    );

    // Attached: the client sees `Exit`; the layout keeps the pane for it.
    host.client.write(&watched, b"\n".to_vec()).unwrap();
    wait_until(|| {
        watched_terminal
            .drain_events(&mut |_| None)
            .0
            .iter()
            .any(|event| matches!(event, TerminalEvent::Exit))
    });
    assert_eq!(saved_tabs(&host.client).len(), 2);
    assert!(
        host.client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == watched && pane.exited)
    );
    drop((kept_terminal, watched_terminal));
    for id in [kept, watched, exited] {
        host.client.close(&id).unwrap();
    }
    assert!(saved_tabs(&host.client).is_empty());
}

/// Audit B17: creates that resolve their own workspace must not fail, or kill
/// their new pane, because another client saved in between.
#[test]
fn concurrent_creates_and_saves_all_succeed() {
    let host = Host::new();
    host.client.set_layout(saved_layout(Vec::new())).unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    // One creator resolves the default workspace, the other names it like
    // `termy mux create --window window --workspace 0`.
    let creators: Vec<_> = [false, true]
        .into_iter()
        .map(|named| {
            let client = SessionClient::connect(&host.root).unwrap();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                (0..3)
                    .map(|_| {
                        if named {
                            client.create_tab_in_workspace(shell("read line"), "window", 0)
                        } else {
                            client.create_tab(shell("read line"), None)
                        }
                        .unwrap()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let saver = {
        let client = SessionClient::connect(&host.root).unwrap();
        std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..40 {
                // A desktop save: selection and presentation change.
                client
                    .update_layout(|layout| {
                        let workspace = &mut layout.windows[0].session.workspaces[0];
                        workspace.pinned = !workspace.pinned;
                        workspace.active_tab = 0;
                        Ok(())
                    })
                    .unwrap();
            }
        })
    };
    let mut created: Vec<String> = creators
        .into_iter()
        .flat_map(|creator| creator.join().unwrap())
        .collect();
    saver.join().unwrap();
    let mut saved: Vec<String> = saved_tabs(&host.client)
        .into_iter()
        .map(|tab| tab[0].0.clone())
        .collect();
    created.sort();
    saved.sort();
    assert_eq!(saved, created);
    assert_eq!(host.client.list().unwrap().len(), 6);
    for id in created {
        host.client.close(&id).unwrap();
    }
}

/// Audit L6: closing a pane whose tab cannot be inferred repairs the layout
/// instead of failing after the process was killed. Audit L7: once the
/// layout change is committed, resize failures are warnings.
#[test]
fn close_and_split_commit_the_layout_and_warn_about_resizes() {
    let host = Host::new();
    let (left, left_terminal) = host.client.create(shell("read line"), None).unwrap();
    let (right, right_terminal) = host.client.create(shell("read line"), None).unwrap();
    drop((left_terminal, right_terminal));
    // Overlapping rects: no split tree can be inferred.
    host.client
        .set_layout(saved_layout(vec![saved_tab(vec![
            saved_pane(&left, 0, 60),
            saved_pane(&right, 30, 50),
            saved_pane("ghost", 0, 80),
        ])]))
        .unwrap();
    let warnings = host.client.close_saved_pane(&right).unwrap();
    assert_eq!(
        saved_tabs(&host.client),
        [[(left.clone(), 40), ("ghost".to_owned(), 40)]]
    );
    assert!(
        host.client
            .list()
            .unwrap()
            .iter()
            .all(|pane| pane.id != right)
    );
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("ghost"), "{warnings:?}");

    let tab = session_model::StoredMultiplexer::parse(host.client.layout().unwrap().as_deref())
        .unwrap()
        .windows[0]
        .session
        .workspaces[0]
        .tabs[0]
        .clone();
    let (new_id, warnings) = host
        .client
        .split_pane(
            &left,
            session_model::PaneResizeAxis::Horizontal,
            &tab,
            shell("read line"),
        )
        .unwrap();
    assert_eq!(saved_tabs(&host.client)[0].len(), 3);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("ghost"), "{warnings:?}");
    for id in [left, new_id] {
        host.client.close(&id).unwrap();
    }
}

#[test]
fn layout_resizes_keep_the_live_cell_metrics() {
    use termy_core::session_model::*;
    let host = Host::new();
    let launch = |size| PaneLaunch {
        size,
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "read done".into()],
        }),
        environment: None,
    };
    // The desktop created this pane with its real font metrics.
    let live = TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 11.0,
        cell_height: 23.0,
    };
    let (id, terminal) = host.client.create(launch(live), None).unwrap();
    let tab = StoredTab {
        zoomed: false,
        presentation: None,
        pinned: false,
        manual_title: None,
        active_pane: 0,
        layout_tree_json: None,
        panes: vec![StoredPane {
            session_id: Some(id.clone()),
            left: 0,
            top: 0,
            width: 80,
            height: 24,
            buffer: None,
            zoom_steps: 0,
        }],
    };
    let layout = StoredMultiplexer {
        version: 1,
        windows: vec![StoredWindow {
            id: "window".into(),
            session: StoredSession {
                workspaces: vec![StoredWorkspace {
                    name: "main".into(),
                    pinned: false,
                    active_tab: 0,
                    tabs: vec![tab.clone()],
                }],
                active_workspace: 0,
                zoom_steps: 0,
            },
        }],
    };
    host.client
        .set_layout(serde_json::to_string(&layout).unwrap())
        .unwrap();

    // The CLI splits without knowing the font; both panes keep 11x23 cells.
    let (new_id, warnings) = host
        .client
        .split_pane(
            &id,
            PaneResizeAxis::Horizontal,
            &tab,
            launch(TerminalSize::default()),
        )
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for pane in [&new_id, &id] {
        let size = host.client.snapshot(pane).unwrap().size;
        assert_eq!((size.cell_width, size.cell_height), (11.0, 23.0));
        assert!(size.cols < 80, "pane was not resized: {size:?}");
    }
    drop(terminal);
    host.client.close(&new_id).unwrap();
    host.client.close(&id).unwrap();
}

/// Audit B20: a view learns that another client resized its pane, also when
/// it never resized the pane itself and when nobody read the size while the
/// frame that confirmed its own resize was current.
#[test]
fn another_clients_resize_overrides_the_size_this_client_claimed() {
    let host = Host::new();
    let size = |cols, rows| TerminalSize {
        cols,
        rows,
        ..TerminalSize::default()
    };
    let grid = |size: TerminalSize| (size.cols, size.rows);
    let (id, mut terminal) = host
        .client
        .create(
            PaneLaunch {
                size: size(100, 30),
                working_directory: None,
                shell_integration: None,
                config: TerminalRuntimeConfig::default(),
                launch: Some(TerminalLaunch::Program {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), "read done".into()],
                }),
                environment: None,
            },
            None,
        )
        .unwrap();
    assert!(!terminal.host_size_overridden());

    // Created at the view's size: this client never sent a resize.
    host.client.resize(&id, size(40, 10)).unwrap();
    wait_until(|| grid(terminal.size()) == (40, 10));
    assert!(terminal.host_size_overridden());
    assert_eq!(grid(terminal.requested_size()), (40, 10));

    // Re-asserting clears the override.
    terminal.resize(size(100, 30));
    assert!(!terminal.host_size_overridden());
    wait_until(|| grid(terminal.size()) == (100, 30));
    assert!(!terminal.host_size_overridden());

    // `requested_size` is not read while the 90x20 frame is current.
    terminal.resize(size(90, 20));
    wait_until(|| grid(terminal.size()) == (90, 20));
    host.client.resize(&id, size(30, 8)).unwrap();
    wait_until(|| grid(terminal.size()) == (30, 8));
    assert!(terminal.host_size_overridden());
    assert_eq!(
        grid(terminal.requested_size()),
        (30, 8),
        "a stale request must not hide the other client's size"
    );
    drop(terminal);
    host.client.close(&id).unwrap();
}
