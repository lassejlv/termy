//! C ABI over the multiplexer client, so embedders keep terminals alive in a
//! session host: programs outlive the app, and relaunching attaches again.
//! A terminal created or attached here is an ordinary `TermyFfiTerminal`;
//! every terminal function works on it.

use super::*;
use crate::multiplexer::{
    PaneLaunch, SessionClient, connect_or_start, default_root, is_exited_pane, is_host_unavailable,
    is_missing_pane, is_version_mismatch,
};

/// A connection to one session host. Calls block on the host, so hosts should
/// make them off their UI thread; one handle may be used from one thread at a
/// time.
pub struct TermyFfiMultiplexer {
    client: SessionClient,
}

fn status_for(error: &anyhow::Error) -> TermyFfiStatus {
    if is_version_mismatch(error) {
        TermyFfiStatus::VersionMismatch
    } else if is_host_unavailable(error) {
        TermyFfiStatus::HostUnavailable
    } else if is_missing_pane(error) || is_exited_pane(error) {
        TermyFfiStatus::NotFound
    } else {
        log::warn!("multiplexer request failed: {error:#}");
        TermyFfiStatus::WriteFailed
    }
}

/// Connecting fails for reasons beyond a missing host too, such as a session
/// directory that isn't private; all of them mean no host to use.
fn connect_status_for(error: &anyhow::Error) -> TermyFfiStatus {
    match status_for(error) {
        TermyFfiStatus::VersionMismatch => TermyFfiStatus::VersionMismatch,
        _ => TermyFfiStatus::HostUnavailable,
    }
}

/// A terminal handle with a wake channel, around a pane view.
fn attached_terminal(
    attach: impl FnOnce(crate::TerminalWakeupNotifier) -> anyhow::Result<Terminal>,
) -> Result<TermyFfiTerminal, TermyFfiStatus> {
    let (wakeups_tx, wakeups_rx) = bounded(1);
    let sender = wakeups_tx.clone();
    let notifier = crate::TerminalWakeupNotifier::new(move || {
        let _ = sender.try_send(());
    });
    let terminal = attach(notifier).map_err(|error| status_for(&error))?;
    Ok(TermyFfiTerminal {
        terminal,
        wakeups_tx: Some(wakeups_tx),
        wakeups_rx: Some(wakeups_rx),
        kitty_png_exports: Vec::new(),
    })
}

/// Connect to the session host in `root`, starting one with `host_executable`
/// (the `termy-session-host` binary) when none runs. An empty `root` uses the
/// directory Termy and `termy mux` share. Starting a host can take seconds.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_connect(
    root_ptr: *const u8,
    root_len: usize,
    host_executable_ptr: *const u8,
    host_executable_len: usize,
    out_multiplexer: *mut *mut TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if out_multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        let root = match unsafe { optional_utf8(root_ptr, root_len) } {
            Ok(Some(root)) if !root.is_empty() => PathBuf::from(root),
            Ok(_) => match default_root() {
                Some(root) => root,
                None => return TermyFfiStatus::HostUnavailable,
            },
            Err(status) => return status,
        };
        let executable = match unsafe { required_utf8(host_executable_ptr, host_executable_len) } {
            Ok(path) => PathBuf::from(path),
            Err(status) => return status,
        };
        match connect_or_start(&root, &executable) {
            Ok(client) => {
                unsafe {
                    *out_multiplexer = Box::into_raw(Box::new(TermyFfiMultiplexer { client }));
                };
                TermyFfiStatus::Ok
            }
            Err(error) => connect_status_for(&error),
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_free(
    multiplexer: *mut TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        drop(unsafe { Box::from_raw(multiplexer) });
        TermyFfiStatus::Ok
    })
}

/// Start a pane in the host and attach a terminal to it. Options work as for
/// `termy_terminal_new_with_options`; the working directory is resolved here
/// and sent absolute. `out_pane_id` receives the pane's ID (free it with
/// `termy_buffer_free`); keep it to attach again after a relaunch.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_create_terminal(
    multiplexer: *const TermyFfiMultiplexer,
    size: TermyFfiSize,
    options: *const TermyFfiTerminalOptions,
    out_pane_id: *mut TermyFfiBytes,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null()
            || options.is_null()
            || out_pane_id.is_null()
            || out_terminal.is_null()
        {
            return TermyFfiStatus::Null;
        }
        let client = unsafe { &(*multiplexer).client };
        let options = unsafe { *options };
        let (mut config, shell_integration) = if options.config.is_null() {
            (TerminalRuntimeConfig::default(), None)
        } else {
            let loaded = unsafe { &(*options.config).loaded };
            (
                loaded.runtime_config.clone(),
                Some(tab_title_shell_integration_from_config(&loaded.app_config)),
            )
        };
        let working_directory = match unsafe {
            optional_utf8(options.working_directory_ptr, options.working_directory_len)
        } {
            Ok(value) => value,
            Err(status) => return status,
        };
        let command = match unsafe {
            optional_utf8(options.startup_command_ptr, options.startup_command_len)
        } {
            Ok(value) => value.filter(|command| !command.trim().is_empty()),
            Err(status) => return status,
        };
        match unsafe { env_vars_from_ffi(options.env_vars_ptr, options.env_vars_len) } {
            Ok(environment) => config.environment.extend(environment),
            Err(status) => return status,
        }
        let launch = PaneLaunch {
            size: size.into(),
            working_directory: PaneLaunch::client_working_directory(
                working_directory,
                crate::WorkingDirFallback::Home,
            ),
            shell_integration,
            config,
            launch: command.map(|command| crate::TerminalLaunch::ShellCommand(command.to_owned())),
            environment: Some(PaneLaunch::client_environment()),
        };
        let mut pane_id = None;
        let terminal = match attached_terminal(|notifier| {
            let (id, terminal) = client.create(launch, Some(notifier))?;
            pane_id = Some(id);
            Ok(terminal)
        }) {
            Ok(terminal) => terminal,
            Err(status) => return status,
        };
        unsafe {
            *out_pane_id = ffi_bytes_from_string(pane_id.unwrap_or_default());
            *out_terminal = Box::into_raw(Box::new(terminal));
        }
        TermyFfiStatus::Ok
    })
}

/// Attach a terminal to a running pane, such as one created before the app
/// last quit. Fails with `TERMY_FFI_NOT_FOUND` for a pane that is gone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_attach_terminal(
    multiplexer: *const TermyFfiMultiplexer,
    pane_id_ptr: *const u8,
    pane_id_len: usize,
    out_terminal: *mut *mut TermyFfiTerminal,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_terminal.is_null() {
            return TermyFfiStatus::Null;
        }
        let client = unsafe { &(*multiplexer).client };
        let id = match unsafe { required_utf8(pane_id_ptr, pane_id_len) } {
            Ok(id) => id,
            Err(status) => return status,
        };
        match attached_terminal(|notifier| client.attach(id, Some(notifier))) {
            Ok(terminal) => {
                unsafe { *out_terminal = Box::into_raw(Box::new(terminal)) };
                TermyFfiStatus::Ok
            }
            Err(status) => status,
        }
    })
}

/// The host's panes as a JSON array of objects with `id`, `child_pid`,
/// `title`, `working_directory`, `exited` and `shell`. Free with
/// `termy_buffer_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_list_json(
    multiplexer: *const TermyFfiMultiplexer,
    out_json: *mut TermyFfiBytes,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() || out_json.is_null() {
            return TermyFfiStatus::Null;
        }
        let panes = match unsafe { (*multiplexer).client.list() } {
            Ok(panes) => panes,
            Err(error) => return status_for(&error),
        };
        match serde_json::to_vec(&panes) {
            Ok(json) => {
                unsafe { *out_json = ffi_bytes_from_vec(json) };
                TermyFfiStatus::Ok
            }
            Err(_) => TermyFfiStatus::SerializeFailed,
        }
    })
}

/// End a pane's program and remove the pane. Detaching only, so the program
/// keeps running, is freeing the terminal handle instead.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_close(
    multiplexer: *const TermyFfiMultiplexer,
    pane_id_ptr: *const u8,
    pane_id_len: usize,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        let id = match unsafe { required_utf8(pane_id_ptr, pane_id_len) } {
            Ok(id) => id,
            Err(status) => return status,
        };
        match unsafe { (*multiplexer).client.close(id) } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(error) => status_for(&error),
        }
    })
}

/// End every pane and stop the host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_multiplexer_shutdown(
    multiplexer: *const TermyFfiMultiplexer,
) -> TermyFfiStatus {
    ffi_status_guard(|| {
        if multiplexer.is_null() {
            return TermyFfiStatus::Null;
        }
        match unsafe { (*multiplexer).client.shutdown() } {
            Ok(()) => TermyFfiStatus::Ok,
            Err(error) => status_for(&error),
        }
    })
}
