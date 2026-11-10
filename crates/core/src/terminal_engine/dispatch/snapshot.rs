//! Terminal state outside the grid, for engine snapshots.

use super::super::{
    MouseEncoding, MouseTracking, Options, Size,
    grid::Grid,
    hyperlinks::Hyperlinks,
    snapshot::{Reader, Result, SnapshotError::Corrupt, Writer},
};
use super::{MAX_KEYBOARD_STACK, MAX_TITLE_STACK, State};
use crate::{TerminalColor, TerminalQueryColors};

/// Saved private modes are bounded by the modes `mode_state` recognizes.
const MAX_SAVED_MODES: usize = 64;

fn terminal_color(writer: &mut Writer, color: TerminalColor) {
    writer.u8(color.r);
    writer.u8(color.g);
    writer.u8(color.b);
}

fn read_terminal_color(reader: &mut Reader<'_>) -> Result<TerminalColor> {
    Ok(TerminalColor {
        r: reader.u8()?,
        g: reader.u8()?,
        b: reader.u8()?,
    })
}

impl State {
    pub(in crate::terminal_engine) fn write_snapshot(&self, writer: &mut Writer) {
        let modes = &self.modes;
        writer.bool(modes.application_cursor);
        writer.bool(modes.application_keypad);
        writer.bool(modes.bracketed_paste);
        writer.bool(modes.focus_events);
        writer.u8(match modes.mouse_tracking {
            MouseTracking::None => 0,
            MouseTracking::Press => 1,
            MouseTracking::Click => 2,
            MouseTracking::Drag => 3,
            MouseTracking::Motion => 4,
        });
        writer.u8(match modes.mouse_encoding {
            MouseEncoding::Default => 0,
            MouseEncoding::Utf8 => 1,
            MouseEncoding::Sgr => 2,
            MouseEncoding::SgrPixels => 3,
        });
        writer.bool(modes.alternate_scroll);
        writer.u8(modes.kitty_keyboard);
        writer.bool(modes.synchronized_update);
        writer.bool(modes.clipboard_paste_events);
        writer.bool(modes.reverse_video);
        writer.bool(self.alternate_screen);

        for color in &self.palette {
            writer.option_color(*color);
        }
        writer.option_color(self.foreground);
        writer.option_color(self.background);
        writer.option_color(self.cursor_color);
        writer.u64(self.palette_revision);
        for color in self.query_colors.ansi {
            terminal_color(writer, color);
        }
        terminal_color(writer, self.query_colors.foreground);
        terminal_color(writer, self.query_colors.background);
        writer.bool(self.query_colors.cursor.is_some());
        terminal_color(writer, self.query_colors.cursor.unwrap_or_default());
        writer.cursor_shape(self.default_cursor_shape);
        writer.bool(self.cursor_shape_overridden);
        writer.u16(self.cell_pixels.0);
        writer.u16(self.cell_pixels.1);
        writer.bool(self.pixel_mouse_reports);

        let (records, changed) = self.program_status.stored();
        writer.str(&serde_json::to_string(records).unwrap_or_else(|_| "[]".to_owned()));
        writer.bool(changed);

        writer.str(&self.title);
        writer.usize(self.title_stack.len());
        for title in &self.title_stack {
            writer.str(title);
        }
        writer.bool(self.working_directory.is_some());
        writer.str(self.working_directory.as_deref().unwrap_or_default());
        for stack in &self.keyboard_stack {
            writer.bytes(stack);
        }
        writer.u8(self.keyboard_flags[0]);
        writer.u8(self.keyboard_flags[1]);
        writer.bool(self.charsets[0]);
        writer.bool(self.charsets[1]);
        writer.usize(self.active_charset);
        for (charsets, active) in &self.saved_charsets {
            writer.bool(charsets[0]);
            writer.bool(charsets[1]);
            writer.usize(*active);
        }
        writer.usize(self.saved_private_modes.len());
        for (mode, enabled) in &self.saved_private_modes {
            writer.varint(u64::from(*mode));
            writer.bool(*enabled);
        }
        writer.bool(self.column_mode);
        writer.varint(self.last_printed.map_or(0, |c| u64::from(u32::from(c)) + 1));

        // Sorted, so the table order does not depend on hashing.
        let mut links: Vec<_> = self.hyperlinks.links().collect();
        links.sort_by(|a, b| (&a.id, &a.uri).cmp(&(&b.id, &b.uri)));
        writer.usize(links.len());
        for link in links {
            let index = writer.link(link);
            writer.varint(u64::from(index));
        }
        writer.usize(if writer.canonical() {
            0
        } else {
            self.hyperlinks.requests()
        });

        self.grid.write_snapshot(writer);
        if writer.version() >= 2 {
            self.graphics.write_snapshot(writer);
        }
    }

    pub(in crate::terminal_engine) fn read_snapshot(reader: &mut Reader<'_>) -> Result<Self> {
        let mut state = Self::new(Size { cols: 1, rows: 1 }, Options::default());
        let modes = &mut state.modes;
        modes.application_cursor = reader.bool()?;
        modes.application_keypad = reader.bool()?;
        modes.bracketed_paste = reader.bool()?;
        modes.focus_events = reader.bool()?;
        modes.mouse_tracking = match reader.u8()? {
            0 => MouseTracking::None,
            1 => MouseTracking::Press,
            2 => MouseTracking::Click,
            3 => MouseTracking::Drag,
            4 => MouseTracking::Motion,
            _ => return Err(Corrupt("mouse tracking")),
        };
        modes.mouse_encoding = match reader.u8()? {
            0 => MouseEncoding::Default,
            1 => MouseEncoding::Utf8,
            2 => MouseEncoding::Sgr,
            3 => MouseEncoding::SgrPixels,
            _ => return Err(Corrupt("mouse encoding")),
        };
        modes.alternate_scroll = reader.bool()?;
        modes.kitty_keyboard = reader.u8()? & 31;
        modes.synchronized_update = reader.bool()?;
        modes.clipboard_paste_events = reader.bool()?;
        modes.reverse_video = reader.bool()?;
        state.alternate_screen = reader.bool()?;

        for color in &mut state.palette {
            *color = reader.option_color()?;
        }
        state.foreground = reader.option_color()?;
        state.background = reader.option_color()?;
        state.cursor_color = reader.option_color()?;
        state.palette_revision = reader.u64()?;
        let mut ansi = [TerminalColor::default(); 16];
        for color in &mut ansi {
            *color = read_terminal_color(reader)?;
        }
        let foreground = read_terminal_color(reader)?;
        let background = read_terminal_color(reader)?;
        let has_cursor = reader.bool()?;
        let cursor = read_terminal_color(reader)?;
        state.query_colors = TerminalQueryColors {
            ansi,
            foreground,
            background,
            cursor: has_cursor.then_some(cursor),
        };
        state.default_cursor_shape = reader.cursor_shape()?;
        state.cursor_shape_overridden = reader.bool()?;
        state.cell_pixels = (reader.u16()?.max(1), reader.u16()?.max(1));
        state.cell_size = (
            f32::from(state.cell_pixels.0),
            f32::from(state.cell_pixels.1),
        );
        state.pixel_mouse_reports = reader.bool()?;
        if state.modes.mouse_encoding == MouseEncoding::SgrPixels && !state.pixel_mouse_reports {
            return Err(Corrupt("mouse encoding"));
        }

        let records =
            serde_json::from_slice(reader.bytes()?).map_err(|_| Corrupt("program status"))?;
        state.program_status =
            crate::program_status::ProgramStatus::restore(records, reader.bool()?);

        state.title = reader.string()?;
        let titles = reader.count(1)?;
        if titles > MAX_TITLE_STACK {
            return Err(Corrupt("title stack"));
        }
        for _ in 0..titles {
            state.title_stack.push(reader.string()?);
        }
        let has_working_directory = reader.bool()?;
        let working_directory = reader.string()?;
        state.working_directory = has_working_directory.then_some(working_directory);
        for stack in &mut state.keyboard_stack {
            let flags = reader.bytes()?;
            if flags.len() > MAX_KEYBOARD_STACK {
                return Err(Corrupt("keyboard stack"));
            }
            *stack = flags.iter().map(|flags| flags & 31).collect();
        }
        state.keyboard_flags = [reader.u8()? & 31, reader.u8()? & 31];
        state.charsets = [reader.bool()?, reader.bool()?];
        state.active_charset = reader.usize()?;
        for saved in &mut state.saved_charsets {
            *saved = ([reader.bool()?, reader.bool()?], reader.usize()?);
            if saved.1 > 1 {
                return Err(Corrupt("charset"));
            }
        }
        if state.active_charset > 1 {
            return Err(Corrupt("charset"));
        }
        let saved_modes = reader.count(2)?;
        if saved_modes > MAX_SAVED_MODES {
            return Err(Corrupt("saved modes"));
        }
        for _ in 0..saved_modes {
            let mode = reader.u16_value()?;
            state.saved_private_modes.push((mode, reader.bool()?));
        }
        state.column_mode = reader.bool()?;
        state.last_printed = match reader.varint()? {
            0 => None,
            value => Some(
                u32::try_from(value - 1)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or(Corrupt("character"))?,
            ),
        };

        let links = reader.count(1)?;
        let mut interned = Vec::with_capacity(links);
        for _ in 0..links {
            interned.push(reader.link()?);
        }
        state.hyperlinks = Hyperlinks::restore(interned, reader.usize()?);

        state.grid = Grid::read_snapshot(reader)?;
        if state.alternate_screen != state.grid.alternate_screen() {
            return Err(Corrupt("alternate screen"));
        }
        if reader.version() >= 2 {
            let (graphics, sized) = super::super::graphics::Graphics::read_snapshot(reader)?;
            state.restore_graphics(graphics, sized);
        }
        Ok(state)
    }
}
