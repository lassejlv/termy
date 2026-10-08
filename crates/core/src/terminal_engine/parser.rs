//! A bounded, streaming parser for UTF-8 terminal output.
//!
//! Parsing text and CSI sequences allocates nothing. String payloads share one
//! reusable buffer and are delivered only after a complete terminator. The
//! generic string limit is 64 KiB; protocols allowing larger transfers should
//! introduce a separately bounded streaming interface instead of removing it.

const MAX_PARAMS: usize = 32;
const MAX_SUBPARAMS: usize = 8;
const MAX_INTERMEDIATES: usize = 2;
const MAX_STRING_BYTES: usize = 64 * 1024;
pub(super) const MAX_APC_BYTES: usize = 256 * 1024 * 1024;

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

pub(super) trait Handler {
    fn print(&mut self, character: char);

    /// Called only with printable ASCII, never UTF-8 or control bytes.
    fn print_ascii(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.print(char::from(byte));
        }
    }

    fn execute(&mut self, byte: u8);
    fn escape(&mut self, intermediates: &[u8], final_byte: u8);
    fn csi(&mut self, params: &[Param], private: Option<u8>, intermediates: &[u8], final_byte: u8);
    fn osc(&mut self, bytes: &[u8]);
    fn osc_terminated(&mut self, bytes: &[u8], _bell: bool) {
        self.osc(bytes);
    }
    fn dcs(&mut self, bytes: &[u8]);
    fn apc(&mut self, bytes: &[u8]);

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
                while offset < bytes.len() && (0x20..=0x7e).contains(&bytes[offset]) {
                    offset += 1;
                }
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
                if bytes[offset] >= 0xc2
                    && let Some((character, len)) = complete_utf8(&bytes[offset..])
                {
                    handler.print(character);
                    offset += len;
                    if PAUSABLE && handler.pause_requested() {
                        break;
                    }
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
                } else if self.discarded {
                    // Keep oversized payloads isolated until termination or
                    // explicit cancellation, including embedded escapes.
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
                    handler.print(
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
            _ => self.discarded = true,
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
        let limit = if kind == StringKind::Apc {
            self.apc_limit
        } else {
            MAX_STRING_BYTES
        };
        if bytes.len() > limit - self.string_len {
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
            // double capacity beyond the payload limit.
            let capacity = required.next_power_of_two().min(limit);
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
                StringKind::Osc => handler.osc_terminated(&self.string, bell),
                StringKind::Dcs => handler.dcs(&self.string),
                StringKind::Apc => handler.apc(&self.string),
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

/// Decode only a complete, valid non-ASCII scalar. The caller retains the
/// streaming decoder for all other input, including incomplete chunk tails.
#[inline]
fn complete_utf8(bytes: &[u8]) -> Option<(char, usize)> {
    let (&first, rest) = bytes.split_first()?;
    let continuation = |byte: u8| byte & 0xc0 == 0x80;
    let (value, len) = match (first, rest) {
        (0xc2..=0xdf, &[second, ..]) if continuation(second) => {
            ((u32::from(first & 0x1f) << 6) | u32::from(second & 0x3f), 2)
        }
        (0xe0..=0xef, &[second, third, ..])
            if continuation(second)
                && continuation(third)
                && (first != 0xe0 || second >= 0xa0)
                && (first != 0xed || second < 0xa0) =>
        {
            (
                (u32::from(first & 0x0f) << 12)
                    | (u32::from(second & 0x3f) << 6)
                    | u32::from(third & 0x3f),
                3,
            )
        }
        (0xf0..=0xf4, &[second, third, fourth, ..])
            if continuation(second)
                && continuation(third)
                && continuation(fourth)
                && (first != 0xf0 || second >= 0x90)
                && (first != 0xf4 || second < 0x90) =>
        {
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
    char::from_u32(value).map(|character| (character, len))
}

#[cfg(test)]
#[path = "parser/tests.rs"]
mod tests;
