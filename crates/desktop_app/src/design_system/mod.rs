//! Termy's settings design system, in GPUI.
//!
//! This module owns the visual language of Termy's chrome — surfaces, controls,
//! rows, and status affordances — as stateless components. It knows nothing
//! about config keys, plugins, or SSH hosts; product surfaces pass values in
//! and get elements back.
//!
//! ```no_run
//! use termy::design_system::{Button, SettingRow, SettingsGroup, Switch, theme};
//! use gpui::{App, ParentElement as _};
//!
//! fn appearance_group(_cx: &mut App) -> SettingsGroup {
//!     SettingsGroup::new("THEME").child(
//!         SettingRow::new("Increase Chrome Contrast")
//!             .description("Increase contrast of non-terminal UI surfaces")
//!             .control(Switch::new("chrome-contrast", true)),
//!     )
//! }
//! ```
//!
//! Hosts install colors once with [`theme::set_tokens`] and register
//! [`icon::Assets`] (or delegate to [`icon::icon_bytes`] from their own asset
//! source) so the icon set resolves.

pub mod controls;
pub mod feedback;
pub mod icon;
pub mod metrics;
pub mod row;
pub mod section;
pub mod sidebar;
pub mod theme;

pub use controls::{
    Button, ButtonSize, ButtonVariant, IconButton, KeyChip, Platform, SegmentedControl, Select,
    SelectItem, SelectMenu, ShortcutBox, ShortcutState, Slider, Stepper, Switch, TextField,
    format_binding,
};
pub use feedback::{Badge, Banner, EmptyState, Toast};
pub use icon::{Assets, Icon, IconName, icon_bytes};
pub use row::{RowTone, SettingRow};
pub use section::{Card, SectionHeader, SettingsContent, SettingsGroup};
pub use sidebar::{Sidebar, SidebarGroupLabel, SidebarItem, SidebarSearch};
pub use theme::{Palette, Tokens, Tone, set_tokens, tokens};
