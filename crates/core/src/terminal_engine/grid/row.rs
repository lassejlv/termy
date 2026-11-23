//! Cold rows retain scalar/flag arrays and style runs; their sparse shared
//! metadata stays in the row's table either way. The live screen keeps dense
//! cells. Borrowed public reads materialize a row until the next mutation;
//! streaming reads use caller-owned scratch instead.
use super::super::types::{CellExtra, Line, MAX_COMBINING_BYTES};
use super::{Cell, Color, Row, Size, Style};
use std::sync::{Arc, OnceLock};

/// A character (21 bits) and its cell flags (top byte) in four bytes.
/// Transparent, so the packing loops store them as `u32` lanes.
#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
struct Scalar(u32);

impl Scalar {
    const FLAGS_SHIFT: u32 = 24;

    /// A cell's character and flags, from its words (see [`cell_words`]).
    /// Cell flags use the low three bits.
    #[inline(always)]
    fn from_words([_, tail]: [u64; 2]) -> Self {
        // The character is the second word's last four bytes and the flags
        // the two before them: shifts on little-endian targets, where the
        // packing loop then vectorizes.
        if cfg!(target_endian = "little") {
            Self((tail >> 32) as u32 | ((tail >> 16 & 0xff) as u32) << Self::FLAGS_SHIFT)
        } else {
            let bytes = tail.to_ne_bytes();
            let character = u32::from_ne_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            Self(character | u32::from(bytes[3]) << Self::FLAGS_SHIFT)
        }
    }

    fn character(self) -> char {
        char::from_u32(self.0 & ((1 << Self::FLAGS_SHIFT) - 1)).unwrap_or(' ')
    }

    fn flags(self) -> u16 {
        (self.0 >> Self::FLAGS_SHIFT) as u16
    }
}

/// A cell's two words. `Cell` is `repr(C, align(16))`: its style's ten
/// bytes, its flags and its character, with no padding.
#[inline(always)]
fn cell_words(cell: &Cell) -> [u64; 2] {
    // SAFETY: a cell is 16 initialized bytes, 16-aligned (see above).
    unsafe { std::ptr::from_ref(cell).cast::<[u64; 2]>().read() }
}

/// The words of a cell's style: its first eight bytes and, in the second
/// word, the two bytes before the flags.
#[inline(always)]
fn style_key(cell: &Cell) -> [u64; 2] {
    let [head, tail] = cell_words(cell);
    let mask = u64::from_ne_bytes([0xff, 0xff, 0, 0, 0, 0, 0, 0]);
    [head, tail & mask]
}

/// Append each cell's [`Scalar`] to `scalars`, which has room for them, and
/// report whether every cell has the first one's style. Four cells at a time
/// with the target's baseline vectors, then one at a time; both passes are
/// branch-free, unlike scanning for a style run's end.
#[inline]
fn pack_scalars(cells: &[Cell], scalars: &mut Vec<Scalar>) -> bool {
    let Some(first) = cells.first() else {
        return true;
    };
    let spare = &mut scalars.spare_capacity_mut()[..cells.len()];
    let grouped = cells.len() / 4 * 4;
    let mut same = pack_groups(&cells[..grouped], &mut spare[..grouped], first);
    let key = style_key(first);
    let mut diff = [0, 0];
    for (slot, cell) in spare[grouped..].iter_mut().zip(&cells[grouped..]) {
        slot.write(Scalar::from_words(cell_words(cell)));
        let other = style_key(cell);
        diff = [diff[0] | (other[0] ^ key[0]), diff[1] | (other[1] ^ key[1])];
    }
    same &= diff == [0, 0];
    // SAFETY: the passes above initialized the `cells.len()` slots after
    // the length.
    unsafe { scalars.set_len(scalars.len() + cells.len()) };
    same
}

/// Sixteen-byte vectors for [`pack_scalars`]: a cell is one vector of four
/// 32-bit lanes, its style's ten bytes in lanes 0 to 2, its flags in lane
/// 2's upper half and its character in lane 3 (little-endian). Stores
/// `cells.len()` (a multiple of four) scalars into `out`; returns whether
/// every cell has `first`'s style.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn pack_groups(cells: &[Cell], out: &mut [std::mem::MaybeUninit<Scalar>], first: &Cell) -> bool {
    use std::arch::x86_64::{
        __m128i, _mm_and_si128, _mm_cmpeq_epi8, _mm_load_si128, _mm_movemask_epi8, _mm_or_si128,
        _mm_set_epi32, _mm_set1_epi32, _mm_setzero_si128, _mm_slli_epi32, _mm_storeu_si128,
        _mm_unpackhi_epi32, _mm_unpackhi_epi64, _mm_unpacklo_epi64, _mm_xor_si128,
    };
    debug_assert!(cells.len().is_multiple_of(4) && out.len() == cells.len());
    // SAFETY: SSE2 is part of the x86_64 baseline. Cells are 16 bytes and
    // 16-aligned, so each aligned load reads one in bounds; group `n` reads
    // cells `4n..4n + 4` and writes the four scalars `4n..4n + 4` of `out`,
    // which `Scalar`'s transparent `u32` makes a 16-byte unaligned store.
    unsafe {
        let key = _mm_load_si128(std::ptr::from_ref(first).cast::<__m128i>());
        let flag_byte = _mm_set1_epi32(0xff00_0000_u32 as i32);
        let mut diff = _mm_setzero_si128();
        let input = cells.as_ptr().cast::<__m128i>();
        let output = out.as_mut_ptr().cast::<__m128i>();
        for group in 0..cells.len() / 4 {
            let cell = |index: usize| _mm_load_si128(input.add(group * 4 + index));
            let [a, b, c, d] = [cell(0), cell(1), cell(2), cell(3)];
            diff = _mm_or_si128(
                diff,
                _mm_or_si128(
                    _mm_or_si128(_mm_xor_si128(a, key), _mm_xor_si128(b, key)),
                    _mm_or_si128(_mm_xor_si128(c, key), _mm_xor_si128(d, key)),
                ),
            );
            // [a2, b2, a3, b3] and [c2, d2, c3, d3].
            let low = _mm_unpackhi_epi32(a, b);
            let high = _mm_unpackhi_epi32(c, d);
            let flags = _mm_unpacklo_epi64(low, high);
            let characters = _mm_unpackhi_epi64(low, high);
            // Lane 2's third byte (the flags' low byte) moves to the top.
            let scalars = _mm_or_si128(
                characters,
                _mm_and_si128(_mm_slli_epi32::<8>(flags), flag_byte),
            );
            _mm_storeu_si128(output.add(group), scalars);
        }
        // The style's ten bytes: lanes 0 and 1 and lane 2's lower half.
        let style = _mm_set_epi32(0, 0xffff, -1, -1);
        let differs = _mm_and_si128(diff, style);
        _mm_movemask_epi8(_mm_cmpeq_epi8(differs, _mm_setzero_si128())) == 0xffff
    }
}

#[cfg(all(target_arch = "aarch64", target_endian = "little"))]
#[inline(always)]
fn pack_groups(cells: &[Cell], out: &mut [std::mem::MaybeUninit<Scalar>], first: &Cell) -> bool {
    use std::arch::aarch64::{
        vandq_u32, vdupq_n_u32, veorq_u32, vld4q_u32, vmaxvq_u32, vorrq_u32, vshlq_n_u32, vst1q_u32,
    };
    debug_assert!(cells.len().is_multiple_of(4) && out.len() == cells.len());
    let [head, tail] = cell_words(first);
    // SAFETY: NEON is part of the aarch64 baseline. Group `n` reads the 64
    // bytes of cells `4n..4n + 4`, deinterleaved into each lane's four
    // cells, and writes the four scalars `4n..4n + 4` of `out`, both in
    // bounds; lanes are as in the x86_64 version (this target is
    // little-endian too).
    unsafe {
        let keys =
            [head as u32, (head >> 32) as u32, tail as u32 & 0xffff].map(|word| vdupq_n_u32(word));
        let style_half = vdupq_n_u32(0xffff);
        let flag_byte = vdupq_n_u32(0xff00_0000);
        let mut diff = vdupq_n_u32(0);
        let input = cells.as_ptr().cast::<u32>();
        let output = out.as_mut_ptr().cast::<u32>();
        for group in 0..cells.len() / 4 {
            let lanes = vld4q_u32(input.add(group * 16));
            diff = vorrq_u32(
                diff,
                vorrq_u32(
                    vorrq_u32(veorq_u32(lanes.0, keys[0]), veorq_u32(lanes.1, keys[1])),
                    vandq_u32(veorq_u32(lanes.2, keys[2]), style_half),
                ),
            );
            let scalars = vorrq_u32(lanes.3, vandq_u32(vshlq_n_u32::<8>(lanes.2), flag_byte));
            vst1q_u32(output.add(group * 4), scalars);
        }
        vmaxvq_u32(diff) == 0
    }
}

#[cfg(not(any(
    target_arch = "x86_64",
    all(target_arch = "aarch64", target_endian = "little")
)))]
#[inline(always)]
fn pack_groups(cells: &[Cell], out: &mut [std::mem::MaybeUninit<Scalar>], first: &Cell) -> bool {
    let key = style_key(first);
    let mut diff = [0, 0];
    for (slot, cell) in out.iter_mut().zip(cells) {
        slot.write(Scalar::from_words(cell_words(cell)));
        let other = style_key(cell);
        diff = [diff[0] | (other[0] ^ key[0]), diff[1] | (other[1] ^ key[1])];
    }
    diff == [0, 0]
}

#[derive(Clone, Debug, Default)]
pub(super) struct PackedCells {
    decoded: OnceLock<Vec<Cell>>,
    cols: usize,
    background: Color,
    scalars: Vec<Scalar>,
    // Run ends (exclusive column) and styles; `u32` keeps a run at 16 bytes.
    styles: Vec<(u32, Style)>,
}

impl PackedCells {
    /// Pack `cells`, returning false when their packed form wouldn't be
    /// smaller enough to keep. With `full_width`, the scalars keep room for
    /// the whole row (see [`Row::retain`]); otherwise they are kept within
    /// twice their length.
    fn encode(&mut self, cells: &[Cell], cols: usize, background: Color, full_width: bool) -> bool {
        self.decoded.take();
        self.cols = cols;
        self.background = background;
        self.scalars.clear();
        self.styles.clear();
        // Buffers recycled from a longer row must not keep its capacity.
        // Room for the whole row is a quarter of its dense size, and lets
        // lines of any length reuse the buffer.
        let target = if full_width {
            cols.max(cells.len())
        } else {
            cells.len()
        };
        let capacity = self.scalars.capacity();
        if capacity < cells.len() {
            self.scalars.reserve_exact(target);
        } else if capacity > target.max(2 * cells.len()) {
            self.scalars.shrink_to(target);
        }
        let single_style = pack_scalars(cells, &mut self.scalars);
        let fixed_bytes = self.scalars.capacity() * size_of::<Scalar>();
        let budget = cols * size_of::<Cell>() / 2;
        if fixed_bytes >= budget {
            return false;
        }
        if let Some(first) = cells.first()
            && single_style
        {
            // Most rows have one run; don't reserve room for four.
            self.styles.reserve_exact(1);
            self.styles.push((cells.len() as u32, first.style));
        } else {
            // Style runs, found by scanning for each run's end instead of
            // probing per cell.
            let mut start = 0;
            while let Some(first) = cells.get(start) {
                let key = style_key(first);
                let end = cells[start..]
                    .iter()
                    .position(|cell| style_key(cell) != key)
                    .map_or(cells.len(), |len| start + len);
                if self.styles.is_empty() {
                    self.styles.reserve_exact(1);
                }
                self.styles.push((end as u32, first.style));
                if fixed_bytes + self.styles.capacity() * size_of::<(u32, Style)>() >= budget {
                    return false;
                }
                start = end;
            }
        }
        if fixed_bytes + self.styles.capacity() * size_of::<(u32, Style)>() >= budget {
            return false;
        }
        // Bounded slack keeps reuse cheap without retaining a wide row's runs.
        if self.styles.capacity() > 2 * self.styles.len() + 4 {
            self.styles.shrink_to(self.styles.len());
        }
        true
    }

    fn decode(&self, cells: &mut Vec<Cell>) {
        let blank = Cell {
            style: Style {
                background: self.background,
                ..Style::default()
            },
            ..Cell::default()
        };
        cells.clear();
        cells.resize(self.cols, blank);
        for (cell, scalar) in cells.iter_mut().zip(&self.scalars) {
            cell.character = scalar.character();
            cell.flags = scalar.flags();
        }
        let mut start = 0;
        for &(end, style) in &self.styles {
            let end = end as usize;
            for cell in &mut cells[start..end] {
                cell.style = style;
            }
            start = end;
        }
    }
}

/// Most of a wide row's packed scalars: lines beyond this are left to the
/// hardware prefetcher, which follows the stores by then.
const PREFETCH_BYTES: usize = 512;

/// Hint that `address` is about to be accessed. Prefetches never fault, so
/// any address is fine.
#[inline(always)]
fn prefetch<T>(address: *const T) {
    #[cfg(target_arch = "x86_64")]
    // SAFETY: SSE is part of the x86_64 baseline; a prefetch reads nothing.
    unsafe {
        use std::arch::x86_64::{_MM_HINT_T0, _mm_prefetch};
        _mm_prefetch::<_MM_HINT_T0>(address.cast());
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: a prefetch for store reads and writes nothing.
    unsafe {
        std::arch::asm!(
            "prfm pstl1keep, [{address}]",
            address = in(reg) address,
            options(nostack, readonly, preserves_flags)
        );
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let _ = address;
}

/// Rows store metadata bytes in units of this many, so a row of 4096
/// maximal composed cells still fits a `u16` beside the row's other fields.
const EXTRA_UNIT: usize = 8;
/// One extra's allocation: `ArcInner`'s strong and weak counts, then the value.
const EXTRA_ALLOCATION: usize = 2 * size_of::<usize>() + size_of::<CellExtra>();
const _: () = assert!(
    Size::MAX_COLS * (EXTRA_ALLOCATION + MAX_COMBINING_BYTES) <= u16::MAX as usize * EXTRA_UNIT
);

/// Measures the heap bytes of the cell metadata a row references: each
/// extra's allocation and combining text. Pen metadata and composed-cell cache
/// entries are shared by runs of cells (and wide glyphs alternate with their
/// spacers' extras), so an extra equal to one of the last two distinct ones is
/// not counted again.
///
/// Extras are compared by value, never by allocation: whether two equal cells
/// share one depends on caches (and pointer-keyed buckets) that differ between
/// an engine and one decoded from its snapshot, and history's eviction must
/// not. [`Row::share_equal_extras`] makes the sharing match the measurement.
#[derive(Default)]
struct ExtraMeter<'a> {
    last: Option<&'a Arc<CellExtra>>,
    before: Option<&'a Arc<CellExtra>>,
    bytes: usize,
}

fn same_extra(left: &Arc<CellExtra>, right: &Arc<CellExtra>) -> bool {
    Arc::ptr_eq(left, right)
        || (left.underline_color == right.underline_color
            && left.combining == right.combining
            && match (&left.hyperlink, &right.hyperlink) {
                (None, None) => true,
                (Some(left), Some(right)) => Arc::ptr_eq(left, right) || left == right,
                _ => false,
            })
}

impl<'a> ExtraMeter<'a> {
    /// The recent extra equal to `extra`, if any; otherwise counts it.
    // Inlined into the measuring loops: left to the inliner, unrelated
    // changes to the crate flip it.
    #[inline(always)]
    fn add(&mut self, extra: &'a Arc<CellExtra>) -> Option<&'a Arc<CellExtra>> {
        for recent in [self.last, self.before].into_iter().flatten() {
            if same_extra(recent, extra) {
                return Some(recent);
            }
        }
        (self.before, self.last) = (self.last, Some(extra));
        self.bytes += EXTRA_ALLOCATION + extra.combining.len();
        None
    }
}

fn extra_units(bytes: usize) -> u16 {
    u16::try_from(bytes.div_ceil(EXTRA_UNIT)).unwrap_or(u16::MAX)
}

impl Row {
    /// Heap bytes of the stored form (dense cells or packed runs, and the
    /// metadata table) plus the metadata charged by [`Self::measure_extras`],
    /// excluding a transient decoded read cache. Interned hyperlinks are
    /// bounded by their own table. History rows don't change, so a row keeps
    /// its charge until it leaves history or is measured again.
    pub(super) fn heap_bytes(&self) -> usize {
        let extras = usize::from(self.extra_units) * EXTRA_UNIT;
        self.cells.capacity() * size_of::<Cell>()
            + self.extras.capacity() * size_of::<(usize, Arc<CellExtra>)>()
            + extras
            + self.packed.as_ref().map_or(0, |packed| {
                size_of::<PackedCells>()
                    + packed.scalars.capacity() * size_of::<Scalar>()
                    + packed.styles.capacity() * size_of::<(u32, Style)>()
            })
    }

    /// What history charges this row besides its ring slot: its cells as a
    /// packed row of its width with one style run, its metadata table's
    /// entries and the metadata charged by [`Self::measure_extras`]. Unlike
    /// [`Self::heap_bytes`], it doesn't depend on whether the row is stored
    /// dense or packed, nor on its buffers' capacities (see `history.rs`).
    #[inline]
    pub(super) fn charged_bytes(&self) -> usize {
        let cols = self
            .packed
            .as_ref()
            .map_or(self.cells.len(), |packed| packed.cols);
        self.charged_bytes_at(cols)
    }

    /// [`Self::charged_bytes`] as though the row were `cols` wide.
    #[inline]
    pub(super) fn charged_bytes_at(&self, cols: usize) -> usize {
        Self::charged_cells(cols) + self.metadata_charge()
    }

    /// The part of [`Self::charged_bytes`] that doesn't depend on the width.
    #[inline]
    pub(super) fn metadata_charge(&self) -> usize {
        self.extras.len() * size_of::<(usize, Arc<CellExtra>)>()
            + usize::from(self.extra_units) * EXTRA_UNIT
    }

    /// [`Self::charged_bytes`] of the cells of a row `cols` wide.
    #[inline]
    pub(super) const fn charged_cells(cols: usize) -> usize {
        size_of::<PackedCells>() + cols * size_of::<Scalar>() + size_of::<(u32, Style)>()
    }

    /// Charge a row entering history its metadata. Plain rows have none;
    /// others are measured from their table, never by scanning cells.
    #[inline]
    pub(super) fn bound_extras(&mut self) {
        if self.has_extras() {
            self.measure_extras();
        } else {
            self.extra_units = 0;
        }
    }

    /// Charge the measured metadata.
    pub(super) fn measure_extras(&mut self) {
        self.share_equal_extras();
        self.extra_units = self.count_extra_units();
    }

    pub(super) fn count_extra_units(&self) -> u16 {
        let mut meter = ExtraMeter::default();
        for (_, extra) in &self.extras {
            meter.add(extra);
        }
        extra_units(meter.bytes)
    }

    /// Point each extra the meter would not count at the equal extra it
    /// matched, so one it charges once is also stored once.
    fn share_equal_extras(&mut self) {
        let mut replace = Vec::new();
        let mut meter = ExtraMeter::default();
        for (index, (_, extra)) in self.extras.iter().enumerate() {
            if let Some(recent) = meter.add(extra)
                && !Arc::ptr_eq(recent, extra)
            {
                replace.push((index, Arc::clone(recent)));
            }
        }
        for (index, extra) in replace {
            self.extras[index].1 = extra;
        }
    }

    /// Prefetch the packed header that recycling this row's storage reads
    /// first. A flood scrolling through a large history recycles rows
    /// whose scattered allocations are long out of cache.
    #[inline(always)]
    pub(super) fn prefetch_packed_header(&self) {
        if let Some(packed) = &self.packed {
            prefetch(std::ptr::from_ref::<PackedCells>(packed));
        }
    }

    /// Prefetch the buffers recycling this row's storage writes. Reads the
    /// packed header, so call it a line after [`Self::prefetch_packed_header`].
    #[inline(always)]
    pub(super) fn prefetch_packed_buffers(&self) {
        if let Some(packed) = &self.packed {
            let scalars = packed.scalars.as_ptr().cast::<u8>();
            let bytes = packed.scalars.capacity() * size_of::<Scalar>();
            for offset in (0..bytes.min(PREFETCH_BYTES)).step_by(64) {
                prefetch(scalars.wrapping_add(offset));
            }
            prefetch(packed.styles.as_ptr());
        }
    }

    #[inline]
    pub(in crate::terminal_engine) fn is_packed(&self) -> bool {
        self.packed.is_some()
    }

    pub(in crate::terminal_engine) fn with_cells<T>(
        &self,
        scratch: &mut Vec<Cell>,
        read: impl FnOnce(&[Cell]) -> T,
    ) -> T {
        if let Some(packed) = &self.packed {
            if let Some(cells) = packed.decoded.get() {
                return read(cells);
            }
            packed.decode(scratch);
            read(scratch)
        } else {
            read(&self.cells)
        }
    }

    /// [`Self::with_cells`], with the cells' metadata.
    pub(in crate::terminal_engine) fn with_line<T>(
        &self,
        scratch: &mut Vec<Cell>,
        read: impl FnOnce(Line<'_>) -> T,
    ) -> T {
        self.with_cells(scratch, |cells| read(Line::new(cells, &self.extras)))
    }

    /// Where this row's erase blanks start, and their background: every
    /// cell from that column on is a blank on that background. Packed rows
    /// report none.
    pub(in crate::terminal_engine) fn blank_suffix(&self) -> (usize, Color) {
        if self.packed.is_some() {
            return (usize::MAX, self.clear_background);
        }
        let start = self.occupied.min(self.cells.len());
        debug_assert!(
            self.cells[start..]
                .iter()
                .all(|cell| *cell == super::Grid::blank_on(self.clear_background))
                && self.extras.last().is_none_or(|(col, _)| *col < start),
            "cells past `occupied` are not erase blanks"
        );
        (start, self.clear_background)
    }

    pub(in crate::terminal_engine) fn cells(&self) -> &[Cell] {
        if let Some(packed) = &self.packed {
            packed.decoded.get_or_init(|| {
                let mut cells = Vec::new();
                packed.decode(&mut cells);
                cells
            })
        } else {
            &self.cells
        }
    }

    pub(super) fn make_dense(&mut self) {
        if let Some(mut packed) = self.packed.take() {
            if let Some(cells) = packed.decoded.take() {
                self.cells = cells;
            } else {
                packed.decode(&mut self.cells);
            }
        }
    }

    /// Drop packed contents without decoding them, keeping a decoded read
    /// cache's allocation (but not its cells) when one exists.
    #[inline]
    pub(super) fn discard_packed(&mut self) -> bool {
        if self.packed.is_none() {
            return false;
        }
        self.discard_packed_contents();
        true
    }

    #[inline(never)]
    fn discard_packed_contents(&mut self) {
        let Some(mut packed) = self.packed.take() else {
            return;
        };
        self.cells = packed.decoded.take().unwrap_or_default();
        self.cells.clear();
    }

    /// Fit a hard-terminated row to `cols` without rewrapping it, keeping
    /// packed history packed. The result matches the logical-line path:
    /// content up to its last non-default cell, then default blanks. Returns
    /// false when the row needs that path (it would wrap, or a cell's width
    /// could be recomputed there).
    pub(super) fn reflow_in_place(&mut self, cols: usize) -> bool {
        let default = Cell::default();
        if self.wrapped || cols < 2 || self.clear_background != default.style.background {
            return false;
        }
        if let Some(packed) = &mut self.packed {
            let recomputed = |col: usize, extra: &CellExtra| {
                packed.scalars[col].flags() & Cell::WIDE == 0 && !extra.combining.is_empty()
            };
            if packed.scalars.len() > cols
                || packed
                    .scalars
                    .iter()
                    .any(|scalar| scalar.flags() & Cell::LEADING_WIDE_SPACER != 0)
                || self
                    .extras
                    .iter()
                    .any(|(col, extra)| recomputed(*col, extra))
            {
                return false;
            }
            packed.decoded.take();
            packed.cols = cols;
            return true;
        }
        let len = self.content_len();
        if len > cols
            || self.cells[..len]
                .iter()
                .any(|cell| cell.flags & Cell::LEADING_WIDE_SPACER != 0)
            || self.extras.iter().any(|(col, extra)| {
                self.cells[*col].flags & Cell::WIDE == 0 && !extra.combining.is_empty()
            })
        {
            return false;
        }
        self.cells.truncate(len);
        self.cells.resize(cols, default);
        self.occupied = len;
        true
    }

    /// Capacities of the stored form: dense cells or packed scalars, packed
    /// style runs, and the metadata table. Snapshots carry them, but decoders
    /// size each buffer to what the row holds.
    pub(super) fn storage_capacities(&self) -> [usize; 3] {
        let (cells, styles) = self
            .packed
            .as_ref()
            .map_or((self.cells.capacity(), 0), |packed| {
                (packed.scalars.capacity(), packed.styles.capacity())
            });
        [cells, styles, self.extras.capacity()]
    }

    pub(super) fn release_read_cache(&mut self) {
        if let Some(packed) = &mut self.packed {
            packed.decoded.take();
        }
    }

    pub(super) fn compact(&mut self) {
        if self.packed.is_some() {
            return;
        }
        let mut packed = Box::<PackedCells>::default();
        let encoded = packed.encode(
            &self.cells[..self.occupied],
            self.cells.len(),
            self.clear_background,
            false,
        );
        // Packed or not, the row is now charged its measured metadata.
        self.measure_extras();
        if !encoded {
            return;
        }
        self.cells = Vec::new();
        self.packed = Some(packed);
    }

    /// Pack this row for history into `recycled`'s packed storage when it
    /// has some, returning it and a live row holding this row's dense cells
    /// (to be cleared). `full_width` packed rows keep room for a whole row,
    /// so a flood recycling them never reallocates.
    pub(super) fn retain(
        mut self,
        recycled: Option<Self>,
        cols: usize,
        blank: &Cell,
        full_width: bool,
    ) -> (Self, Self) {
        if cols < 16 {
            return (self, recycled.unwrap_or_else(|| Self::new(cols, blank)));
        }
        let mut recycled = recycled;
        let mut packed = recycled
            .as_mut()
            .and_then(|row| row.packed.take())
            .unwrap_or_default();
        if !packed.encode(
            &self.cells[..self.occupied],
            cols,
            self.clear_background,
            full_width,
        ) {
            let mut recycled = recycled.unwrap_or_else(|| Self::new(cols, blank));
            if recycled.cells.is_empty() {
                recycled.cells.resize(cols, *blank);
                recycled.extras.clear();
                recycled.occupied = 0;
                recycled.clear_background = blank.style.background;
            }
            return (self, recycled);
        }
        // The packed row keeps its metadata; the live buffer holds its old
        // cells, which the caller clears.
        let live = Self {
            cells: std::mem::take(&mut self.cells),
            extras: Vec::new(),
            packed: None,
            occupied: self.occupied,
            clear_background: self.clear_background,
            wrapped: false,
            prompt: 0,
            extra_units: 0,
        };
        self.packed = Some(packed);
        (self, live)
    }
}
#[cfg(test)]
mod tests {
    use super::super::super::types::ExtraSlot;
    use super::*;
    use crate::terminal_engine::Hyperlink;

    #[test]
    fn plain_packed_rows_store_one_tight_style_run() {
        assert_eq!(size_of::<(u32, Style)>(), 16);
        assert_eq!(size_of::<Scalar>(), 4);
        let mut row = Row::new(120, &Cell::default());
        for (cell, character) in row.cells.iter_mut().zip("plain text".chars()) {
            cell.character = character;
        }
        row.occupied = 10;
        row.compact();
        let packed = row.packed.as_ref().unwrap();
        assert_eq!((packed.styles.len(), packed.styles.capacity()), (1, 1));
        assert_eq!(packed.scalars.capacity(), 10);
    }

    /// Cells with distinct characters and flags in one style.
    fn varied_cells(len: usize, style: Style) -> Vec<Cell> {
        (0..len)
            .map(|index| Cell {
                style,
                flags: (index % 8) as u16,
                character: char::from_u32(0x41 + index as u32 * 997 % 0x10_0000)
                    .filter(|_| index % 5 != 0)
                    .unwrap_or('界'),
            })
            .collect()
    }

    #[test]
    fn packing_finds_a_style_change_in_any_cell_and_byte() {
        use super::super::super::types::UnderlineStyle;
        let base = Style {
            foreground: Color::rgb(1, 2, 3),
            background: Color::indexed(4),
            attributes: Style::BOLD,
            underline: UnderlineStyle::Single,
        };
        // One variant per byte of the style's ten.
        let variants = [
            Style {
                foreground: Color::rgb(9, 2, 3),
                ..base
            },
            Style {
                foreground: Color::rgb(1, 2, 9),
                ..base
            },
            Style {
                background: Color::rgb(0, 0, 4),
                ..base
            },
            Style {
                background: Color::indexed(5),
                ..base
            },
            Style {
                attributes: 0,
                ..base
            },
            Style {
                underline: UnderlineStyle::Curly,
                ..base
            },
        ];
        // Lengths around the four-cell groups and their remainders.
        for len in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 33] {
            let cells = varied_cells(len, base);
            let mut row = Row::new(120, &Cell::default());
            row.cells[..len].copy_from_slice(&cells);
            row.occupied = len;
            row.compact();
            assert_eq!(row.packed.as_ref().unwrap().styles.len(), 1, "{len}");
            assert_eq!(&row.cells()[..len], cells);
            for (changed, variant) in (0..len).flat_map(|col| variants.map(|style| (col, style))) {
                let mut cells = cells.clone();
                cells[changed].style = variant;
                let mut row = Row::new(120, &Cell::default());
                row.cells[..len].copy_from_slice(&cells);
                row.occupied = len;
                row.compact();
                let runs = 1 + usize::from(changed != 0) + usize::from(changed + 1 != len);
                let packed = row.packed.as_ref().expect("packed");
                assert_eq!(packed.styles.len(), runs, "{len} {changed} {variant:?}");
                assert_eq!(&row.cells()[..len], cells, "{len} {changed}");
            }
        }
    }

    #[test]
    fn full_width_packing_reuses_its_buffer_for_any_line() {
        let blank = Cell::default();
        let mut previous = None;
        let mut pointer = None;
        for len in [3, 90, 7, 120, 1, 64] {
            let mut row = Row::new(120, &blank);
            row.cells[..len].copy_from_slice(&varied_cells(len, Style::default()));
            row.occupied = len;
            let expected = row.cells.clone();
            let (packed, live) = row.retain(previous.take(), 120, &blank, true);
            let scalars = &packed.packed.as_ref().expect("packed").scalars;
            assert_eq!(scalars.capacity(), 120);
            assert_eq!(*pointer.get_or_insert(scalars.as_ptr()), scalars.as_ptr());
            assert_eq!(packed.cells(), expected);
            assert_eq!(live.cells.len(), 120);
            previous = Some(packed);
        }
        // Tight packing still sheds a longer line's room.
        let mut row = Row::new(120, &blank);
        row.cells[..2].copy_from_slice(&varied_cells(2, Style::default()));
        row.occupied = 2;
        let (packed, _) = row.retain(previous, 120, &blank, false);
        assert_eq!(packed.packed.as_ref().unwrap().scalars.capacity(), 2);
    }

    #[test]
    fn packed_rows_roundtrip_wide_flags_styles_and_shared_metadata() {
        let blank = Cell {
            style: Style {
                background: Color::indexed(4),
                ..Style::default()
            },
            ..Cell::default()
        };
        let mut row = Row::new(120, &blank);
        row.cells[0].character = '👩';
        row.cells[0].flags = Cell::WIDE;
        row.set_extra(
            0,
            Some(Arc::new(CellExtra {
                combining: "🏽‍💻".into(),
                hyperlink: Some(Arc::new(Hyperlink {
                    id: "id".into(),
                    uri: "https://example.test".into(),
                })),
                ..CellExtra::default()
            })),
        );
        row.cells[1].flags = Cell::WIDE_SPACER;
        row.cells[2].character = 'x';
        row.cells[2].style.foreground = Color::rgb(1, 2, 3);
        row.occupied = 3;
        row.wrapped = true;
        let expected = row.cells.clone();
        let expected_extra = Arc::clone(row.extra(0).unwrap());
        row.compact();
        assert_eq!(row.cells.capacity(), 0);
        assert!(row.packed.is_some());
        let mut scratch = Vec::new();
        row.with_cells(&mut scratch, |cells| assert_eq!(cells, expected));
        assert!(row.packed.as_ref().unwrap().decoded.get().is_none());
        assert_eq!(row.cells(), expected);
        row.release_read_cache();
        assert!(row.packed.as_ref().unwrap().decoded.get().is_none());
        row.make_dense();
        assert_eq!(row.cells, expected);
        assert!(row.wrapped);
        // The metadata never left the row's table.
        assert_eq!(row.extras.len(), 1);
        assert!(Arc::ptr_eq(row.extra(0).unwrap(), &expected_extra));
    }

    #[test]
    fn compact_history_reuses_allocations_and_keeps_live_buffers_dense() {
        let mut grid = super::super::Grid::new(super::super::Size { cols: 120, rows: 4 }, 32);
        for _ in 0..100 {
            grid.write_ascii(b"a short log line");
            grid.carriage_return();
            grid.linefeed();
        }
        grid.compact_history(usize::MAX);
        assert!(grid.primary.rows.iter().all(|row| row.packed.is_none()));
        let mut before: Vec<_> = grid
            .history
            .iter()
            .map(|row| {
                assert!(row.cells.capacity() == 0);
                let packed = row.packed.as_ref().unwrap();
                assert!(packed.decoded.get().is_none());
                packed.scalars.as_ptr()
            })
            .collect();
        for _ in 0..100 {
            grid.write_ascii(b"a short log line");
            grid.carriage_return();
            grid.linefeed();
        }
        let mut after: Vec<_> = grid
            .history
            .iter()
            .map(|row| row.packed.as_ref().unwrap().scalars.as_ptr())
            .collect();
        before.sort();
        after.sort();
        assert_eq!(before, after);
    }

    #[test]
    fn heap_bytes_count_cell_metadata_once_per_shared_run() {
        const ALLOCATION: usize = 2 * size_of::<usize>() + size_of::<CellExtra>();
        // The measurement fits in the row's padding.
        assert_eq!(size_of::<Row>(), size_of::<[usize; 8]>() + 8);
        let mut row = Row::new(120, &Cell::default());
        let plain = row.heap_bytes();
        assert_eq!(plain, 120 * size_of::<Cell>());
        // A run sharing the pen's link counts one allocation.
        let link = Arc::new(CellExtra {
            hyperlink: Some(Arc::new(Hyperlink {
                id: String::new(),
                uri: "https://example.test".into(),
            })),
            ..CellExtra::default()
        });
        for col in 0..10 {
            row.set_extra(col, Some(Arc::clone(&link)));
        }
        // Alternating glyph and spacer extras count once each.
        let color = Arc::new(CellExtra {
            underline_color: Color::indexed(1),
            ..CellExtra::default()
        });
        for col in 10..20 {
            row.set_extra(
                col,
                Some(Arc::clone(if col % 2 == 0 { &link } else { &color })),
            );
        }
        // Distinct composed cells each count their allocation and text.
        let mut combining = 0;
        for col in 20..30 {
            let mut extra = None;
            extra.push_combining(char::from_u32(0x300 + col as u32).unwrap());
            combining += extra.as_ref().unwrap().combining.capacity();
            row.set_extra(col, extra);
        }
        row.occupied = 30;
        // Measuring charges each distinct run's allocation and text, beside
        // the table itself; a row entering history is charged the same.
        let table = row.extras.capacity() * size_of::<(usize, Arc<CellExtra>)>();
        let extras = (12 * ALLOCATION + combining).div_ceil(EXTRA_UNIT) * EXTRA_UNIT;
        assert_eq!(row.heap_bytes(), plain + table);
        row.measure_extras();
        assert_eq!(row.heap_bytes(), plain + table + extras);
        row.extra_units = 0;
        row.bound_extras();
        assert_eq!(row.heap_bytes(), plain + table + extras);
        // The packed row keeps the same table and charge.
        row.compact();
        assert_eq!(row.count_extra_units(), row.extra_units);
        let packed = row.packed.as_ref().unwrap();
        assert_eq!(
            row.heap_bytes(),
            size_of::<PackedCells>()
                + packed.scalars.capacity() * size_of::<Scalar>()
                + packed.styles.capacity() * size_of::<(u32, Style)>()
                + table
                + extras
        );
    }

    #[test]
    fn pathological_style_changes_keep_the_bounded_dense_representation() {
        let mut row = Row::new(120, &Cell::default());
        for (col, cell) in row.cells.iter_mut().enumerate() {
            cell.style.foreground = Color::indexed(col as u8);
        }
        row.occupied = 120;
        row.compact();
        assert!(row.packed.is_none());
        assert_eq!(row.cells.len(), 120);
    }
}
