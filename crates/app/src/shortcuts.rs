#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Shortcut {
    Copy,
    Find,
    NewTab,
    CloseTab,
    NextMatch,
    NextTab,
    OpenConfig,
    Paste,
    PreviousTab,
    Quit,
    ResetFontSize,
    SelectTab(usize),
    ZoomIn,
    ZoomOut,
}

pub(crate) fn shortcut_for_character(character: &str) -> Option<Shortcut> {
    let shortcut = character.to_lowercase();
    Some(match shortcut.as_str() {
        "q" => Shortcut::Quit,
        "v" => Shortcut::Paste,
        "+" | "=" => Shortcut::ZoomIn,
        "-" | "_" => Shortcut::ZoomOut,
        "0" => Shortcut::ResetFontSize,
        "c" => Shortcut::Copy,
        "," => Shortcut::OpenConfig,
        "f" => Shortcut::Find,
        "g" => Shortcut::NextMatch,
        "t" => Shortcut::NewTab,
        "w" => Shortcut::CloseTab,
        "[" | "{" => Shortcut::PreviousTab,
        "]" | "}" => Shortcut::NextTab,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
            Shortcut::SelectTab(usize::from(shortcut.as_bytes()[0] - b'1'))
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::{Shortcut, shortcut_for_character};

    #[test]
    fn command_w_maps_to_close_tab() {
        assert_eq!(shortcut_for_character("w"), Some(Shortcut::CloseTab));
        assert_eq!(shortcut_for_character("W"), Some(Shortcut::CloseTab));
    }

    #[test]
    fn command_comma_maps_to_open_config() {
        assert_eq!(shortcut_for_character(","), Some(Shortcut::OpenConfig));
    }
}
