#![cfg(unix)]

use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use termy_core::multiplexer::{PaneLaunch, SessionClient, connect_or_start};
use termy_core::*;

#[track_caller]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn text(terminal: &Terminal) -> String {
    terminal
        .render_read(true)
        .cells
        .iter()
        .map(|cell| cell.text.as_str())
        .collect()
}

fn launch(script: &str, root: &Path) -> PaneLaunch {
    PaneLaunch {
        size: TerminalSize {
            cols: 40,
            rows: 8,
            ..TerminalSize::default()
        },
        working_directory: Some(root.to_string_lossy().into_owned()),
        shell_integration: None,
        config: TerminalRuntimeConfig::default(),
        launch: Some(TerminalLaunch::Program {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                script.into(),
                "session-test".into(),
                root.to_string_lossy().into_owned(),
            ],
        }),
    }
}

const SCRIPT: &str = r#"
stty -echo
(while :; do printf x >> "$1/ticks"; sleep 0.05; done) &
ticker=$!
trap 'kill "$ticker" 2>/dev/null' EXIT HUP TERM
printf 'MAIN-SCREEN\r\n'
printf '\033[?1049h\033[2J\033[H\033[?2004h\033[?1003h\033[?1006h\033[?1h\033[?25l\033[31mALT-SCREEN\r\n'
printf '\033]8;;https://example.com\033\\LINK\033]8;;\033\\\r\n'
printf '\033_Ga=T,f=32,s=1,v=1,i=7,q=2;AQID/w==\033\\'
count=0
while IFS= read -r value; do
    count=$((count + 1))
    case "$value" in
        partial) printf '\033[38;2;1;2;'; touch "$1/partial" ;;
        finish) printf '3mQ\r\ncount=%s\r\n' "$count" ;;
        main) printf '\033[?1049l\r\ncount=%s\r\n' "$count" ;;
        *) printf '\r\ncount=%s:%s\r\n' "$count" "$value" ;;
    esac
done
"#;

// This test is also launched as a separate executable by the parent test.
// Each phase exits the complete client process, including all UI-side threads.
#[test]
fn child_client() {
    let Ok(root) = std::env::var("TERMY_MUX_TEST_ROOT") else {
        return;
    };
    let root = Path::new(&root);
    let phase = std::env::var("TERMY_MUX_TEST_PHASE").unwrap();
    let executable = std::env::var_os("TERMY_MUX_TEST_HOST_BINARY").map_or_else(
        || Path::new(env!("CARGO_BIN_EXE_termy-session-host")).to_owned(),
        std::path::PathBuf::from,
    );
    let client = connect_or_start(root, &executable).unwrap();
    if phase == "create" {
        let (id, terminal) = client.create(launch(SCRIPT, root), None).unwrap();
        wait_until(|| text(&terminal).contains("ALT-SCREEN"));
        terminal.write(b"first\n");
        wait_until(|| text(&terminal).contains("count=1:first"));
        terminal.write(b"partial\n");
        wait_until(|| root.join("partial").exists());
        let (history_id, history) = client.create(launch("stty -echo; seq 1 300; printf 'HISTORY-READY'; while IFS= read -r line; do printf '%s' \"$line\"; done", root), None).unwrap();
        wait_until(|| text(&history).contains("HISTORY-READY"));
        client
            .set_layout(format!(
                "{{\"tabs\":[\"{id}\",\"{history_id}\"],\"active\":1}}"
            ))
            .unwrap();
        fs::write(
            root.join("ids"),
            format!("{id}\n{history_id}\n{}", terminal.child_pid().unwrap()),
        )
        .unwrap();
    } else {
        let ids = fs::read_to_string(root.join("ids")).unwrap();
        let mut ids = ids.lines();
        let id = ids.next().unwrap();
        let history_id = ids.next().unwrap();
        let pid: u32 = ids.next().unwrap().parse().unwrap();
        let terminal = client.attach(id, None).unwrap();
        assert_eq!(terminal.child_pid(), Some(pid));
        assert!(terminal.alternate_screen_mode());
        assert!(terminal.bracketed_paste_mode());
        assert!(terminal.mouse_mode().report_motion);
        assert!(terminal.mouse_mode().sgr_encoding);
        assert_eq!(
            terminal.keyboard_mode(),
            TerminalKeyboardMode::from_flags(true, false, false, false, false, false)
        );
        assert!(terminal.cursor_state().is_none());
        assert_eq!(
            terminal.hyperlink_at(1, 0).unwrap().target,
            "https://example.com"
        );
        let graphics = terminal.kitty_graphics_placements();
        assert_eq!(graphics.len(), 1);
        assert_eq!(graphics[0].image_id, 7);
        assert_eq!((graphics[0].image.width, graphics[0].image.height), (1, 1));
        assert!(graphics[0].image.png().starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(text(&terminal).contains("ALT-SCREEN"));
        let red = terminal
            .render_read(true)
            .cells
            .into_iter()
            .find(|cell| cell.text == "A")
            .unwrap();
        assert_eq!(red.foreground, TerminalRenderColor::Indexed(1));
        terminal.write(b"finish\n");
        wait_until(|| text(&terminal).contains("count=3"));
        let q = terminal
            .render_read(true)
            .cells
            .into_iter()
            .find(|cell| cell.text == "Q")
            .unwrap();
        assert_eq!(
            q.foreground,
            TerminalRenderColor::Rgb(TerminalColor { r: 1, g: 2, b: 3 })
        );
        terminal.write(b"main\n");
        wait_until(|| !terminal.alternate_screen_mode());
        assert!(text(&terminal).contains("MAIN-SCREEN"));
        wait_until(|| text(&terminal).contains("count=4"));

        let history = client.attach(history_id, None).unwrap();
        assert!(history.scroll_state().1 >= 290);
        assert!(
            !history
                .search_with_options(
                    "^42$",
                    TermySearchOptions {
                        regex: true,
                        case_sensitive: true
                    }
                )
                .is_empty()
        );
        let bounds = history.line_bounds();
        let mut copied = String::new();
        history.visit_line_cells(bounds.0, bounds.1, |_, _, _, cell| {
            copied.push_str(cell.text.as_str());
        });
        assert!(copied.contains("42"));
        assert!(history.scroll_display(200));
        wait_until(|| history.scroll_state().0 == 200);
        assert!(history.scroll_to_bottom());
        wait_until(|| history.scroll_state().0 == 0);
        assert!(client.layout().unwrap().unwrap().contains(history_id));
        client.close(id).unwrap();
        client.close(history_id).unwrap();
        wait_until(|| client.list().unwrap().is_empty());
        wait_until(|| {
            // SAFETY: signal zero only probes the already-observed child PID.
            unsafe { libc::kill(pid as i32, 0) != 0 }
        });
    }
}

#[test]
fn processes_and_terminal_state_survive_client_process_exit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("host");
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            if let Ok(client) = SessionClient::connect(self.0) {
                let _ = client.shutdown();
            }
        }
    }
    let _cleanup = Cleanup(root.as_path());
    let run = |phase| {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_client", "--nocapture"])
            .env("TERMY_MUX_TEST_ROOT", root.as_path())
            .env("TERMY_MUX_TEST_PHASE", phase)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "client {phase} failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run("create");
    let before = fs::metadata(root.as_path().join("ticks")).unwrap().len();
    // The creator process has exited. The shell's subprocess must continue
    // writing while no app or client is connected.
    wait_until(|| fs::metadata(root.as_path().join("ticks")).unwrap().len() >= before + 3);
    run("restore");
}
