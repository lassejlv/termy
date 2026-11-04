use anyhow::{Context, Result, anyhow};
use std::collections::HashMap;

use termy_core::tmux_control_core::types::{
    TmuxPaneMouseMode, TmuxPaneState, TmuxSessionSummary, TmuxSnapshot, TmuxWindowState,
};

const SNAPSHOT_FIELD_SEP: char = '\u{1f}';
const SESSION_SNAPSHOT_FORMAT: &str = concat!(
    "#{q:session_name}",
    "\u{1f}",
    "#{session_id}",
    "\u{1f}",
    "#{session_windows}",
    "\u{1f}",
    "#{session_attached}",
);
pub(crate) const WINDOW_SNAPSHOT_FORMAT: &str = concat!(
    "#{window_id}",
    "\u{1f}",
    "#{window_index}",
    "\u{1f}",
    "#{q:window_name}",
    "\u{1f}",
    "#{q:window_layout}",
    "\u{1f}",
    "#{window_active}",
    "\u{1f}",
    "#{automatic-rename}",
);
pub(crate) const PANE_SNAPSHOT_FORMAT: &str = concat!(
    "#{pane_id}",
    "\u{1f}",
    "#{window_id}",
    "\u{1f}",
    "#{session_id}",
    "\u{1f}",
    "#{pane_active}",
    "\u{1f}",
    "#{pane_left}",
    "\u{1f}",
    "#{pane_top}",
    "\u{1f}",
    "#{pane_width}",
    "\u{1f}",
    "#{pane_height}",
    "\u{1f}",
    "#{cursor_x}",
    "\u{1f}",
    "#{cursor_y}",
    "\u{1f}",
    "#{mouse_standard_flag}",
    "\u{1f}",
    "#{mouse_button_flag}",
    "\u{1f}",
    "#{mouse_any_flag}",
    "\u{1f}",
    "#{mouse_sgr_flag}",
    "\u{1f}",
    "#{mouse_utf8_flag}",
    "\u{1f}",
    "#{q:pane_current_path}",
    "\u{1f}",
    "#{q:pane_current_command}",
);
pub(crate) const PANE_MOUSE_MODE_FORMAT: &str = concat!(
    "#{mouse_standard_flag}",
    "#{mouse_button_flag}",
    "#{mouse_any_flag}",
    "#{mouse_sgr_flag}",
    "#{mouse_utf8_flag}",
);

pub(crate) fn session_snapshot_format() -> &'static str {
    SESSION_SNAPSHOT_FORMAT
}

/// Splits one snapshot record into fields and decodes each field's escapes,
/// in a single pass so every escape is decoded exactly once.
///
/// Records come in two encodings. Output of `tmux -F` run as a subprocess has
/// raw `\x1f` separators. Output of the same query sent through control mode
/// is escaped again by tmux: every byte below 0x20 (the separator too) prints
/// as an octal escape (`\037`), but backslashes are left as they are. So in
/// control mode, `\\` is a backslash that `#{q:...}` escaped, `\NNN` is a byte,
/// and an escaped 0x1f ends the field. Decoding the whole line first and then
/// decoding fields again turned `a\\101` (a window named `a\101`) into `aA`.
///
/// tmux escapes a 0x1f inside a `#{q:...}` field as `\037` too, so in control
/// mode such a byte still cannot be told apart from a separator; the field
/// count check below rejects the record instead of misreading it.
fn decode_snapshot_fields(line: &str) -> Result<Vec<String>> {
    let escaped_separator = !line.contains(SNAPSHOT_FIELD_SEP);
    let bytes = line.as_bytes();
    let mut fields = Vec::new();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let current = bytes[index];
        if current == SNAPSHOT_FIELD_SEP as u8 && !escaped_separator {
            fields.push(snapshot_field_text(std::mem::take(&mut output)));
            index += 1;
            continue;
        }
        if current != b'\\' {
            output.push(current);
            index += 1;
            continue;
        }

        index += 1;
        let escape = bytes
            .get(index)
            .ok_or_else(|| anyhow!("invalid trailing escape in snapshot field '{line}'"))?;
        let decoded = match escape {
            b'\\' => {
                index += 1;
                b'\\'
            }
            b'n' => {
                index += 1;
                b'\n'
            }
            b'r' => {
                index += 1;
                b'\r'
            }
            b't' => {
                index += 1;
                b'\t'
            }
            b'x' => {
                let hex = bytes
                    .get(index + 1..index + 3)
                    .ok_or_else(|| anyhow!("invalid hex escape in snapshot field '{line}'"))?;
                let hi = (hex[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| anyhow!("invalid hex escape in snapshot field '{line}'"))?;
                let lo = (hex[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| anyhow!("invalid hex escape in snapshot field '{line}'"))?;
                index += 3;
                ((hi << 4) | lo) as u8
            }
            b'0'..=b'7' => {
                let octal = bytes
                    .get(index..index + 3)
                    .ok_or_else(|| anyhow!("invalid octal escape in snapshot field '{line}'"))?;
                if !octal.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                    return Err(anyhow!("invalid octal escape in snapshot field '{line}'"));
                }
                index += 3;
                // Computed in u16: `\400`..`\777` would overflow a u8 shift.
                ((u16::from(octal[0] - b'0') << 6)
                    | (u16::from(octal[1] - b'0') << 3)
                    | u16::from(octal[2] - b'0')) as u8
            }
            _ => {
                // tmux `#{q:...}` uses shell-style escaping for punctuation and whitespace
                // (for example: "\\[", "\\(", "\\ ", "\\*", "\\?"), so decode these as literals.
                index += 1;
                *escape
            }
        };
        if decoded == SNAPSHOT_FIELD_SEP as u8 && escaped_separator {
            fields.push(snapshot_field_text(std::mem::take(&mut output)));
        } else {
            output.push(decoded);
        }
    }

    fields.push(snapshot_field_text(output));
    Ok(fields)
}

/// tmux escapes bytes that are not valid UTF-8 (a directory named with a raw
/// 0xff byte prints as `\377`), so a decoded field can hold them. Keep the rest
/// of the snapshot rather than failing it over one name or path.
fn snapshot_field_text(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    }
}

/// A pane path that could not be decoded as UTF-8 is reported as unknown
/// (empty) instead of a path with replacement characters, which no directory
/// matches and which would make new splits and tabs fail to start there.
fn snapshot_pane_path(path: String) -> String {
    if path.contains(char::REPLACEMENT_CHARACTER) {
        String::new()
    } else {
        path
    }
}

fn parse_snapshot_fields<const N: usize>(line: &str, kind: &str) -> Result<[String; N]> {
    // Snapshot rows must have a fixed schema. Rejecting mismatched field counts
    // prevents silent record drift when delimiters appear unescaped in data.
    let fields = decode_snapshot_fields(line)?;
    let field_count = fields.len();

    fields.try_into().map_err(|_| {
        anyhow!("invalid tmux {kind} line: expected {N} fields, got {field_count}: '{line}'")
    })
}

fn parse_snapshot_bool(value: &str, field: &str, kind: &str, line: &str) -> Result<bool> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(anyhow!("invalid {field} in tmux {kind} line: '{line}'")),
    }
}

fn parse_snapshot_u16(value: &str, field: &str, kind: &str, line: &str) -> Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("invalid {field} in tmux {kind} line: '{line}'"))
}

fn parse_snapshot_i32(value: &str, field: &str, kind: &str, line: &str) -> Result<i32> {
    value
        .parse::<i32>()
        .with_context(|| format!("invalid {field} in tmux {kind} line: '{line}'"))
}

pub(crate) fn parse_pane_mouse_mode(value: &str) -> Result<TmuxPaneMouseMode> {
    let [standard, button, any, sgr, utf8] = value.as_bytes() else {
        return Err(anyhow!(
            "invalid tmux pane mouse mode: expected 5 flags, got {value:?}"
        ));
    };
    let flag = |value| match value {
        b'0' => Ok(false),
        b'1' => Ok(true),
        _ => Err(anyhow!("invalid tmux pane mouse mode flag: {value:?}")),
    };

    Ok(TmuxPaneMouseMode {
        standard: flag(*standard)?,
        button: flag(*button)?,
        any: flag(*any)?,
        sgr: flag(*sgr)?,
        utf8: flag(*utf8)?,
    })
}

pub(crate) fn parse_session_summaries(output: &str) -> Result<Vec<TmuxSessionSummary>> {
    let mut sessions = Vec::new();

    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let [name, id, window_count, attached_clients] =
            parse_snapshot_fields::<4>(line, "session")?;
        let window_count = parse_snapshot_u16(&window_count, "session_windows", "session", line)?;
        let attached_clients =
            parse_snapshot_u16(&attached_clients, "session_attached", "session", line)?;
        sessions.push(TmuxSessionSummary {
            name,
            id,
            window_count,
            attached_clients,
        });
    }

    Ok(sessions)
}

pub(crate) fn parse_snapshot(
    session_name: &str,
    windows: &str,
    panes: &str,
) -> Result<TmuxSnapshot> {
    let mut panes_by_window: HashMap<String, Vec<TmuxPaneState>> = HashMap::new();
    let mut session_id = None::<String>;

    for line in panes.lines().filter(|line| !line.trim().is_empty()) {
        let [
            pane_id,
            window_id,
            pane_session_id,
            pane_active,
            pane_left,
            pane_top,
            pane_width,
            pane_height,
            cursor_x,
            cursor_y,
            mouse_standard,
            mouse_button,
            mouse_any,
            mouse_sgr,
            mouse_utf8,
            current_path,
            current_command,
        ] = parse_snapshot_fields::<17>(line, "pane")?;
        let is_active = parse_snapshot_bool(&pane_active, "pane_active", "pane", line)?;
        let left = parse_snapshot_u16(&pane_left, "pane_left", "pane", line)?;
        let top = parse_snapshot_u16(&pane_top, "pane_top", "pane", line)?;
        let width = parse_snapshot_u16(&pane_width, "pane_width", "pane", line)?;
        let height = parse_snapshot_u16(&pane_height, "pane_height", "pane", line)?;
        let cursor_x = parse_snapshot_u16(&cursor_x, "cursor_x", "pane", line)?;
        let cursor_y = parse_snapshot_u16(&cursor_y, "cursor_y", "pane", line)?;
        let mouse_mode = TmuxPaneMouseMode {
            standard: parse_snapshot_bool(&mouse_standard, "mouse_standard_flag", "pane", line)?,
            button: parse_snapshot_bool(&mouse_button, "mouse_button_flag", "pane", line)?,
            any: parse_snapshot_bool(&mouse_any, "mouse_any_flag", "pane", line)?,
            sgr: parse_snapshot_bool(&mouse_sgr, "mouse_sgr_flag", "pane", line)?,
            utf8: parse_snapshot_bool(&mouse_utf8, "mouse_utf8_flag", "pane", line)?,
        };

        if session_id.is_none() {
            session_id = Some(pane_session_id.clone());
        }

        panes_by_window
            .entry(window_id.clone())
            .or_default()
            .push(TmuxPaneState {
                id: pane_id,
                window_id,
                session_id: pane_session_id,
                is_active,
                left,
                top,
                width,
                height,
                cursor_x,
                cursor_y,
                mouse_mode,
                current_path: snapshot_pane_path(current_path),
                current_command,
            });
    }

    let mut parsed_windows = Vec::new();
    for line in windows.lines().filter(|line| !line.trim().is_empty()) {
        let [
            window_id,
            window_index,
            name,
            layout,
            window_active,
            automatic_rename,
        ] = parse_snapshot_fields::<6>(line, "window")?;
        let index = parse_snapshot_i32(&window_index, "window_index", "window", line)?;
        let is_active = parse_snapshot_bool(&window_active, "window_active", "window", line)?;
        let automatic_rename =
            parse_snapshot_bool(&automatic_rename, "automatic-rename", "window", line)?;

        let mut window_panes = panes_by_window.remove(&window_id).unwrap_or_default();
        window_panes.sort_by_key(|pane| (pane.top, pane.left));
        let active_pane_id = window_panes
            .iter()
            .find(|pane| pane.is_active)
            .map(|pane| pane.id.clone());

        parsed_windows.push(TmuxWindowState {
            id: window_id,
            index,
            name,
            layout,
            is_active,
            automatic_rename,
            active_pane_id,
            panes: window_panes,
        });
    }

    parsed_windows.sort_by_key(|window| window.index);

    Ok(TmuxSnapshot {
        session_name: session_name.to_string(),
        session_id,
        windows: parsed_windows,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        SNAPSHOT_FIELD_SEP, parse_pane_mouse_mode, parse_session_summaries, parse_snapshot,
    };

    #[test]
    fn parse_pane_mouse_mode_accepts_tmux_flags() {
        let mode = parse_pane_mouse_mode("00110").expect("mouse mode");
        assert!(mode.any);
        assert!(mode.sgr);
        assert!(!mode.standard);
        assert!(!mode.button);
        assert!(!mode.utf8);
    }

    #[test]
    fn parse_pane_mouse_mode_rejects_invalid_flags() {
        assert!(parse_pane_mouse_mode("0011").is_err());
        assert!(parse_pane_mouse_mode("00x10").is_err());
    }

    #[test]
    fn parse_snapshot_builds_windows_and_panes() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows = format!(
            "@1{sep}0{sep}one{sep}layout-a{sep}1{sep}1\n@2{sep}1{sep}two{sep}layout-b{sep}0{sep}0\n",
        );
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}13{sep}22{sep}0{sep}0{sep}1{sep}1{sep}0{sep}/tmp{sep}zsh\n\
             %2{sep}@2{sep}$1{sep}1{sep}0{sep}0{sep}60{sep}24{sep}7{sep}2{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/work{sep}sleep\n\
             %3{sep}@2{sep}$1{sep}0{sep}61{sep}0{sep}19{sep}24{sep}3{sep}8{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/work{sep}zsh\n",
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(snapshot.windows.len(), 2);
        assert_eq!(snapshot.windows[0].id, "@1");
        assert_eq!(snapshot.windows[0].panes.len(), 1);
        assert_eq!(snapshot.windows[1].panes.len(), 2);
        assert!(snapshot.windows[0].automatic_rename);
        assert!(!snapshot.windows[1].automatic_rename);
        assert_eq!(snapshot.windows[0].panes[0].cursor_x, 13);
        assert_eq!(snapshot.windows[0].panes[0].cursor_y, 22);
        assert!(snapshot.windows[0].panes[0].mouse_mode.any);
        assert!(snapshot.windows[0].panes[0].mouse_mode.sgr);
        assert!(!snapshot.windows[0].panes[0].mouse_mode.standard);
        assert_eq!(snapshot.windows[0].panes[0].current_path, "/tmp");
        assert_eq!(snapshot.windows[1].panes[0].current_command, "sleep");
    }

    #[test]
    fn parse_snapshot_accepts_escaped_field_delimiters_in_window_name_and_command() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows =
            format!("@1{sep}0{sep}name\\x09with-tab\\x1fwindow{sep}layout\\x1fgrid{sep}1{sep}1\n");
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp\\x1fdir\\x09tab{sep}cmd\\x0awith-nl\\x1fpart\n",
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(snapshot.windows[0].name, "name\twith-tab\x1fwindow");
        assert_eq!(snapshot.windows[0].layout, "layout\x1fgrid");
        assert_eq!(
            snapshot.windows[0].panes[0].current_path,
            "/tmp\x1fdir\ttab"
        );
        assert_eq!(
            snapshot.windows[0].panes[0].current_command,
            "cmd\nwith-nl\x1fpart"
        );
    }

    #[test]
    fn parse_snapshot_accepts_tmux_q_octal_escapes_for_tabs_newlines_and_delimiters() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows =
            format!("@1{sep}0{sep}name\\011with-tab\\037window{sep}layout\\037grid{sep}1{sep}1\n");
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp\\011dir\\037tab{sep}cmd\\012with-nl\\037part\n",
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(snapshot.windows[0].name, "name\twith-tab\x1fwindow");
        assert_eq!(snapshot.windows[0].layout, "layout\x1fgrid");
        assert_eq!(
            snapshot.windows[0].panes[0].current_path,
            "/tmp\tdir\x1ftab"
        );
        assert_eq!(
            snapshot.windows[0].panes[0].current_command,
            "cmd\nwith-nl\x1fpart"
        );
    }

    #[test]
    fn parse_snapshot_accepts_tmux_q_shell_escaped_window_layout() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows = format!(
            "@1{sep}0{sep}one{sep}aeea,149x39,0,0{{74x39,0,0\\[74x19,0,0,0,74x19,0,20,2],74x39,75,0,1}}{sep}1{sep}1\n",
        );
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}149{sep}39{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp{sep}zsh\n"
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(
            snapshot.windows[0].layout,
            "aeea,149x39,0,0{74x39,0,0[74x19,0,0,0,74x19,0,20,2],74x39,75,0,1}"
        );
    }

    #[test]
    fn parse_snapshot_accepts_tmux_q_shell_escaped_punctuation() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows =
            format!("@1{sep}0{sep}name\\[a\\]\\(b\\)\\ c\\*d\\?e{sep}layout-a{sep}1{sep}1\n",);
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp\\ path\\[x\\]\\(y\\){sep}cmd\\ \\\"quoted\\\"\\ and\\ symbols\\*\\?\n",
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(snapshot.windows[0].name, "name[a](b) c*d?e");
        assert_eq!(snapshot.windows[0].panes[0].current_path, "/tmp path[x](y)");
        assert_eq!(
            snapshot.windows[0].panes[0].current_command,
            "cmd \"quoted\" and symbols*?"
        );
    }

    #[test]
    fn parse_snapshot_rejects_unescaped_field_separator_in_window_record() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows = format!("@1{sep}0{sep}broken{sep}name{sep}layout{sep}1{sep}1\n");
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp{sep}zsh\n"
        );
        let error = parse_snapshot("termy", windows.as_str(), panes.as_str()).unwrap_err();
        assert!(error.to_string().contains("expected 6 fields, got 7"));
    }

    #[test]
    fn parse_snapshot_rejects_invalid_hex_escape_in_fields() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows = format!("@1{sep}0{sep}name\\x0g{sep}layout{sep}1{sep}1\n");
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp{sep}zsh\n"
        );
        let error = parse_snapshot("termy", windows.as_str(), panes.as_str()).unwrap_err();
        assert!(error.to_string().contains("invalid hex escape"));
    }

    /// A record as tmux prints it in control mode: separators and other bytes
    /// below 0x20 octal-escaped, backslashes left as `#{q:...}` wrote them.
    fn control_mode_record(fields: &[&str]) -> String {
        fields.join("\\037")
    }

    #[test]
    fn parse_snapshot_decodes_control_mode_escapes_once() {
        // `#{q:window_name}` prints a window named `a\101` as `a\\101`.
        let windows = control_mode_record(&["@1", "0", r"a\\101", r"lay\ out", "1", "1"]);
        let panes = control_mode_record(&[
            "%1",
            "@1",
            "$1",
            "1",
            "0",
            "0",
            "80",
            "24",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            r"/tmp/x\\101\011tab",
            "zsh",
        ]);
        let snapshot = parse_snapshot("termy", &windows, &panes).expect("snapshot");
        assert_eq!(snapshot.windows[0].name, r"a\101");
        assert_eq!(snapshot.windows[0].layout, "lay out");
        assert_eq!(
            snapshot.windows[0].panes[0].current_path,
            "/tmp/x\\101\ttab"
        );
    }

    #[test]
    fn parse_snapshot_keeps_going_when_a_field_is_not_utf8() {
        // tmux prints a raw 0xff byte in a format as `\377`.
        let windows = control_mode_record(&["@1", "0", r"a\377b", "layout", "1", "1"]);
        let panes = control_mode_record(&[
            "%1",
            "@1",
            "$1",
            "1",
            "0",
            "0",
            "80",
            "24",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            r"/tmp/a\377b",
            r"z\377sh",
        ]);
        let snapshot = parse_snapshot("termy", &windows, &panes).expect("snapshot");
        assert_eq!(snapshot.windows[0].name, "a\u{fffd}b");
        let pane = &snapshot.windows[0].panes[0];
        // No directory has the replacement-character name, so the path is unknown.
        assert_eq!(pane.current_path, "");
        assert_eq!(pane.current_command, "z\u{fffd}sh");

        // The same record with raw separators (a subprocess query) decodes alike.
        let sep = SNAPSHOT_FIELD_SEP.to_string();
        let raw_windows = windows.replace("\\037", &sep);
        let raw_panes = panes.replace("\\037", &sep);
        let raw = parse_snapshot("termy", &raw_windows, &raw_panes).expect("raw snapshot");
        assert_eq!(raw.windows[0].name, "a\u{fffd}b");
        assert_eq!(raw.windows[0].panes[0].current_path, "");
    }

    #[test]
    fn parse_session_summaries_builds_session_rows() {
        let sep = SNAPSHOT_FIELD_SEP;
        let output = format!("work{sep}$1{sep}3{sep}1\nsandbox{sep}$2{sep}1{sep}0\n",);
        let sessions = parse_session_summaries(output.as_str()).expect("sessions");
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "work");
        assert_eq!(sessions[0].window_count, 3);
        assert_eq!(sessions[0].attached_clients, 1);
        assert_eq!(sessions[1].name, "sandbox");
    }

    #[test]
    fn parse_session_summaries_rejects_non_numeric_fields() {
        let sep = SNAPSHOT_FIELD_SEP;
        let output = format!("work{sep}$1{sep}x{sep}0\n");
        let error = parse_session_summaries(output.as_str()).expect_err("invalid session row");
        assert!(error.to_string().contains("session_windows"));
    }

    #[test]
    fn parse_snapshot_ignores_orphan_panes_for_missing_windows() {
        let sep = SNAPSHOT_FIELD_SEP;
        let windows = format!("@1{sep}0{sep}one{sep}layout-a{sep}1{sep}1\n");
        let panes = format!(
            "%1{sep}@1{sep}$1{sep}1{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp{sep}zsh\n\
             %2{sep}@2{sep}$1{sep}0{sep}0{sep}0{sep}80{sep}24{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}0{sep}/tmp{sep}zsh\n"
        );
        let snapshot = parse_snapshot("termy", windows.as_str(), panes.as_str()).expect("snapshot");
        assert_eq!(snapshot.windows.len(), 1);
        assert_eq!(snapshot.windows[0].id, "@1");
        assert_eq!(snapshot.windows[0].panes.len(), 1);
    }
}
