use super::*;
use gpui_kit::{TestAppContext, WindowBounds, WindowOptions};

/// Record the size before emitting any output or accepting input. Blocking the
/// UI until this file exists keeps a later render/resize from hiding an 80x24
/// startup size (which shell line editors can cache during initialization).
fn size_recording_shell(root: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = root.join("shell");
        std::fs::write(&script, "#!/bin/sh\nstty size > \"$0.size\"\nread line\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script
    }
    #[cfg(windows)]
    {
        let script = root.join("shell.cmd");
        std::fs::write(
            &script,
            "@echo off\r\npowershell.exe -NoLogo -NoProfile -NonInteractive -Command \"[IO.File]::WriteAllText('%~f0.size', ('{0} {1}' -f [Console]::WindowHeight, [Console]::WindowWidth))\"\r\nset /p line=\r\n",
        )
        .unwrap();
        script
    }
}

#[gpui_kit::test]
fn shell_starts_at_the_window_size_before_the_first_render(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    // The second window also exercises opening a terminal again, with another
    // viewport and font size rather than reusing the previous shell's geometry.
    for (width, font_size) in [(1440.0, 18.0), (1000.0, 14.0)] {
        let root = tempfile::tempdir().unwrap();
        let shell = size_recording_shell(root.path());
        let recorded = PathBuf::from(format!("{}.size", shell.display()));
        let config = AppConfig {
            tmux_enabled: false,
            multiplexer_enabled: false,
            native_tab_persistence: false,
            native_layout_autosave: false,
            auto_update: false,
            sidebar_enabled: width > 1200.0,
            tab_bar_position: TabBarPosition::Right,
            auto_hide_tabbar: false,
            padding_x: 12.0,
            padding_y: 8.0,
            shell: Some(shell.to_string_lossy().into_owned()),
            font_size,
            ..Default::default()
        };
        cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.0), px(0.0)),
                        Size {
                            width: px(width),
                            height: px(800.0),
                        },
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    cx.new(|cx| {
                        let mut view = TerminalView::new(window, cx, config);
                        view.owns_persisted_session = false;
                        let deadline = Instant::now() + Duration::from_secs(20);
                        let recorded = loop {
                            if let Ok(text) = std::fs::read_to_string(&recorded) {
                                let values: Vec<u16> = text
                                    .split_whitespace()
                                    .filter_map(|part| part.parse().ok())
                                    .collect();
                                if values.len() == 2 {
                                    break (values[1], values[0]);
                                }
                            }
                            assert!(
                                Instant::now() < deadline,
                                "shell did not record its initial size"
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        };
                        let cell_size = view.calculate_cell_size(window, cx);
                        view.sync_terminal_size(window, cell_size, cx);
                        let fitted = view.active_terminal().unwrap().requested_size();
                        assert!(
                            fitted.cols > 80,
                            "test window must be wider than the default"
                        );
                        assert_eq!(
                            recorded,
                            (fitted.cols, fitted.rows),
                            "shell initialized before the window was measured"
                        );
                        view
                    })
                },
            )
            .unwrap();
        });
    }
}
