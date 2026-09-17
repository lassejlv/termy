#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use anyhow::{Context, Result, anyhow};
use flume::{Receiver, RecvTimeoutError, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use std::io::{Read, Write};
#[cfg(test)]
use termy_tmux_control_core::command::split_control_completion_token;
use termy_tmux_control_core::command::{
    SEND_INPUT_BULK_HEX_BYTES, SEND_INPUT_CHUNKED_HEX_BYTES, SendInputMode, choose_send_input_mode,
    next_control_completion_token, send_keys_hex_command, tmux_command_line,
    tmux_control_command_line,
};
use termy_tmux_control_core::control::{
    ControlRequest, NotificationCoalescer, try_enqueue_control_request,
};

#[cfg(unix)]
use super::launch::spawn_tmux_control_mode;
use super::launch::{
    SessionLaunchPlan, append_working_dir_args, managed_session_window_option_override_commands,
    spawn_tmux_control_mode_via_prefix,
};
use super::session::{self, run_tmux_command_with_socket};
use super::shutdown::{
    is_tmux_missing_client_error, is_tmux_no_server_error, normalize_shutdown_teardown_result,
    run_shutdown_actions,
};
use super::snapshot::{
    PANE_MOUSE_MODE_FORMAT, PANE_SNAPSHOT_FORMAT, WINDOW_SNAPSHOT_FORMAT, parse_pane_mouse_mode,
    parse_snapshot,
};
use termy_tmux_control_core::control::{
    FATAL_EXIT_QUEUE_BOUND, NOTIFICATION_QUEUE_BOUND, PENDING_QUEUE_BOUND, REQUEST_QUEUE_BOUND,
    spawn_control_threads,
};
use termy_tmux_control_core::payload::{
    capture_full_pane_args, capture_pane_range_args, sanitize_tmux_payload, unescape_tmux_payload,
};
use termy_tmux_control_core::types::{
    TmuxControlError, TmuxLaunchTarget, TmuxNotification, TmuxPaneMouseMode, TmuxRuntimeConfig,
    TmuxSessionSummary, TmuxShutdownMode, TmuxSnapshot, TmuxSocketTarget,
};

pub struct TmuxClient {
    tmux_binary: String,
    /// Argv prefix used to reach the tmux binary for out-of-band commands
    /// (`["wsl.exe", "-e"]`, `["ssh", "myhost"]`); empty for local tmux.
    command_prefix: Vec<String>,
    /// Whether out-of-band tmux commands (spawning the tmux binary directly,
    /// possibly through `command_prefix`) can reach the same server as the
    /// control channel. False for stream-based clients, whose transport is
    /// opaque to this process.
    out_of_band_commands_available: bool,
    session_name: String,
    socket_target: TmuxSocketTarget,
    show_active_pane_border: bool,
    control_client_pid: u32,
    shutdown_mode_on_drop: TmuxShutdownMode,
    shutdown_in_progress: AtomicBool,
    shutdown_completed: AtomicBool,
    request_tx: Sender<ControlRequest>,
    notifications_rx: Receiver<TmuxNotification>,
    fatal_exit_rx: Receiver<Option<String>>,
}

fn new_window_after_args<'a>(
    target_window_id: &'a str,
    working_dir: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["new-window", "-a", "-t", target_window_id];
    append_working_dir_args(&mut args, working_dir);
    args
}

fn split_vertical_args<'a>(pane_id: &'a str, working_dir: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["split-window", "-h", "-t", pane_id];
    append_working_dir_args(&mut args, working_dir);
    args
}

fn split_horizontal_args<'a>(pane_id: &'a str, working_dir: Option<&'a str>) -> Vec<&'a str> {
    let mut args = vec!["split-window", "-t", pane_id];
    append_working_dir_args(&mut args, working_dir);
    args
}

impl TmuxClient {
    fn launch_plan(config: &TmuxRuntimeConfig) -> SessionLaunchPlan {
        super::launch::launch_plan(config)
    }

    pub fn new(
        config: TmuxRuntimeConfig,
        cols: u16,
        rows: u16,
        initial_working_dir: Option<&str>,
        event_wakeup_tx: Option<Sender<()>>,
    ) -> Result<Self> {
        let launch_plan = Self::launch_plan(&config);
        let enforce_managed_session_ui = matches!(&config.launch, TmuxLaunchTarget::Managed { .. });
        if launch_plan.session_name.trim().is_empty() {
            return Err(anyhow!("tmux session name cannot be empty"));
        }

        if !config.command_prefix.is_empty() {
            let (child, child_stdin, child_stdout) = spawn_tmux_control_mode_via_prefix(
                &config,
                &launch_plan.socket_target,
                launch_plan.session_name.as_str(),
                launch_plan.attach_existing,
            )?;
            // The local child (wsl.exe, ssh, ...) is not the tmux client the
            // server sees, so its pid cannot be matched against
            // `#{client_pid}`; pid 0 routes detach through the control
            // channel instead.
            return Self::from_spawned_control_client(
                config,
                launch_plan,
                enforce_managed_session_ui,
                child,
                0,
                child_stdin,
                child_stdout,
                cols,
                rows,
                event_wakeup_tx,
            );
        }

        #[cfg(unix)]
        {
            let (child, child_stdin, child_stdout) = spawn_tmux_control_mode(
                &config,
                &launch_plan.socket_target,
                launch_plan.session_name.as_str(),
                launch_plan.attach_existing,
                initial_working_dir,
            )?;
            let control_client_pid = child.id();
            Self::from_spawned_control_client(
                config,
                launch_plan,
                enforce_managed_session_ui,
                child,
                control_client_pid,
                child_stdin,
                child_stdout,
                cols,
                rows,
                event_wakeup_tx,
            )
        }
        #[cfg(not(unix))]
        {
            let _ = initial_working_dir;
            Err(anyhow!(
                "tmux control mode on this platform requires tmux_command_prefix \
                 (for example `wsl.exe -e` or `ssh myhost`)"
            ))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn from_spawned_control_client<W, R>(
        config: TmuxRuntimeConfig,
        launch_plan: SessionLaunchPlan,
        enforce_managed_session_ui: bool,
        child: std::process::Child,
        control_client_pid: u32,
        child_stdin: W,
        child_stdout: R,
        cols: u16,
        rows: u16,
        event_wakeup_tx: Option<Sender<()>>,
    ) -> Result<Self>
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        let (request_tx, request_rx) = flume::bounded::<ControlRequest>(REQUEST_QUEUE_BOUND);
        let (pending_tx, pending_rx) = flume::bounded(PENDING_QUEUE_BOUND);
        let (notifications_tx, notifications_rx) =
            flume::bounded::<TmuxNotification>(NOTIFICATION_QUEUE_BOUND);
        let (fatal_exit_tx, fatal_exit_rx) =
            flume::bounded::<Option<String>>(FATAL_EXIT_QUEUE_BOUND);

        spawn_control_threads(
            Some(child),
            child_stdin,
            child_stdout,
            request_rx,
            pending_tx,
            pending_rx,
            notifications_tx,
            fatal_exit_tx,
            event_wakeup_tx,
        );

        let client = Self {
            tmux_binary: config.binary,
            command_prefix: config.command_prefix,
            out_of_band_commands_available: true,
            session_name: launch_plan.session_name,
            socket_target: launch_plan.socket_target,
            show_active_pane_border: config.show_active_pane_border,
            control_client_pid,
            shutdown_mode_on_drop: launch_plan.shutdown_mode_on_drop,
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_completed: AtomicBool::new(false),
            request_tx,
            notifications_rx,
            fatal_exit_rx,
        };
        if enforce_managed_session_ui {
            client.enforce_native_session_ui()?;
        }
        client.set_client_size(cols, rows)?;
        Ok(client)
    }

    /// Create a `TmuxClient` from existing control-mode I/O streams instead of
    /// spawning a local tmux process.
    ///
    /// `stdin`/`stdout` must be connected to a tmux control-mode client
    /// (`tmux -CC`) that is already running — for example over an SSH channel,
    /// in an embedded host, or an in-memory pair for tests. The streams are
    /// wired directly into the existing control worker threads; no local
    /// process is spawned or waited on.
    ///
    /// `tmux_binary` and `socket_target` are still used for out-of-band tmux
    /// commands issued outside the control channel. Because there is no local
    /// child process, shutdown is always downgraded to detach-only regardless
    /// of the requested mode.
    pub fn from_streams<W, R>(
        stdin: W,
        stdout: R,
        session_name: String,
        tmux_binary: String,
        socket_target: TmuxSocketTarget,
        event_wakeup_tx: Option<Sender<()>>,
    ) -> Result<Self>
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        if session_name.trim().is_empty() {
            return Err(anyhow!("tmux session name cannot be empty"));
        }

        let (request_tx, request_rx) = flume::bounded::<ControlRequest>(REQUEST_QUEUE_BOUND);
        let (pending_tx, pending_rx) = flume::bounded(PENDING_QUEUE_BOUND);
        let (notifications_tx, notifications_rx) =
            flume::bounded::<TmuxNotification>(NOTIFICATION_QUEUE_BOUND);
        let (fatal_exit_tx, fatal_exit_rx) =
            flume::bounded::<Option<String>>(FATAL_EXIT_QUEUE_BOUND);

        spawn_control_threads(
            None,
            stdin,
            stdout,
            request_rx,
            pending_tx,
            pending_rx,
            notifications_tx,
            fatal_exit_tx,
            event_wakeup_tx,
        );

        Ok(Self {
            tmux_binary,
            command_prefix: Vec::new(),
            out_of_band_commands_available: false,
            session_name,
            socket_target,
            show_active_pane_border: false,
            control_client_pid: 0,
            shutdown_mode_on_drop: TmuxShutdownMode::DetachOnly,
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_completed: AtomicBool::new(false),
            request_tx,
            notifications_rx,
            fatal_exit_rx,
        })
    }

    pub fn set_client_size(&self, cols: u16, rows: u16) -> Result<()> {
        let size = format!("{cols}x{rows}");
        let command = tmux_command_line(&["refresh-client", "-C", size.as_str()]);
        // `refresh-client -C` operates on the *current control client*.
        // Running it as an out-of-band tmux process can fail with no client
        // context during attach/re-attach; issuing it through the active control
        // channel binds it to the correct client deterministically.
        self.send_control_command_wait(command.as_str())
            .with_context(|| format!("tmux status command failed: {command}"))
            .map(|_| ())
    }

    pub fn send_command(&self, command: &str) -> Result<String> {
        self.send_control_command_wait(command)
            .map(|result| result.output)
    }

    pub fn send_command_async(&self, command: &str) -> Result<()> {
        self.send_control_command_async(command)
    }

    /// Register a format subscription for this control client (tmux 3.4+).
    ///
    /// `what` selects the scope (`%*` all panes, `@*` all windows, `$*` all
    /// sessions, or a specific id); `format` is a tmux format string such as
    /// `#{pane_current_path}`. tmux then emits a `%subscription-changed`
    /// notification (surfaced as [`TmuxNotification::SubscriptionChanged`])
    /// whenever the value changes, capped at roughly once per second.
    ///
    /// Like `refresh-client -C`, `-B` operates on the *current control client*,
    /// so it is issued through the active control channel to bind it
    /// deterministically.
    ///
    /// `name` must be free of spaces and colons: tmux parses the `-B` argument
    /// as `name:what:format` on the first two colons, and the
    /// `%subscription-changed` line is whitespace-delimited, so either character
    /// in `name` would corrupt both the spec and the parsed notification. A name
    /// that violates this rule is rejected with a runtime error; the spec is
    /// issued through the validated control-command path, which also rejects
    /// embedded control bytes in `what`/`format`.
    pub fn subscribe(&self, name: &str, what: &str, format: &str) -> Result<()> {
        if name.contains([':', ' ']) {
            return Err(anyhow!(TmuxControlError::protocol(format!(
                "tmux subscription name must not contain ':' or ' ': {name:?}"
            ))));
        }
        let spec = format!("{name}:{what}:{format}");
        self.run_control_status_args(&["refresh-client", "-B", spec.as_str()])
            .with_context(|| format!("tmux subscribe command failed: refresh-client -B {spec}"))
    }

    pub fn session_name(&self) -> &str {
        self.session_name.as_str()
    }

    pub fn poll_notifications(&self) -> Vec<TmuxNotification> {
        let mut coalescer = NotificationCoalescer::with_output_byte_limit(usize::MAX);
        for notification in self.notifications_rx.try_iter() {
            // Draining already-bounded channel contents cannot grow unbounded;
            // this second-stage coalescing keeps refresh/output bursts from
            // triggering redraw storms in the UI event loop.
            coalescer.push(notification);
        }

        if let Some(exit_reason) = self.fatal_exit_rx.try_iter().last() {
            return vec![TmuxNotification::Exit(exit_reason)];
        }

        coalescer.drain()
    }

    pub fn refresh_snapshot(&self) -> Result<TmuxSnapshot> {
        let windows_output = self.run_control_capture_args(&[
            "list-windows",
            "-t",
            self.session_name.as_str(),
            "-F",
            // Use an explicit non-printable field separator and escaped string fields
            // so tabs/newlines inside names cannot corrupt record framing.
            WINDOW_SNAPSHOT_FORMAT,
        ])?;

        let panes_output = self.run_control_capture_args(&[
            "list-panes",
            "-s",
            "-t",
            self.session_name.as_str(),
            "-F",
            PANE_SNAPSHOT_FORMAT,
        ])?;

        parse_snapshot(&self.session_name, &windows_output, &panes_output)
    }

    /// Query tmux's authoritative mouse mode for one pane. This is a short,
    /// bounded fallback for context-menu decisions when cached mode metadata is
    /// stale; it must not turn a right click into a multi-second UI stall.
    pub fn query_pane_mouse_mode(&self, pane_id: &str) -> Result<TmuxPaneMouseMode> {
        const MOUSE_MODE_QUERY_TIMEOUT: Duration = Duration::from_millis(250);
        let command = tmux_control_command_line(&[
            "display-message",
            "-p",
            "-t",
            pane_id,
            PANE_MOUSE_MODE_FORMAT,
        ])
        .map_err(|error| {
            anyhow!(TmuxControlError::protocol(format!(
                "refusing unsafe tmux mouse mode query: {error}"
            )))
        })?;
        let response = self
            .send_control_command_wait_with_timeout(command.as_str(), MOUSE_MODE_QUERY_TIMEOUT)
            .with_context(|| format!("tmux pane mouse mode query failed: {command}"))?;
        let unescaped = unescape_tmux_payload(response.output.as_bytes());
        let value = String::from_utf8(unescaped)
            .with_context(|| format!("tmux pane mouse mode is not valid UTF-8: {command}"))?;
        parse_pane_mouse_mode(value.trim())
    }

    pub fn new_window_after(
        &self,
        target_window_id: &str,
        working_dir: Option<&str>,
    ) -> Result<()> {
        // Use explicit insert-after targeting so Termy tab creation is deterministic:
        // new tabs always appear immediately to the right of the active tab.
        let args = new_window_after_args(target_window_id, working_dir);
        self.run_control_status_args(&args)
    }

    pub fn kill_window(&self, window_id: &str) -> Result<()> {
        self.run_control_status_args(&["kill-window", "-t", window_id])
    }

    /// Move a live tmux window through the destination control connection, so
    /// removing the source session's last window cannot interrupt the reply.
    pub fn move_window_from(&self, source: &Self, window_id: &str) -> Result<()> {
        if !self.out_of_band_commands_available
            || !source.out_of_band_commands_available
            || self.tmux_binary != source.tmux_binary
            || self.command_prefix != source.command_prefix
            || self.socket_target != source.socket_target
        {
            return Err(anyhow!(
                "Tabs can only move between sessions on the same tmux server"
            ));
        }
        if self.session_name == source.session_name {
            return Err(anyhow!("These windows already share the same tmux session"));
        }
        let target = format!("{}:", self.session_name);
        self.run_control_status_args(&["move-window", "-d", "-s", window_id, "-t", &target])
    }

    pub fn rename_window(&self, window_id: &str, name: &str) -> Result<()> {
        self.run_control_status_args(&["rename-window", "-t", window_id, name])
    }

    pub fn previous_window(&self) -> Result<()> {
        self.run_control_status_args(&["previous-window", "-t", self.session_name.as_str()])
    }

    pub fn next_window(&self) -> Result<()> {
        self.run_control_status_args(&["next-window", "-t", self.session_name.as_str()])
    }

    pub fn select_window(&self, window_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-window", "-t", window_id])
    }

    pub fn swap_windows(&self, src: &str, dst: &str) -> Result<()> {
        self.run_control_status_args(&["swap-window", "-s", src, "-t", dst])
    }

    pub fn split_vertical(&self, pane_id: &str, working_dir: Option<&str>) -> Result<()> {
        let args = split_vertical_args(pane_id, working_dir);
        self.run_control_status_args(&args)
    }

    pub fn split_horizontal(&self, pane_id: &str, working_dir: Option<&str>) -> Result<()> {
        let args = split_horizontal_args(pane_id, working_dir);
        self.run_control_status_args(&args)
    }

    pub fn close_pane(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["kill-pane", "-t", pane_id])
    }

    pub fn focus_pane_left(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-pane", "-L", "-t", pane_id])
    }

    pub fn focus_pane_right(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-pane", "-R", "-t", pane_id])
    }

    pub fn focus_pane_up(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-pane", "-U", "-t", pane_id])
    }

    pub fn focus_pane_down(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-pane", "-D", "-t", pane_id])
    }

    pub fn select_pane(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["select-pane", "-t", pane_id])
    }

    pub fn resize_pane_left(&self, pane_id: &str, cells: u16) -> Result<()> {
        let cells = cells.to_string();
        self.run_control_status_args(&["resize-pane", "-L", "-t", pane_id, cells.as_str()])
    }

    pub fn resize_pane_right(&self, pane_id: &str, cells: u16) -> Result<()> {
        let cells = cells.to_string();
        self.run_control_status_args(&["resize-pane", "-R", "-t", pane_id, cells.as_str()])
    }

    pub fn resize_pane_up(&self, pane_id: &str, cells: u16) -> Result<()> {
        let cells = cells.to_string();
        self.run_control_status_args(&["resize-pane", "-U", "-t", pane_id, cells.as_str()])
    }

    pub fn resize_pane_down(&self, pane_id: &str, cells: u16) -> Result<()> {
        let cells = cells.to_string();
        self.run_control_status_args(&["resize-pane", "-D", "-t", pane_id, cells.as_str()])
    }

    pub fn toggle_pane_zoom(&self, pane_id: &str) -> Result<()> {
        self.run_control_status_args(&["resize-pane", "-Z", "-t", pane_id])
    }

    pub fn detach_client(&self) -> Result<()> {
        if self.control_client_pid == 0 {
            return match self.run_control_status_args(&["detach-client"]) {
                Ok(()) => Ok(()),
                Err(e) if is_tmux_missing_client_error(&e) || is_tmux_no_server_error(&e) => Ok(()),
                Err(e) => Err(e),
            };
        }

        let Some(client_name) = self.resolve_control_client_name_by_pid()? else {
            return Ok(());
        };

        match self.run_control_status_args(&["detach-client", "-t", client_name.as_str()]) {
            Ok(()) => Ok(()),
            Err(error) => {
                if is_tmux_missing_client_error(&error) || is_tmux_no_server_error(&error) {
                    // The targeted control client already disappeared between list and detach.
                    return Ok(());
                }
                Err(error)
            }
        }
    }

    pub fn shutdown(&self, mode: TmuxShutdownMode) -> Result<()> {
        self.run_shutdown_attempt(|| self.shutdown_impl(mode))
    }

    pub fn shutdown_default(&self) -> Result<()> {
        self.shutdown(self.shutdown_mode_on_drop)
    }

    fn run_shutdown_attempt<F>(&self, shutdown_action: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        if self.shutdown_completed.load(Ordering::Acquire) {
            return Ok(());
        }

        if self.shutdown_in_progress.swap(true, Ordering::AcqRel) {
            return Ok(());
        }

        // Another thread may have completed shutdown between our optimistic
        // completion check and acquiring the in-progress flag.
        if self.shutdown_completed.load(Ordering::Acquire) {
            self.shutdown_in_progress.store(false, Ordering::Release);
            return Ok(());
        }

        let result = shutdown_action();
        if result.is_ok() {
            self.shutdown_completed.store(true, Ordering::Release);
        }
        // Failures must unlock retries so drop/reconnect can attempt cleanup again.
        self.shutdown_in_progress.store(false, Ordering::Release);
        result
    }

    fn shutdown_impl(&self, mode: TmuxShutdownMode) -> Result<()> {
        // Stream-based clients cannot reach the tmux binary out-of-band.
        // Downgrade to detach-only to avoid running local tmux commands against
        // a potentially unrelated or nonexistent session.
        let mode = if self.out_of_band_commands_available {
            mode
        } else {
            TmuxShutdownMode::DetachOnly
        };
        run_shutdown_actions(
            mode,
            self.session_name.as_str(),
            || {
                self.detach_client().with_context(|| {
                    format!(
                        "failed to detach tmux control client for session '{}'",
                        self.session_name
                    )
                })
            },
            || {
                // Isolated managed sessions are ephemeral. Teardown is always attempted in
                // this mode, even if detach failed, so stale sessions cannot accumulate.
                let teardown_result = Self::kill_session(
                    &self.command_prefix,
                    self.tmux_binary.as_str(),
                    self.socket_target.clone(),
                    self.session_name.as_str(),
                );
                normalize_shutdown_teardown_result(self.session_name.as_str(), teardown_result)
            },
        )
    }

    fn resolve_control_client_name_by_pid(&self) -> Result<Option<String>> {
        if self.control_client_pid == 0 {
            return Ok(None);
        }
        let output =
            match self.run_tmux_command(&["list-clients", "-F", "#{client_pid}\t#{client_name}"]) {
                Ok(output) => output,
                Err(error) => {
                    if is_tmux_no_server_error(&error) {
                        return Ok(None);
                    }
                    return Err(error).context("failed to resolve tmux control client identity");
                }
            };

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
            let Some((pid_raw, client_name_raw)) = line.split_once('\t') else {
                return Err(anyhow!(
                    "invalid tmux list-clients row while resolving control client pid {}: '{}'",
                    self.control_client_pid,
                    line
                ));
            };
            let pid = pid_raw.trim().parse::<u32>().with_context(|| {
                format!(
                    "invalid tmux client pid '{}' while resolving control client pid {}",
                    pid_raw.trim(),
                    self.control_client_pid
                )
            })?;

            if pid != self.control_client_pid {
                continue;
            }

            let client_name = client_name_raw.trim();
            if client_name.is_empty() {
                return Err(anyhow!(
                    "tmux client pid {} has empty client_name",
                    self.control_client_pid
                ));
            }
            return Ok(Some(client_name.to_string()));
        }

        Ok(None)
    }

    pub fn send_input(&self, pane_id: &str, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }

        let (mode, _) = choose_send_input_mode(bytes.len());
        match mode {
            SendInputMode::ChunkedHex => {
                for chunk in bytes.chunks(SEND_INPUT_CHUNKED_HEX_BYTES) {
                    let command = send_keys_hex_command(pane_id, chunk);
                    self.send_control_command_async(command.as_str())?;
                }
            }
            SendInputMode::Bulk => {
                // Large pastes must honor per-command completion so bounded control queues
                // cannot be flooded by thousands of async send-keys requests.
                for chunk in bytes.chunks(SEND_INPUT_BULK_HEX_BYTES) {
                    let command = send_keys_hex_command(pane_id, chunk);
                    let _ = self.send_control_command_wait(command.as_str())?;
                }
            }
        }

        Ok(())
    }

    pub fn capture_pane(&self, pane_id: &str, max_rows: usize) -> Result<Vec<u8>> {
        // Hydration capture must stay bounded to avoid expensive full-history
        // scans that can time out during reattach on large tmux histories.
        let start_row = format!("-{}", max_rows.max(1));
        let args = capture_full_pane_args(pane_id, start_row.as_str());
        let out = self.run_control_capture_args(&args)?;
        Ok(finalize_capture_payload(&out, true))
    }

    /// Capture a bounded scrollback range of a pane with caller-chosen end line
    /// and wrap handling. `start_row`/`end_row` are tmux `capture-pane -S/-E`
    /// line specifiers (negative = history, `0` = top of the visible screen,
    /// `-` = the extreme). With `join_wraps` off, every captured line maps 1:1
    /// to a grid row, so a caller can splice captured history against a live
    /// grid line-exact. Unlike `capture_pane`, this does not assume `-E -`.
    pub fn capture_pane_range(
        &self,
        pane_id: &str,
        start_row: &str,
        end_row: &str,
        join_wraps: bool,
    ) -> Result<Vec<u8>> {
        let args = capture_pane_range_args(pane_id, start_row, end_row, join_wraps);
        let out = self.run_control_capture_args(&args)?;
        Ok(finalize_capture_payload(&out, false))
    }

    pub fn verify_tmux_version(
        command_prefix: &[String],
        binary: &str,
        minimum_major: u8,
        minimum_minor: u8,
    ) -> Result<()> {
        session::verify_tmux_version(command_prefix, binary, minimum_major, minimum_minor)
    }

    pub fn list_sessions(
        command_prefix: &[String],
        binary: &str,
        socket_target: TmuxSocketTarget,
    ) -> Result<Vec<TmuxSessionSummary>> {
        session::list_sessions(command_prefix, binary, socket_target)
    }

    pub fn rename_session(
        command_prefix: &[String],
        binary: &str,
        socket_target: TmuxSocketTarget,
        current_session_name: &str,
        next_session_name: &str,
    ) -> Result<()> {
        session::rename_session(
            command_prefix,
            binary,
            socket_target,
            current_session_name,
            next_session_name,
        )
    }

    pub fn kill_session(
        command_prefix: &[String],
        binary: &str,
        socket_target: TmuxSocketTarget,
        session_name: &str,
    ) -> Result<()> {
        session::kill_session(command_prefix, binary, socket_target, session_name)
    }

    fn enqueue_control_request(&self, request: ControlRequest) -> Result<()> {
        try_enqueue_control_request(&self.request_tx, request).map_err(anyhow::Error::new)
    }

    fn send_control_command_async(&self, command: &str) -> Result<()> {
        self.enqueue_control_request(ControlRequest {
            command: command.to_string(),
            completion_token: next_control_completion_token(),
            response_tx: None,
        })
    }

    fn send_control_command_wait_with_timeout(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<termy_tmux_control_core::control::ControlCommandResult> {
        let (response_tx, response_rx) = flume::bounded(1);
        self.enqueue_control_request(ControlRequest {
            command: command.to_string(),
            completion_token: next_control_completion_token(),
            response_tx: Some(response_tx),
        })?;

        let response = match response_rx.recv_timeout(timeout) {
            Ok(response) => response,
            Err(RecvTimeoutError::Timeout) => {
                return Err(anyhow!(TmuxControlError::channel(format!(
                    "timed out waiting for command completion after {timeout:?}: '{command}'"
                ))));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!(TmuxControlError::channel(format!(
                    "tmux control worker channel disconnected before command completion: '{command}'"
                ))));
            }
        };
        response.map_err(anyhow::Error::new)
    }

    fn send_control_command_wait(
        &self,
        command: &str,
    ) -> Result<termy_tmux_control_core::control::ControlCommandResult> {
        const CONTROL_COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
        self.send_control_command_wait_with_timeout(command, CONTROL_COMMAND_TIMEOUT)
    }

    fn run_control_capture_args(&self, args: &[&str]) -> Result<String> {
        const CONTROL_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
        let command = tmux_control_command_line(args).map_err(|error| {
            anyhow!(TmuxControlError::protocol(format!(
                "refusing unsafe tmux capture command: {error}"
            )))
        })?;
        let response = self
            .send_control_command_wait_with_timeout(command.as_str(), CONTROL_CAPTURE_TIMEOUT)
            .with_context(|| format!("tmux capture command failed: {command}"))?;
        let unescaped = unescape_tmux_payload(response.output.as_bytes());
        String::from_utf8(unescaped)
            .with_context(|| format!("tmux capture response is not valid UTF-8: {command}"))
    }

    fn run_control_status_args(&self, args: &[&str]) -> Result<()> {
        let command = tmux_control_command_line(args).map_err(|error| {
            anyhow!(TmuxControlError::protocol(format!(
                "refusing unsafe tmux status command: {error}"
            )))
        })?;
        self.send_control_command_wait(command.as_str())
            .with_context(|| format!("tmux status command failed: {command}"))
            .map(|_| ())
    }

    fn run_tmux_command(&self, args: &[&str]) -> Result<std::process::Output> {
        run_tmux_command_with_socket(
            &self.command_prefix,
            self.tmux_binary.as_str(),
            &self.socket_target,
            args,
        )
        .with_context(|| {
            format!(
                "failed to execute tmux command via '{}': {}",
                self.tmux_binary,
                tmux_command_line(args)
            )
        })
    }

    fn enforce_native_session_ui(&self) -> Result<()> {
        let session = self.session_name.as_str();
        let all_windows_target = format!("{session}:*");

        self.run_control_status_args(&[
            "set-environment",
            "-t",
            session,
            "TERMY_SHELL_INTEGRATION",
            "0",
        ])
        .context("failed to disable termy shell integration env in tmux session")?;
        self.run_control_status_args(&[
            "set-environment",
            "-u",
            "-t",
            session,
            "TERMY_TAB_TITLE_PREFIX",
        ])
        .context("failed to clear termy shell title prefix env in tmux session")?;
        self.run_control_status_args(&["set-environment", "-t", session, "PROMPT_EOL_MARK", ""])
            .context("failed to disable zsh prompt eol mark env in tmux session")?;

        self.run_control_status_args(&["set-option", "-q", "-t", session, "status", "off"])
            .context("failed to disable tmux status line for managed session")?;
        // Managed persistence must survive detach->reattach even when the user's tmux
        // config enables `destroy-unattached`, which would otherwise tear down the
        // session as soon as Termy's control client detaches.
        self.run_control_status_args(&[
            "set-option",
            "-q",
            "-t",
            session,
            "destroy-unattached",
            "off",
        ])
        .context("failed to disable destroy-unattached for managed session")?;
        for command in managed_session_window_option_override_commands(
            all_windows_target.as_str(),
            self.show_active_pane_border,
        ) {
            self.run_control_status_args(&command).with_context(|| {
                let option_key = command.get(4).copied().unwrap_or("<missing-option-key>");
                let option_value = command.get(5).copied().unwrap_or("<missing-option-value>");
                format!(
                    "failed to apply tmux managed-session window option override '{}={}' (command: {})",
                    option_key,
                    option_value,
                    tmux_command_line(&command),
                )
            })?;
        }
        self.run_control_status_args(&["refresh-client"])
            .context("failed to refresh tmux client after managed-session ui configuration")?;

        Ok(())
    }
}

fn trim_trailing_line_terminators(mut bytes: &[u8]) -> &[u8] {
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

/// Decode and normalize a control-mode capture payload. When `trim_trailing_rows`
/// is set, trailing blank captured rows are dropped (full-pane hydration wants a
/// compact buffer); bounded range captures pass `false` to keep the promised 1:1
/// captured-line-to-grid-row mapping intact, including trailing blank rows.
fn finalize_capture_payload(out: &str, trim_trailing_rows: bool) -> Vec<u8> {
    let bytes = out.as_bytes();
    let payload = if trim_trailing_rows {
        trim_trailing_line_terminators(bytes)
    } else {
        bytes
    };
    sanitize_tmux_payload(unescape_tmux_payload(payload))
}

impl Drop for TmuxClient {
    fn drop(&mut self) {
        if let Err(error) = self.shutdown_default() {
            let action = match self.shutdown_mode_on_drop {
                TmuxShutdownMode::DetachOnly => "detach tmux control client",
                TmuxShutdownMode::DetachAndTeardownSession => {
                    "detach tmux control client and teardown managed session"
                }
            };
            eprintln!(
                "Termy shutdown warning: failed to {} '{}': {}",
                action, self.session_name, error
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::Cell;
    use termy_tmux_control_core::control::coalescer::signal_fatal_exit;

    fn test_tmux_client(shutdown_mode_on_drop: TmuxShutdownMode) -> TmuxClient {
        let (request_tx, _request_rx) = flume::bounded::<ControlRequest>(1);
        let (_notifications_tx, notifications_rx) = flume::bounded::<TmuxNotification>(1);
        let (_fatal_exit_tx, fatal_exit_rx) = flume::bounded::<Option<String>>(1);
        TmuxClient {
            tmux_binary: "tmux".to_string(),
            command_prefix: Vec::new(),
            out_of_band_commands_available: false,
            session_name: "test-session".to_string(),
            socket_target: TmuxSocketTarget::DedicatedTermy,
            show_active_pane_border: false,
            control_client_pid: 0,
            shutdown_mode_on_drop,
            shutdown_in_progress: AtomicBool::new(false),
            shutdown_completed: AtomicBool::new(false),
            request_tx,
            notifications_rx,
            fatal_exit_rx,
        }
    }

    #[test]
    fn subscribe_rejects_name_with_colon() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let error = client
            .subscribe("bad:name", "%*", "#{pane_current_path}")
            .expect_err("colon in name must be rejected");
        assert!(error.to_string().contains("must not contain"));
    }

    #[test]
    fn subscribe_rejects_name_with_space() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let error = client
            .subscribe("bad name", "%*", "#{pane_current_path}")
            .expect_err("space in name must be rejected");
        assert!(error.to_string().contains("must not contain"));
    }

    #[test]
    fn subscribe_rejects_control_bytes_in_format() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let error = client
            .subscribe("p_all", "%*", "fmt\nrun-shell")
            .expect_err("control byte must be rejected");
        // The validated path rejects the embedded newline; the cause is carried in
        // the error chain, so inspect the full chain rather than the outer context.
        assert!(format!("{error:#}").contains("refusing unsafe"));
    }

    #[test]
    fn poll_notifications_prioritizes_dedicated_fatal_exit_signal() {
        let (notifications_tx, notifications_rx) = flume::bounded::<TmuxNotification>(4);
        let (fatal_exit_tx, fatal_exit_rx) = flume::bounded::<Option<String>>(1);
        notifications_tx
            .send(TmuxNotification::Output {
                pane_id: "%1".to_string(),
                bytes: b"stale".to_vec(),
            })
            .expect("queue stale output");
        notifications_tx
            .send(TmuxNotification::NeedsRefresh)
            .expect("queue stale refresh");
        signal_fatal_exit(&fatal_exit_tx, Some("control-mode failure".to_string()));

        let mut client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        client.notifications_rx = notifications_rx;
        client.fatal_exit_rx = fatal_exit_rx;
        client.shutdown_completed.store(true, Ordering::Release);
        let notifications = client.poll_notifications();
        assert_eq!(
            notifications,
            vec![TmuxNotification::Exit(Some(
                "control-mode failure".to_string()
            ))]
        );
    }

    #[test]
    fn shutdown_latch_resets_after_failed_attempt() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let attempts = Cell::new(0usize);

        let first = client.run_shutdown_attempt(|| {
            attempts.set(attempts.get() + 1);
            Err(anyhow!("forced shutdown failure"))
        });
        assert!(first.is_err());
        assert_eq!(attempts.get(), 1);
        assert!(!client.shutdown_in_progress.load(Ordering::Acquire));
        assert!(!client.shutdown_completed.load(Ordering::Acquire));

        let second = client.run_shutdown_attempt(|| {
            attempts.set(attempts.get() + 1);
            Ok(())
        });
        assert!(second.is_ok());
        assert_eq!(attempts.get(), 2);
        assert!(client.shutdown_completed.load(Ordering::Acquire));
    }

    #[test]
    fn successful_shutdown_keeps_latch_completed() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let attempts = Cell::new(0usize);

        let first = client.run_shutdown_attempt(|| {
            attempts.set(attempts.get() + 1);
            Ok(())
        });
        assert!(first.is_ok());
        assert_eq!(attempts.get(), 1);
        assert!(client.shutdown_completed.load(Ordering::Acquire));

        let second = client.run_shutdown_attempt(|| {
            attempts.set(attempts.get() + 1);
            Err(anyhow!("must not execute after successful shutdown"))
        });
        assert!(second.is_ok());
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn shutdown_retry_after_forced_detach_failure_can_still_teardown() {
        let client = test_tmux_client(TmuxShutdownMode::DetachAndTeardownSession);
        let teardown_attempts = Cell::new(0usize);

        let first = client.run_shutdown_attempt(|| {
            run_shutdown_actions(
                TmuxShutdownMode::DetachAndTeardownSession,
                "test-session",
                || Err(anyhow!("forced detach failure")),
                || {
                    teardown_attempts.set(teardown_attempts.get() + 1);
                    Err(anyhow!("forced teardown failure"))
                },
            )
        });
        assert!(first.is_err());
        assert_eq!(teardown_attempts.get(), 1);
        assert!(!client.shutdown_completed.load(Ordering::Acquire));

        let second = client.run_shutdown_attempt(|| {
            run_shutdown_actions(
                TmuxShutdownMode::DetachAndTeardownSession,
                "test-session",
                || Ok(()),
                || {
                    teardown_attempts.set(teardown_attempts.get() + 1);
                    Ok(())
                },
            )
        });
        assert!(second.is_ok());
        assert_eq!(teardown_attempts.get(), 2);
        assert!(client.shutdown_completed.load(Ordering::Acquire));
    }

    #[test]
    fn control_channel_ordering_completes_only_after_token_suffix() {
        let token = "__termy_cmd_done_77";
        let partial = "row-1\nrow-2";
        let full = format!("{partial}\n{token}");
        assert_eq!(
            split_control_completion_token(full.as_str(), token),
            Some(partial.to_string())
        );
        assert_eq!(split_control_completion_token(partial, token), None);
    }

    #[test]
    fn backpressure_single_oversized_burst_forces_refresh_warning_without_exit() {
        let mut coalescer = NotificationCoalescer::with_output_byte_limit(8);
        coalescer.push(TmuxNotification::Output {
            pane_id: "%9".to_string(),
            bytes: b"0123456789abcdef".to_vec(),
        });

        let drained = coalescer.drain();
        assert!(
            drained
                .iter()
                .any(|n| matches!(n, TmuxNotification::NeedsRefresh))
        );
        assert!(
            drained
                .iter()
                .any(|n| matches!(n, TmuxNotification::Warning(_)))
        );
        assert!(
            !drained
                .iter()
                .any(|n| matches!(n, TmuxNotification::Exit(_)))
        );
    }

    #[test]
    fn trim_trailing_line_terminators_preserves_trailing_spaces_and_tabs() {
        assert_eq!(
            trim_trailing_line_terminators(b"abc \t\r\n"),
            b"abc \t".as_slice()
        );
        assert_eq!(
            trim_trailing_line_terminators(b"abc \t"),
            b"abc \t".as_slice()
        );
    }

    #[test]
    fn finalize_capture_payload_trims_trailing_blank_rows_for_full_capture() {
        assert_eq!(finalize_capture_payload("A\nB\n\n", true), b"A\r\nB");
    }

    #[test]
    fn finalize_capture_payload_preserves_trailing_blank_rows_for_range_capture() {
        assert_eq!(
            finalize_capture_payload("A\nB\n\n", false),
            b"A\r\nB\r\n\r\n"
        );
    }

    #[test]
    fn new_window_after_args_include_working_directory_when_provided() {
        assert_eq!(
            new_window_after_args("@2", Some("/tmp/project")),
            vec!["new-window", "-a", "-t", "@2", "-c", "/tmp/project"]
        );
    }

    #[test]
    fn new_window_after_args_omit_working_directory_when_missing() {
        assert_eq!(
            new_window_after_args("@2", Some("  ")),
            vec!["new-window", "-a", "-t", "@2"]
        );
    }

    #[test]
    fn split_vertical_args_include_working_directory_when_provided() {
        assert_eq!(
            split_vertical_args("%7", Some("/tmp/project")),
            vec!["split-window", "-h", "-t", "%7", "-c", "/tmp/project"]
        );
    }

    #[test]
    fn split_horizontal_args_omit_working_directory_when_missing() {
        assert_eq!(
            split_horizontal_args("%7", None),
            vec!["split-window", "-t", "%7"]
        );
    }

    #[test]
    fn tmux_window_args_omit_unsafe_working_directories() {
        assert_eq!(
            new_window_after_args("@2", Some("/tmp/project\nrun-shell")),
            vec!["new-window", "-a", "-t", "@2"]
        );
        assert_eq!(
            split_vertical_args("%7", Some("/tmp/#(run-shell)")),
            vec!["split-window", "-h", "-t", "%7"]
        );
    }

    #[test]
    fn send_command_on_disconnected_channel_returns_error() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let result = client.send_command("list-windows");
        assert!(result.is_err());
    }

    #[test]
    fn send_command_async_on_disconnected_channel_returns_error() {
        let client = test_tmux_client(TmuxShutdownMode::DetachOnly);
        let result = client.send_command_async("list-windows");
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn from_streams_creates_client_with_in_memory_streams() {
        let stdin = Vec::<u8>::new();
        let stdout = std::io::Cursor::new(Vec::<u8>::new());

        let client = TmuxClient::from_streams(
            stdin,
            stdout,
            "test-remote".to_string(),
            "tmux".to_string(),
            TmuxSocketTarget::Default,
            None,
        )
        .expect("from_streams should succeed with valid streams");

        assert_eq!(client.session_name(), "test-remote");
        assert_eq!(client.control_client_pid, 0);
        assert!(matches!(
            client.shutdown_mode_on_drop,
            TmuxShutdownMode::DetachOnly
        ));
    }

    #[cfg(unix)]
    #[test]
    fn from_streams_rejects_empty_session_name() {
        let stdin = Vec::<u8>::new();
        let stdout = std::io::Cursor::new(Vec::<u8>::new());

        let result = TmuxClient::from_streams(
            stdin,
            stdout,
            "  ".to_string(),
            "tmux".to_string(),
            TmuxSocketTarget::Default,
            None,
        );

        let Err(error) = result else {
            panic!("empty session name must be rejected");
        };
        assert!(error.to_string().contains("cannot be empty"));
    }

    #[cfg(not(unix))]
    #[test]
    fn new_reports_unsupported_platform_on_non_unix() {
        let result = TmuxClient::new(
            TmuxRuntimeConfig::default(),
            120,
            40,
            None,
            None::<flume::Sender<()>>,
        );
        let Err(error) = result else {
            panic!("non-unix targets must reject tmux runtime startup");
        };
        assert!(
            error
                .to_string()
                .contains("tmux control mode is only supported on unix targets")
        );
    }
}
