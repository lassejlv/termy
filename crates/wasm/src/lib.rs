//! wasm-bindgen surface over Termy's headless engine.
//!
//! The binding is deliberately low level and copy-based: every read returns a
//! fresh typed array, so the JavaScript side never holds views into wasm memory
//! that a later allocation could detach. Rendering, input capture and transport
//! live in `@termysh/web`; this crate only exposes terminal semantics.

mod cells;
mod glyphs;
mod graphics;
mod input;
mod theme;

use termy_core::terminal_engine::{
    CursorShape, Damage, Engine, Event, MouseTracking, Options, Size,
};
use termy_core::{ProgressState, TerminalColor, TerminalQueryColors};
use wasm_bindgen::prelude::*;
use web_time::Instant;

pub use cells::{CELL_STRIDE, CellBuffer};
pub use glyphs::glyph_plan;
pub use graphics::{GraphicsSnapshot, PLACEMENT_STRIDE};
pub use theme::{theme_colors, theme_ids};

/// Mode bits returned by [`TermyEngine::mode_bits`].
pub mod mode_bits {
    pub const APPLICATION_CURSOR: u32 = 1;
    pub const APPLICATION_KEYPAD: u32 = 1 << 1;
    pub const BRACKETED_PASTE: u32 = 1 << 2;
    pub const FOCUS_EVENTS: u32 = 1 << 3;
    pub const MOUSE_TRACKING: u32 = 1 << 4;
    pub const SYNCHRONIZED_UPDATE: u32 = 1 << 5;
    pub const ALTERNATE_SCREEN: u32 = 1 << 6;
    pub const CURSOR_VISIBLE: u32 = 1 << 7;
}

/// One headless terminal: parser, grid, scrollback and protocol state.
#[wasm_bindgen]
pub struct TermyEngine {
    engine: Engine,
    replies: Vec<u8>,
    cells: CellBuffer,
    graphics: GraphicsSnapshot,
}

#[wasm_bindgen]
impl TermyEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(cols: u32, rows: u32, scrollback: u32) -> Self {
        Self {
            engine: Engine::new(
                size(cols, rows),
                Options {
                    scrollback_history: scrollback as usize,
                },
            ),
            replies: Vec::new(),
            cells: CellBuffer::default(),
            graphics: GraphicsSnapshot::default(),
        }
    }

    /// Feed child/host output into the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.engine.feed(bytes);
    }

    pub fn feed_str(&mut self, text: &str) {
        self.engine.feed(text.as_bytes());
    }

    pub fn resize(&mut self, cols: u32, rows: u32) {
        self.engine.resize(size(cols, rows));
    }

    /// Pixel cell size, used for XTWINOPS size reports and image placement.
    pub fn set_cell_pixels(&mut self, width: f32, height: f32) {
        self.engine.set_cell_pixels(width, height);
    }

    pub fn set_scrollback(&mut self, lines: u32) {
        self.engine.set_options(Options {
            scrollback_history: lines as usize,
        });
    }

    pub fn cols(&self) -> u32 {
        self.engine.size().cols as u32
    }

    pub fn rows(&self) -> u32 {
        self.engine.size().rows as u32
    }

    /// Increments whenever parsed output or a viewport change may alter a read.
    pub fn generation(&self) -> f64 {
        self.engine.generation() as f64
    }

    /// Protocol replies (DA, DSR, color queries, ...) for the host transport.
    pub fn take_replies(&mut self) -> Vec<u8> {
        self.engine.drain_replies(&mut self.replies);
        std::mem::take(&mut self.replies)
    }

    /// Drained events as plain objects: `{ type, ... }`.
    pub fn take_events(&mut self) -> js_sys::Array {
        let events = js_sys::Array::new();
        if let Some(records) = self.engine.take_program_status() {
            let json = serde_json::json!({ "type": "programStatus", "records": records });
            if let Ok(event) = js_sys::JSON::parse(&json.to_string()) {
                events.push(&event);
            }
        }
        while let Some(event) = self.engine.pop_event() {
            if let Some(object) = event_object(event) {
                events.push(&object);
            }
        }
        events
    }

    /// Call when the host transport reports process exit, then drain events.
    pub fn process_exited(&mut self) {
        self.engine.process_exited();
    }

    pub fn mode_bits(&self) -> u32 {
        let modes = self.engine.modes();
        let mut bits = 0;
        let mut set = |flag: u32, enabled: bool| {
            if enabled {
                bits |= flag;
            }
        };
        set(mode_bits::APPLICATION_CURSOR, modes.application_cursor);
        set(mode_bits::APPLICATION_KEYPAD, modes.application_keypad);
        set(mode_bits::BRACKETED_PASTE, modes.bracketed_paste);
        set(mode_bits::FOCUS_EVENTS, modes.focus_events);
        set(
            mode_bits::MOUSE_TRACKING,
            modes.mouse_tracking != MouseTracking::None,
        );
        set(mode_bits::SYNCHRONIZED_UPDATE, modes.synchronized_update);
        set(mode_bits::ALTERNATE_SCREEN, self.engine.alternate_screen());
        set(mode_bits::CURSOR_VISIBLE, self.engine.cursor().visible);
        bits
    }

    /// `[row, col, visible, shape, blinking]`; shape 0 block, 1 bar, 2 underline.
    pub fn cursor(&self) -> Vec<u32> {
        let cursor = self.engine.cursor();
        let shape = match cursor.shape {
            CursorShape::Block => 0,
            CursorShape::Beam => 1,
            CursorShape::Underline => 2,
        };
        vec![
            cursor.row as u32,
            cursor.col as u32,
            u32::from(cursor.visible),
            shape,
            u32::from(cursor.blinking),
        ]
    }

    /// Default cursor shape until an application overrides it with DECSCUSR.
    pub fn set_default_cursor_shape(&mut self, shape: u32) {
        self.engine.set_default_cursor_shape(match shape {
            1 => CursorShape::Beam,
            2 => CursorShape::Underline,
            _ => CursorShape::Block,
        });
    }

    /// `[full, scrollCount, (top, bottom, lines)*, (row, start, end)*]`.
    /// Scroll entries rotate retained rows before the spans are repainted;
    /// `lines` is an i32 stored in a u32 slot.
    pub fn take_damage(&mut self) -> Vec<u32> {
        let (damage, scrolls) = self.engine.take_render_damage();
        encode_damage(&damage, &scrolls)
    }

    /// Flat cells for viewport rows `[start, end)`; see [`CELL_STRIDE`].
    pub fn read_rows(&mut self, start: u32, end: u32) -> Vec<u32> {
        self.cells
            .read_viewport(&self.engine, start as usize, end as usize)
    }

    /// Strings referenced by the last [`Self::read_rows`] call.
    pub fn read_string(&self, index: u32) -> Option<String> {
        self.cells.string(index as usize).map(str::to_owned)
    }

    /// Flat cells for one buffer line; strings via [`Self::read_strings`].
    pub fn read_line(&mut self, line: i32) -> Vec<u32> {
        self.cells.read_line(&self.engine, line)
    }

    /// The whole string table of the last [`Self::read_rows`] call.
    pub fn read_strings(&self) -> Vec<String> {
        self.cells.strings().to_vec()
    }

    /// Text of a buffer line. Negative lines are scrollback, -1 the newest.
    pub fn line_text(&self, line: i32, trim_end: bool) -> Option<String> {
        self.engine
            .line(line)
            .map(|cells| cells::line_text(cells, trim_end))
    }

    /// Text of columns `[start, end)` of a buffer line; wide characters are
    /// included when their leading cell is in range.
    pub fn line_text_range(
        &self,
        line: i32,
        start: u32,
        end: u32,
        trim_end: bool,
    ) -> Option<String> {
        self.engine.line(line).map(|cells| {
            let end = (end as usize).min(cells.len());
            let start = (start as usize).min(end);
            cells::line_text(&cells[start..end], trim_end)
        })
    }

    pub fn line_wrapped(&self, line: i32) -> bool {
        self.engine.line_wrapped(line)
    }

    pub fn history_size(&self) -> u32 {
        self.engine.history_size() as u32
    }

    pub fn display_offset(&self) -> u32 {
        self.engine.display_offset() as u32
    }

    /// Positive deltas scroll back into history.
    pub fn scroll_display(&mut self, delta: i32) -> bool {
        self.engine.scroll_display(delta)
    }

    pub fn scroll_to_bottom(&mut self) -> bool {
        let offset = self.engine.display_offset() as i32;
        offset > 0 && self.engine.scroll_display(-offset)
    }

    pub fn clear_scrollback(&mut self) {
        self.engine.clear_scrollback();
    }

    /// Pack cold scrollback; call after output goes quiet.
    pub fn compact_history(&mut self) {
        self.engine.compact_history();
    }

    /// Milliseconds until a pending synchronized update must be committed, or -1.
    pub fn sync_deadline_ms(&self) -> f64 {
        self.engine
            .synchronized_update_deadline()
            .map_or(-1.0, |deadline| {
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64()
                    * 1000.0
            })
    }

    pub fn flush_sync(&mut self) -> bool {
        self.engine.stop_synchronized_update()
    }

    /// OSC 4 overrides: 256 raw colors where 0 means "use the theme".
    pub fn palette_overrides(&self) -> Vec<u32> {
        self.engine
            .palette()
            .iter()
            .map(|color| color.map_or(0, cells::raw_color))
            .collect()
    }

    /// OSC 10/11/12 overrides as raw colors: `[foreground, background, cursor]`.
    pub fn dynamic_color_overrides(&self) -> Vec<u32> {
        [
            self.engine.foreground(),
            self.engine.background(),
            self.engine.cursor_color(),
        ]
        .iter()
        .map(|color| color.map_or(0, cells::raw_color))
        .collect()
    }

    pub fn palette_revision(&self) -> f64 {
        self.engine.palette_revision() as f64
    }

    /// Colors answered to OSC 4/10/11/12 queries: `[fg, bg, cursor, ansi0..15]`
    /// as `0xRRGGBB`.
    pub fn set_query_colors(&mut self, colors: &[u32]) {
        if colors.len() < 19 {
            return;
        }
        let color = |value: u32| TerminalColor {
            r: (value >> 16) as u8,
            g: (value >> 8) as u8,
            b: value as u8,
        };
        let mut ansi = [TerminalColor::default(); 16];
        for (slot, value) in ansi.iter_mut().zip(&colors[3..19]) {
            *slot = color(*value);
        }
        self.engine.set_query_colors(TerminalQueryColors {
            ansi,
            foreground: color(colors[0]),
            background: color(colors[1]),
            cursor: Some(color(colors[2])),
        });
    }

    /// Active kitty keyboard protocol flags (CSI > u), 0 when legacy encoding.
    pub fn keyboard_flags(&self) -> u32 {
        u32::from(self.engine.modes().kitty_keyboard)
    }

    /// Changes when kitty graphics placements or animation frames change.
    pub fn graphics_revision(&mut self) -> f64 {
        self.engine.poll_graphics_revision() as f64
    }

    /// Visible kitty graphics placements laid out for a cell size in CSS
    /// pixels; see [`PLACEMENT_STRIDE`].
    pub fn read_graphics(&mut self, cell_width: f32, cell_height: f32) -> Vec<f64> {
        self.graphics
            .capture(&mut self.engine, (cell_width, cell_height))
    }

    /// Pixels of placement `index` from the last [`Self::read_graphics`]:
    /// RGBA, or a PNG stream when [`Self::graphics_image_is_png`] is true.
    pub fn graphics_image(&self, index: u32) -> Option<Vec<u8>> {
        self.graphics.image(index as usize).map(|(_, bytes)| bytes)
    }

    pub fn graphics_image_is_png(&self, index: u32) -> bool {
        self.graphics
            .image(index as usize)
            .is_some_and(|(png, _)| png)
    }

    /// Milliseconds until the next animation frame of a visible image, or -1.
    pub fn graphics_deadline_ms(&self) -> f64 {
        self.graphics.deadline_ms()
    }

    /// Encode a key event. `modifiers`: 1 ctrl, 2 alt, 4 shift, 8 meta.
    /// `kind`: 0 press, 1 repeat, 2 release. `key` uses Termy key names
    /// (`enter`, `up`, `f1`, `a`, ...); `text` is the produced character.
    pub fn encode_key(
        &self,
        key: &str,
        text: Option<String>,
        modifiers: u32,
        kind: u32,
        option_as_alt: bool,
    ) -> Option<Vec<u8>> {
        input::encode_key(&self.engine, key, text, modifiers, kind, option_as_alt)
    }

    /// Encode a mouse report, or nothing when the application has not enabled
    /// the matching tracking mode. `kind`: 0 press, 1 release, 2 drag, 3 move,
    /// 4-7 wheel up/down/left/right. `button`: 0 left, 1 middle, 2 right.
    pub fn encode_mouse(
        &self,
        kind: u32,
        button: u32,
        col: u32,
        row: u32,
        modifiers: u32,
    ) -> Option<Vec<u8>> {
        input::encode_mouse(&self.engine, kind, button, col, row, modifiers)
    }

    /// Normalize newlines and apply bracketed paste when the application asked for it.
    pub fn encode_paste(&self, text: &str) -> Vec<u8> {
        input::encode_paste(self.engine.modes().bracketed_paste, text)
    }

    pub fn encode_focus(&self, focused: bool) -> Option<Vec<u8>> {
        self.engine.modes().focus_events.then(|| {
            if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            }
        })
    }
}

/// Built-in theme ids, e.g. `termy`, `tokyo-night`, `dracula`.
#[wasm_bindgen(js_name = themeIds)]
pub fn js_theme_ids() -> Vec<String> {
    theme_ids()
}

/// `[fg, bg, cursor, ansi0..15]` as `0xRRGGBB`, or undefined for unknown ids.
#[wasm_bindgen(js_name = themeColors)]
pub fn js_theme_colors(id: &str) -> Option<Vec<u32>> {
    theme_colors(id)
}

/// Geometry for a special glyph, or undefined when it should be shaped as text.
/// `neighbors` are the code points two before, one before, one after and two
/// after in the row (0 when absent). See `glyphs.rs` for the layout.
#[wasm_bindgen(js_name = glyphPlan)]
pub fn js_glyph_plan(
    code_point: u32,
    neighbors: &[u32],
    cell_width: f32,
    cell_height: f32,
    font_size: f32,
) -> Option<Vec<f32>> {
    let character = char::from_u32(code_point)?;
    let neighbor = |index: usize| {
        neighbors
            .get(index)
            .copied()
            .filter(|value| *value != 0)
            .and_then(char::from_u32)
    };
    glyph_plan(
        character,
        [neighbor(0), neighbor(1), neighbor(2), neighbor(3)],
        [cell_width, cell_height, font_size],
    )
}

#[wasm_bindgen(js_name = placementStride)]
pub fn js_placement_stride() -> u32 {
    PLACEMENT_STRIDE as u32
}

#[wasm_bindgen(js_name = cellStride)]
pub fn js_cell_stride() -> u32 {
    CELL_STRIDE as u32
}

fn size(cols: u32, rows: u32) -> Size {
    Size {
        cols: cols.max(1) as usize,
        rows: rows.max(1) as usize,
    }
}

pub fn encode_damage(
    damage: &Damage,
    scrolls: &[termy_core::terminal_engine::ViewportScroll],
) -> Vec<u32> {
    let spans = match damage {
        Damage::Full => &[][..],
        Damage::Partial(spans) => spans.as_slice(),
    };
    let mut out = Vec::with_capacity(2 + scrolls.len() * 3 + spans.len() * 3);
    out.push(u32::from(matches!(damage, Damage::Full)));
    out.push(scrolls.len() as u32);
    for scroll in scrolls {
        out.extend([scroll.top as u32, scroll.bottom as u32, scroll.lines as u32]);
    }
    for span in spans {
        out.extend([span.row as u32, span.start as u32, span.end as u32]);
    }
    out
}

fn event_object(event: Event) -> Option<js_sys::Object> {
    let object = js_sys::Object::new();
    let set = |key: &str, value: JsValue| {
        let _ = js_sys::Reflect::set(&object, &JsValue::from_str(key), &value);
    };
    match event {
        Event::Bell => set("type", "bell".into()),
        Event::Title(title) => {
            set("type", "title".into());
            set("title", title.into());
        }
        Event::ResetTitle => set("type", "resetTitle".into()),
        Event::WorkingDirectory(path) => {
            set("type", "cwd".into());
            set("cwd", path.into());
        }
        Event::ShellIntegration(payload) => {
            set("type", "shellIntegration".into());
            set("payload", payload.into());
        }
        Event::Progress(progress) => {
            let (state, value) = match progress {
                ProgressState::Clear => ("clear", 0),
                ProgressState::InProgress(value) => ("progress", value),
                ProgressState::Error(value) => ("error", value),
                ProgressState::Indeterminate => ("indeterminate", 0),
                ProgressState::Warning(value) => ("warning", value),
            };
            set("type", "progress".into());
            set("state", state.into());
            set("value", value.into());
        }
        Event::Clipboard { selection, data } => {
            set("type", "clipboard".into());
            set("selection", selection.into());
            set("data", data.into());
        }
        // Kitty OSC 5522 needs a permission-aware host; not exposed yet.
        Event::KittyClipboard(_) | Event::KittyClipboardControl(_) => return None,
    }
    Some(object)
}

#[cfg(test)]
mod tests;
