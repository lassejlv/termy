//! Layout ownership between the desktop and other clients (audit B2, B3, B4,
//! B5, B15), against a real host process.
use super::*;
use termy_core::multiplexer::PaneLaunch;
use termy_core::session_model::{
    PaneResizeAxis, StoredMultiplexer, StoredPane, StoredSession, StoredTab, StoredWindow,
    StoredWorkspace, WindowEdit, WorkspaceTarget,
};

fn start_host(temp: &tempfile::TempDir) -> (Host, SessionClient) {
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
    (host, SessionClient::connect(&root).unwrap())
}

fn shell(script: &str) -> TerminalLaunch {
    TerminalLaunch::Program {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
    }
}

fn pane_launch(script: &str) -> PaneLaunch {
    PaneLaunch {
        size: TerminalSize::default(),
        working_directory: None,
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(shell(script)),
    }
}

fn saved_pane(id: &str, left: u16, width: u16) -> StoredPane {
    StoredPane {
        session_id: Some(id.into()),
        left,
        top: 0,
        width,
        height: 24,
        buffer: None,
        zoom_steps: 0,
    }
}

fn saved_tab(panes: Vec<StoredPane>) -> StoredTab {
    StoredTab {
        zoomed: false,
        presentation: None,
        pinned: false,
        manual_title: None,
        active_pane: 0,
        layout_tree_json: None,
        panes,
    }
}

fn saved_workspace(name: &str, tabs: Vec<StoredTab>) -> StoredWorkspace {
    StoredWorkspace {
        name: name.into(),
        pinned: false,
        active_tab: 0,
        tabs,
    }
}

fn set_window(client: &SessionClient, workspaces: Vec<StoredWorkspace>, active: usize) {
    let layout = StoredMultiplexer {
        version: 1,
        windows: vec![StoredWindow {
            id: "window".into(),
            session: StoredSession {
                workspaces,
                active_workspace: active,
                zoom_steps: 0,
            },
        }],
    };
    client
        .set_layout(serde_json::to_string(&layout).unwrap())
        .unwrap();
}

fn host_layout(client: &SessionClient) -> StoredMultiplexer {
    StoredMultiplexer::parse(client.layout().unwrap().as_deref()).unwrap()
}

fn tab_ids(workspace: &StoredWorkspace) -> Vec<Vec<String>> {
    workspace
        .tabs
        .iter()
        .map(|tab| {
            tab.panes
                .iter()
                .filter_map(|pane| pane.session_id.clone())
                .collect()
        })
        .collect()
}

fn view_tab_ids(view: &TerminalView) -> Vec<Vec<String>> {
    view.session
        .tabs
        .iter()
        .map(|tab| {
            let mut panes: Vec<_> = tab.panes.iter().collect();
            if let Some(zoom) = view.session.native_pane_zoom_snapshots.get(&tab.id) {
                panes.extend(&zoom.other_panes);
            }
            panes
                .iter()
                .filter_map(|pane| pane.terminal.session_id().map(str::to_owned))
                .collect()
        })
        .collect()
}

fn exit_pane(client: &SessionClient, id: &str) {
    client.write(id, b"\n".to_vec()).unwrap();
    wait(|| {
        client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == id && pane.exited)
    });
}

/// Audit B4 and B2: start-up publishes the reconciled layout, keeps empty
/// workspaces and closes exited panes instead of reattaching them.
#[gpui_kit::test]
fn install_publishes_the_reconciled_layout_and_closes_exited_panes(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let (live, live_terminal) = client.create(pane_launch("read line"), None).unwrap();
    // Still attached when it exits, like a window that quit on `exit`.
    let (exited, exited_terminal) = client
        .create(pane_launch("read line; exit 0"), None)
        .unwrap();
    exit_pane(&client, &exited);
    drop((live_terminal, exited_terminal));
    let mut work = saved_workspace(
        "Work",
        vec![
            saved_tab(vec![saved_pane(&live, 0, 40), saved_pane(&exited, 40, 40)]),
            saved_tab(vec![saved_pane("dead", 0, 80)]),
        ],
    );
    work.active_tab = 1;
    set_window(
        &client,
        vec![work, saved_workspace("Review", Vec::new())],
        1,
    );

    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let layout = host_layout(&client);
    let session = &layout.windows[0].session;
    assert_eq!(session.workspaces.len(), 2, "the empty workspace is kept");
    assert_eq!(session.workspaces[1].name, "Review");
    assert_eq!(session.active_workspace, 1);
    assert_eq!(tab_ids(&session.workspaces[0]), [[live.clone()]]);
    assert_eq!(session.workspaces[0].tabs[0].panes[0].width, 80);
    assert!(
        client.list().unwrap().iter().all(|pane| pane.id != exited),
        "an exited pane must not be reattached on the next launch"
    );
    assert_eq!(cx.update(|cx| crate::multiplexer::pending_windows(cx)), 1);
    client.close(&live).unwrap();
}

/// Audit B3: after the CLI changed a window's structure, desktop saves rebase
/// instead of failing, and keep working afterwards.
#[gpui_kit::test]
fn desktop_saves_rebase_onto_cli_structure_changes(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let session = cx.update(|cx| {
        crate::multiplexer::install_for_test(client.clone(), cx).unwrap();
        crate::multiplexer::claim_window(cx, false).unwrap().0
    });
    let (first, first_terminal) = client.create(pane_launch("read line"), None).unwrap();
    let (local, local_terminal) = client.create(pane_launch("read line"), None).unwrap();
    drop((first_terminal, local_terminal));
    let base = StoredSession {
        workspaces: vec![saved_workspace(
            "Work",
            vec![saved_tab(vec![saved_pane(&first, 0, 80)])],
        )],
        active_workspace: 0,
        zoom_steps: 0,
    };
    session.save(base.clone(), session.base()).unwrap();
    let window_id = host_layout(&client).windows[0].id.clone();

    let (created, terminal) = client
        .create_tab(
            pane_launch("read line"),
            None,
            Some(WorkspaceTarget {
                window_id,
                index: 0,
                expected: base.workspaces[0].clone(),
            }),
        )
        .unwrap();
    drop(terminal);
    let tab = host_layout(&client).windows[0].session.workspaces[0].tabs[0].clone();
    let (split, terminal, _) = client
        .split_pane(
            &first,
            PaneResizeAxis::Horizontal,
            &tab,
            pane_launch("read line"),
        )
        .unwrap();
    drop(terminal);

    let mut changed = base;
    changed.workspaces[0].tabs[0].manual_title = Some("server".into());
    changed.workspaces[0]
        .tabs
        .push(saved_tab(vec![saved_pane(&local, 0, 80)]));
    session.save(changed.clone(), session.base()).unwrap();
    let layout = host_layout(&client);
    let work = &layout.windows[0].session.workspaces[0];
    assert_eq!(
        tab_ids(work),
        [
            vec![first.clone(), split.clone()],
            vec![local.clone()],
            vec![created.clone()]
        ]
    );
    assert_eq!(work.tabs[0].manual_title.as_deref(), Some("server"));
    // One divergence must not fail every later save.
    changed.workspaces[0].pinned = true;
    session.save(changed, session.base()).unwrap();
    assert!(host_layout(&client).windows[0].session.workspaces[0].pinned);
    for id in [first, local, created, split] {
        client.close(&id).unwrap();
    }
}

/// Audit B15: a workspace not opened yet still opens when some of its panes
/// were closed or exited meanwhile.
#[gpui_kit::test]
fn cold_workspaces_open_after_their_panes_went_away(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let (visible, visible_terminal) = client.create(pane_launch("read line"), None).unwrap();
    let (cold, cold_terminal) = client.create(pane_launch("read line"), None).unwrap();
    drop((visible_terminal, cold_terminal));
    set_window(
        &client,
        vec![
            saved_workspace("Work", vec![saved_tab(vec![saved_pane(&visible, 0, 80)])]),
            saved_workspace("Docs", vec![saved_tab(vec![saved_pane(&cold, 0, 80)])]),
        ],
        0,
    );
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, false);
    // Exited while a client was attached, so the host kept it listed.
    let (exited, exited_terminal) = client
        .create(pane_launch("read line; exit 0"), None)
        .unwrap();
    exit_pane(&client, &exited);
    drop(exited_terminal);
    update(cx, handle, |view, cx| {
        assert!(view.session.workspaces[1].pending_restore.is_some());
        // The layout this window read still lists both panes.
        let pending = view.session.workspaces[1].pending_restore.as_mut().unwrap();
        pending
            .tabs
            .push(saved_tab(vec![saved_pane("closed", 0, 80)]));
        pending
            .tabs
            .push(saved_tab(vec![saved_pane(&exited, 0, 80)]));
        view.switch_workspace(1, cx);
        assert_eq!(view.session.active_workspace, 1);
        assert_eq!(view_tab_ids(view), [[cold.clone()]]);
    });
    assert!(client.list().unwrap().iter().all(|pane| pane.id != exited));
}

/// Audit B5: an open window shows tabs, splits, closes and workspaces that
/// the CLI made, without a relaunch.
#[gpui_kit::test]
fn open_windows_apply_cli_layout_changes(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    let first = update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read line")), cx));
        view.sync_persisted_native_workspace();
        view_tab_ids(view)[0][0].clone()
    });
    let layout = host_layout(&client);
    let window_id = layout.windows[0].id.clone();
    let settle = |cx: &mut TestAppContext, done: &dyn Fn(&TerminalView) -> bool| {
        wait(|| {
            cx.run_until_parked();
            update(cx, handle, |view, _| done(view))
        });
    };

    let (created, terminal) = client
        .create_tab(
            pane_launch("read line"),
            None,
            Some(WorkspaceTarget {
                window_id: window_id.clone(),
                index: 0,
                expected: layout.windows[0].session.workspaces[0].clone(),
            }),
        )
        .unwrap();
    drop(terminal);
    settle(cx, &|view| view_tab_ids(view).len() == 2);
    update(cx, handle, |view, _| {
        assert_eq!(view.session.active_tab, 1, "the CLI selected its new tab");
        assert_eq!(view_tab_ids(view)[1], std::slice::from_ref(&created));
    });

    let tab = host_layout(&client).windows[0].session.workspaces[0]
        .tabs
        .iter()
        .find(|tab| tab.panes[0].session_id.as_deref() == Some(first.as_str()))
        .unwrap()
        .clone();
    let (split, terminal, _) = client
        .split_pane(
            &first,
            PaneResizeAxis::Horizontal,
            &tab,
            pane_launch("read line"),
        )
        .unwrap();
    drop(terminal);
    settle(cx, &|view| view_tab_ids(view)[0].len() == 2);
    update(cx, handle, |view, _| {
        let tab = &view.session.tabs[0];
        assert!(view.session.native_pane_layout_trees.contains_key(&tab.id));
        assert!(tab.panes[1].left > tab.panes[0].left, "split left/right");
    });

    client.close_saved_pane(&created).unwrap();
    settle(cx, &|view| view_tab_ids(view).len() == 1);

    let session = host_layout(&client).windows[0].session.clone();
    client
        .edit_window(
            &window_id,
            &session,
            &WindowEdit::CreateWorkspace {
                name: "Review".into(),
            },
        )
        .unwrap();
    settle(cx, &|view| view.session.workspaces.len() == 2);
    update(cx, handle, |view, _| {
        assert_eq!(view.session.active_workspace, 1);
        assert_eq!(view.session.workspaces[1].name, "Review");
        assert!(view.session.tabs.is_empty());
        assert_eq!(view.session.workspaces[0].tabs.len(), 1);
    });
    let mut live: Vec<_> = client
        .list()
        .unwrap()
        .into_iter()
        .map(|pane| pane.id)
        .collect();
    live.sort();
    let mut expected = vec![first, split];
    expected.sort();
    assert_eq!(live, expected, "no process may be lost or leaked");
    client.shutdown().unwrap();
}

/// Audit B2: `exit` in a window's only pane closes the window for good; the
/// layout no longer lists it, so a relaunch does not reattach the dead pane.
#[gpui_kit::test]
fn shell_exit_in_the_last_pane_removes_the_window(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    let id = update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read line; exit 0")), cx));
        view.sync_persisted_native_workspace();
        view_tab_ids(view)[0][0].clone()
    });
    assert_eq!(host_layout(&client).windows.len(), 1);
    client.write(&id, b"\n".to_vec()).unwrap();
    wait(|| {
        cx.run_until_parked();
        let _ = cx.update(|cx| {
            handle.update(cx, |view, _, cx| {
                view.process_terminal_events(cx);
            })
        });
        client.list().unwrap().is_empty()
    });
    cx.run_until_parked();
    assert!(host_layout(&client).windows.is_empty());
}

/// Review finding: `exit` in the zoomed pane of a split closes that pane and
/// keeps the parked one running; the window must not close or end it.
#[gpui_kit::test]
fn an_exit_in_a_zoomed_split_keeps_the_parked_pane(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read line")), cx));
        assert!(view.split_active_pane_vertical_with_launch(
            None,
            Some(&shell("read line; exit 0")),
            cx
        ));
    });
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| view.session.tabs[0].panes.len() == 2)
    });
    let (editor, build) = update(cx, handle, |view, cx| {
        let editor = view
            .active_terminal()
            .unwrap()
            .session_id()
            .unwrap()
            .to_owned();
        assert!(view.toggle_pane_zoom(cx));
        assert_eq!(view.session.tabs[0].panes.len(), 1, "zoomed");
        let ids = view_tab_ids(view);
        let build = ids[0].iter().find(|id| **id != editor).unwrap().clone();
        (editor, build)
    });
    client.write(&editor, b"\n".to_vec()).unwrap();
    wait(|| {
        cx.run_until_parked();
        let _ = cx.update(|cx| {
            handle.update(cx, |view, _, cx| {
                view.process_terminal_events(cx);
            })
        });
        client.list().unwrap().iter().all(|pane| pane.id != editor)
    });
    cx.run_until_parked();
    update(cx, handle, |view, _| {
        assert_eq!(view_tab_ids(view), [[build.clone()]]);
        assert!(view.session.native_pane_zoom_snapshots.is_empty());
        view.sync_persisted_native_workspace();
    });
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == build && !pane.exited)
    );
    assert_eq!(
        tab_ids(&host_layout(&client).windows[0].session.workspaces[0]),
        [[build.clone()]]
    );
    client.shutdown().unwrap();
}

/// Review finding: a save that failed must not move the merge base, or the
/// next save would drop the unsaved tab and orphan its shell.
#[gpui_kit::test]
fn a_failed_save_keeps_its_edits_for_the_next_save(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let session = cx.update(|cx| {
        crate::multiplexer::install_for_test(client.clone(), cx).unwrap();
        crate::multiplexer::claim_window(cx, false).unwrap().0
    });
    let first = client.create_detached(pane_launch("read line")).unwrap();
    let added = client.create_detached(pane_launch("read line")).unwrap();
    let base = StoredSession {
        workspaces: vec![saved_workspace(
            "Work",
            vec![saved_tab(vec![saved_pane(&first, 0, 80)])],
        )],
        active_workspace: 0,
        zoom_steps: 0,
    };
    session.save(base.clone(), session.base()).unwrap();
    let mut with_tab = base.clone();
    with_tab.workspaces[0]
        .tabs
        .push(saved_tab(vec![saved_pane(&added, 0, 80)]));
    // The host refuses layouts over its size cap.
    let mut too_large = with_tab.clone();
    too_large.workspaces[0].tabs[0].manual_title = Some("x".repeat(5 * 1024 * 1024));
    assert!(session.save(too_large, session.base()).is_err());
    assert_eq!(session.base(), Some(base));
    session.save(with_tab, session.base()).unwrap();
    assert_eq!(
        tab_ids(&host_layout(&client).windows[0].session.workspaces[0]),
        [[first], [added]]
    );
    client.shutdown().unwrap();
}

/// Review finding: a save collected before the window adopted a remote change
/// must merge against the base it was collected with, or the remote tab would
/// look like a local close and vanish.
#[gpui_kit::test]
fn a_save_collected_before_a_remote_change_keeps_that_change(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let session = cx.update(|cx| {
        crate::multiplexer::install_for_test(client.clone(), cx).unwrap();
        crate::multiplexer::claim_window(cx, false).unwrap().0
    });
    let first = client.create_detached(pane_launch("read line")).unwrap();
    let base = StoredSession {
        workspaces: vec![saved_workspace(
            "Work",
            vec![saved_tab(vec![saved_pane(&first, 0, 80)])],
        )],
        active_workspace: 0,
        zoom_steps: 0,
    };
    session.save(base.clone(), session.base()).unwrap();
    let window_id = host_layout(&client).windows[0].id.clone();
    // The view collects a save on the foreground...
    let mut local = base;
    local.workspaces[0].tabs[0].manual_title = Some("server".into());
    let collected_base = session.base();
    // ...then the CLI adds a tab and the view adopts it before the save runs.
    let (created, terminal) = client
        .create_tab_in_workspace(pane_launch("read line"), None, &window_id, 0)
        .unwrap();
    drop(terminal);
    let (revision, json) = client.layout_revision().unwrap();
    let remote = StoredMultiplexer::parse(json.as_deref()).unwrap().windows[0]
        .session
        .clone();
    session.adopt_remote(revision, remote.clone());
    session.save(local, collected_base).unwrap();
    let layout = host_layout(&client);
    let work = &layout.windows[0].session.workspaces[0];
    assert_eq!(tab_ids(work), [[first], [created]]);
    assert_eq!(work.tabs[0].manual_title.as_deref(), Some("server"));
    assert_eq!(session.base(), Some(remote), "the adopted base is kept");
    client.shutdown().unwrap();
}

/// Audit B2: a window whose shells all exited while Termy was closed does not
/// come back as a blank window; the next start opens a normal one.
#[gpui_kit::test]
fn a_window_whose_shells_exited_while_detached_is_not_restored(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    let gone = client
        .create_detached(pane_launch("read line; exit 0"))
        .unwrap();
    set_window(
        &client,
        vec![saved_workspace(
            "Work",
            vec![saved_tab(vec![saved_pane(&gone, 0, 80)])],
        )],
        0,
    );
    exit_pane(&client, &gone);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    assert_eq!(cx.update(|cx| crate::multiplexer::pending_windows(cx)), 0);
    assert!(host_layout(&client).windows.is_empty());
    let handle = window(cx, false);
    let ids = update(cx, handle, |view, _| view_tab_ids(view));
    assert_eq!(ids.len(), 1);
    assert_ne!(ids[0], std::slice::from_ref(&gone));
    assert!(
        client
            .list()
            .unwrap()
            .iter()
            .any(|pane| pane.id == ids[0][0] && !pane.exited)
    );
    client.shutdown().unwrap();
}

/// Review finding: a pane another client listed that cannot be attached is
/// left out of the view, but must stay in the host layout.
#[gpui_kit::test]
fn a_pane_that_fails_to_attach_stays_in_the_host_layout(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read line")), cx));
        view.sync_persisted_native_workspace();
    });
    let added = client.create_detached(pane_launch("read line")).unwrap();
    client
        .update_layout(|layout| {
            let tabs = &mut layout.windows[0].session.workspaces[0].tabs;
            tabs.push(saved_tab(vec![saved_pane("ghost", 0, 80)]));
            tabs.push(saved_tab(vec![saved_pane(&added, 0, 80)]));
            Ok(())
        })
        .unwrap();
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| view_tab_ids(view).len() == 2)
    });
    update(cx, handle, |view, _| view.sync_persisted_native_workspace());
    let saved: Vec<_> = host_layout(&client)
        .session_ids()
        .map(str::to_owned)
        .collect();
    assert!(saved.contains(&"ghost".to_owned()), "{saved:?}");
    assert!(saved.contains(&added), "{saved:?}");
    client.shutdown().unwrap();
}

/// Audit B5: after a CLI split, the window fits both terminals to its own
/// size again instead of keeping the sizes the CLI gave them.
#[gpui_kit::test]
fn a_cli_split_is_refit_to_the_window(cx: &mut TestAppContext) {
    let temp = tempfile::tempdir().unwrap();
    let (_host, client) = start_host(&temp);
    cx.update(|cx| crate::multiplexer::install_for_test(client.clone(), cx).unwrap());
    let handle = window(cx, true);
    let first = update(cx, handle, |view, cx| {
        assert!(view.add_tab_with_launch(None, Some(&shell("read line")), cx));
        view.sync_persisted_native_workspace();
        view_tab_ids(view)[0][0].clone()
    });
    // The layout another client saved is narrower than this window.
    client
        .update_layout(|layout| {
            let pane = &mut layout.windows[0].session.workspaces[0].tabs[0].panes[0];
            (pane.width, pane.height) = (40, 10);
            Ok(())
        })
        .unwrap();
    let tab = host_layout(&client).windows[0].session.workspaces[0].tabs[0].clone();
    let (split, terminal, _) = client
        .split_pane(
            &first,
            PaneResizeAxis::Horizontal,
            &tab,
            pane_launch("read line"),
        )
        .unwrap();
    drop(terminal);
    let host_cols = |id: &str| client.attach(id, None).unwrap().size().cols;
    assert_eq!((host_cols(&first), host_cols(&split)), (20, 20));
    wait(|| {
        cx.run_until_parked();
        update(cx, handle, |view, _| view_tab_ids(view)[0].len() == 2)
    });
    wait(|| {
        cx.update(|cx| {
            handle
                .update(cx, |view, window, cx| {
                    let cell = view.layout_cell_size();
                    view.sync_terminal_size(window, cell, cx);
                })
                .unwrap();
        });
        cx.run_until_parked();
        host_cols(&first) > 20 && host_cols(&split) > 20
    });
    client.shutdown().unwrap();
}
