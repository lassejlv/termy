//! Legacy frame cells: engine cells with their colors and attributes
//! resolved for hosts that draw `TermyCell`s (the C ABI's frame updates).
use super::*;
use std::mem::MaybeUninit;

/// Converts runs of engine cells to legacy frame cells. Neighbouring cells
/// almost always share a style, so its colors and attribute flags are
/// resolved once per run instead of once per cell.
pub(super) struct LegacyCells<'a> {
    palette: &'a TerminalPalette,
    fallback: TerminalQueryColors,
    style: Option<(engine::Style, Template)>,
    // The four-cell stores of that style, built on first use.
    quads: [Option<Quad>; 2],
    // The erase blank on a background, and four of it as stored; see
    // `extend_row`.
    blank: Option<(engine::Color, TermyCell, Blocks)>,
}

impl<'a> LegacyCells<'a> {
    pub(super) fn new(palette: &'a TerminalPalette, fallback: TerminalQueryColors) -> Self {
        Self {
            palette,
            fallback,
            style: None,
            quads: [None, None],
            blank: None,
        }
    }

    /// Append `line`, whose last cell ends a wrapped row when `wrapped`.
    #[cfg(test)]
    fn extend(&mut self, cells: &mut Vec<TermyCell>, line: &[engine::Cell], wrapped: bool) {
        self.extend_row(cells, line, wrapped, line.len(), engine::Color::DEFAULT);
    }

    /// [`Self::extend`] for a row read with `Engine::with_viewport_row_blanks`:
    /// the cells of `line` from `blanks` on are erase blanks on `background`,
    /// so they are stored from one template without being read.
    pub(super) fn extend_row(
        &mut self,
        cells: &mut Vec<TermyCell>,
        line: &[engine::Cell],
        wrapped: bool,
        blanks: usize,
        background: engine::Color,
    ) {
        if line.is_empty() {
            return;
        }
        cells.reserve(line.len());
        let start = cells.len();
        // Writing into the reserved tail, rather than pushing, keeps the
        // vector's length and capacity out of the per-cell loops.
        let slots = &mut cells.spare_capacity_mut()[..line.len()];
        let (text, rest) = slots.split_at_mut(blanks.min(line.len()));
        self.store_text(text, &line[..text.len()]);
        if !rest.is_empty() {
            // Every row's blanks are the same frame cell: stored four at a
            // time from one pattern, they cost stores instead of a
            // conversion each.
            let (blank, blocks) = match self.blank {
                Some((key, blank, blocks)) if key == background => (blank, blocks),
                _ => self.blank(background),
            };
            fill(rest, blank, &blocks);
        }
        // SAFETY: `reserve` made room for `line.len()` cells after `start`,
        // and the stores above initialized every one of them.
        unsafe { cells.set_len(start + line.len()) };
        if wrapped && let Some(last) = cells.last_mut() {
            last.line_wrapped = true;
        }
    }

    /// Store the frame cells of `line` into `slots`, its length.
    fn store_text(&mut self, slots: &mut [MaybeUninit<TermyCell>], line: &[engine::Cell]) {
        let mut done = 0;
        while let Some(cell) = line.get(done) {
            let style = cell.style;
            let template = match self.style {
                Some((key, template)) if key == style => template,
                _ => {
                    let template = self.template(style);
                    self.style = Some((style, template));
                    self.quads = [None, None];
                    template
                }
            };
            // The run's first cell has its style, so every pass stores one.
            done += template.store_run(
                &mut slots[done..],
                &line[done..],
                style.bits(),
                &mut self.quads,
            );
        }
    }

    /// The erase blank on `background`, and four of it as stored.
    #[inline(never)]
    fn blank(&mut self, background: engine::Color) -> (TermyCell, Blocks) {
        let blank = engine::Cell {
            style: engine::Style {
                background,
                ..engine::Style::default()
            },
            ..engine::Cell::default()
        };
        let blank = legacy_cell(&blank, false, self.palette, self.fallback);
        let mut bytes = [0; 80];
        for record in bytes.as_chunks_mut::<20>().0 {
            record.copy_from_slice(&cell_bytes(blank));
        }
        let blocks = blocks(&bytes);
        self.blank = Some((background, blank, blocks));
        (blank, blocks)
    }

    /// The frame cell fields `style` determines, for a printable character.
    #[inline(never)]
    fn template(&self, style: engine::Style) -> Template {
        let blank = engine::Cell {
            style,
            character: 'x',
            ..engine::Cell::default()
        };
        Template::new(legacy_cell(&blank, false, self.palette, self.fallback))
    }
}

// `Template` stores frame cells as these byte ranges: the character and the
// foreground, the background and four flags (`render_text` and the spacer
// flag last), then the other four flags.
const _: () = assert!(
    size_of::<TermyCell>() == 20
        && std::mem::offset_of!(TermyCell, char) == 0
        && std::mem::offset_of!(TermyCell, fg) == 4
        && std::mem::offset_of!(TermyCell, bg) == 8
        && std::mem::offset_of!(TermyCell, uses_terminal_default_bg) == 12
        && std::mem::offset_of!(TermyCell, bold) == 13
        && std::mem::offset_of!(TermyCell, render_text) == 14
        && std::mem::offset_of!(TermyCell, wide_character_spacer) == 15
        && std::mem::offset_of!(TermyCell, line_wrapped) == 16
        && std::mem::offset_of!(TermyCell, italic) == 17
        && std::mem::offset_of!(TermyCell, underline) == 18
        && std::mem::offset_of!(TermyCell, strikethrough) == 19
);

/// A style's frame cell as the words it is stored with, so each cell is
/// three register stores rather than one per field. The character,
/// `render_text` and spacer bytes are left clear for `store` to fill in.
#[derive(Clone, Copy)]
pub(super) struct Template {
    low: u64,
    middle: u64,
    high: u32,
    render_text: bool,
}

/// A frame cell's bytes.
fn cell_bytes(cell: TermyCell) -> [u8; 20] {
    // SAFETY: `TermyCell` is `repr(C)` with no padding (asserted above):
    // every byte of a valid cell is initialized.
    unsafe { std::mem::transmute::<TermyCell, [u8; 20]>(cell) }
}

/// Four frame cells as the five 16-byte blocks they are stored in.
type Blocks = [[u32; 4]; 5];

fn blocks(bytes: &[u8; 80]) -> Blocks {
    std::array::from_fn(|block| {
        std::array::from_fn(|lane| {
            let at = block * 16 + lane * 4;
            u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        })
    })
}

/// Fill `slots` with `cell`, four of which are `blocks`.
#[inline]
fn fill(slots: &mut [MaybeUninit<TermyCell>], cell: TermyCell, blocks: &Blocks) {
    let (groups, rest) = slots.as_chunks_mut::<4>();
    // SAFETY: SSE2 is part of the x86_64 baseline. Four slots are 80 bytes,
    // the five blocks, which hold four copies of `cell`'s bytes.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{__m128i, _mm_loadu_si128, _mm_storeu_si128};
        let blocks = blocks
            .each_ref()
            .map(|block| _mm_loadu_si128(block.as_ptr().cast::<__m128i>()));
        for group in groups {
            let out = group.as_mut_ptr().cast::<__m128i>();
            for (index, block) in blocks.into_iter().enumerate() {
                _mm_storeu_si128(out.add(index), block);
            }
        }
    }
    // SAFETY: NEON is part of the aarch64 baseline; as above.
    #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
    unsafe {
        use std::arch::aarch64::{vld1q_u32, vst1q_u32};
        let blocks = blocks.each_ref().map(|block| vld1q_u32(block.as_ptr()));
        for group in groups {
            let out = group.as_mut_ptr().cast::<u32>();
            for (index, block) in blocks.into_iter().enumerate() {
                vst1q_u32(out.add(index * 4), block);
            }
        }
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        all(target_arch = "aarch64", target_endian = "little")
    )))]
    {
        let _ = blocks;
        for slot in groups.as_flattened_mut() {
            slot.write(cell);
        }
    }
    for slot in rest {
        slot.write(cell);
    }
}

impl Template {
    fn new(cell: TermyCell) -> Self {
        let bytes = cell_bytes(cell);
        let word = |range: std::ops::Range<usize>| {
            let mut word = [0; 8];
            word[..range.len()].copy_from_slice(&bytes[range]);
            word
        };
        let mut low = word(0..8);
        low[..4].fill(0);
        let mut middle = word(8..16);
        middle[6..].fill(0);
        let [h0, h1, h2, h3, ..] = word(16..20);
        Self {
            low: u64::from_ne_bytes(low),
            middle: u64::from_ne_bytes(middle),
            high: u32::from_ne_bytes([h0, h1, h2, h3]),
            render_text: cell.render_text,
        }
    }

    /// Store the leading cells of `line` whose style has `key` bits into
    /// `slots`, returning how many. `quads` holds the style's four-cell
    /// stores (text, then wide glyphs with their spacers), built on first
    /// use. Out of line, so the template and key stay in registers across
    /// the loop.
    #[inline(never)]
    fn store_run(
        self,
        slots: &mut [MaybeUninit<TermyCell>],
        line: &[engine::Cell],
        key: (u64, u16),
        quads: &mut [Option<Quad>; 2],
    ) -> usize {
        const SPACERS: u16 = engine::Cell::WIDE_SPACER | engine::Cell::LEADING_WIDE_SPACER;
        // The middle word of most cells: their text renders unless hidden
        // (when the template has `render_text` clear).
        let text = self.middle | u64::from_ne_bytes([0, 0, 0, 0, 0, 0, self.render_text.into(), 0]);
        let spacer_middle = self.middle | u64::from_ne_bytes([0, 0, 0, 0, 0, 0, 0, 1]);
        let count = slots.len().min(line.len());
        let (slots, line) = (&mut slots[..count], &line[..count]);
        let mut index = 0;
        while index < count {
            // Most cells come four at a time in one of these shapes.
            if VECTOR_QUADS && count - index >= 4 {
                let kind = match line[index].flags {
                    0 => Some(0),
                    engine::Cell::WIDE => Some(1),
                    _ => None,
                };
                if let Some(kind) = kind {
                    let quad = quads[kind].get_or_insert_with(|| {
                        const WIDE: u16 = engine::Cell::WIDE;
                        const SPACER: u16 = engine::Cell::WIDE_SPACER;
                        if kind == 0 {
                            Quad::new(self, key, [text; 4], [0; 4])
                        } else {
                            let middles = [text, spacer_middle, text, spacer_middle];
                            Quad::new(self, key, middles, [WIDE, SPACER, WIDE, SPACER])
                        }
                    });
                    index += quad.store(&mut slots[index..], &line[index..]);
                    if index == count {
                        break;
                    }
                }
            }
            let cell = &line[index];
            if cell.style.bits() != key {
                return index;
            }
            // C0 and C1 controls don't render, nor do spacers.
            let code = u32::from(cell.character);
            let control = code < 0x20 || code.wrapping_sub(0x7f) <= 0x9f - 0x7f;
            if cell.flags == 0 && !control {
                self.store(&mut slots[index], cell.character, text);
                index += 1;
                continue;
            }
            let middle = if cell.flags & SPACERS != 0 {
                spacer_middle
            } else if control {
                self.middle
            } else {
                text
            };
            self.store(&mut slots[index], cell.character, middle);
            index += 1;
            // A wide glyph's spacer usually follows it, and is stored
            // without the checks only rendered text needs.
            if cell.flags & engine::Cell::WIDE != 0
                && let Some(next) = line.get(index)
                && next.flags & SPACERS != 0
                && next.style.bits() == key
            {
                self.store(&mut slots[index], next.character, spacer_middle);
                index += 1;
            }
        }
        count
    }

    /// Store a cell of `character` whose middle word, the template's with
    /// its `render_text` and spacer bytes set, is `middle`.
    #[inline(always)]
    fn store(self, slot: &mut MaybeUninit<TermyCell>, character: char, middle: u64) {
        let [c0, c1, c2, c3] = u32::from(character).to_ne_bytes();
        let low = self.low | u64::from_ne_bytes([c0, c1, c2, c3, 0, 0, 0, 0]);
        let cell = slot.as_mut_ptr().cast::<u8>();
        // SAFETY: the three writes cover the slot's 20 bytes with the bytes
        // of a valid cell: the template's, a character's and two booleans.
        // Cells are 4-aligned, so the 8-byte writes are unaligned.
        unsafe {
            cell.cast::<u64>().write_unaligned(low);
            cell.add(8).cast::<u64>().write_unaligned(middle);
            cell.add(16).cast::<u32>().write_unaligned(self.high);
        }
    }
}

/// Whether `Quad::store` stores anything on this target.
const VECTOR_QUADS: bool = cfg!(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
));

/// Four frame cells of one style, without their characters, as the five
/// 16-byte blocks they are stored in; and the four engine cells they stand
/// for, but for their characters (the style and flags of each).
#[derive(Clone, Copy)]
struct Quad {
    blocks: Blocks,
    keys: [[u32; 4]; 4],
}

impl Quad {
    /// Frame cells from `template` with the middle words `middles`, for
    /// engine cells with the style bits `key` and the flags `flags`.
    fn new(template: Template, key: (u64, u16), middles: [u64; 4], flags: [u16; 4]) -> Self {
        let mut bytes = [0; 80];
        for (record, middle) in bytes.as_chunks_mut::<20>().0.iter_mut().zip(middles) {
            record[..8].copy_from_slice(&template.low.to_ne_bytes());
            record[8..16].copy_from_slice(&middle.to_ne_bytes());
            record[16..].copy_from_slice(&template.high.to_ne_bytes());
        }
        let word = |bytes: &[u8]| u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let blocks = blocks(&bytes);
        // An engine cell's first twelve bytes: its style, then its flags.
        let head = key.0.to_ne_bytes();
        let [t0, t1] = key.1.to_ne_bytes();
        let keys = flags.map(|flags| {
            let [f0, f1] = flags.to_ne_bytes();
            [
                word(&head[..4]),
                word(&head[4..]),
                word(&[t0, t1, f0, f1]),
                0,
            ]
        });
        Self { blocks, keys }
    }

    /// Store the leading groups of four cells of `line` that this quad
    /// stands for into `slots`, returning how many cells. Each group's
    /// characters, which must render (no C0 or C1 control), go to lane `k`
    /// of block `k`: cell `k`'s character field.
    #[cfg(target_arch = "x86_64")]
    #[inline(always)]
    fn store(&self, slots: &mut [MaybeUninit<TermyCell>], line: &[engine::Cell]) -> usize {
        use std::arch::x86_64::{
            __m128i, _mm_and_si128, _mm_cmpeq_epi32, _mm_cmpgt_epi32, _mm_cmplt_epi32,
            _mm_load_si128, _mm_loadu_si128, _mm_movemask_epi8, _mm_or_si128, _mm_set_epi32,
            _mm_set1_epi32, _mm_storeu_si128, _mm_unpackhi_epi32, _mm_unpackhi_epi64,
        };
        let groups = slots.len().min(line.len()) / 4;
        let mut stored = 0;
        // SAFETY: SSE2 is part of the x86_64 baseline. Group `n` reads cells
        // `4n..4n + 4` of `line` (16-aligned, 16 bytes each) and writes the
        // 80 bytes of slots `4n..4n + 4`, both in bounds. The bytes written
        // are the template's frame cells with a character in each character
        // field, as `Template::store` writes them.
        unsafe {
            let load = |words: &[u32; 4]| _mm_loadu_si128(words.as_ptr().cast::<__m128i>());
            let blocks = self.blocks.each_ref().map(load);
            let keys = self.keys.each_ref().map(load);
            let lanes = [
                _mm_set_epi32(0, 0, 0, -1),
                _mm_set_epi32(0, 0, -1, 0),
                _mm_set_epi32(0, -1, 0, 0),
                _mm_set_epi32(-1, 0, 0, 0),
            ];
            let (space, tilde, nbsp) = (
                _mm_set1_epi32(0x20),
                _mm_set1_epi32(0x7e),
                _mm_set1_epi32(0xa0),
            );
            for group in 0..groups {
                let source = line.as_ptr().add(group * 4).cast::<__m128i>();
                let cells = [0, 1, 2, 3].map(|cell| _mm_load_si128(source.add(cell)));
                let same = _mm_and_si128(
                    _mm_and_si128(
                        _mm_cmpeq_epi32(cells[0], keys[0]),
                        _mm_cmpeq_epi32(cells[1], keys[1]),
                    ),
                    _mm_and_si128(
                        _mm_cmpeq_epi32(cells[2], keys[2]),
                        _mm_cmpeq_epi32(cells[3], keys[3]),
                    ),
                );
                // Each cell's last lane, its character: [c0, c1, c2, c3].
                let characters = _mm_unpackhi_epi64(
                    _mm_unpackhi_epi32(cells[0], cells[1]),
                    _mm_unpackhi_epi32(cells[2], cells[3]),
                );
                // Characters are below 0x110000, so signed compares hold.
                let control = _mm_or_si128(
                    _mm_cmplt_epi32(characters, space),
                    _mm_and_si128(
                        _mm_cmpgt_epi32(characters, tilde),
                        _mm_cmplt_epi32(characters, nbsp),
                    ),
                );
                if _mm_movemask_epi8(same) & 0x0fff != 0x0fff || _mm_movemask_epi8(control) != 0 {
                    break;
                }
                let out = slots.as_mut_ptr().add(group * 4).cast::<__m128i>();
                for (block, lane) in lanes.into_iter().enumerate() {
                    let character = _mm_and_si128(characters, lane);
                    _mm_storeu_si128(out.add(block), _mm_or_si128(blocks[block], character));
                }
                _mm_storeu_si128(out.add(4), blocks[4]);
                stored += 4;
            }
        }
        stored
    }

    #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
    #[inline(always)]
    fn store(&self, slots: &mut [MaybeUninit<TermyCell>], line: &[engine::Cell]) -> usize {
        use std::arch::aarch64::{
            vandq_u32, vceqq_u32, vcgtq_u32, vcltq_u32, vdupq_n_u32, vld1q_u32, vmaxvq_u32,
            vminvq_u32, vorrq_u32, vreinterpretq_u32_u64, vreinterpretq_u64_u32, vst1q_u32,
            vzip2q_u32, vzip2q_u64,
        };
        let groups = slots.len().min(line.len()) / 4;
        let mut stored = 0;
        // SAFETY: NEON is part of the aarch64 baseline. Bounds and the bytes
        // written are as in the x86_64 version (this target is little-endian
        // too).
        unsafe {
            let load = |words: &[u32; 4]| vld1q_u32(words.as_ptr());
            let blocks = self.blocks.each_ref().map(load);
            let keys = self.keys.each_ref().map(load);
            let lanes = [0, 1, 2, 3].map(|lane| {
                let mut words = [0; 4];
                words[lane] = u32::MAX;
                vld1q_u32(words.as_ptr())
            });
            // Lane 3 of `same` compares characters with nothing: ignore it.
            let character_lane = lanes[3];
            let (space, tilde, nbsp) = (vdupq_n_u32(0x20), vdupq_n_u32(0x7e), vdupq_n_u32(0xa0));
            for group in 0..groups {
                let source = line.as_ptr().add(group * 4).cast::<u32>();
                let cells = [0, 1, 2, 3].map(|cell| vld1q_u32(source.add(cell * 4)));
                let same = vandq_u32(
                    vandq_u32(vceqq_u32(cells[0], keys[0]), vceqq_u32(cells[1], keys[1])),
                    vandq_u32(vceqq_u32(cells[2], keys[2]), vceqq_u32(cells[3], keys[3])),
                );
                // Each cell's last lane, its character: [c0, c1, c2, c3].
                let characters = vreinterpretq_u32_u64(vzip2q_u64(
                    vreinterpretq_u64_u32(vzip2q_u32(cells[0], cells[1])),
                    vreinterpretq_u64_u32(vzip2q_u32(cells[2], cells[3])),
                ));
                let control = vorrq_u32(
                    vcltq_u32(characters, space),
                    vandq_u32(vcgtq_u32(characters, tilde), vcltq_u32(characters, nbsp)),
                );
                if vminvq_u32(vorrq_u32(same, character_lane)) != u32::MAX
                    || vmaxvq_u32(control) != 0
                {
                    break;
                }
                let out = slots.as_mut_ptr().add(group * 4).cast::<u32>();
                for (block, lane) in lanes.into_iter().enumerate() {
                    let character = vandq_u32(characters, lane);
                    vst1q_u32(out.add(block * 4), vorrq_u32(blocks[block], character));
                }
                vst1q_u32(out.add(16), blocks[4]);
                stored += 4;
            }
        }
        stored
    }

    #[cfg(not(any(
        target_arch = "x86_64",
        all(target_arch = "aarch64", target_endian = "little")
    )))]
    #[inline(always)]
    fn store(&self, _slots: &mut [MaybeUninit<TermyCell>], _line: &[engine::Cell]) -> usize {
        0
    }
}

fn legacy_cell(
    cell: &engine::Cell,
    wrapped: bool,
    palette: &TerminalPalette,
    fallback: TerminalQueryColors,
) -> TermyCell {
    let attributes = cell.style.attributes;
    let bold = attributes & engine::Style::BOLD != 0;
    let inverse = attributes & engine::Style::INVERSE != 0;
    let foreground = render_color(cell.style.foreground, true);
    let background = render_color(cell.style.background, false);
    let (mut foreground, background) = if inverse {
        (background, foreground)
    } else {
        (foreground, background)
    };
    if bold && let TerminalRenderColor::Indexed(index @ 0..=7) = foreground {
        foreground = TerminalRenderColor::Indexed(index + 8);
    }
    let mut fg = resolve_color(foreground, palette, fallback);
    if attributes & engine::Style::DIM != 0 {
        fg.r /= 2;
        fg.g /= 2;
        fg.b /= 2;
    }
    let spacer = cell.flags & (engine::Cell::WIDE_SPACER | engine::Cell::LEADING_WIDE_SPACER) != 0;
    TermyCell {
        char: cell.character,
        fg,
        bg: resolve_color(background, palette, fallback),
        uses_terminal_default_bg: background == TerminalRenderColor::DefaultBackground,
        bold,
        italic: attributes & engine::Style::ITALIC != 0,
        underline: cell.style.underline != engine::UnderlineStyle::None,
        strikethrough: attributes & engine::Style::STRIKE != 0,
        render_text: !spacer
            && attributes & engine::Style::HIDDEN == 0
            && cell.character != '\0'
            && !cell.character.is_control(),
        wide_character_spacer: spacer,
        line_wrapped: wrapped,
    }
}

/// The frame cell of a render cell, resolved like [`legacy_cell`] resolves
/// the engine cell it was read from: a viewer of a hosted pane builds frame
/// updates from the state it mirrors, as the host would (see
/// [`legacy_frame_update`]).
fn legacy_render_cell(
    cell: &TerminalRenderCell,
    palette: &TerminalPalette,
    fallback: TerminalQueryColors,
) -> TermyCell {
    let (mut foreground, background) = if cell.inverse {
        (cell.background, cell.foreground)
    } else {
        (cell.foreground, cell.background)
    };
    if cell.bold
        && let TerminalRenderColor::Indexed(index @ 0..=7) = foreground
    {
        foreground = TerminalRenderColor::Indexed(index + 8);
    }
    let mut fg = resolve_color(foreground, palette, fallback);
    if cell.dim {
        fg.r /= 2;
        fg.g /= 2;
        fg.b /= 2;
    }
    let character = cell.text.chars().next().unwrap_or(' ');
    let spacer = cell.wide_character_spacer || cell.leading_wide_character_spacer;
    TermyCell {
        char: character,
        fg,
        bg: resolve_color(background, palette, fallback),
        uses_terminal_default_bg: background == TerminalRenderColor::DefaultBackground,
        bold: cell.bold,
        italic: cell.italic,
        underline: cell.underline_style != TerminalUnderlineStyle::None,
        strikethrough: cell.strikethrough,
        render_text: !spacer && !cell.hidden && character != '\0' && !character.is_control(),
        wide_character_spacer: spacer,
        line_wrapped: cell.line_wrapped,
    }
}

/// A frame update from a render read of the whole viewport (such as a
/// hosted pane's mirrored state) and the damage since the read before.
/// Scrolls become damage on the rows they moved, as in a local terminal's
/// [`CustomBackend::frame_update`].
pub(crate) fn legacy_frame_update(
    read: &TerminalRenderRead,
    mut update: TerminalRenderDamageSnapshot,
    fallback: TerminalQueryColors,
) -> TermyFrameUpdate {
    let metadata = read.metadata;
    let cols = usize::from(metadata.cols);
    let rows = usize::from(metadata.rows);
    expand_legacy_scroll_damage(&mut update, cols);
    let mut damage = update.damage;
    // Damage that does not fit the read, or a read that does not cover the
    // viewport, cannot patch it: send the whole viewport.
    let fits = read.cells.len() == cols * rows
        && match &damage {
            TerminalDamageSnapshot::Full => true,
            TerminalDamageSnapshot::Partial(spans) => spans.iter().all(|span| {
                span.row < rows && span.left_col <= span.right_col && span.right_col < cols
            }),
        };
    if !fits {
        damage = TerminalDamageSnapshot::Full;
    }
    let cells = match &damage {
        TerminalDamageSnapshot::Full => {
            let mut cells = crate::frame::frame_cells(read.cells.len());
            cells.extend(
                read.cells
                    .iter()
                    .map(|cell| legacy_render_cell(cell, &read.palette, fallback)),
            );
            cells
        }
        TerminalDamageSnapshot::Partial(spans) => {
            let mut cells = crate::frame::frame_cells(
                spans
                    .iter()
                    .map(|span| span.right_col + 1 - span.left_col)
                    .sum(),
            );
            for span in spans {
                let start = span.row * cols;
                cells.extend(
                    read.cells[start + span.left_col..=start + span.right_col]
                        .iter()
                        .map(|cell| legacy_render_cell(cell, &read.palette, fallback)),
                );
            }
            cells
        }
    };
    TermyFrameUpdate {
        cols: metadata.cols,
        rows: metadata.rows,
        cells,
        cursor: metadata.cursor,
        display_offset: metadata.display_offset,
        history_size: metadata.history_size,
        damage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The run conversion stores each cell from its style's template; it
    // must produce exactly what resolving every cell on its own does.
    #[test]
    fn templated_runs_match_cells_resolved_one_by_one() {
        let mut palette = TerminalPalette {
            indexed: [None; 256],
            foreground: Some(TerminalColor { r: 1, g: 2, b: 3 }),
            background: None,
            cursor: None,
            revision: 0,
        };
        palette.indexed[9] = Some(TerminalColor { r: 9, g: 8, b: 7 });
        let fallback = TerminalQueryColors::default();
        let styles = [
            engine::Style::default(),
            engine::Style {
                foreground: engine::Color::indexed(1),
                background: engine::Color::rgb(4, 5, 6),
                attributes: engine::Style::BOLD | engine::Style::ITALIC,
                underline: engine::UnderlineStyle::Curly,
            },
            engine::Style {
                foreground: engine::Color::rgb(200, 100, 50),
                attributes: engine::Style::DIM
                    | engine::Style::INVERSE
                    | engine::Style::STRIKE
                    | engine::Style::HIDDEN,
                ..engine::Style::default()
            },
        ];
        let characters = [
            'a',
            ' ',
            '\0',
            '\u{1b}',
            '\u{85}',
            '\u{a0}',
            '界',
            '\u{1f642}',
        ];
        let flags = [
            0,
            engine::Cell::WIDE,
            engine::Cell::WIDE_SPACER,
            engine::Cell::LEADING_WIDE_SPACER,
        ];
        let mut line = Vec::new();
        for (index, &character) in characters.iter().cycle().take(97).enumerate() {
            line.push(engine::Cell {
                character,
                style: styles[index / 5 % styles.len()],
                flags: flags[index % flags.len()],
            });
        }
        for wrapped in [false, true] {
            let mut cells = Vec::new();
            let mut legacy = LegacyCells::new(&palette, fallback);
            // Two calls: the second starts from the cached style.
            legacy.extend(&mut cells, &line[..40], false);
            legacy.extend(&mut cells, &line[40..], wrapped);
            let expected: Vec<_> = line
                .iter()
                .enumerate()
                .map(|(col, cell)| {
                    legacy_cell(cell, wrapped && col + 1 == line.len(), &palette, fallback)
                })
                .collect();
            assert_eq!(cells, expected);
        }
    }

    // A row's erase blanks are stored from a pattern of four, not read: for
    // every suffix length and background they must be the cells resolved
    // one by one.
    #[test]
    fn blank_suffixes_match_cells_resolved_one_by_one() {
        let mut palette = TerminalPalette {
            indexed: [None; 256],
            foreground: None,
            background: Some(TerminalColor { r: 7, g: 7, b: 7 }),
            cursor: None,
            revision: 0,
        };
        palette.indexed[4] = Some(TerminalColor { r: 1, g: 2, b: 3 });
        let fallback = TerminalQueryColors::default();
        let backgrounds = [
            engine::Color::DEFAULT,
            engine::Color::indexed(4),
            engine::Color::rgb(9, 8, 7),
        ];
        let mut legacy = LegacyCells::new(&palette, fallback);
        let mut cells = Vec::new();
        let mut expected = Vec::new();
        for count in 0..14 {
            for (index, &background) in backgrounds.iter().enumerate() {
                let wrapped = count % 2 == index % 2;
                let mut line: Vec<_> = "abc"[..index]
                    .chars()
                    .map(|character| engine::Cell {
                        character,
                        ..engine::Cell::default()
                    })
                    .collect();
                let text = line.len();
                line.extend((0..count).map(|_| engine::Cell {
                    style: engine::Style {
                        background,
                        ..engine::Style::default()
                    },
                    ..engine::Cell::default()
                }));
                legacy.extend_row(&mut cells, &line, wrapped, text, background);
                expected.extend(line.iter().enumerate().map(|(col, cell)| {
                    legacy_cell(cell, wrapped && col + 1 == line.len(), &palette, fallback)
                }));
                assert_eq!(cells, expected, "{count} blanks on {background:?}");
            }
        }
    }

    // Lines of text runs and wide glyph pairs, interrupted at random by
    // cells the four-cell stores turn away, convert at every offset as
    // each cell resolved on its own does.
    #[test]
    fn four_cell_stores_match_cells_resolved_one_by_one() {
        let palette = TerminalPalette {
            indexed: [None; 256],
            foreground: None,
            background: None,
            cursor: None,
            revision: 0,
        };
        let fallback = TerminalQueryColors::default();
        let styles = [
            engine::Style::default(),
            engine::Style {
                foreground: engine::Color::indexed(3),
                attributes: engine::Style::HIDDEN | engine::Style::BOLD,
                ..engine::Style::default()
            },
        ];
        let cell = |character, style: usize, flags| engine::Cell {
            character,
            style: styles[style],
            flags,
        };
        let mut seed = 0x9e37_79b9_u32;
        let mut next = |bound: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed % bound
        };
        for _ in 0..200 {
            let mut line = Vec::new();
            while line.len() < 150 {
                let style = next(2) as usize;
                match next(6) {
                    0..=2 => {
                        for _ in 0..next(24) {
                            let character = char::from(b' ' + next(95) as u8);
                            line.push(cell(character, style, 0));
                        }
                    }
                    3 => {
                        for _ in 0..next(12) {
                            let glyph = char::from_u32(0x4e00 + next(0x100)).unwrap();
                            line.push(cell(glyph, style, engine::Cell::WIDE));
                            line.push(cell(' ', style, engine::Cell::WIDE_SPACER));
                        }
                    }
                    4 => {
                        let characters = ['\0', '\u{1b}', '\u{7f}', '\u{85}', '\u{9f}', '\u{a0}'];
                        line.push(cell(characters[next(6) as usize], style, 0));
                    }
                    _ => {
                        let flags = [
                            engine::Cell::WIDE,
                            engine::Cell::WIDE_SPACER,
                            engine::Cell::LEADING_WIDE_SPACER,
                            engine::Cell::WIDE | engine::Cell::LEADING_WIDE_SPACER,
                        ];
                        line.push(cell('x', style, flags[next(4) as usize]));
                    }
                }
            }
            for start in 0..4 {
                let line = &line[start..];
                let mut cells = Vec::new();
                LegacyCells::new(&palette, fallback).extend(&mut cells, line, false);
                let expected: Vec<_> = line
                    .iter()
                    .map(|cell| legacy_cell(cell, false, &palette, fallback))
                    .collect();
                assert_eq!(cells, expected, "{line:?}");
            }
        }
    }
}
