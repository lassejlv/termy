import XCTest
@testable import TermySwift

final class SettingsSchemaParityTests: XCTestCase {
    func testSwiftSettingsSchemaExposesSharedNativeConfigurationSurface() throws {
        let schema = try SettingsBridge.loadSchema(contents: "")
        let settingsByKey = schema.settingsByKey
        let requiredKeys = [
            "window_width",
            "window_height",
            "warn_on_quit",
            "warn_on_quit_with_running_process",
            "auto_update",
            "tmux_enabled",
            "tmux_persistence",
            "tmux_exclusive",
            "tmux_binary",
            "tmux_command_prefix",
            "tmux_show_active_pane_border",
            "simple_mode",
            "native_tab_persistence",
            "native_layout_autosave",
            "native_buffer_persistence",
            "show_debug_overlay",
            "onboarding_complete",
            "tab_close_visibility",
            "tab_width_mode",
            "tab_bar_position",
            "native_tab_placement",
            "tab_switch_modifier_hints",
            "ui_font_family",
            "chrome_contrast",
            "command_palette_show_keybinds",
            "app_icon",
            "shell_integration_enabled",
            "progress_indicator_enabled",
            "auto_hide_tabbar",
            "sidebar_enabled",
            "sidebar_width",
            "show_termy_in_titlebar"
        ]

        let missingKeys = requiredKeys.filter { settingsByKey[$0] == nil }
        XCTAssertEqual(missingKeys, [])
    }

    func testSwiftSettingsSchemaReflectsParsedFixtureValues() throws {
        let schema = try SettingsBridge.loadSchema(contents: """
        auto_update = false
        tmux_enabled = true
        tmux_binary = /opt/homebrew/bin/tmux
        tab_close_visibility = always
        tab_width_mode = active_grow_sticky
        tab_bar_position = right
        native_tab_placement = sidebar
        ui_font_family = Avenir Next
        app_icon = default
        """)
        let settingsByKey = schema.settingsByKey

        XCTAssertEqual(settingsByKey["auto_update"]?.value, "false")
        XCTAssertEqual(settingsByKey["tmux_enabled"]?.value, "true")
        XCTAssertEqual(settingsByKey["tmux_binary"]?.value, "/opt/homebrew/bin/tmux")
        XCTAssertEqual(settingsByKey["tab_close_visibility"]?.value, "always")
        XCTAssertEqual(settingsByKey["tab_width_mode"]?.value, "active_grow_sticky")
        XCTAssertEqual(settingsByKey["tab_bar_position"]?.value, "right")
        XCTAssertEqual(settingsByKey["native_tab_placement"]?.value, "sidebar")
        XCTAssertEqual(settingsByKey["ui_font_family"]?.value, "Avenir Next")
        XCTAssertEqual(settingsByKey["app_icon"]?.value, "default")
    }

    func testSwiftSettingsSchemaIncludesExpectedEnumChoices() throws {
        let schema = try SettingsBridge.loadSchema(contents: "")
        let settingsByKey = schema.settingsByKey

        XCTAssertEqual(
            settingsByKey["tab_close_visibility"]?.choices?.map(\.value),
            ["active_hover", "hover", "always"]
        )
        XCTAssertEqual(
            settingsByKey["tab_width_mode"]?.choices?.map(\.value),
            ["uniform", "stable", "active_grow", "active_grow_sticky"]
        )
        XCTAssertEqual(
            settingsByKey["tab_bar_position"]?.choices?.map(\.value),
            ["top", "right"]
        )
        XCTAssertEqual(
            settingsByKey["native_tab_placement"]?.choices?.map(\.value),
            ["native_tabbar", "sidebar"]
        )
        XCTAssertEqual(
            settingsByKey["app_icon"]?.choices?.map(\.value),
            ["default", "old"]
        )
    }

    func testSwiftSettingsSchemaDoesNotSynthesizeLegacyBuiltinThemeChoices() throws {
        let schema = try SettingsBridge.loadSchema(contents: "")
        let settingsByKey = schema.settingsByKey
        let choices = try XCTUnwrap(settingsByKey["theme"]?.choices)

        XCTAssertTrue(choices.contains { $0.value == "termy" })
        XCTAssertTrue(choices.contains { $0.value == "shell-decide" })
        XCTAssertFalse(choices.contains { $0.value == "tokyo-night" })
        XCTAssertTrue(choices.first { $0.value == "termy" }?.swatches?.isEmpty ?? true)
    }
}

private extension SettingsSchema {
    var settingsByKey: [String: Setting] {
        Dictionary(
            uniqueKeysWithValues: sections
                .flatMap { section in section.groups ?? [] }
                .flatMap(\.settings)
                .map { ($0.key, $0) }
        )
    }
}
