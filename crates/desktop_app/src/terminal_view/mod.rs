use crate::chrome_style::ChromeContrastProfile;
use crate::colors::TerminalColors;
use crate::commands::{self, CommandAction};
use crate::config::{
    self, AppConfig, CursorStyle as AppCursorStyle, PaneFocusEffect, SystemAppearance,
    TabBarPosition, TabCloseVisibility, TabTitleConfig, TabTitleSource, TabWidthMode, TaskConfig,
    TerminalScrollbarStyle, TerminalScrollbarVisibility, resolve_active_theme,
    system_appearance_from_window,
};
use crate::keybindings;
use crate::ui::scrollbar::{ScrollbarVisibilityController, ScrollbarVisibilityMode};
use alacritty_terminal::{grid::Dimensions, term::cell::Flags};
use flume::{Sender, bounded};
use gpui::AppContext;
use gpui::{
    AnyElement, App, AsyncApp, Bounds, ClipboardItem, Context, Element, Entity, ExternalPaths,
    FocusHandle, Focusable, Font, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Render, ScrollWheelEvent, SharedString, Size,
    StatefulInteractiveElement, Styled, TouchPhase, WeakEntity, Window, WindowBackgroundAppearance,
    div, point, px, relative,
};
#[cfg(target_os = "macos")]
use std::process::Stdio;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ops::{Deref, Range},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use termy_auto_update::{AutoUpdater, UpdateState};
use termy_config_core::{MAX_LINE_HEIGHT, MIN_LINE_HEIGHT};
use termy_plugin_runtime::{PluginEvent, PluginRuntime};
use termy_search::SearchState;
use termy_terminal_ui::{
    CellRenderInfo, CommandLifecycle, KittyGraphicsRenderPlacement, PaneTerminal, ProgressState,
    TabTitleShellIntegration, Terminal as NativeTerminal, TerminalClipboardTarget,
    TerminalCursorState, TerminalCursorStyle, TerminalDamageSnapshot, TerminalDirtySpan,
    TerminalEvent, TerminalGrid, TerminalGridPaintCacheHandle, TerminalGridPaintDamage,
    TerminalGridRows, TerminalKeyEventKind, TerminalKeyboardMode, TerminalLaunch,
    TerminalMouseMode, TerminalOptions, TerminalQueryColors, TerminalReplyHost,
    TerminalRuntimeConfig, TerminalSize, TerminalWakeupNotifier, TmuxLaunchTarget,
    TmuxPaneMouseMode, WindowsShell as RuntimeWindowsShell,
    WorkingDirFallback as RuntimeWorkingDirFallback, find_link_in_line, hyperlink_at_viewport_cell,
    keystroke_to_input, link_at_viewport_cell, normalize_working_directory_candidate,
    resolve_launch_working_directory, resolve_working_directory_path,
};
use termy_toast::ToastManager;

mod appearance;
mod benchmark;
mod command_palette;
mod constants;
mod inline_input;
mod inspector;
mod interaction;
#[cfg(target_os = "macos")]
mod macos_file_drop;
mod metrics;
mod overlay_view;
mod persistence;
mod plugin_ui;
mod render;
mod render_cache;
mod runtime;
mod scrollbar;
mod search;
pub(crate) mod tab_strip;
mod tabs;
mod titles;
mod tmon_adapter;
mod update_toasts;
mod workspaces;

use self::benchmark::{BENCHMARK_SAMPLE_INTERVAL, BenchmarkConfig, BenchmarkSession};
use self::scrollbar::{
    TerminalScrollbarDragState, TerminalScrollbarHit, TerminalScrollbarMarkerCache,
    TerminalScrollbarMarkerCacheKey, TerminalScrollbarTrackHoldState,
};
pub(crate) use appearance::initial_window_background_appearance;
use appearance::{
    BackgroundSupportContext, BlurFallbackReason, OverlayStyleBuilder, PaneFocusPreset,
    background_opacity_factor, blend_rgba, pane_divider_color, pane_focus_preset,
    pane_focus_strength_factor, resolve_background_appearance, resolve_chrome_stroke_color,
    scaled_background_alpha_for_opacity, scaled_chrome_alpha_for_opacity,
};
use command_palette::{
    CommandPaletteMode, CommandPaletteState, PluginLifecycleState, TmuxSessionIntent,
};
use constants::*;
use inline_input::{InlineInputAlignment, InlineInputState};
use interaction::{
    HoveredLink, MouseReportTargetCell, MouseReportingState, PaneDropRegion, PaneMoveDragState,
    PendingCursorMoveClick, PendingCursorMovePreview, PendingKeyRelease, TabContextMenuState,
    TerminalContextMenuState, kitty_graphics_placement_bounds,
    kitty_graphics_placement_intersects_selection,
};
#[cfg(target_os = "macos")]
pub(crate) use macos_file_drop::{NativeDropResult, install_native_file_drop};
use metrics::DebugOverlayStats;
#[cfg(debug_assertions)]
use metrics::{TerminalRenderMetricsCounters, TerminalRenderMetricsState};
use overlay_view::TerminalOverlayView;
use plugin_ui::PluginUiView;
use render_cache::{
    TerminalPaneCellColorTransformKey, TerminalPaneRenderCache, TerminalPaneRenderCacheKey,
};
use runtime::{RuntimeKind, RuntimeState, TmuxRuntime};
pub(crate) use tab_strip::constants::*;
use tab_strip::state::TabStripState;

type TabId = u64;
type NativeTerminalWakeupId = u64;

static NEXT_NATIVE_TERMINAL_WAKEUP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct NativeTerminalWakeupRouter {
    ready: Arc<Mutex<HashSet<NativeTerminalWakeupId>>>,
    view_wakeup_tx: Sender<()>,
}

impl NativeTerminalWakeupRouter {
    fn new(view_wakeup_tx: Sender<()>) -> Self {
        Self {
            ready: Arc::new(Mutex::new(HashSet::new())),
            view_wakeup_tx,
        }
    }

    fn notifier(&self, wakeup_id: NativeTerminalWakeupId) -> TerminalWakeupNotifier {
        let router = self.clone();
        TerminalWakeupNotifier::new(move || router.mark_ready(wakeup_id))
    }

    fn tmon_notifier(&self, wakeup_id: NativeTerminalWakeupId) -> tmon::WakeupNotifier {
        let router = self.clone();
        tmon::WakeupNotifier::new(move || router.mark_ready(wakeup_id))
    }

    fn mark_ready(&self, wakeup_id: NativeTerminalWakeupId) {
        self.ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(wakeup_id);
        let _ = self.view_wakeup_tx.try_send(());
    }

    fn drain_ready_into(&self, target: &mut HashSet<NativeTerminalWakeupId>) {
        target.clear();
        let mut ready = self
            .ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        target.extend(ready.drain());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellPos {
    col: usize,
    row: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalResizeSignature {
    viewport_width_bits: u32,
    viewport_height_bits: u32,
    cell_width_bits: u32,
    cell_height_bits: u32,
    sidebar_width_bits: u32,
    content_top_inset_bits: u32,
    padding_x_bits: u32,
    padding_y_bits: u32,
    runtime_kind: RuntimeKind,
    active_tab_id: Option<TabId>,
    pane_layout_fingerprint: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeferredTerminalResize {
    Idle,
    Pending {
        signature: TerminalResizeSignature,
        deadline: Instant,
    },
    Ready {
        signature: TerminalResizeSignature,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SelectionPos {
    col: usize,
    line: i32,
}

#[derive(Clone)]
pub(in crate::terminal_view) struct KittyImageSelection {
    pub(in crate::terminal_view) pane_id: String,
    pub(in crate::terminal_view) placement: KittyGraphicsRenderPlacement,
}

impl KittyImageSelection {
    fn matches(&self, pane_id: &str, placement: &KittyGraphicsRenderPlacement) -> bool {
        self.pane_id == pane_id
            && self.placement.placement_serial == placement.placement_serial
            && self.placement.image_id == placement.image_id
            && self.placement.image_generation == placement.image_generation
    }

    fn current_placement<'a>(
        &self,
        active_pane_id: Option<&str>,
        placements: &'a [KittyGraphicsRenderPlacement],
    ) -> Option<&'a KittyGraphicsRenderPlacement> {
        let active_pane_id = active_pane_id?;
        placements
            .iter()
            .find(|placement| self.matches(active_pane_id, placement))
    }
}

impl std::fmt::Debug for KittyImageSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KittyImageSelection")
            .field("pane_id", &self.pane_id)
            .field("placement_serial", &self.placement.placement_serial)
            .field("image_id", &self.placement.image_id)
            .field("placement_id", &self.placement.placement_id)
            .field("image_generation", &self.placement.image_generation)
            .finish()
    }
}

impl PartialEq for KittyImageSelection {
    fn eq(&self, other: &Self) -> bool {
        self.matches(other.pane_id.as_str(), &other.placement)
    }
}

impl Eq for KittyImageSelection {}

#[derive(Clone, Copy, Debug)]
pub(super) struct TerminalViewportGeometry {
    origin_x: f32,
    origin_y: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalContentRect {
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
}

impl TerminalContentRect {
    fn new(origin_x: f32, origin_y: f32, width: f32, height: f32) -> Option<Self> {
        if width <= f32::EPSILON || height <= f32::EPSILON {
            return None;
        }

        Some(Self {
            origin_x,
            origin_y,
            width,
            height,
        })
    }

    fn right(self) -> f32 {
        self.origin_x + self.width
    }

    fn bottom(self) -> f32 {
        self.origin_y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalScrollbarSurfaceGeometry {
    origin_x: f32,
    origin_y: f32,
    width: f32,
    height: f32,
}

impl TerminalScrollbarSurfaceGeometry {
    fn new(origin_x: f32, origin_y: f32, width: f32, height: f32) -> Option<Self> {
        if width <= f32::EPSILON || height <= f32::EPSILON {
            return None;
        }

        Some(Self {
            origin_x,
            origin_y,
            width,
            height,
        })
    }

    fn gutter_frame(self) -> Option<TerminalScrollbarGutterFrame> {
        let gutter_width = TERMINAL_SCROLLBAR_GUTTER_WIDTH.min(self.width.max(0.0));
        if gutter_width <= f32::EPSILON {
            return None;
        }

        Some(TerminalScrollbarGutterFrame {
            left: (self.origin_x + self.width.max(0.0) - gutter_width).max(self.origin_x),
            top: self.origin_y,
            width: gutter_width,
            height: self.height,
        })
    }

    fn local_y(self, content_y: f32) -> Option<f32> {
        if content_y < self.origin_y || content_y > self.origin_y + self.height {
            return None;
        }

        Some(content_y - self.origin_y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalScrollbarGutterFrame {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TerminalPaneNeighborGaps {
    right_cells: Option<u32>,
    bottom_cells: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalPaneLayout {
    frame: TerminalContentRect,
    content_frame: TerminalContentRect,
    scrollbar_surface: TerminalScrollbarSurfaceGeometry,
    cell_width: f32,
    cell_height: f32,
    extends_right_edge: bool,
    extends_bottom_edge: bool,
    gaps: TerminalPaneNeighborGaps,
}

#[derive(Clone, Debug, PartialEq)]
struct TerminalPaneDivider {
    pane_id: String,
    handle_id: SharedString,
    axis: PaneResizeAxis,
    edge: PaneResizeEdge,
    line_frame: TerminalContentRect,
    hit_frame: TerminalContentRect,
    grip_frame: TerminalContentRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NativePaneRect {
    left: u16,
    top: u16,
    width: u16,
    height: u16,
}

impl NativePaneRect {
    fn right(self) -> u16 {
        self.left.saturating_add(self.width)
    }

    fn bottom(self) -> u16 {
        self.top.saturating_add(self.height)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct NativePaneLayoutTree {
    root: NativePaneLayoutNode,
}

#[derive(Clone, Debug, PartialEq)]
enum NativePaneLayoutNode {
    Leaf {
        pane_id: String,
    },
    Split {
        axis: PaneResizeAxis,
        ratio: f32,
        first: Box<NativePaneLayoutNode>,
        second: Box<NativePaneLayoutNode>,
    },
}

impl TerminalPaneDivider {
    fn hit_distance(&self, x: f32, y: f32) -> Option<f32> {
        if x < self.hit_frame.origin_x
            || x > self.hit_frame.right()
            || y < self.hit_frame.origin_y
            || y > self.hit_frame.bottom()
        {
            return None;
        }

        let center_x = self.hit_frame.origin_x + (self.hit_frame.width * 0.5);
        let center_y = self.hit_frame.origin_y + (self.hit_frame.height * 0.5);
        Some(match self.axis {
            PaneResizeAxis::Horizontal => (x - center_x).abs(),
            PaneResizeAxis::Vertical => (y - center_y).abs(),
        })
    }
}

fn cell_ranges_overlap(start_a: u32, end_a: u32, start_b: u32, end_b: u32) -> bool {
    start_a < end_b && start_b < end_a
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneResizeAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneResizeEdge {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Clone, Debug)]
struct PaneResizeDragState {
    pane_id: String,
    axis: PaneResizeAxis,
    edge: PaneResizeEdge,
    start_x: f32,
    start_y: f32,
    applied_steps: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HoveredPaneDivider {
    pane_id: String,
    axis: PaneResizeAxis,
    edge: PaneResizeEdge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneResizeResult {
    Applied,
    BlockedByMinimum,
    NoChange,
}

#[allow(clippy::large_enum_variant)]
enum Terminal {
    Tmux(PaneTerminal),
    Native(NativeTerminalInstance),
    Tmon(TmonTerminalInstance),
}

struct NativeTerminalInstance {
    wakeup_id: NativeTerminalWakeupId,
    terminal: Mutex<NativeTerminal>,
}

struct TmonTerminalInstance {
    wakeup_id: NativeTerminalWakeupId,
    terminal: tmon::Terminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalLineRange {
    first_line: i32,
    last_line: i32,
    columns: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TerminalViewportScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalViewportScroll {
    top: usize,
    bottom: usize,
    count: usize,
    direction: TerminalViewportScrollDirection,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TerminalRenderDamageSnapshot {
    damage: TerminalDamageSnapshot,
    scrolls: Vec<TerminalViewportScroll>,
    generation: Option<u64>,
}

impl TerminalRenderDamageSnapshot {
    fn from_core(damage: TerminalDamageSnapshot) -> Self {
        Self {
            damage,
            scrolls: Vec::new(),
            generation: None,
        }
    }
}

#[derive(Clone, Copy)]
enum TerminalCellRef<'a> {
    Alacritty(&'a alacritty_terminal::term::cell::Cell),
    Tmon(&'a tmon::Cell, Option<tmon::Combining<'a>>),
}

impl<'a> From<&'a alacritty_terminal::term::cell::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a alacritty_terminal::term::cell::Cell) -> Self {
        Self::Alacritty(cell)
    }
}

impl<'a> From<&'a tmon::Cell> for TerminalCellRef<'a> {
    fn from(cell: &'a tmon::Cell) -> Self {
        Self::Tmon(cell, None)
    }
}

impl TerminalCellRef<'_> {
    fn character(self) -> char {
        match self {
            Self::Alacritty(cell) => cell.c,
            Self::Tmon(cell, _) => cell.character,
        }
    }

    fn is_wide_spacer(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            Self::Tmon(cell, _) => cell.wide_spacer() || cell.leading_wide_spacer(),
        }
    }

    fn is_trailing_wide_spacer(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell.flags.contains(Flags::WIDE_CHAR_SPACER),
            Self::Tmon(cell, _) => cell.wide_spacer(),
        }
    }

    fn is_hidden(self) -> bool {
        match self {
            Self::Alacritty(cell) => cell.flags.contains(Flags::HIDDEN),
            Self::Tmon(cell, _) => cell.attributes.hidden(),
        }
    }

    fn combining(self) -> Option<SharedString> {
        match self {
            Self::Alacritty(_) => None,
            Self::Tmon(_, combining) => combining
                .map(tmon::Combining::to_owned_string)
                .map(SharedString::from),
        }
    }

    fn append_combining_to(self, text: &mut String) {
        match self {
            Self::Alacritty(_) => {}
            Self::Tmon(_, combining) => {
                if let Some(combining) = combining {
                    combining.append_to(text);
                }
            }
        }
    }
}

fn tmon_engine_requested(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "1")
}

fn tmon_engine_available() -> bool {
    tmon::native_pty_available()
}

fn tmon_engine_enabled_for(value: Option<&std::ffi::OsStr>, available: bool) -> bool {
    tmon_engine_requested(value) && available
}

fn tmon_engine_enabled(value: Option<&std::ffi::OsStr>) -> bool {
    tmon_engine_enabled_for(value, tmon_engine_available())
}

fn terminal_engine_label(terminal: Option<&Terminal>) -> &'static str {
    match terminal {
        Some(Terminal::Tmon(_)) => "tmon",
        Some(Terminal::Native(_) | Terminal::Tmux(_)) => "alacritty",
        None => "-",
    }
}

impl Deref for TmonTerminalInstance {
    type Target = tmon::Terminal;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl Deref for NativeTerminalInstance {
    type Target = Mutex<NativeTerminal>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

#[derive(Default)]
struct ClipboardTextCache {
    // Outer `Option` records whether GPUI has been queried; the inner value is
    // the clipboard result, which can legitimately be empty.
    value: Option<Option<String>>,
}

impl ClipboardTextCache {
    fn get_or_read(&mut self, read: impl FnOnce() -> Option<String>) -> Option<String> {
        if let Some(value) = &self.value {
            return value.clone();
        }

        let value = read();
        self.value = Some(value.clone());
        value
    }
}

struct GpuiClipboardReplyHost<'host, 'cx> {
    cx: &'host mut Context<'cx, TerminalView>,
    clipboard_text: &'host mut ClipboardTextCache,
}

impl<'host, 'cx> GpuiClipboardReplyHost<'host, 'cx> {
    fn new(
        cx: &'host mut Context<'cx, TerminalView>,
        clipboard_text: &'host mut ClipboardTextCache,
    ) -> Self {
        Self { cx, clipboard_text }
    }
}

impl TerminalReplyHost for GpuiClipboardReplyHost<'_, '_> {
    fn load_clipboard(&mut self, _target: TerminalClipboardTarget) -> Option<String> {
        // GPUI exposes a single host clipboard source here, so both OSC 52
        // targets resolve through the same adapter.
        self.clipboard_text
            .get_or_read(|| self.cx.read_from_clipboard().and_then(|item| item.text()))
    }
}

impl Terminal {
    fn tmon_enabled() -> bool {
        let value = std::env::var_os("TERMY_EXPERIMENTAL_TMON_ENGINE");
        let value = value.as_deref();
        if tmon_engine_requested(value) && !tmon_engine_available() {
            log::warn!(
                "TERMY_EXPERIMENTAL_TMON_ENGINE=1 requested, but Tmon's native PTY is unavailable; \
                 falling back to the native Alacritty terminal engine"
            );
        }
        tmon_engine_enabled(value)
    }

    fn new_tmux(size: TerminalSize, options: TerminalOptions) -> Self {
        Self::Tmux(PaneTerminal::new(size, options))
    }

    fn new_native(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_router: Option<&NativeTerminalWakeupRouter>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        startup_command: Option<&str>,
    ) -> anyhow::Result<Self> {
        let wakeup_id = NEXT_NATIVE_TERMINAL_WAKEUP_ID.fetch_add(1, Ordering::Relaxed);
        if Self::tmon_enabled() {
            log::info!("using experimental Tmon terminal engine");
            let launch = startup_command
                .map(str::to_string)
                .map(TerminalLaunch::ShellCommand);
            return Ok(Self::Tmon(TmonTerminalInstance {
                wakeup_id,
                terminal: tmon::Terminal::new(
                    tmon_adapter::size(size),
                    tmon_adapter::config(
                        configured_working_dir,
                        tab_title_shell_integration,
                        runtime_config,
                        launch.as_ref(),
                    )?,
                    wakeup_router.map(|router| router.tmon_notifier(wakeup_id)),
                )?,
            }));
        }
        let wakeup_notifier = wakeup_router.map(|router| router.notifier(wakeup_id));
        Ok(Self::Native(NativeTerminalInstance {
            wakeup_id,
            terminal: Mutex::new(NativeTerminal::new_with_wakeup_notifier(
                size,
                configured_working_dir,
                wakeup_notifier,
                tab_title_shell_integration,
                runtime_config,
                startup_command,
            )?),
        }))
    }

    fn new_native_with_launch(
        size: TerminalSize,
        configured_working_dir: Option<&str>,
        wakeup_router: Option<&NativeTerminalWakeupRouter>,
        tab_title_shell_integration: Option<&TabTitleShellIntegration>,
        runtime_config: Option<&TerminalRuntimeConfig>,
        launch: Option<&TerminalLaunch>,
    ) -> anyhow::Result<Self> {
        let wakeup_id = NEXT_NATIVE_TERMINAL_WAKEUP_ID.fetch_add(1, Ordering::Relaxed);
        if Self::tmon_enabled() {
            log::info!("using experimental Tmon terminal engine");
            return Ok(Self::Tmon(TmonTerminalInstance {
                wakeup_id,
                terminal: tmon::Terminal::new(
                    tmon_adapter::size(size),
                    tmon_adapter::config(
                        configured_working_dir,
                        tab_title_shell_integration,
                        runtime_config,
                        launch,
                    )?,
                    wakeup_router.map(|router| router.tmon_notifier(wakeup_id)),
                )?,
            }));
        }
        let wakeup_notifier = wakeup_router.map(|router| router.notifier(wakeup_id));
        Ok(Self::Native(NativeTerminalInstance {
            wakeup_id,
            terminal: Mutex::new(NativeTerminal::new_with_launch_and_wakeup_notifier(
                size,
                configured_working_dir,
                wakeup_notifier,
                tab_title_shell_integration,
                runtime_config,
                launch,
            )?),
        }))
    }

    fn wakeup_id(&self) -> Option<NativeTerminalWakeupId> {
        match self {
            Self::Tmux(_) => None,
            Self::Native(terminal) => Some(terminal.wakeup_id),
            Self::Tmon(terminal) => Some(terminal.wakeup_id),
        }
    }

    fn feed_output(&self, bytes: &[u8]) {
        if let Self::Tmux(terminal) = self {
            terminal.feed_output(bytes);
        }
    }

    fn hydrate_output(&self, bytes: &[u8]) {
        match self {
            Self::Tmux(terminal) => terminal.feed_output(bytes),
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.hydrate_output(bytes);
                }
            }
            Self::Tmon(terminal) => terminal.hydrate_output(bytes),
        }
    }

    fn write_input(&self, input: &[u8]) {
        match self {
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.write(input);
                }
            }
            Self::Tmon(terminal) => terminal.write(input),
            Self::Tmux(_) => {}
        }
    }

    fn write_input_owned(&self, input: Vec<u8>) {
        match self {
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.write_owned(input);
                }
            }
            Self::Tmon(terminal) => terminal.write_owned(input),
            Self::Tmux(_) => {}
        }
    }

    fn set_wakeup_enabled(&self, enabled: bool) {
        match self {
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.set_wakeup_enabled(enabled);
                }
            }
            Self::Tmon(terminal) => terminal.set_wakeup_enabled(enabled),
            Self::Tmux(_) => {}
        }
    }

    /// Drain pending events. Returns collected events and whether more remain.
    fn drain_events(&self, host: &mut impl TerminalReplyHost) -> (Vec<TerminalEvent>, bool) {
        match self {
            Self::Tmux(_) => (Vec::new(), false),
            Self::Native(terminal) => terminal
                .lock()
                .map(|terminal| terminal.drain_events(host))
                .unwrap_or_default(),
            Self::Tmon(terminal) => {
                let (events, has_more) = terminal.drain_events();
                let mut translated = Vec::with_capacity(events.len());
                for event in events {
                    match event {
                        tmon::Event::ClipboardLoad(request) => {
                            if let Some(text) = host
                                .load_clipboard(tmon_adapter::clipboard_target(request.target()))
                            {
                                terminal.write_protocol_reply_owned(request.format_reply(&text));
                            }
                        }
                        event => translated.extend(tmon_adapter::event(event)),
                    }
                }
                (translated, has_more)
            }
        }
    }

    fn resize(&self, new_size: TerminalSize) {
        match self {
            Self::Tmux(terminal) => terminal.resize(new_size),
            Self::Native(terminal) => {
                if let Ok(mut terminal) = terminal.lock() {
                    terminal.resize(new_size);
                }
            }
            Self::Tmon(terminal) => terminal.resize(tmon_adapter::size(new_size)),
        }
    }

    /// Re-send the current PTY size to deliver SIGWINCH without changing dimensions.
    fn nudge_resize(&self) {
        match self {
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.nudge_resize();
                }
            }
            Self::Tmon(terminal) => terminal.nudge_resize(),
            Self::Tmux(_) => {}
        }
    }

    fn size(&self) -> TerminalSize {
        match self {
            Self::Tmux(terminal) => terminal.size(),
            Self::Native(terminal) => terminal
                .lock()
                .map(|terminal| terminal.size())
                .unwrap_or_default(),
            Self::Tmon(terminal) => tmon_adapter::terminal_size(terminal.size()),
        }
    }

    fn child_pid(&self) -> Option<u32> {
        match self {
            Self::Tmux(_) => None,
            Self::Native(terminal) => terminal
                .lock()
                .ok()
                .and_then(|terminal| terminal.child_pid()),
            Self::Tmon(terminal) => terminal.child_pid(),
        }
    }

    fn scroll_display(&self, delta_lines: i32) -> bool {
        match self {
            Self::Tmux(terminal) => terminal.scroll_display(delta_lines),
            Self::Native(terminal) => terminal
                .lock()
                .is_ok_and(|terminal| terminal.scroll_display(delta_lines)),
            Self::Tmon(terminal) => terminal.scroll_display(delta_lines),
        }
    }

    fn scroll_to_bottom(&self) -> bool {
        match self {
            Self::Tmux(terminal) => terminal.scroll_to_bottom(),
            Self::Native(terminal) => terminal
                .lock()
                .is_ok_and(|terminal| terminal.scroll_to_bottom()),
            Self::Tmon(terminal) => terminal.scroll_to_bottom(),
        }
    }

    fn scroll_state(&self) -> (usize, usize) {
        match self {
            Self::Tmux(terminal) => terminal.scroll_state(),
            Self::Native(terminal) => terminal
                .lock()
                .map_or((0, 0), |terminal| terminal.scroll_state()),
            Self::Tmon(terminal) => terminal.scroll_state(),
        }
    }

    fn cursor_state(&self) -> Option<TerminalCursorState> {
        match self {
            Self::Tmux(terminal) => terminal.cursor_state(),
            Self::Native(terminal) => terminal
                .lock()
                .map_or(None, |terminal| terminal.cursor_state()),
            Self::Tmon(terminal) => terminal.cursor_state().map(tmon_adapter::cursor_state),
        }
    }

    fn cursor_position(&self) -> (usize, usize) {
        match self {
            Self::Tmux(terminal) => terminal.cursor_position(),
            Self::Native(terminal) => terminal
                .lock()
                .map_or((0, 0), |terminal| terminal.cursor_position()),
            Self::Tmon(terminal) => terminal.cursor_position(),
        }
    }

    fn set_term_options(&self, options: TerminalOptions) {
        match self {
            Self::Tmux(terminal) => terminal.set_term_options(options),
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    terminal.set_term_options(options);
                }
            }
            Self::Tmon(terminal) => terminal.set_options(tmon_adapter::options(options)),
        }
    }

    fn set_query_colors(&self, query_colors: TerminalQueryColors) {
        match self {
            Self::Native(terminal) => {
                if let Ok(mut terminal) = terminal.lock() {
                    terminal.set_query_colors(query_colors);
                }
            }
            Self::Tmon(terminal) => {
                terminal.set_query_colors(tmon_adapter::query_colors(query_colors));
            }
            Self::Tmux(_) => {}
        }
    }

    fn tmon_palette(&self) -> Option<tmon::Palette> {
        match self {
            Self::Tmon(terminal) => Some(terminal.palette()),
            Self::Native(_) | Self::Tmux(_) => None,
        }
    }

    fn bracketed_paste_mode(&self) -> bool {
        match self {
            Self::Tmux(terminal) => terminal.bracketed_paste_mode(),
            Self::Native(terminal) => terminal
                .lock()
                .is_ok_and(|terminal| terminal.bracketed_paste_mode()),
            Self::Tmon(terminal) => terminal.bracketed_paste_mode(),
        }
    }

    fn alternate_screen_mode(&self) -> bool {
        match self {
            Self::Tmux(terminal) => terminal.alternate_screen_mode(),
            Self::Native(terminal) => terminal
                .lock()
                .is_ok_and(|terminal| terminal.alternate_screen_mode()),
            Self::Tmon(terminal) => terminal.alternate_screen_mode(),
        }
    }

    fn mouse_mode(&self) -> TerminalMouseMode {
        match self {
            Self::Tmux(terminal) => terminal.mouse_mode(),
            Self::Native(terminal) => terminal
                .lock()
                .map(|terminal| terminal.mouse_mode())
                .unwrap_or_default(),
            Self::Tmon(terminal) => tmon_adapter::mouse_mode(terminal.mouse_mode()),
        }
    }

    fn keyboard_mode(&self) -> TerminalKeyboardMode {
        match self {
            Self::Tmux(terminal) => terminal.keyboard_mode(),
            Self::Native(terminal) => terminal
                .lock()
                .map(|terminal| terminal.keyboard_mode())
                .unwrap_or_default(),
            Self::Tmon(terminal) => tmon_adapter::keyboard_mode(terminal.keyboard_mode()),
        }
    }

    fn with_grid<R>(
        &self,
        f: impl FnOnce(&alacritty_terminal::grid::Grid<alacritty_terminal::term::cell::Cell>) -> R,
    ) -> Option<R> {
        match self {
            Self::Tmux(terminal) => Some(terminal.with_term(|term| f(term.grid()))),
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    Some(terminal.with_term(|term| f(term.grid())))
                } else {
                    None
                }
            }
            Self::Tmon(_) => None,
        }
    }

    fn take_render_damage_snapshot(&self) -> TerminalRenderDamageSnapshot {
        match self {
            Self::Tmux(terminal) => {
                TerminalRenderDamageSnapshot::from_core(terminal.take_damage_snapshot())
            }
            Self::Native(terminal) => terminal.lock().map_or_else(
                |_| TerminalRenderDamageSnapshot::from_core(TerminalDamageSnapshot::Full),
                |terminal| TerminalRenderDamageSnapshot::from_core(terminal.take_damage_snapshot()),
            ),
            Self::Tmon(terminal) => {
                tmon_adapter::render_damage(terminal.take_render_damage_snapshot())
            }
        }
    }

    fn try_kitty_graphics_placements(&self) -> Option<Vec<KittyGraphicsRenderPlacement>> {
        match self {
            Self::Tmux(terminal) => Some(terminal.kitty_graphics_placements()),
            Self::Native(terminal) => terminal
                .lock()
                .ok()
                .map(|terminal| terminal.kitty_graphics_placements()),
            Self::Tmon(terminal) => Some(
                terminal
                    .kitty_graphics_placements()
                    .into_iter()
                    .map(tmon_adapter::kitty_graphics_placement)
                    .collect(),
            ),
        }
    }

    fn kitty_graphics_placements(&self) -> Vec<KittyGraphicsRenderPlacement> {
        self.try_kitty_graphics_placements().unwrap_or_default()
    }

    /// The OSC 8 hyperlink under the given viewport cell, if any.
    fn hyperlink_at(&self, row: usize, col: usize) -> Option<termy_terminal_ui::DetectedLink> {
        match self {
            Self::Tmux(terminal) => {
                terminal.with_term(|term| hyperlink_at_viewport_cell(term, row, col))
            }
            Self::Native(terminal) => terminal
                .lock()
                .ok()
                .and_then(|terminal| terminal.hyperlink_at(row, col)),
            Self::Tmon(terminal) => {
                terminal
                    .hyperlink_at(row, col)
                    .map(|link| termy_terminal_ui::DetectedLink {
                        start_col: link.start_col,
                        end_col: link.end_col,
                        target: link.target,
                    })
            }
        }
    }

    /// The OSC 8 or detected text link under the given viewport cell,
    /// including links spanning soft-wrapped rows.
    fn link_at(&self, row: usize, col: usize) -> Option<termy_terminal_ui::DetectedViewportLink> {
        match self {
            Self::Tmux(terminal) => {
                terminal.with_term(|term| link_at_viewport_cell(term, row, col))
            }
            Self::Native(terminal) => terminal
                .lock()
                .ok()
                .and_then(|terminal| terminal.link_at(row, col)),
            Self::Tmon(terminal) => terminal
                .link_at(row, col, |characters, hovered_index| {
                    find_link_in_line(characters, hovered_index).map(|link| tmon::LinkMatch {
                        start_col: link.start_col,
                        end_col: link.end_col,
                        target: link.target,
                    })
                })
                .map(|link| termy_terminal_ui::DetectedViewportLink {
                    start_row: link.start_row,
                    start_col: link.start_col,
                    end_row: link.end_row,
                    end_col: link.end_col,
                    target: link.target,
                }),
        }
    }

    fn for_each_renderable_cell(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, TerminalCellRef<'_>),
    ) -> Option<usize> {
        macro_rules! visit_term_cells {
            ($term:expr) => {{
                let content = $term.renderable_content();
                let display_offset = content.display_offset;
                for cell in content.display_iter {
                    visitor(
                        display_offset,
                        cell.point.line.0,
                        cell.point.column.0,
                        TerminalCellRef::Alacritty(cell.cell),
                    );
                }
                display_offset
            }};
        }

        match self {
            Self::Tmux(terminal) => Some(terminal.with_term(|term| visit_term_cells!(term))),
            Self::Native(terminal) => {
                if let Ok(terminal) = terminal.lock() {
                    Some(terminal.with_term(|term| visit_term_cells!(term)))
                } else {
                    None
                }
            }
            Self::Tmon(terminal) => Some(terminal.for_each_viewport_cell(
                |display_offset, line, col, cell, combining| {
                    visitor(
                        display_offset,
                        line,
                        col,
                        TerminalCellRef::Tmon(cell, combining),
                    );
                },
            )),
        }
    }

    fn for_each_full_rebuild_cell(
        &self,
        mut visitor: impl FnMut(usize, i32, usize, TerminalCellRef<'_>),
    ) -> Option<usize> {
        match self {
            Self::Tmon(terminal) => Some(
                terminal
                    .visit_viewport_cells_and_clear_damage(
                        |display_offset, line, col, cell, combining| {
                            visitor(
                                display_offset,
                                line,
                                col,
                                TerminalCellRef::Tmon(cell, combining),
                            );
                        },
                    )
                    .display_offset,
            ),
            Self::Tmux(_) | Self::Native(_) => self.for_each_renderable_cell(visitor),
        }
    }

    fn for_each_damage_cell(
        &self,
        spans: &[TerminalDirtySpan],
        generation: Option<u64>,
        mut visitor: impl FnMut(usize, usize, i32, usize, TerminalCellRef<'_>),
    ) -> bool {
        match self {
            Self::Tmon(terminal) => {
                let Some(generation) = generation else {
                    return false;
                };
                terminal
                    .for_each_viewport_range_at_generation(
                        generation,
                        spans
                            .iter()
                            .map(|span| (span.row, span.left_col, span.right_col)),
                        |row, display_offset, line, col, cell, combining| {
                            visitor(
                                row,
                                display_offset,
                                line,
                                col,
                                TerminalCellRef::Tmon(cell, combining),
                            );
                        },
                    )
                    .is_some()
            }
            Self::Tmux(_) | Self::Native(_) => self
                .with_grid(|grid| {
                    let display_offset = grid.display_offset();
                    let screen_lines = grid.screen_lines();
                    let cols = grid.columns();
                    for span in spans {
                        if span.row >= screen_lines || span.left_col >= cols {
                            continue;
                        }
                        let line = span.row as i32 - display_offset as i32;
                        let row = &grid[alacritty_terminal::index::Line(line)];
                        let right = span.right_col.min(cols.saturating_sub(1));
                        for col in span.left_col..=right {
                            visitor(
                                span.row,
                                display_offset,
                                line,
                                col,
                                TerminalCellRef::Alacritty(
                                    &row[alacritty_terminal::index::Column(col)],
                                ),
                            );
                        }
                    }
                    true
                })
                .unwrap_or(false),
        }
    }

    fn line_bounds(&self) -> Option<(i32, i32)> {
        match self {
            Self::Tmon(terminal) => Some(terminal.line_bounds()),
            Self::Tmux(_) | Self::Native(_) => self.with_grid(|grid| {
                let history = grid.total_lines().saturating_sub(grid.screen_lines());
                (
                    -(history as i32),
                    grid.screen_lines().saturating_sub(1) as i32,
                )
            }),
        }
    }

    /// Visit an inclusive line range while holding exactly one backing-terminal
    /// lock, so bounds, dimensions, and cells all belong to the same state.
    fn for_each_line_cell_range(
        &self,
        requested_first: i32,
        requested_last: i32,
        mut visitor: impl FnMut(TerminalLineRange, i32, usize, TerminalCellRef<'_>),
    ) -> Option<TerminalLineRange> {
        match self {
            Self::Tmon(terminal) => {
                let range = terminal.for_each_line_cell_range(
                    requested_first,
                    requested_last,
                    |range, line, col, cell, combining| {
                        visitor(
                            TerminalLineRange {
                                first_line: range.first_line,
                                last_line: range.last_line,
                                columns: range.columns,
                            },
                            line,
                            col,
                            TerminalCellRef::Tmon(cell, combining),
                        );
                    },
                );
                Some(TerminalLineRange {
                    first_line: range.first_line,
                    last_line: range.last_line,
                    columns: range.columns,
                })
            }
            Self::Tmux(_) | Self::Native(_) => self.with_grid(|grid| {
                let history = grid.total_lines().saturating_sub(grid.screen_lines());
                let range = TerminalLineRange {
                    first_line: -(history as i32),
                    last_line: grid.screen_lines().saturating_sub(1) as i32,
                    columns: grid.columns(),
                };
                if requested_first <= requested_last {
                    let first = requested_first.max(range.first_line);
                    let last = requested_last.min(range.last_line);
                    if first <= last {
                        for line in first..=last {
                            let row = &grid[alacritty_terminal::index::Line(line)];
                            for col in 0..range.columns {
                                visitor(
                                    range,
                                    line,
                                    col,
                                    TerminalCellRef::Alacritty(
                                        &row[alacritty_terminal::index::Column(col)],
                                    ),
                                );
                            }
                        }
                    }
                }
                range
            }),
        }
    }
}

struct TerminalPane {
    id: String,
    left: u16,
    top: u16,
    width: u16,
    height: u16,
    pane_zoom_steps: i16,
    degraded: bool,
    tmux_mouse_mode: Option<TmuxPaneMouseMode>,
    terminal: Terminal,
    // Progress reported by this pane's shell via OSC 9;4; the tab strip shows
    // the per-tab aggregate (TerminalTab::aggregate_progress_state).
    progress_state: ProgressState,
    render_cache: RefCell<TerminalPaneRenderCache>,
    /// Tracks the previous alternate-screen state so that transitions can be
    /// detected during `sync_terminal_size` and a SIGWINCH nudge sent.
    last_alternate_screen: Cell<bool>,
    /// Pre-computed element IDs to avoid per-frame `format!()` allocations.
    cached_element_ids: PaneCachedElementIds,
}

/// Pre-computed GPUI element IDs for a terminal pane, avoiding `format!()`
/// string allocations on every render frame.
struct PaneCachedElementIds {
    pane: SharedString,
    resize_handle_right: SharedString,
    resize_handle_bottom: SharedString,
    focus_accent: SharedString,
    degraded_accent: SharedString,
    drag_handle: SharedString,
}

impl PaneCachedElementIds {
    fn new(id: &str) -> Self {
        Self {
            pane: SharedString::from(format!("pane-{id}")),
            resize_handle_right: SharedString::from(format!("pane-resize-handle-right-{id}")),
            resize_handle_bottom: SharedString::from(format!("pane-resize-handle-bottom-{id}")),
            focus_accent: SharedString::from(format!("pane-focus-accent-{id}")),
            degraded_accent: SharedString::from(format!("pane-degraded-accent-{id}")),
            drag_handle: SharedString::from(format!("pane-drag-handle-{id}")),
        }
    }
}

impl TerminalPane {
    fn new_native(
        id: String,
        left: u16,
        top: u16,
        width: u16,
        height: u16,
        terminal: Terminal,
    ) -> Self {
        let cached_element_ids = PaneCachedElementIds::new(&id);
        Self {
            id,
            left,
            top,
            width,
            height,
            pane_zoom_steps: 0,
            degraded: false,
            tmux_mouse_mode: None,
            progress_state: ProgressState::default(),
            terminal,
            render_cache: RefCell::new(TerminalPaneRenderCache::default()),
            last_alternate_screen: Cell::new(false),
            cached_element_ids,
        }
    }

    fn terminal(&self) -> &Terminal {
        &self.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }
}

struct TerminalTab {
    id: TabId,
    window_id: String,
    window_index: i32,
    panes: Vec<TerminalPane>,
    active_pane_id: String,
    pinned: bool,
    manual_title: Option<String>,
    explicit_title: Option<String>,
    /// When `true`, `explicit_title` is a speculative seed derived from the
    /// initial working directory at tab creation—not a title confirmed by shell
    /// integration.  While this flag is set, `title_source_candidate` prefers a
    /// live `shell_title` over the prediction.  Cleared to `false` by any real
    /// explicit-title event (`set_explicit_title`, `activate_pending_command_title`)
    /// or by `clear_terminal_titles`.
    explicit_title_is_prediction: bool,
    shell_title: Option<String>,
    current_command: Option<String>,
    pending_command_title: Option<String>,
    pending_command_token: u64,
    last_prompt_cwd: Option<String>,
    title: String,
    title_text_width: f32,
    sticky_title_width: f32,
    display_width: f32,
    running_process: bool,
    command_lifecycle: CommandLifecycle,
}

struct NativePaneZoomSnapshot {
    other_panes: Vec<TerminalPane>,
    active_pane_geometry: (u16, u16, u16, u16),
    active_pane_id: String,
    active_original_index: usize,
    layout_tree: Option<NativePaneLayoutTree>,
}
impl TerminalTab {
    fn clear_render_caches(&self) {
        for pane in &self.panes {
            pane.render_cache.borrow_mut().clear();
        }
    }

    /// Tab-level progress summary across panes, by severity: any error wins,
    /// then warnings, then determinate progress (averaged across reporting
    /// panes), then indeterminate.
    fn aggregate_progress_state(&self) -> ProgressState {
        let mut warning: Option<u8> = None;
        let mut in_progress_sum = 0u32;
        let mut in_progress_count = 0u32;
        let mut has_indeterminate = false;
        for pane in &self.panes {
            match pane.progress_state {
                ProgressState::Error(percent) => return ProgressState::Error(percent),
                ProgressState::Warning(percent) => warning = warning.or(Some(percent)),
                ProgressState::InProgress(percent) => {
                    in_progress_sum += u32::from(percent);
                    in_progress_count += 1;
                }
                ProgressState::Indeterminate => has_indeterminate = true,
                ProgressState::Clear => {}
            }
        }
        if let Some(percent) = warning {
            return ProgressState::Warning(percent);
        }
        if let Some(average) = in_progress_sum.checked_div(in_progress_count) {
            return ProgressState::InProgress(average as u8);
        }
        if has_indeterminate {
            return ProgressState::Indeterminate;
        }
        ProgressState::Clear
    }

    fn has_active_pane(&self) -> bool {
        self.panes.iter().any(|pane| pane.id == self.active_pane_id)
    }

    fn assert_active_pane_invariant(&self) {
        assert!(
            self.panes.is_empty() || self.has_active_pane(),
            "tab {} is missing active pane {}",
            self.window_id,
            self.active_pane_id
        );
    }

    fn active_pane_index(&self) -> Option<usize> {
        self.panes
            .iter()
            .position(|pane| pane.id == self.active_pane_id)
            .or_else(|| (!self.panes.is_empty()).then_some(0))
    }

    fn active_terminal(&self) -> Option<&Terminal> {
        self.active_pane_index()
            .and_then(|index| self.panes.get(index))
            .map(TerminalPane::terminal)
    }

    fn active_pane_id(&self) -> Option<&str> {
        self.active_pane_index()
            .and_then(|index| self.panes.get(index))
            .map(|pane| pane.id.as_str())
    }
}

enum ExplicitTitlePayload {
    Prompt { title: String, cwd: String },
    Command { title: String, command: String },
    Title(String),
}

#[derive(Clone, Debug)]
struct ChildWorkingDirCacheEntry {
    value: Option<String>,
    resolved_at: Instant,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TabBarVisibility {
    #[default]
    FollowConfig,
    ForceVisible,
    ForceHidden,
}

/// The main terminal view component
pub struct TerminalView {
    tabs: Vec<TerminalTab>,
    /// All workspaces in sidebar order. The entry at `active_workspace` always
    /// has an empty `tabs` vec: the active workspace's tabs live in `tabs`
    /// above so the existing tab machinery only ever sees the visible strip.
    workspaces: Vec<workspaces::WorkspaceEntry>,
    active_workspace: usize,
    next_workspace_id: u64,
    workspace_sidebar_enabled: bool,
    workspace_sidebar_width: f32,
    workspace_sidebar_resize_drag: Option<workspaces::WorkspaceSidebarResizeDragState>,
    workspace_sidebar_collapsed: bool,
    workspace_sidebar_peek_visible: bool,
    workspace_drag: Option<workspaces::WorkspaceDragState>,
    renaming_workspace: Option<usize>,
    workspace_rename_input: InlineInputState,
    native_pane_zoom_snapshots: HashMap<TabId, NativePaneZoomSnapshot>,
    native_pane_layout_trees: HashMap<TabId, NativePaneLayoutTree>,
    next_tab_id: TabId,
    active_tab: usize,
    renaming_tab: Option<usize>,
    rename_input: InlineInputState,
    event_wakeup_tx: Sender<()>,
    native_terminal_wakeup_router: NativeTerminalWakeupRouter,
    native_terminal_wakeup_batch: HashSet<NativeTerminalWakeupId>,
    focus_handle: FocusHandle,
    theme_id: String,
    theme_mode: config::AppearanceMode,
    manual_theme: String,
    light_theme: String,
    dark_theme: String,
    custom_colors: config::CustomColors,
    system_appearance: SystemAppearance,
    appearance_subscription: Option<gpui::Subscription>,
    colors: TerminalColors,
    inactive_tab_scrollback: Option<usize>,
    tasks: Vec<TaskConfig>,
    warn_on_quit: bool,
    warn_on_quit_with_running_process: bool,
    tab_title: TabTitleConfig,
    tab_close_visibility: TabCloseVisibility,
    tab_width_mode: TabWidthMode,
    tab_bar_position: TabBarPosition,
    sidebar_collapsed: bool,
    last_viewport_width: f32,
    auto_hide_tabbar: bool,
    tab_bar_visibility: TabBarVisibility,
    new_tab_animation_tab_id: Option<TabId>,
    new_tab_animation_start: Option<Instant>,
    new_tab_animation_scheduled: bool,
    show_termy_in_titlebar: bool,
    tab_shell_integration: TabTitleShellIntegration,
    shell_integration_enabled: bool,
    macos_option_as_alt: bool,
    progress_indicator_enabled: bool,
    progress_indicator_animation_scheduled: bool,
    configured_working_dir: Option<String>,
    child_working_dir_cache: HashMap<u32, ChildWorkingDirCacheEntry>,
    child_working_dir_lookup_pending: HashSet<u32>,
    terminal_runtime: TerminalRuntimeConfig,
    runtime: RuntimeState,
    tmux_enabled_config: bool,
    native_tab_persistence: bool,
    native_layout_autosave: bool,
    native_buffer_persistence: bool,
    current_named_layout: Option<String>,
    native_persist_revision: Arc<AtomicU64>,
    native_persist_write_gate: Arc<Mutex<()>>,
    /// Lazily opened SQLite-backed session store; `None` once opening failed
    /// (logged), so persistence degrades to a no-op instead of retry spam.
    workspace_store: std::cell::OnceCell<Option<Arc<crate::workspace_store::WorkspaceStore>>>,
    tmux_show_active_pane_border: bool,
    tmux_exclusive: bool,
    config_path: Option<PathBuf>,
    config_fingerprint: Option<u64>,
    last_config_error_message: Option<String>,
    cached_tmux_binary: Option<String>,
    cached_tmux_command_prefix: Vec<String>,
    font_family: SharedString,
    ui_font_family: SharedString,
    base_font_size: f32,
    font_size: Pixels,
    cursor_style: AppCursorStyle,
    cursor_blink: bool,
    cursor_blink_visible: bool,
    last_cursor_input_at: Option<Instant>,
    background_opacity: f32,
    chrome_contrast: bool,
    background_opacity_cells: bool,
    preview_background_opacity: Option<config::BackgroundOpacityPreview>,
    background_blur: bool,
    background_support_context: BackgroundSupportContext,
    last_window_background_appearance: Option<WindowBackgroundAppearance>,
    warned_blur_unsupported_once: bool,
    padding_x: f32,
    padding_y: f32,
    mouse_scroll_multiplier: f32,
    pane_focus_effect: PaneFocusEffect,
    pane_focus_strength: f32,
    line_height: f32,
    copy_on_select: bool,
    copy_on_select_toast: bool,
    last_terminal_modifiers: gpui::Modifiers,
    pending_key_releases: HashMap<String, PendingKeyRelease>,
    deferred_ime_key_releases: HashSet<String>,
    selection_anchor: Option<SelectionPos>,
    selection_head: Option<SelectionPos>,
    selection_dragging: bool,
    selection_moved: bool,
    kitty_image_selection: Option<KittyImageSelection>,
    /// Tracks the active terminal's display_offset as observed from the UI thread.
    /// Updated after every user-initiated scroll and after each content-scroll adjustment,
    /// so that process_terminal_events can detect only content-driven offset changes.
    content_scroll_baseline: usize,
    pending_cursor_move_click: Option<PendingCursorMoveClick>,
    pending_cursor_move_preview: Option<PendingCursorMovePreview>,
    terminal_context_menu: Option<TerminalContextMenuState>,
    tab_context_menu: Option<TabContextMenuState>,
    /// Window-space top-left of the "+" dropdown for platform-specific tab
    /// choices; `None` while closed.
    new_tab_menu_anchor: Option<(f32, f32)>,
    saved_ssh_hosts: Vec<termy_ssh_core::SshHost>,
    hovered_link: Option<HoveredLink>,
    hovered_toast: Option<u64>,
    copied_toast_feedback: Option<(u64, Instant)>,
    toast_animation_scheduled: bool,
    toast_manager: ToastManager,
    overlay_view: Option<Entity<TerminalOverlayView>>,
    plugin_ui: Option<Entity<PluginUiView>>,
    plugin_runtime: PluginRuntime,
    plugin_lifecycle: PluginLifecycleState,
    plugin_refresh_in_flight: bool,
    plugin_last_error: Option<String>,
    launch_probe_scheduled: bool,
    command_palette: CommandPaletteState,
    simple_mode: bool,
    last_viewport_size_px: Option<(i32, i32)>,
    resize_indicator_dims: Option<(u16, u16)>,
    resize_indicator_visible_until: Option<Instant>,
    resize_indicator_animation_scheduled: bool,
    resize_throttle_task: Option<gpui::Task<()>>,
    last_resize_applied_at: Option<Instant>,
    last_terminal_resize_signature: Option<TerminalResizeSignature>,
    deferred_inactive_resize: DeferredTerminalResize,
    benchmark_session: Option<BenchmarkSession>,
    benchmark_exit_scheduled: bool,
    show_debug_overlay: bool,
    inspector: inspector::InspectorState,
    debug_overlay_stats: DebugOverlayStats,
    install_cli_available: bool,
    tab_strip: TabStripState,
    inline_input_selecting: bool,
    mouse_reporting: MouseReportingState,
    terminal_scroll_accumulator_y: f32,
    input_scroll_suppress_until: Option<Instant>,
    last_tmux_resize_error_at: Option<Instant>,
    terminal_scrollbar_visibility: TerminalScrollbarVisibility,
    terminal_scrollbar_style: TerminalScrollbarStyle,
    terminal_scrollbar_visibility_controller: ScrollbarVisibilityController,
    terminal_scrollbar_animation_active: bool,
    terminal_scrollbar_drag: Option<TerminalScrollbarDragState>,
    terminal_scrollbar_track_hold: Option<TerminalScrollbarTrackHoldState>,
    terminal_scrollbar_track_hold_active: bool,
    command_palette_scrollbar_drag: Option<TerminalScrollbarDragState>,
    command_palette_scrollbar_lane_bounds: Option<Bounds<Pixels>>,
    pane_resize_drag: Option<PaneResizeDragState>,
    pane_move_drag: Option<PaneMoveDragState>,
    hovered_pane_divider: Option<HoveredPaneDivider>,
    pane_resize_blocked: bool,
    terminal_scrollbar_marker_cache: TerminalScrollbarMarkerCache,
    /// Cached cell dimensions keyed by font-size bits.
    cell_size_cache: HashMap<u32, Size<Pixels>>,
    // Search state
    search_open: bool,
    search_input: InlineInputState,
    search_state: SearchState,
    search_debounce_token: u64,
    // IME composing state for terminal mode
    ime_marked_text: Option<String>,
    ime_selected_range: Option<Range<usize>>,
    // Pending clipboard write from OSC 52
    pending_clipboard: Option<String>,
    #[cfg(debug_assertions)]
    render_metrics: TerminalRenderMetricsState,
    quit_prompt_in_flight: bool,
    allow_quit_without_prompt: bool,
    auto_updater: Option<Entity<AutoUpdater>>,
    show_update_banner: bool,
    last_notified_update_state: Option<UpdateState>,
    update_check_toast_id: Option<u64>,
    #[cfg(target_os = "macos")]
    native_file_drop_enabled: bool,
}

impl TerminalView {
    fn native_leaf_rect(
        node: &NativePaneLayoutNode,
        target_pane_id: &str,
        rect: NativePaneRect,
    ) -> Option<NativePaneRect> {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => (pane_id == target_pane_id).then_some(rect),
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*axis, *ratio, rect);
                Self::native_leaf_rect(first, target_pane_id, first_rect)
                    .or_else(|| Self::native_leaf_rect(second, target_pane_id, second_rect))
            }
        }
    }

    fn native_tree_leaf_count(node: &NativePaneLayoutNode) -> usize {
        match node {
            NativePaneLayoutNode::Leaf { .. } => 1,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_tree_leaf_count(first) + Self::native_tree_leaf_count(second)
            }
        }
    }

    fn native_tree_first_leaf_id(node: &NativePaneLayoutNode) -> Option<String> {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => Some(pane_id.clone()),
            NativePaneLayoutNode::Split { first, .. } => Self::native_tree_first_leaf_id(first),
        }
    }

    fn native_tree_contains_leaf(node: &NativePaneLayoutNode, target_pane_id: &str) -> bool {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => pane_id == target_pane_id,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_tree_contains_leaf(first, target_pane_id)
                    || Self::native_tree_contains_leaf(second, target_pane_id)
            }
        }
    }

    fn native_axis_group_contains_leaf(
        node: &NativePaneLayoutNode,
        axis: PaneResizeAxis,
        target_pane_id: &str,
    ) -> bool {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => pane_id == target_pane_id,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                first,
                second,
                ..
            } if *split_axis == axis => {
                Self::native_axis_group_contains_leaf(first, axis, target_pane_id)
                    || Self::native_axis_group_contains_leaf(second, axis, target_pane_id)
            }
            NativePaneLayoutNode::Split { .. } => false,
        }
    }

    fn native_collect_axis_group_nodes(
        node: NativePaneLayoutNode,
        axis: PaneResizeAxis,
        nodes: &mut Vec<NativePaneLayoutNode>,
    ) {
        match node {
            NativePaneLayoutNode::Split {
                axis: split_axis,
                first,
                second,
                ..
            } if split_axis == axis => {
                Self::native_collect_axis_group_nodes(*first, axis, nodes);
                Self::native_collect_axis_group_nodes(*second, axis, nodes);
            }
            node => nodes.push(node),
        }
    }

    fn native_rebuild_even_axis_group(
        axis: PaneResizeAxis,
        mut nodes: Vec<NativePaneLayoutNode>,
    ) -> Option<NativePaneLayoutNode> {
        if nodes.len() <= 1 {
            return nodes.pop();
        }

        let total_count = nodes.len();
        let split_index = total_count / 2;
        let right_nodes = nodes.split_off(split_index);
        let first = Self::native_rebuild_even_axis_group(axis, nodes)
            .expect("balanced native split group must have a first branch");
        let second = Self::native_rebuild_even_axis_group(axis, right_nodes)
            .expect("balanced native split group must have a second branch");

        Some(NativePaneLayoutNode::Split {
            axis,
            ratio: split_index as f32 / total_count as f32,
            first: Box::new(first),
            second: Box::new(second),
        })
    }

    fn native_balance_axis_group(node: &mut NativePaneLayoutNode, axis: PaneResizeAxis) {
        let placeholder = NativePaneLayoutNode::Leaf {
            pane_id: String::new(),
        };
        let original = std::mem::replace(node, placeholder);
        let mut nodes = Vec::new();
        Self::native_collect_axis_group_nodes(original, axis, &mut nodes);
        if let Some(rebuilt) = Self::native_rebuild_even_axis_group(axis, nodes) {
            *node = rebuilt;
        }
    }

    fn native_balance_split_group_containing_leaf(
        node: &mut NativePaneLayoutNode,
        axis: PaneResizeAxis,
        pane_id: &str,
    ) -> bool {
        if matches!(
            node,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                ..
            } if *split_axis == axis
        ) && Self::native_axis_group_contains_leaf(node, axis, pane_id)
        {
            Self::native_balance_axis_group(node, axis);
            return true;
        }

        match node {
            NativePaneLayoutNode::Leaf { .. } => false,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_balance_split_group_containing_leaf(first, axis, pane_id)
                    || Self::native_balance_split_group_containing_leaf(second, axis, pane_id)
            }
        }
    }

    fn native_split_extent(axis: PaneResizeAxis, rect: NativePaneRect) -> u16 {
        match axis {
            PaneResizeAxis::Horizontal => rect.width,
            PaneResizeAxis::Vertical => rect.height,
        }
    }

    fn native_split_rects(
        axis: PaneResizeAxis,
        ratio: f32,
        rect: NativePaneRect,
    ) -> (NativePaneRect, NativePaneRect) {
        let total = Self::native_split_extent(axis, rect);
        let first_extent = if total <= 1 {
            1
        } else {
            ((f32::from(total) * ratio.clamp(0.0, 1.0)).round() as u16)
                .clamp(1, total.saturating_sub(1))
        };
        match axis {
            PaneResizeAxis::Horizontal => (
                NativePaneRect {
                    width: first_extent,
                    ..rect
                },
                NativePaneRect {
                    left: rect.left.saturating_add(first_extent),
                    width: total.saturating_sub(first_extent).max(1),
                    ..rect
                },
            ),
            PaneResizeAxis::Vertical => (
                NativePaneRect {
                    height: first_extent,
                    ..rect
                },
                NativePaneRect {
                    top: rect.top.saturating_add(first_extent),
                    height: total.saturating_sub(first_extent).max(1),
                    ..rect
                },
            ),
        }
    }

    fn native_collect_leaf_rects(
        node: &NativePaneLayoutNode,
        rect: NativePaneRect,
        rects: &mut HashMap<String, NativePaneRect>,
    ) {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => {
                rects.insert(pane_id.clone(), rect);
            }
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*axis, *ratio, rect);
                Self::native_collect_leaf_rects(first, first_rect, rects);
                Self::native_collect_leaf_rects(second, second_rect, rects);
            }
        }
    }

    fn native_coverage(intervals: &[(u16, u16)], start: u16, end: u16) -> u16 {
        if intervals.is_empty() || start >= end {
            return 0;
        }
        let mut merged = intervals.to_vec();
        merged
            .sort_unstable_by_key(|&(interval_start, interval_end)| (interval_start, interval_end));
        let mut total = 0u16;
        let mut current = merged[0];
        for interval in merged.into_iter().skip(1) {
            if interval.0 <= current.1 {
                current.1 = current.1.max(interval.1);
            } else {
                total = total.saturating_add(current.1.saturating_sub(current.0));
                current = interval;
            }
        }
        total
            .saturating_add(current.1.saturating_sub(current.0))
            .min(end.saturating_sub(start))
    }

    fn native_tree_can_split_at_boundary(
        panes: &[&TerminalPane],
        rect: NativePaneRect,
        axis: PaneResizeAxis,
        boundary: u16,
    ) -> bool {
        let mut first_count = 0usize;
        let mut second_count = 0usize;
        let mut first_intervals = Vec::new();
        let mut second_intervals = Vec::new();

        for pane in panes {
            let pane_rect = NativePaneRect {
                left: pane.left,
                top: pane.top,
                width: pane.width,
                height: pane.height,
            };
            match axis {
                PaneResizeAxis::Horizontal => {
                    if pane_rect.right() <= boundary {
                        first_count += 1;
                        first_intervals.push((
                            pane_rect.top.max(rect.top),
                            pane_rect.bottom().min(rect.bottom()),
                        ));
                    } else if pane_rect.left >= boundary {
                        second_count += 1;
                        second_intervals.push((
                            pane_rect.top.max(rect.top),
                            pane_rect.bottom().min(rect.bottom()),
                        ));
                    } else {
                        return false;
                    }
                }
                PaneResizeAxis::Vertical => {
                    if pane_rect.bottom() <= boundary {
                        first_count += 1;
                        first_intervals.push((
                            pane_rect.left.max(rect.left),
                            pane_rect.right().min(rect.right()),
                        ));
                    } else if pane_rect.top >= boundary {
                        second_count += 1;
                        second_intervals.push((
                            pane_rect.left.max(rect.left),
                            pane_rect.right().min(rect.right()),
                        ));
                    } else {
                        return false;
                    }
                }
            }
        }

        if first_count == 0 || second_count == 0 {
            return false;
        }

        match axis {
            PaneResizeAxis::Horizontal => {
                Self::native_coverage(&first_intervals, rect.top, rect.bottom()) >= rect.height
                    && Self::native_coverage(&second_intervals, rect.top, rect.bottom())
                        >= rect.height
            }
            PaneResizeAxis::Vertical => {
                Self::native_coverage(&first_intervals, rect.left, rect.right()) >= rect.width
                    && Self::native_coverage(&second_intervals, rect.left, rect.right())
                        >= rect.width
            }
        }
    }

    fn native_infer_layout_tree_from_rects(
        panes: &[&TerminalPane],
        rect: NativePaneRect,
    ) -> Option<NativePaneLayoutNode> {
        if panes.len() == 1 {
            return Some(NativePaneLayoutNode::Leaf {
                pane_id: panes[0].id.clone(),
            });
        }

        let right_boundaries = panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .filter(|boundary| *boundary > rect.left && *boundary < rect.right())
            .collect::<Vec<_>>();
        for boundary in right_boundaries {
            if !Self::native_tree_can_split_at_boundary(
                panes,
                rect,
                PaneResizeAxis::Horizontal,
                boundary,
            ) {
                continue;
            }
            let (first_panes, second_panes): (Vec<_>, Vec<_>) = panes
                .iter()
                .copied()
                .partition(|pane| pane.left.saturating_add(pane.width) <= boundary);
            let first_rect = NativePaneRect {
                width: boundary.saturating_sub(rect.left),
                ..rect
            };
            let second_rect = NativePaneRect {
                left: boundary,
                width: rect.right().saturating_sub(boundary),
                ..rect
            };
            let first = Self::native_infer_layout_tree_from_rects(&first_panes, first_rect)?;
            let second = Self::native_infer_layout_tree_from_rects(&second_panes, second_rect)?;
            return Some(NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Horizontal,
                ratio: f32::from(first_rect.width) / f32::from(rect.width.max(1)),
                first: Box::new(first),
                second: Box::new(second),
            });
        }

        let bottom_boundaries = panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .filter(|boundary| *boundary > rect.top && *boundary < rect.bottom())
            .collect::<Vec<_>>();
        for boundary in bottom_boundaries {
            if !Self::native_tree_can_split_at_boundary(
                panes,
                rect,
                PaneResizeAxis::Vertical,
                boundary,
            ) {
                continue;
            }
            let (first_panes, second_panes): (Vec<_>, Vec<_>) = panes
                .iter()
                .copied()
                .partition(|pane| pane.top.saturating_add(pane.height) <= boundary);
            let first_rect = NativePaneRect {
                height: boundary.saturating_sub(rect.top),
                ..rect
            };
            let second_rect = NativePaneRect {
                top: boundary,
                height: rect.bottom().saturating_sub(boundary),
                ..rect
            };
            let first = Self::native_infer_layout_tree_from_rects(&first_panes, first_rect)?;
            let second = Self::native_infer_layout_tree_from_rects(&second_panes, second_rect)?;
            return Some(NativePaneLayoutNode::Split {
                axis: PaneResizeAxis::Vertical,
                ratio: f32::from(first_rect.height) / f32::from(rect.height.max(1)),
                first: Box::new(first),
                second: Box::new(second),
            });
        }

        None
    }

    fn native_layout_tree_from_panes(panes: &[TerminalPane]) -> Option<NativePaneLayoutTree> {
        let only = panes.first()?;
        if panes.len() == 1 {
            return Some(NativePaneLayoutTree {
                root: NativePaneLayoutNode::Leaf {
                    pane_id: only.id.clone(),
                },
            });
        }
        let cols = panes
            .iter()
            .map(|pane| pane.left.saturating_add(pane.width))
            .max()
            .unwrap_or(only.width)
            .max(1);
        let rows = panes
            .iter()
            .map(|pane| pane.top.saturating_add(pane.height))
            .max()
            .unwrap_or(only.height)
            .max(1);
        let pane_refs = panes.iter().collect::<Vec<_>>();
        Self::native_infer_layout_tree_from_rects(
            &pane_refs,
            NativePaneRect {
                left: 0,
                top: 0,
                width: cols,
                height: rows,
            },
        )
        .map(|root| NativePaneLayoutTree { root })
    }

    fn ensure_native_layout_tree_for_tab_id(&mut self, tab_id: TabId) -> bool {
        if self.native_pane_layout_trees.contains_key(&tab_id) {
            return true;
        }
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return false;
        };
        let Some(tree) = Self::native_layout_tree_from_panes(&tab.panes) else {
            return false;
        };
        self.native_pane_layout_trees.insert(tab_id, tree);
        true
    }

    fn apply_native_layout_tree_to_tab(&mut self, tab_id: TabId, cols: u16, rows: u16) -> bool {
        let Some(tree) = self.native_pane_layout_trees.get(&tab_id).cloned() else {
            return false;
        };
        let Some(tab_index) = self.tab_index_by_id(tab_id) else {
            return false;
        };
        let Some(tab) = self.tabs.get_mut(tab_index) else {
            return false;
        };
        let mut rects = HashMap::new();
        Self::native_collect_leaf_rects(
            &tree.root,
            NativePaneRect {
                left: 0,
                top: 0,
                width: cols.max(1),
                height: rows.max(1),
            },
            &mut rects,
        );
        for pane in &mut tab.panes {
            if let Some(rect) = rects.get(&pane.id).copied() {
                pane.left = rect.left;
                pane.top = rect.top;
                pane.width = rect.width.max(1);
                pane.height = rect.height.max(1);
            }
        }
        true
    }

    fn native_replace_leaf_with_split(
        node: &mut NativePaneLayoutNode,
        target_pane_id: &str,
        axis: PaneResizeAxis,
        new_pane_id: &str,
    ) -> bool {
        Self::native_replace_leaf_with_split_ordered(node, target_pane_id, axis, new_pane_id, false)
    }

    /// Replace the `target_pane_id` leaf with an even split between it and a
    /// new leaf. `new_first` places the new leaf on the left/top side.
    fn native_replace_leaf_with_split_ordered(
        node: &mut NativePaneLayoutNode,
        target_pane_id: &str,
        axis: PaneResizeAxis,
        new_pane_id: &str,
        new_first: bool,
    ) -> bool {
        if Self::native_tree_contains_leaf(node, new_pane_id) {
            return false;
        }

        match node {
            NativePaneLayoutNode::Leaf { pane_id } if pane_id == target_pane_id => {
                let existing = NativePaneLayoutNode::Leaf {
                    pane_id: pane_id.clone(),
                };
                let added = NativePaneLayoutNode::Leaf {
                    pane_id: new_pane_id.to_string(),
                };
                let (first, second) = if new_first {
                    (added, existing)
                } else {
                    (existing, added)
                };
                *node = NativePaneLayoutNode::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            NativePaneLayoutNode::Leaf { .. } => false,
            NativePaneLayoutNode::Split { first, second, .. } => {
                Self::native_replace_leaf_with_split_ordered(
                    first,
                    target_pane_id,
                    axis,
                    new_pane_id,
                    new_first,
                ) || Self::native_replace_leaf_with_split_ordered(
                    second,
                    target_pane_id,
                    axis,
                    new_pane_id,
                    new_first,
                )
            }
        }
    }

    /// Swap two leaves in the layout tree by renaming their pane ids.
    /// Returns `true` only when both leaves were found.
    fn native_swap_leaves(
        node: &mut NativePaneLayoutNode,
        first_id: &str,
        second_id: &str,
    ) -> bool {
        fn walk(node: &mut NativePaneLayoutNode, first_id: &str, second_id: &str) -> (bool, bool) {
            match node {
                NativePaneLayoutNode::Leaf { pane_id } => {
                    if pane_id == first_id {
                        *pane_id = second_id.to_string();
                        (true, false)
                    } else if pane_id == second_id {
                        *pane_id = first_id.to_string();
                        (false, true)
                    } else {
                        (false, false)
                    }
                }
                NativePaneLayoutNode::Split { first, second, .. } => {
                    let left = walk(first, first_id, second_id);
                    let right = walk(second, first_id, second_id);
                    (left.0 || right.0, left.1 || right.1)
                }
            }
        }

        let (found_first, found_second) = walk(node, first_id, second_id);
        found_first && found_second
    }

    fn native_adjust_tree_split(
        node: &mut NativePaneLayoutNode,
        pane_id: &str,
        axis: PaneResizeAxis,
        edge: PaneResizeEdge,
        divider_delta: i16,
        rect: NativePaneRect,
        min_extent: u16,
    ) -> PaneResizeResult {
        match node {
            NativePaneLayoutNode::Leaf { .. } => PaneResizeResult::NoChange,
            NativePaneLayoutNode::Split {
                axis: split_axis,
                ratio,
                first,
                second,
            } => {
                let (first_rect, second_rect) = Self::native_split_rects(*split_axis, *ratio, rect);
                let first_leaf_rect = Self::native_leaf_rect(first, pane_id, first_rect);
                let second_leaf_rect = Self::native_leaf_rect(second, pane_id, second_rect);

                if *split_axis == axis {
                    let total = Self::native_split_extent(axis, rect).max(1);
                    let first_extent = Self::native_split_extent(axis, first_rect);
                    let touches_boundary = match axis {
                        PaneResizeAxis::Horizontal => {
                            (edge == PaneResizeEdge::Right
                                && first_leaf_rect
                                    .is_some_and(|leaf| leaf.right() == first_rect.right()))
                                || (edge == PaneResizeEdge::Left
                                    && second_leaf_rect
                                        .is_some_and(|leaf| leaf.left == second_rect.left))
                        }
                        PaneResizeAxis::Vertical => {
                            (edge == PaneResizeEdge::Bottom
                                && first_leaf_rect
                                    .is_some_and(|leaf| leaf.bottom() == first_rect.bottom()))
                                || (edge == PaneResizeEdge::Top
                                    && second_leaf_rect
                                        .is_some_and(|leaf| leaf.top == second_rect.top))
                        }
                    };

                    if touches_boundary {
                        let next_first_extent = i32::from(first_extent) + i32::from(divider_delta);
                        let next_second_extent = i32::from(total) - next_first_extent;
                        if next_first_extent < i32::from(min_extent)
                            || next_second_extent < i32::from(min_extent)
                        {
                            return PaneResizeResult::BlockedByMinimum;
                        }
                        *ratio = (next_first_extent as f32 / f32::from(total)).clamp(0.0, 1.0);
                        return PaneResizeResult::Applied;
                    }
                }

                let first_result = Self::native_adjust_tree_split(
                    first,
                    pane_id,
                    axis,
                    edge,
                    divider_delta,
                    first_rect,
                    min_extent,
                );
                if first_result != PaneResizeResult::NoChange {
                    return first_result;
                }
                Self::native_adjust_tree_split(
                    second,
                    pane_id,
                    axis,
                    edge,
                    divider_delta,
                    second_rect,
                    min_extent,
                )
            }
        }
    }

    fn native_remove_leaf_from_tree(
        node: NativePaneLayoutNode,
        pane_id: &str,
    ) -> (Option<NativePaneLayoutNode>, Option<String>, bool) {
        match node {
            NativePaneLayoutNode::Leaf { pane_id: leaf_id } => {
                if leaf_id == pane_id {
                    (None, None, true)
                } else {
                    (
                        Some(NativePaneLayoutNode::Leaf { pane_id: leaf_id }),
                        None,
                        false,
                    )
                }
            }
            NativePaneLayoutNode::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let original_first = *first;
                let original_second = *second;
                let (next_first, first_focus, removed) =
                    Self::native_remove_leaf_from_tree(original_first.clone(), pane_id);
                if removed {
                    return if let Some(next_first) = next_first {
                        (
                            Some(NativePaneLayoutNode::Split {
                                axis,
                                ratio,
                                first: Box::new(next_first),
                                second: Box::new(original_second),
                            }),
                            first_focus,
                            true,
                        )
                    } else {
                        let focus_id = first_focus
                            .or_else(|| Self::native_tree_first_leaf_id(&original_second));
                        (Some(original_second), focus_id, true)
                    };
                }

                let (next_second, second_focus, removed) =
                    Self::native_remove_leaf_from_tree(original_second.clone(), pane_id);
                if removed {
                    return if let Some(next_second) = next_second {
                        (
                            Some(NativePaneLayoutNode::Split {
                                axis,
                                ratio,
                                first: Box::new(original_first),
                                second: Box::new(next_second),
                            }),
                            second_focus,
                            true,
                        )
                    } else {
                        let focus_id = second_focus
                            .or_else(|| Self::native_tree_first_leaf_id(&original_first));
                        (Some(original_first), focus_id, true)
                    };
                }

                (
                    Some(NativePaneLayoutNode::Split {
                        axis,
                        ratio,
                        first: Box::new(original_first),
                        second: Box::new(original_second),
                    }),
                    None,
                    false,
                )
            }
        }
    }

    fn install_cli_availability_from_probe(is_cli_installed: bool) -> bool {
        !is_cli_installed
    }

    fn install_cli_available_from_system() -> bool {
        Self::install_cli_availability_from_probe(termy_cli_install_core::is_cli_installed())
    }

    fn refreshed_install_cli_availability(
        current_available: bool,
        is_cli_installed: bool,
    ) -> (bool, bool) {
        let next_available = Self::install_cli_availability_from_probe(is_cli_installed);
        (next_available, next_available != current_available)
    }

    pub(super) fn install_cli_available(&self) -> bool {
        self.install_cli_available
    }

    pub(super) fn update_banner_visible(&self) -> bool {
        self.show_update_banner
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn set_native_file_drop_enabled(&mut self, enabled: bool) {
        self.native_file_drop_enabled = enabled;
    }

    pub(super) fn refresh_install_cli_availability(&mut self) -> bool {
        let (next_available, changed) = Self::refreshed_install_cli_availability(
            self.install_cli_available,
            termy_cli_install_core::is_cli_installed(),
        );
        self.install_cli_available = next_available;
        changed
    }

    fn runtime_config_from_app_config(
        config: &AppConfig,
        colors: &TerminalColors,
    ) -> TerminalRuntimeConfig {
        let working_dir_fallback = match config.working_dir_fallback {
            config::WorkingDirFallback::Home => RuntimeWorkingDirFallback::Home,
            config::WorkingDirFallback::Process => RuntimeWorkingDirFallback::Process,
        };

        TerminalRuntimeConfig {
            shell: config.shell.clone(),
            windows_shell: match config.windows_shell {
                config::WindowsShell::Cmd => RuntimeWindowsShell::Cmd,
                config::WindowsShell::PowerShell => RuntimeWindowsShell::PowerShell,
                config::WindowsShell::PowerShellCore => RuntimeWindowsShell::PowerShellCore,
                config::WindowsShell::GitBash => RuntimeWindowsShell::GitBash,
            },
            term: config.term.clone(),
            colorterm: config.colorterm.clone(),
            environment: Default::default(),
            query_colors: Self::terminal_query_colors(colors),
            working_dir_fallback,
            scrollback_history: config.scrollback_history,
            default_cursor_style: match config.cursor_style {
                AppCursorStyle::Line => TerminalCursorStyle::Line,
                AppCursorStyle::Block => TerminalCursorStyle::Block,
            },
        }
    }

    fn terminal_query_colors(colors: &TerminalColors) -> TerminalQueryColors {
        TerminalQueryColors {
            ansi: colors.ansi.map(Self::ansi_rgb_from_rgba),
            foreground: Self::ansi_rgb_from_rgba(colors.foreground),
            background: Self::ansi_rgb_from_rgba(colors.background),
            cursor: None,
        }
    }

    fn ansi_rgb_from_rgba(color: gpui::Rgba) -> alacritty_terminal::vte::ansi::Rgb {
        let to_u8 = |component: f32| (component.clamp(0.0, 1.0) * 255.0).round() as u8;
        alacritty_terminal::vte::ansi::Rgb {
            r: to_u8(color.r),
            g: to_u8(color.g),
            b: to_u8(color.b),
        }
    }

    #[cfg(test)]
    fn uses_event_driven_tmux_wakeup() -> bool {
        true
    }

    fn user_home_dir() -> Option<PathBuf> {
        dirs::home_dir()
    }

    fn display_working_directory_for_prompt(path: &Path) -> String {
        if let Some(home) = Self::user_home_dir() {
            if path == home.as_path() {
                return "~".to_string();
            }

            if let Ok(relative) = path.strip_prefix(&home) {
                let relative = relative.to_string_lossy();
                return format!("~{}{}", std::path::MAIN_SEPARATOR, relative);
            }
        }

        path.to_string_lossy().into_owned()
    }

    fn predicted_prompt_cwd(
        configured_working_dir: Option<&str>,
        fallback: RuntimeWorkingDirFallback,
    ) -> Option<String> {
        let path = resolve_launch_working_directory(configured_working_dir, fallback)?;
        Some(path.to_string_lossy().into_owned())
    }

    fn working_dir_title_candidate(value: &str) -> Option<&str> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }

        if let Some(prompt) = value
            .rsplit_once("prompt:")
            .map(|(_, prompt)| prompt.trim())
            && !prompt.is_empty()
        {
            return Some(prompt);
        }

        if let Some(cwd) = value.strip_prefix("cwd:").map(str::trim)
            && !cwd.is_empty()
        {
            return Some(cwd);
        }

        Some(value)
    }

    fn looks_like_working_dir_path(value: &str) -> bool {
        value.starts_with(std::path::MAIN_SEPARATOR)
            || value == "~"
            || value.starts_with("~/")
            || value.starts_with("~\\")
            || value.chars().nth(1).is_some_and(|ch| ch == ':')
                && value
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                && value
                    .chars()
                    .nth(2)
                    .is_some_and(|sep| sep == '/' || sep == '\\')
    }

    fn working_dir_for_child_pid_blocking(pid: u32) -> Option<String> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        {
            let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
            return path.is_dir().then(|| path.to_string_lossy().into_owned());
        }

        #[cfg(target_os = "macos")]
        {
            let output = Command::new("lsof")
                .args(["-a", "-d", "cwd", "-p", &pid.to_string(), "-Fn"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(path) = line.strip_prefix('n') {
                    let path = path.trim();
                    if !path.is_empty() {
                        return Some(path.to_string());
                    }
                }
            }
            None
        }

        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
        {
            let _ = pid;
            None
        }
    }

    fn complete_child_working_dir_lookup(&mut self, pid: u32, value: Option<String>) {
        self.child_working_dir_lookup_pending.remove(&pid);
        self.child_working_dir_cache.insert(
            pid,
            ChildWorkingDirCacheEntry {
                value,
                resolved_at: Instant::now(),
            },
        );
    }

    fn schedule_child_working_dir_lookup(&mut self, pid: u32, cx: &mut Context<Self>) {
        if !self.child_working_dir_lookup_pending.insert(pid) {
            return;
        }

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let value = smol::unblock(move || Self::working_dir_for_child_pid_blocking(pid)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, _cx| {
                    view.complete_child_working_dir_lookup(pid, value);
                })
            });
        })
        .detach();
    }

    fn cached_child_working_dir_for_pid(
        &mut self,
        pid: u32,
        cx: &mut Context<Self>,
    ) -> Option<Option<String>> {
        let (cached_value, resolved_at) = self
            .child_working_dir_cache
            .get(&pid)
            .map(|entry| (entry.value.clone(), entry.resolved_at))?;
        let is_fresh =
            Instant::now().saturating_duration_since(resolved_at) <= CHILD_WORKING_DIR_CACHE_TTL;
        if !is_fresh {
            self.schedule_child_working_dir_lookup(pid, cx);
        }
        Some(cached_value)
    }

    fn immediate_process_cwd_for_session_creation(
        cached_value: Option<&str>,
        pid: u32,
    ) -> Option<String> {
        normalize_working_directory_candidate(cached_value)
            .or_else(|| Self::working_dir_for_child_pid_blocking(pid))
    }

    pub(in crate::terminal_view) fn cached_or_resolved_working_dir_for_child_pid(
        &mut self,
        pid: u32,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if let Some(cached_value) = self.cached_child_working_dir_for_pid(pid, cx) {
            return cached_value;
        }

        let value = Self::immediate_process_cwd_for_session_creation(None, pid);
        self.complete_child_working_dir_lookup(pid, value.clone());
        if value.is_none() {
            self.schedule_child_working_dir_lookup(pid, cx);
        }
        value
    }

    fn inherited_working_dir_candidate(candidate: Option<&str>) -> Option<String> {
        // Session-derived cwds (prompt/process/title) can name paths from another
        // filesystem namespace — a shell inside WSL or over SSH reports its remote
        // cwd. A candidate that does not resolve locally must fall through to the
        // configured working_dir instead of shadowing it and collapsing to the
        // home-directory fallback at spawn time (issue #336).
        normalize_working_directory_candidate(candidate)
            .filter(|value| resolve_working_directory_path(Some(value)).is_some())
    }

    fn resolve_preferred_working_directory(
        explicit_working_dir: Option<&str>,
        prompt_cwd: Option<&str>,
        process_cwd: Option<&str>,
        title_cwd: Option<&str>,
        configured_working_dir: Option<&str>,
        fallback: RuntimeWorkingDirFallback,
    ) -> Option<String> {
        // Keep tmux and native session creation on the same cwd precedence chain so
        // new tabs/panes do not drift based on which runtime happens to be active.
        let explicit_working_dir = normalize_working_directory_candidate(explicit_working_dir);
        let prompt_cwd = Self::inherited_working_dir_candidate(prompt_cwd);
        let process_cwd = Self::inherited_working_dir_candidate(process_cwd);
        let title_cwd = title_cwd
            .map(str::trim)
            .filter(|value| Self::looks_like_working_dir_path(value))
            .and_then(|value| Self::inherited_working_dir_candidate(Some(value)));

        explicit_working_dir
            .or(prompt_cwd)
            .or(process_cwd)
            .or(title_cwd)
            .or_else(|| {
                resolve_launch_working_directory(configured_working_dir, fallback)
                    .map(|path| path.to_string_lossy().into_owned())
            })
    }

    fn preferred_working_dir_for_new_session(
        &mut self,
        explicit_working_dir: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let active_tab = self.active_tab;
        let prompt_cwd = self
            .tabs
            .get(active_tab)
            .and_then(|tab| tab.last_prompt_cwd.clone());
        let process_cwd = self
            .tabs
            .get(active_tab)
            .and_then(TerminalTab::active_terminal)
            .and_then(Terminal::child_pid)
            .and_then(|pid| self.cached_or_resolved_working_dir_for_child_pid(pid, cx));
        let title_cwd = self
            .tabs
            .get(active_tab)
            .and_then(|tab| {
                [
                    tab.explicit_title.as_deref(),
                    tab.shell_title.as_deref(),
                    Some(tab.title.as_str()),
                ]
                .into_iter()
                .flatten()
                .find_map(Self::working_dir_title_candidate)
            })
            .map(|candidate| candidate.to_string());

        Self::resolve_preferred_working_directory(
            explicit_working_dir,
            prompt_cwd.as_deref(),
            process_cwd.as_deref(),
            title_cwd.as_deref(),
            self.configured_working_dir.as_deref(),
            self.terminal_runtime.working_dir_fallback,
        )
    }

    fn runtime_kind(&self) -> RuntimeKind {
        self.runtime.kind()
    }

    fn runtime_uses_tmux(&self) -> bool {
        self.runtime_kind().uses_tmux()
    }

    fn tmux_runtime(&self) -> &TmuxRuntime {
        self.runtime
            .as_tmux()
            .expect("tmux runtime must exist while tmux backend is active")
    }

    fn tmux_runtime_mut(&mut self) -> &mut TmuxRuntime {
        self.runtime
            .as_tmux_mut()
            .expect("tmux runtime must exist while tmux backend is active")
    }

    fn create_native_tab(
        tab_id: TabId,
        terminal: Terminal,
        cols: u16,
        rows: u16,
        predicted_prompt_title: Option<String>,
    ) -> TerminalTab {
        let explicit_title_is_prediction = predicted_prompt_title.is_some();
        let title = predicted_prompt_title
            .as_deref()
            .unwrap_or(DEFAULT_TAB_TITLE)
            .to_string();
        let title_text_width = 0.0;
        let sticky_title_width = Self::tab_display_width_for_text_px_without_close_with_max(
            title_text_width,
            TAB_MAX_WIDTH,
        );
        let display_width =
            Self::tab_display_width_for_text_px_with_max(title_text_width, TAB_MAX_WIDTH);
        let pane_id = format!("%native-{tab_id}");
        let pane = TerminalPane {
            id: pane_id.clone(),
            ..TerminalPane::new_native(pane_id.clone(), 0, 0, cols.max(1), rows.max(1), terminal)
        };
        TerminalTab {
            id: tab_id,
            window_id: format!("@native-{tab_id}"),
            window_index: 0,
            panes: vec![pane],
            active_pane_id: pane_id,
            pinned: false,
            manual_title: None,
            explicit_title: predicted_prompt_title,
            explicit_title_is_prediction,
            shell_title: None,
            current_command: None,
            pending_command_title: None,
            pending_command_token: 0,
            last_prompt_cwd: None,
            title,
            title_text_width,
            sticky_title_width,
            display_width,
            running_process: false,
            command_lifecycle: CommandLifecycle::default(),
        }
    }

    fn pane_terminal_by_id(&self, pane_id: &str) -> Option<&Terminal> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .find(|pane| pane.id == pane_id)
            .map(TerminalPane::terminal)
    }

    fn is_active_pane_id(&self, pane_id: &str) -> bool {
        self.tabs
            .get(self.active_tab)
            .and_then(|tab| tab.active_pane_id())
            == Some(pane_id)
    }

    fn active_pane_id(&self) -> Option<&str> {
        self.tabs
            .get(self.active_tab)
            .and_then(|tab| tab.active_pane_id())
    }

    fn active_tab_ref(&self) -> Option<&TerminalTab> {
        self.tabs.get(self.active_tab)
    }

    fn active_pane_ref(&self) -> Option<&TerminalPane> {
        let tab = self.active_tab_ref()?;
        let index = tab.active_pane_index()?;
        tab.panes.get(index)
    }

    fn background_opacity_factor(&self) -> f32 {
        background_opacity_factor(self.effective_background_opacity())
    }

    fn scaled_background_alpha(&self, base_alpha: f32) -> f32 {
        scaled_background_alpha_for_opacity(base_alpha, self.effective_background_opacity())
    }

    fn scaled_chrome_alpha(&self, base_alpha: f32) -> f32 {
        scaled_chrome_alpha_for_opacity(base_alpha, self.effective_background_opacity())
    }

    fn chrome_contrast_profile(&self) -> ChromeContrastProfile {
        ChromeContrastProfile::from_enabled(self.chrome_contrast)
    }

    fn scaled_chrome_surface_alpha(&self, base_alpha: f32) -> f32 {
        scaled_chrome_alpha_for_opacity(
            self.chrome_contrast_profile().surface_alpha(base_alpha),
            self.effective_background_opacity(),
        )
    }

    fn scaled_chrome_neutral_border_alpha(&self, base_alpha: f32) -> f32 {
        scaled_chrome_alpha_for_opacity(
            self.chrome_contrast_profile()
                .neutral_border_alpha(base_alpha),
            self.effective_background_opacity(),
        )
    }

    fn scaled_chrome_accent_alpha(&self, base_alpha: f32) -> f32 {
        scaled_chrome_alpha_for_opacity(
            self.chrome_contrast_profile().accent_alpha(base_alpha),
            self.effective_background_opacity(),
        )
    }

    fn effective_background_opacity(&self) -> f32 {
        config::effective_background_opacity(
            self.background_opacity,
            self.preview_background_opacity,
        )
    }

    fn tab_switch_hints_blocked(&self) -> bool {
        self.is_command_palette_open() || self.plugin_ui.is_some() || self.search_open
    }

    pub(crate) fn tab_switch_hint_progress(&self, now: Instant) -> f32 {
        self.tab_strip
            .switch_hints
            .progress(now, self.tab_switch_hints_blocked())
    }

    fn schedule_tab_switch_hint_animation(&mut self, cx: &mut Context<Self>) {
        if !self
            .tab_strip
            .switch_hints
            .begin_animation_frame(Instant::now(), self.tab_switch_hints_blocked())
        {
            return;
        }

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(TAB_SWITCH_HINT_ANIMATION_FRAME_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.tab_strip.switch_hints.finish_animation_frame();
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(crate) fn schedule_tab_interaction_animation(&mut self, cx: &mut Context<Self>) {
        if self.tab_strip.interaction_animation_scheduled
            || !self.tab_strip.interaction_animation_active(Instant::now())
        {
            return;
        }
        self.tab_strip.interaction_animation_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(TAB_INTERACTION_ANIMATION_FRAME_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.tab_strip.interaction_animation_scheduled = false;
                    if view.tab_strip.interaction_animation_active(Instant::now()) {
                        view.schedule_tab_interaction_animation(cx);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(crate) fn start_new_tab_animation(&mut self, tab_id: TabId, cx: &mut Context<Self>) {
        self.new_tab_animation_tab_id = Some(tab_id);
        self.new_tab_animation_start = Some(Instant::now());
        self.new_tab_animation_scheduled = false;
        self.schedule_new_tab_animation(cx);
    }

    fn schedule_new_tab_animation(&mut self, cx: &mut Context<Self>) {
        if self.new_tab_animation_scheduled {
            return;
        }
        self.new_tab_animation_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(NEW_TAB_ANIMATION_FRAME_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.new_tab_animation_scheduled = false;
                    let still_animating = view.new_tab_animation_start.is_some_and(|start| {
                        Instant::now().saturating_duration_since(start) < NEW_TAB_ANIMATION_DURATION
                    });
                    view.mark_tab_strip_layout_dirty();
                    if still_animating {
                        view.schedule_new_tab_animation(cx);
                    } else {
                        view.new_tab_animation_tab_id = None;
                        view.new_tab_animation_start = None;
                    }
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(crate) fn new_tab_animation_progress(&self, now: Instant) -> Option<(usize, f32)> {
        let tab_id = self.new_tab_animation_tab_id?;
        let start = self.new_tab_animation_start?;
        let elapsed = now.saturating_duration_since(start).as_secs_f32();
        let total = NEW_TAB_ANIMATION_DURATION.as_secs_f32();
        if elapsed >= total {
            return None;
        }
        let raw = (elapsed / total).clamp(0.0, 1.0);
        let progress = 1.0 - (1.0 - raw).powi(3); // ease_out_cubic
        let index = self.tabs.iter().position(|t| t.id == tab_id)?;
        Some((index, progress))
    }

    fn pane_focus_config(&self) -> Option<(PaneFocusPreset, f32)> {
        let preset = pane_focus_preset(self.pane_focus_effect)?;
        let strength = pane_focus_strength_factor(self.pane_focus_strength);
        (strength > f32::EPSILON).then_some((preset, strength))
    }

    fn effective_terminal_padding(&self) -> (f32, f32) {
        if Self::uses_outer_terminal_padding(
            self.tabs
                .get(self.active_tab)
                .map_or(0, |tab| tab.panes.len()),
        ) {
            (self.padding_x, self.padding_y)
        } else {
            // Multi-pane layouts use per-pane content padding (native) or pane-managed
            // geometry (tmux), so disable global outer padding in that mode.
            (0.0, 0.0)
        }
    }

    fn native_split_content_padding(&self) -> (f32, f32) {
        if Self::uses_native_split_content_padding(
            self.runtime_uses_tmux(),
            self.tabs
                .get(self.active_tab)
                .map_or(0, |tab| tab.panes.len()),
        ) {
            (self.padding_x, self.padding_y)
        } else {
            (0.0, 0.0)
        }
    }

    fn uses_outer_terminal_padding(pane_count: usize) -> bool {
        pane_count <= 1
    }

    fn uses_native_split_content_padding(runtime_uses_tmux: bool, pane_count: usize) -> bool {
        !runtime_uses_tmux && pane_count > 1
    }

    fn overlay_style(&self) -> OverlayStyleBuilder<'_> {
        OverlayStyleBuilder::new(
            &self.colors,
            self.effective_background_opacity(),
            self.chrome_contrast_profile(),
        )
    }

    fn ensure_overlay_view(&mut self, cx: &mut Context<Self>) -> Entity<TerminalOverlayView> {
        if let Some(overlay_view) = self.overlay_view.clone() {
            return overlay_view;
        }

        let parent = cx.entity().downgrade();
        let overlay_view = cx.new(|_| TerminalOverlayView::new(parent));
        self.overlay_view = Some(overlay_view.clone());
        overlay_view
    }

    fn notify_overlay(&mut self, cx: &mut Context<Self>) {
        let overlay_view = self.ensure_overlay_view(cx);
        overlay_view.update(cx, |_overlay_view, cx| {
            cx.notify();
        });
    }

    fn record_benchmark_view_wakeup(&mut self) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut() {
            benchmark_session.record_view_wakeup();
        }
    }

    fn record_benchmark_terminal_event_drain_pass(&mut self) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut() {
            benchmark_session.record_terminal_event_drain_pass();
        }
    }

    fn record_benchmark_terminal_redraw(&mut self) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut() {
            benchmark_session.record_terminal_redraw();
        }
    }

    fn record_benchmark_frame(&mut self, now: Instant) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut() {
            benchmark_session.record_frame(now);
        }
    }

    fn sample_benchmark_session(&mut self) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut() {
            benchmark_session.sample_if_due(Instant::now());
        }
    }

    fn finish_benchmark_session(&mut self) {
        if let Some(benchmark_session) = self.benchmark_session.as_mut()
            && let Err(error) = benchmark_session.finish()
        {
            log::error!("Failed to write benchmark metrics: {error}");
        }
    }

    fn benchmark_exit_on_complete(&self) -> bool {
        self.benchmark_session
            .as_ref()
            .is_some_and(BenchmarkSession::exit_on_complete)
    }

    fn schedule_benchmark_exit(&mut self, cx: &mut Context<Self>) {
        if self.benchmark_exit_scheduled {
            return;
        }

        self.benchmark_exit_scheduled = true;
        self.allow_quit_without_prompt = true;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(BENCHMARK_EXIT_GRACE_DURATION).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    if view
                        .benchmark_session
                        .as_ref()
                        .is_none_or(BenchmarkSession::is_finished)
                    {
                        return;
                    }
                    view.finish_benchmark_session();
                    cx.quit();
                })
            });
        })
        .detach();
    }

    fn record_debug_overlay_frame(&mut self) {
        if !self.show_debug_overlay && !self.inspector_collects_render_stats() {
            return;
        }
        self.debug_overlay_stats.record_frame(Instant::now());
    }

    fn debug_overlay_memory_label(&self) -> String {
        let mib = self.debug_overlay_stats.memory_bytes as f64 / (1024.0 * 1024.0);
        format!("{mib:.1} MB")
    }

    fn track_window_resize_indicator(&mut self, viewport: Size<Pixels>, now: Instant) {
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let viewport_key = (
            viewport_width.round() as i32,
            viewport_height.round() as i32,
        );
        if self.last_viewport_size_px == Some(viewport_key) {
            return;
        }
        self.last_viewport_size_px = Some(viewport_key);

        if let Some(size) = self.active_terminal().map(|terminal| terminal.size()) {
            self.resize_indicator_dims = Some((size.cols, size.rows));
            self.resize_indicator_visible_until =
                Some(now + Duration::from_millis(WINDOW_RESIZE_INDICATOR_MS));
        }
    }

    fn resize_throttle_duration() -> Duration {
        Duration::from_millis(RESIZE_THROTTLE_MS)
    }

    fn can_apply_resize_at(now: Instant, last_applied_at: Option<Instant>) -> bool {
        last_applied_at
            .is_none_or(|last| now.duration_since(last) >= Self::resize_throttle_duration())
    }

    fn resize_throttle_follow_up_delay(now: Instant, last_applied_at: Option<Instant>) -> Duration {
        let throttle = Self::resize_throttle_duration();
        let Some(last) = last_applied_at else {
            return Duration::from_millis(1);
        };
        throttle
            .saturating_sub(now.saturating_duration_since(last))
            .saturating_add(Duration::from_millis(1))
    }

    /// Schedules a follow-up task to apply pending resize after the throttle window.
    /// This ensures the final resize is applied even if no new frames are rendered.
    fn schedule_resize_throttle_follow_up(&mut self, delay: Duration, cx: &mut Context<Self>) {
        // Only schedule if not already scheduled
        if self.resize_throttle_task.is_some() {
            return;
        }

        self.resize_throttle_task = Some(cx.spawn(async move |this, cx| {
            smol::Timer::after(delay).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    view.resize_throttle_task = None;
                    // Clear the timestamp to allow immediate resize on next frame
                    view.last_resize_applied_at = None;
                    // Trigger a redraw to apply any pending resize
                    cx.notify();
                })
            });
        }));
    }

    pub(super) fn overlay_banner_visible_for_state(state: Option<&UpdateState>) -> bool {
        matches!(
            state,
            Some(
                UpdateState::Available { .. }
                    | UpdateState::Downloading { .. }
                    | UpdateState::Downloaded { .. }
                    | UpdateState::Installing { .. }
                    | UpdateState::InstallerLaunched { .. }
                    | UpdateState::Installed { .. }
                    | UpdateState::Error(_)
            )
        )
    }

    fn handle_auto_update_state_change(&mut self, state: &UpdateState, cx: &mut Context<Self>) {
        self.sync_update_toasts(Some(state));
        let was_banner_visible = self.show_update_banner;
        self.show_update_banner = Self::overlay_banner_visible_for_state(Some(state));
        self.notify_overlay(cx);
        if self.show_update_banner != was_banner_visible {
            cx.notify();
        }

        if matches!(state, UpdateState::InstallerLaunched { .. }) {
            self.sync_persisted_native_workspace();
            self.allow_quit_without_prompt = true;
            cx.quit();
        }
    }

    fn observe_auto_updater(&mut self, updater: &Entity<AutoUpdater>, cx: &mut Context<Self>) {
        cx.observe(updater, |view, updater, cx| {
            let state = updater.read(cx).state.clone();
            view.handle_auto_update_state_change(&state, cx);
        })
        .detach();
    }

    fn ensure_auto_updater(&mut self, cx: &mut Context<Self>) -> Option<Entity<AutoUpdater>> {
        if !AutoUpdater::supported_on_current_platform() {
            return None;
        }

        if let Some(updater) = self.auto_updater.as_ref() {
            return Some(updater.clone());
        }

        let updater = cx.new(|_| AutoUpdater::new(crate::APP_VERSION));
        self.observe_auto_updater(&updater, cx);
        self.auto_updater = Some(updater.clone());
        Some(updater)
    }

    fn scrollbar_color(
        &self,
        overlay_style: OverlayStyleBuilder<'_>,
        base_alpha: f32,
    ) -> gpui::Rgba {
        match self.terminal_scrollbar_style {
            TerminalScrollbarStyle::Neutral => overlay_style.panel_foreground(base_alpha),
            TerminalScrollbarStyle::MutedTheme => {
                let background = overlay_style.panel_background(base_alpha);
                let accent = overlay_style.panel_cursor(base_alpha);
                blend_rgba(background, accent, TERMINAL_SCROLLBAR_MUTED_THEME_BLEND)
            }
            TerminalScrollbarStyle::Theme => overlay_style.panel_cursor(base_alpha),
        }
    }

    pub(super) fn terminal_scrollbar_mode(&self) -> ScrollbarVisibilityMode {
        match self.terminal_scrollbar_visibility {
            TerminalScrollbarVisibility::Off => ScrollbarVisibilityMode::AlwaysOff,
            TerminalScrollbarVisibility::Always => ScrollbarVisibilityMode::AlwaysOn,
            TerminalScrollbarVisibility::OnScroll => ScrollbarVisibilityMode::OnScroll,
        }
    }

    pub(super) fn terminal_scrollbar_alpha(&self, now: Instant) -> f32 {
        self.terminal_scrollbar_visibility_controller.alpha(
            self.terminal_scrollbar_mode(),
            now,
            TERMINAL_SCROLLBAR_HOLD_DURATION,
            TERMINAL_SCROLLBAR_FADE_DURATION,
        )
    }

    fn terminal_scrollbar_layout_for_track(
        &self,
        track_height: f32,
    ) -> Option<scrollbar::TerminalScrollbarLayout> {
        let terminal = self.active_terminal()?;
        let size = terminal.size();
        let viewport_rows = size.rows as usize;
        if viewport_rows == 0 {
            return None;
        }

        let line_height: f32 = size.cell_height;
        let (display_offset, history_size) = terminal.scroll_state();
        scrollbar::compute_layout(
            display_offset,
            history_size,
            viewport_rows,
            line_height,
            track_height,
            TERMINAL_SCROLLBAR_MIN_THUMB_HEIGHT,
        )
    }

    fn terminal_content_bounds(&self, window: &Window) -> Option<TerminalContentRect> {
        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        // Sidebars on either side shorten the terminal content width; the
        // origin stays at (0, 0) because these bounds are local to the
        // terminal surface, which flex layout already offsets past the
        // left workspace sidebar.
        TerminalContentRect::new(
            0.0,
            0.0,
            (viewport_width - self.effective_sidebar_width() - self.workspace_sidebar_width())
                .max(0.0),
            viewport_height - self.terminal_content_top_inset() - self.inspector_bottom_inset(),
        )
    }

    fn pane_neighbor_gaps(pane: &TerminalPane, panes: &[TerminalPane]) -> TerminalPaneNeighborGaps {
        let pane_left = u32::from(pane.left);
        let pane_top = u32::from(pane.top);
        let pane_right = pane_left + u32::from(pane.width);
        let pane_bottom = pane_top + u32::from(pane.height);
        let mut gaps = TerminalPaneNeighborGaps::default();

        for candidate in panes {
            if candidate.id == pane.id {
                continue;
            }

            let candidate_left = u32::from(candidate.left);
            let candidate_top = u32::from(candidate.top);
            let candidate_right = candidate_left + u32::from(candidate.width);
            let candidate_bottom = candidate_top + u32::from(candidate.height);

            if candidate_left >= pane_right
                && cell_ranges_overlap(pane_top, pane_bottom, candidate_top, candidate_bottom)
            {
                let gap = candidate_left.saturating_sub(pane_right);
                gaps.right_cells = Some(gaps.right_cells.map_or(gap, |current| current.min(gap)));
            }

            if candidate_top >= pane_bottom
                && cell_ranges_overlap(pane_left, pane_right, candidate_left, candidate_right)
            {
                let gap = candidate_top.saturating_sub(pane_bottom);
                gaps.bottom_cells = Some(gaps.bottom_cells.map_or(gap, |current| current.min(gap)));
            }
        }

        gaps
    }

    fn native_pane_dividers(&self, tab: &TerminalTab) -> Vec<TerminalPaneDivider> {
        if tab.panes.len() <= 1 {
            return Vec::new();
        }

        let layout_cell_size = self.layout_cell_size();
        let layout_cell_width: f32 = layout_cell_size.width.into();
        let layout_cell_height: f32 = layout_cell_size.height.into();
        if layout_cell_width <= f32::EPSILON || layout_cell_height <= f32::EPSILON {
            return Vec::new();
        }

        let (outer_padding_x, outer_padding_y) = self.effective_terminal_padding();
        let max_right = tab
            .panes
            .iter()
            .map(|pane| u32::from(pane.left).saturating_add(u32::from(pane.width)))
            .max()
            .unwrap_or(0);
        let max_bottom = tab
            .panes
            .iter()
            .map(|pane| u32::from(pane.top).saturating_add(u32::from(pane.height)))
            .max()
            .unwrap_or(0);
        let mut dividers = Vec::new();

        for pane in &tab.panes {
            let Some(frame) = TerminalContentRect::new(
                outer_padding_x + (f32::from(pane.left) * layout_cell_width),
                outer_padding_y + (f32::from(pane.top) * layout_cell_height),
                f32::from(pane.width) * layout_cell_width,
                f32::from(pane.height) * layout_cell_height,
            ) else {
                continue;
            };

            let gaps = Self::pane_neighbor_gaps(pane, &tab.panes);
            let pane_right = u32::from(pane.left).saturating_add(u32::from(pane.width));
            let pane_bottom = u32::from(pane.top).saturating_add(u32::from(pane.height));

            if pane_right < max_right
                && let Some(gap_cells) = gaps.right_cells
            {
                let gap_px = (gap_cells as f32) * layout_cell_width;
                let center_x = frame.right() + (gap_px * 0.5);
                let hit_width = gap_px.max(12.0);
                let grip_height = (frame.height * 0.24).clamp(18.0, 84.0);
                let grip_width = 4.0;
                let line_frame =
                    TerminalContentRect::new(center_x - 0.5, frame.origin_y, 1.0, frame.height);
                let hit_frame = TerminalContentRect::new(
                    center_x - (hit_width * 0.5),
                    frame.origin_y,
                    hit_width,
                    frame.height,
                );
                let grip_frame = TerminalContentRect::new(
                    center_x - (grip_width * 0.5),
                    frame.origin_y + ((frame.height - grip_height) * 0.5),
                    grip_width,
                    grip_height,
                );
                if let (Some(line_frame), Some(hit_frame), Some(grip_frame)) =
                    (line_frame, hit_frame, grip_frame)
                {
                    dividers.push(TerminalPaneDivider {
                        pane_id: pane.id.clone(),
                        handle_id: pane.cached_element_ids.resize_handle_right.clone(),
                        axis: PaneResizeAxis::Horizontal,
                        edge: PaneResizeEdge::Right,
                        line_frame,
                        hit_frame,
                        grip_frame,
                    });
                }
            }

            if pane_bottom < max_bottom
                && let Some(gap_cells) = gaps.bottom_cells
            {
                let gap_px = (gap_cells as f32) * layout_cell_height;
                let center_y = frame.bottom() + (gap_px * 0.5);
                let hit_height = gap_px.max(12.0);
                let grip_width = (frame.width * 0.24).clamp(18.0, 84.0);
                let grip_height = 4.0;
                let line_frame =
                    TerminalContentRect::new(frame.origin_x, center_y - 0.5, frame.width, 1.0);
                let hit_frame = TerminalContentRect::new(
                    frame.origin_x,
                    center_y - (hit_height * 0.5),
                    frame.width,
                    hit_height,
                );
                let grip_frame = TerminalContentRect::new(
                    frame.origin_x + ((frame.width - grip_width) * 0.5),
                    center_y - (grip_height * 0.5),
                    grip_width,
                    grip_height,
                );
                if let (Some(line_frame), Some(hit_frame), Some(grip_frame)) =
                    (line_frame, hit_frame, grip_frame)
                {
                    dividers.push(TerminalPaneDivider {
                        pane_id: pane.id.clone(),
                        handle_id: pane.cached_element_ids.resize_handle_bottom.clone(),
                        axis: PaneResizeAxis::Vertical,
                        edge: PaneResizeEdge::Bottom,
                        line_frame,
                        hit_frame,
                        grip_frame,
                    });
                }
            }
        }

        dividers
    }

    fn terminal_pane_layout(
        &self,
        tab: &TerminalTab,
        pane: &TerminalPane,
        content_bounds: TerminalContentRect,
    ) -> Option<TerminalPaneLayout> {
        let layout_cell_size = self.layout_cell_size();
        let layout_cell_width: f32 = layout_cell_size.width.into();
        let layout_cell_height: f32 = layout_cell_size.height.into();
        if layout_cell_width <= f32::EPSILON || layout_cell_height <= f32::EPSILON {
            return None;
        }

        let (outer_padding_x, outer_padding_y) = self.effective_terminal_padding();
        let (content_padding_x, content_padding_y) = self.native_split_content_padding();
        let frame = TerminalContentRect::new(
            outer_padding_x + (f32::from(pane.left) * layout_cell_width),
            outer_padding_y + (f32::from(pane.top) * layout_cell_height),
            f32::from(pane.width) * layout_cell_width,
            f32::from(pane.height) * layout_cell_height,
        )?;
        let terminal_size = pane.terminal().size();
        if terminal_size.cols == 0 || terminal_size.rows == 0 {
            return None;
        }
        let cell_width: f32 = terminal_size.cell_width;
        let cell_height: f32 = terminal_size.cell_height;
        if cell_width <= f32::EPSILON || cell_height <= f32::EPSILON {
            return None;
        }
        let content_width = f32::from(terminal_size.cols) * cell_width;
        let content_height = f32::from(terminal_size.rows) * cell_height;
        let content_frame = TerminalContentRect::new(
            frame.origin_x + content_padding_x,
            frame.origin_y + content_padding_y,
            content_width,
            content_height,
        )?;
        let gaps = Self::pane_neighbor_gaps(pane, &tab.panes);
        let pane_right = u32::from(pane.left).saturating_add(u32::from(pane.width));
        let pane_bottom = u32::from(pane.top).saturating_add(u32::from(pane.height));
        let max_right = tab
            .panes
            .iter()
            .map(|candidate| u32::from(candidate.left).saturating_add(u32::from(candidate.width)))
            .max()
            .unwrap_or(pane_right);
        let max_bottom = tab
            .panes
            .iter()
            .map(|candidate| u32::from(candidate.top).saturating_add(u32::from(candidate.height)))
            .max()
            .unwrap_or(pane_bottom);
        let multi_pane = tab.panes.len() > 1;
        let extends_right_edge = !multi_pane || pane_right == max_right;
        let extends_bottom_edge = !multi_pane || pane_bottom == max_bottom;
        let scrollbar_surface = TerminalScrollbarSurfaceGeometry::new(
            if multi_pane {
                frame.origin_x
            } else {
                content_bounds.origin_x
            },
            if multi_pane {
                frame.origin_y
            } else {
                content_bounds.origin_y
            },
            if multi_pane && !extends_right_edge {
                frame.width
            } else if multi_pane {
                (content_bounds.right() - frame.origin_x).max(0.0)
            } else {
                content_bounds.width
            },
            if multi_pane && !extends_bottom_edge {
                frame.height
            } else if multi_pane {
                (content_bounds.bottom() - frame.origin_y).max(0.0)
            } else {
                content_bounds.height
            },
        )?;

        Some(TerminalPaneLayout {
            frame,
            content_frame,
            scrollbar_surface,
            cell_width: layout_cell_width,
            cell_height: layout_cell_height,
            extends_right_edge,
            extends_bottom_edge,
            gaps,
        })
    }

    fn active_terminal_pane_layout(&self, window: &Window) -> Option<TerminalPaneLayout> {
        let content_bounds = self.terminal_content_bounds(window)?;
        let tab = self.active_tab_ref()?;
        let pane_index = tab.active_pane_index()?;
        let pane = tab.panes.get(pane_index)?;
        pane.terminal();
        self.terminal_pane_layout(tab, pane, content_bounds)
    }

    pub(super) fn terminal_viewport_geometry(&self) -> Option<TerminalViewportGeometry> {
        let tab = self.active_tab_ref()?;
        let pane_index = tab.active_pane_index()?;
        let pane = tab.panes.get(pane_index)?;
        let terminal = pane.terminal();
        let layout_cell_size = self.layout_cell_size();
        let layout_cell_width: f32 = layout_cell_size.width.into();
        let layout_cell_height: f32 = layout_cell_size.height.into();
        let size = terminal.size();
        if layout_cell_width <= f32::EPSILON
            || layout_cell_height <= f32::EPSILON
            || size.cols == 0
            || size.rows == 0
        {
            return None;
        }
        let (padding_x, padding_y) = self.effective_terminal_padding();
        let (content_padding_x, content_padding_y) = self.native_split_content_padding();
        let pane_cell_height: f32 = size.cell_height;
        Some(TerminalViewportGeometry {
            origin_x: padding_x + (f32::from(pane.left) * layout_cell_width) + content_padding_x,
            origin_y: padding_y + (f32::from(pane.top) * layout_cell_height) + content_padding_y,
            height: pane_cell_height * f32::from(size.rows),
        })
    }

    pub(super) fn clear_terminal_scrollbar_marker_cache(&mut self) {
        self.terminal_scrollbar_marker_cache.clear();
    }

    pub(super) fn clear_pane_render_caches(&self) {
        for tab in &self.tabs {
            for pane in &tab.panes {
                pane.render_cache.borrow_mut().clear();
            }
        }
    }

    pub(super) fn set_tab_bar_visibility(&mut self, visibility: TabBarVisibility) -> bool {
        if self.tab_bar_visibility == visibility {
            return false;
        }

        self.tab_bar_visibility = visibility;
        self.clear_pane_render_caches();
        self.clear_terminal_scrollbar_marker_cache();
        self.cell_size_cache.clear();
        self.mark_tab_strip_layout_dirty();
        true
    }

    pub(super) fn mark_terminal_scrollbar_activity(&mut self, cx: &mut Context<Self>) {
        if self.terminal_scrollbar_mode() != ScrollbarVisibilityMode::OnScroll {
            return;
        }

        self.terminal_scrollbar_visibility_controller
            .mark_activity(Instant::now());
        self.start_terminal_scrollbar_animation(cx);
    }

    pub(super) fn start_terminal_scrollbar_drag(
        &mut self,
        thumb_grab_offset: f32,
        cx: &mut Context<Self>,
    ) {
        self.terminal_scrollbar_track_hold = None;
        self.terminal_scrollbar_drag = Some(TerminalScrollbarDragState { thumb_grab_offset });
        self.terminal_scrollbar_visibility_controller
            .start_drag(Instant::now());
        self.start_terminal_scrollbar_animation(cx);
    }

    pub(super) fn finish_terminal_scrollbar_drag(&mut self, cx: &mut Context<Self>) -> bool {
        if self.terminal_scrollbar_drag.take().is_none() {
            return false;
        }

        self.terminal_scrollbar_visibility_controller
            .end_drag(Instant::now());
        self.start_terminal_scrollbar_animation(cx);
        true
    }

    fn start_terminal_scrollbar_track_hold(
        &mut self,
        state: TerminalScrollbarTrackHoldState,
        cx: &mut Context<Self>,
    ) {
        self.terminal_scrollbar_track_hold = Some(state);
        self.mark_terminal_scrollbar_activity(cx);
        if self.terminal_scrollbar_track_hold_active {
            return;
        }

        self.terminal_scrollbar_track_hold_active = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                smol::Timer::after(Duration::from_millis(
                    TERMINAL_SCROLLBAR_TRACK_HOLD_REPEAT_MS,
                ))
                .await;
                let mut keep_running = false;
                let result = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        keep_running = view.handle_terminal_scrollbar_track_hold_tick(cx);
                        if !keep_running {
                            view.terminal_scrollbar_track_hold_active = false;
                        }
                    })
                });

                if result.is_err() || !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn update_terminal_scrollbar_track_hold(&mut self, local_y: f32) {
        if let Some(state) = self.terminal_scrollbar_track_hold.as_mut() {
            state.local_y = local_y;
        }
    }

    pub(super) fn stop_terminal_scrollbar_track_hold(&mut self) -> bool {
        self.terminal_scrollbar_track_hold.take().is_some()
    }

    fn terminal_scrollbar_needs_animation(&self, now: Instant) -> bool {
        self.terminal_scrollbar_visibility_controller
            .needs_animation(
                self.terminal_scrollbar_mode(),
                now,
                TERMINAL_SCROLLBAR_HOLD_DURATION,
                TERMINAL_SCROLLBAR_FADE_DURATION,
            )
    }

    fn start_terminal_scrollbar_animation(&mut self, cx: &mut Context<Self>) {
        if self.terminal_scrollbar_animation_active
            || self.terminal_scrollbar_mode() != ScrollbarVisibilityMode::OnScroll
            || !self.terminal_scrollbar_needs_animation(Instant::now())
        {
            return;
        }

        self.terminal_scrollbar_animation_active = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                smol::Timer::after(Duration::from_millis(16)).await;

                let mut keep_running = false;
                let result = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        keep_running = view.terminal_scrollbar_needs_animation(Instant::now());
                        if !keep_running {
                            view.terminal_scrollbar_animation_active = false;
                        }
                        cx.notify();
                    })
                });

                if result.is_err() || !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    fn sync_window_background_appearance(&mut self, window: &mut Window) {
        let resolved = resolve_background_appearance(
            self.effective_background_opacity(),
            self.background_blur,
            self.background_support_context,
        );

        if self.last_window_background_appearance != Some(resolved.appearance) {
            window.set_background_appearance(resolved.appearance);
            self.last_window_background_appearance = Some(resolved.appearance);
        }

        if self.background_blur
            && resolved.blur_fallback == BlurFallbackReason::KnownUnsupported
            && !self.warned_blur_unsupported_once
        {
            self.warned_blur_unsupported_once = true;
            termy_toast::warning(
                "Background blur is unsupported in this session; using transparency",
            );
        }
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>, config: AppConfig) -> Self {
        let effective_font_family = crate::font_families::effective_terminal_font_family(
            &config.font_family,
            cx.text_system().as_ref(),
        );
        let focus_handle = cx.focus_handle();
        let blur_focus_handle = focus_handle.clone();
        let (event_wakeup_tx, event_wakeup_rx) = bounded(1);
        let native_terminal_wakeup_router =
            NativeTerminalWakeupRouter::new(event_wakeup_tx.clone());
        let config_change_rx = config::subscribe_config_changes();
        let background_opacity_preview_rx = config::subscribe_background_opacity_preview();
        #[cfg(test)]
        let _ = &config_change_rx;
        #[cfg(test)]
        let _ = &background_opacity_preview_rx;
        let terminal_frame_drain_scheduled = Arc::new(AtomicBool::new(false));
        let window_handle = window.window_handle();

        // Focus the terminal immediately
        focus_handle.focus(window, cx);

        // Process terminal events on the next frame so bursty PTY wakeups do not monopolize
        // the UI executor and starve actual paints.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while event_wakeup_rx.recv_async().await.is_ok() {
                while event_wakeup_rx.try_recv().is_ok() {}
                let mut should_schedule = false;
                let result = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        view.record_benchmark_view_wakeup();
                        view.debug_overlay_stats.record_view_wake_signal();
                        if view.benchmark_session.is_some() {
                            if view.process_terminal_events(cx) {
                                cx.notify();
                            }
                            return;
                        }
                        if !terminal_frame_drain_scheduled.swap(true, Ordering::AcqRel) {
                            should_schedule = true;
                        }
                    })
                });
                if result.is_err() {
                    break;
                }
                if !should_schedule {
                    continue;
                }

                let this = this.clone();
                let terminal_frame_drain_scheduled = terminal_frame_drain_scheduled.clone();
                if cx
                    .update_window(window_handle, move |_, window, _| {
                        window.on_next_frame(move |_window, cx| {
                            terminal_frame_drain_scheduled.store(false, Ordering::Release);
                            let _ = this.update(cx, |view, cx| {
                                if view.process_terminal_events(cx) {
                                    cx.notify();
                                }
                            });
                        });
                        window.refresh();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        #[cfg(not(test))]
        {
            // Reload when either an in-process update or the config file watcher reports a change.
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    if config_change_rx.recv_async().await.is_err() {
                        break;
                    }
                    while config_change_rx.try_recv().is_ok() {}
                    let result = cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            if view.reload_config_if_changed(cx) {
                                cx.notify();
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        // Toggle cursor visibility for blink in both terminal and inline inputs.
        // Skip the redraw request when the blink has no visible effect (blink disabled,
        // command palette covering the terminal cursor, or the window inactive — an
        // unfocused cursor is drawn solid, so blinking would only burn redraws).
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                smol::Timer::after(Duration::from_millis(CURSOR_BLINK_INTERVAL_MS)).await;
                let result = cx
                    .update_window(window_handle, |_, window, cx| {
                        let window_active = window.is_window_active();
                        this.update(cx, |view, cx| {
                            if !window_active {
                                view.cursor_blink_visible = true;
                                return;
                            }
                            if view.tick_cursor_blink()
                                && !view.is_command_palette_open()
                                && view.renaming_tab.is_none()
                                && view.renaming_workspace.is_none()
                            {
                                cx.notify();
                            }
                        })
                    })
                    .and_then(|inner| inner);
                if result.is_err() {
                    break;
                }
            }
        })
        .detach();

        let mut last_config_error_message = None;
        let config_path = match config::ensure_config_file() {
            Ok(path) => Some(path),
            Err(error) => {
                config::report_config_error_once(
                    &mut last_config_error_message,
                    "Failed to resolve config path for terminal view",
                    &error,
                );
                None
            }
        };
        let config_fingerprint = config_path.as_deref().and_then(config::config_fingerprint);
        let saved_ssh_hosts = match crate::ssh::load_hosts(config_path.as_deref()) {
            Ok(hosts) => hosts,
            Err(error) => {
                log::warn!("Failed to load saved SSH hosts: {error}");
                termy_toast::error(error);
                Vec::new()
            }
        };
        let system_appearance = system_appearance_from_window(window.appearance());
        let theme_id = resolve_active_theme(&config, system_appearance).to_string();
        let colors = TerminalColors::from_config(&config, system_appearance);
        let base_font_size = config.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        let padding_x = config.padding_x.max(0.0);
        let padding_y = config.padding_y.max(0.0);
        let background_support_context = BackgroundSupportContext::current();
        let configured_working_dir = config.working_dir.clone();
        let benchmark_config = match BenchmarkConfig::from_env() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("Termy startup blocked: {error}");
                std::process::exit(1);
            }
        };
        let configured_runtime_kind = RuntimeKind::from_app_config(&config);
        if benchmark_config.is_some() && configured_runtime_kind == RuntimeKind::Tmux {
            eprintln!("Termy startup blocked: benchmark mode requires native runtime");
            std::process::exit(1);
        }
        let tab_title = config.tab_title.clone();
        let tab_shell_integration = TabTitleShellIntegration {
            enabled: tab_title.shell_integration,
            explicit_prefix: tab_title.explicit_prefix.clone(),
        };
        let terminal_runtime = Self::runtime_config_from_app_config(&config, &colors);
        let predicted_prompt_cwd = Self::predicted_prompt_cwd(
            configured_working_dir.as_deref(),
            terminal_runtime.working_dir_fallback,
        );
        let startup_predicted_title =
            Self::predicted_prompt_seed_title(&tab_title, predicted_prompt_cwd.as_deref());
        let initial_cols = TerminalSize::default().cols;
        let initial_rows = TerminalSize::default().rows;
        let startup_native_session = if configured_runtime_kind == RuntimeKind::Native {
            match Self::load_startup_native_session(&config) {
                Ok(session) => session,
                Err(error) => {
                    log::error!("Failed to preload native tab workspace: {error}");
                    termy_toast::error("Failed to load saved native tabs");
                    None
                }
            }
        } else {
            None
        };
        let defer_native_terminal = startup_native_session
            .as_ref()
            .is_some_and(|startup| startup.session.is_some());
        let (runtime, initial_snapshot, mut native_terminal) =
            Self::runtime_startup_from_app_config(
                &config,
                &event_wakeup_tx,
                &native_terminal_wakeup_router,
                configured_working_dir.as_deref(),
                &tab_shell_integration,
                &terminal_runtime,
                benchmark_config
                    .as_ref()
                    .map(|config| config.command.as_str()),
                initial_cols,
                initial_rows,
                defer_native_terminal,
            );
        let resolved_runtime_kind = runtime.kind();

        let plugin_runtime = PluginRuntime::new(config_path.as_deref());
        let mut view = Self {
            tabs: Vec::new(),
            workspaces: vec![workspaces::WorkspaceEntry::new(1)],
            active_workspace: 0,
            next_workspace_id: 2,
            workspace_sidebar_enabled: config.sidebar_enabled,
            workspace_sidebar_width: Self::clamp_workspace_sidebar_width(config.sidebar_width),
            workspace_sidebar_resize_drag: None,
            workspace_sidebar_collapsed: false,
            workspace_sidebar_peek_visible: false,
            workspace_drag: None,
            renaming_workspace: None,
            workspace_rename_input: InlineInputState::new(String::new()),
            native_pane_zoom_snapshots: HashMap::new(),
            native_pane_layout_trees: HashMap::new(),
            next_tab_id: 1,
            active_tab: 0,
            renaming_tab: None,
            rename_input: InlineInputState::new(String::new()),
            event_wakeup_tx,
            native_terminal_wakeup_router,
            native_terminal_wakeup_batch: HashSet::new(),
            focus_handle,
            theme_id,
            theme_mode: config.theme_mode,
            manual_theme: config.theme.clone(),
            light_theme: config.theme_light.clone(),
            dark_theme: config.theme_dark.clone(),
            custom_colors: config.colors.clone(),
            system_appearance,
            appearance_subscription: None,
            colors,
            inactive_tab_scrollback: config.inactive_tab_scrollback,
            tasks: config.tasks.clone(),
            warn_on_quit: config.warn_on_quit,
            warn_on_quit_with_running_process: config.warn_on_quit_with_running_process,
            tab_title,
            tab_close_visibility: config.tab_close_visibility,
            tab_width_mode: config.tab_width_mode,
            tab_bar_position: config.tab_bar_position,
            sidebar_collapsed: false,
            last_viewport_width: 1280.0,
            auto_hide_tabbar: config.auto_hide_tabbar,
            tab_bar_visibility: TabBarVisibility::FollowConfig,
            new_tab_animation_tab_id: None,
            new_tab_animation_start: None,
            new_tab_animation_scheduled: false,
            show_termy_in_titlebar: config.show_termy_in_titlebar,
            tab_shell_integration,
            shell_integration_enabled: config.shell_integration_enabled,
            macos_option_as_alt: config.macos_option_as_alt,
            progress_indicator_enabled: config.progress_indicator_enabled,
            progress_indicator_animation_scheduled: false,
            configured_working_dir,
            child_working_dir_cache: HashMap::new(),
            child_working_dir_lookup_pending: HashSet::new(),
            terminal_runtime,
            runtime,
            tmux_enabled_config: config.tmux_enabled,
            native_tab_persistence: config.native_tab_persistence,
            native_layout_autosave: config.native_layout_autosave,
            native_buffer_persistence: config.native_buffer_persistence,
            current_named_layout: None,
            native_persist_revision: Arc::new(AtomicU64::new(0)),
            native_persist_write_gate: Arc::new(Mutex::new(())),
            workspace_store: std::cell::OnceCell::new(),
            tmux_show_active_pane_border: config.tmux_show_active_pane_border,
            tmux_exclusive: config.tmux_exclusive,
            config_path,
            config_fingerprint,
            last_config_error_message,
            cached_tmux_binary: {
                let binary = config.tmux_binary.trim().to_string();
                (!binary.is_empty()).then_some(binary)
            },
            cached_tmux_command_prefix: config.tmux_command_prefix_argv(),
            font_family: effective_font_family,
            ui_font_family: config.ui_font_family.into(),
            base_font_size,
            font_size: px(base_font_size),
            cursor_style: config.cursor_style,
            cursor_blink: config.cursor_blink,
            cursor_blink_visible: true,
            last_cursor_input_at: None,
            background_opacity: config.background_opacity,
            chrome_contrast: config.chrome_contrast,
            background_opacity_cells: config.background_opacity_cells,
            preview_background_opacity: config::current_background_opacity_preview(),
            background_blur: config.background_blur,
            background_support_context,
            last_window_background_appearance: None,
            warned_blur_unsupported_once: false,
            padding_x,
            padding_y,
            mouse_scroll_multiplier: config.mouse_scroll_multiplier,
            pane_focus_effect: config.pane_focus_effect,
            pane_focus_strength: config.pane_focus_strength,
            line_height: config.line_height.clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT),
            copy_on_select: config.copy_on_select,
            copy_on_select_toast: config.copy_on_select_toast,
            last_terminal_modifiers: gpui::Modifiers::default(),
            pending_key_releases: HashMap::default(),
            deferred_ime_key_releases: HashSet::default(),
            selection_anchor: None,
            selection_head: None,
            selection_dragging: false,
            selection_moved: false,
            kitty_image_selection: None,
            content_scroll_baseline: 0,
            pending_cursor_move_click: None,
            pending_cursor_move_preview: None,
            terminal_context_menu: None,
            tab_context_menu: None,
            new_tab_menu_anchor: None,
            saved_ssh_hosts,
            hovered_link: None,
            hovered_toast: None,
            copied_toast_feedback: None,
            toast_animation_scheduled: false,
            toast_manager: ToastManager::new(),
            overlay_view: None,
            plugin_ui: None,
            plugin_runtime,
            plugin_lifecycle: PluginLifecycleState::new(window_handle),
            plugin_refresh_in_flight: false,
            plugin_last_error: None,
            launch_probe_scheduled: false,
            command_palette: CommandPaletteState::new(config.command_palette_show_keybinds),
            simple_mode: config.simple_mode,
            last_viewport_size_px: None,
            resize_indicator_dims: None,
            resize_indicator_visible_until: None,
            resize_indicator_animation_scheduled: false,
            resize_throttle_task: None,
            last_resize_applied_at: None,
            last_terminal_resize_signature: None,
            deferred_inactive_resize: DeferredTerminalResize::Idle,
            benchmark_session: benchmark_config.map(BenchmarkSession::new),
            benchmark_exit_scheduled: false,
            show_debug_overlay: config.show_debug_overlay,
            inspector: inspector::InspectorState::new(config.inspector_height),
            debug_overlay_stats: DebugOverlayStats::new(),
            install_cli_available: Self::install_cli_available_from_system(),
            tab_strip: TabStripState::new(config.tab_switch_modifier_hints),
            inline_input_selecting: false,
            mouse_reporting: MouseReportingState::default(),
            terminal_scroll_accumulator_y: 0.0,
            input_scroll_suppress_until: None,
            last_tmux_resize_error_at: None,
            terminal_scrollbar_visibility: config.terminal_scrollbar_visibility,
            terminal_scrollbar_style: config.terminal_scrollbar_style,
            terminal_scrollbar_visibility_controller: ScrollbarVisibilityController::default(),
            terminal_scrollbar_animation_active: false,
            terminal_scrollbar_drag: None,
            terminal_scrollbar_track_hold: None,
            terminal_scrollbar_track_hold_active: false,
            command_palette_scrollbar_drag: None,
            command_palette_scrollbar_lane_bounds: None,
            pane_resize_drag: None,
            pane_move_drag: None,
            hovered_pane_divider: None,
            pane_resize_blocked: false,
            terminal_scrollbar_marker_cache: TerminalScrollbarMarkerCache::default(),
            cell_size_cache: HashMap::new(),
            search_open: false,
            search_input: InlineInputState::new(String::new()),
            search_state: SearchState::new(),
            search_debounce_token: 0,
            ime_marked_text: None,
            ime_selected_range: None,
            pending_clipboard: None,
            #[cfg(debug_assertions)]
            render_metrics: TerminalRenderMetricsState::from_env(),
            quit_prompt_in_flight: false,
            allow_quit_without_prompt: false,
            auto_updater: None,
            show_update_banner: false,
            last_notified_update_state: None,
            update_check_toast_id: None,
            #[cfg(target_os = "macos")]
            native_file_drop_enabled: false,
        };
        #[cfg(target_os = "windows")]
        if config.tmux_enabled {
            // Surface explicit feedback when a synced/shared config requests tmux on Windows.
            termy_toast::warning(TMUX_UNSUPPORTED_WINDOWS_TOAST);
        }
        let restored_native_workspace = if resolved_runtime_kind == RuntimeKind::Native {
            match startup_native_session {
                Some(startup) => {
                    let _ = view.workspace_store.set(Some(startup.store));
                    match startup.session {
                        Some(session) => match view.restore_stored_session(session, cx) {
                            Ok(()) => true,
                            Err(error) => {
                                log::error!("Failed to restore native tab workspace: {error}");
                                termy_toast::error("Failed to restore saved native tabs");
                                false
                            }
                        },
                        None => false,
                    }
                }
                None => false,
            }
        } else {
            false
        };
        if resolved_runtime_kind == RuntimeKind::Native
            && !restored_native_workspace
            && native_terminal.is_none()
        {
            native_terminal = Some(Self::start_native_terminal(
                &view.native_terminal_wakeup_router,
                view.configured_working_dir.as_deref(),
                &view.tab_shell_integration,
                &view.terminal_runtime,
                view.benchmark_session
                    .as_ref()
                    .map(|session| session.command()),
                initial_cols,
                initial_rows,
            ));
        }

        match initial_snapshot {
            Some(initial_snapshot) => view.apply_tmux_snapshot(initial_snapshot),
            None => {
                if !restored_native_workspace && let Some(native_terminal) = native_terminal {
                    let tab_id = view.allocate_tab_id();
                    view.tabs = vec![Self::create_native_tab(
                        tab_id,
                        native_terminal,
                        initial_cols,
                        initial_rows,
                        startup_predicted_title,
                    )];
                    view.active_tab = 0;
                    view.refresh_tab_title(0);
                    view.mark_tab_strip_layout_dirty();
                }
            }
        }

        if view.benchmark_session.is_some() {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    smol::Timer::after(BENCHMARK_SAMPLE_INTERVAL).await;
                    let result = cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            view.sample_benchmark_session();
                            let should_exit =
                                view.benchmark_session.as_ref().is_some_and(|session| {
                                    session.exit_on_complete()
                                        && session.completion_deadline_reached(Instant::now())
                                });
                            if should_exit {
                                view.schedule_benchmark_exit(cx);
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.observe_window_activation(window, |view, window, cx| {
            if window.is_window_active() {
                if view.refresh_install_cli_availability() {
                    view.refresh_command_palette_items_for_current_mode(cx);
                    cx.set_menus(crate::menus::app_menus(
                        view.install_cli_available(),
                        view.runtime_uses_tmux(),
                        view.simple_mode,
                    ));
                    cx.notify();
                }
            } else if view.release_all_forwarded_mouse_presses() {
                cx.notify();
            }
        })
        .detach();
        cx.on_blur(&blur_focus_handle, window, |view, _window, cx| {
            let released_mouse_presses = view.release_all_forwarded_mouse_presses();
            let released_keyboard_modifiers = view.release_forwarded_modifiers(cx);
            let cleared_tab_switch_hint_state = view.tab_strip.switch_hints.reset_hold_state();
            let dismissed_context_menu = view.close_terminal_context_menu(cx);
            if released_mouse_presses
                || released_keyboard_modifiers
                || cleared_tab_switch_hint_state
                || dismissed_context_menu
            {
                cx.notify();
            }
        })
        .detach();

        #[cfg(not(test))]
        {
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    let Ok(mut opacity) = background_opacity_preview_rx.recv_async().await else {
                        break;
                    };
                    while let Ok(next_opacity) = background_opacity_preview_rx.try_recv() {
                        opacity = next_opacity;
                    }
                    let result = cx.update(|cx| {
                        this.update(cx, |view, cx| {
                            if view.preview_background_opacity != opacity {
                                view.preview_background_opacity = opacity;
                                cx.notify();
                            }
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            })
            .detach();
        }

        if config.auto_update
            && let Some(updater) = view.ensure_auto_updater(cx)
        {
            let weak = updater.downgrade();
            cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
                smol::Timer::after(Duration::from_millis(5000)).await;
                cx.update(|cx| AutoUpdater::check(weak, cx));
            })
            .detach();
        }

        #[cfg(not(test))]
        view.schedule_initial_plugin_refresh(cx);
        view.sync_native_terminal_wakeup_interest();
        view.appearance_subscription =
            Some(cx.observe_window_appearance(window, |view, window, cx| {
                view.handle_window_appearance_change(window.appearance(), cx);
            }));
        view
    }

    fn handle_window_appearance_change(
        &mut self,
        appearance: gpui::WindowAppearance,
        cx: &mut Context<Self>,
    ) {
        let next = system_appearance_from_window(appearance);
        if self.system_appearance == next {
            return;
        }
        self.system_appearance = next;
        if self.theme_mode != config::AppearanceMode::Manual {
            self.reapply_theme(cx);
        }
    }

    fn reapply_theme(&mut self, cx: &mut Context<Self>) {
        let resolved = match self.theme_mode {
            config::AppearanceMode::Manual => self.manual_theme.clone(),
            config::AppearanceMode::System => match self.system_appearance {
                SystemAppearance::Light => self.light_theme.clone(),
                SystemAppearance::Dark => self.dark_theme.clone(),
            },
        };
        self.apply_resolved_theme(resolved, cx);
    }

    fn apply_resolved_theme(&mut self, resolved: String, cx: &mut Context<Self>) {
        self.theme_id = resolved;
        self.colors = match self.theme_mode {
            config::AppearanceMode::Manual => {
                TerminalColors::from_theme(&self.theme_id, &self.custom_colors)
            }
            config::AppearanceMode::System => TerminalColors::from_system_theme(
                &self.theme_id,
                &self.custom_colors,
                self.system_appearance,
            ),
        };
        let query_colors = Self::terminal_query_colors(&self.colors);
        self.terminal_runtime.query_colors = query_colors;

        // Cell colors are resolved before entering the pane cache. A repaint by
        // itself would otherwise reuse rows from the previous appearance.
        self.clear_pane_render_caches();
        for tab in &self.tabs {
            for pane in &tab.panes {
                pane.terminal().set_query_colors(query_colors);
            }
        }
        cx.notify();
    }

    pub(crate) fn reload_theme_assets(&mut self, cx: &mut Context<Self>) {
        let resolved = match self.theme_mode {
            config::AppearanceMode::Manual => self.manual_theme.clone(),
            config::AppearanceMode::System => match self.system_appearance {
                SystemAppearance::Light => self.light_theme.clone(),
                SystemAppearance::Dark => self.dark_theme.clone(),
            },
        };
        self.apply_resolved_theme(resolved, cx);
    }

    fn apply_runtime_config(&mut self, config: AppConfig, cx: &mut Context<Self>) -> bool {
        let effective_font_family = crate::font_families::effective_terminal_font_family(
            &config.font_family,
            cx.text_system().as_ref(),
        );
        crate::app_icon::apply_from_config(&config);
        keybindings::install_keybindings(cx, &config, self.runtime_uses_tmux());
        self.cached_tmux_binary = {
            let binary = config.tmux_binary.trim().to_string();
            (!binary.is_empty()).then_some(binary)
        };
        self.cached_tmux_command_prefix = config.tmux_command_prefix_argv();
        let previous_font_family = self.font_family.clone();
        let previous_font_size = self.font_size;
        self.theme_mode = config.theme_mode;
        self.manual_theme = config.theme.clone();
        self.light_theme = config.theme_light.clone();
        self.dark_theme = config.theme_dark.clone();
        self.custom_colors = config.colors.clone();
        self.theme_id = resolve_active_theme(&config, self.system_appearance).to_string();
        self.colors = TerminalColors::from_config(&config, self.system_appearance);
        self.inactive_tab_scrollback = config.inactive_tab_scrollback;
        self.tasks = config.tasks.clone();
        self.warn_on_quit = config.warn_on_quit;
        self.warn_on_quit_with_running_process = config.warn_on_quit_with_running_process;
        self.tab_title = config.tab_title.clone();
        let tab_close_visibility_changed = self.tab_close_visibility != config.tab_close_visibility;
        let tab_width_mode_changed = self.tab_width_mode != config.tab_width_mode;
        let tab_bar_position_changed = self.tab_bar_position != config.tab_bar_position;
        let tab_switch_modifier_hints_changed = self
            .tab_strip
            .switch_hints
            .sync_enabled(config.tab_switch_modifier_hints);
        let show_termy_in_titlebar_changed =
            self.show_termy_in_titlebar != config.show_termy_in_titlebar;
        let show_debug_overlay_changed = self.show_debug_overlay != config.show_debug_overlay;
        let simple_mode_changed = self.simple_mode != config.simple_mode;
        let auto_hide_tabbar_changed = self.auto_hide_tabbar != config.auto_hide_tabbar;
        let workspace_sidebar_enabled_changed =
            self.workspace_sidebar_enabled != config.sidebar_enabled;
        let workspace_sidebar_width_changed =
            self.sync_workspace_sidebar_width_from_config(config.sidebar_width);
        let workspace_sidebar_changed =
            workspace_sidebar_enabled_changed || workspace_sidebar_width_changed;
        self.workspace_sidebar_enabled = config.sidebar_enabled;
        if !self.workspace_sidebar_enabled {
            self.workspace_sidebar_collapsed = false;
            self.workspace_sidebar_peek_visible = false;
            // Stashed workspaces would be unreachable without the sidebar.
            self.collapse_workspaces_into_active(cx);
        }
        self.tab_close_visibility = config.tab_close_visibility;
        self.tab_width_mode = config.tab_width_mode;
        self.tab_bar_position = config.tab_bar_position;
        self.auto_hide_tabbar = config.auto_hide_tabbar;
        self.show_termy_in_titlebar = config.show_termy_in_titlebar;
        self.show_debug_overlay = config.show_debug_overlay;
        if self.sync_inspector_height_from_config(config.inspector_height) {
            cx.notify();
        }
        self.simple_mode = config.simple_mode;
        if self.simple_mode {
            if self.is_command_palette_open() {
                self.close_command_palette(cx);
            }
            crate::app_actions::close_settings_windows(cx);
        }
        self.tab_shell_integration = TabTitleShellIntegration {
            enabled: self.tab_title.shell_integration,
            explicit_prefix: self.tab_title.explicit_prefix.clone(),
        };
        self.shell_integration_enabled = config.shell_integration_enabled;
        self.macos_option_as_alt = config.macos_option_as_alt;
        self.progress_indicator_enabled = config.progress_indicator_enabled;
        #[cfg(target_os = "windows")]
        if !self.tmux_enabled_config && config.tmux_enabled {
            // Keep this visible on config reload so users understand why runtime did not switch.
            termy_toast::warning(TMUX_UNSUPPORTED_WINDOWS_TOAST);
        }
        #[cfg(not(target_os = "windows"))]
        let next_runtime_kind = Self::runtime_kind_from_app_config(&config);
        #[cfg(not(target_os = "windows"))]
        let tmux_enabled_changed = config.tmux_enabled != self.tmux_enabled_config;
        #[cfg(not(target_os = "windows"))]
        if next_runtime_kind != self.runtime_kind() && tmux_enabled_changed {
            termy_toast::info(
                "tmux startup default saved. Use Tmux Sessions to switch runtime now.",
            );
        }
        self.tmux_enabled_config = config.tmux_enabled;
        let native_tab_persistence_changed =
            self.native_tab_persistence != config.native_tab_persistence;
        let native_layout_autosave_changed =
            self.native_layout_autosave != config.native_layout_autosave;
        let native_buffer_persistence_changed =
            self.native_buffer_persistence != config.native_buffer_persistence;
        self.native_tab_persistence = config.native_tab_persistence;
        self.native_layout_autosave = config.native_layout_autosave;
        self.native_buffer_persistence = config.native_buffer_persistence;
        self.tmux_show_active_pane_border = config.tmux_show_active_pane_border;
        self.tmux_exclusive = config.tmux_exclusive;
        self.configured_working_dir = config.working_dir.clone();
        self.terminal_runtime = Self::runtime_config_from_app_config(&config, &self.colors);
        if workspace_sidebar_enabled_changed
            || native_tab_persistence_changed
            || native_layout_autosave_changed
            || native_buffer_persistence_changed
        {
            let should_persist_last_native_session =
                self.native_tab_persistence || self.workspace_sidebar_enabled;
            if (workspace_sidebar_enabled_changed || native_tab_persistence_changed)
                && !should_persist_last_native_session
                && let Err(error) = self.clear_persisted_native_workspace()
            {
                log::error!("Failed to clear saved native tab workspace: {error}");
            }
            if native_buffer_persistence_changed
                && !self.native_buffer_persistence
                && let Err(error) = self.rewrite_persisted_native_workspace_without_buffers()
            {
                log::error!(
                    "Failed to rewrite saved native tab workspace without buffers: {error}"
                );
            }
            if should_persist_last_native_session
                || (self.native_layout_autosave && self.current_named_layout.is_some())
            {
                self.sync_persisted_native_workspace();
            }
        }
        let reconnect_managed_tmux = self.runtime_uses_tmux()
            && matches!(
                self.tmux_runtime().config.launch,
                TmuxLaunchTarget::Managed { .. }
            );
        if reconnect_managed_tmux {
            self.reconnect_tmux_runtime(Self::tmux_runtime_from_app_config(&config));
        } else if self.runtime_uses_tmux() {
            // Session-attached runtime keeps its explicit launch target across config reloads.
            // Only update the binary path used for external tmux command invocations.
            self.tmux_runtime_mut().config.binary = config.tmux_binary.trim().to_string();
        }
        self.font_family = effective_font_family;
        self.ui_font_family = config.ui_font_family.into();
        self.base_font_size = config.font_size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        self.font_size = px(self.base_font_size);
        self.line_height = config.line_height.clamp(MIN_LINE_HEIGHT, MAX_LINE_HEIGHT);
        self.cursor_style = config.cursor_style;
        self.cursor_blink = config.cursor_blink;
        self.cursor_blink_visible = true;
        self.cell_size_cache.clear();
        if self.font_family != previous_font_family || self.font_size != previous_font_size {
            self.clear_tab_title_width_cache();
            self.mark_tab_strip_layout_dirty();
        }
        self.background_opacity = config.background_opacity;
        self.chrome_contrast = config.chrome_contrast;
        self.background_opacity_cells = config.background_opacity_cells;
        self.preview_background_opacity = config::synced_background_opacity_preview(
            self.background_opacity,
            self.preview_background_opacity,
        );
        self.background_blur = config.background_blur;
        self.padding_x = config.padding_x.max(0.0);
        self.padding_y = config.padding_y.max(0.0);
        self.copy_on_select = config.copy_on_select;
        self.copy_on_select_toast = config.copy_on_select_toast;
        self.mouse_scroll_multiplier = config.mouse_scroll_multiplier;
        self.pane_focus_effect = config.pane_focus_effect;
        self.pane_focus_strength = config.pane_focus_strength;
        if self.terminal_scrollbar_visibility != config.terminal_scrollbar_visibility {
            self.terminal_scrollbar_visibility = config.terminal_scrollbar_visibility;
            self.terminal_scrollbar_visibility_controller.reset();
            self.terminal_scrollbar_drag = None;
            self.terminal_scrollbar_track_hold = None;
            self.terminal_scrollbar_track_hold_active = false;
            self.terminal_scrollbar_animation_active = false;
            self.clear_terminal_scrollbar_marker_cache();
        }
        self.terminal_scrollbar_style = config.terminal_scrollbar_style;
        self.set_command_palette_show_keybinds(config.command_palette_show_keybinds);
        if show_debug_overlay_changed {
            self.debug_overlay_stats.reset();
            self.notify_overlay(cx);
            cx.notify();
        }
        self.clear_pane_render_caches();
        let inactive_history = self
            .inactive_tab_scrollback
            .unwrap_or(self.terminal_runtime.scrollback_history);
        let active_options = self.terminal_runtime.term_options();
        let inactive_options = (inactive_history != active_options.scrollback_history)
            .then(|| active_options.with_scrollback_history(inactive_history));
        for (tab_index, tab) in self.tabs.iter().enumerate() {
            let options = if tab_index == self.active_tab {
                active_options
            } else {
                inactive_options.unwrap_or(active_options)
            };
            for pane in &tab.panes {
                pane.terminal().set_term_options(options);
                pane.terminal()
                    .set_query_colors(self.terminal_runtime.query_colors);
            }
        }

        for index in 0..self.tabs.len() {
            self.refresh_tab_title(index);
        }
        if tab_close_visibility_changed
            || tab_width_mode_changed
            || tab_bar_position_changed
            || show_termy_in_titlebar_changed
            || simple_mode_changed
            || auto_hide_tabbar_changed
            || workspace_sidebar_changed
        {
            self.mark_tab_strip_layout_dirty();
        }
        if tab_switch_modifier_hints_changed
            || tab_bar_position_changed
            || auto_hide_tabbar_changed
            || simple_mode_changed
            || workspace_sidebar_changed
        {
            cx.notify();
        }

        if self.is_command_palette_open() {
            self.refresh_command_palette_matches(true, cx);
        }

        true
    }

    #[cfg(not(test))]
    fn reload_config_if_changed(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.config_path.clone() else {
            let loaded = config::load_runtime_config(
                &mut self.last_config_error_message,
                "Failed to reload config for terminal view",
            );
            self.config_path = loaded.path;
            self.config_fingerprint = loaded.fingerprint;
            if loaded.loaded_from_disk {
                let changed = self.apply_runtime_config(loaded.config, cx);
                if changed {
                    termy_toast::info("Configuration reloaded");
                }
                return changed;
            }
            return false;
        };

        let Some(fingerprint) = config::config_fingerprint(&path) else {
            return false;
        };

        if self.config_fingerprint == Some(fingerprint) {
            return false;
        }

        let loaded = config::load_runtime_config(
            &mut self.last_config_error_message,
            "Failed to reload config for terminal view",
        );
        self.config_path = loaded.path;
        self.config_fingerprint = loaded.fingerprint;
        if loaded.loaded_from_disk {
            let changed = self.apply_runtime_config(loaded.config, cx);
            if changed {
                termy_toast::info("Configuration reloaded");
            }
            changed
        } else {
            false
        }
    }

    pub(super) fn reload_config(&mut self, cx: &mut Context<Self>) {
        let loaded = config::load_runtime_config(
            &mut self.last_config_error_message,
            "Failed to reload config for terminal view",
        );
        self.config_path = loaded.path;
        self.config_fingerprint = loaded.fingerprint;
        if loaded.loaded_from_disk {
            self.apply_runtime_config(loaded.config, cx);
        }
    }

    pub(super) fn persist_theme_selection(
        &mut self,
        theme_id: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        if theme_id == self.theme_id {
            return Ok(false);
        }

        config::set_theme_in_config(theme_id)?;
        self.reload_config(cx);
        Ok(true)
    }

    fn tick_cursor_blink(&mut self) -> bool {
        if !self.cursor_blink {
            if self.cursor_blink_visible {
                return false;
            }
            self.cursor_blink_visible = true;
            return true;
        }

        // Hold the cursor solid for one blink interval after the last keystroke
        // so sustained typing doesn't fight the 530ms toggle. Matches xterm.
        if let Some(t) = self.last_cursor_input_at
            && t.elapsed() < Duration::from_millis(CURSOR_BLINK_INTERVAL_MS)
        {
            if !self.cursor_blink_visible {
                self.cursor_blink_visible = true;
                return true;
            }
            return false;
        }

        self.cursor_blink_visible = !self.cursor_blink_visible;
        true
    }

    pub(super) fn reset_cursor_blink_phase(&mut self) {
        self.cursor_blink_visible = true;
        self.last_cursor_input_at = Some(Instant::now());
    }

    pub(super) fn cursor_visible_for_focus(&self, focused: bool) -> bool {
        !self.cursor_blink || !focused || self.cursor_blink_visible
    }

    pub(super) fn terminal_cursor_style(&self) -> TerminalCursorStyle {
        match self.cursor_style {
            AppCursorStyle::Line => TerminalCursorStyle::Line,
            AppCursorStyle::Block => TerminalCursorStyle::Block,
        }
    }

    fn process_terminal_events(&mut self, cx: &mut Context<Self>) -> bool {
        self.debug_overlay_stats.record_terminal_event_drain_pass();

        let mut should_redraw = if self.runtime_uses_tmux() {
            self.process_tmux_terminal_events(cx)
        } else {
            let mut ready_terminal_ids = std::mem::take(&mut self.native_terminal_wakeup_batch);
            self.native_terminal_wakeup_router
                .drain_ready_into(&mut ready_terminal_ids);
            let should_redraw = self.process_native_terminal_events(cx, &mut ready_terminal_ids);
            ready_terminal_ids.clear();
            self.native_terminal_wakeup_batch = ready_terminal_ids;
            should_redraw
        };
        self.sync_plugin_lifecycle_state(self.runtime_uses_tmux(), cx);

        if self.clear_stale_kitty_image_state() {
            should_redraw = true;
        }

        if should_redraw {
            self.debug_overlay_stats.record_terminal_redraw();

            // Detect content-driven display_offset changes: Alacritty auto-increments the
            // offset to keep the viewport stable when new lines arrive while the user is
            // scrolled into history. The background PTY thread already updated the offset
            // before we got here, so we compare against content_scroll_baseline (a value
            // we maintain separately from user-initiated scrolls) to find the delta.
            let current_offset = self.active_terminal().map_or(0, |t| t.scroll_state().0);
            if current_offset != self.content_scroll_baseline {
                self.adjust_selection_for_display_offset_change(
                    self.content_scroll_baseline,
                    current_offset,
                );
                self.content_scroll_baseline = current_offset;
            }
        }

        should_redraw
    }

    fn process_native_terminal_events(
        &mut self,
        cx: &mut Context<Self>,
        ready_terminal_ids: &mut HashSet<NativeTerminalWakeupId>,
    ) -> bool {
        if ready_terminal_ids.is_empty() {
            return false;
        }
        let mut should_redraw = false;
        let mut should_quit = false;
        let active_tab = self.active_tab;
        let mut clipboard_text = ClipboardTextCache::default();
        self.record_benchmark_terminal_event_drain_pass();

        let mut pending_tab_closures: HashSet<TabId> = HashSet::new();
        let mut pending_pane_closures: Vec<(TabId, String)> = Vec::new();
        let mut pending_workspace_close = false;
        let mut plugin_events = Vec::new();
        let wakeup_router = self.native_terminal_wakeup_router.clone();
        let mut simulated_tab_count = self.tabs.len();
        // Built lazily on the first Exit event: this drain pass runs for every
        // PTY wakeup burst, and allocating the map per pass is wasted work.
        let mut simulated_pane_counts: Option<HashMap<TabId, usize>> = None;

        let tab_count = self.tabs.len();
        let tab_indices = std::iter::once(active_tab)
            .filter(|tab_index| *tab_index < tab_count)
            .chain((0..tab_count).filter(|tab_index| *tab_index != active_tab));
        'ready_tabs: for tab_index in tab_indices {
            let tab_id = self.tabs[tab_index].id;
            // Most ready batches belong to the active pane. Visit the active
            // tab first and defer string clones until a terminal yields work.
            let mut active_pane_id = None;

            for pane_index in 0..self.tabs[tab_index].panes.len() {
                if ready_terminal_ids.is_empty() {
                    break 'ready_tabs;
                }
                let terminal = self.tabs[tab_index].panes[pane_index].terminal();
                let Some(wakeup_id) = terminal.wakeup_id() else {
                    continue;
                };
                if !ready_terminal_ids.remove(&wakeup_id) {
                    continue;
                }
                let (events, has_more) = {
                    let mut reply_host = GpuiClipboardReplyHost::new(cx, &mut clipboard_text);
                    terminal.drain_events(&mut reply_host)
                };
                if events.is_empty() && !has_more {
                    continue;
                }

                let active_pane_id = active_pane_id
                    .get_or_insert_with(|| self.tabs[tab_index].active_pane_id.clone());
                let pane_id = self.tabs[tab_index].panes[pane_index].id.clone();
                let pane_is_active = pane_id.as_str() == active_pane_id.as_str();
                if has_more {
                    wakeup_router.mark_ready(wakeup_id);
                    if tab_index == active_tab {
                        should_redraw = true;
                    }
                }

                for event in events {
                    match event {
                        TerminalEvent::Wakeup | TerminalEvent::Bell => {
                            if tab_index == active_tab {
                                should_redraw = true;
                            }
                        }
                        TerminalEvent::Exit => {
                            let simulated_pane_counts =
                                simulated_pane_counts.get_or_insert_with(|| {
                                    self.tabs
                                        .iter()
                                        .map(|tab| (tab.id, tab.panes.len()))
                                        .collect()
                                });
                            let exit_should_quit = Self::schedule_native_exit(
                                tab_id,
                                pane_id.as_str(),
                                &mut simulated_tab_count,
                                simulated_pane_counts,
                                &mut pending_tab_closures,
                                &mut pending_pane_closures,
                            );
                            if exit_should_quit {
                                // Other workspaces still hold live tabs: the
                                // exit of the visible strip's last pane closes
                                // the workspace, not the app.
                                if self.has_other_workspaces() {
                                    pending_workspace_close = true;
                                } else {
                                    should_quit = true;
                                }
                            }
                            if tab_index == active_tab {
                                should_redraw = true;
                            }
                        }
                        TerminalEvent::Title(title) => {
                            if pane_is_active && self.apply_terminal_title(tab_index, &title, cx) {
                                should_redraw = true;
                            }
                        }
                        TerminalEvent::ResetTitle => {
                            if pane_is_active && self.clear_terminal_titles(tab_index) {
                                should_redraw = true;
                            }
                        }
                        TerminalEvent::ClipboardStore(text) => {
                            if tab_index == active_tab && pane_is_active {
                                self.pending_clipboard = Some(text);
                                should_redraw = true;
                            }
                        }
                        // Shell integration events (OSC 133)
                        TerminalEvent::ShellPromptStart => {
                            if self.shell_integration_enabled {
                                self.tabs[tab_index].command_lifecycle.prompt_start();
                            }
                        }
                        TerminalEvent::ShellCommandStart => {
                            if self.shell_integration_enabled {
                                self.tabs[tab_index].command_lifecycle.command_start();
                            }
                        }
                        TerminalEvent::ShellCommandExecuting => {
                            if self.shell_integration_enabled {
                                self.tabs[tab_index].command_lifecycle.command_executing();
                            }
                        }
                        TerminalEvent::ShellCommandFinished(code) => {
                            if self.shell_integration_enabled {
                                let command = self.tabs[tab_index].current_command.clone();
                                let duration_ms = self.tabs[tab_index]
                                    .command_lifecycle
                                    .elapsed()
                                    .map(|duration| {
                                        u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
                                    });
                                self.tabs[tab_index]
                                    .command_lifecycle
                                    .command_finished(code);
                                if tab_index == active_tab && pane_is_active {
                                    plugin_events.push(PluginEvent::CommandFinished {
                                        command,
                                        exit_code: code,
                                        duration_ms,
                                    });
                                }
                            }
                        }
                        // Progress indicator (OSC 9;4) — tracked per pane; the
                        // tab strip shows the per-tab aggregate.
                        TerminalEvent::Progress(state) => {
                            if self.progress_indicator_enabled
                                && self.tabs[tab_index].panes[pane_index].progress_state != state
                            {
                                self.tabs[tab_index].panes[pane_index].progress_state = state;
                                should_redraw = true;
                            }
                        }
                        // Working directory (OSC 7)
                        TerminalEvent::WorkingDirectory(path) => {
                            if pane_is_active {
                                self.tabs[tab_index].last_prompt_cwd = Some(path);
                            }
                        }
                    }
                }
            }
        }

        if !ready_terminal_ids.is_empty() {
            self.drain_stashed_workspace_terminal_events(
                cx,
                &mut clipboard_text,
                ready_terminal_ids,
            );
        }

        for (tab_id, pane_id) in pending_pane_closures.drain(..) {
            let closed = self.close_native_pane_by_id(tab_id, pane_id.as_str(), cx);
            should_redraw |= closed;
        }
        if should_quit {
            pending_tab_closures.clear();
        }
        for tab_id in pending_tab_closures.drain() {
            if let Some(tab_index) = self.tab_index_by_id(tab_id) {
                self.close_tab(tab_index, cx);
                should_redraw = true;
            }
        }
        if pending_workspace_close {
            self.close_active_workspace(cx);
            should_redraw = true;
        }

        if should_quit {
            // Shell `exit` in the last native pane should close the app immediately.
            self.sync_persisted_native_workspace();
            if self.benchmark_exit_on_complete() {
                self.schedule_benchmark_exit(cx);
                should_redraw = true;
            } else {
                self.finish_benchmark_session();
                self.allow_quit_without_prompt = true;
                cx.quit();
            }
        }

        if should_redraw {
            self.record_benchmark_terminal_redraw();
        }
        for event in plugin_events {
            self.enqueue_plugin_event(event, cx);
        }
        should_redraw
    }

    fn native_exit_should_quit_app(tab_count: usize, pane_count: usize) -> bool {
        tab_count == 1 && pane_count == 1
    }

    fn schedule_native_exit(
        tab_id: TabId,
        pane_id: &str,
        simulated_tab_count: &mut usize,
        simulated_pane_counts: &mut HashMap<TabId, usize>,
        pending_tab_closures: &mut HashSet<TabId>,
        pending_pane_closures: &mut Vec<(TabId, String)>,
    ) -> bool {
        let simulated_pane_count = simulated_pane_counts.get(&tab_id).copied().unwrap_or(0);
        if Self::native_exit_should_quit_app(*simulated_tab_count, simulated_pane_count) {
            return true;
        }
        if pending_tab_closures.contains(&tab_id) {
            return false;
        }
        if simulated_pane_count <= 1 {
            pending_tab_closures.insert(tab_id);
            *simulated_tab_count = simulated_tab_count.saturating_sub(1);
            simulated_pane_counts.insert(tab_id, 0);
            return false;
        }
        if pending_pane_closures
            .iter()
            .any(|(pending_tab, pending_pane)| {
                *pending_tab == tab_id && pending_pane.as_str() == pane_id
            })
        {
            return false;
        }
        pending_pane_closures.push((tab_id, pane_id.to_string()));
        simulated_pane_counts.insert(tab_id, simulated_pane_count.saturating_sub(1));
        false
    }

    fn clear_text_selection(&mut self) -> bool {
        let anchor_changed = self.selection_anchor.take().is_some();
        let head_changed = self.selection_head.take().is_some();
        let dragging_changed = std::mem::replace(&mut self.selection_dragging, false);
        let moved_changed = std::mem::replace(&mut self.selection_moved, false);
        anchor_changed || head_changed || dragging_changed || moved_changed
    }

    fn clear_selection(&mut self) -> bool {
        let text_changed = self.clear_text_selection();
        let image_changed = self.kitty_image_selection.take().is_some();
        text_changed || image_changed
    }

    fn current_kitty_image_placement(
        &self,
        selection: &KittyImageSelection,
    ) -> Option<KittyGraphicsRenderPlacement> {
        let active_pane_id = self.active_pane_id()?;
        let placements = self.active_terminal()?.try_kitty_graphics_placements()?;
        selection
            .current_placement(Some(active_pane_id), &placements)
            .cloned()
    }

    fn clear_kitty_image_state(&mut self) -> bool {
        let selection_changed = self.kitty_image_selection.take().is_some();
        let context_changed = self
            .terminal_context_menu
            .as_mut()
            .is_some_and(|state| state.image.take().is_some());
        selection_changed || context_changed
    }

    fn clear_stale_kitty_image_state(&mut self) -> bool {
        let has_image_state = self.kitty_image_selection.is_some()
            || self
                .terminal_context_menu
                .as_ref()
                .is_some_and(|state| state.image.is_some());
        if !has_image_state {
            return false;
        }

        let Some(active_pane_id) = self.active_pane_id().map(str::to_owned) else {
            return self.clear_kitty_image_state();
        };
        let Some(active_terminal) = self.active_terminal() else {
            return self.clear_kitty_image_state();
        };
        // A contended native terminal lock does not prove that a placement is stale. Copy
        // validation still fails closed while the lock is unavailable, but pruning waits for
        // the next event drain instead of dropping a valid selection spuriously.
        let Some(placements) = active_terminal.try_kitty_graphics_placements() else {
            return false;
        };

        let selection_is_stale = self
            .kitty_image_selection
            .as_ref()
            .is_some_and(|selection| {
                selection
                    .current_placement(Some(active_pane_id.as_str()), &placements)
                    .is_none()
            });
        let context_image_is_stale = self
            .terminal_context_menu
            .as_ref()
            .and_then(|state| state.image.as_ref())
            .is_some_and(|selection| {
                selection
                    .current_placement(Some(active_pane_id.as_str()), &placements)
                    .is_none()
            });

        if selection_is_stale {
            self.kitty_image_selection = None;
        }
        if context_image_is_stale && let Some(state) = self.terminal_context_menu.as_mut() {
            state.image = None;
        }

        selection_is_stale || context_image_is_stale
    }

    fn clear_hovered_link(&mut self) -> bool {
        if self.hovered_link.is_some() {
            self.hovered_link = None;
            true
        } else {
            false
        }
    }

    fn active_terminal(&self) -> Option<&Terminal> {
        self.tabs
            .get(self.active_tab)
            .and_then(TerminalTab::active_terminal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn tmon_engine_requires_explicit_opt_in() {
        assert!(tmon_engine_requested(Some(std::ffi::OsStr::new("1"))));
        assert!(!tmon_engine_requested(None));
        assert!(!tmon_engine_requested(Some(std::ffi::OsStr::new("0"))));
        assert!(!tmon_engine_requested(Some(std::ffi::OsStr::new("true"))));
        assert!(tmon_engine_enabled_for(
            Some(std::ffi::OsStr::new("1")),
            true
        ));
        assert!(!tmon_engine_enabled_for(
            Some(std::ffi::OsStr::new("1")),
            false
        ));
    }

    #[test]
    fn engine_label_reports_tmon_without_changing_tmux_or_native_labels() {
        let size = TerminalSize::default();
        let options = TerminalOptions::default();
        let tmux = Terminal::new_tmux(size, options);
        let native = Terminal::Native(NativeTerminalInstance {
            wakeup_id: 1,
            terminal: Mutex::new(NativeTerminal::new_display(size, None)),
        });
        let tmon = Terminal::Tmon(TmonTerminalInstance {
            wakeup_id: 2,
            terminal: tmon::Terminal::new_display(
                tmon_adapter::size(size),
                tmon::Config::default(),
            ),
        });

        assert_eq!(terminal_engine_label(Some(&tmux)), "alacritty");
        assert_eq!(terminal_engine_label(Some(&native)), "alacritty");
        assert_eq!(terminal_engine_label(Some(&tmon)), "tmon");
        assert_eq!(terminal_engine_label(None), "-");
    }

    #[test]
    fn tmon_link_adapter_matches_native_soft_wrapped_ranges() {
        let size = TerminalSize {
            cols: 10,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let native = Terminal::Native(NativeTerminalInstance {
            wakeup_id: 1,
            terminal: Mutex::new(NativeTerminal::new_display(size, None)),
        });
        let tmon = Terminal::Tmon(TmonTerminalInstance {
            wakeup_id: 2,
            terminal: tmon::Terminal::new_display(
                tmon_adapter::size(size),
                tmon::Config::default(),
            ),
        });
        let output = b"go https://example.com/path";
        native.hydrate_output(output);
        tmon.hydrate_output(output);

        for (row, col) in [(0, 4), (1, 5), (2, 4)] {
            assert_eq!(
                tmon.link_at(row, col),
                native.link_at(row, col),
                "wrapped link mismatch at {row}:{col}"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    #[test]
    fn tmon_engine_is_enabled_when_requested_on_unix() {
        assert!(tmon_engine_available());
        assert!(tmon_engine_enabled(Some(std::ffi::OsStr::new("1"))));
        assert!(!tmon_engine_enabled(None));
        assert!(!tmon_engine_enabled(Some(std::ffi::OsStr::new("0"))));
        assert!(!tmon_engine_enabled(Some(std::ffi::OsStr::new("true"))));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn tmon_engine_windows_gate_tracks_runtime_conpty_availability() {
        assert_eq!(
            tmon_engine_enabled(Some(std::ffi::OsStr::new("1"))),
            tmon_engine_available()
        );
        assert!(!tmon_engine_enabled(None));
        assert!(!tmon_engine_enabled(Some(std::ffi::OsStr::new("0"))));
        assert!(!tmon_engine_enabled(Some(std::ffi::OsStr::new("true"))));
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "windows"
    )))]
    #[test]
    fn tmon_engine_request_falls_back_when_unavailable() {
        assert!(!tmon_engine_available());
        assert!(!tmon_engine_enabled(Some(std::ffi::OsStr::new("1"))));
    }

    #[test]
    fn clipboard_text_cache_reads_the_host_lazily_once() {
        let reads = Cell::new(0);
        let mut cache = ClipboardTextCache::default();
        assert_eq!(reads.get(), 0);

        let first = cache.get_or_read(|| {
            reads.set(reads.get() + 1);
            Some("clipboard".to_string())
        });
        let second = cache.get_or_read(|| {
            reads.set(reads.get() + 1);
            Some("changed".to_string())
        });

        assert_eq!(first.as_deref(), Some("clipboard"));
        assert_eq!(second.as_deref(), Some("clipboard"));
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn tmon_clipboard_query_reaches_the_desktop_reply_host() {
        let size = TerminalSize {
            cols: 12,
            rows: 4,
            cell_width: 9.0,
            cell_height: 18.0,
        };
        let config = tmon::Config {
            osc52: tmon::Osc52::CopyPaste,
            ..tmon::Config::default()
        };
        let engine = tmon::Terminal::new_display(tmon_adapter::size(size), config);
        engine.feed_output(b"\x1b]52;c;?\x07");
        let terminal = Terminal::Tmon(TmonTerminalInstance {
            wakeup_id: 1,
            terminal: engine,
        });
        let mut requested = Vec::new();

        let (_, has_more) = terminal.drain_events(&mut |target| {
            requested.push(target);
            Some("clipboard".to_string())
        });
        let replies = match &terminal {
            Terminal::Tmon(terminal) => terminal.drain_protocol_replies(),
            Terminal::Native(_) | Terminal::Tmux(_) => unreachable!("test constructs Tmon"),
        };

        assert!(!has_more);
        assert_eq!(requested, [TerminalClipboardTarget::Clipboard]);
        assert_eq!(replies, b"\x1b]52;c;Y2xpcGJvYXJk\x07");
    }

    #[test]
    fn terminal_overlay_geometry_defaults_to_square_edges() {
        assert_eq!(TERMINAL_OVERLAY_GEOMETRY.panel_radius, 0.0);
        assert_eq!(TERMINAL_OVERLAY_GEOMETRY.input_radius, 0.0);
        assert_eq!(TERMINAL_OVERLAY_GEOMETRY.control_radius, 0.0);
    }

    #[test]
    fn toast_geometry_uses_rounded_corners() {
        assert_eq!(TOAST_GEOMETRY.panel_radius, 10.0);
        assert_eq!(TOAST_GEOMETRY.input_radius, 6.0);
        assert_eq!(TOAST_GEOMETRY.control_radius, 6.0);
    }

    #[test]
    fn native_exit_quits_only_for_single_tab_single_pane() {
        assert!(TerminalView::native_exit_should_quit_app(1, 1));
        assert!(!TerminalView::native_exit_should_quit_app(1, 2));
        assert!(!TerminalView::native_exit_should_quit_app(2, 1));
        assert!(!TerminalView::native_exit_should_quit_app(0, 0));
    }

    #[test]
    fn native_exit_schedules_tab_close_after_two_panes_exit() {
        let tab_id = 1;
        let mut pending_tab_closures = HashSet::new();
        let mut pending_pane_closures = Vec::new();
        let mut simulated_tab_count = 2;
        let mut simulated_pane_counts = HashMap::from([(tab_id, 2), (2, 1)]);

        let should_quit = TerminalView::schedule_native_exit(
            tab_id,
            "%native-1",
            &mut simulated_tab_count,
            &mut simulated_pane_counts,
            &mut pending_tab_closures,
            &mut pending_pane_closures,
        );
        assert!(!should_quit);
        assert_eq!(pending_pane_closures.len(), 1);

        let should_quit = TerminalView::schedule_native_exit(
            tab_id,
            "%native-2",
            &mut simulated_tab_count,
            &mut simulated_pane_counts,
            &mut pending_tab_closures,
            &mut pending_pane_closures,
        );
        assert!(!should_quit);
        assert!(pending_tab_closures.contains(&tab_id));
        assert_eq!(simulated_tab_count, 1);
    }

    #[test]
    fn native_exit_quits_after_two_single_pane_tabs_exit() {
        let mut pending_tab_closures = HashSet::new();
        let mut pending_pane_closures = Vec::new();
        let mut simulated_tab_count = 2;
        let mut simulated_pane_counts = HashMap::from([(1, 1), (2, 1)]);

        let should_quit = TerminalView::schedule_native_exit(
            1,
            "%native-1",
            &mut simulated_tab_count,
            &mut simulated_pane_counts,
            &mut pending_tab_closures,
            &mut pending_pane_closures,
        );
        assert!(!should_quit);
        assert!(pending_tab_closures.contains(&1));
        assert_eq!(simulated_tab_count, 1);

        let should_quit = TerminalView::schedule_native_exit(
            2,
            "%native-2",
            &mut simulated_tab_count,
            &mut simulated_pane_counts,
            &mut pending_tab_closures,
            &mut pending_pane_closures,
        );
        assert!(should_quit);
    }

    fn native_test_leaf(pane_id: &str) -> NativePaneLayoutNode {
        NativePaneLayoutNode::Leaf {
            pane_id: pane_id.to_string(),
        }
    }

    fn native_test_split(
        axis: PaneResizeAxis,
        ratio: f32,
        first: NativePaneLayoutNode,
        second: NativePaneLayoutNode,
    ) -> NativePaneLayoutNode {
        NativePaneLayoutNode::Split {
            axis,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    fn native_test_leaf_ids(node: &NativePaneLayoutNode, pane_ids: &mut Vec<String>) {
        match node {
            NativePaneLayoutNode::Leaf { pane_id } => pane_ids.push(pane_id.clone()),
            NativePaneLayoutNode::Split { first, second, .. } => {
                native_test_leaf_ids(first, pane_ids);
                native_test_leaf_ids(second, pane_ids);
            }
        }
    }

    #[test]
    fn native_split_layout_tree_preserves_unique_live_pane_ids() {
        // This is the state produced when the first split inserts pane "b"
        // before ensure_native_layout_tree_for_tab_id infers the layout tree.
        let mut root = native_test_split(
            PaneResizeAxis::Horizontal,
            0.5,
            native_test_leaf("a"),
            native_test_leaf("b"),
        );

        // native_split_active_pane then applies the split operation to that
        // already-split tree.
        let _ = TerminalView::native_replace_leaf_with_split(
            &mut root,
            "a",
            PaneResizeAxis::Horizontal,
            "b",
        );
        let _ = TerminalView::native_balance_split_group_containing_leaf(
            &mut root,
            PaneResizeAxis::Horizontal,
            "b",
        );

        let mut pane_ids = Vec::new();
        native_test_leaf_ids(&root, &mut pane_ids);
        let unique_pane_ids = pane_ids.iter().map(String::as_str).collect::<HashSet<_>>();

        assert_eq!(pane_ids.len(), 2, "one layout leaf per live pane");
        assert_eq!(unique_pane_ids.len(), 2, "layout pane ids must be unique");
        assert!(unique_pane_ids.contains("a"));
        assert!(unique_pane_ids.contains("b"));
    }

    fn native_test_rects(
        root: &NativePaneLayoutNode,
        width: u16,
        height: u16,
    ) -> HashMap<String, NativePaneRect> {
        let mut rects = HashMap::new();
        TerminalView::native_collect_leaf_rects(
            root,
            NativePaneRect {
                left: 0,
                top: 0,
                width,
                height,
            },
            &mut rects,
        );
        rects
    }

    #[test]
    fn native_balanced_split_makes_three_columns_even() {
        let mut root = native_test_split(
            PaneResizeAxis::Horizontal,
            0.5,
            native_test_leaf("a"),
            native_test_leaf("b"),
        );

        assert!(TerminalView::native_replace_leaf_with_split(
            &mut root,
            "a",
            PaneResizeAxis::Horizontal,
            "c",
        ));
        assert!(TerminalView::native_balance_split_group_containing_leaf(
            &mut root,
            PaneResizeAxis::Horizontal,
            "c",
        ));

        let rects = native_test_rects(&root, 90, 20);
        assert_eq!(
            rects["a"],
            NativePaneRect {
                left: 0,
                top: 0,
                width: 30,
                height: 20,
            }
        );
        assert_eq!(
            rects["c"],
            NativePaneRect {
                left: 30,
                top: 0,
                width: 30,
                height: 20,
            }
        );
        assert_eq!(
            rects["b"],
            NativePaneRect {
                left: 60,
                top: 0,
                width: 30,
                height: 20,
            }
        );
    }

    #[test]
    fn native_balanced_split_makes_three_rows_even() {
        let mut root = native_test_split(
            PaneResizeAxis::Vertical,
            0.5,
            native_test_leaf("a"),
            native_test_leaf("b"),
        );

        assert!(TerminalView::native_replace_leaf_with_split(
            &mut root,
            "a",
            PaneResizeAxis::Vertical,
            "c",
        ));
        assert!(TerminalView::native_balance_split_group_containing_leaf(
            &mut root,
            PaneResizeAxis::Vertical,
            "c",
        ));

        let rects = native_test_rects(&root, 80, 45);
        assert_eq!(
            rects["a"],
            NativePaneRect {
                left: 0,
                top: 0,
                width: 80,
                height: 15,
            }
        );
        assert_eq!(
            rects["c"],
            NativePaneRect {
                left: 0,
                top: 15,
                width: 80,
                height: 15,
            }
        );
        assert_eq!(
            rects["b"],
            NativePaneRect {
                left: 0,
                top: 30,
                width: 80,
                height: 15,
            }
        );
    }

    #[test]
    fn native_balanced_split_preserves_unrelated_parent_ratio() {
        let mut root = native_test_split(
            PaneResizeAxis::Horizontal,
            0.7,
            native_test_split(
                PaneResizeAxis::Vertical,
                0.5,
                native_test_leaf("a"),
                native_test_leaf("b"),
            ),
            native_test_leaf("c"),
        );

        assert!(TerminalView::native_replace_leaf_with_split(
            &mut root,
            "a",
            PaneResizeAxis::Horizontal,
            "d",
        ));
        assert!(TerminalView::native_balance_split_group_containing_leaf(
            &mut root,
            PaneResizeAxis::Horizontal,
            "d",
        ));

        let rects = native_test_rects(&root, 100, 40);
        assert_eq!(
            rects["a"],
            NativePaneRect {
                left: 0,
                top: 0,
                width: 35,
                height: 20,
            }
        );
        assert_eq!(
            rects["d"],
            NativePaneRect {
                left: 35,
                top: 0,
                width: 35,
                height: 20,
            }
        );
        assert_eq!(
            rects["b"],
            NativePaneRect {
                left: 0,
                top: 20,
                width: 70,
                height: 20,
            }
        );
        assert_eq!(
            rects["c"],
            NativePaneRect {
                left: 70,
                top: 0,
                width: 30,
                height: 40,
            }
        );
    }

    #[test]
    fn native_adjust_tree_split_applies_batched_delta() {
        let mut root = native_test_split(
            PaneResizeAxis::Horizontal,
            0.5,
            native_test_leaf("a"),
            native_test_leaf("b"),
        );

        let result = TerminalView::native_adjust_tree_split(
            &mut root,
            "a",
            PaneResizeAxis::Horizontal,
            PaneResizeEdge::Right,
            12,
            NativePaneRect {
                left: 0,
                top: 0,
                width: 100,
                height: 20,
            },
            24,
        );

        assert_eq!(result, PaneResizeResult::Applied);
        let rects = native_test_rects(&root, 100, 20);
        assert_eq!(
            rects["a"],
            NativePaneRect {
                left: 0,
                top: 0,
                width: 62,
                height: 20,
            }
        );
        assert_eq!(
            rects["b"],
            NativePaneRect {
                left: 62,
                top: 0,
                width: 38,
                height: 20,
            }
        );
    }

    #[test]
    fn native_adjust_tree_split_blocks_oversized_batch_without_mutating() {
        let mut root = native_test_split(
            PaneResizeAxis::Horizontal,
            0.5,
            native_test_leaf("a"),
            native_test_leaf("b"),
        );

        let result = TerminalView::native_adjust_tree_split(
            &mut root,
            "a",
            PaneResizeAxis::Horizontal,
            PaneResizeEdge::Right,
            40,
            NativePaneRect {
                left: 0,
                top: 0,
                width: 100,
                height: 20,
            },
            24,
        );

        assert_eq!(result, PaneResizeResult::BlockedByMinimum);
        let rects = native_test_rects(&root, 100, 20);
        assert_eq!(
            rects["a"],
            NativePaneRect {
                left: 0,
                top: 0,
                width: 50,
                height: 20,
            }
        );
        assert_eq!(
            rects["b"],
            NativePaneRect {
                left: 50,
                top: 0,
                width: 50,
                height: 20,
            }
        );
    }

    #[test]
    fn resize_throttle_allows_next_frame_after_interval() {
        let last = Instant::now();
        let throttle = TerminalView::resize_throttle_duration();

        assert!(!TerminalView::can_apply_resize_at(
            last + throttle.saturating_sub(Duration::from_millis(1)),
            Some(last),
        ));
        assert!(TerminalView::can_apply_resize_at(
            last + throttle,
            Some(last),
        ));
    }

    #[test]
    fn resize_throttle_follow_up_waits_only_for_remaining_interval() {
        let last = Instant::now();
        let throttle = TerminalView::resize_throttle_duration();
        let now = last + throttle.saturating_sub(Duration::from_millis(1));

        assert_eq!(
            TerminalView::resize_throttle_follow_up_delay(now, Some(last)),
            Duration::from_millis(2)
        );
        assert_eq!(
            TerminalView::resize_throttle_follow_up_delay(last + throttle, Some(last)),
            Duration::from_millis(1)
        );
        assert_eq!(
            TerminalView::resize_throttle_follow_up_delay(now, None),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn preferred_working_directory_prefers_active_sources_before_configured_and_fallback() {
        // Inherited candidates only count when they exist locally, so use two
        // distinct real directories.
        let prompt = std::env::temp_dir().to_string_lossy().into_owned();
        let process = std::env::current_dir()
            .expect("current dir")
            .to_string_lossy()
            .into_owned();
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            Some(prompt.as_str()),
            Some(process.as_str()),
            None,
            Some("/configured"),
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some(prompt.as_str()));

        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            None,
            Some(process.as_str()),
            None,
            Some("/configured"),
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some(process.as_str()));
    }

    #[test]
    fn preferred_working_directory_uses_explicit_value_first() {
        let cwd = TerminalView::resolve_preferred_working_directory(
            Some(" /explicit "),
            Some("/prompt"),
            Some("/process"),
            Some("/title"),
            Some("/configured"),
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some("/explicit"));
    }

    #[test]
    fn preferred_working_directory_expands_tilde_candidates() {
        let expected = TerminalView::user_home_dir()
            .expect("home dir")
            .to_string_lossy()
            .into_owned();
        let cwd = TerminalView::resolve_preferred_working_directory(
            Some("~"),
            None,
            None,
            None,
            None,
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn preferred_working_directory_uses_configured_before_fallback() {
        let configured = std::env::current_dir().expect("current dir");
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            None,
            None,
            None,
            Some(configured.to_string_lossy().as_ref()),
            RuntimeWorkingDirFallback::Home,
        );
        assert_eq!(cwd.as_deref(), Some(configured.to_string_lossy().as_ref()));
    }

    #[test]
    fn immediate_process_cwd_for_session_creation_prefers_cached_value() {
        let cwd = TerminalView::immediate_process_cwd_for_session_creation(Some("/cached"), 0);
        assert_eq!(cwd.as_deref(), Some("/cached"));
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
    #[test]
    fn immediate_process_cwd_for_session_creation_resolves_on_cache_miss() {
        let expected = std::env::current_dir()
            .expect("current dir")
            .to_string_lossy()
            .into_owned();
        let cwd =
            TerminalView::immediate_process_cwd_for_session_creation(None, std::process::id());
        assert_eq!(cwd.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn invalid_configured_working_directory_falls_back_instead_of_passing_through() {
        let fallback = std::env::current_dir()
            .expect("current dir")
            .to_string_lossy()
            .into_owned();
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            None,
            None,
            None,
            Some("/definitely/not/a/real/termy/path"),
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some(fallback.as_str()));
    }

    #[test]
    fn attach_resolution_uses_active_working_directory_before_default_launch_dir() {
        let active = std::env::current_dir()
            .expect("current dir")
            .to_string_lossy()
            .into_owned();
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            Some(active.as_str()),
            None,
            None,
            Some("/configured"),
            RuntimeWorkingDirFallback::Process,
        );
        assert_eq!(cwd.as_deref(), Some(active.as_str()));
    }

    #[test]
    fn non_local_prompt_cwd_falls_through_to_configured_working_directory() {
        // A shell inside WSL or over SSH reports a cwd that does not exist on the
        // host; new sessions must keep honoring the configured working_dir
        // instead of collapsing to the home fallback (issue #336).
        let configured = std::env::current_dir().expect("current dir");
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            Some("/home/kamr"),
            Some("/also/not/a/local/path"),
            None,
            Some(configured.to_string_lossy().as_ref()),
            RuntimeWorkingDirFallback::Home,
        );
        assert_eq!(cwd.as_deref(), Some(configured.to_string_lossy().as_ref()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unresolved_working_directory_fallback_uses_home_on_macos() {
        let expected = TerminalView::user_home_dir()
            .expect("home dir")
            .to_string_lossy()
            .into_owned();
        let cwd = TerminalView::resolve_preferred_working_directory(
            None,
            None,
            None,
            None,
            None,
            RuntimeWorkingDirFallback::Home,
        );
        assert_eq!(cwd.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn overlay_banner_visibility_tracks_updater_state_policy() {
        assert!(!TerminalView::overlay_banner_visible_for_state(None));
        assert!(!TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Idle
        )));
        assert!(!TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Checking
        )));
        assert!(!TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::UpToDate
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Available {
                version: "1.2.3".to_string(),
                asset_name: "Termy-v1.2.3-macos-arm64.dmg".to_string(),
                url: "https://example.com/installer".to_string(),
                checksum_asset_name: Some("checksums.txt".to_string()),
                checksum_url: Some("https://example.com/checksums.txt".to_string()),
                extension: "dmg".to_string(),
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Downloading {
                version: "1.2.3".to_string(),
                downloaded: 5,
                total: 10,
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Downloaded {
                version: "1.2.3".to_string(),
                installer_path: std::path::PathBuf::from("/tmp/termy-installer.dmg"),
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Installing {
                version: "1.2.3".to_string(),
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::InstallerLaunched {
                version: "1.2.3".to_string(),
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Installed {
                version: "1.2.3".to_string(),
            }
        )));
        assert!(TerminalView::overlay_banner_visible_for_state(Some(
            &UpdateState::Error("boom".to_string())
        )));
    }

    #[test]
    fn install_cli_availability_is_inverse_of_installed_probe() {
        assert!(TerminalView::install_cli_availability_from_probe(false));
        assert!(!TerminalView::install_cli_availability_from_probe(true));
    }

    #[test]
    fn refresh_install_cli_availability_reports_state_changes() {
        let (next_available, changed) =
            TerminalView::refreshed_install_cli_availability(true, true);
        assert!(!next_available);
        assert!(changed);

        let (next_available, changed) =
            TerminalView::refreshed_install_cli_availability(false, true);
        assert!(!next_available);
        assert!(!changed);
    }

    #[test]
    fn runtime_kind_follows_tmux_enabled_flag() {
        let config = AppConfig {
            tmux_enabled: false,
            ..Default::default()
        };
        assert_eq!(
            TerminalView::runtime_kind_from_app_config(&config),
            RuntimeKind::Native
        );

        let config = AppConfig {
            tmux_enabled: true,
            ..Default::default()
        };
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            TerminalView::runtime_kind_from_app_config(&config),
            RuntimeKind::Tmux
        );
        #[cfg(target_os = "windows")]
        assert_eq!(
            TerminalView::runtime_kind_from_app_config(&config),
            RuntimeKind::Native
        );
    }

    #[test]
    fn tmux_runtime_uses_event_driven_wakeup_strategy() {
        assert!(TerminalView::uses_event_driven_tmux_wakeup());
    }

    #[test]
    fn native_terminal_wakeup_router_coalesces_ready_terminal_ids() {
        let (view_wakeup_tx, view_wakeup_rx) = bounded(1);
        let router = NativeTerminalWakeupRouter::new(view_wakeup_tx);

        router.mark_ready(7);
        router.mark_ready(7);
        router.mark_ready(9);

        assert_eq!(view_wakeup_rx.try_iter().count(), 1);
        let mut ready = HashSet::new();
        router.drain_ready_into(&mut ready);
        assert_eq!(ready, HashSet::from([7, 9]));
        router.drain_ready_into(&mut ready);
        assert!(ready.is_empty());
    }

    #[test]
    fn create_native_tab_starts_with_one_full_size_pane() {
        let terminal = Terminal::new_tmux(
            TerminalSize::default(),
            TerminalOptions {
                scrollback_history: 2000,
                ..TerminalOptions::default()
            },
        );
        let tab = TerminalView::create_native_tab(7, terminal, 120, 42, None);

        assert_eq!(tab.panes.len(), 1);
        assert_eq!(tab.window_id, "@native-7");
        assert_eq!(tab.window_index, 0);
        assert_eq!(tab.active_pane_id, "%native-7");

        let pane = &tab.panes[0];
        assert_eq!(pane.id, "%native-7");
        assert_eq!(pane.left, 0);
        assert_eq!(pane.top, 0);
        assert_eq!(pane.width, 120);
        assert_eq!(pane.height, 42);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inactive_tab_render_cache_eviction_preserves_the_active_tab() {
        let make_tab = |tab_id| {
            TerminalView::create_native_tab(
                tab_id,
                Terminal::new_tmux(TerminalSize::default(), TerminalOptions::default()),
                80,
                24,
                None,
            )
        };
        let tabs = vec![make_tab(1), make_tab(2)];
        tabs[0].panes[0]
            .render_cache
            .borrow()
            .paint_cache
            .debug_seed_rows_for_tests(3);
        tabs[1].panes[0]
            .render_cache
            .borrow()
            .paint_cache
            .debug_seed_rows_for_tests(4);

        TerminalView::evict_inactive_tab_render_caches(&tabs, 0);

        assert_eq!(
            tabs[0].panes[0]
                .render_cache
                .borrow()
                .paint_cache
                .debug_row_cache_len_for_tests(),
            3
        );
        assert_eq!(
            tabs[1].panes[0]
                .render_cache
                .borrow()
                .paint_cache
                .debug_row_cache_len_for_tests(),
            0
        );
    }

    #[test]
    fn terminal_effective_background_opacity_prefers_preview() {
        assert_eq!(
            config::effective_background_opacity(
                0.9,
                Some(config::BackgroundOpacityPreview {
                    owner_id: 1,
                    opacity: 0.35,
                }),
            ),
            0.35
        );
        assert_eq!(config::effective_background_opacity(0.9, None), 0.9);
    }

    #[test]
    fn terminal_preview_clears_when_saved_matches() {
        assert_eq!(
            config::synced_background_opacity_preview(
                0.35,
                Some(config::BackgroundOpacityPreview {
                    owner_id: 1,
                    opacity: 0.35,
                }),
            ),
            None
        );
        assert_eq!(
            config::synced_background_opacity_preview(
                0.35,
                Some(config::BackgroundOpacityPreview {
                    owner_id: 1,
                    opacity: 0.5,
                }),
            ),
            Some(config::BackgroundOpacityPreview {
                owner_id: 1,
                opacity: 0.5,
            })
        );
    }

    #[test]
    fn single_pane_layout_keeps_outer_terminal_padding() {
        assert!(TerminalView::uses_outer_terminal_padding(0));
        assert!(TerminalView::uses_outer_terminal_padding(1));
        assert!(!TerminalView::uses_outer_terminal_padding(2));
    }

    #[test]
    fn native_split_content_padding_is_only_used_for_native_multi_pane_tabs() {
        assert!(!TerminalView::uses_native_split_content_padding(false, 0));
        assert!(!TerminalView::uses_native_split_content_padding(false, 1));
        assert!(TerminalView::uses_native_split_content_padding(false, 2));
        assert!(!TerminalView::uses_native_split_content_padding(true, 2));
    }

    #[test]
    fn terminal_content_rect_reports_right_and_bottom_edges() {
        let rect = TerminalContentRect::new(32.0, 48.0, 640.0, 420.0).expect("rect");

        assert_eq!(rect.right(), 672.0);
        assert_eq!(rect.bottom(), 468.0);
    }

    #[test]
    fn terminal_scrollbar_surface_geometry_requires_positive_size() {
        assert!(TerminalScrollbarSurfaceGeometry::new(0.0, 0.0, 0.0, 10.0).is_none());
        assert!(TerminalScrollbarSurfaceGeometry::new(0.0, 0.0, 10.0, 0.0).is_none());
    }

    #[test]
    fn terminal_query_colors_omits_cursor_without_explicit_override() {
        let colors = TerminalColors::default();
        let query_colors = TerminalView::terminal_query_colors(&colors);

        assert_eq!(query_colors.cursor, None);
    }
}
