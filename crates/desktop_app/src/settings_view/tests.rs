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
            assert_eq!(tokens.row_separator, view.row_separator_color());
            assert_eq!(tokens.text_primary, view.text_primary());
            assert_eq!(tokens.text_secondary, view.text_secondary());
            assert_eq!(tokens.text_muted, view.text_muted());
            assert_eq!(tokens.accent, view.accent());
            assert_eq!(tokens.accent_soft, view.sidebar_selection_bg());

            // The kit's own palette-derived tokens are opaque. Deriving them
            // that way here would silently drop this window's translucency,
            // so the mapped surfaces must keep their alpha.
            assert!(tokens.bg_card.a < 1.0);
            assert!(tokens.bg_panel.a < 1.0);

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

    let settings_window: AnyWindowHandle = settings.into();
    cx.dispatch_keystroke(settings_window, Keystroke::parse("escape").unwrap());

    assert_eq!(settings_window_count(cx), 0);
}
