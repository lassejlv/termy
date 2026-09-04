//! Release-only native Metal performance harness.

// Work samples intentionally copy counter snapshots so deltas cannot observe later mutation.
#![allow(clippy::large_types_passed_by_value)]

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use engine::{
    SelectionPoint, TERMINAL_SNAPSHOT_VERSION, Terminal, TerminalConfig, TerminalMemoryStats,
    TerminalMetrics,
    pty::{PtyCommand, pty_size},
};
use mux::{Client as MuxClient, PROTOCOL_VERSION, TerminalSize};
use render::{MetalRenderer, RenderStatus, RendererConfig, RendererFrameTimings, RendererMetrics};
use serde::Serialize;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowAttributes, WindowId},
};

use crate::terminal_window_attributes;

pub(crate) const BENCHMARK_ARGUMENT: &str = "--benchmark-metal";

const REPORT_VERSION: u16 = 3;
const DEFAULT_SAMPLES: usize = 30;
const IDLE_OBSERVATION: Duration = Duration::from_millis(250);
const MUX_TIMEOUT: Duration = Duration::from_secs(2);
const OCCLUSION_TIMEOUT: Duration = Duration::from_mins(2);

type Stage = (&'static str, fn(&FrameSample) -> u64);

pub(crate) fn run(arguments: Vec<OsString>) -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("the native Metal benchmark must be run with --release");
    }
    let config = BenchmarkConfig::parse(arguments)?;
    let event_loop = EventLoop::new().context("creating benchmark event loop")?;
    let mut application = BenchmarkApplication::new(config);
    event_loop
        .run_app(&mut application)
        .context("running native Metal benchmark")?;
    if let Some(error) = application.error.take() {
        return Err(error);
    }
    Ok(())
}

#[derive(Debug)]
struct BenchmarkConfig {
    output: PathBuf,
    samples: usize,
}

impl BenchmarkConfig {
    fn parse(arguments: Vec<OsString>) -> Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_secs();
        let mut output = PathBuf::from(format!("performance/results/tmon-metal-{timestamp}.json"));
        let mut samples = DEFAULT_SAMPLES;
        let mut arguments = arguments.into_iter();
        while let Some(argument) = arguments.next() {
            match argument.to_str() {
                Some("--output") => {
                    output = PathBuf::from(arguments.next().context("--output requires a path")?);
                }
                Some("--samples") => {
                    let value = arguments
                        .next()
                        .context("--samples requires a positive integer")?;
                    samples = value
                        .to_str()
                        .context("--samples must be UTF-8")?
                        .parse()
                        .context("parsing --samples")?;
                    if samples == 0 {
                        bail!("--samples must be greater than zero");
                    }
                }
                Some("--help" | "-h") => {
                    println!("Usage: tmon {BENCHMARK_ARGUMENT} [--samples N] [--output PATH]");
                    std::process::exit(0);
                }
                _ => bail!(
                    "unknown Metal benchmark argument {}",
                    argument.to_string_lossy()
                ),
            }
        }
        Ok(Self { output, samples })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Workload {
    ColdAscii,
    SparseTyping,
    OutputScroll,
    BurstyCompiler,
    RapidTui,
    HistoryScroll,
    DenseCjkEmoji,
    BoxBraille,
    CursorBlink,
    SelectionDrag,
    Resize,
    SurfaceOnlyResize,
    ScaleFactor,
    SurfaceRecreate,
    NativeTabSwitch,
    MultiplexerOutput,
}

impl Workload {
    const ALL: [Self; 16] = [
        Self::ColdAscii,
        Self::SparseTyping,
        Self::OutputScroll,
        Self::BurstyCompiler,
        Self::RapidTui,
        Self::HistoryScroll,
        Self::DenseCjkEmoji,
        Self::BoxBraille,
        Self::CursorBlink,
        Self::SelectionDrag,
        Self::Resize,
        Self::SurfaceOnlyResize,
        Self::ScaleFactor,
        Self::SurfaceRecreate,
        Self::NativeTabSwitch,
        Self::MultiplexerOutput,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::ColdAscii => "cold_ascii",
            Self::SparseTyping => "sparse_shell_typing",
            Self::OutputScroll => "one_line_full_screen_scroll",
            Self::BurstyCompiler => "bursty_compiler_output",
            Self::RapidTui => "rapid_full_screen_tui",
            Self::HistoryScroll => "history_scroll",
            Self::DenseCjkEmoji => "dense_cjk_emoji",
            Self::BoxBraille => "box_braille_heavy",
            Self::CursorBlink => "cursor_blink",
            Self::SelectionDrag => "selection_drag",
            Self::Resize => "resize",
            Self::SurfaceOnlyResize => "surface_only_resize",
            Self::ScaleFactor => "scale_factor_rebuild",
            Self::SurfaceRecreate => "surface_recreate",
            Self::NativeTabSwitch => "native_tab_switch",
            Self::MultiplexerOutput => "multiplexer_output",
        }
    }

    const fn samples(self, requested: usize) -> usize {
        if matches!(self, Self::ColdAscii) {
            1
        } else {
            requested
        }
    }

    const fn needs_warmup(self) -> bool {
        !matches!(self, Self::ColdAscii)
    }
}

#[derive(Debug)]
enum HarnessState {
    Frames,
    Idle {
        started: Instant,
        presented_before: u64,
        text_prepares_before: u64,
        uploads_before: u64,
    },
    Occluded,
    Inactive,
    Finished,
}

#[derive(Debug)]
struct PendingFrame {
    workload: Workload,
    iteration: usize,
    record: bool,
    atlas_state: &'static str,
    pipeline_started: Instant,
    mux_wake_to_drain_ns: u64,
    mux_drain_decode_ns: u64,
    terminal_feed_ns: u64,
    frame_extraction_ns: u64,
    terminal_before: TerminalMetrics,
    renderer_before: RendererMetrics,
}

#[derive(Debug)]
struct MuxFixture {
    client: MuxClient,
    tab_id: u64,
    directory: PathBuf,
}

impl MuxFixture {
    fn new(size: TerminalSize) -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("tmon-metal-benchmark-{}", std::process::id()));
        let socket = directory.join("multiplexer.sock");
        let executable = std::env::current_exe().context("locating benchmark executable")?;
        let command =
            PtyCommand::new("/bin/sh").with_arguments(["-c", "stty -echo; exec /bin/cat"]);
        let mut client = MuxClient::connect_or_spawn(&socket, &executable)?;
        let restore = client.attach(&command, size, 1_000, 100)?;
        Ok(Self {
            client,
            tab_id: restore.active_tab_id,
            directory,
        })
    }

    fn output(&mut self, payload: &[u8]) -> Result<(Vec<u8>, u64, u64)> {
        let wake_started = Instant::now();
        self.client.write(self.tab_id, payload)?;
        let deadline = wake_started + MUX_TIMEOUT;
        let mut drain_decode_ns = 0_u64;
        loop {
            let drain_started = Instant::now();
            let batch = self.client.drain()?;
            drain_decode_ns = drain_decode_ns.saturating_add(duration_ns(drain_started.elapsed()));
            let bytes = batch
                .outputs
                .into_iter()
                .filter(|output| output.tab_id == self.tab_id)
                .flat_map(|output| output.bytes)
                .collect::<Vec<_>>();
            if !bytes.is_empty() {
                return Ok((bytes, duration_ns(wake_started.elapsed()), drain_decode_ns));
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for multiplexer benchmark output");
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

impl Drop for MuxFixture {
    fn drop(&mut self) {
        let _ = self.client.shutdown_daemon();
        for _ in 0..20 {
            if !self.directory.join("multiplexer.sock").exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[derive(Debug)]
struct BenchmarkApplication {
    config: BenchmarkConfig,
    state: HarnessState,
    window: Option<Arc<Window>>,
    secondary_window: Option<Arc<Window>>,
    renderer: Option<MetalRenderer>,
    terminal: Terminal,
    mux: Option<MuxFixture>,
    workload_index: usize,
    iteration: usize,
    warming_up: bool,
    pending: Option<PendingFrame>,
    samples: Vec<FrameSample>,
    observations: Vec<BehaviorObservation>,
    started: Instant,
    starting_rss_bytes: Option<u64>,
    peak_rss_bytes: Option<u64>,
    display_refresh_hz: f64,
    display_scale: f64,
    adapter_name: String,
    grid_columns: usize,
    grid_rows: usize,
    error: Option<anyhow::Error>,
    retry_deadline: Option<Instant>,
    occluded_since: Option<Instant>,
}

impl BenchmarkApplication {
    fn new(config: BenchmarkConfig) -> Self {
        Self {
            config,
            state: HarnessState::Frames,
            window: None,
            secondary_window: None,
            renderer: None,
            terminal: Terminal::new(TerminalConfig::default()),
            mux: None,
            workload_index: 0,
            iteration: 0,
            warming_up: false,
            pending: None,
            samples: Vec::new(),
            observations: Vec::new(),
            started: Instant::now(),
            starting_rss_bytes: current_rss_bytes(),
            peak_rss_bytes: current_rss_bytes(),
            display_refresh_hz: 60.0,
            display_scale: 1.0,
            adapter_name: String::new(),
            grid_columns: 0,
            grid_rows: 0,
            error: None,
            retry_deadline: None,
            occluded_since: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop
                .create_window(
                    terminal_window_attributes()
                        .with_title("Tmon Metal Benchmark")
                        .with_inner_size(LogicalSize::new(1000.0, 640.0)),
                )
                .context("creating benchmark window")?,
        );
        let secondary = Arc::new(
            event_loop
                .create_window(
                    WindowAttributes::default()
                        .with_title("Tmon Metal Benchmark Tab")
                        .with_visible(false)
                        .with_inner_size(LogicalSize::new(1000.0, 640.0)),
                )
                .context("creating benchmark tab window")?,
        );
        self.display_scale = window.scale_factor();
        if let Some(refresh_millihertz) = window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
        {
            self.display_refresh_hz = f64::from(refresh_millihertz) / 1_000.0;
        }
        let mut renderer = pollster::block_on(MetalRenderer::new(
            Arc::clone(&window),
            event_loop,
            RendererConfig::default(),
        ))?;
        renderer.set_measurement_enabled(true);
        let refresh_budget_ns = (1_000_000_000.0 / self.display_refresh_hz.max(1.0)) as u64;
        renderer.set_frame_budget_ns(Some(refresh_budget_ns));
        let (columns, rows) = renderer.grid_dimensions();
        renderer.adapter_name().clone_into(&mut self.adapter_name);
        self.grid_columns = columns;
        self.grid_rows = rows;
        self.terminal = Terminal::new(TerminalConfig {
            columns,
            rows,
            scrollback_limit: 2_000,
        });
        self.window = Some(window);
        self.secondary_window = Some(secondary);
        self.renderer = Some(renderer);
        self.window().set_visible(true);
        self.window().focus_window();
        self.start_workload()?;
        Ok(())
    }

    fn start_workload(&mut self) -> Result<()> {
        let Some(workload) = Workload::ALL.get(self.workload_index).copied() else {
            let metrics = self.renderer().metrics();
            self.state = HarnessState::Idle {
                started: Instant::now(),
                presented_before: metrics.presented_frames,
                text_prepares_before: metrics.text_prepares,
                uploads_before: total_uploads(metrics),
            };
            return Ok(());
        };
        eprintln!("benchmark workload: {}", workload.name());
        self.iteration = 0;
        self.warming_up = workload.needs_warmup();
        self.terminal = Terminal::new(TerminalConfig {
            columns: self.grid_columns,
            rows: self.grid_rows,
            scrollback_limit: 2_000,
        });
        if matches!(workload, Workload::MultiplexerOutput) {
            self.ensure_mux_fixture()?;
            let _ = self
                .mux
                .as_mut()
                .expect("multiplexer fixture exists")
                .output(b"mux warmup\r\n")?;
        }
        self.seed_workload(workload);
        if self.warming_up {
            self.prepare_terminal_frame(workload, 0, false, "warm", 0, 0);
        } else {
            self.prepare_sample(workload)?;
        }
        Ok(())
    }

    fn seed_workload(&mut self, workload: Workload) {
        match workload {
            Workload::ColdAscii | Workload::SparseTyping | Workload::SelectionDrag => {
                for row in 1..=self.grid_rows {
                    self.terminal
                        .feed(format!("\x1b[{row};1H$ printf 'tmon row {row:03}'").as_bytes());
                }
            }
            Workload::OutputScroll | Workload::BurstyCompiler => {
                for row in 0..self.grid_rows {
                    self.terminal
                        .feed(format!("seed output {row:03}\r\n").as_bytes());
                }
            }
            Workload::RapidTui => {
                for row in 1..=self.grid_rows {
                    self.terminal.feed(
                        format!(
                            "\x1b[{row};1H│ task {row:03}                                      │"
                        )
                        .as_bytes(),
                    );
                }
            }
            Workload::HistoryScroll => {
                for row in 0..400 {
                    self.terminal
                        .feed(format!("history row {row:04}\r\n").as_bytes());
                }
            }
            Workload::DenseCjkEmoji => {
                let line = "界語🙂".repeat(self.grid_columns.div_ceil(5));
                for row in 1..=self.grid_rows {
                    self.terminal
                        .feed(format!("\x1b[{row};1H{line}").as_bytes());
                }
            }
            Workload::BoxBraille => {
                let line = "┌─┬─┐│█⣿│└─┴─┘".repeat(self.grid_columns.div_ceil(15));
                for row in 1..=self.grid_rows {
                    self.terminal
                        .feed(format!("\x1b[{row};1H{line}").as_bytes());
                }
            }
            Workload::CursorBlink
            | Workload::Resize
            | Workload::SurfaceOnlyResize
            | Workload::ScaleFactor
            | Workload::SurfaceRecreate
            | Workload::NativeTabSwitch
            | Workload::MultiplexerOutput => {
                self.terminal.feed(b"Tmon native Metal performance fixture");
            }
        }
    }

    fn prepare_sample(&mut self, workload: Workload) -> Result<()> {
        let iteration = self.iteration;
        match workload {
            Workload::ColdAscii => {
                self.prepare_terminal_frame(workload, iteration, true, "cold", 0, 0);
                Ok(())
            }
            Workload::SparseTyping => {
                let row = iteration % self.grid_rows.max(1) + 1;
                let column = iteration % self.grid_columns.saturating_sub(1).max(1) + 1;
                let payload = format!(
                    "\x1b[{row};{column}H{}",
                    char::from(b'a' + (iteration % 26) as u8)
                );
                self.feed_and_prepare(workload, iteration, payload.as_bytes(), 0, 0);
                Ok(())
            }
            Workload::OutputScroll => {
                self.feed_and_prepare(
                    workload,
                    iteration,
                    format!("scroll line {iteration:05}\r\n").as_bytes(),
                    0,
                    0,
                );
                Ok(())
            }
            Workload::BurstyCompiler => {
                let mut payload = String::new();
                for line in 0..8 {
                    let _ = write!(
                        payload,
                        "Compiling crate_{:03} error[E{:04}] at src/lib.rs:{}\r\n",
                        (iteration + line) % 97,
                        1000 + line,
                        iteration + line + 1
                    );
                }
                self.feed_and_prepare(workload, iteration, payload.as_bytes(), 0, 0);
                Ok(())
            }
            Workload::RapidTui => {
                let mut payload = String::new();
                for offset in 0..8 {
                    let row = (iteration + offset) % self.grid_rows.max(1) + 1;
                    let _ = write!(
                        payload,
                        "\x1b[{row};3Htask {row:03}: {:3}% cpu {:02}%",
                        (iteration * 7 + offset) % 101,
                        (iteration * 3 + offset) % 99
                    );
                }
                self.feed_and_prepare(workload, iteration, payload.as_bytes(), 0, 0);
                Ok(())
            }
            Workload::HistoryScroll => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                self.terminal
                    .scroll_display(if iteration.is_multiple_of(2) { 1 } else { -1 });
                self.extract_apply_and_queue(
                    workload,
                    iteration,
                    true,
                    "warm",
                    pipeline_started,
                    0,
                    0,
                    0,
                    terminal_before,
                    renderer_before,
                );
                Ok(())
            }
            Workload::DenseCjkEmoji => {
                let row = iteration % self.grid_rows.max(1) + 1;
                let line = if iteration.is_multiple_of(2) {
                    "界語🙂".repeat(self.grid_columns.div_ceil(5))
                } else {
                    "漢字🚀".repeat(self.grid_columns.div_ceil(5))
                };
                self.feed_and_prepare(
                    workload,
                    iteration,
                    format!("\x1b[{row};1H{line}").as_bytes(),
                    0,
                    0,
                );
                Ok(())
            }
            Workload::BoxBraille => {
                let row = iteration % self.grid_rows.max(1) + 1;
                let line = if iteration.is_multiple_of(2) {
                    "┌─┬─┐│█⣿│└─┴─┘"
                } else {
                    "╭━┯━╮┃▄⣷┃╰━┷━╯"
                };
                self.feed_and_prepare(
                    workload,
                    iteration,
                    format!(
                        "\x1b[{row};1H{}",
                        line.repeat(self.grid_columns.div_ceil(15))
                    )
                    .as_bytes(),
                    0,
                    0,
                );
                Ok(())
            }
            Workload::CursorBlink => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                self.renderer_mut().toggle_cursor_blink();
                self.pending = Some(PendingFrame {
                    workload,
                    iteration,
                    record: true,
                    atlas_state: "warm",
                    pipeline_started,
                    mux_wake_to_drain_ns: 0,
                    mux_drain_decode_ns: 0,
                    terminal_feed_ns: 0,
                    frame_extraction_ns: 0,
                    terminal_before,
                    renderer_before,
                });
                Ok(())
            }
            Workload::SelectionDrag => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                let row = iteration % self.grid_rows.max(1);
                if iteration == 0 {
                    self.terminal
                        .begin_selection(SelectionPoint { column: 0, row });
                }
                self.terminal.update_selection(SelectionPoint {
                    column: (iteration + 3) % self.grid_columns.max(1),
                    row,
                });
                self.extract_apply_and_queue(
                    workload,
                    iteration,
                    true,
                    "warm",
                    pipeline_started,
                    0,
                    0,
                    0,
                    terminal_before,
                    renderer_before,
                );
                Ok(())
            }
            Workload::Resize => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                let base = self.window().inner_size();
                let delta = if iteration.is_multiple_of(2) { 16 } else { 0 };
                let size = PhysicalSize::new(base.width + delta, base.height + delta);
                self.renderer_mut().resize_surface(size);
                let dimensions = self.renderer().grid_dimensions_for_size(size);
                self.terminal.resize(dimensions.0, dimensions.1);
                self.extract_apply_and_queue(
                    workload,
                    iteration,
                    true,
                    "warm",
                    pipeline_started,
                    0,
                    0,
                    0,
                    terminal_before,
                    renderer_before,
                );
                Ok(())
            }
            Workload::SurfaceOnlyResize => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                let base = self.renderer().surface_size();
                let dimensions = self.renderer().grid_dimensions_for_size(base);
                let size = [
                    PhysicalSize::new(base.width.saturating_add(1), base.height),
                    PhysicalSize::new(base.width, base.height.saturating_add(1)),
                    PhysicalSize::new(base.width.saturating_sub(1), base.height),
                    PhysicalSize::new(base.width, base.height.saturating_sub(1)),
                ]
                .into_iter()
                .find(|candidate| {
                    candidate.width > 0
                        && candidate.height > 0
                        && self.renderer().grid_dimensions_for_size(*candidate) == dimensions
                })
                .context("finding a surface-only resize that preserves grid dimensions")?;
                self.renderer_mut().resize_surface(size);
                self.pending = Some(PendingFrame {
                    workload,
                    iteration,
                    record: true,
                    atlas_state: "warm",
                    pipeline_started,
                    mux_wake_to_drain_ns: 0,
                    mux_drain_decode_ns: 0,
                    terminal_feed_ns: 0,
                    frame_extraction_ns: 0,
                    terminal_before,
                    renderer_before,
                });
                Ok(())
            }
            Workload::ScaleFactor => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                let scale_factor = if iteration.is_multiple_of(2) {
                    if self.display_scale >= 1.5 { 1.0 } else { 2.0 }
                } else {
                    self.display_scale
                };
                self.renderer_mut().set_scale_factor(scale_factor);
                let dimensions = self.renderer().grid_dimensions();
                self.terminal.resize(dimensions.0, dimensions.1);
                self.extract_apply_and_queue(
                    workload,
                    iteration,
                    true,
                    "warm",
                    pipeline_started,
                    0,
                    0,
                    0,
                    terminal_before,
                    renderer_before,
                );
                Ok(())
            }
            Workload::SurfaceRecreate => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                self.renderer_mut().recreate_surface()?;
                self.pending = Some(PendingFrame {
                    workload,
                    iteration,
                    record: true,
                    atlas_state: "warm",
                    pipeline_started,
                    mux_wake_to_drain_ns: 0,
                    mux_drain_decode_ns: 0,
                    terminal_feed_ns: 0,
                    frame_extraction_ns: 0,
                    terminal_before,
                    renderer_before,
                });
                Ok(())
            }
            Workload::NativeTabSwitch => {
                let pipeline_started = Instant::now();
                let terminal_before = self.terminal.metrics();
                let renderer_before = self.renderer().metrics();
                let primary = Arc::clone(self.window.as_ref().expect("primary window exists"));
                let secondary = Arc::clone(
                    self.secondary_window
                        .as_ref()
                        .expect("secondary window exists"),
                );
                let target = if iteration.is_multiple_of(2) {
                    secondary.set_visible(true);
                    primary.set_visible(false);
                    secondary
                } else {
                    primary.set_visible(true);
                    secondary.set_visible(false);
                    primary
                };
                self.renderer_mut().retarget_window(target)?;
                self.pending = Some(PendingFrame {
                    workload,
                    iteration,
                    record: true,
                    atlas_state: "warm",
                    pipeline_started,
                    mux_wake_to_drain_ns: 0,
                    mux_drain_decode_ns: 0,
                    terminal_feed_ns: 0,
                    frame_extraction_ns: 0,
                    terminal_before,
                    renderer_before,
                });
                Ok(())
            }
            Workload::MultiplexerOutput => {
                let pipeline_started = Instant::now();
                let payload = format!("mux output {iteration:05}\r\n");
                let (bytes, wake_ns, drain_ns) = self
                    .mux
                    .as_mut()
                    .expect("multiplexer fixture exists")
                    .output(payload.as_bytes())?;
                self.feed_and_prepare_started(
                    workload,
                    iteration,
                    &bytes,
                    wake_ns,
                    drain_ns,
                    pipeline_started,
                );
                Ok(())
            }
        }
    }

    fn ensure_mux_fixture(&mut self) -> Result<()> {
        if self.mux.is_none() {
            let metrics = self.renderer().cell_metrics();
            let size = TerminalSize::from(pty_size(
                self.grid_columns,
                self.grid_rows,
                metrics.width,
                metrics.height,
            ));
            self.mux = Some(MuxFixture::new(size)?);
        }
        Ok(())
    }

    fn feed_and_prepare(
        &mut self,
        workload: Workload,
        iteration: usize,
        payload: &[u8],
        mux_wake_to_drain_ns: u64,
        mux_drain_decode_ns: u64,
    ) {
        self.feed_and_prepare_started(
            workload,
            iteration,
            payload,
            mux_wake_to_drain_ns,
            mux_drain_decode_ns,
            Instant::now(),
        );
    }

    fn feed_and_prepare_started(
        &mut self,
        workload: Workload,
        iteration: usize,
        payload: &[u8],
        mux_wake_to_drain_ns: u64,
        mux_drain_decode_ns: u64,
        pipeline_started: Instant,
    ) {
        let terminal_before = self.terminal.metrics();
        let renderer_before = self.renderer().metrics();
        let feed_started = Instant::now();
        self.terminal.feed(payload);
        let terminal_feed_ns = duration_ns(feed_started.elapsed());
        self.extract_apply_and_queue(
            workload,
            iteration,
            true,
            "warm",
            pipeline_started,
            mux_wake_to_drain_ns,
            mux_drain_decode_ns,
            terminal_feed_ns,
            terminal_before,
            renderer_before,
        );
    }

    fn prepare_terminal_frame(
        &mut self,
        workload: Workload,
        iteration: usize,
        record: bool,
        atlas_state: &'static str,
        mux_wake_to_drain_ns: u64,
        mux_drain_decode_ns: u64,
    ) {
        let terminal_before = self.terminal.metrics();
        let renderer_before = self.renderer().metrics();
        self.extract_apply_and_queue(
            workload,
            iteration,
            record,
            atlas_state,
            Instant::now(),
            mux_wake_to_drain_ns,
            mux_drain_decode_ns,
            0,
            terminal_before,
            renderer_before,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn extract_apply_and_queue(
        &mut self,
        workload: Workload,
        iteration: usize,
        record: bool,
        atlas_state: &'static str,
        pipeline_started: Instant,
        mux_wake_to_drain_ns: u64,
        mux_drain_decode_ns: u64,
        terminal_feed_ns: u64,
        terminal_before: TerminalMetrics,
        renderer_before: RendererMetrics,
    ) {
        let extraction_started = Instant::now();
        let update = self
            .terminal
            .frame_update(!record || matches!(workload, Workload::ColdAscii));
        let frame_extraction_ns = duration_ns(extraction_started.elapsed());
        self.renderer_mut().apply_frame(&update);
        self.pending = Some(PendingFrame {
            workload,
            iteration,
            record,
            atlas_state,
            pipeline_started,
            mux_wake_to_drain_ns,
            mux_drain_decode_ns,
            terminal_feed_ns,
            frame_extraction_ns,
            terminal_before,
            renderer_before,
        });
        self.renderer().window().request_redraw();
    }

    fn rendered(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let status = self.renderer_mut().render()?;
        match status {
            RenderStatus::Retry => {
                self.retry_deadline = Some(Instant::now() + Duration::from_millis(16));
                return Ok(());
            }
            RenderStatus::Occluded => {
                let since = self.occluded_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= OCCLUSION_TIMEOUT {
                    bail!("active benchmark surface stayed occluded for two minutes")
                }
                self.retry_deadline = Some(Instant::now() + Duration::from_millis(16));
                return Ok(());
            }
            RenderStatus::Presented => {}
        }
        self.retry_deadline = None;
        self.occluded_since = None;
        let pending = self
            .pending
            .take()
            .context("presented an untracked benchmark frame")?;
        if pending.record {
            let timings = self
                .renderer_mut()
                .take_last_frame_timings()
                .context("renderer measurement did not produce frame timings")?;
            let sample = FrameSample::new(
                &pending,
                timings,
                self.terminal.metrics(),
                self.renderer().metrics(),
                duration_ns(pending.pipeline_started.elapsed()),
            );
            self.samples.push(sample);
            self.iteration += 1;
        } else {
            self.warming_up = false;
        }
        self.update_peak_rss();
        let workload = Workload::ALL[self.workload_index];
        if self.iteration >= workload.samples(self.config.samples) {
            eprintln!("benchmark complete: {}", workload.name());
            if matches!(workload, Workload::ScaleFactor) {
                let display_scale = self.display_scale;
                self.renderer_mut().set_scale_factor(display_scale);
                let dimensions = self.renderer().grid_dimensions();
                self.grid_columns = dimensions.0;
                self.grid_rows = dimensions.1;
            }
            self.workload_index += 1;
            self.start_workload()?;
        } else {
            self.prepare_sample(workload)?;
        }
        if matches!(self.state, HarnessState::Idle { .. }) {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + IDLE_OBSERVATION));
        }
        Ok(())
    }

    fn finish_idle(&mut self) -> Result<()> {
        let HarnessState::Idle {
            presented_before,
            text_prepares_before,
            uploads_before,
            ..
        } = self.state
        else {
            return Ok(());
        };
        let metrics = self.renderer().metrics();
        self.observations.push(BehaviorObservation {
            name: "idle".to_owned(),
            duration_ns: duration_ns(IDLE_OBSERVATION),
            presented_frames: metrics.presented_frames.saturating_sub(presented_before),
            text_prepares: metrics.text_prepares.saturating_sub(text_prepares_before),
            uploads: total_uploads(metrics).saturating_sub(uploads_before),
            passed: metrics.presented_frames == presented_before
                && metrics.text_prepares == text_prepares_before
                && total_uploads(metrics) == uploads_before,
        });
        self.state = HarnessState::Occluded;
        let before = self.renderer().metrics();
        self.renderer_mut().set_occluded(true);
        let status = self.renderer_mut().render()?;
        let after = self.renderer().metrics();
        self.observations.push(BehaviorObservation {
            name: "occluded".to_owned(),
            duration_ns: 0,
            presented_frames: after
                .presented_frames
                .saturating_sub(before.presented_frames),
            text_prepares: after.text_prepares.saturating_sub(before.text_prepares),
            uploads: total_uploads(after).saturating_sub(total_uploads(before)),
            passed: status == RenderStatus::Occluded
                && after.presented_frames == before.presented_frames
                && after.text_prepares == before.text_prepares
                && total_uploads(after) == total_uploads(before),
        });
        self.renderer_mut().set_occluded(false);
        self.state = HarnessState::Inactive;
        let before = self.renderer().metrics();
        let mut inactive = Terminal::new(TerminalConfig {
            columns: self.grid_columns,
            rows: self.grid_rows,
            scrollback_limit: 100,
        });
        inactive.feed(b"inactive tab output\r\ninactive tab output\r\n");
        let after = self.renderer().metrics();
        self.observations.push(BehaviorObservation {
            name: "inactive_tab".to_owned(),
            duration_ns: 0,
            presented_frames: after
                .presented_frames
                .saturating_sub(before.presented_frames),
            text_prepares: after.text_prepares.saturating_sub(before.text_prepares),
            uploads: total_uploads(after).saturating_sub(total_uploads(before)),
            passed: after == before && inactive.metrics().bytes_fed > 0,
        });
        Ok(())
    }

    fn write_report(&mut self) -> Result<()> {
        self.update_peak_rss();
        let report = BenchmarkReport::new(self);
        if let Some(parent) = self.config.output.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).with_context(|| {
                format!("creating benchmark output directory {}", parent.display())
            })?;
        }
        let encoded = serde_json::to_vec_pretty(&report).context("encoding benchmark JSON")?;
        fs::write(&self.config.output, encoded).with_context(|| {
            format!("writing benchmark report {}", self.config.output.display())
        })?;
        print_human_summary(&report, &self.config.output);
        if !report.release_gate.passed {
            bail!(
                "native Metal release gate failed (coverage: {}; timing: {}; behavior: {})",
                failure_summary(&report.release_gate.coverage_failures),
                failure_summary(&report.release_gate.timing_failures),
                failure_summary(&report.release_gate.behavior_failures)
            );
        }
        self.state = HarnessState::Finished;
        self.mux.take();
        Ok(())
    }

    fn update_peak_rss(&mut self) {
        if let Some(rss) = current_rss_bytes() {
            self.peak_rss_bytes = Some(self.peak_rss_bytes.map_or(rss, |peak| peak.max(rss)));
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: anyhow::Error) {
        self.error = Some(error);
        self.state = HarnessState::Finished;
        event_loop.exit();
    }

    fn renderer(&self) -> &MetalRenderer {
        self.renderer.as_ref().expect("renderer is initialized")
    }

    fn renderer_mut(&mut self) -> &mut MetalRenderer {
        self.renderer.as_mut().expect("renderer is initialized")
    }

    fn window(&self) -> &Window {
        self.window.as_ref().expect("window is initialized")
    }
}

impl ApplicationHandler for BenchmarkApplication {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.initialize(event_loop)
        {
            self.fail(event_loop, error);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        if matches!(event, WindowEvent::CloseRequested) {
            self.fail(event_loop, anyhow::anyhow!("benchmark window was closed"));
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested)
            && self
                .renderer
                .as_ref()
                .is_some_and(|renderer| renderer.window().id() == window_id)
            && matches!(self.state, HarnessState::Frames)
            && let Err(error) = self.rendered(event_loop)
        {
            self.fail(event_loop, error);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.state {
            HarnessState::Idle { started, .. } if started.elapsed() >= IDLE_OBSERVATION => {
                if let Err(error) = self.finish_idle().and_then(|()| self.write_report()) {
                    self.fail(event_loop, error);
                } else {
                    event_loop.exit();
                }
            }
            HarnessState::Idle { started, .. } => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(started + IDLE_OBSERVATION));
            }
            HarnessState::Frames
                if self.pending.is_some()
                    && self.retry_deadline.is_some_and(|at| at <= Instant::now()) =>
            {
                self.retry_deadline = None;
                self.renderer().window().request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            HarnessState::Frames if self.retry_deadline.is_some() => {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    self.retry_deadline.expect("checked above"),
                ));
            }
            HarnessState::Finished => event_loop.exit(),
            HarnessState::Frames | HarnessState::Occluded | HarnessState::Inactive => {}
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct FrameSample {
    workload: String,
    iteration: usize,
    atlas_state: String,
    mux_wake_to_drain_ns: u64,
    mux_drain_decode_ns: u64,
    terminal_feed_ns: u64,
    frame_extraction_ns: u64,
    apply_frame_ns: u64,
    surface_acquire_ns: u64,
    viewport_update_ns: u64,
    glyph_prepare_ns: u64,
    geometry_build_ns: u64,
    geometry_upload_ns: u64,
    encoding_ns: u64,
    submission_ns: u64,
    presentation_ns: u64,
    cpu_preparation_ns: u64,
    renderer_end_to_end_ns: u64,
    pipeline_end_to_end_ns: u64,
    work: WorkCounts,
}

impl FrameSample {
    fn new(
        pending: &PendingFrame,
        timings: RendererFrameTimings,
        terminal_after: TerminalMetrics,
        renderer_after: RendererMetrics,
        pipeline_end_to_end_ns: u64,
    ) -> Self {
        let cpu_preparation_ns = pending
            .mux_drain_decode_ns
            .saturating_add(pending.terminal_feed_ns)
            .saturating_add(pending.frame_extraction_ns)
            .saturating_add(timings.apply_frame_ns)
            .saturating_add(timings.viewport_update_ns)
            .saturating_add(timings.glyph_prepare_ns)
            .saturating_add(timings.geometry_build_ns)
            .saturating_add(timings.geometry_upload_ns)
            .saturating_add(timings.encoding_ns)
            .saturating_add(timings.submission_ns);
        Self {
            workload: pending.workload.name().to_owned(),
            iteration: pending.iteration,
            atlas_state: pending.atlas_state.to_owned(),
            mux_wake_to_drain_ns: pending.mux_wake_to_drain_ns,
            mux_drain_decode_ns: pending.mux_drain_decode_ns,
            terminal_feed_ns: pending.terminal_feed_ns,
            frame_extraction_ns: pending.frame_extraction_ns,
            apply_frame_ns: timings.apply_frame_ns,
            surface_acquire_ns: timings.surface_acquire_ns,
            viewport_update_ns: timings.viewport_update_ns,
            glyph_prepare_ns: timings.glyph_prepare_ns,
            geometry_build_ns: timings.geometry_build_ns,
            geometry_upload_ns: timings.geometry_upload_ns,
            encoding_ns: timings.encoding_ns,
            submission_ns: timings.submission_ns,
            presentation_ns: timings.presentation_ns,
            cpu_preparation_ns,
            renderer_end_to_end_ns: timings.end_to_end_ns,
            pipeline_end_to_end_ns,
            work: WorkCounts::delta(
                pending.terminal_before,
                terminal_after,
                pending.renderer_before,
                renderer_after,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct WorkCounts {
    row_updates: u64,
    cells_copied: u64,
    row_moves: u64,
    rows_moved: u64,
    rebuilt_rows: u64,
    text_prepares: u64,
    text_row_prepares: u64,
    text_rows_reused: u64,
    ascii_rows_shaped: u64,
    complex_rows_shaped: u64,
    geometry_builds: u64,
    buffer_writes: u64,
    upload_bytes: u64,
    allocation_growths: u64,
    static_geometry_builds: u64,
    static_geometry_writes: u64,
    static_geometry_bytes: u64,
    static_rows_reused: u64,
    dynamic_geometry_builds: u64,
    dynamic_geometry_writes: u64,
    dynamic_geometry_bytes: u64,
    geometry_buffer_growths: u64,
    transform_writes: u64,
    transform_bytes: u64,
    surface_retries: u64,
    acquired_frames: u64,
    presented_frames: u64,
    missed_refresh_deadlines: u64,
    coalesced_updates: u64,
    skipped_frames: u64,
}

impl WorkCounts {
    fn delta(
        terminal_before: TerminalMetrics,
        terminal_after: TerminalMetrics,
        renderer_before: RendererMetrics,
        renderer_after: RendererMetrics,
    ) -> Self {
        Self {
            row_updates: terminal_after
                .row_updates
                .saturating_sub(terminal_before.row_updates),
            cells_copied: terminal_after
                .cells_copied
                .saturating_sub(terminal_before.cells_copied),
            row_moves: terminal_after
                .row_moves
                .saturating_sub(terminal_before.row_moves),
            rows_moved: terminal_after
                .rows_moved
                .saturating_sub(terminal_before.rows_moved),
            rebuilt_rows: renderer_after
                .rebuilt_rows
                .saturating_sub(renderer_before.rebuilt_rows),
            text_prepares: renderer_after
                .text_prepares
                .saturating_sub(renderer_before.text_prepares),
            text_row_prepares: renderer_after
                .text_row_prepares
                .saturating_sub(renderer_before.text_row_prepares),
            text_rows_reused: renderer_after
                .text_rows_reused
                .saturating_sub(renderer_before.text_rows_reused),
            ascii_rows_shaped: renderer_after
                .ascii_rows_shaped
                .saturating_sub(renderer_before.ascii_rows_shaped),
            complex_rows_shaped: renderer_after
                .complex_rows_shaped
                .saturating_sub(renderer_before.complex_rows_shaped),
            geometry_builds: total_geometry_builds(renderer_after)
                .saturating_sub(total_geometry_builds(renderer_before)),
            buffer_writes: total_uploads(renderer_after)
                .saturating_sub(total_uploads(renderer_before)),
            upload_bytes: renderer_after
                .upload_bytes
                .saturating_sub(renderer_before.upload_bytes),
            allocation_growths: total_allocation_growths(renderer_after)
                .saturating_sub(total_allocation_growths(renderer_before)),
            static_geometry_builds: renderer_after
                .static_geometry_builds
                .saturating_sub(renderer_before.static_geometry_builds),
            static_geometry_writes: renderer_after
                .static_geometry_writes
                .saturating_sub(renderer_before.static_geometry_writes),
            static_geometry_bytes: renderer_after
                .static_geometry_bytes
                .saturating_sub(renderer_before.static_geometry_bytes),
            static_rows_reused: renderer_after
                .static_rows_reused
                .saturating_sub(renderer_before.static_rows_reused),
            dynamic_geometry_builds: renderer_after
                .dynamic_geometry_builds
                .saturating_sub(renderer_before.dynamic_geometry_builds),
            dynamic_geometry_writes: renderer_after
                .dynamic_geometry_writes
                .saturating_sub(renderer_before.dynamic_geometry_writes),
            dynamic_geometry_bytes: renderer_after
                .dynamic_geometry_bytes
                .saturating_sub(renderer_before.dynamic_geometry_bytes),
            geometry_buffer_growths: renderer_after
                .geometry_buffer_growths
                .saturating_sub(renderer_before.geometry_buffer_growths),
            transform_writes: renderer_after
                .transform_writes
                .saturating_sub(renderer_before.transform_writes),
            transform_bytes: renderer_after
                .transform_bytes
                .saturating_sub(renderer_before.transform_bytes),
            surface_retries: renderer_after
                .surface_retries
                .saturating_sub(renderer_before.surface_retries),
            acquired_frames: renderer_after
                .acquired_frames
                .saturating_sub(renderer_before.acquired_frames),
            presented_frames: renderer_after
                .presented_frames
                .saturating_sub(renderer_before.presented_frames),
            missed_refresh_deadlines: renderer_after
                .missed_refresh_deadlines
                .saturating_sub(renderer_before.missed_refresh_deadlines),
            coalesced_updates: renderer_after
                .coalesced_updates
                .saturating_sub(renderer_before.coalesced_updates),
            skipped_frames: renderer_after
                .skipped_frames
                .saturating_sub(renderer_before.skipped_frames),
        }
    }

    fn add(&mut self, other: Self) {
        self.row_updates = self.row_updates.saturating_add(other.row_updates);
        self.cells_copied = self.cells_copied.saturating_add(other.cells_copied);
        self.row_moves = self.row_moves.saturating_add(other.row_moves);
        self.rows_moved = self.rows_moved.saturating_add(other.rows_moved);
        self.rebuilt_rows = self.rebuilt_rows.saturating_add(other.rebuilt_rows);
        self.text_prepares = self.text_prepares.saturating_add(other.text_prepares);
        self.text_row_prepares = self
            .text_row_prepares
            .saturating_add(other.text_row_prepares);
        self.text_rows_reused = self.text_rows_reused.saturating_add(other.text_rows_reused);
        self.ascii_rows_shaped = self
            .ascii_rows_shaped
            .saturating_add(other.ascii_rows_shaped);
        self.complex_rows_shaped = self
            .complex_rows_shaped
            .saturating_add(other.complex_rows_shaped);
        self.geometry_builds = self.geometry_builds.saturating_add(other.geometry_builds);
        self.buffer_writes = self.buffer_writes.saturating_add(other.buffer_writes);
        self.upload_bytes = self.upload_bytes.saturating_add(other.upload_bytes);
        self.allocation_growths = self
            .allocation_growths
            .saturating_add(other.allocation_growths);
        self.static_geometry_builds = self
            .static_geometry_builds
            .saturating_add(other.static_geometry_builds);
        self.static_geometry_writes = self
            .static_geometry_writes
            .saturating_add(other.static_geometry_writes);
        self.static_geometry_bytes = self
            .static_geometry_bytes
            .saturating_add(other.static_geometry_bytes);
        self.static_rows_reused = self
            .static_rows_reused
            .saturating_add(other.static_rows_reused);
        self.dynamic_geometry_builds = self
            .dynamic_geometry_builds
            .saturating_add(other.dynamic_geometry_builds);
        self.dynamic_geometry_writes = self
            .dynamic_geometry_writes
            .saturating_add(other.dynamic_geometry_writes);
        self.dynamic_geometry_bytes = self
            .dynamic_geometry_bytes
            .saturating_add(other.dynamic_geometry_bytes);
        self.geometry_buffer_growths = self
            .geometry_buffer_growths
            .saturating_add(other.geometry_buffer_growths);
        self.transform_writes = self.transform_writes.saturating_add(other.transform_writes);
        self.transform_bytes = self.transform_bytes.saturating_add(other.transform_bytes);
        self.surface_retries = self.surface_retries.saturating_add(other.surface_retries);
        self.acquired_frames = self.acquired_frames.saturating_add(other.acquired_frames);
        self.presented_frames = self.presented_frames.saturating_add(other.presented_frames);
        self.missed_refresh_deadlines = self
            .missed_refresh_deadlines
            .saturating_add(other.missed_refresh_deadlines);
        self.coalesced_updates = self
            .coalesced_updates
            .saturating_add(other.coalesced_updates);
        self.skipped_frames = self.skipped_frames.saturating_add(other.skipped_frames);
    }
}

#[derive(Clone, Debug, Serialize)]
struct BehaviorObservation {
    name: String,
    duration_ns: u64,
    presented_frames: u64,
    text_prepares: u64,
    uploads: u64,
    passed: bool,
}

#[derive(Debug, Serialize)]
struct BenchmarkReport {
    report_version: u16,
    generated_unix_seconds: u64,
    source: BenchmarkSource,
    conditions: BenchmarkConditions,
    budgets: Vec<FrameBudget>,
    gpu_timing: SupportStatus,
    workloads: Vec<WorkloadSummary>,
    observations: Vec<BehaviorObservation>,
    release_gate: ReleaseGate,
    process: ProcessSummary,
    terminal_memory: TerminalMemoryReport,
    raw_samples: Vec<FrameSample>,
}

impl BenchmarkReport {
    fn new(application: &BenchmarkApplication) -> Self {
        let generated_unix_seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let workloads = Workload::ALL
            .iter()
            .map(|workload| WorkloadSummary::from_samples(workload.name(), &application.samples))
            .collect::<Vec<_>>();
        let observations = application.observations.clone();
        let release_gate = ReleaseGate::evaluate(
            application.display_refresh_hz,
            application.config.samples,
            &workloads,
            &observations,
        );
        Self {
            report_version: REPORT_VERSION,
            generated_unix_seconds,
            source: BenchmarkSource {
                application_version: env!("CARGO_PKG_VERSION").to_owned(),
                bundle_build_number: option_env!("TMON_BUILD_NUMBER")
                    .unwrap_or("unavailable")
                    .to_owned(),
                source_revision: option_env!("TMON_SOURCE_REVISION")
                    .unwrap_or("unavailable")
                    .to_owned(),
                source_dirty: compiled_source_dirty(),
                terminal_snapshot_version: TERMINAL_SNAPSHOT_VERSION,
                mux_protocol_version: PROTOCOL_VERSION,
            },
            conditions: BenchmarkConditions {
                hardware: command_output("/usr/sbin/system_profiler", &["SPHardwareDataType", "-detailLevel", "mini"]),
                macos: command_output("/usr/bin/sw_vers", &[]),
                metal_adapter: application.adapter_name.clone(),
                display_scale: application.display_scale,
                display_refresh_hz: application.display_refresh_hz,
                window_pixels: [
                    application.renderer().surface_size().width,
                    application.renderer().surface_size().height,
                ],
                grid: [application.grid_columns, application.grid_rows],
                font_family: "Menlo".to_owned(),
                font_size_points: 15.0,
                warmup_policy: "one unrecorded full frame per workload; cold_ascii records the first atlas frame".to_owned(),
                samples_per_warm_workload: application.config.samples,
                build_profile: "release-lto-thin".to_owned(),
            },
            budgets: [60.0_f64, 120.0]
                .into_iter()
                .map(FrameBudget::for_refresh_rate)
                .collect(),
            gpu_timing: SupportStatus {
                supported: false,
                reason: "wgpu Metal timestamp queries are not enabled by this adapter contract; use Instruments Metal System Trace for GPU execution time".to_owned(),
            },
            workloads,
            observations,
            release_gate,
            process: ProcessSummary {
                elapsed_ns: duration_ns(application.started.elapsed()),
                cpu_percent_at_end: current_cpu_percent(),
                rss_start_bytes: application.starting_rss_bytes,
                rss_peak_sampled_bytes: application.peak_rss_bytes,
                rss_end_bytes: current_rss_bytes(),
            },
            terminal_memory: TerminalMemoryReport::from(application.terminal.memory_stats()),
            raw_samples: application.samples.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ReleaseGate {
    evaluated_refresh_hz: f64,
    cpu_preparation_p95_budget_ns: u64,
    renderer_end_to_end_p99_budget_ns: u64,
    coverage_failures: Vec<String>,
    timing_failures: Vec<String>,
    behavior_failures: Vec<String>,
    passed: bool,
}

impl ReleaseGate {
    fn evaluate(
        refresh_hz: f64,
        requested_samples: usize,
        workloads: &[WorkloadSummary],
        observations: &[BehaviorObservation],
    ) -> Self {
        let budget = FrameBudget::for_refresh_rate(refresh_hz.max(1.0));
        let coverage_failures = workloads
            .iter()
            .filter(|workload| {
                let expected = if workload.name == Workload::ColdAscii.name() {
                    1
                } else {
                    requested_samples
                };
                workload.sample_count != expected
            })
            .map(|workload| workload.name.clone())
            .collect::<Vec<_>>();
        let timing_failures = workloads
            .iter()
            .filter(|workload| workload.name != Workload::ColdAscii.name())
            .filter(|workload| {
                workload.cpu_preparation.p95_ns > budget.cpu_preparation_budget_ns
                    || workload.renderer_end_to_end.p99_ns > budget.end_to_end_budget_ns
            })
            .map(|workload| workload.name.clone())
            .collect::<Vec<_>>();
        let behavior_failures = observations
            .iter()
            .filter(|observation| !observation.passed)
            .map(|observation| observation.name.clone())
            .collect::<Vec<_>>();
        let passed = coverage_failures.is_empty()
            && timing_failures.is_empty()
            && behavior_failures.is_empty();
        Self {
            evaluated_refresh_hz: refresh_hz,
            cpu_preparation_p95_budget_ns: budget.cpu_preparation_budget_ns,
            renderer_end_to_end_p99_budget_ns: budget.end_to_end_budget_ns,
            coverage_failures,
            timing_failures,
            behavior_failures,
            passed,
        }
    }
}

#[derive(Debug, Serialize)]
struct BenchmarkSource {
    application_version: String,
    bundle_build_number: String,
    source_revision: String,
    source_dirty: Option<bool>,
    terminal_snapshot_version: u16,
    mux_protocol_version: u16,
}

#[derive(Debug, Serialize)]
struct BenchmarkConditions {
    hardware: String,
    macos: String,
    metal_adapter: String,
    display_scale: f64,
    display_refresh_hz: f64,
    window_pixels: [u32; 2],
    grid: [usize; 2],
    font_family: String,
    font_size_points: f32,
    warmup_policy: String,
    samples_per_warm_workload: usize,
    build_profile: String,
}

#[derive(Debug, Serialize)]
struct FrameBudget {
    refresh_hz: f64,
    refresh_interval_ns: u64,
    cpu_preparation_budget_ns: u64,
    end_to_end_budget_ns: u64,
    policy: String,
}

impl FrameBudget {
    fn for_refresh_rate(refresh_hz: f64) -> Self {
        let refresh_interval_ns = (1_000_000_000.0 / refresh_hz) as u64;
        Self {
            refresh_hz,
            refresh_interval_ns,
            cpu_preparation_budget_ns: refresh_interval_ns * 7 / 10,
            end_to_end_budget_ns: refresh_interval_ns.saturating_mul(2),
            policy: "warm CPU preparation p95 <= 70% of one refresh interval; warm renderer end-to-end p99 <= two intervals; missed-refresh samples still count frames over one interval".to_owned(),
        }
    }
}

#[derive(Debug, Serialize)]
struct SupportStatus {
    supported: bool,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ProcessSummary {
    elapsed_ns: u64,
    cpu_percent_at_end: Option<f64>,
    rss_start_bytes: Option<u64>,
    rss_peak_sampled_bytes: Option<u64>,
    rss_end_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
struct TerminalMemoryReport {
    live_rows: usize,
    scrollback_rows: usize,
    total_cell_capacity: usize,
    cell_capacity_bytes: usize,
}

impl From<TerminalMemoryStats> for TerminalMemoryReport {
    fn from(stats: TerminalMemoryStats) -> Self {
        Self {
            live_rows: stats.live_rows,
            scrollback_rows: stats.scrollback_rows,
            total_cell_capacity: stats.total_cell_capacity(),
            cell_capacity_bytes: stats.cell_capacity_bytes(),
        }
    }
}

#[derive(Debug, Serialize)]
struct WorkloadSummary {
    name: String,
    sample_count: usize,
    atlas_states: Vec<String>,
    cpu_preparation: Percentiles,
    renderer_end_to_end: Percentiles,
    pipeline_end_to_end: Percentiles,
    stage_p95_ns: BTreeMap<String, u64>,
    dominant_stage: String,
    work: WorkCounts,
}

impl WorkloadSummary {
    fn from_samples(name: &str, samples: &[FrameSample]) -> Self {
        let samples = samples
            .iter()
            .filter(|sample| sample.workload == name)
            .collect::<Vec<_>>();
        let mut atlas_states = samples
            .iter()
            .map(|sample| sample.atlas_state.clone())
            .collect::<Vec<_>>();
        atlas_states.sort();
        atlas_states.dedup();
        let stages: [Stage; 12] = [
            ("mux_drain_decode", |sample| sample.mux_drain_decode_ns),
            ("terminal_feed", |sample| sample.terminal_feed_ns),
            ("frame_extraction", |sample| sample.frame_extraction_ns),
            ("retained_apply", |sample| sample.apply_frame_ns),
            ("surface_acquire", |sample| sample.surface_acquire_ns),
            ("viewport_update", |sample| sample.viewport_update_ns),
            ("glyph_prepare", |sample| sample.glyph_prepare_ns),
            ("geometry_build", |sample| sample.geometry_build_ns),
            ("geometry_upload", |sample| sample.geometry_upload_ns),
            ("encoding", |sample| sample.encoding_ns),
            ("submission", |sample| sample.submission_ns),
            ("presentation", |sample| sample.presentation_ns),
        ];
        let mut stage_p95_ns = BTreeMap::new();
        for (stage, value) in stages {
            stage_p95_ns.insert(
                stage.to_owned(),
                percentile(samples.iter().map(|sample| value(sample)), 95),
            );
        }
        let dominant_stage = stage_p95_ns
            .iter()
            .max_by_key(|(_, value)| *value)
            .map_or_else(|| "none".to_owned(), |(stage, _)| stage.clone());
        let mut work = WorkCounts::default();
        for sample in &samples {
            work.add(sample.work);
        }
        Self {
            name: name.to_owned(),
            sample_count: samples.len(),
            atlas_states,
            cpu_preparation: Percentiles::from_values(
                samples.iter().map(|sample| sample.cpu_preparation_ns),
            ),
            renderer_end_to_end: Percentiles::from_values(
                samples.iter().map(|sample| sample.renderer_end_to_end_ns),
            ),
            pipeline_end_to_end: Percentiles::from_values(
                samples.iter().map(|sample| sample.pipeline_end_to_end_ns),
            ),
            stage_p95_ns,
            dominant_stage,
            work,
        }
    }
}

#[allow(clippy::struct_field_names)]
#[derive(Debug, Serialize)]
struct Percentiles {
    p50_ns: u64,
    p95_ns: u64,
    p99_ns: u64,
}

impl Percentiles {
    fn from_values(values: impl Iterator<Item = u64> + Clone) -> Self {
        Self {
            p50_ns: percentile(values.clone(), 50),
            p95_ns: percentile(values.clone(), 95),
            p99_ns: percentile(values, 99),
        }
    }
}

fn percentile(values: impl Iterator<Item = u64>, percentile: usize) -> u64 {
    let mut values = values.collect::<Vec<_>>();
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let index = (values.len() * percentile).div_ceil(100).saturating_sub(1);
    values[index.min(values.len() - 1)]
}

fn print_human_summary(report: &BenchmarkReport, output: &Path) {
    println!(
        "Tmon Metal benchmark: {} @ {:.1} Hz, scale {:.1}, grid {}x{}",
        report.conditions.metal_adapter,
        report.conditions.display_refresh_hz,
        report.conditions.display_scale,
        report.conditions.grid[0],
        report.conditions.grid[1]
    );
    println!(
        "{:<28} {:>9} {:>9} {:>9} {:>9}  dominant",
        "workload", "cpu p50", "cpu p95", "e2e p95", "uploads"
    );
    for workload in &report.workloads {
        println!(
            "{:<28} {:>8.3}ms {:>8.3}ms {:>8.3}ms {:>9}  {}",
            workload.name,
            ns_to_ms(workload.cpu_preparation.p50_ns),
            ns_to_ms(workload.cpu_preparation.p95_ns),
            ns_to_ms(workload.pipeline_end_to_end.p95_ns),
            workload.work.upload_bytes,
            workload.dominant_stage
        );
    }
    for observation in &report.observations {
        println!(
            "{}: {} (presented {}, prepares {}, uploads {})",
            observation.name,
            if observation.passed { "pass" } else { "FAIL" },
            observation.presented_frames,
            observation.text_prepares,
            observation.uploads
        );
    }
    println!(
        "release gate: {} ({} Hz CPU p95 <= {:.3}ms, renderer p99 <= {:.3}ms)",
        if report.release_gate.passed {
            "pass"
        } else {
            "FAIL"
        },
        report.release_gate.evaluated_refresh_hz,
        ns_to_ms(report.release_gate.cpu_preparation_p95_budget_ns),
        ns_to_ms(report.release_gate.renderer_end_to_end_p99_budget_ns)
    );
    println!("machine-readable report: {}", output.display());
}

fn failure_summary(failures: &[String]) -> String {
    if failures.is_empty() {
        "none".to_owned()
    } else {
        failures.join(", ")
    }
}

fn duration_ns(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn ns_to_ms(nanoseconds: u64) -> f64 {
    nanoseconds as f64 / 1_000_000.0
}

fn total_geometry_builds(metrics: RendererMetrics) -> u64 {
    metrics
        .static_geometry_builds
        .saturating_add(metrics.dynamic_geometry_builds)
}

fn total_uploads(metrics: RendererMetrics) -> u64 {
    metrics
        .static_geometry_writes
        .saturating_add(metrics.dynamic_geometry_writes)
        .saturating_add(metrics.transform_writes)
}

fn total_allocation_growths(metrics: RendererMetrics) -> u64 {
    metrics
        .rectangle_scratch_growths
        .saturating_add(metrics.rounded_corner_scratch_growths)
        .saturating_add(metrics.braille_scratch_growths)
        .saturating_add(metrics.geometry_buffer_growths)
}

fn current_rss_bytes() -> Option<u64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    String::from_utf8(output.stdout)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|kilobytes| kilobytes.saturating_mul(1_024))
}

fn current_cpu_percent() -> Option<f64> {
    let output = Command::new("/bin/ps")
        .args(["-o", "%cpu=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn command_output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map_or_else(
            || "unavailable".to_owned(),
            |output| output.trim().to_owned(),
        )
}

fn compiled_source_dirty() -> Option<bool> {
    match option_env!("TMON_SOURCE_DIRTY") {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{
        BehaviorObservation, Percentiles, ReleaseGate, WorkCounts, WorkloadSummary, percentile,
    };

    #[test]
    fn percentile_uses_nearest_rank_and_handles_empty_samples() {
        assert_eq!(percentile([].into_iter(), 95), 0);
        assert_eq!(percentile([1, 2, 3, 4].into_iter(), 50), 2);
        assert_eq!(percentile([1, 2, 3, 4].into_iter(), 95), 4);
        let summary = Percentiles::from_values([9, 1, 5].into_iter());
        assert_eq!(summary.p50_ns, 5);
        assert_eq!(summary.p95_ns, 9);
        assert_eq!(summary.p99_ns, 9);
    }

    #[test]
    fn release_gate_rejects_incomplete_slow_or_active_idle_evidence() {
        let cold = workload("cold_ascii", 1, u64::MAX, u64::MAX);
        let incomplete = workload("resize", 29, 1, 1);
        let slow = workload("surface_recreate", 30, 6_000_000, 17_000_000);
        let active_idle = BehaviorObservation {
            name: "idle".to_owned(),
            duration_ns: 1,
            presented_frames: 1,
            text_prepares: 0,
            uploads: 0,
            passed: false,
        };
        let gate = ReleaseGate::evaluate(120.0, 30, &[cold, incomplete, slow], &[active_idle]);
        assert_eq!(gate.coverage_failures, ["resize"]);
        assert_eq!(gate.timing_failures, ["surface_recreate"]);
        assert_eq!(gate.behavior_failures, ["idle"]);
        assert!(!gate.passed);
    }

    #[test]
    fn release_gate_accepts_exact_budget_and_cold_atlas_outlier() {
        let cold = workload("cold_ascii", 1, u64::MAX, u64::MAX);
        let warm = workload("scale_factor_rebuild", 30, 5_833_333, 16_666_666);
        let idle = BehaviorObservation {
            name: "idle".to_owned(),
            duration_ns: 1,
            presented_frames: 0,
            text_prepares: 0,
            uploads: 0,
            passed: true,
        };
        assert!(ReleaseGate::evaluate(120.0, 30, &[cold, warm], &[idle]).passed);
    }

    fn workload(
        name: &str,
        sample_count: usize,
        cpu_p95_ns: u64,
        renderer_p99_ns: u64,
    ) -> WorkloadSummary {
        WorkloadSummary {
            name: name.to_owned(),
            sample_count,
            atlas_states: vec!["warm".to_owned()],
            cpu_preparation: Percentiles {
                p50_ns: 0,
                p95_ns: cpu_p95_ns,
                p99_ns: cpu_p95_ns,
            },
            renderer_end_to_end: Percentiles {
                p50_ns: 0,
                p95_ns: renderer_p99_ns,
                p99_ns: renderer_p99_ns,
            },
            pipeline_end_to_end: Percentiles {
                p50_ns: 0,
                p95_ns: 0,
                p99_ns: 0,
            },
            stage_p95_ns: BTreeMap::new(),
            dominant_stage: "none".to_owned(),
            work: WorkCounts::default(),
        }
    }
}
