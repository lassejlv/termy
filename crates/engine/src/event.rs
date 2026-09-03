//! Events emitted by terminal control sequences.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DynamicColor {
    Foreground,
    Background,
    Cursor,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum MousePointerShape {
    #[default]
    Default,
    Pointer,
    Text,
    Crosshair,
    Move,
    NotAllowed,
    Help,
    Progress,
    Wait,
    Cell,
    VerticalText,
    Alias,
    Copy,
    NoDrop,
    Grab,
    Grabbing,
    EResize,
    NResize,
    NeResize,
    NwResize,
    SResize,
    SeResize,
    SwResize,
    WResize,
    EwResize,
    NsResize,
    NeswResize,
    NwseResize,
    ZoomIn,
    ZoomOut,
}

impl MousePointerShape {
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "default" => Self::Default,
            "pointer" => Self::Pointer,
            "text" => Self::Text,
            "crosshair" => Self::Crosshair,
            "move" => Self::Move,
            "not-allowed" => Self::NotAllowed,
            "help" => Self::Help,
            "progress" => Self::Progress,
            "wait" => Self::Wait,
            "cell" => Self::Cell,
            "vertical-text" => Self::VerticalText,
            "alias" => Self::Alias,
            "copy" => Self::Copy,
            "no-drop" => Self::NoDrop,
            "grab" => Self::Grab,
            "grabbing" => Self::Grabbing,
            "e-resize" => Self::EResize,
            "n-resize" => Self::NResize,
            "ne-resize" => Self::NeResize,
            "nw-resize" => Self::NwResize,
            "s-resize" => Self::SResize,
            "se-resize" => Self::SeResize,
            "sw-resize" => Self::SwResize,
            "w-resize" => Self::WResize,
            "ew-resize" => Self::EwResize,
            "ns-resize" => Self::NsResize,
            "nesw-resize" => Self::NeswResize,
            "nwse-resize" => Self::NwseResize,
            "zoom-in" => Self::ZoomIn,
            "zoom-out" => Self::ZoomOut,
            _ => return None,
        })
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Pointer => "pointer",
            Self::Text => "text",
            Self::Crosshair => "crosshair",
            Self::Move => "move",
            Self::NotAllowed => "not-allowed",
            Self::Help => "help",
            Self::Progress => "progress",
            Self::Wait => "wait",
            Self::Cell => "cell",
            Self::VerticalText => "vertical-text",
            Self::Alias => "alias",
            Self::Copy => "copy",
            Self::NoDrop => "no-drop",
            Self::Grab => "grab",
            Self::Grabbing => "grabbing",
            Self::EResize => "e-resize",
            Self::NResize => "n-resize",
            Self::NeResize => "ne-resize",
            Self::NwResize => "nw-resize",
            Self::SResize => "s-resize",
            Self::SeResize => "se-resize",
            Self::SwResize => "sw-resize",
            Self::WResize => "w-resize",
            Self::EwResize => "ew-resize",
            Self::NsResize => "ns-resize",
            Self::NeswResize => "nesw-resize",
            Self::NwseResize => "nwse-resize",
            Self::ZoomIn => "zoom-in",
            Self::ZoomOut => "zoom-out",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TerminalEvent {
    Bell,
    Title(String),
    ResetTitle,
    CurrentDirectory(String),
    ClipboardStore {
        selection: String,
        text: String,
    },
    SetDynamicColor {
        target: DynamicColor,
        color: [u8; 3],
    },
    ResetDynamicColor {
        target: DynamicColor,
    },
    MousePointerShape(MousePointerShape),
    Reply(Vec<u8>),
}
