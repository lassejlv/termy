mod backend;
mod cell_metrics;
mod config;
mod frame;
mod glyph_geometry;
mod keyboard;
mod kitty_graphics;
mod links;
mod locale;
mod monotonic_time;
mod mouse_protocol;
mod osc_intercept;
mod path_env;
mod protocol;
mod render_metrics;
mod resize_anchor;
mod runtime;
mod search;
mod shell_integration;

pub use cell_metrics::{TerminalCellMetrics, measure_cell, measure_cell_from_config};
pub use config::{
    LoadedTermyConfig, ResolvedThemeColors, TermyConfigError, load_config_from_contents,
    load_config_from_default_path, load_config_from_path, resolve_theme_colors_from_app_config,
    runtime_config_from_app_config, runtime_config_from_app_config_with_query_colors,
    runtime_config_from_app_config_with_theme, terminal_query_colors_from_resolved_theme,
};
pub use frame::{
    TerminalColor, TerminalPalette, TerminalRenderCell, TerminalRenderColor,
    TerminalRenderDamageSnapshot, TerminalRenderRead, TerminalRenderText, TerminalUnderlineStyle,
    TerminalViewportMetadata, TerminalViewportScroll, TerminalViewportScrollDirection, TermyCell,
    TermyColor, TermyFrame, TermyFrameUpdate,
};
pub use glyph_geometry::{
    MAX_TERMINAL_GLYPH_RECTS, MAX_TERMINAL_GLYPH_STROKE_POINTS, MAX_TERMINAL_GLYPH_STROKES,
    TerminalGlyphMetrics, TerminalGlyphNeighbors, TerminalGlyphPlan, TerminalGlyphPoint,
    TerminalGlyphRect, TerminalGlyphRectSnap, TerminalGlyphRenderKind, TerminalGlyphStroke,
    TerminalGlyphStrokeKind, terminal_glyph_plan,
};
pub use keyboard::{
    Keystroke, Modifiers, TerminalKeyEventKind, TerminalKeyboardMode, TermyKeystroke,
    TermyModifiers, keystroke_to_input, keystroke_to_input_with_options,
};
pub use kitty_graphics::{
    KittyGraphicsApplyResult, KittyGraphicsCommand, KittyGraphicsInterceptor, KittyGraphicsItem,
    KittyGraphicsPlaceholder, KittyGraphicsRenderPlacement, KittyGraphicsScreen,
    KittyGraphicsState, kitty_graphics_placeholders_from_alacritty_grid,
};
pub use links::{DetectedLink, DetectedViewportLink, classify_link_token, find_link_in_line};
#[cfg(unix)]
pub use locale::{
    DEFAULT_UTF8_LOCALE, Utf8LocaleOverridePlan, preferred_utf8_locale, utf8_locale_override_plan,
};
pub use monotonic_time::monotonic_now_ns;
pub use mouse_protocol::{
    TerminalMouseButton, TerminalMouseEventKind, TerminalMouseMode, TerminalMouseModifiers,
    TerminalMousePosition, encode_mouse_report,
};
pub use osc_intercept::{OscEvent, OscInterceptor};
pub use path_env::normalized_path_env;
pub use protocol::{
    KittyClipboardControl, KittyClipboardHostState, KittyClipboardInput, KittyClipboardInterceptor,
    KittyClipboardOsc, KittyClipboardOscTerminator, TerminalClipboardContent,
    TerminalClipboardLocation, TerminalClipboardReadRequest, TerminalClipboardReadResult,
    TerminalClipboardTarget, TerminalClipboardWriteRequest, TerminalClipboardWriteResult,
    TerminalQueryColors, TerminalReplyHost,
};
pub use render_metrics::{
    TerminalUiRenderMetricsSnapshot, add_span_damage_compute_us, add_span_grid_paint_us,
    add_span_row_ops_rebuild_us, add_span_text_shaping_us, increment_grid_paint_count,
    increment_shape_line_calls, increment_shaped_line_cache_hit, increment_shaped_line_cache_miss,
    terminal_ui_render_metrics_enabled, terminal_ui_render_metrics_reset,
    terminal_ui_render_metrics_snapshot,
};
pub use runtime::{
    KittyGraphicsCursorTracker, KittyGraphicsTextEffects, MAX_TERMINAL_SCROLLBACK_HISTORY,
    ResolvedTerminalLaunch, TabTitleShellIntegration, Terminal, TerminalCursorState,
    TerminalCursorStyle, TerminalDamageSnapshot, TerminalDirtySpan, TerminalEvent, TerminalLaunch,
    TerminalOptions, TerminalRuntimeConfig, TerminalSize, TerminalWakeupNotifier, WindowsShell,
    WorkingDirFallback, normalize_working_directory_candidate, resolve_launch_working_directory,
    resolve_terminal_launch, resolve_working_directory_path, terminal_environment_overrides,
};
pub use search::{
    TermySearchMatch, TermySearchOptions, TermySharedSearchMatch, search_frame,
    search_frame_shared, search_frame_shared_with_options, search_frame_with_options,
};
pub use shell_integration::{CommandLifecycle, CommandPhase, ProgressState};
pub use termy_config_core::{
    AppConfig, ConfigDiagnostic, ConfigDiagnosticKind, ConfigParseReport,
    CursorStyle as AppConfigCursorStyle, SystemAppearance, config_path,
};
pub use tmon::{GraphicsImage, graphics_display_size};
