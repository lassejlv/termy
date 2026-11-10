//! Encoders: input events to the bytes a program expects. None of them
//! writes anywhere; hosts send the returned bytes to their transport.

use termy_core::terminal_engine::{Engine, MouseEncoding, MouseTracking};
use termy_core::{
    TerminalKeyEventKind, TermyKeystroke, TermyModifiers, keystroke_to_input_with_options,
};

use crate::{TermyEngine, TermyEngineBytes, TermyEngineStatus, guard, handle, input};

pub const MOD_SHIFT: u32 = 1;
pub const MOD_ALT: u32 = 1 << 1;
pub const MOD_CTRL: u32 = 1 << 2;
pub const MOD_SUPER: u32 = 1 << 3;
pub const MOD_LEFT_ALT: u32 = 1 << 8;
pub const MOD_RIGHT_ALT: u32 = 1 << 9;

pub const OPTION_AS_ALT_NONE: u32 = 0;
pub const OPTION_AS_ALT_LEFT: u32 = 1;
pub const OPTION_AS_ALT_RIGHT: u32 = 2;
pub const OPTION_AS_ALT_BOTH: u32 = 3;

pub const KEY_PRESS: u8 = 0;
pub const KEY_REPEAT: u8 = 1;
pub const KEY_RELEASE: u8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermyEngineKeyEvent {
    pub key_ptr: *const u8,
    pub key_len: usize,
    pub text_ptr: *const u8,
    pub text_len: usize,
    pub unshifted_codepoint: u32,
    pub mods: u32,
    pub consumed_mods: u32,
    pub action: u8,
    /// A C `bool`, read as a byte: any nonzero value is true, so a host that
    /// writes another byte cannot produce an invalid Rust `bool`.
    pub composing: u8,
}

pub const MOUSE_PRESS: u8 = 0;
pub const MOUSE_RELEASE: u8 = 1;
pub const MOUSE_MOTION: u8 = 2;

pub const MOUSE_BUTTON_NONE: u8 = 0;
pub const MOUSE_BUTTON_LEFT: u8 = 1;
pub const MOUSE_BUTTON_MIDDLE: u8 = 2;
pub const MOUSE_BUTTON_RIGHT: u8 = 3;
pub const MOUSE_WHEEL_UP: u8 = 4;
pub const MOUSE_WHEEL_DOWN: u8 = 5;
pub const MOUSE_WHEEL_LEFT: u8 = 6;
pub const MOUSE_WHEEL_RIGHT: u8 = 7;
pub const MOUSE_BUTTON_8: u8 = 8;
pub const MOUSE_BUTTON_11: u8 = 11;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct TermyEngineMouseEvent {
    pub action: u8,
    pub button: u8,
    /// A C `bool`, read as a byte; any nonzero value is true.
    pub any_button_pressed: u8,
    pub mods: u32,
    pub col: u32,
    pub row: u32,
    pub x_px: u32,
    pub y_px: u32,
}

// Mirrored by tests/c/replica.c, so the header and Rust agree on every field.
const _: () = {
    use std::mem::offset_of;
    let pointer = size_of::<usize>();
    assert!(offset_of!(TermyEngineKeyEvent, unshifted_codepoint) == 4 * pointer);
    assert!(offset_of!(TermyEngineKeyEvent, action) == 4 * pointer + 12);
    assert!(offset_of!(TermyEngineKeyEvent, composing) == 4 * pointer + 13);
    assert!(offset_of!(TermyEngineMouseEvent, any_button_pressed) == 2);
    assert!(offset_of!(TermyEngineMouseEvent, mods) == 4);
    assert!(offset_of!(TermyEngineMouseEvent, y_px) == 20);
    assert!(size_of::<TermyEngineMouseEvent>() == 24);
};

/// Encode a key event. `option_as_alt` says which Option keys act as Alt;
/// an Alt from the other side composes text instead (`consumed_mods`).
pub(crate) fn encode_key(
    engine: &Engine,
    event: &TermyEngineKeyEvent,
    key: &str,
    text: &str,
    option_as_alt: u32,
) -> Vec<u8> {
    if event.composing != 0 {
        return Vec::new();
    }
    let kind = match event.action {
        KEY_REPEAT => TerminalKeyEventKind::Repeat,
        KEY_RELEASE => TerminalKeyEventKind::Release,
        _ => TerminalKeyEventKind::Press,
    };
    let mode = engine.modes().keyboard_mode();
    let enhanced = mode.enhanced_reporting_active();
    let mut mods = event.mods;
    // An Alt that the host reports by side is Alt only on a configured side.
    let sides = mods & (MOD_LEFT_ALT | MOD_RIGHT_ALT);
    let alt_is_alt = sides == 0
        || (sides & MOD_LEFT_ALT != 0 && option_as_alt & OPTION_AS_ALT_LEFT != 0)
        || (sides & MOD_RIGHT_ALT != 0 && option_as_alt & OPTION_AS_ALT_RIGHT != 0);
    if mods & MOD_ALT != 0 && !alt_is_alt {
        // Option composed the text; it is not a modifier for the program.
        mods &= !MOD_ALT;
    } else if !enhanced {
        // Legacy encoding sends composed text as is: drop the modifiers the
        // layout consumed to produce it.
        if !text.is_empty() {
            mods &= !(event.consumed_mods & !MOD_ALT);
        }
    }
    let alt = mods & MOD_ALT != 0;
    let control = mods & MOD_CTRL != 0;
    let shift = mods & MOD_SHIFT != 0;
    let platform = mods & MOD_SUPER != 0;

    let base_key = if key.is_empty() {
        char::from_u32(event.unshifted_codepoint)
            .filter(|_| event.unshifted_codepoint != 0)
            .map(|c| c.to_lowercase().collect::<String>())
            .unwrap_or_default()
    } else {
        key.to_owned()
    };
    if base_key.is_empty() && text.is_empty() {
        return Vec::new();
    }

    // Legacy Alt+character is ESC followed by the character.
    if alt
        && !enhanced
        && !control
        && !platform
        && kind != TerminalKeyEventKind::Release
        && (base_key == "space" || base_key.chars().count() == 1)
    {
        let character = if base_key == "space" {
            " ".to_owned()
        } else if shift {
            base_key.to_uppercase()
        } else {
            base_key
        };
        let mut out = Vec::with_capacity(1 + character.len());
        out.push(0x1b);
        out.extend_from_slice(character.as_bytes());
        return out;
    }

    let keystroke = TermyKeystroke {
        modifiers: TermyModifiers {
            control,
            alt,
            shift,
            platform,
            function: false,
        },
        key: if base_key.is_empty() {
            text.to_lowercase()
        } else {
            base_key
        },
        key_char: (!text.is_empty()).then(|| text.to_owned()),
    };
    keystroke_to_input_with_options(&keystroke, kind, mode, false, false).unwrap_or_default()
}

/// xterm mouse reporting for every tracking mode and encoding.
pub(crate) fn encode_mouse(engine: &Engine, event: &TermyEngineMouseEvent) -> Vec<u8> {
    let modes = engine.modes();
    let tracking = modes.mouse_tracking;
    if tracking == MouseTracking::None || event.button > MOUSE_BUTTON_11 {
        return Vec::new();
    }
    let wheel = (MOUSE_WHEEL_UP..=MOUSE_WHEEL_RIGHT).contains(&event.button);
    let reported = match event.action {
        MOUSE_PRESS => event.button != MOUSE_BUTTON_NONE,
        // Wheels have no release; X10 reports presses only.
        MOUSE_RELEASE => !wheel && tracking != MouseTracking::Press,
        MOUSE_MOTION => match tracking {
            MouseTracking::Drag => event.any_button_pressed != 0,
            MouseTracking::Motion => true,
            _ => false,
        },
        _ => false,
    };
    if !reported {
        return Vec::new();
    }

    let button_code: u32 = match event.button {
        MOUSE_BUTTON_LEFT => 0,
        MOUSE_BUTTON_MIDDLE => 1,
        MOUSE_BUTTON_RIGHT => 2,
        MOUSE_BUTTON_NONE => 3,
        // Wheel 4-7 are 64-67; extra buttons 8-11 are 128-131.
        button if wheel => 64 + u32::from(button - MOUSE_WHEEL_UP),
        button => 128 + u32::from(button - MOUSE_BUTTON_8),
    };
    let sgr = matches!(
        modes.mouse_encoding,
        MouseEncoding::Sgr | MouseEncoding::SgrPixels
    );
    let mut code = if event.action == MOUSE_RELEASE && !sgr {
        3
    } else {
        button_code
    };
    if event.action == MOUSE_MOTION {
        code += 32;
    }
    if tracking != MouseTracking::Press {
        if event.mods & MOD_SHIFT != 0 {
            code += 4;
        }
        if event.mods & MOD_ALT != 0 {
            code += 8;
        }
        if event.mods & MOD_CTRL != 0 {
            code += 16;
        }
    }

    match modes.mouse_encoding {
        MouseEncoding::Sgr | MouseEncoding::SgrPixels => {
            let (x, y) = if modes.mouse_encoding == MouseEncoding::SgrPixels {
                (event.x_px, event.y_px)
            } else {
                (event.col, event.row)
            };
            let terminator = if event.action == MOUSE_RELEASE {
                'm'
            } else {
                'M'
            };
            format!(
                "\x1b[<{code};{};{}{terminator}",
                u64::from(x) + 1,
                u64::from(y) + 1
            )
            .into_bytes()
        }
        MouseEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            // Saturate so huge positions are skipped below rather than wrapping.
            let position = [event.col.saturating_add(33), event.row.saturating_add(33)];
            for value in [code + 32, position[0], position[1]] {
                // UTF-8 extended coordinates stop at 2047.
                let Some(character) = char::from_u32(value).filter(|_| value < 2048) else {
                    return Vec::new();
                };
                let mut buffer = [0; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            out
        }
        MouseEncoding::Default => {
            let values = [
                code + 32,
                event.col.saturating_add(33),
                event.row.saturating_add(33),
            ];
            if values.iter().any(|value| *value > 255) {
                return Vec::new();
            }
            let mut out = b"\x1b[M".to_vec();
            out.extend(values.iter().map(|value| *value as u8));
            out
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_encode_key(
    engine: *mut TermyEngine,
    event: *const TermyEngineKeyEvent,
    option_as_alt: u32,
    out_bytes: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or valid structs.
        let (Some(event), Some(out)) = (unsafe { event.as_ref() }, unsafe { out_bytes.as_mut() })
        else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: the event's slices are valid for their lengths.
        let (Some(key), Some(text)) = (unsafe { input(event.key_ptr, event.key_len) }, unsafe {
            input(event.text_ptr, event.text_len)
        }) else {
            return TermyEngineStatus::Null;
        };
        let (Ok(key), Ok(text)) = (std::str::from_utf8(key), std::str::from_utf8(text)) else {
            return TermyEngineStatus::InvalidArgument;
        };
        if event.action > KEY_RELEASE || option_as_alt > OPTION_AS_ALT_BOTH {
            return TermyEngineStatus::InvalidArgument;
        }
        *out = TermyEngineBytes::from_vec(encode_key(
            &engine.engine,
            event,
            &key.to_ascii_lowercase(),
            text,
            option_as_alt,
        ));
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_encode_mouse(
    engine: *mut TermyEngine,
    event: *const TermyEngineMouseEvent,
    out_bytes: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a live handle.
        let Some(engine) = (unsafe { handle(engine) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or valid structs.
        let (Some(event), Some(out)) = (unsafe { event.as_ref() }, unsafe { out_bytes.as_mut() })
        else {
            return TermyEngineStatus::Null;
        };
        if event.action > MOUSE_MOTION || event.button > MOUSE_BUTTON_11 {
            return TermyEngineStatus::InvalidArgument;
        }
        *out = TermyEngineBytes::from_vec(encode_mouse(&engine.engine, event));
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_encode_paste(
    text_ptr: *const u8,
    text_len: usize,
    bracketed: bool,
    out_bytes: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: valid for `text_len` bytes per the header contract.
        let Some(text) = (unsafe { input(text_ptr, text_len) }) else {
            return TermyEngineStatus::Null;
        };
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_bytes.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        *out = TermyEngineBytes::from_vec(termy_core::encode_paste(
            &String::from_utf8_lossy(text),
            bracketed,
        ));
        TermyEngineStatus::Ok
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn termy_engine_encode_focus(
    gained: bool,
    out_bytes: *mut TermyEngineBytes,
) -> TermyEngineStatus {
    guard(|| {
        // SAFETY: null or a writable buffer struct.
        let Some(out) = (unsafe { out_bytes.as_mut() }) else {
            return TermyEngineStatus::Null;
        };
        let bytes: &[u8] = if gained { b"\x1b[I" } else { b"\x1b[O" };
        *out = TermyEngineBytes::from_vec(bytes.to_vec());
        TermyEngineStatus::Ok
    })
}
