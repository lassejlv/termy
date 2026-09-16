use super::*;
use gpui::prelude::FluentBuilder as _;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchKeyAction {
    Close,
    Next,
    Previous,
}

fn search_key_action(key: &str, shift_pressed: bool) -> Option<SearchKeyAction> {
    match key {
        "escape" => Some(SearchKeyAction::Close),
        "up" => Some(SearchKeyAction::Next),
        "down" => Some(SearchKeyAction::Previous),
        "enter" if shift_pressed => Some(SearchKeyAction::Previous),
        "enter" => Some(SearchKeyAction::Next),
        _ => None,
    }
}

fn search_counter_label(
    current: usize,
    total: usize,
    query_is_empty: bool,
    invalid_pattern: bool,
    scan_incomplete: bool,
) -> Option<String> {
    if query_is_empty {
        return None;
    }
    if invalid_pattern {
        return Some("Invalid pattern".to_string());
    }
    if total == 0 {
        return Some(if scan_incomplete {
            "Searching…".to_string()
        } else {
            "No matches".to_string()
        });
    }
    if scan_incomplete {
        Some(format!("{current} of {total}+"))
    } else {
        Some(format!("{current} of {total}"))
    }
}

fn visible_search_line_range(display_offset: usize, rows: u16) -> (i32, i32) {
    let first = -(i32::try_from(display_offset).unwrap_or(i32::MAX));
    let last = first.saturating_add(i32::from(rows.max(1)).saturating_sub(1));
    (first, last)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchScanKind {
    Viewport,
    Full,
}

fn search_line_span(first: i32, last: i32) -> i32 {
    if first > last {
        0
    } else {
        last.saturating_sub(first).saturating_add(1)
    }
}

impl TerminalView {
    fn normalize_search_prefill(text: &str) -> String {
        let mut normalized = String::with_capacity(text.len());
        let mut last_was_line_break = false;
        for character in text.chars() {
            if matches!(character, '\n' | '\r') {
                if !last_was_line_break {
                    normalized.push(' ');
                }
                last_was_line_break = true;
            } else {
                normalized.push(character);
                last_was_line_break = false;
            }
        }
        normalized.trim().to_string()
    }

    fn search_prefill_from_selection(selection: Option<String>) -> Option<String> {
        selection
            .map(|text| Self::normalize_search_prefill(&text))
            .filter(|text| !text.is_empty())
    }

    pub(super) fn execute_search_command_action(
        &mut self,
        action: CommandAction,
        cx: &mut Context<Self>,
    ) -> bool {
        match action {
            CommandAction::OpenSearch => {
                self.open_search(cx);
                true
            }
            CommandAction::CloseSearch => {
                self.close_search(cx);
                true
            }
            CommandAction::SearchNext => {
                if self.search_open {
                    self.search_next(cx);
                } else {
                    self.open_search(cx);
                }
                true
            }
            CommandAction::SearchPrevious => {
                if self.search_open {
                    self.search_previous(cx);
                } else {
                    self.open_search(cx);
                }
                true
            }
            CommandAction::ToggleSearchCaseSensitive => {
                self.search_state.toggle_case_sensitive();
                if self.search_open {
                    self.perform_search();
                    self.scroll_to_current_match(cx);
                }
                self.notify_search_ui(cx);
                true
            }
            CommandAction::ToggleSearchRegex => {
                self.search_state.toggle_regex_mode();
                if self.search_open {
                    self.perform_search();
                    self.scroll_to_current_match(cx);
                }
                self.notify_search_ui(cx);
                true
            }
            _ => false,
        }
    }

    pub(super) fn open_search(&mut self, cx: &mut Context<Self>) {
        let prefill = Self::search_prefill_from_selection(self.selected_text());

        if self.search_open {
            if let Some(prefill) = prefill {
                self.search_input.set_text(prefill);
                self.search_input.select_all();
                self.perform_search();
                self.scroll_to_current_match(cx);
                self.reset_cursor_blink_phase();
                self.notify_search_ui(cx);
            } else {
                self.search_input.select_all();
                self.reset_cursor_blink_phase();
                self.notify_search_ui(cx);
            }
            return;
        }

        let _ = self.close_terminal_context_menu(cx);

        // Close other overlays
        if self.is_command_palette_open() {
            self.close_command_palette(cx);
        }
        if self.renaming_tab.is_some() {
            self.cancel_rename_tab(cx);
        }
        if self.renaming_workspace.is_some() {
            self.cancel_rename_workspace(cx);
        }

        self.search_open = true;
        self.search_state.open();
        if let Some(prefill) = prefill {
            self.search_input.set_text(prefill);
            self.perform_search();
            self.scroll_to_current_match(cx);
        } else if !self.search_input.text().is_empty() {
            self.perform_search();
            self.scroll_to_current_match(cx);
        } else {
            self.clear_terminal_scrollbar_marker_cache();
        }
        self.search_input.select_all();
        self.reset_cursor_blink_phase();
        self.notify_search_ui(cx);
    }

    pub(super) fn close_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }

        self.search_open = false;
        self.search_state.hide();
        self.search_scan_incomplete = false;
        self.search_options_open = false;
        self.search_debounce_token = self.search_debounce_token.wrapping_add(1);
        self.clear_terminal_scrollbar_marker_cache();
        self.notify_search_ui(cx);
    }

    pub(super) fn refresh_search_if_open(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }
        self.perform_search();
        self.scroll_to_current_match(cx);
        self.notify_search_ui(cx);
    }

    fn notify_search_ui(&mut self, cx: &mut Context<Self>) {
        self.notify_overlay(cx);
        cx.notify();
    }

    fn toggle_search_options(&mut self, cx: &mut Context<Self>) {
        self.search_options_open = !self.search_options_open;
        self.notify_search_ui(cx);
    }

    /// Navigate to the next match in the result list. Since results are ordered
    /// newest-first (bottom of terminal = index 0), this moves toward older content.
    pub(super) fn search_next(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_state.results().is_empty() {
            return;
        }

        self.search_state.next_match();
        self.scroll_to_current_match(cx);
        self.notify_search_ui(cx);
    }

    /// Navigate to the previous match in the result list. Since results are ordered
    /// newest-first (bottom of terminal = index 0), this moves toward newer content.
    pub(super) fn search_previous(&mut self, cx: &mut Context<Self>) {
        if !self.search_open || self.search_state.results().is_empty() {
            return;
        }

        self.search_state.previous_match();
        self.scroll_to_current_match(cx);
        self.notify_search_ui(cx);
    }

    fn scroll_to_current_match(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.search_state.results().current() else {
            return;
        };

        let Some(terminal) = self.active_terminal() else {
            return;
        };
        let size = terminal.size();
        let rows = size.rows as i32;

        let (display_offset, history_size) = terminal.scroll_state();

        // `current.line` uses Alacritty coordinates: negative values are scrollback.
        let viewport_row = current.line + display_offset as i32;

        if viewport_row >= 0 && viewport_row < rows {
            return;
        }

        let target_offset = if current.line < 0 {
            (-current.line) as usize
        } else {
            0
        };

        let target_offset = target_offset.min(history_size);
        let delta = target_offset as i32 - display_offset as i32;

        if delta != 0 {
            terminal.scroll_display(delta);
            self.sync_content_scroll_baseline();
            self.mark_terminal_scrollbar_activity(cx);
        }
    }

    pub(super) fn perform_search(&mut self) {
        self.search_scan_incomplete = false;
        self.apply_search_scan(SearchScanKind::Full, false);
    }

    fn perform_viewport_search(&mut self) {
        self.apply_search_scan(SearchScanKind::Viewport, true);
    }

    fn apply_search_scan(&mut self, kind: SearchScanKind, preserve_cursor: bool) {
        self.search_state.set_query(self.search_input.text());

        if !self.search_state.has_valid_pattern() {
            self.search_state.clear_results_preserving_query();
            self.search_scan_incomplete = false;
            self.clear_terminal_scrollbar_marker_cache();
            return;
        }

        let Some(terminal) = self.active_terminal() else {
            self.search_state.clear_results_preserving_query();
            self.search_scan_incomplete = false;
            self.clear_terminal_scrollbar_marker_cache();
            return;
        };

        let previous = preserve_cursor.then(|| {
            self.search_state
                .results()
                .current()
                .map(|search_match| (search_match.line, search_match.start_col))
        });
        let size = terminal.size();
        let (display_offset, _) = terminal.scroll_state();
        let (viewport_first, viewport_last) = visible_search_line_range(display_offset, size.rows);
        let full_bounds = terminal
            .line_bounds()
            .unwrap_or((viewport_first, viewport_last));
        let (first, last) = match kind {
            SearchScanKind::Viewport => (viewport_first, viewport_last),
            SearchScanKind::Full => full_bounds,
        };

        let line_texts = collect_search_line_texts(terminal, first, last);
        let start_line = line_texts.first_line;
        let end_line = line_texts.last_line();
        self.search_state
            .search(start_line, end_line, |line_idx| line_texts.line(line_idx));
        self.search_scan_incomplete = matches!(kind, SearchScanKind::Viewport)
            && search_line_span(full_bounds.0, full_bounds.1) > search_line_span(first, last);

        if let Some(Some((line, start_col))) = previous {
            if !self.search_state.restore_current_match(line, start_col) {
                self.search_state.jump_to_nearest(viewport_first);
            }
        } else {
            self.search_state.jump_to_first();
        }
        if self.search_state.results().is_empty() && !self.search_scan_incomplete {
            self.clear_terminal_scrollbar_marker_cache();
        }
    }

    pub(super) fn handle_search_key_down(
        &mut self,
        key: &str,
        shift_pressed: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        match search_key_action(key, shift_pressed) {
            Some(SearchKeyAction::Close) => {
                self.close_search(cx);
                true
            }
            Some(SearchKeyAction::Next) => {
                self.search_next(cx);
                true
            }
            Some(SearchKeyAction::Previous) => {
                self.search_previous(cx);
                true
            }
            None => {
                // Text input is handled elsewhere via InlineInput actions
                false
            }
        }
    }

    pub(super) fn handle_search_input_changed(&mut self, cx: &mut Context<Self>) {
        self.search_state.set_query(self.search_input.text());
        if !self.search_state.has_valid_pattern() {
            self.search_debounce_token = self.search_debounce_token.wrapping_add(1);
            self.search_scan_incomplete = false;
            self.search_state.clear_results_preserving_query();
            self.clear_terminal_scrollbar_marker_cache();
            self.notify_search_inline_input(cx);
            return;
        }

        let scan_full_now = {
            let Some(terminal) = self.active_terminal() else {
                self.search_scan_incomplete = false;
                self.search_state.clear_results_preserving_query();
                self.clear_terminal_scrollbar_marker_cache();
                self.notify_search_inline_input(cx);
                return;
            };
            let size = terminal.size();
            let (display_offset, _) = terminal.scroll_state();
            let (viewport_first, viewport_last) =
                visible_search_line_range(display_offset, size.rows);
            let full_bounds = terminal
                .line_bounds()
                .unwrap_or((viewport_first, viewport_last));
            search_line_span(full_bounds.0, full_bounds.1) <= SEARCH_SYNC_LINE_LIMIT
        };
        if scan_full_now {
            self.perform_search();
            self.scroll_to_current_match(cx);
            self.notify_search_inline_input(cx);
            return;
        }

        self.perform_viewport_search();
        self.scroll_to_current_match(cx);
        self.notify_search_inline_input(cx);

        self.search_debounce_token = self.search_debounce_token.wrapping_add(1);
        let token = self.search_debounce_token;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            smol::Timer::after(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |view, cx| {
                    if view.search_debounce_token == token {
                        view.apply_search_scan(SearchScanKind::Full, true);
                        view.scroll_to_current_match(cx);
                        view.notify_search_inline_input(cx);
                    }
                })
            });
        })
        .detach();
    }

    pub(super) fn render_search_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let overlay_style = self.overlay_style();
        let bar_bg = overlay_style.chrome_panel_background_with_floor(
            SEARCH_BAR_BG_ALPHA,
            COMMAND_PALETTE_PANEL_SOLID_ALPHA,
        );
        let input_bg = overlay_style.chrome_panel_background(SEARCH_INPUT_BG_ALPHA);
        let input_border = overlay_style.panel_foreground(0.10);
        let counter_text = overlay_style.panel_foreground(SEARCH_COUNTER_TEXT_ALPHA);
        let button_text = overlay_style.panel_foreground(SEARCH_BUTTON_TEXT_ALPHA);
        let button_hover_bg = overlay_style.chrome_panel_cursor(SEARCH_BUTTON_HOVER_BG_ALPHA);
        let button_active_bg = overlay_style.chrome_panel_cursor(0.28);
        let button_pressed_bg = overlay_style.chrome_panel_cursor(0.36);
        let strong_text = overlay_style.panel_foreground(OVERLAY_PRIMARY_TEXT_ALPHA);
        let muted_text = overlay_style.panel_foreground(OVERLAY_MUTED_TEXT_ALPHA);
        let query_empty = self.search_input.text().is_empty();
        let (current, total) = self.search_state.results().position().unwrap_or((0, 0));
        let has_error = self.search_state.error().is_some();
        let case_sensitive = self.search_state.is_case_sensitive();
        let regex_mode = matches!(self.search_state.mode(), termy_search::SearchMode::Regex);
        let error_color = gpui::Rgba {
            r: 0.98,
            g: 0.48,
            b: 0.48,
            a: 1.0,
        };
        let status_label = search_counter_label(
            current,
            total,
            query_empty,
            has_error,
            self.search_scan_incomplete,
        );
        let radius = SEARCH_OVERLAY_GEOMETRY.control_radius;

        let nav_button =
            |id: &'static str, icon: &'static str, next: bool, cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .w(px(28.0))
                    .h(px(26.0))
                    .rounded(px(radius))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(button_text)
                    .hover(|style| style.bg(button_hover_bg))
                    .active(move |style| style.bg(button_pressed_bg))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            if next {
                                this.search_next(cx);
                            } else {
                                this.search_previous(cx);
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        gpui::svg()
                            .path(gpui::SharedString::from(icon))
                            .size(px(13.0))
                            .text_color(button_text),
                    )
            };

        let mode_chip = |id: &'static str,
                         label: &'static str,
                         active: bool,
                         action: CommandAction,
                         cx: &mut Context<Self>| {
            div()
                .id(id)
                .h(px(26.0))
                .px(px(8.0))
                .rounded(px(radius))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.0))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active { strong_text } else { button_text })
                .bg(if active {
                    button_active_bg
                } else {
                    gpui::Rgba {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }
                })
                .hover(|style| style.bg(button_hover_bg))
                .active(move |style| style.bg(button_pressed_bg))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.execute_search_command_action(action, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(label)
        };

        crate::ui::motion::enter_from_above(
            div()
                .id("search-bar-host")
                .absolute()
                .top(px(self.terminal_content_top_inset() + SEARCH_BAR_INSET))
                .right(px(SEARCH_BAR_INSET))
                .w(px(SEARCH_BAR_WIDTH))
                .child(
                    div()
                        .id("search-bar")
                        .w(px(SEARCH_BAR_WIDTH))
                        .min_h(px(SEARCH_BAR_HEIGHT))
                        .occlude()
                        .bg(bar_bg)
                        .rounded(px(SEARCH_OVERLAY_GEOMETRY.panel_radius))
                        .border_1()
                        .border_color(overlay_style.panel_foreground(0.10))
                        .shadow_lg()
                        .px(px(10.0))
                        .py(px(8.0))
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _event, _window, cx| {
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(4.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .h(px(30.0))
                                        .relative()
                                        .overflow_hidden()
                                        .rounded(px(SEARCH_OVERLAY_GEOMETRY.input_radius))
                                        .bg(input_bg)
                                        .border_1()
                                        .border_color(if has_error {
                                            error_color
                                        } else {
                                            input_border
                                        })
                                        .child(
                                            div()
                                                .absolute()
                                                .left(px(10.0))
                                                .top_0()
                                                .bottom_0()
                                                .w(px(14.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    gpui::svg()
                                                        .path(gpui::SharedString::from(
                                                            "icons/settings/search.svg",
                                                        ))
                                                        .size(px(13.0))
                                                        .text_color(button_text),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .left(px(32.0))
                                                .right(px(if status_label.is_some() {
                                                    132.0
                                                } else {
                                                    10.0
                                                }))
                                                .top_0()
                                                .bottom_0()
                                                .overflow_hidden()
                                                .when(query_empty, |el| {
                                                    el.child(
                                                        div()
                                                            .absolute()
                                                            .left_0()
                                                            .top_0()
                                                            .bottom_0()
                                                            .flex()
                                                            .items_center()
                                                            .text_size(px(13.0))
                                                            .text_color(muted_text)
                                                            .child("Find in terminal"),
                                                    )
                                                })
                                                .child(
                                                    div().relative().size_full().child(
                                                        self.render_inline_input_layer(
                                                            Font {
                                                                family: self.ui_font_family.clone(),
                                                                ..gpui::font("")
                                                            },
                                                            px(13.0),
                                                            strong_text.into(),
                                                            overlay_style
                                                                .chrome_panel_cursor(
                                                                    SEARCH_INPUT_SELECTION_ALPHA,
                                                                )
                                                                .into(),
                                                            InlineInputAlignment::Left,
                                                            cx,
                                                        ),
                                                    ),
                                                ),
                                        )
                                        .children(status_label.map(|label| {
                                            div()
                                                .absolute()
                                                .right(px(6.0))
                                                .top(px(4.0))
                                                .bottom(px(4.0))
                                                .px(px(8.0))
                                                .rounded(px(6.0))
                                                .bg(if has_error {
                                                    let mut bg = error_color;
                                                    bg.a = 0.14;
                                                    bg
                                                } else {
                                                    overlay_style.panel_foreground(0.06)
                                                })
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(11.0))
                                                .text_color(if has_error {
                                                    error_color
                                                } else {
                                                    counter_text
                                                })
                                                .child(label)
                                        })),
                                )
                                .child(nav_button(
                                    "search-prev",
                                    "icons/settings/chevron-up.svg",
                                    true,
                                    cx,
                                ))
                                .child(nav_button(
                                    "search-next",
                                    "icons/settings/chevron-down.svg",
                                    false,
                                    cx,
                                ))
                                .child(
                                    div()
                                        .id("search-options")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(radius))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(
                                            if self.search_options_open
                                                || case_sensitive
                                                || regex_mode
                                            {
                                                strong_text
                                            } else {
                                                button_text
                                            },
                                        )
                                        .bg(if self.search_options_open {
                                            button_active_bg
                                        } else if case_sensitive || regex_mode {
                                            overlay_style.panel_foreground(0.08)
                                        } else {
                                            gpui::Rgba {
                                                r: 0.0,
                                                g: 0.0,
                                                b: 0.0,
                                                a: 0.0,
                                            }
                                        })
                                        .hover(|style| style.bg(button_hover_bg))
                                        .active(move |style| style.bg(button_pressed_bg))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _event, _window, cx| {
                                                this.toggle_search_options(cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(
                                            gpui::svg()
                                                .path(gpui::SharedString::from(
                                                    "icons/settings/more.svg",
                                                ))
                                                .size(px(14.0))
                                                .text_color(
                                                    if self.search_options_open
                                                        || case_sensitive
                                                        || regex_mode
                                                    {
                                                        strong_text
                                                    } else {
                                                        button_text
                                                    },
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .id("search-close")
                                        .w(px(26.0))
                                        .h(px(26.0))
                                        .rounded(px(radius))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(button_text)
                                        .hover(|style| style.bg(button_hover_bg))
                                        .active(move |style| style.bg(button_pressed_bg))
                                        .cursor_pointer()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _event, _window, cx| {
                                                this.close_search(cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(
                                            gpui::svg()
                                                .path(gpui::SharedString::from(
                                                    "icons/tab_strip/x.svg",
                                                ))
                                                .size(px(11.0))
                                                .text_color(button_text),
                                        ),
                                ),
                        )
                        .when(self.search_options_open, |bar| {
                            bar.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(mode_chip(
                                        "search-case-sensitive",
                                        "Case",
                                        case_sensitive,
                                        CommandAction::ToggleSearchCaseSensitive,
                                        cx,
                                    ))
                                    .child(mode_chip(
                                        "search-regex",
                                        "Regex",
                                        regex_mode,
                                        CommandAction::ToggleSearchRegex,
                                        cx,
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .items_center()
                                            .justify_end()
                                            .pr(px(4.0))
                                            .text_size(px(10.0))
                                            .text_color(muted_text)
                                            .child("↵ next  ·  ⇧↵ prev  ·  esc"),
                                    ),
                            )
                        }),
                ),
            "search-bar-enter",
        )
    }
}

struct SearchLineSnapshot {
    first_line: i32,
    text: String,
    ranges: Vec<Range<usize>>,
}

impl SearchLineSnapshot {
    const MISSING_LINE: usize = usize::MAX;

    fn new(first_line: i32, line_count: usize) -> Self {
        Self {
            first_line,
            text: String::new(),
            ranges: Vec::with_capacity(line_count),
        }
    }

    fn line(&self, line_idx: i32) -> Option<&str> {
        let offset = usize::try_from(line_idx.checked_sub(self.first_line)?).ok()?;
        let range = self.ranges.get(offset)?;
        if range.start == Self::MISSING_LINE {
            return None;
        }
        self.text.get(range.clone())
    }

    fn last_line(&self) -> i32 {
        i32::try_from(self.ranges.len().saturating_sub(1))
            .ok()
            .and_then(|offset| self.first_line.checked_add(offset))
            .unwrap_or(self.first_line)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.ranges.len()
    }

    #[cfg(test)]
    fn lines(&self) -> impl Iterator<Item = &str> {
        self.ranges
            .iter()
            .filter(|range| range.start != Self::MISSING_LINE)
            .filter_map(|range| self.text.get(range.clone()))
    }
}

fn collect_search_line_texts(
    terminal: &Terminal,
    start_line: i32,
    end_line: i32,
) -> SearchLineSnapshot {
    let mut line_texts = None;
    let captured_range =
        terminal.for_each_line_cell_range(start_line, end_line, |range, line_idx, _, cell| {
            let snapshot = line_texts.get_or_insert_with(|| {
                let first = start_line.max(range.first_line);
                let last = end_line.min(range.last_line);
                let line_count = inclusive_line_count(first, last);
                let mut snapshot = SearchLineSnapshot::new(first, line_count);
                snapshot.ranges.resize(
                    line_count,
                    SearchLineSnapshot::MISSING_LINE..SearchLineSnapshot::MISSING_LINE,
                );
                snapshot
                    .text
                    .reserve(line_count.saturating_mul(range.columns));
                snapshot
            });
            let Some(index) = line_idx
                .checked_sub(snapshot.first_line)
                .and_then(|offset| usize::try_from(offset).ok())
            else {
                return;
            };
            let Some(cell_range) = snapshot.ranges.get_mut(index) else {
                return;
            };
            if cell_range.start == SearchLineSnapshot::MISSING_LINE {
                *cell_range = snapshot.text.len()..snapshot.text.len();
            }
            let character = cell.character();
            if character == '\0' || cell.is_trailing_wide_spacer() || character.is_control() {
                snapshot.text.push(' ');
            } else {
                snapshot.text.push(character);
                cell.append_combining_to(&mut snapshot.text);
            }
            cell_range.end = snapshot.text.len();
        });

    line_texts.unwrap_or_else(|| {
        let Some(range) = captured_range else {
            return SearchLineSnapshot::new(start_line, 0);
        };
        let first = start_line.max(range.first_line);
        let last = end_line.min(range.last_line);
        let line_count = inclusive_line_count(first, last);
        let mut snapshot = SearchLineSnapshot::new(first, line_count);
        snapshot.ranges.resize(
            line_count,
            SearchLineSnapshot::MISSING_LINE..SearchLineSnapshot::MISSING_LINE,
        );
        snapshot
    })
}

fn inclusive_line_count(first: i32, last: i32) -> usize {
    if first > last {
        return 0;
    }
    usize::try_from(i64::from(last) - i64::from(first) + 1).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use termy_core::TerminalSize;

    #[test]
    fn search_prefill_ignores_empty_selection() {
        assert_eq!(TerminalView::search_prefill_from_selection(None), None);
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("   \n\t".to_string())),
            None
        );
    }

    #[test]
    fn search_prefill_keeps_non_empty_selection() {
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("panic: missing semicolon".into())),
            Some("panic: missing semicolon".into())
        );
    }

    #[test]
    fn search_prefill_normalizes_multiline_selection_for_single_line_input() {
        assert_eq!(
            TerminalView::search_prefill_from_selection(Some("  line-1\r\nline-2\n  ".into())),
            Some("line-1 line-2".into())
        );
    }

    #[test]
    fn search_counter_label_hides_empty_query_without_results() {
        assert_eq!(search_counter_label(0, 0, true, false, false), None);
    }

    #[test]
    fn search_counter_label_reports_zero_results_for_non_empty_query() {
        assert_eq!(
            search_counter_label(0, 0, false, false, false),
            Some("No matches".to_string())
        );
    }

    #[test]
    fn search_counter_label_reports_current_result_position() {
        assert_eq!(
            search_counter_label(3, 12, false, false, false),
            Some("3 of 12".to_string())
        );
    }

    #[test]
    fn search_counter_label_marks_incomplete_scans() {
        assert_eq!(
            search_counter_label(1, 4, false, false, true),
            Some("1 of 4+".to_string())
        );
        assert_eq!(
            search_counter_label(0, 0, false, false, true),
            Some("Searching…".to_string())
        );
        assert_eq!(
            search_counter_label(0, 0, false, true, false),
            Some("Invalid pattern".to_string())
        );
    }

    #[test]
    fn visible_search_line_range_maps_display_offset_to_alacritty_lines() {
        assert_eq!(visible_search_line_range(0, 24), (0, 23));
        assert_eq!(visible_search_line_range(10, 24), (-10, 13));
    }

    #[test]
    fn search_line_span_counts_inclusive_alacritty_lines() {
        assert_eq!(search_line_span(0, 23), 24);
        assert_eq!(search_line_span(-10, 13), 24);
        assert_eq!(search_line_span(4, 3), 0);
    }

    fn filled_line_count(lines: &SearchLineSnapshot) -> usize {
        lines.lines().filter(|line| !line.trim().is_empty()).count()
    }

    #[test]
    fn terminal_read_adapter_extracts_lines_for_both_runtime_variants() {
        let size = TerminalSize {
            cols: 24,
            rows: 4,
            ..TerminalSize::default()
        };

        let tmux = Terminal::new_tmux(
            size,
            TerminalOptions {
                scrollback_history: 256,
                ..TerminalOptions::default()
            },
        );
        tmux.feed_output(b"tmux-line\r\n");
        let tmux_lines = collect_search_line_texts(&tmux, 0, i32::from(size.rows) - 1);
        assert_eq!(tmux_lines.len(), usize::from(size.rows));
        assert!(
            filled_line_count(&tmux_lines) >= 1,
            "tmux terminal should expose at least one non-empty line"
        );

        let native = Terminal::new_native(size, None, None, None, None, None)
            .expect("native terminal should initialize for read adapter test");
        let native_lines = collect_search_line_texts(&native, 0, i32::from(size.rows) - 1);
        assert_eq!(native_lines.len(), usize::from(size.rows));
        let native_has_non_empty_buffer = native_lines.lines().any(|line| !line.is_empty());
        assert!(
            native_has_non_empty_buffer,
            "native terminal read adapter should expose at least one non-empty line buffer"
        );
    }

    #[test]
    fn terminal_read_adapter_preserves_core_combining_characters_in_history() {
        let size = TerminalSize {
            cols: 4,
            rows: 2,
            ..TerminalSize::default()
        };
        let terminal = Terminal::new_test_display(size);
        terminal.hydrate_output("e\u{301}\r\nmid\r\nnew".as_bytes());

        let lines = collect_search_line_texts(&terminal, -1, -1);
        assert_eq!(lines.line(-1), Some("e\u{301}   "));
    }

    #[test]
    fn search_key_action_uses_shift_for_enter_navigation() {
        assert_eq!(
            search_key_action("enter", false),
            Some(SearchKeyAction::Next)
        );
        assert_eq!(
            search_key_action("enter", true),
            Some(SearchKeyAction::Previous)
        );
        assert_eq!(
            search_key_action("escape", false),
            Some(SearchKeyAction::Close)
        );
        assert_eq!(search_key_action("a", true), None);
        assert_eq!(search_key_action("up", false), Some(SearchKeyAction::Next));
        assert_eq!(
            search_key_action("down", false),
            Some(SearchKeyAction::Previous)
        );
    }
}
