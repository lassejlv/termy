import Foundation
internal import CTermy

/// Edits the user's Termy config file on disk.
///
/// These calls write to the same file the Termy app reads. Reload a
/// `TermyConfig` afterwards to observe the change.
public enum TermySettings {
    /// Sets a root setting such as `font_size` to a config-syntax value.
    public static func set(_ key: String, to value: String) throws(TermyError) {
        let status = withUTF8(key) { keyPtr, keyLen in
            withUTF8(value) { termy_settings_set_root(keyPtr, keyLen, $0, $1) }
        }
        try check(status, "termy_settings_set_root")
    }

    /// Removes a root setting so it falls back to its default.
    public static func reset(_ key: String) throws(TermyError) {
        try check(withUTF8(key) { termy_settings_reset_root($0, $1) }, "termy_settings_reset_root")
    }

    /// Sets a color key, such as `foreground`, to a `#rrggbb` value.
    public static func setColor(_ key: String, hex: String) throws(TermyError) {
        let status = withUTF8(key) { keyPtr, keyLen in
            withUTF8(hex) { termy_settings_set_color(keyPtr, keyLen, $0, $1) }
        }
        try check(status, "termy_settings_set_color")
    }

    /// Replaces the keybinding lines in the config.
    public static func setKeybinds(_ text: String) throws(TermyError) {
        try check(withUTF8(text) { termy_settings_set_keybinds($0, $1) }, "termy_settings_set_keybinds")
    }

    /// Rewrites the config file in canonical formatting.
    public static func prettifyConfig() throws(TermyError) {
        try check(termy_settings_prettify_config(), "termy_settings_prettify_config")
    }

    /// Installs the `termy` command-line tool for `shell` and returns a
    /// human-readable result message.
    public static func installCLI(shell: String) throws(TermyError) -> String {
        try withUTF8(shell) { (pointer, length) throws(TermyError) -> String in
            try takeString("termy_cli_install") { termy_cli_install(pointer, length, $0) }
        }
    }
}

/// Library-wide defaults.
public enum TermyDefaults {
    /// The runtime's default scrollback length in rows.
    public static var runtimeScrollback: Int { termy_runtime_config_default_scrollback() }

    /// The default scrollback for new terminals in rows.
    public static var terminalScrollback: Int { termy_terminal_options_default_scrollback() }

    /// The foreground color libtermy reports to color queries by default.
    public static var queryForeground: TermyColor? {
        var color = TermyFfiColor()
        guard termy_query_color_default_foreground(&color) == TERMY_FFI_OK else { return nil }
        return TermyColor(color)
    }
}
