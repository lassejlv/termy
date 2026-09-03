//! C ABI for the engine-owned pseudo-terminal lifecycle.

#![allow(
    clippy::wildcard_imports,
    reason = "the ABI constants and record types intentionally share one generated-style namespace"
)]

use std::{
    ffi::c_void,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use engine::pty::{PtyCommand, PtyEvent, PtySession, pty_size};

use crate::{
    error::{FfiError, ffi_status},
    types::*,
    util::{
        required_mut, required_ref, slice_view, to_u64, utf8_from_view, views_from_raw, write_out,
    },
};

#[derive(Debug)]
pub struct TmonPty {
    session: PtySession,
    output: Vec<u8>,
}

#[derive(Debug, Default)]
struct CallbackState {
    ready: bool,
    pending: Vec<PtyEvent>,
}

#[derive(Debug)]
struct CallbackGate {
    callback: Option<TmonPtyEventCallback>,
    user_data: usize,
    ready: AtomicBool,
    state: Mutex<CallbackState>,
}

impl CallbackGate {
    fn new(callback: Option<TmonPtyEventCallback>, user_data: *mut c_void) -> Self {
        Self {
            callback,
            user_data: user_data as usize,
            ready: AtomicBool::new(false),
            state: Mutex::new(CallbackState::default()),
        }
    }

    fn push(&self, event: PtyEvent) {
        if self.callback.is_none() {
            return;
        }
        if self.ready.load(Ordering::Acquire) {
            self.dispatch(event);
            return;
        }

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.ready {
            drop(state);
            self.dispatch(event);
        } else {
            state.pending.push(event);
        }
    }

    fn activate(&self) {
        let pending = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.ready = true;
            self.ready.store(true, Ordering::Release);
            std::mem::take(&mut state.pending)
        };
        for event in pending {
            self.dispatch(event);
        }
    }

    fn dispatch(&self, event: PtyEvent) {
        let Some(callback) = self.callback else {
            return;
        };
        match event {
            PtyEvent::Wake => {
                let event = TmonPtyEvent {
                    kind: TMON_PTY_WAKE,
                    ..TmonPtyEvent::default()
                };
                // SAFETY: This is the foreign callback supplied to `tmon_pty_spawn`. The
                // event is borrowed only for the duration of the call.
                unsafe { callback(self.user_data as *mut c_void, &raw const event) };
            }
            PtyEvent::Exit { code, signal } => {
                let (data, has_data) = signal
                    .as_deref()
                    .map_or((TmonByteSlice::empty(), 0), |signal| {
                        (slice_view(signal.as_bytes()), 1)
                    });
                let event = TmonPtyEvent {
                    kind: TMON_PTY_EXIT,
                    exit_code: code,
                    data,
                    has_data,
                };
                // SAFETY: See the wake-event callback above. `signal` remains alive through the
                // callback.
                unsafe { callback(self.user_data as *mut c_void, &raw const event) };
            }
            PtyEvent::ReadError(error) => {
                let event = TmonPtyEvent {
                    kind: TMON_PTY_READ_ERROR,
                    data: slice_view(error.as_bytes()),
                    has_data: 1,
                    ..TmonPtyEvent::default()
                };
                // SAFETY: See the wake-event callback above. `error` remains alive through the
                // callback.
                unsafe { callback(self.user_data as *mut c_void, &raw const event) };
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tmon_pty_config_default() -> TmonPtyConfig {
    TmonPtyConfig::default()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_spawn(
    config: *const TmonPtyConfig,
    callback: Option<TmonPtyEventCallback>,
    user_data: *mut c_void,
    out_pty: *mut *mut TmonPty,
) -> u32 {
    ffi_status(|| {
        if out_pty.is_null() {
            return Err(FfiError::null("out_pty"));
        }
        // SAFETY: `config` points to one readable configuration record.
        let config = unsafe { required_ref(config, "config")? };
        // SAFETY: The program view is borrowed for command construction only.
        let program = unsafe { utf8_from_view(config.program, "config.program")? };
        if program.is_empty() {
            return Err(FfiError::invalid("config.program must not be empty"));
        }
        // SAFETY: The argument array has `argument_count` readable views.
        let argument_views =
            unsafe { views_from_raw(config.arguments, config.argument_count, "config.arguments")? };
        let mut arguments = Vec::with_capacity(argument_views.len());
        for argument in argument_views.iter().copied() {
            // SAFETY: Each argument view is borrowed only during this call.
            let argument = unsafe { utf8_from_view(argument, "config.arguments[]")? };
            arguments.push(argument.to_owned());
        }

        let mut command = PtyCommand::new(program).with_arguments(&arguments);
        if config.has_working_directory != 0 {
            // SAFETY: The working-directory view is borrowed only during this call.
            let directory =
                unsafe { utf8_from_view(config.working_directory, "config.working_directory")? };
            if directory.is_empty() {
                return Err(FfiError::invalid(
                    "config.working_directory must not be empty when present",
                ));
            }
            command = command.with_working_directory(PathBuf::from(directory));
        }

        let gate = Arc::new(CallbackGate::new(callback, user_data));
        let event_gate = Arc::clone(&gate);
        let size = pty_size(
            config.columns,
            config.rows,
            config.cell_width,
            config.cell_height,
        );
        let session = PtySession::spawn(&command, size, move |event| event_gate.push(event))
            .map_err(FfiError::engine)?;
        let handle = Box::into_raw(Box::new(TmonPty {
            session,
            output: Vec::new(),
        }));
        // SAFETY: Null was rejected before spawning; the caller supplies one writable slot.
        unsafe { write_out(out_pty, handle, "out_pty")? };
        // Events emitted during spawn are delivered only after the handle is visible to the host.
        gate.activate();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_free(pty: *mut TmonPty) -> u32 {
    if pty.is_null() {
        return TMON_OK;
    }
    ffi_status(|| {
        // SAFETY: Ownership of the handle is transferred back exactly once by the ABI contract.
        drop(unsafe { Box::from_raw(pty) });
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_drain_output(
    pty: *mut TmonPty,
    out_bytes: *mut TmonByteSlice,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Exclusive handle access is part of the ABI contract because the borrowed output
        // storage is mutated.
        let pty = unsafe { required_mut(pty, "pty")? };
        pty.session
            .drain_output_into(&mut pty.output)
            .map_err(FfiError::engine)?;
        let view = slice_view(&pty.output);
        // SAFETY: The output points to one writable byte view.
        unsafe { write_out(out_bytes, view, "out_bytes") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_write(
    pty: *const TmonPty,
    bytes: *const u8,
    length: usize,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is enough because `PtySession` synchronizes its writer.
        let pty = unsafe { required_ref(pty, "pty")? };
        // SAFETY: The caller provides `length` readable bytes for this call.
        let bytes = unsafe { crate::util::bytes_from_raw(bytes, length, "bytes")? };
        pty.session.write(bytes).map_err(FfiError::engine)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_resize(
    pty: *const TmonPty,
    columns: usize,
    rows: usize,
    cell_width: f32,
    cell_height: f32,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is enough because `PtySession` synchronizes resize state.
        let pty = unsafe { required_ref(pty, "pty")? };
        pty.session
            .resize(pty_size(columns, rows, cell_width, cell_height))
            .map_err(FfiError::engine)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_child_pid(
    pty: *const TmonPty,
    out_pid: *mut TmonOptionalU32,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let pid = unsafe { required_ref(pty, "pty")? }.session.child_pid();
        let pid = TmonOptionalU32 {
            value: pid.unwrap_or_default(),
            has_value: u8::from(pid.is_some()),
        };
        // SAFETY: The output points to one writable optional integer.
        unsafe { write_out(out_pid, pid, "out_pid") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_buffer_metrics(
    pty: *const TmonPty,
    out_metrics: *mut TmonPtyBufferMetrics,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let metrics = unsafe { required_ref(pty, "pty")? }
            .session
            .buffer_metrics()
            .map_err(FfiError::engine)?;
        let metrics = TmonPtyBufferMetrics {
            pending_bytes: to_u64(metrics.pending_bytes),
            pending_capacity_bytes: to_u64(metrics.pending_capacity_bytes),
            high_water_bytes: to_u64(metrics.high_water_bytes),
            bytes_buffered: metrics.bytes_buffered,
            bytes_drained: metrics.bytes_drained,
            drain_calls: metrics.drain_calls,
            producer_waits: metrics.producer_waits,
            buffer_growths: metrics.buffer_growths,
            wake_events: metrics.wake_events,
        };
        // SAFETY: The output points to one writable metrics record.
        unsafe { write_out(out_metrics, metrics, "out_metrics") }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tmon_pty_io_metrics(
    pty: *const TmonPty,
    out_metrics: *mut TmonPtyIoMetrics,
) -> u32 {
    ffi_status(|| {
        // SAFETY: Shared handle access is sufficient for this query.
        let metrics = unsafe { required_ref(pty, "pty")? }.session.io_metrics();
        let metrics = TmonPtyIoMetrics {
            resize_requests: metrics.resize_requests,
            resize_ioctls: metrics.resize_ioctls,
            resize_suppressed: metrics.resize_suppressed,
        };
        // SAFETY: The output points to one writable metrics record.
        unsafe { write_out(out_metrics, metrics, "out_metrics") }
    })
}
