use super::*;
use crate::ui::markdown::{Block, Inline, parse_markdown};
use crate::ui::update_banner::{
    UpdateBannerAction, UpdateBannerButton, UpdateBannerTone, UpdateButtonStyle, UpdateProgress,
};
use gpui::prelude::FluentBuilder;
use gpui::{Animation, AnimationExt as _, FontWeight, bounce, ease_in_out, relative};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReleaseNotesDialog {
    pub(super) version: String,
    pub(super) status: ReleaseNotesStatus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReleaseNotesStatus {
    Loading,
    Ready { title: String, markdown: String },
    Error { message: String },
}

struct BannerTone {
    accent: gpui::Rgba,
    tile_bg: gpui::Rgba,
    icon_path: &'static str,
}

impl TerminalView {
    pub(super) fn release_notes_open(&self) -> bool {
        self.release_notes.is_some()
    }

    pub(super) fn close_release_notes(&mut self, cx: &mut Context<Self>) {
        if self.release_notes.take().is_some() {
            self.notify_overlay(cx);
            cx.notify();
        }
    }

    pub(super) fn open_release_notes(&mut self, version: String, cx: &mut Context<Self>) {
        if let Some(existing) = self.release_notes.as_ref()
            && existing.version == version
            && matches!(
                existing.status,
                ReleaseNotesStatus::Loading | ReleaseNotesStatus::Ready { .. }
            )
        {
            self.notify_overlay(cx);
            return;
        }

        self.start_release_notes_fetch(version, cx);
    }

    fn retry_release_notes(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self
            .release_notes
            .as_ref()
            .map(|dialog| dialog.version.clone())
        else {
            return;
        };
        self.start_release_notes_fetch(version, cx);
    }

    fn start_release_notes_fetch(&mut self, version: String, cx: &mut Context<Self>) {
        self.release_notes_generation = self.release_notes_generation.wrapping_add(1);
        let generation = self.release_notes_generation;
        self.release_notes_scroll = gpui::ScrollHandle::new();
        self.release_notes = Some(ReleaseNotesDialog {
            version: version.clone(),
            status: ReleaseNotesStatus::Loading,
        });
        self.notify_overlay(cx);
        cx.notify();

        let bg = cx
            .background_executor()
            .spawn(async move { crate::ui::release_notes::fetch_release_notes(&version) });
        cx.spawn(async move |this, cx| {
            let result = bg.await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    if view.release_notes_generation != generation {
                        return;
                    }
                    if let Some(dialog) = view.release_notes.as_mut() {
                        dialog.status = match result {
                            Ok(notes) => ReleaseNotesStatus::Ready {
                                title: notes.title,
                                markdown: notes.markdown,
                            },
                            Err(error) => ReleaseNotesStatus::Error {
                                message: error.user_message(),
                            },
                        };
                    }
                    view.notify_overlay(cx);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub(super) fn render_update_banner(
        &mut self,
        state: &UpdateState,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let model = crate::ui::update_banner::UpdateBannerModel::from_state(state)?;
        let updater_weak = self.auto_updater.as_ref().map(|e| e.downgrade());
        let overlay_style = self.overlay_style();
        let banner_bg = overlay_style.chrome_panel_background_with_floor(0.94, 0.88);
        let border_color = overlay_style.chrome_panel_neutral(0.16);
        let primary_text = overlay_style.panel_foreground(OVERLAY_PRIMARY_TEXT_ALPHA);
        let muted_text = overlay_style.panel_foreground(OVERLAY_MUTED_TEXT_ALPHA);
        let tone = banner_tone(model.tone, colors);
        let release_notes_version = model.version.clone();
        let has_buttons = !model.buttons.is_empty();
        let percent_chip = match model.progress.as_ref() {
            Some(UpdateProgress::Determinate { percent, .. }) => Some(*percent),
            _ => None,
        };

        let mut actions = div().flex().flex_wrap().items_center().gap(px(8.0));
        for button in model.buttons {
            actions = actions.child(self.render_update_banner_button(
                button,
                updater_weak.clone(),
                release_notes_version.clone(),
                tone.accent,
                primary_text,
                colors.background,
                cx,
            ));
        }

        let progress = model
            .progress
            .as_ref()
            .map(|progress| render_update_progress(progress, tone.accent, muted_text));

        Some(crate::ui::motion::enter_from_above(
            div()
                .id("update-dialog")
                .w_full()
                .max_w(px(UPDATE_BANNER_MAX_WIDTH))
                .flex_none()
                .bg(banner_bg)
                .border_1()
                .border_color(border_color)
                .rounded(px(UPDATE_BANNER_GEOMETRY.panel_radius))
                .shadow_lg()
                .overflow_hidden()
                .child(
                    div()
                        .w_full()
                        .flex()
                        .child(
                            div()
                                .w(px(UPDATE_BANNER_ACCENT_WIDTH))
                                .flex_none()
                                .bg(tone.accent),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .px(px(UPDATE_BANNER_PAD))
                                .py(px(12.0))
                                .flex()
                                .flex_col()
                                .gap(px(10.0))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_start()
                                        .gap(px(10.0))
                                        .child(banner_icon_tile(
                                            tone.tile_bg,
                                            tone.accent,
                                            tone.icon_path,
                                        ))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w(px(0.0))
                                                .flex()
                                                .flex_col()
                                                .gap(px(3.0))
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap(px(8.0))
                                                        .child(
                                                            div()
                                                                .min_w(px(0.0))
                                                                .flex_1()
                                                                .text_size(px(13.0))
                                                                .font_weight(FontWeight::SEMIBOLD)
                                                                .text_color(primary_text)
                                                                .overflow_hidden()
                                                                .child(model.message),
                                                        )
                                                        .child(banner_badge_chip(
                                                            model.badge,
                                                            percent_chip,
                                                            tone.accent,
                                                            tone.tile_bg,
                                                        )),
                                                )
                                                .children(model.detail.map(|detail| {
                                                    div()
                                                        .text_size(px(12.0))
                                                        .text_color(muted_text)
                                                        .line_height(px(16.0))
                                                        .child(detail)
                                                        .into_any()
                                                })),
                                        ),
                                )
                                .children(progress)
                                .when(has_buttons, |this| this.child(actions)),
                        ),
                ),
            "update-dialog-enter",
        ))
    }

    fn render_update_banner_button(
        &mut self,
        button: UpdateBannerButton,
        updater_weak: Option<gpui::WeakEntity<AutoUpdater>>,
        release_notes_version: Option<String>,
        accent: gpui::Rgba,
        primary_text: gpui::Rgba,
        background: gpui::Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let action = button.action;
        let mut secondary_bg = accent;
        secondary_bg.a = 0.16;
        let mut secondary_hover = accent;
        secondary_hover.a = 0.26;
        let mut ghost_hover = accent;
        ghost_hover.a = 0.12;
        let mut primary_hover = accent;
        primary_hover.a = 0.88;

        let (button_bg, hover_bg, button_text) = match button.style {
            UpdateButtonStyle::Primary => (accent, primary_hover, background),
            UpdateButtonStyle::Secondary => (secondary_bg, secondary_hover, accent),
            UpdateButtonStyle::Ghost => (
                gpui::Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 0.0,
                },
                ghost_hover,
                primary_text,
            ),
        };

        div()
            .id(gpui::ElementId::from(gpui::SharedString::from(format!(
                "update-banner-btn-{action:?}"
            ))))
            .h(px(28.0))
            .px(px(12.0))
            .rounded(px(UPDATE_BANNER_GEOMETRY.control_radius))
            .bg(button_bg)
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(button_text)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |style| style.bg(hover_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| match action {
                    UpdateBannerAction::Install => {
                        if let Some(ref weak) = updater_weak
                            && let Some(entity) = weak.upgrade()
                        {
                            AutoUpdater::install(entity.downgrade(), cx);
                            this.notify_overlay(cx);
                        }
                    }
                    UpdateBannerAction::Restart => match this.restart_application_with_persist() {
                        Ok(()) => {
                            this.allow_quit_without_prompt = true;
                            cx.quit();
                        }
                        Err(error) => {
                            crate::ui::toast::error(format!("Restart failed: {error}"));
                            this.notify_overlay(cx);
                        }
                    },
                    UpdateBannerAction::ViewReleaseNotes => {
                        if let Some(version) = release_notes_version.clone() {
                            this.open_release_notes(version, cx);
                        }
                    }
                    UpdateBannerAction::Dismiss => {
                        if let Some(ref weak) = updater_weak
                            && let Some(entity) = weak.upgrade()
                        {
                            entity.update(cx, |updater, cx| updater.dismiss(cx));
                        }
                    }
                }),
            )
            .child(button.label)
            .into_any_element()
    }

    pub(super) fn render_release_notes_dialog(
        &mut self,
        window: &mut Window,
        colors: &TerminalColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.release_notes.clone()?;
        let overlay_style = self.overlay_style();
        let panel_bg = overlay_style.chrome_panel_background_with_floor(
            COMMAND_PALETTE_PANEL_BG_ALPHA,
            COMMAND_PALETTE_PANEL_SOLID_ALPHA,
        );
        let border = overlay_style.chrome_panel_neutral(0.16);
        let primary_text = overlay_style.panel_foreground(OVERLAY_PRIMARY_TEXT_ALPHA);
        let muted_text = overlay_style.panel_foreground(OVERLAY_MUTED_TEXT_ALPHA);
        let mut accent = colors.ansi[4];
        accent.a = 1.0;
        let mut code_bg = colors.foreground;
        code_bg.a = 0.08;
        let mut chip_bg = accent;
        chip_bg.a = 0.16;
        let mut close_hover = colors.foreground;
        close_hover.a = 0.10;
        let mut scrim = colors.background;
        scrim.a = RELEASE_NOTES_SCRIM_ALPHA;

        let viewport = window.viewport_size();
        let viewport_width: f32 = viewport.width.into();
        let viewport_height: f32 = viewport.height.into();
        let panel_width = RELEASE_NOTES_PANEL_WIDTH.min((viewport_width - 48.0).max(320.0));
        let panel_max_height = RELEASE_NOTES_PANEL_MAX_HEIGHT
            .min((viewport_height - self.terminal_content_top_inset() - 48.0).max(240.0));

        let title = match &dialog.status {
            ReleaseNotesStatus::Ready { title, .. } => title.clone(),
            _ => format!("Version {}", dialog.version),
        };
        let body = match &dialog.status {
            ReleaseNotesStatus::Loading => div()
                .w_full()
                .py(px(36.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(muted_text)
                        .child("Fetching release notes from GitHub…"),
                )
                .child(render_update_progress(
                    &UpdateProgress::Indeterminate { caption: None },
                    accent,
                    muted_text,
                ))
                .into_any_element(),
            ReleaseNotesStatus::Error { message } => div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(primary_text)
                        .child(message.clone()),
                )
                .child(
                    div()
                        .id("release-notes-retry")
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(px(UPDATE_BANNER_GEOMETRY.control_radius))
                        .bg(chip_bg)
                        .text_size(px(12.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(accent)
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.retry_release_notes(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child("Try again"),
                )
                .into_any_element(),
            ReleaseNotesStatus::Ready { markdown, .. } if markdown.trim().is_empty() => div()
                .w_full()
                .py(px(24.0))
                .text_size(px(13.0))
                .text_color(muted_text)
                .child("This release has no written notes.")
                .into_any_element(),
            ReleaseNotesStatus::Ready { markdown, .. } => self.render_markdown_document(
                markdown,
                primary_text,
                muted_text,
                accent,
                code_bg,
                cx,
            ),
        };

        let panel = div()
            .id("release-notes-panel")
            .w(px(panel_width))
            .max_h(px(panel_max_height))
            .rounded(px(14.0))
            .bg(panel_bg)
            .border_1()
            .border_color(border)
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event, _window, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .w_full()
                    .px(px(16.0))
                    .py(px(12.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(banner_icon_tile(
                                chip_bg,
                                accent,
                                "icons/command_palette/info.svg",
                            ))
                            .child(
                                div()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(primary_text)
                                            .child("Release notes"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.0))
                                            .text_color(muted_text)
                                            .child(title),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .id("release-notes-close")
                            .w(px(24.0))
                            .h(px(24.0))
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(move |style| style.bg(close_hover))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.close_release_notes(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(
                                gpui::svg()
                                    .path(gpui::SharedString::from("icons/tab_strip/x.svg"))
                                    .size(px(12.0))
                                    .text_color(muted_text),
                            ),
                    ),
            )
            .child(
                div()
                    .id("release-notes-body")
                    .w_full()
                    .min_h(px(0.0))
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.release_notes_scroll)
                    .px(px(18.0))
                    .py(px(16.0))
                    .child(body),
            );

        Some(
            div()
                .id("release-notes-modal")
                .size_full()
                .absolute()
                .top_0()
                .left_0()
                .occlude()
                .bg(scrim)
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.close_release_notes(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(crate::ui::motion::enter_from_above(
                    panel,
                    "release-notes-dialog-enter",
                ))
                .into_any_element(),
        )
    }

    fn render_markdown_document(
        &mut self,
        markdown: &str,
        primary_text: gpui::Rgba,
        muted_text: gpui::Rgba,
        accent: gpui::Rgba,
        code_bg: gpui::Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let style = MarkdownStyle {
            primary_text,
            muted_text,
            accent,
            code_bg,
            mono_font: self.font_family.clone(),
        };
        let mut column = div().w_full().flex().flex_col().gap(px(12.0));
        for (index, block) in parse_markdown(markdown).into_iter().enumerate() {
            column = column.child(self.render_markdown_block(index, block, &style, cx));
        }
        column.into_any_element()
    }

    fn render_markdown_block(
        &mut self,
        index: usize,
        block: Block,
        style: &MarkdownStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match block {
            Block::Heading { level, children } => {
                let size = match level {
                    1 => 20.0,
                    2 => 16.0,
                    3 => 14.0,
                    _ => 13.0,
                };
                div()
                    .w_full()
                    .pt(px(if index == 0 { 0.0 } else { 6.0 }))
                    .text_size(px(size))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(style.primary_text)
                    .child(self.render_markdown_inlines(&children, style, cx))
                    .into_any_element()
            }
            Block::Paragraph(children) => div()
                .w_full()
                .text_size(px(13.0))
                .line_height(px(19.0))
                .text_color(style.primary_text)
                .child(self.render_markdown_inlines(&children, style, cx))
                .into_any_element(),
            Block::Quote(children) => div()
                .w_full()
                .pl(px(10.0))
                .border_l_1()
                .border_color(style.accent)
                .text_size(px(13.0))
                .text_color(style.muted_text)
                .child(self.render_markdown_inlines(&children, style, cx))
                .into_any_element(),
            Block::List { ordered, items } => {
                let mut list = div().w_full().flex().flex_col().gap(px(6.0));
                for (item_index, item) in items.into_iter().enumerate() {
                    let marker = if ordered {
                        format!("{}.", item_index + 1)
                    } else {
                        "•".to_string()
                    };
                    list = list.child(
                        div()
                            .w_full()
                            .flex()
                            .items_start()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .w(px(16.0))
                                    .flex_none()
                                    .text_size(px(13.0))
                                    .text_color(style.muted_text)
                                    .child(marker),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(13.0))
                                    .line_height(px(19.0))
                                    .text_color(style.primary_text)
                                    .child(self.render_markdown_inlines(&item, style, cx)),
                            ),
                    );
                }
                list.into_any_element()
            }
            Block::Code { code, .. } => div()
                .w_full()
                .px(px(10.0))
                .py(px(10.0))
                .rounded(px(UPDATE_BANNER_GEOMETRY.control_radius))
                .bg(style.code_bg)
                .font_family(style.mono_font.clone())
                .text_size(px(12.0))
                .text_color(style.primary_text)
                .child(code)
                .into_any_element(),
            Block::Rule => {
                let mut rule = style.muted_text;
                rule.a = 0.18;
                div().w_full().h(px(1.0)).bg(rule).into_any_element()
            }
        }
    }

    fn render_markdown_inlines(
        &mut self,
        inlines: &[Inline],
        style: &MarkdownStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut row = div().flex().flex_wrap().items_baseline();
        for (index, inline) in inlines.iter().enumerate() {
            row = row.child(self.render_markdown_inline(index, inline, style, cx));
        }
        row.into_any_element()
    }

    fn render_markdown_inline(
        &mut self,
        index: usize,
        inline: &Inline,
        style: &MarkdownStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match inline {
            Inline::Text(text) => div().child(text.clone()).into_any_element(),
            Inline::Strong(text) => div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(text.clone())
                .into_any_element(),
            Inline::Emphasis(text) => div().italic().child(text.clone()).into_any_element(),
            Inline::Code(text) => div()
                .px(px(4.0))
                .rounded(px(4.0))
                .bg(style.code_bg)
                .font_family(style.mono_font.clone())
                .text_size(px(12.0))
                .child(text.clone())
                .into_any_element(),
            Inline::Link { label, url } => {
                let url = url.clone();
                let mut hover = style.accent;
                hover.a = 0.16;
                div()
                    .id(gpui::ElementId::from(gpui::SharedString::from(format!(
                        "release-notes-link-{index}-{label}"
                    ))))
                    .text_color(style.accent)
                    .underline()
                    .cursor_pointer()
                    .hover(move |this| this.bg(hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_this, _event, _window, cx| {
                            if webbrowser::open(&url).is_err() {
                                crate::ui::toast::error("Could not open link");
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(label.clone())
                    .into_any_element()
            }
        }
    }
}

struct MarkdownStyle {
    primary_text: gpui::Rgba,
    muted_text: gpui::Rgba,
    accent: gpui::Rgba,
    code_bg: gpui::Rgba,
    mono_font: gpui::SharedString,
}

fn banner_tone(tone: UpdateBannerTone, colors: &TerminalColors) -> BannerTone {
    let mut accent = match tone {
        UpdateBannerTone::Info => colors.cursor,
        UpdateBannerTone::Success => colors.ansi[2],
        UpdateBannerTone::Error => colors.ansi[1],
    };
    accent.a = 1.0;
    let mut tile_bg = accent;
    tile_bg.a = 0.16;
    BannerTone {
        accent,
        tile_bg,
        icon_path: match tone {
            UpdateBannerTone::Info => "icons/command_palette/check-update.svg",
            UpdateBannerTone::Success => "icons/check.svg",
            UpdateBannerTone::Error => "icons/alert.svg",
        },
    }
}

fn banner_icon_tile(
    tile_bg: gpui::Rgba,
    accent: gpui::Rgba,
    icon_path: &'static str,
) -> AnyElement {
    div()
        .flex_none()
        .w(px(UPDATE_BANNER_ICON_SIZE))
        .h(px(UPDATE_BANNER_ICON_SIZE))
        .rounded(px(8.0))
        .bg(tile_bg)
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .path(gpui::SharedString::from(icon_path))
                .size(px(14.0))
                .text_color(accent),
        )
        .into_any_element()
}

fn banner_badge_chip(
    badge: &'static str,
    percent: Option<u8>,
    accent: gpui::Rgba,
    tile_bg: gpui::Rgba,
) -> AnyElement {
    div()
        .flex_none()
        .h(px(20.0))
        .px(px(8.0))
        .rounded(px(999.0))
        .bg(tile_bg)
        .flex()
        .items_center()
        .gap(px(6.0))
        .child(
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(accent)
                .child(badge),
        )
        .children(percent.map(|percent| {
            div()
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(accent)
                .child(format!("{percent}%"))
                .into_any_element()
        }))
        .into_any_element()
}

fn render_update_progress(
    progress: &UpdateProgress,
    accent: gpui::Rgba,
    muted_text: gpui::Rgba,
) -> AnyElement {
    let mut track = accent;
    track.a = 0.14;
    let bar = match progress {
        UpdateProgress::Determinate { percent, .. } => div()
            .id("update-progress-track")
            .w_full()
            .h(px(UPDATE_BANNER_PROGRESS_HEIGHT))
            .rounded_full()
            .bg(track)
            .overflow_hidden()
            .child(
                div()
                    .id("update-progress-fill")
                    .h_full()
                    .w(relative(f32::from(*percent) / 100.0))
                    .rounded_full()
                    .bg(accent),
            )
            .into_any_element(),
        UpdateProgress::Indeterminate { .. } => div()
            .id("update-progress-track")
            .relative()
            .w_full()
            .h(px(UPDATE_BANNER_PROGRESS_HEIGHT))
            .rounded_full()
            .bg(track)
            .overflow_hidden()
            .child(
                div()
                    .id("update-progress-blob")
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(relative(0.32))
                    .rounded_full()
                    .bg(accent)
                    .with_animation(
                        "update-progress-indeterminate",
                        Animation::new(Duration::from_millis(1400))
                            .repeat()
                            .with_easing(bounce(ease_in_out)),
                        |element, delta| element.left(relative(delta * 0.68)),
                    ),
            )
            .into_any_element(),
    };

    let caption = match progress {
        UpdateProgress::Determinate { caption, .. } | UpdateProgress::Indeterminate { caption } => {
            caption.clone()
        }
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(bar)
        .children(caption.map(|caption| {
            div()
                .text_size(px(11.0))
                .text_color(muted_text)
                .child(caption)
                .into_any_element()
        }))
        .into_any_element()
}
