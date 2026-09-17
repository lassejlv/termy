use termy_core::{TerminalKeyEventKind, TerminalKeyboardMode, TermyKeystroke, TermyModifiers};

pub fn keystroke_to_input(
    keystroke: &gpui::Keystroke,
    event_kind: TerminalKeyEventKind,
    keyboard_mode: TerminalKeyboardMode,
    prompt_shortcuts_enabled: bool,
    macos_option_as_alt: bool,
) -> Option<Vec<u8>> {
    let keystroke = TermyKeystroke {
        modifiers: TermyModifiers {
            control: keystroke.modifiers.control,
            alt: keystroke.modifiers.alt,
            shift: keystroke.modifiers.shift,
            platform: keystroke.modifiers.platform,
            function: keystroke.modifiers.function,
        },
        key: keystroke.key.clone(),
        key_char: keystroke.key_char.clone(),
    };
    termy_core::keystroke_to_input_with_options(
        &keystroke,
        event_kind,
        keyboard_mode,
        prompt_shortcuts_enabled,
        macos_option_as_alt,
    )
}
