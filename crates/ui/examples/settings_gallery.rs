//! Renders the Appearance settings screen out of `termy_ui`, for eyeballing the
//! kit against the design.
//!
//! ```sh
//! cargo run -p termy_ui --example settings_gallery
//! ```

use gpui::{
    App, AppContext, Application, Bounds, Context, IntoElement, ParentElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use termy_ui::{
    Badge, Button, ButtonSize, EmptyState, IconButton, IconName, Palette, SectionHeader, Select,
    SelectItem, SelectMenu, SettingRow, SettingsContent, SettingsGroup, Sidebar, SidebarGroupLabel,
    SidebarItem, SidebarSearch, Slider, Stepper, Switch, Tokens, metrics, theme,
};

const SECTIONS: &[(&str, &str, IconName)] = &[
    ("appearance", "Appearance", IconName::Appearance),
    ("colors", "Colors", IconName::Colors),
    ("themes", "Themes", IconName::Themes),
    ("tabs", "Tabs", IconName::Tabs),
    ("terminal", "Terminal", IconName::Terminal),
    ("ssh", "SSH hosts", IconName::Ssh),
    ("keybindings", "Keybindings", IconName::Keybindings),
    ("plugins", "Plugins", IconName::Plugins),
    ("general", "General", IconName::General),
];

struct Gallery {
    selected: usize,
    theme_menu_open: bool,
    chrome_contrast: bool,
    font_size: u32,
    window_padding: u32,
    background_opacity: f32,
}

impl Gallery {
    fn new() -> Self {
        Self {
            selected: 0,
            theme_menu_open: false,
            chrome_contrast: true,
            font_size: 14,
            window_padding: 12,
            background_opacity: 0.85,
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> Sidebar {
        let group = |label: &'static str, first: bool| SidebarGroupLabel::new(label).first(first);
        let item = |index: usize, selected: bool, cx: &mut Context<Self>| {
            let (id, label, icon) = SECTIONS[index];
            SidebarItem::new(id, label)
                .icon(icon)
                .selected(selected)
                .on_click(cx.listener(move |gallery: &mut Self, _event, _window, cx| {
                    gallery.selected = index;
                    gallery.theme_menu_open = false;
                    cx.notify();
                }))
        };

        Sidebar::new()
            .child(SidebarSearch::new())
            .child(group("INTERFACE", true))
            .children((0..4).map(|index| item(index, self.selected == index, cx)))
            .child(group("SESSION", false))
            .children((4..7).map(|index| item(index, self.selected == index, cx)))
            .child(group("SYSTEM", false))
            .children((7..9).map(|index| item(index, self.selected == index, cx)))
            .footer("Termy v0.2.6")
    }

    fn theme_group(&self, cx: &mut Context<Self>) -> SettingsGroup {
        let theme_mode = Select::new("theme-mode", "Manual")
            .open(self.theme_menu_open)
            .on_click(cx.listener(|gallery: &mut Self, _event, _window, cx| {
                gallery.theme_menu_open = !gallery.theme_menu_open;
                cx.notify();
            }));

        // The open menu hangs under the closed control, the way the app anchors
        // its own popup.
        let theme_mode_control = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(4.0))
            .w(metrics::CONTROL_WIDTH)
            .child(theme_mode)
            .children(self.theme_menu_open.then(|| {
                SelectMenu::new()
                    .item(SelectItem::new("Manual").selected(true))
                    .item(SelectItem::new("Follow system appearance"))
            }));

        let contrast_row = SettingRow::new("Increase Chrome Contrast")
            .description("Increase contrast of non-terminal UI surfaces")
            .control(
                Switch::new("chrome-contrast", self.chrome_contrast).on_click(cx.listener(
                    |gallery: &mut Self, _event, _window, cx| {
                        gallery.chrome_contrast = !gallery.chrome_contrast;
                        cx.notify();
                    },
                )),
            );
        let contrast_row = if self.chrome_contrast {
            contrast_row.badge(Badge::new("SAVED"))
        } else {
            contrast_row
        };

        SettingsGroup::new("THEME")
            .child(
                SettingRow::new("Theme Mode")
                    .description("Use a single theme or switch with system appearance")
                    .control(theme_mode_control),
            )
            .child(
                SettingRow::new("Theme")
                    .description("Current color scheme name")
                    .control(
                        Select::new("theme", "tokyo-night-storm")
                            .swatch(Palette::tokyo_night_storm().cursor),
                    )
                    .reset(IconButton::new("reset-theme", IconName::Reset)),
            )
            .child(contrast_row)
    }

    fn window_group(&self, cx: &mut Context<Self>) -> SettingsGroup {
        SettingsGroup::new("WINDOW")
            .child(
                SettingRow::new("Background Opacity")
                    .description("Live preview while dragging · 5% steps")
                    .control(Slider::new(
                        "background-opacity",
                        self.background_opacity,
                        format!("{:.2}", self.background_opacity),
                    ))
                    .reset(IconButton::new("reset-opacity", IconName::Reset).on_click(
                        cx.listener(|gallery: &mut Self, _event, _window, cx| {
                            gallery.background_opacity = 1.0;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                SettingRow::new("Window Padding")
                    .description("Inner gap between terminal grid and window edge")
                    .control(
                        Stepper::new("window-padding", format!("{} px", self.window_padding))
                            .on_increment(cx.listener(|gallery: &mut Self, _event, _window, cx| {
                                gallery.window_padding = gallery.window_padding.saturating_add(1);
                                cx.notify();
                            }))
                            .on_decrement(cx.listener(
                                |gallery: &mut Self, _event, _window, cx| {
                                    gallery.window_padding =
                                        gallery.window_padding.saturating_sub(1);
                                    cx.notify();
                                },
                            )),
                    ),
            )
    }

    fn typography_group(&self, cx: &mut Context<Self>) -> SettingsGroup {
        SettingsGroup::new("TYPOGRAPHY & SPACING")
            .child(
                SettingRow::new("Font Family")
                    .description("Monospace family used for the terminal grid")
                    .control(Select::new("font-family", "JetBrains Mono"))
                    .reset(IconButton::new("reset-font-family", IconName::Reset)),
            )
            .child(
                SettingRow::new("Font Size")
                    .description("Grid cell type size · affects rows and columns")
                    .control(
                        Stepper::new("font-size", format!("{} px", self.font_size))
                            .on_increment(cx.listener(|gallery: &mut Self, _event, _window, cx| {
                                gallery.font_size = gallery.font_size.saturating_add(1);
                                cx.notify();
                            }))
                            .on_decrement(cx.listener(
                                |gallery: &mut Self, _event, _window, cx| {
                                    gallery.font_size = gallery.font_size.saturating_sub(1);
                                    cx.notify();
                                },
                            )),
                    ),
            )
    }

    fn content(&self, cx: &mut Context<Self>) -> SettingsContent {
        let (_, label, icon) = SECTIONS[self.selected];

        if self.selected != 0 {
            return SettingsContent::new()
                .child(SectionHeader::new(label))
                .child(
                    EmptyState::new(format!("{label} is not in this gallery"))
                        .icon(icon)
                        .body("The example ships the Appearance screen. Every other section is built from the same components.")
                        .action(Button::new("back-to-appearance", "Back to Appearance").on_click(
                            cx.listener(|gallery: &mut Self, _event, _window, cx| {
                                gallery.selected = 0;
                                cx.notify();
                            }),
                        )),
                );
        }

        SettingsContent::new()
            .child(
                SectionHeader::new("Appearance")
                    .subtitle("Customize the look and feel")
                    .action(Button::new("reset-section", "Reset section").size(ButtonSize::Small)),
            )
            .child(self.theme_group(cx))
            .child(self.window_group(cx))
            .child(self.typography_group(cx))
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::tokens(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(colors.bg_window)
            .font_family("JetBrains Mono")
            .text_color(colors.text_primary)
            .child(
                // Stand-in for the app's chrome: the real traffic lights are
                // drawn by the platform over this strip.
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .h(px(38.0))
                    .bg(colors.bg_panel)
                    .border_b_1()
                    .border_color(colors.row_separator)
                    .text_size(metrics::LABEL_SIZE)
                    .text_color(colors.text_muted)
                    .child("SETTINGS"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_grow()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(self.content(cx)),
            )
    }
}

fn main() {
    Application::new()
        .with_assets(termy_ui::Assets)
        .run(|cx: &mut App| {
            theme::set_tokens(Tokens::dark(), cx);

            let bounds = Bounds {
                origin: point(px(120.0), px(120.0)),
                size: size(px(1120.0), px(760.0)),
            };

            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Termy — Settings".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.0), px(13.0))),
                    }),
                    ..Default::default()
                },
                |_window, cx| cx.new(|_cx| Gallery::new()),
            )
            .expect("failed to open the gallery window");

            cx.activate(true);
        });
}
