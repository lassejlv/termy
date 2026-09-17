#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalMouseMode {
    pub enabled: bool,
    pub report_click: bool,
    pub report_drag: bool,
    pub report_motion: bool,
    pub sgr_encoding: bool,
    pub utf8_encoding: bool,
}

impl TerminalMouseMode {
    pub const fn can_report_drag(self) -> bool {
        self.report_drag || self.report_motion
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseEventKind {
    Press(TerminalMouseButton),
    Release(TerminalMouseButton),
    Drag(TerminalMouseButton),
    Move,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalMousePosition {
    pub col: usize,
    pub row: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalMouseModifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

fn modifiers_code(modifiers: TerminalMouseModifiers) -> u8 {
    let mut code = 0;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    code
}

fn base_button_code(event: TerminalMouseEventKind) -> u8 {
    match event {
        TerminalMouseEventKind::Press(button) => match button {
            TerminalMouseButton::Left => 0,
            TerminalMouseButton::Middle => 1,
            TerminalMouseButton::Right => 2,
        },
        TerminalMouseEventKind::Release(button) => match button {
            // Non-SGR release uses button 3; SGR release keeps original button.
            TerminalMouseButton::Left => 0,
            TerminalMouseButton::Middle => 1,
            TerminalMouseButton::Right => 2,
        },
        TerminalMouseEventKind::Drag(button) => match button {
            TerminalMouseButton::Left => 32,
            TerminalMouseButton::Middle => 33,
            TerminalMouseButton::Right => 34,
        },
        TerminalMouseEventKind::Move => 35,
        TerminalMouseEventKind::WheelUp => 64,
        TerminalMouseEventKind::WheelDown => 65,
        TerminalMouseEventKind::WheelLeft => 66,
        TerminalMouseEventKind::WheelRight => 67,
    }
}

fn utf8_coordinate_bytes(pos: usize) -> [u8; 2] {
    let pos = 32 + 1 + pos;
    let first = 0xC0 + pos / 64;
    let second = 0x80 + (pos & 63);
    [first as u8, second as u8]
}

fn normal_mouse_report(
    mode: TerminalMouseMode,
    event: TerminalMouseEventKind,
    position: TerminalMousePosition,
    modifiers: TerminalMouseModifiers,
) -> Option<Vec<u8>> {
    let utf8 = mode.utf8_encoding;
    let max_point = if utf8 { 2015 } else { 223 };
    if position.row >= max_point || position.col >= max_point {
        return None;
    }

    let modifiers = modifiers_code(modifiers);
    let base = base_button_code(event);
    let encoded_button = match event {
        TerminalMouseEventKind::Release(_) => 3 + modifiers,
        _ => base + modifiers,
    };

    let mut msg = vec![b'\x1b', b'[', b'M', 32 + encoded_button];

    if utf8 && position.col >= 95 {
        let bytes = utf8_coordinate_bytes(position.col);
        msg.extend_from_slice(&bytes);
    } else {
        msg.push(32 + 1 + position.col as u8);
    }

    if utf8 && position.row >= 95 {
        let bytes = utf8_coordinate_bytes(position.row);
        msg.extend_from_slice(&bytes);
    } else {
        msg.push(32 + 1 + position.row as u8);
    }

    Some(msg)
}

fn sgr_mouse_report(
    event: TerminalMouseEventKind,
    position: TerminalMousePosition,
    modifiers: TerminalMouseModifiers,
) -> Vec<u8> {
    let button = base_button_code(event) + modifiers_code(modifiers);
    let suffix = match event {
        TerminalMouseEventKind::Release(_) => 'm',
        _ => 'M',
    };
    format!(
        "\x1b[<{};{};{}{}",
        button,
        position.col + 1,
        position.row + 1,
        suffix
    )
    .into_bytes()
}

fn event_allowed(mode: TerminalMouseMode, event: TerminalMouseEventKind) -> bool {
    if !mode.enabled {
        return false;
    }

    match event {
        TerminalMouseEventKind::Move => mode.report_motion,
        TerminalMouseEventKind::Drag(_) => mode.can_report_drag(),
        TerminalMouseEventKind::Press(_)
        | TerminalMouseEventKind::Release(_)
        | TerminalMouseEventKind::WheelUp
        | TerminalMouseEventKind::WheelDown
        | TerminalMouseEventKind::WheelLeft
        | TerminalMouseEventKind::WheelRight => true,
    }
}

pub fn encode_mouse_report(
    mode: TerminalMouseMode,
    event: TerminalMouseEventKind,
    position: TerminalMousePosition,
    modifiers: TerminalMouseModifiers,
) -> Option<Vec<u8>> {
    if !event_allowed(mode, event) {
        return None;
    }

    if mode.sgr_encoding {
        Some(sgr_mouse_report(event, position, modifiers))
    } else {
        normal_mouse_report(mode, event, position, modifiers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode() -> TerminalMouseMode {
        TerminalMouseMode {
            enabled: true,
            report_click: true,
            report_drag: true,
            report_motion: true,
            sgr_encoding: true,
            utf8_encoding: false,
        }
    }

    #[test]
    fn sgr_press_encodes_expected_packet() {
        let bytes = encode_mouse_report(
            mode(),
            TerminalMouseEventKind::Press(TerminalMouseButton::Left),
            TerminalMousePosition { col: 4, row: 2 },
            TerminalMouseModifiers::default(),
        )
        .expect("packet");
        assert_eq!(bytes, b"\x1b[<0;5;3M");
    }

    #[test]
    fn sgr_right_press_encodes_expected_packet() {
        let bytes = encode_mouse_report(
            mode(),
            TerminalMouseEventKind::Press(TerminalMouseButton::Right),
            TerminalMousePosition { col: 1, row: 1 },
            TerminalMouseModifiers::default(),
        )
        .expect("packet");
        assert_eq!(bytes, b"\x1b[<2;2;2M");
    }

    #[test]
    fn sgr_release_uses_lowercase_suffix() {
        let bytes = encode_mouse_report(
            mode(),
            TerminalMouseEventKind::Release(TerminalMouseButton::Right),
            TerminalMousePosition { col: 1, row: 1 },
            TerminalMouseModifiers::default(),
        )
        .expect("packet");
        assert_eq!(bytes, b"\x1b[<2;2;2m");
    }

    #[test]
    fn sgr_drag_and_modifiers_add_expected_button_bits() {
        let bytes = encode_mouse_report(
            mode(),
            TerminalMouseEventKind::Drag(TerminalMouseButton::Middle),
            TerminalMousePosition { col: 0, row: 0 },
            TerminalMouseModifiers {
                shift: true,
                alt: true,
                control: false,
            },
        )
        .expect("packet");
        // 33 + (4 + 8) = 45
        assert_eq!(bytes, b"\x1b[<45;1;1M");
    }

    #[test]
    fn legacy_release_uses_button_three_code() {
        let bytes = encode_mouse_report(
            TerminalMouseMode {
                sgr_encoding: false,
                ..mode()
            },
            TerminalMouseEventKind::Release(TerminalMouseButton::Left),
            TerminalMousePosition { col: 5, row: 8 },
            TerminalMouseModifiers::default(),
        )
        .expect("packet");
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 35, 38, 41]);
    }

    #[test]
    fn utf8_encoding_supports_extended_coordinates() {
        let bytes = encode_mouse_report(
            TerminalMouseMode {
                sgr_encoding: false,
                utf8_encoding: true,
                ..mode()
            },
            TerminalMouseEventKind::Press(TerminalMouseButton::Left),
            TerminalMousePosition { col: 120, row: 130 },
            TerminalMouseModifiers::default(),
        )
        .expect("packet");
        assert_eq!(bytes[0..4], [0x1b, b'[', b'M', 32]);
        assert!(bytes.len() > 6);
    }

    #[test]
    fn legacy_encoding_rejects_out_of_range_coordinates() {
        let packet = encode_mouse_report(
            TerminalMouseMode {
                sgr_encoding: false,
                utf8_encoding: false,
                ..mode()
            },
            TerminalMouseEventKind::Press(TerminalMouseButton::Left),
            TerminalMousePosition { col: 300, row: 0 },
            TerminalMouseModifiers::default(),
        );
        assert!(packet.is_none());
    }

    #[test]
    fn motion_requires_motion_mode() {
        let packet = encode_mouse_report(
            TerminalMouseMode {
                report_motion: false,
                report_drag: false,
                ..mode()
            },
            TerminalMouseEventKind::Move,
            TerminalMousePosition { col: 0, row: 0 },
            TerminalMouseModifiers::default(),
        );
        assert!(packet.is_none());
    }

    #[test]
    fn disabled_mode_rejects_right_button_events() {
        for event in [
            TerminalMouseEventKind::Press(TerminalMouseButton::Right),
            TerminalMouseEventKind::Release(TerminalMouseButton::Right),
        ] {
            let packet = encode_mouse_report(
                TerminalMouseMode::default(),
                event,
                TerminalMousePosition { col: 0, row: 0 },
                TerminalMouseModifiers::default(),
            );
            assert!(packet.is_none());
        }
    }
}
