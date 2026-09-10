use gpui::Window;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Instant,
};

const PROBE_PATH_ENV: &str = "TERMY_LAUNCH_PROBE_FILE";
static PROCESS_STARTED_AT: OnceLock<Instant> = OnceLock::new();
static STAGES: Mutex<Vec<(&'static str, u128)>> = Mutex::new(Vec::new());

pub(crate) fn mark_process_start() {
    let _ = PROCESS_STARTED_AT.set(Instant::now());
}

pub(crate) fn record_stage(name: &'static str) {
    if env::var_os(PROBE_PATH_ENV).is_none() {
        return;
    }
    if let Some(started_at) = PROCESS_STARTED_AT.get()
        && let Ok(mut stages) = STAGES.lock()
    {
        stages.push((name, started_at.elapsed().as_micros()));
    }
}

pub(crate) fn record_after_next_frame(window: &mut Window) {
    let Some(path) = env::var_os(PROBE_PATH_ENV)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
    else {
        return;
    };
    let started_at = *PROCESS_STARTED_AT.get_or_init(Instant::now);
    window.on_next_frame(move |window, _cx| {
        let viewport = window.viewport_size();
        let width = f32::from(viewport.width).round().max(0.0) as u32;
        let height = f32::from(viewport.height).round().max(0.0) as u32;
        if width == 0 || height == 0 {
            return;
        }
        let elapsed_ms = started_at.elapsed().as_millis();
        let mut contents = probe_contents(std::process::id(), elapsed_ms, width, height);
        if let Ok(stages) = STAGES.lock() {
            use std::fmt::Write;
            for (name, micros) in stages.iter() {
                let _ = writeln!(contents, "stage_{name}_us={micros}");
            }
        }
        if let Err(error) = write_probe(&path, &contents) {
            log::warn!("Failed to write launch probe '{}': {error}", path.display());
        }
    });
    window.refresh();
}

fn probe_contents(pid: u32, elapsed_ms: u128, width: u32, height: u32) -> String {
    format!(
        "pid={pid}\nvisible=true\nterminal_ready=true\nelapsed_ms={elapsed_ms}\n\
         content_width={width}\ncontent_height={height}\n"
    )
}

fn write_probe(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create probe directory '{}': {error}",
                parent.display()
            )
        })?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, contents)
        .map_err(|error| format!("failed to write '{}': {error}", temporary.display()))?;
    match fs::rename(&temporary, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            let _ = fs::remove_file(&temporary);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(format!("failed to publish '{}': {error}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_probe_reports_gpui_frame_and_terminal_readiness() {
        let contents = probe_contents(42, 123, 1100, 720);
        assert!(contents.contains("pid=42\n"));
        assert!(contents.contains("visible=true\n"));
        assert!(contents.contains("terminal_ready=true\n"));
        assert!(contents.contains("elapsed_ms=123\n"));
        assert!(contents.contains("content_width=1100\ncontent_height=720\n"));
    }

    #[test]
    fn launch_probe_write_is_atomic_and_first_writer_wins() {
        let temp = tempfile::tempdir().expect("temp dir");
        let path = temp.path().join("nested/ready.txt");
        write_probe(&path, "first").expect("first probe");
        write_probe(&path, "second").expect("duplicate probe");
        assert_eq!(fs::read_to_string(path).expect("probe contents"), "first");
    }
}
