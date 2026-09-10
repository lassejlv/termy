use super::test_utils::open_settings_window_handle;
use super::*;
use gpui::{AnyWindowHandle, Keystroke, TestAppContext};

fn settings_window_count(cx: &TestAppContext) -> usize {
    cx.windows()
        .into_iter()
        .filter(|handle| handle.downcast::<SettingsWindow>().is_some())
        .count()
}

#[test]
fn settings_effective_background_opacity_prefers_preview() {
    assert_eq!(
        config::effective_background_opacity(
            0.9,
            Some(config::BackgroundOpacityPreview {
                owner_id: 1,
                opacity: 0.35,
            }),
        ),
        0.35
    );
    assert_eq!(config::effective_background_opacity(0.9, None), 0.9);
}

#[test]
fn settings_preview_clears_when_saved_matches_preview() {
    assert_eq!(
        config::synced_background_opacity_preview(
            0.4,
            Some(config::BackgroundOpacityPreview {
                owner_id: 1,
                opacity: 0.4,
            }),
        ),
        None
    );
}

#[test]
fn settings_preview_keeps_unrelated_value() {
    assert_eq!(
        config::synced_background_opacity_preview(
            0.4,
            Some(config::BackgroundOpacityPreview {
                owner_id: 1,
                opacity: 0.6,
            }),
        ),
        Some(config::BackgroundOpacityPreview {
            owner_id: 1,
            opacity: 0.6,
        })
    );
}

#[gpui::test]
fn settings_ui_tokens_track_the_windows_own_chrome_colors(cx: &mut TestAppContext) {
    let settings = open_settings_window_handle(cx);

    let tokens = settings
        .update(cx, |view, _window, cx| {
            let tokens = view.ui_tokens();

            assert_eq!(tokens.bg_window, view.bg_primary());
            assert_eq!(tokens.bg_panel, view.bg_secondary());
            assert_eq!(tokens.bg_card, view.bg_elevated());
            assert_eq!(tokens.bg_input, view.bg_input());
            assert_eq!(tokens.bg_hover, view.bg_hover());
            assert_eq!(tokens.border, view.border_color());
            assert_eq!(tokens.card_border, view.card_border_color());
            assert_eq!(tokens.row_separator, view.divider_color());
            assert_eq!(tokens.text_primary, view.text_primary());
            assert_eq!(tokens.text_secondary, view.text_secondary());
            assert_eq!(tokens.text_muted, view.text_muted());
            assert_eq!(tokens.accent, view.accent());
            assert_eq!(tokens.accent, view.sidebar_selection_bg());

            // Settings uses theme colors, while terminal opacity only affects
            // the terminal windows themselves.
            view.config.background_opacity = 0.0;
            assert_eq!(view.ui_tokens(), tokens);
            assert_eq!(tokens.bg_card.a, 1.0);
            assert_eq!(tokens.bg_panel.a, 1.0);

            view.colors.background = gpui::rgb(0xfafafa);
            view.colors.foreground = gpui::rgb(0x383a42);
            view.colors.cursor = gpui::rgb(0x006cde);
            let tokens = view.ui_tokens();
            assert_eq!(tokens.bg_window, view.colors.background);
            assert_eq!(tokens.text_primary, view.colors.foreground);
            assert_eq!(tokens.accent, view.colors.cursor);
            view.sync_ui_tokens(cx);
            tokens
        })
        .expect("settings window should still be open");

    cx.update(|app| {
        assert_eq!(
            app.try_global::<termy_ui::Tokens>().copied(),
            Some(tokens),
            "components render from the global, so it has to carry this window's colors"
        );
    });
}

#[gpui::test]
fn escape_closes_settings_window_with_sidebar_search_active(cx: &mut TestAppContext) {
    let settings = open_settings_window_handle(cx);
    assert_eq!(settings_window_count(cx), 1);
    settings
        .update(cx, |view, _, _| view.sidebar_search_active = true)
        .unwrap();

    let settings_window: AnyWindowHandle = settings.into();
    cx.dispatch_keystroke(settings_window, Keystroke::parse("escape").unwrap());

    assert_eq!(settings_window_count(cx), 0);
}

#[gpui::test]
fn settings_preserve_manual_theme_across_system_appearance_changes(cx: &mut TestAppContext) {
    let settings = open_settings_window_handle(cx);
    settings
        .update(cx, |view, _, cx| {
            view.config.theme_mode = config::AppearanceMode::Manual;
            let tokens = view.ui_tokens();
            view.handle_window_appearance_change(gpui::WindowAppearance::Light, cx);
            assert_eq!(view.ui_tokens(), tokens);
            view.handle_window_appearance_change(gpui::WindowAppearance::Dark, cx);
            assert_eq!(view.ui_tokens(), tokens);
        })
        .unwrap();
}

#[gpui::test]
fn settings_scrollbar_cannot_intercept_sidebar_clicks(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(SettingsWindow::new);
    cx.draw(
        point(px(0.0), px(0.0)),
        gpui::size(px(980.0), px(740.0)),
        |_, _| view.clone().into_any_element(),
    );
    cx.simulate_click(point(px(80.0), px(180.0)), gpui::Modifiers::none());
    cx.update(|_, cx| assert_eq!(view.read(cx).active_section, SettingsSection::Appearance));
    cx.simulate_keystrokes("cmd-f");
    cx.update(|_, cx| assert!(view.read(cx).sidebar_search_active));
    cx.update(|window, cx| {
        let range = view.read(cx).settings_scrollbar_range(window);
        assert!(
            range.max_offset > 0.0 && range.viewport_extent > 0.0,
            "{range:?}"
        );
    });

    // A second layout includes the scrollbar thumb and active text input.
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.sidebar_search_state = TextInputState::new("cursor".into());
            view.refresh_search_navigation(window, cx);
        });
    });
    cx.draw(
        point(px(0.0), px(0.0)),
        gpui::size(px(980.0), px(740.0)),
        |_, _| view.clone().into_any_element(),
    );
    cx.simulate_click(point(px(186.0), px(67.0)), gpui::Modifiers::none());
    cx.update(|_, cx| assert!(view.read(cx).sidebar_search_state.text().is_empty()));
}
