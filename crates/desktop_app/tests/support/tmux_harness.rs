use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::{self, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use termy::terminal_ui::TmuxClient;

const TEST_SOCKET_NAME: &str = "termy";
static TMUX_TMPDIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tmux_env_lock() -> &'static Mutex<()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| Mutex::new(()))
}

pub(crate) fn tmux_test_guard(binary: &str) -> IsolatedTmuxEnvGuard {
    IsolatedTmuxEnvGuard::new(binary)
}

pub(crate) fn tmux_test_binary() -> String {
    if let Ok(binary) = env::var("TERMY_TEST_TMUX_BIN") {
        return binary;
    }

    static WRAPPER_PATH: OnceLock<PathBuf> = OnceLock::new();
    let path = WRAPPER_PATH.get_or_init(|| {
        let path = PathBuf::from("/tmp").join(format!("tmux-e2e-wrapper-{}", process::id()));
        let script = "\
#!/usr/bin/env bash\n\
set -euo pipefail\n\
args=(\"$@\")\n\
is_control_start=0\n\
has_attach=0\n\
for ((i=0; i<${#args[@]}; i++)); do\n\
  arg=\"${args[$i]}\"\n\
  if [[ \"$arg\" == \"-CC\" && $((i+1)) -lt ${#args[@]} && \"${args[$((i+1))]}\" == \"new-session\" ]]; then\n\
    is_control_start=1\n\
  fi\n\
  if [[ \"$arg\" == \"-A\" ]]; then\n\
    has_attach=1\n\
  fi\n\
done\n\
if [[ $is_control_start -eq 1 ]]; then\n\
  if [[ $has_attach -eq 0 ]]; then\n\
    args+=(\"-A\")\n\
  fi\n\
fi\n\
exec tmux -f /dev/null \"${args[@]}\"\n";
        fs::write(&path, script).expect("failed to write tmux test wrapper");
        let mut permissions = fs::metadata(&path)
            .expect("failed to stat tmux test wrapper")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).expect("failed to chmod tmux test wrapper");
        path
    });

    path.to_string_lossy().into_owned()
}

pub(crate) fn tmux_preflight(binary: &str) {
    TmuxClient::verify_tmux_version(&[], binary, 3, 3).unwrap_or_else(|error| {
        panic!("tmux integration preflight failed for binary '{binary}': {error}")
    });
}

fn kill_test_server(binary: &str, tmux_tmpdir: &Path) {
    let _ = Command::new(binary)
        .env_remove("TMUX")
        .env("TMUX_TMPDIR", tmux_tmpdir)
        .arg("-L")
        .arg(TEST_SOCKET_NAME)
        .arg("kill-server")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

pub(crate) struct IsolatedTmuxEnvGuard {
    // Hold the process-wide environment lock for the entire lifetime of this guard,
    // so every unsafe set_var/remove_var call is serialized and restored atomically.
    _env_lock: std::sync::MutexGuard<'static, ()>,
    previous_tmux: Option<OsString>,
    previous_tmux_tmpdir: Option<OsString>,
    tmux_tmpdir: PathBuf,
    binary: String,
}

impl IsolatedTmuxEnvGuard {
    fn new(binary: &str) -> Self {
        let env_lock = tmux_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous_tmux = env::var_os("TMUX");
        let previous_tmux_tmpdir = env::var_os("TMUX_TMPDIR");

        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let sequence = TMUX_TMPDIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let unique_suffix = format!("{}-{now_ns}-{sequence}", process::id());
        let tmux_tmpdir = PathBuf::from("/tmp").join(format!("ttmx-{unique_suffix}"));
        fs::create_dir_all(&tmux_tmpdir).expect("failed to create isolated TMUX_TMPDIR");

        // Keep integration tests isolated from user/session state that may also
        // use `-L termy`. Clearing TMUX avoids nested-session hints from parent shells.
        unsafe { env::remove_var("TMUX") };
        unsafe { env::set_var("TMUX_TMPDIR", &tmux_tmpdir) };

        Self {
            _env_lock: env_lock,
            previous_tmux,
            previous_tmux_tmpdir,
            tmux_tmpdir,
            binary: binary.to_string(),
        }
    }
}

impl Drop for IsolatedTmuxEnvGuard {
    fn drop(&mut self) {
        kill_test_server(self.binary.as_str(), self.tmux_tmpdir.as_path());
        let _ = fs::remove_dir_all(&self.tmux_tmpdir);

        if let Some(previous_tmux) = self.previous_tmux.take() {
            unsafe { env::set_var("TMUX", previous_tmux) };
        } else {
            unsafe { env::remove_var("TMUX") };
        }

        if let Some(previous_tmux_tmpdir) = self.previous_tmux_tmpdir.take() {
            unsafe { env::set_var("TMUX_TMPDIR", previous_tmux_tmpdir) };
        } else {
            unsafe { env::remove_var("TMUX_TMPDIR") };
        }
    }
}
