//! Golden test of the multiplexer wire format (audit I5).
//!
//! Hosts and clients exchange untagged bincode of crate-wide types, so any
//! change to one of them silently breaks a running host of the same
//! `VERSION`. This test encodes a sample of every variant of every wire enum,
//! and every struct they carry, and hashes the result.
//!
//! - Adding a variant: the exhaustive `match` for its enum stops compiling.
//!   Add the arm with the next index, a sample, then follow the fingerprint
//!   message below.
//! - Changing a field, or a type a wire type embeds: the fingerprint changes.
//!   Bump `VERSION` (unless the unreleased change already did) and update
//!   `WIRE_FINGERPRINT` in `protocol.rs`.
//! - Samples must encode the same on every platform: no platform defaults, at
//!   most one `HashMap` entry, and deadlines that have passed.
//! - Fields of the same type hold distinct values, so swapping two of them
//!   changes the fingerprint. Groups of `bool`s also get one sample per field
//!   with only that field set (see [`Samples::one_hot`]).
use super::protocol::*;
use crate::remote::{graphics::GraphicsEncoder, *};
use crate::*;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Instant};

fn encode(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_message(&mut bytes, value).unwrap();
    bytes
}

/// The number of variants `T` decodes. Probes each index followed by zeros:
/// only an index past the last variant fails as an invalid variant index.
fn variant_count<T: DeserializeOwned>() -> u32 {
    (0..1024)
        .find(|index: &u32| {
            let mut body = index.to_le_bytes().to_vec();
            body.resize(4096, 0);
            let mut frame = (body.len() as u32).to_le_bytes().to_vec();
            frame.extend(body);
            read_message::<T>(&mut frame.as_slice())
                .err()
                .is_some_and(|error| format!("{error:#}").contains("expected variant index"))
        })
        .expect("enum with over 1024 variants")
}

#[derive(Default)]
struct Samples {
    hasher: Sha256,
    count: usize,
}

impl Samples {
    fn add(&mut self, name: &str, value: &impl Serialize) {
        let bytes = encode(value);
        self.hasher.update((name.len() as u64).to_le_bytes());
        self.hasher.update(name);
        self.hasher.update(&bytes);
        self.count += 1;
    }

    /// One sample per variant of `T`. `index` is an exhaustive match that
    /// names each variant's wire index.
    fn variants<T: Serialize + DeserializeOwned>(
        &mut self,
        name: &str,
        samples: Vec<T>,
        index: impl Fn(&T) -> u32,
    ) {
        let mut covered = Vec::new();
        for sample in &samples {
            let expected = index(sample);
            let encoded = encode(sample);
            let actual = u32::from_le_bytes(encoded[4..8].try_into().unwrap());
            assert_eq!(
                actual, expected,
                "{name}: a variant moved from index {expected} to {actual}. Never reorder or insert variants; add new ones at the end"
            );
            covered.push(expected);
            self.add(&format!("{name}::{expected}"), sample);
        }
        covered.sort_unstable();
        covered.dedup();
        let count = variant_count::<T>();
        assert_eq!(
            covered,
            (0..count).collect::<Vec<_>>(),
            "{name} has {count} variants; add a sample for each one"
        );
    }

    /// One sample per flag of a group of `N` booleans, with only that flag
    /// set, so that swapping two of them changes the fingerprint.
    fn one_hot<const N: usize, T: Serialize>(&mut self, name: &str, make: impl Fn([bool; N]) -> T) {
        for index in 0..N {
            let mut flags = [false; N];
            flags[index] = true;
            self.add(&format!("{name}#{index}"), &make(flags));
        }
    }

    fn fingerprint(self) -> u64 {
        u64::from_le_bytes(self.hasher.finalize()[..8].try_into().unwrap())
    }
}

fn color(seed: u8) -> TerminalColor {
    TerminalColor {
        r: seed,
        g: seed.wrapping_add(1),
        b: seed.wrapping_add(2),
    }
}

fn size() -> TerminalSize {
    TerminalSize {
        cols: 80,
        rows: 24,
        cell_width: 9.5,
        cell_height: 18.25,
    }
}

fn query_colors() -> TerminalQueryColors {
    TerminalQueryColors {
        ansi: std::array::from_fn(|index| color(index as u8 * 3)),
        foreground: color(200),
        background: color(10),
        cursor: Some(color(30)),
    }
}

fn runtime_config() -> TerminalRuntimeConfig {
    TerminalRuntimeConfig {
        shell: Some("/bin/zsh".into()),
        windows_shell: WindowsShell::PowerShellCore,
        term: "xterm-256color".into(),
        colorterm: Some("truecolor".into()),
        term_program: TermProgram::Termy,
        environment: HashMap::from([("KEY".into(), "value".into())]),
        query_colors: query_colors(),
        working_dir_fallback: WorkingDirFallback::Home,
        scrollback_history: 5000,
        default_cursor_style: TerminalCursorStyle::Line,
    }
}

fn pane_launch() -> PaneLaunch {
    PaneLaunch {
        size: size(),
        working_directory: Some("/tmp".into()),
        shell_integration: Some(TabTitleShellIntegration {
            enabled: true,
            explicit_prefix: "termy:".into(),
        }),
        config: runtime_config(),
        launch: Some(TerminalLaunch::Program {
            program: "ssh".into(),
            args: vec!["host".into()],
        }),
    }
}

fn pane_info() -> PaneInfo {
    PaneInfo {
        id: "pane".into(),
        child_pid: Some(42),
        title: Some("title".into()),
        working_directory: Some("/tmp".into()),
        exited: true,
    }
}

fn cursor() -> TerminalCursorState {
    TerminalCursorState {
        col: 1,
        row: 2,
        style: TerminalCursorStyle::Block,
    }
}

fn render_cell() -> TerminalRenderCell {
    render_cell_with([
        true, false, true, false, true, false, true, false, true, false,
    ])
}

fn render_cell_with(flags: [bool; 10]) -> TerminalRenderCell {
    let [
        bold,
        dim,
        italic,
        inverse,
        hidden,
        strikethrough,
        hyperlink,
        wide_character_spacer,
        leading_wide_character_spacer,
        line_wrapped,
    ] = flags;
    TerminalRenderCell {
        text: TerminalRenderText::from_cell_suffix('e', Some("\u{301}")),
        foreground: TerminalRenderColor::Rgb(color(1)),
        background: TerminalRenderColor::Indexed(4),
        underline_color: Some(TerminalRenderColor::DimIndexed(2)),
        bold,
        dim,
        italic,
        underline_style: TerminalUnderlineStyle::Curly,
        inverse,
        hidden,
        strikethrough,
        hyperlink,
        wide_character_spacer,
        leading_wide_character_spacer,
        line_wrapped,
    }
}

fn mouse_mode(flags: [bool; 6]) -> TerminalMouseMode {
    let [
        enabled,
        report_click,
        report_drag,
        report_motion,
        sgr_encoding,
        utf8_encoding,
    ] = flags;
    TerminalMouseMode {
        enabled,
        report_click,
        report_drag,
        report_motion,
        sgr_encoding,
        utf8_encoding,
    }
}

fn keyboard_mode(flags: [bool; 6]) -> TerminalKeyboardMode {
    let [a, b, c, d, e, f] = flags;
    TerminalKeyboardMode::from_flags(a, b, c, d, e, f)
}

fn remote_state() -> RemoteState {
    remote_state_with([true, false, true])
}

/// `flags`: bracketed paste, alternate screen, clipboard paste events.
fn remote_state_with(flags: [bool; 3]) -> RemoteState {
    let [bracketed_paste, alternate_screen, clipboard_paste_events] = flags;
    RemoteState {
        render: TerminalRenderRead {
            metadata: TerminalViewportMetadata {
                cols: 4,
                rows: 3,
                cursor: Some(cursor()),
                display_offset: 5,
                history_size: 10,
                palette_revision: 7,
                generation: 9,
            },
            palette: TerminalPalette {
                indexed: std::array::from_fn(|index| (index % 3 == 0).then(|| color(index as u8))),
                foreground: Some(color(5)),
                background: None,
                cursor: Some(color(6)),
                revision: 7,
            },
            cells: vec![render_cell()],
            update: TerminalRenderDamageSnapshot {
                damage: TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                    row: 1,
                    left_col: 2,
                    right_col: 3,
                }]),
                scrolls: vec![TerminalViewportScroll {
                    top: 1,
                    bottom: 3,
                    count: 2,
                    direction: TerminalViewportScrollDirection::Up,
                }],
                generation: 9,
                palette_revision: 7,
            },
        },
        size: size(),
        child_pid: Some(42),
        cursor_position: (1, 2),
        mouse_mode: mouse_mode([true, true, false, true, true, false]),
        keyboard_mode: keyboard_mode([true, false, true, false, true, false]),
        bracketed_paste,
        alternate_screen,
        clipboard_paste_events,
        graphics_revision: 5,
        // A passed deadline encodes as zero delay, whenever the test runs.
        graphics_deadline: Some(Instant::now()),
        graphics: None,
    }
}

fn placement(image_id: u32, image: GraphicsImage) -> KittyGraphicsRenderPlacement {
    KittyGraphicsRenderPlacement {
        placement_serial: 11,
        image_id,
        placement_id: 3,
        image: Arc::new(image),
        image_width: 13,
        image_height: 14,
        image_generation: 12,
        animation_deadline: Some(Instant::now()),
        viewport_row: -1,
        col: 5,
        col_offset: -2,
        virtual_cell: Some((15, 16)),
        source_x: 17,
        source_y: 18,
        source_width: 19,
        source_height: 20,
        display_cols: Some(21),
        display_rows: Some(22),
        occupied_cols: 23,
        occupied_rows: 24,
        clip_top_rows: 25,
        clip_bottom_rows: 26,
        x_offset: 27,
        y_offset: 28,
        z_index: -3,
    }
}

fn png_placement() -> KittyGraphicsRenderPlacement {
    placement(2, GraphicsImage::from_png(4, 5, b"\x89PNG sample".to_vec()))
}

fn graphics_update() -> graphics::GraphicsUpdate {
    let rgba = placement(7, GraphicsImage::from_rgba(2, 3, (0..24).collect()));
    GraphicsEncoder::default().encode(&[rgba, png_placement()])
}

fn termy_cell(flags: [bool; 8]) -> TermyCell {
    let [
        uses_terminal_default_bg,
        bold,
        render_text,
        wide_character_spacer,
        line_wrapped,
        italic,
        underline,
        strikethrough,
    ] = flags;
    TermyCell {
        char: 'x',
        fg: TermyColor {
            r: 1,
            g: 2,
            b: 3,
            a: 4,
        },
        bg: TermyColor {
            r: 5,
            g: 6,
            b: 7,
            a: 8,
        },
        uses_terminal_default_bg,
        bold,
        render_text,
        wide_character_spacer,
        line_wrapped,
        italic,
        underline,
        strikethrough,
    }
}

fn frame() -> TermyFrame {
    TermyFrame {
        cols: 3,
        rows: 2,
        cells: vec![termy_cell([
            true, true, true, false, true, true, false, true,
        ])],
        cursor: Some(cursor()),
        display_offset: 1,
        history_size: 2,
    }
}

fn clipboard_content() -> TerminalClipboardContent {
    TerminalClipboardContent {
        mime_type: "text/plain".into(),
        data: b"copied".to_vec(),
    }
}

fn read_request() -> TerminalClipboardReadRequest {
    read_request_with([true, true, false])
}

fn read_request_with(flags: [bool; 3]) -> TerminalClipboardReadRequest {
    let [list_available, permission_granted, can_remember_permission] = flags;
    TerminalClipboardReadRequest {
        location: TerminalClipboardLocation::Primary,
        mime_types: vec!["text/plain".into()],
        list_available,
        name: Some("name".into()),
        permission_granted,
        can_remember_permission,
    }
}

fn write_request() -> TerminalClipboardWriteRequest {
    write_request_with([false, true])
}

fn write_request_with(flags: [bool; 2]) -> TerminalClipboardWriteRequest {
    let [permission_granted, can_remember_permission] = flags;
    TerminalClipboardWriteRequest {
        location: TerminalClipboardLocation::Clipboard,
        contents: vec![clipboard_content()],
        name: None,
        permission_granted,
        can_remember_permission,
    }
}

/// Flag groups whose fields could swap unnoticed with a single sample.
fn add_flag_groups(samples: &mut Samples) {
    samples.one_hot("TerminalRenderCell", render_cell_with);
    samples.one_hot("TerminalMouseMode", mouse_mode);
    samples.one_hot("TerminalKeyboardMode", keyboard_mode);
    samples.one_hot("RemoteState", remote_state_with);
    samples.one_hot("TermyCell", termy_cell);
    samples.one_hot("TerminalClipboardReadRequest", read_request_with);
    samples.one_hot("TerminalClipboardWriteRequest", write_request_with);
}

fn status_record() -> ProgramStatusRecord {
    ProgramStatusRecord {
        id: "build".into(),
        state: ProgramState::Working,
        kind: Some(ProgramStatusKind::Question),
        progress: Some(40),
        app: Some("cargo".into()),
        title: Some("Building".into()),
        msg: None,
    }
}

fn add_requests(samples: &mut Samples) {
    use Request as R;
    let command = || RemoteCommand::Scroll(-3);
    samples.variants(
        "Request",
        vec![
            R::RetiredHello,
            R::Create(Box::new(pane_launch())),
            R::Attach("pane".into()),
            R::RetiredSubscribe,
            R::Command {
                pane: "pane".into(),
                command: command(),
            },
            R::HostReply {
                pane: "pane".into(),
                id: 9,
                reply: RemoteHostReply::Load(Some("text".into())),
            },
            R::List,
            R::Close("pane".into()),
            R::GetLayout,
            R::SetLayout("{}".into()),
            R::Shutdown,
            R::CompareAndSetLayout {
                expected: Some("{}".into()),
                replacement: "{\"version\":1}".into(),
            },
            R::SubscribeGraphics("pane".into()),
            R::ViewportCommand {
                pane: "pane".into(),
                command: command(),
            },
            R::Authenticate {
                version: VERSION,
                client_nonce: [1; 32],
            },
            R::Proof([2; 32]),
        ],
        |request| match request {
            R::RetiredHello => 0,
            R::Create(_) => 1,
            R::Attach(_) => 2,
            R::RetiredSubscribe => 3,
            R::Command { .. } => 4,
            R::HostReply { .. } => 5,
            R::List => 6,
            R::Close(_) => 7,
            R::GetLayout => 8,
            R::SetLayout(_) => 9,
            R::Shutdown => 10,
            R::CompareAndSetLayout { .. } => 11,
            R::SubscribeGraphics(_) => 12,
            R::ViewportCommand { .. } => 13,
            R::Authenticate { .. } => 14,
            R::Proof(_) => 15,
        },
    );
}

fn add_responses(samples: &mut Samples) {
    use Response as R;
    samples.variants(
        "Response",
        vec![
            R::Ok,
            R::Attached(pane_info(), Arc::new(remote_state())),
            R::Reply(RemoteReply::Changed(true)),
            R::Panes(vec![pane_info()]),
            R::Layout(Some("{}".into())),
            R::Error("failed".into()),
            R::LayoutUpdated(true),
            R::Viewport {
                changed: true,
                generation: 11,
            },
            R::Challenge {
                server_nonce: [3; 32],
                server_proof: [4; 32],
            },
        ],
        |response| match response {
            R::Ok => 0,
            R::Attached(..) => 1,
            R::Reply(_) => 2,
            R::Panes(_) => 3,
            R::Layout(_) => 4,
            R::Error(_) => 5,
            R::LayoutUpdated(_) => 6,
            R::Viewport { .. } => 7,
            R::Challenge { .. } => 8,
        },
    );
    samples.variants(
        "Update",
        vec![
            Update::State(Arc::new(remote_state())),
            Update::Events(vec![TerminalEvent::Bell]),
            Update::Host {
                id: 3,
                request: RemoteHostRequest::Read(read_request()),
            },
            Update::GraphicsState(Arc::new(remote_state()), graphics_update()),
        ],
        |update| match update {
            Update::State(_) => 0,
            Update::Events(_) => 1,
            Update::Host { .. } => 2,
            Update::GraphicsState(..) => 3,
        },
    );
}

fn add_remote(samples: &mut Samples) {
    use RemoteCommand as C;
    samples.variants(
        "RemoteCommand",
        vec![
            C::Write(b"input".to_vec()),
            C::FeedOutput(b"output".to_vec()),
            C::HydrateOutput(b"restored".to_vec()),
            C::Resize(size()),
            C::NudgeResize,
            C::Scroll(-3),
            C::ScrollToBottom,
            C::ClearScrollback,
            C::SetOptions(TerminalOptions {
                scrollback_history: 300,
                default_cursor_style: TerminalCursorStyle::Line,
            }),
            C::SetScrollback(400),
            C::SetQueryColors(query_colors()),
            C::Snapshot,
            C::Lines { first: -5, last: 2 },
            C::Search {
                query: "needle".into(),
                options: TermySearchOptions {
                    case_sensitive: true,
                    regex: false,
                },
            },
            C::Hyperlink { row: 1, col: 2 },
            C::Link { row: 3, col: 4 },
            C::Graphics,
            C::ClipboardPaste {
                location: TerminalClipboardLocation::Primary,
                formats: vec!["text/plain".into()],
            },
        ],
        |command| match command {
            C::Write(_) => 0,
            C::FeedOutput(_) => 1,
            C::HydrateOutput(_) => 2,
            C::Resize(_) => 3,
            C::NudgeResize => 4,
            C::Scroll(_) => 5,
            C::ScrollToBottom => 6,
            C::ClearScrollback => 7,
            C::SetOptions(_) => 8,
            C::SetScrollback(_) => 9,
            C::SetQueryColors(_) => 10,
            C::Snapshot => 11,
            C::Lines { .. } => 12,
            C::Search { .. } => 13,
            C::Hyperlink { .. } => 14,
            C::Link { .. } => 15,
            C::Graphics => 16,
            C::ClipboardPaste { .. } => 17,
        },
    );
    use RemoteReply as R;
    samples.variants(
        "RemoteReply",
        vec![
            R::Ok,
            R::Changed(true),
            R::Frame(frame()),
            R::Lines {
                bounds: (-5, 2, 7),
                cells: vec![render_cell()],
            },
            R::Search(vec![TermySearchMatch {
                row: 1,
                start_col: 2,
                end_col: 3,
                line: "a needle".into(),
            }]),
            R::Hyperlink(Some(DetectedLink {
                start_col: 1,
                end_col: 9,
                target: "https://example.com".into(),
            })),
            R::Link(Some(DetectedViewportLink {
                start_row: 1,
                start_col: 2,
                end_row: 3,
                end_col: 4,
                target: "file:///tmp".into(),
            })),
            R::Graphics(6, vec![png_placement()]),
        ],
        |reply| match reply {
            R::Ok => 0,
            R::Changed(_) => 1,
            R::Frame(_) => 2,
            R::Lines { .. } => 3,
            R::Search(_) => 4,
            R::Hyperlink(_) => 5,
            R::Link(_) => 6,
            R::Graphics(..) => 7,
        },
    );
    samples.variants(
        "RemoteHostRequest",
        vec![
            RemoteHostRequest::Load(TerminalClipboardTarget::Selection),
            RemoteHostRequest::Read(read_request()),
            RemoteHostRequest::Write(write_request()),
        ],
        |request| match request {
            RemoteHostRequest::Load(_) => 0,
            RemoteHostRequest::Read(_) => 1,
            RemoteHostRequest::Write(_) => 2,
        },
    );
    samples.variants(
        "RemoteHostReply",
        vec![
            RemoteHostReply::Load(Some("text".into())),
            RemoteHostReply::Read(TerminalClipboardReadResult::Denied),
            RemoteHostReply::Write(TerminalClipboardWriteResult::Busy),
        ],
        |reply| match reply {
            RemoteHostReply::Load(_) => 0,
            RemoteHostReply::Read(_) => 1,
            RemoteHostReply::Write(_) => 2,
        },
    );
    samples.add("PaneLaunch", &pane_launch());
    samples.add("PaneInfo", &pane_info());
    samples.add("RemoteState", &remote_state());
}

fn add_events(samples: &mut Samples) {
    use TerminalEvent as E;
    samples.variants(
        "TerminalEvent",
        vec![
            E::Wakeup,
            E::Title("title".into()),
            E::ResetTitle,
            E::Bell,
            E::Exit,
            E::InputDropped,
            E::ClipboardStore("copied".into()),
            E::ShellPromptStart,
            E::ShellCommandStart,
            E::ShellCommandExecuting,
            E::ShellCommandFinished(Some(1)),
            E::Progress(ProgressState::Warning(30)),
            E::WorkingDirectory("/tmp".into()),
            E::ProgramStatus(vec![status_record()]),
        ],
        |event| match event {
            E::Wakeup => 0,
            E::Title(_) => 1,
            E::ResetTitle => 2,
            E::Bell => 3,
            E::Exit => 4,
            E::InputDropped => 5,
            E::ClipboardStore(_) => 6,
            E::ShellPromptStart => 7,
            E::ShellCommandStart => 8,
            E::ShellCommandExecuting => 9,
            E::ShellCommandFinished(_) => 10,
            E::Progress(_) => 11,
            E::WorkingDirectory(_) => 12,
            E::ProgramStatus(_) => 13,
        },
    );
}

/// Enums that only appear inside the wire types above.
fn add_embedded_enums(samples: &mut Samples) {
    samples.variants(
        "TerminalLaunch",
        vec![
            TerminalLaunch::ShellCommand("make".into()),
            TerminalLaunch::Program {
                program: "ssh".into(),
                args: vec!["host".into()],
            },
        ],
        |launch| match launch {
            TerminalLaunch::ShellCommand(_) => 0,
            TerminalLaunch::Program { .. } => 1,
        },
    );
    samples.variants(
        "TermProgram",
        vec![TermProgram::Termy, TermProgram::Ghostty],
        |program| match program {
            TermProgram::Termy => 0,
            TermProgram::Ghostty => 1,
        },
    );
    use WindowsShell as W;
    samples.variants(
        "WindowsShell",
        vec![
            W::Cmd,
            W::PowerShell,
            W::PowerShellCore,
            W::GitBash,
            W::Auto,
        ],
        |shell| match shell {
            W::Cmd => 0,
            W::PowerShell => 1,
            W::PowerShellCore => 2,
            W::GitBash => 3,
            W::Auto => 4,
        },
    );
    samples.variants(
        "WorkingDirFallback",
        vec![WorkingDirFallback::Home, WorkingDirFallback::Process],
        |fallback| match fallback {
            WorkingDirFallback::Home => 0,
            WorkingDirFallback::Process => 1,
        },
    );
    samples.variants(
        "TerminalCursorStyle",
        vec![TerminalCursorStyle::Line, TerminalCursorStyle::Block],
        |style| match style {
            TerminalCursorStyle::Line => 0,
            TerminalCursorStyle::Block => 1,
        },
    );
    use TerminalRenderColor as C;
    samples.variants(
        "TerminalRenderColor",
        vec![
            C::DefaultForeground,
            C::DefaultBackground,
            C::Cursor,
            C::Indexed(1),
            C::DimIndexed(2),
            C::BrightForeground,
            C::DimForeground,
            C::Rgb(color(3)),
        ],
        |color| match color {
            C::DefaultForeground => 0,
            C::DefaultBackground => 1,
            C::Cursor => 2,
            C::Indexed(_) => 3,
            C::DimIndexed(_) => 4,
            C::BrightForeground => 5,
            C::DimForeground => 6,
            C::Rgb(_) => 7,
        },
    );
    use TerminalUnderlineStyle as U;
    samples.variants(
        "TerminalUnderlineStyle",
        vec![
            U::None,
            U::Single,
            U::Double,
            U::Curly,
            U::Dotted,
            U::Dashed,
        ],
        |style| match style {
            U::None => 0,
            U::Single => 1,
            U::Double => 2,
            U::Curly => 3,
            U::Dotted => 4,
            U::Dashed => 5,
        },
    );
    samples.variants(
        "TerminalDamageSnapshot",
        vec![
            TerminalDamageSnapshot::Full,
            TerminalDamageSnapshot::Partial(vec![TerminalDirtySpan {
                row: 1,
                left_col: 2,
                right_col: 3,
            }]),
        ],
        |damage| match damage {
            TerminalDamageSnapshot::Full => 0,
            TerminalDamageSnapshot::Partial(_) => 1,
        },
    );
    samples.variants(
        "TerminalViewportScrollDirection",
        vec![
            TerminalViewportScrollDirection::Up,
            TerminalViewportScrollDirection::Down,
        ],
        |direction| match direction {
            TerminalViewportScrollDirection::Up => 0,
            TerminalViewportScrollDirection::Down => 1,
        },
    );
    samples.variants(
        "TerminalClipboardLocation",
        vec![
            TerminalClipboardLocation::Clipboard,
            TerminalClipboardLocation::Primary,
        ],
        |location| match location {
            TerminalClipboardLocation::Clipboard => 0,
            TerminalClipboardLocation::Primary => 1,
        },
    );
    samples.variants(
        "TerminalClipboardTarget",
        vec![
            TerminalClipboardTarget::Clipboard,
            TerminalClipboardTarget::Selection,
        ],
        |target| match target {
            TerminalClipboardTarget::Clipboard => 0,
            TerminalClipboardTarget::Selection => 1,
        },
    );
    use TerminalClipboardReadResult as Read;
    samples.variants(
        "TerminalClipboardReadResult",
        vec![
            Read::Success {
                available_formats: vec!["text/plain".into()],
                contents: vec![clipboard_content()],
                remember_permission: true,
            },
            Read::Denied,
            Read::Unsupported,
            Read::Busy,
            Read::IoError,
        ],
        |result| match result {
            Read::Success { .. } => 0,
            Read::Denied => 1,
            Read::Unsupported => 2,
            Read::Busy => 3,
            Read::IoError => 4,
        },
    );
    use TerminalClipboardWriteResult as Write;
    samples.variants(
        "TerminalClipboardWriteResult",
        vec![
            Write::Success {
                remember_permission: true,
            },
            Write::Denied,
            Write::Unsupported,
            Write::Busy,
            Write::InvalidData,
            Write::IoError,
        ],
        |result| match result {
            Write::Success { .. } => 0,
            Write::Denied => 1,
            Write::Unsupported => 2,
            Write::Busy => 3,
            Write::InvalidData => 4,
            Write::IoError => 5,
        },
    );
    use ProgressState as P;
    samples.variants(
        "ProgressState",
        vec![
            P::Clear,
            P::InProgress(1),
            P::Error(2),
            P::Indeterminate,
            P::Warning(3),
        ],
        |state| match state {
            P::Clear => 0,
            P::InProgress(_) => 1,
            P::Error(_) => 2,
            P::Indeterminate => 3,
            P::Warning(_) => 4,
        },
    );
    use ProgramState as S;
    samples.variants(
        "ProgramState",
        vec![S::Idle, S::Working, S::Done, S::Blocked, S::Error],
        |state| match state {
            S::Idle => 0,
            S::Working => 1,
            S::Done => 2,
            S::Blocked => 3,
            S::Error => 4,
        },
    );
    use ProgramStatusKind as K;
    samples.variants(
        "ProgramStatusKind",
        vec![K::Permission, K::Question, K::Auth],
        |kind| match kind {
            K::Permission => 0,
            K::Question => 1,
            K::Auth => 2,
        },
    );
}

#[test]
fn wire_format_matches_its_fingerprint() {
    let mut samples = Samples::default();
    add_requests(&mut samples);
    add_responses(&mut samples);
    add_remote(&mut samples);
    add_events(&mut samples);
    add_embedded_enums(&mut samples);
    add_flag_groups(&mut samples);
    assert!(samples.count > 100);
    let fingerprint = samples.fingerprint();
    assert!(
        fingerprint == WIRE_FINGERPRINT,
        "The multiplexer wire format changed (fingerprint {fingerprint:#018x}, expected {WIRE_FINGERPRINT:#018x}). \
         A running host of the same VERSION cannot decode it. Bump VERSION in \
         crates/core/src/multiplexer/protocol.rs, unless this unreleased change already \
         bumped it, then set WIRE_FINGERPRINT to {fingerprint:#018x}."
    );
}

#[test]
fn samples_encode_the_same_every_time() {
    // Guards the fingerprint against samples that depend on time or order.
    assert_eq!(encode(&remote_state()), encode(&remote_state()));
    assert_eq!(encode(&graphics_update()), encode(&graphics_update()));
    assert_eq!(variant_count::<Request>(), 16);
}
