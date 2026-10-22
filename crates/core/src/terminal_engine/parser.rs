//! A bounded, streaming parser for UTF-8 terminal output.
//!
//! Parsing text and CSI sequences allocates nothing. String payloads share one
//! reusable buffer and are delivered only after a complete terminator. The
//! generic string limit is 64 KiB. Kitty graphics APCs and OSC 52 clipboard
//! writes, chosen by their first bytes, get their own larger bounds. OSC 1337
//! inline images share the APC bound; the 3-byte prefix that selects it also
//! covers OSC 133, whose small prompt marks never approach it.

const MAX_PARAMS: usize = 32;
const MAX_SUBPARAMS: usize = 8;
const MAX_INTERMEDIATES: usize = 2;
pub(super) const MAX_STRING_BYTES: usize = 64 * 1024;
pub(super) const MAX_APC_BYTES: usize = 256 * 1024 * 1024;
// Base64 for about 6 MiB of copied text, enough for large editor yanks.
pub(super) const MAX_CLIPBOARD_OSC_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Param {
    values: [Option<u16>; MAX_SUBPARAMS + 1],
    len: u8,
}

impl Param {
    const EMPTY: Self = Self {
        values: [None; MAX_SUBPARAMS + 1],
        len: 1,
    };

    pub(super) fn value(&self) -> Option<u16> {
        self.values[0]
    }

    /// Colon-separated values after the primary value, including omissions.
    pub(super) fn subparams(&self) -> &[Option<u16>] {
        &self.values[1..usize::from(self.len)]
    }

    fn digit(&mut self, digit: u8) {
        let value = &mut self.values[usize::from(self.len) - 1];
        *value = Some(
            value
                .unwrap_or(0)
                .saturating_mul(10)
                .saturating_add(u16::from(digit)),
        );
    }

    fn next_subparam(&mut self) -> bool {
        if usize::from(self.len) == self.values.len() {
            return false;
        }
        self.values[usize::from(self.len)] = None;
        self.len += 1;
        true
    }
}

/// Length of the printable ASCII (`0x20..=0x7e`) prefix of `bytes`, tested
/// eight bytes at a time: ordinary output is long runs of it.
#[inline]
fn printable_ascii_len(bytes: &[u8]) -> usize {
    const ONES: u64 = 0x0101_0101_0101_0101;
    const HIGH: u64 = 0x8080_8080_8080_8080;
    let (blocks, rest) = bytes.as_chunks::<16>();
    let mut len = 0;
    for block in blocks {
        if let Some(stop) = first_unprintable(block) {
            return len + stop;
        }
        len += 16;
    }
    let (chunks, remainder) = rest.as_chunks::<8>();
    for chunk in chunks {
        let word = u64::from_le_bytes(*chunk);
        // Per byte, neither sum carries into the next byte. A high bit is set
        // where the byte is 0x7f or above (any non-ASCII byte included) ...
        let low = word & !HIGH;
        let from_delete = (low + ONES) | word;
        // ... or below 0x20, where adding 0x60 to the low seven bits stays
        // under 0x80.
        let below_space = !((low + 0x60 * ONES) | word);
        let stop = (from_delete | below_space) & HIGH;
        if stop != 0 {
            return len + (stop.trailing_zeros() / 8) as usize;
        }
        len += 8;
    }
    len + remainder
        .iter()
        .take_while(|byte| (0x20..=0x7e).contains(*byte))
        .count()
}

/// The index of the first byte of `block` outside `0x20..=0x7e`, with the
/// baseline vector instructions of the target (NEON, SSE2).
#[inline(always)]
fn first_unprintable(block: &[u8; 16]) -> Option<usize> {
    #[cfg(target_arch = "aarch64")]
    {
        use std::arch::aarch64::{
            vcltq_u8, vdupq_n_u8, vget_lane_u64, vld1q_u8, vminvq_u8, vmvnq_u8,
            vreinterpret_u64_u8, vreinterpretq_u16_u8, vshrn_n_u16, vsubq_u8,
        };
        // SAFETY: NEON is part of the aarch64 baseline, and the load reads
        // exactly the 16 bytes of `block`.
        unsafe {
            let bytes = vld1q_u8(block.as_ptr());
            let printable = vcltq_u8(vsubq_u8(bytes, vdupq_n_u8(0x20)), vdupq_n_u8(0x5f));
            if vminvq_u8(printable) == u8::MAX {
                return None;
            }
            // Narrow to four mask bits per byte to find the first stop.
            let stops = vreinterpretq_u16_u8(vmvnq_u8(printable));
            let mask = vget_lane_u64::<0>(vreinterpret_u64_u8(vshrn_n_u16::<4>(stops)));
            Some((mask.trailing_zeros() / 4) as usize)
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::{
            __m128i, _mm_and_si128, _mm_cmpgt_epi8, _mm_cmplt_epi8, _mm_loadu_si128,
            _mm_movemask_epi8, _mm_set1_epi8,
        };
        // SAFETY: SSE2 is part of the x86_64 baseline, and the unaligned load
        // reads exactly the 16 bytes of `block`.
        unsafe {
            let bytes = _mm_loadu_si128(block.as_ptr().cast::<__m128i>());
            // As signed bytes, 0x20..=0x7e is 32..=126 and every byte from
            // 0x80 is negative.
            let printable = _mm_and_si128(
                _mm_cmpgt_epi8(bytes, _mm_set1_epi8(0x1f)),
                _mm_cmplt_epi8(bytes, _mm_set1_epi8(0x7f)),
            );
            let mask = _mm_movemask_epi8(printable) as u32;
            (mask != 0xffff).then(|| (!mask).trailing_zeros() as usize)
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        block.iter().position(|byte| !(0x20..=0x7e).contains(byte))
    }
}

pub(super) trait Handler {
    fn print(&mut self, character: char);

    /// Called only with printable ASCII, never UTF-8 or control bytes.
    fn print_ascii(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.print(char::from(byte));
        }
    }

    /// Print a leading run of `bytes`, which starts with a non-ASCII byte,
    /// and return how many bytes it consumed. Only complete scalars and
    /// printable ASCII may be consumed, each exactly as `print` would; the
    /// parser handles everything else, so consuming nothing is always valid.
    fn print_utf8(&mut self, _bytes: &[u8]) -> usize {
        0
    }

    fn execute(&mut self, byte: u8);
    fn escape(&mut self, intermediates: &[u8], final_byte: u8);
    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8);
    fn osc(&mut self, bytes: &[u8]);
    fn osc_terminated(&mut self, bytes: &[u8], _bell: bool) {
        self.osc(bytes);
    }
    /// Like `osc_terminated`, but large payloads (OSC 1337 inline images)
    /// may be taken with `mem::take` instead of copied.
    fn osc_owned(&mut self, bytes: &mut Vec<u8>, bell: bool) {
        self.osc_terminated(bytes, bell);
    }
    fn dcs(&mut self, bytes: &[u8]);
    /// Large graphics payloads may be taken with `mem::take` instead of copied.
    fn apc(&mut self, bytes: &mut Vec<u8>);

    /// Stop at a completed callback without consuming the following bytes.
    fn pause_requested(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi,
    String(StringKind),
    StringEscape(StringKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StringKind {
    Osc,
    Dcs,
    Apc,
    Ignore,
}

pub(super) struct Parser {
    state: State,
    params: [Param; MAX_PARAMS],
    param_len: usize,
    private: Option<u8>,
    intermediates: [u8; MAX_INTERMEDIATES],
    intermediate_len: usize,
    discarded: bool,
    string: Vec<u8>,
    string_len: usize,
    string_wire_len: usize,
    collect_strings: bool,
    apc_limit: usize,
    // OSC and DCS payloads, and OSC 52's; see `set_string_limits`.
    string_limit: usize,
    clipboard_limit: usize,
    // The current string's first bytes select its limit, even for scanners
    // that do not collect it. Bytes past `string_len` are stale.
    prefix: [u8; 3],
    utf8_value: u32,
    utf8_remaining: u8,
    utf8_min: u8,
    utf8_max: u8,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            state: State::Ground,
            params: [Param::EMPTY; MAX_PARAMS],
            param_len: 0,
            private: None,
            intermediates: [0; MAX_INTERMEDIATES],
            intermediate_len: 0,
            discarded: false,
            string: Vec::new(),
            string_len: 0,
            string_wire_len: 0,
            collect_strings: true,
            apc_limit: MAX_STRING_BYTES,
            string_limit: MAX_STRING_BYTES,
            clipboard_limit: MAX_CLIPBOARD_OSC_BYTES,
            prefix: [0; 3],
            utf8_value: 0,
            utf8_remaining: 0,
            utf8_min: 0x80,
            utf8_max: 0xbf,
        }
    }
}

impl Parser {
    /// Graphics protocols may require a larger APC payload than ordinary
    /// terminal strings. OSC and DCS retain their independent 64 KiB bound.
    pub(super) fn with_apc_limit(limit: usize) -> Self {
        Self {
            apc_limit: limit.clamp(1, MAX_APC_BYTES),
            ..Self::default()
        }
    }

    /// Lower the payload limits: OSC and DCS strings longer than `strings`
    /// bytes, and OSC 52 strings longer than `clipboard`, are dropped whole.
    /// Each is clamped to `1..=` its default (64 KiB and 8 MiB).
    pub(super) fn set_string_limits(&mut self, strings: usize, clipboard: usize) {
        self.string_limit = strings.clamp(1, MAX_STRING_BYTES);
        self.clipboard_limit = clipboard.clamp(1, MAX_CLIPBOARD_OSC_BYTES);
    }

    pub(super) fn string_limits(&self) -> (usize, usize) {
        (self.string_limit, self.clipboard_limit)
    }

    /// Recognize identical string boundaries without retaining their payloads.
    /// String callbacks receive empty slices; marker scanners ignore them.
    pub(super) fn scanner(apc_limit: usize) -> Self {
        Self {
            collect_strings: false,
            ..Self::with_apc_limit(apc_limit)
        }
    }

    /// Reset protocol state while retaining ordinary-sized string allocations.
    pub(super) fn reset(&mut self) {
        self.state = State::Ground;
        self.param_len = 0;
        self.private = None;
        self.intermediate_len = 0;
        self.discarded = false;
        self.clear_string();
        self.utf8_remaining = 0;
    }

    /// Between sequences and scalars, where a printable ASCII byte is
    /// printed and nothing else.
    #[inline]
    pub(super) fn at_ground(&self) -> bool {
        self.state == State::Ground && self.utf8_remaining == 0
    }

    pub(super) fn advance(&mut self, handler: &mut impl Handler, bytes: &[u8]) -> usize {
        self.advance_impl::<true>(handler, bytes)
    }

    /// Replay an already bounded synchronized batch without pausing on nested
    /// begin markers. Handler state changes still occur in their normal order.
    pub(super) fn advance_uninterrupted(
        &mut self,
        handler: &mut impl Handler,
        bytes: &[u8],
    ) -> usize {
        self.advance_impl::<false>(handler, bytes)
    }

    fn advance_impl<const PAUSABLE: bool>(
        &mut self,
        handler: &mut impl Handler,
        bytes: &[u8],
    ) -> usize {
        let mut offset = 0;
        while offset < bytes.len() {
            if self.state == State::Ground && self.utf8_remaining == 0 {
                let start = offset;
                offset += printable_ascii_len(&bytes[offset..]);
                if offset != start {
                    handler.print_ascii(&bytes[start..offset]);
                    if PAUSABLE && handler.pause_requested() {
                        break;
                    }
                    continue;
                }
                // A complete scalar in this chunk needs one dispatch, rather
                // than revisiting the VT state machine for each continuation.
                // Invalid and fragmented sequences keep the streaming path's
                // replacement/reconsumption behavior below.
                // Line endings and the other C0 controls but ESC execute
                // directly, as `ground` would.
                // A control sequence introducer opens most escapes, and calls
                // no handler: both bytes are taken at once.
                if bytes[offset] == 0x1b && bytes.get(offset + 1) == Some(&b'[') {
                    self.begin_escape();
                    self.state = State::Csi;
                    offset += 2;
                    continue;
                }
                if bytes[offset] < 0x20 && bytes[offset] != 0x1b {
                    handler.execute(bytes[offset]);
                    offset += 1;
                    // A line ending's line feed executes here, as the next
                    // pass would: a carriage return requests no pause.
                    if bytes[offset - 1] == b'\r' && bytes.get(offset) == Some(&b'\n') {
                        handler.execute(b'\n');
                        offset += 1;
                    }
                    if PAUSABLE && handler.pause_requested() {
                        break;
                    }
                    continue;
                }
                if bytes[offset] >= 0xc2 {
                    // The handler may write a whole run of text, decoding it
                    // as it goes. Printing never requests a pause, so one
                    // check after the run matches checking per scalar.
                    let written = handler.print_utf8(&bytes[offset..]);
                    if written != 0 {
                        offset += written;
                        if PAUSABLE && handler.pause_requested() {
                            break;
                        }
                        continue;
                    }
                    if let Some((character, len)) = complete_utf8(&bytes[offset..]) {
                        self.print_scalar(handler, character);
                        offset += len;
                        if PAUSABLE && handler.pause_requested() {
                            break;
                        }
                        continue;
                    }
                }
            } else if self.state == State::Csi && !self.discarded && self.intermediate_len == 0 {
                // Parameters are most of a CSI sequence's bytes; take their
                // run here instead of dispatching each through the state
                // machine. Anything else but the final byte does.
                let start = offset;
                while let Some(&byte) = bytes.get(offset) {
                    match byte {
                        b'0'..=b'9' => {
                            self.ensure_param();
                            self.params[self.param_len - 1].digit(byte - b'0');
                        }
                        b';' => {
                            self.ensure_param();
                            if self.param_len == MAX_PARAMS {
                                self.discarded = true;
                                offset += 1;
                                break;
                            }
                            self.params[self.param_len] = Param::EMPTY;
                            self.param_len += 1;
                        }
                        _ => break,
                    }
                    offset += 1;
                }
                // So does the final byte, as `csi_byte` would.
                if !self.discarded
                    && let Some(&byte @ 0x40..=0x7e) = bytes.get(offset)
                {
                    self.state = State::Ground;
                    handler.csi(&self.params[..self.param_len], self.private, &[], byte);
                    offset += 1;
                    if PAUSABLE && handler.pause_requested() {
                        break;
                    }
                    continue;
                }
                if offset != start {
                    continue;
                }
            } else if let State::String(kind) = self.state {
                let start = offset;
                while offset < bytes.len() && !Self::string_control(kind, bytes[offset]) {
                    offset += 1;
                }
                if offset != start {
                    self.append_string(kind, &bytes[start..offset]);
                    continue;
                }
            }

            self.advance_byte(handler, bytes[offset]);
            offset += 1;
            if PAUSABLE && handler.pause_requested() {
                break;
            }
        }
        offset
    }

    fn advance_byte(&mut self, handler: &mut impl Handler, byte: u8) {
        match self.state {
            State::Ground => self.ground(handler, byte),
            State::String(kind) => match byte {
                0x18 | 0x1a => self.cancel(),
                0x1b => self.state = State::StringEscape(kind),
                0x07 if kind == StringKind::Osc => self.finish_string(handler, kind, true),
                0x00..=0x1f | 0x7f if kind == StringKind::Osc => {
                    self.string_wire_len = self.string_wire_len.saturating_add(1);
                }
                _ => self.append_string(kind, &[byte]),
            },
            State::StringEscape(kind) => {
                if byte == b'\\' {
                    self.finish_string(handler, kind, false);
                } else if self.discarded && !matches!(byte, b'c' | b'[') {
                    // Keep oversized payloads isolated until termination or
                    // explicit cancellation, including embedded escapes. A
                    // full reset (ESC c, sent by `reset`) or a CSI still
                    // recovers: graphics and clipboard payloads never contain
                    // ESC [, but the text after a stray `ESC P` usually does.
                    self.state = State::String(kind);
                    if Self::string_control(kind, byte) {
                        self.advance_byte(handler, byte);
                    }
                } else {
                    // ESC interrupts an unfinished string. Reconsume the byte
                    // as part of the new escape sequence, including C0 and CSI.
                    self.interrupt_string(handler, byte);
                }
            }
            State::Escape | State::Csi => self.sequence_byte(handler, byte),
        }
    }

    fn ground(&mut self, handler: &mut impl Handler, byte: u8) {
        if self.utf8_remaining != 0 {
            if (self.utf8_min..=self.utf8_max).contains(&byte) {
                self.utf8_value = (self.utf8_value << 6) | u32::from(byte & 0x3f);
                self.utf8_remaining -= 1;
                self.utf8_min = 0x80;
                self.utf8_max = 0xbf;
                if self.utf8_remaining == 0 {
                    // Lead/continuation restrictions exclude surrogates,
                    // overlong encodings, and values greater than U+10FFFF.
                    self.print_scalar(
                        handler,
                        char::from_u32(self.utf8_value).unwrap_or(char::REPLACEMENT_CHARACTER),
                    );
                }
                return;
            }
            self.utf8_remaining = 0;
            handler.print(char::REPLACEMENT_CHARACTER);
            // An invalid continuation may itself start UTF-8 or an escape.
        }

        match byte {
            0x1b => self.begin_escape(),
            0x00..=0x1f => handler.execute(byte),
            0x20..=0x7e => handler.print(char::from(byte)),
            0x7f => {}
            0xc2..=0xdf => self.begin_utf8(byte & 0x1f, 1, 0x80, 0xbf),
            0xe0 => self.begin_utf8(byte & 0x0f, 2, 0xa0, 0xbf),
            0xe1..=0xec | 0xee..=0xef => self.begin_utf8(byte & 0x0f, 2, 0x80, 0xbf),
            0xed => self.begin_utf8(byte & 0x0f, 2, 0x80, 0x9f),
            0xf0 => self.begin_utf8(byte & 0x07, 3, 0x90, 0xbf),
            0xf1..=0xf3 => self.begin_utf8(byte & 0x07, 3, 0x80, 0xbf),
            0xf4 => self.begin_utf8(byte & 0x07, 3, 0x80, 0x8f),
            _ => handler.print(char::REPLACEMENT_CHARACTER),
        }
    }

    /// A decoded scalar. C1 controls sent as UTF-8 (U+0080 to U+009F) act
    /// as their 7-bit forms, ESC and the byte 0x40 below, as in xterm: U+009B
    /// opens a CSI sequence. Raw 8-bit C1 bytes are not UTF-8 and print U+FFFD.
    #[inline(always)]
    fn print_scalar(&mut self, handler: &mut impl Handler, character: char) {
        if matches!(character, '\u{80}'..='\u{9f}') {
            self.c1_control(handler, character as u8);
        } else {
            handler.print(character);
        }
    }

    #[cold]
    #[inline(never)]
    fn c1_control(&mut self, handler: &mut impl Handler, code: u8) {
        self.begin_escape();
        self.escape_byte(handler, code - 0x40);
    }

    fn begin_utf8(&mut self, value: u8, remaining: u8, min: u8, max: u8) {
        self.utf8_value = u32::from(value);
        self.utf8_remaining = remaining;
        self.utf8_min = min;
        self.utf8_max = max;
    }

    fn begin_escape(&mut self) {
        self.state = State::Escape;
        self.param_len = 0;
        self.private = None;
        self.intermediate_len = 0;
        self.discarded = false;
    }

    fn sequence_byte(&mut self, handler: &mut impl Handler, byte: u8) {
        match byte {
            0x18 | 0x1a => self.cancel(),
            0x1b => self.begin_escape(),
            0x00..=0x1f => {
                if !self.discarded {
                    handler.execute(byte);
                }
            }
            0x7f => {}
            _ if self.state == State::Escape => self.escape_byte(handler, byte),
            _ => self.csi_byte(handler, byte),
        }
    }

    fn escape_byte(&mut self, handler: &mut impl Handler, byte: u8) {
        match byte {
            0x20..=0x2f => self.intermediate(byte),
            0x30..=0x7e => {
                self.state = State::Ground;
                if self.discarded {
                    return;
                }
                if self.intermediate_len == 0 {
                    let string_kind = match byte {
                        b'[' => {
                            self.state = State::Csi;
                            return;
                        }
                        b']' => Some(StringKind::Osc),
                        b'P' => Some(StringKind::Dcs),
                        b'_' => Some(StringKind::Apc),
                        b'X' | b'^' => Some(StringKind::Ignore),
                        _ => None,
                    };
                    if let Some(kind) = string_kind {
                        self.begin_string(kind);
                        return;
                    }
                }
                handler.escape(&self.intermediates[..self.intermediate_len], byte);
            }
            // No escape sequence contains a non-ASCII byte. Abandon it and
            // print the text instead of discarding it up to some final byte.
            _ => {
                self.state = State::Ground;
                self.ground(handler, byte);
            }
        }
    }

    fn csi_byte(&mut self, handler: &mut impl Handler, byte: u8) {
        if (0x40..=0x7e).contains(&byte) {
            self.state = State::Ground;
            if !self.discarded {
                handler.csi(
                    &self.params[..self.param_len],
                    self.private,
                    &self.intermediates[..self.intermediate_len],
                    byte,
                );
            }
            return;
        }
        if self.discarded {
            return;
        }
        match byte {
            0x20..=0x2f => self.intermediate(byte),
            b'0'..=b'9' if self.intermediate_len == 0 => {
                self.ensure_param();
                self.params[self.param_len - 1].digit(byte - b'0');
            }
            b';' if self.intermediate_len == 0 => {
                self.ensure_param();
                if self.param_len == MAX_PARAMS {
                    self.discarded = true;
                } else {
                    self.params[self.param_len] = Param::EMPTY;
                    self.param_len += 1;
                }
            }
            b':' if self.intermediate_len == 0 => {
                self.ensure_param();
                if !self.params[self.param_len - 1].next_subparam() {
                    self.discarded = true;
                }
            }
            0x3c..=0x3f
                if self.param_len == 0 && self.private.is_none() && self.intermediate_len == 0 =>
            {
                self.private = Some(byte);
            }
            _ => self.discarded = true,
        }
    }

    fn ensure_param(&mut self) {
        if self.param_len == 0 {
            self.params[0] = Param::EMPTY;
            self.param_len = 1;
        }
    }

    fn intermediate(&mut self, byte: u8) {
        if self.intermediate_len == MAX_INTERMEDIATES {
            self.discarded = true;
        } else {
            self.intermediates[self.intermediate_len] = byte;
            self.intermediate_len += 1;
        }
    }

    fn string_control(kind: StringKind, byte: u8) -> bool {
        matches!(byte, 0x18 | 0x1a | 0x1b)
            || (kind == StringKind::Osc && (byte < 0x20 || byte == 0x7f))
    }

    fn append_string(&mut self, kind: StringKind, bytes: &[u8]) {
        self.string_wire_len = self.string_wire_len.saturating_add(bytes.len());
        if self.discarded || kind == StringKind::Ignore {
            return;
        }
        if let Some(missing) = self.prefix.get_mut(self.string_len..) {
            let count = missing.len().min(bytes.len());
            missing[..count].copy_from_slice(&bytes[..count]);
        }
        // Until the prefix is complete the string is far below every limit,
        // so stale prefix bytes cannot affect the check.
        let limit = match kind {
            StringKind::Apc if self.prefix[0] == b'G' => self.apc_limit,
            StringKind::Osc if self.prefix == *b"52;" => self.clipboard_limit,
            StringKind::Osc if self.prefix == *b"133" => self.apc_limit,
            _ => self.string_limit,
        };
        if bytes.len() > limit.saturating_sub(self.string_len) {
            self.discarded = true;
            self.clear_string();
            return;
        }
        self.string_len += bytes.len();
        if !self.collect_strings {
            return;
        }
        let required = self.string_len;
        if required > self.string.capacity() {
            // Control geometric growth explicitly so a large chunk cannot
            // double capacity beyond the payload limit. A 1.5x step keeps
            // amortized appends without reserving up to twice the payload.
            let grown = self.string.capacity() + self.string.capacity() / 2;
            let capacity = required.max(grown).min(limit);
            self.string.reserve_exact(capacity - self.string.len());
        }
        self.string.extend_from_slice(bytes);
    }

    // Keep allocation cleanup and string dispatch out of the ordinary CSI
    // byte path, including the registers needed across their allocator calls.
    #[cold]
    #[inline(never)]
    fn begin_string(&mut self, kind: StringKind) {
        self.clear_string();
        self.state = State::String(kind);
    }

    #[cold]
    #[inline(never)]
    fn interrupt_string(&mut self, handler: &mut impl Handler, byte: u8) {
        self.clear_string();
        self.begin_escape();
        self.sequence_byte(handler, byte);
    }

    #[cold]
    #[inline(never)]
    fn finish_string(&mut self, handler: &mut impl Handler, kind: StringKind, bell: bool) {
        // OSC 7501 bounds the entire wire sequence, including ignored controls.
        // Conservatively count the longer ST terminator even when BEL was used.
        let oversized_status = kind == StringKind::Osc
            && self.string.starts_with(b"7501;")
            && self.string_wire_len > 4092;
        if !self.discarded && !oversized_status {
            match kind {
                StringKind::Osc => handler.osc_owned(&mut self.string, bell),
                StringKind::Dcs => handler.dcs(&self.string),
                StringKind::Apc => handler.apc(&mut self.string),
                StringKind::Ignore => {}
            }
        }
        self.clear_string();
        self.discarded = false;
        self.state = State::Ground;
    }

    #[cold]
    #[inline(never)]
    fn cancel(&mut self) {
        self.clear_string();
        self.discarded = false;
        self.state = State::Ground;
    }

    fn clear_string(&mut self) {
        self.string_len = 0;
        self.string_wire_len = 0;
        // Large APC transfers must not permanently raise every session's
        // retained heap. Ordinary OSC/DCS buffers still reuse their allocation.
        if self.string.capacity() > MAX_STRING_BYTES {
            self.release_large_string();
        } else {
            self.string.clear();
        }
    }

    #[cold]
    #[inline(never)]
    fn release_large_string(&mut self) {
        self.string = Vec::new();
    }
}

impl Parser {
    /// The in-progress sequence: a snapshot decoded mid-escape, mid-string
    /// or mid-scalar continues it exactly as this parser would.
    pub(super) fn write_snapshot(&self, writer: &mut super::snapshot::Writer) {
        let kind = |kind: StringKind| match kind {
            StringKind::Osc => 0,
            StringKind::Dcs => 1,
            StringKind::Apc => 2,
            StringKind::Ignore => 3,
        };
        match self.state {
            State::Ground => writer.u8(0),
            State::Escape => writer.u8(1),
            State::Csi => writer.u8(2),
            State::String(string) => {
                writer.u8(3);
                writer.u8(kind(string));
            }
            State::StringEscape(string) => {
                writer.u8(4);
                writer.u8(kind(string));
            }
        }
        // Parameters past `param_len` are rewritten before they are read.
        writer.usize(self.param_len);
        for param in &self.params[..self.param_len] {
            writer.u8(param.len);
            for value in &param.values[..usize::from(param.len)] {
                writer.varint(value.map_or(0, |value| u64::from(value) + 1));
            }
        }
        writer.u8(self.private.map_or(0, |_| 1));
        writer.u8(self.private.unwrap_or(0));
        writer.usize(self.intermediate_len);
        writer.u8(self.intermediates[0]);
        writer.u8(self.intermediates[1]);
        writer.bool(self.discarded);
        writer.bool(self.collect_strings);
        writer.usize(self.apc_limit);
        if writer.version() >= 2 {
            writer.usize(self.string_limit);
            writer.usize(self.clipboard_limit);
        }
        writer.usize(self.string_len);
        writer.usize(self.string_wire_len);
        writer.bytes(&self.string);
        writer.u8(self.prefix[0]);
        writer.u8(self.prefix[1]);
        writer.u8(self.prefix[2]);
        writer.varint(u64::from(self.utf8_value));
        writer.u8(self.utf8_remaining);
        writer.u8(self.utf8_min);
        writer.u8(self.utf8_max);
    }

    pub(super) fn read_snapshot(
        reader: &mut super::snapshot::Reader<'_>,
    ) -> super::snapshot::Result<Self> {
        use super::snapshot::SnapshotError::Corrupt;
        let kind = |reader: &mut super::snapshot::Reader<'_>| {
            Ok(match reader.u8()? {
                0 => StringKind::Osc,
                1 => StringKind::Dcs,
                2 => StringKind::Apc,
                3 => StringKind::Ignore,
                _ => return Err(Corrupt("parser string kind")),
            })
        };
        let state = match reader.u8()? {
            0 => State::Ground,
            1 => State::Escape,
            2 => State::Csi,
            3 => State::String(kind(reader)?),
            4 => State::StringEscape(kind(reader)?),
            _ => return Err(Corrupt("parser state")),
        };
        let mut parser = Self {
            state,
            ..Self::default()
        };
        parser.param_len = reader.usize()?;
        if parser.param_len > MAX_PARAMS {
            return Err(Corrupt("parser parameters"));
        }
        for param in &mut parser.params[..parser.param_len] {
            param.len = reader.u8()?;
            if param.len == 0 || usize::from(param.len) > MAX_SUBPARAMS + 1 {
                return Err(Corrupt("parser parameter"));
            }
            for value in &mut param.values[..usize::from(param.len)] {
                *value = match reader.varint()? {
                    0 => None,
                    value => {
                        Some(u16::try_from(value - 1).map_err(|_| Corrupt("parser parameter"))?)
                    }
                };
            }
        }
        let has_private = reader.bool()?;
        let private = reader.u8()?;
        parser.private = has_private.then_some(private);
        parser.intermediate_len = reader.usize()?;
        if parser.intermediate_len > MAX_INTERMEDIATES {
            return Err(Corrupt("parser intermediates"));
        }
        parser.intermediates = [reader.u8()?, reader.u8()?];
        parser.discarded = reader.bool()?;
        parser.collect_strings = reader.bool()?;
        parser.apc_limit = reader.usize()?.clamp(1, MAX_APC_BYTES);
        if reader.version() >= 2 {
            let strings = reader.usize()?;
            parser.set_string_limits(strings, reader.usize()?);
        }
        parser.string_len = reader.usize()?;
        parser.string_wire_len = reader.usize()?;
        parser.string = reader.bytes()?.to_vec();
        if parser.string_len > MAX_APC_BYTES
            || (parser.collect_strings && parser.string.len() != parser.string_len)
            || (!parser.collect_strings && !parser.string.is_empty())
        {
            return Err(Corrupt("parser string"));
        }
        parser.prefix = [reader.u8()?, reader.u8()?, reader.u8()?];
        parser.utf8_value =
            u32::try_from(reader.varint()?).map_err(|_| Corrupt("parser scalar"))?;
        parser.utf8_remaining = reader.u8()?;
        parser.utf8_min = reader.u8()?;
        parser.utf8_max = reader.u8()?;
        if parser.utf8_remaining > 3 {
            return Err(Corrupt("parser scalar"));
        }
        Ok(parser)
    }
}

/// Decode only a complete, valid non-ASCII scalar. The caller retains the
/// streaming decoder for all other input, including incomplete chunk tails.
#[inline(always)]
pub(super) fn complete_utf8(bytes: &[u8]) -> Option<(char, usize)> {
    let continuation = |byte: u8| byte & 0xc0 == 0x80;
    let (value, len) = match *bytes.first()? {
        first @ 0xc2..=0xdf => {
            let second = *bytes.get(1)?;
            if !continuation(second) {
                return None;
            }
            ((u32::from(first & 0x1f) << 6) | u32::from(second & 0x3f), 2)
        }
        first @ 0xe0..=0xef => {
            let [_, second, third, ..] = *bytes else {
                return None;
            };
            if !continuation(second)
                || !continuation(third)
                || (first == 0xe0 && second < 0xa0)
                || (first == 0xed && second >= 0xa0)
            {
                return None;
            }
            (
                (u32::from(first & 0x0f) << 12)
                    | (u32::from(second & 0x3f) << 6)
                    | u32::from(third & 0x3f),
                3,
            )
        }
        first @ 0xf0..=0xf4 => {
            let [_, second, third, fourth, ..] = *bytes else {
                return None;
            };
            if !continuation(second)
                || !continuation(third)
                || !continuation(fourth)
                || (first == 0xf0 && second < 0x90)
                || (first == 0xf4 && second >= 0x90)
            {
                return None;
            }
            (
                (u32::from(first & 7) << 18)
                    | (u32::from(second & 0x3f) << 12)
                    | (u32::from(third & 0x3f) << 6)
                    | u32::from(fourth & 0x3f),
                4,
            )
        }
        _ => return None,
    };
    // SAFETY: the lead and continuation ranges above exclude overlong
    // encodings, surrogates (ED A0..) and values above U+10FFFF (F4 90..),
    // so `value` is a Unicode scalar value.
    Some((unsafe { char::from_u32_unchecked(value) }, len))
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;
