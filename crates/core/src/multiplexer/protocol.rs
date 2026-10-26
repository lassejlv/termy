use crate::{remote::*, *};
use anyhow::{Context, ensure};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::sync::Arc;

// Version 2 adds the OSC 7501 snapshot event to the bincode event enum.
// Version 3 adds TERM_PROGRAM identity to PaneLaunch's binary runtime config.
// Version 4 requires mutual authentication, retires the legacy capability
// fallbacks and freezes the shutdown handshake (see `Request`). It also
// sends a subscription's replayed state as `Update::Replay`, gives the
// session layout a revision and adds layout watching. It routes clipboard
// requests only to host-capable subscriptions, with an expiry, and adds the
// subscription-free `Snapshot` and `Size` requests, frame deltas, cached
// images in messages of their own, paused subscriptions, text history
// queries, motion reports and anchored link queries. It also reports
// rejected input, each pane's shell-integration state and launch settings,
// and the working directory a pane actually started in.
pub(crate) const VERSION: u32 = 4;
/// The largest message either side accepts. One streamed image (at most the
/// engine's image quota) fits with room to spare; every other message stays
/// far smaller, because frames are deltas and history replies are chunked.
pub(crate) const MAX_MESSAGE_BYTES: u64 =
    crate::kitty_graphics::MAX_IMAGE_BYTES as u64 + 16 * 1024 * 1024;

/// Hash of the encoding of every wire type, checked by the `wire_golden`
/// test. When it changes, the wire changed: bump [`VERSION`] (unless the
/// unreleased change already did) and update this value.
pub(crate) const WIRE_FINGERPRINT: u64 = 0xc2ed_3f3a_6917_9273;

/// Identifies the build that wrote `endpoint.json`. A client logs a warning
/// when it differs from its own under the same [`VERSION`], which points at
/// a wire change that missed a [`VERSION`] bump.
pub(crate) fn build_id() -> String {
    format!(
        "termy_core {} wire {WIRE_FINGERPRINT:016x}",
        env!("CARGO_PKG_VERSION")
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneLaunch {
    pub size: TerminalSize,
    /// Should be absolute: the host resolves a relative path against its own
    /// directory, the user's home directory, not against the client's.
    pub working_directory: Option<String>,
    pub shell_integration: Option<TabTitleShellIntegration>,
    pub config: TerminalRuntimeConfig,
    pub launch: Option<TerminalLaunch>,
    /// The environment the pane's program starts from, like a local tab
    /// started by this client: normally [`PaneLaunch::client_environment`].
    /// Termy's terminal variables, `config.environment` and `TERMY_MUX_DIR`
    /// and `TERMY_MUX_PANE` apply on top. `None`, for clients that do not
    /// send one, starts from the host's own environment, which holds only the
    /// login session's variables.
    pub environment: Option<Vec<(String, String)>>,
}

impl PaneLaunch {
    /// A value for [`PaneLaunch::working_directory`]: `configured`, or the
    /// `fallback`, resolved in this process as for a local tab (`~` with this
    /// process's home directory, a relative path against its directory) and
    /// made absolute, since the host runs elsewhere. `None` only when no
    /// directory can be found at all.
    pub fn client_working_directory(
        configured: Option<&str>,
        fallback: WorkingDirFallback,
    ) -> Option<String> {
        let directory = crate::resolve_launch_working_directory(configured, fallback)?;
        let directory = std::path::absolute(&directory).unwrap_or(directory);
        Some(directory.to_string_lossy().into_owned())
    }

    /// This process's environment, for [`PaneLaunch::environment`]. Variables
    /// whose name or value is not UTF-8 are left out.
    pub fn client_environment() -> Vec<(String, String)> {
        let mut dropped = 0;
        let environment = std::env::vars_os()
            .filter_map(|(name, value)| {
                let variable = name.into_string().ok().zip(value.into_string().ok());
                dropped += usize::from(variable.is_none());
                variable
            })
            .collect();
        if dropped > 0 {
            log::debug!("{dropped} environment variables are not UTF-8 and do not reach new panes");
        }
        environment
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: String,
    pub child_pid: Option<u32>,
    pub title: Option<String>,
    /// The directory the pane started in, then the latest one its shell
    /// reported (OSC 7).
    pub working_directory: Option<String>,
    pub exited: bool,
    /// The shell's command lifecycle; `None` until the pane reported an
    /// OSC 133 event, which needs shell integration.
    pub shell: Option<ShellState>,
}

/// A pane's command lifecycle, from OSC 133 shell integration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellState {
    /// A command was submitted (`C`) and has not finished yet.
    pub running: bool,
    /// The number of commands that started (`C`).
    pub commands_started: u64,
    /// The number of commands that finished (`D`).
    pub commands_finished: u64,
    /// The exit code the last finished command reported.
    pub exit_code: Option<i32>,
    /// The command that the latest input started; `None` before any input.
    pub input_command: Option<InputCommand>,
}

/// What became of the command that a pane's latest input started. A finish
/// counts only after a start (`C`) that followed the input, so neither a
/// command that ran when the input arrived nor a prompt's stray `D` (fish
/// prints one at its first prompt) is taken for it. A shell that never
/// reports `C` (PowerShell) finishes it with the first `D` after a prompt
/// that showed when, or after, the input arrived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputCommand {
    /// Input arrived; no command it started has been reported yet.
    Pending,
    Running,
    /// The command finished with this exit code, if it reported one.
    Finished(Option<i32>),
}

/// The settings a pane was started with that a pane split from it reuses:
/// the shell, terminal identity, scrollback, cursor style and shell
/// integration, without the program, directory or environment of its
/// launch (see [`Request::LaunchConfig`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub config: TerminalRuntimeConfig,
    pub shell_integration: Option<TabTitleShellIntegration>,
}

/// Wire rules for [`Request`], [`Response`], [`Update`] and the remote types
/// they carry:
///
/// - Never reorder or remove a variant; add new ones at the end of the enum.
///   A retired variant stays as a unit placeholder so later indices keep.
/// - Frozen frames: any client must be able to shut down any host, of any
///   [`VERSION`], with frames that `escape` encodes by hand (audit B1). These
///   variants keep their index and field shape forever, as does the framing
///   (4-byte little-endian length, then fixint bincode): `Request::Shutdown`
///   (10), `Request::Authenticate` (14), `Request::Proof` (15), `Response::Ok`
///   (0), `Response::Error` (5) and `Response::Challenge` (8). The test
///   `frozen_frames_match_the_wire_enums` checks them byte for byte.
/// - Any other wire change changes the `wire_golden` fingerprint; follow its
///   failure message.
#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    /// Retired: the plaintext-token greeting of VERSION 1 and early VERSION 2
    /// hosts. Only `escape` still sends it, and only to those hosts.
    RetiredHello,
    Create(Box<PaneLaunch>),
    Attach(String),
    /// Retired: the subscription without the graphics stream.
    RetiredSubscribe,
    Command {
        pane: String,
        command: RemoteCommand,
    },
    HostReply {
        pane: String,
        id: u64,
        reply: RemoteHostReply,
    },
    List,
    Close(String),
    /// Answered with [`Response::Layout`].
    GetLayout,
    /// Replaces the layout unconditionally; answered with
    /// [`Response::LayoutUpdated`].
    SetLayout(String),
    /// Frozen (index 10).
    Shutdown,
    /// Replaces the layout only while it is still at revision `expected`.
    CompareAndSetLayout {
        expected: u64,
        replacement: String,
    },
    /// Retired: the subscription without the host-capable flag.
    RetiredSubscribeGraphics,
    /// Scroll or clear the viewport, publish the new frame immediately and
    /// reply with its generation.
    ViewportCommand {
        pane: String,
        command: RemoteCommand,
    },
    /// Frozen (index 14). Mutual authentication: the host must answer with
    /// [`Response::Challenge`] proving it knows the token before the client
    /// sends anything else. The token itself never crosses the wire.
    Authenticate {
        version: u32,
        client_nonce: [u8; 32],
    },
    /// Frozen (index 15). The client's proof of the token, answering
    /// [`Response::Challenge`].
    Proof([u8; 32]),
    /// Turns the connection into a stream of [`Response::LayoutChanged`]:
    /// the current revision first, then one message per accepted write.
    WatchLayout,
    /// Stream the pane's frames, graphics and events on this connection.
    /// Only a `host_capable` subscriber, a view that can ask the user, is
    /// sent the pane's clipboard requests.
    Subscribe {
        pane: String,
        host_capable: bool,
    },
    /// The pane's current state, as [`Response::Attached`], without
    /// subscribing and without publishing a frame to subscribers.
    Snapshot(String),
    /// The pane's size and cell metrics, as [`Response::Size`].
    Size(String),
    /// Sent on a subscription's own connection, after [`Request::Subscribe`]:
    /// `true` while the view is hidden. The host then sends events and
    /// clipboard requests, but no frames, and the latest frame once on
    /// resume (audit I7). The host does not answer it.
    SetPaused(bool),
    /// Sent on a subscription's own connection when the client cannot apply
    /// a frame (it lost the frame or an image the delta builds on). The host
    /// sends the next frame in full, with its images, and the client ignores
    /// deltas until then instead of disconnecting.
    Resync,
    /// The pane's [`LaunchConfig`], as [`Response::LaunchConfig`].
    LaunchConfig(String),
}

/// See [`Request`] for the wire rules and the frozen variants.
#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    /// Frozen (index 0).
    Ok,
    Attached(PaneInfo, Arc<RemoteState>),
    Reply(RemoteReply),
    Panes(Vec<PaneInfo>),
    /// The layout and its revision: 0 before any layout was stored, then
    /// one more for every accepted write.
    Layout {
        revision: u64,
        layout: Option<String>,
    },
    /// Frozen (index 5).
    Error(String),
    /// Whether a layout write was applied, and the revision now current.
    LayoutUpdated {
        updated: bool,
        revision: u64,
    },
    Viewport {
        changed: bool,
        generation: u64,
    },
    /// Frozen (index 8).
    Challenge {
        server_nonce: [u8; 32],
        server_proof: [u8; 32],
    },
    /// Sent on a [`Request::WatchLayout`] stream.
    LayoutChanged {
        revision: u64,
    },
    Size(TerminalSize),
    LaunchConfig(Box<LaunchConfig>),
}

/// See [`Request`] for the wire rules.
#[derive(Serialize, Deserialize)]
pub(crate) enum Update {
    /// A pane's new state inside the host; clients receive [`Update::Frame`].
    State(Arc<RemoteState>),
    Events(Vec<TerminalEvent>),
    /// Retired: the clipboard request without an expiry.
    RetiredHost,
    /// Retired: the full viewport with every frame.
    RetiredGraphicsState,
    /// The pane's current state, sent once before any live [`Update::Events`]
    /// of a new subscription. Replayed events describe the state at attach
    /// time; they did not just happen.
    Replay(Vec<TerminalEvent>),
    /// A clipboard request to answer with [`Request::HostReply`]. The host
    /// denies it once `deadline` passes, so the client drops it unanswered
    /// after that instead of prompting or writing late.
    HostRequest {
        id: u64,
        request: RemoteHostRequest,
        #[serde(with = "crate::serde_deadline")]
        deadline: Option<std::time::Instant>,
    },
    /// A frame: the rows changed since the frame sent before on this
    /// subscription, or the whole viewport (audit I1), with its images.
    Frame(
        Box<crate::remote::frame::FrameUpdate>,
        graphics::GraphicsUpdate,
    ),
    /// The pixels of an image a later [`Update::Frame`] shows, sent once
    /// while both ends cache it (audit I2).
    Image(graphics::ImageUpload),
    /// Confirms [`Request::SetPaused`], in order with the frames: none
    /// follows a `Paused(true)` until a `Paused(false)`. A paused client
    /// drops the cached images its shown frame does not use; on resume both
    /// ends start their image caches afresh.
    Paused(bool),
}

const HOST_PROOF_LABEL: &[u8] = b"termy-multiplexer host proof v1";
const CLIENT_PROOF_LABEL: &[u8] = b"termy-multiplexer client proof v1";

/// The host's answer to a client nonce: HMAC-SHA256 keyed by the token.
pub(crate) fn host_proof(
    token: &str,
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
) -> [u8; 32] {
    hmac_sha256(
        token.as_bytes(),
        &[HOST_PROOF_LABEL, client_nonce, server_nonce],
    )
}

/// The client's answer to the host nonce. A distinct label keeps either side
/// from reflecting the other's proof.
pub(crate) fn client_proof(
    token: &str,
    client_nonce: &[u8; 32],
    server_nonce: &[u8; 32],
) -> [u8; 32] {
    hmac_sha256(
        token.as_bytes(),
        &[CLIENT_PROOF_LABEL, server_nonce, client_nonce],
    )
}

fn hmac_sha256(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    const BLOCK: usize = 64;
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|byte| byte ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|byte| byte ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Compares secrets without exiting early on the first differing byte.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Events whose latest value is the pane's current state, kept by the host
/// so a reattaching client can be given the current title, directory and
/// progress. The host also keeps the shell phase (key 3): while it is
/// `ShellCommandExecuting`, progress and status are never expired as stale.
/// The client does not pass replayed phases to the view. `ShellCommandFinished`
/// is not state: replaying it would report a command that finished long ago
/// (audit I18); it clears the kept phase instead. Program status snapshots are
/// state too, but the pane keeps them per record.
pub(crate) fn sticky_key(event: &TerminalEvent) -> Option<u8> {
    match event {
        TerminalEvent::ShellPromptStart
        | TerminalEvent::ShellCommandStart
        | TerminalEvent::ShellCommandExecuting => Some(3),
        TerminalEvent::ProgramStatus(_) => None,
        event => coalesce_key(event),
    }
}

/// Events in a live queue that a newer event of the same kind replaces.
/// OSC 133 events are not merged: each one is a step of the command
/// lifecycle, and the shell integrations print `D` and `A` back to back.
fn coalesce_key(event: &TerminalEvent) -> Option<u8> {
    match event {
        TerminalEvent::Title(_) | TerminalEvent::ResetTitle => Some(0),
        TerminalEvent::WorkingDirectory(_) => Some(1),
        TerminalEvent::Progress(_) => Some(2),
        TerminalEvent::ProgramStatus(_) => Some(4),
        _ => None,
    }
}

fn is_shell_integration(event: &TerminalEvent) -> bool {
    matches!(
        event,
        TerminalEvent::ShellPromptStart
            | TerminalEvent::ShellCommandStart
            | TerminalEvent::ShellCommandExecuting
            | TerminalEvent::ShellCommandFinished(_)
    )
}

/// OSC 133 events a queue keeps for a client that is not draining: 16 full
/// prompt cycles. The oldest cycles are dropped first, so the latest commands
/// still report their exit codes.
const MAX_QUEUED_SHELL_EVENTS: usize = 64;

/// Drops the oldest OSC 133 events of `queue` up to and including the first
/// prompt start, so no queued `D` is left without the `C` that timed it.
/// Without a prompt start, drops only the oldest one.
fn evict_oldest_shell_cycle(queue: &mut Vec<TerminalEvent>) {
    if queue
        .iter()
        .any(|event| matches!(event, TerminalEvent::ShellPromptStart))
    {
        let mut evicted = false;
        queue.retain(|event| {
            if evicted || !is_shell_integration(event) {
                return true;
            }
            evicted = matches!(event, TerminalEvent::ShellPromptStart);
            false
        });
    } else if let Some(oldest) = queue.iter().position(is_shell_integration) {
        queue.remove(oldest);
    }
}

/// Prepares `queue`, the undelivered events of a client that may not be
/// draining, for `event`: a newer state replaces the queued one, only the
/// latest clipboard store matters and only the latest OSC 133 events are
/// kept. Returns false when `event` repeats one that is already queued (a
/// bell rings once until the client catches up). This keeps the queue
/// bounded by the number of event kinds plus [`MAX_QUEUED_SHELL_EVENTS`].
pub(crate) fn coalesce_event(queue: &mut Vec<TerminalEvent>, event: &TerminalEvent) -> bool {
    if let Some(key) = coalesce_key(event) {
        queue.retain(|old| coalesce_key(old) != Some(key));
    }
    match event {
        TerminalEvent::Bell | TerminalEvent::InputDropped | TerminalEvent::Exit => !queue
            .iter()
            .any(|old| std::mem::discriminant(old) == std::mem::discriminant(event)),
        TerminalEvent::ClipboardStore(_) => {
            queue.retain(|old| !matches!(old, TerminalEvent::ClipboardStore(_)));
            true
        }
        event if is_shell_integration(event) => {
            if queue.iter().filter(|old| is_shell_integration(old)).count()
                >= MAX_QUEUED_SHELL_EVENTS
            {
                evict_oldest_shell_cycle(queue);
            }
            true
        }
        _ => true,
    }
}

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_MESSAGE_BYTES)
        .reject_trailing_bytes()
}

/// `message` framed for the wire: its length, then its body. Fails, without
/// writing anything, when it cannot be encoded or exceeds the limit.
pub(crate) fn encode_message(message: &impl Serialize) -> anyhow::Result<Vec<u8>> {
    encode_message_limited(message, MAX_MESSAGE_BYTES)
}

fn encode_message_limited(message: &impl Serialize, limit: u64) -> anyhow::Result<Vec<u8>> {
    let encoding_error = |error: bincode::Error| match *error {
        bincode::ErrorKind::SizeLimit => anyhow::anyhow!("multiplexer message too large"),
        error => anyhow::Error::new(error).context("encode multiplexer message"),
    };
    // Size the body first, so a large image is copied into a buffer of the
    // right size once instead of growing it step by step.
    let size = codec()
        .with_limit(limit.min(MAX_MESSAGE_BYTES))
        .serialized_size(message)
        .map_err(encoding_error)?;
    let mut frame = Vec::with_capacity(4 + size as usize);
    frame.extend_from_slice(&(size as u32).to_le_bytes());
    codec()
        .with_no_limit()
        .serialize_into(&mut frame, message)
        .map_err(encoding_error)?;
    ensure!(
        frame.len() as u64 == 4 + size,
        "multiplexer message changed while it was encoded"
    );
    Ok(frame)
}

/// Writes a frame from [`encode_message`] in one write, so that the length
/// and the body do not travel as separate packets.
pub(crate) fn write_frame(writer: &mut impl Write, frame: &[u8]) -> anyhow::Result<()> {
    writer.write_all(frame)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn write_message(
    writer: &mut impl Write,
    message: &impl Serialize,
) -> anyhow::Result<()> {
    write_frame(writer, &encode_message(message)?)
}

/// Encodes `response` before writing it. A reply that cannot be encoded, or
/// is too large, is answered with [`Response::Error`] instead, so the client
/// gets an error rather than a closed connection (audit B13).
pub(crate) fn write_response(writer: &mut impl Write, response: &Response) -> anyhow::Result<()> {
    write_response_limited(writer, response, MAX_MESSAGE_BYTES)
}

fn write_response_limited(
    writer: &mut impl Write,
    response: &Response,
    limit: u64,
) -> anyhow::Result<()> {
    // Bincode performs sizing and writing passes. Keep the PNG exports of a
    // `RemoteReply::Graphics` snapshot alive across both, so each image is
    // encoded once; the graphics stream sends raw image generations instead.
    let _exports = match response {
        Response::Reply(RemoteReply::Graphics(_, placements)) => {
            Some(crate::remote::serde_image::retain_png_exports(placements))
        }
        _ => None,
    };
    let frame = match encode_message_limited(response, limit) {
        Ok(frame) => frame,
        Err(error) => {
            log::warn!("session reply could not be sent: {error:#}");
            encode_message(&Response::Error(format!("{error:#}")))?
        }
    };
    write_frame(writer, &frame)
}

pub(crate) fn read_message<T: DeserializeOwned>(reader: &mut impl Read) -> anyhow::Result<T> {
    read_message_limited(reader, MAX_MESSAGE_BYTES)
}

pub(crate) fn read_message_limited<T: DeserializeOwned>(
    reader: &mut impl Read,
    limit: u64,
) -> anyhow::Result<T> {
    let mut size = [0; 4];
    reader.read_exact(&mut size)?;
    let size = u32::from_le_bytes(size) as usize;
    ensure!(
        size as u64 <= limit.min(MAX_MESSAGE_BYTES),
        "multiplexer message too large"
    );
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    codec()
        .deserialize(&bytes)
        .context("decode multiplexer message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_engine::{Engine, Options, Size};

    fn shared_image_reply() -> (RemoteReply, Arc<GraphicsImage>) {
        let mut engine = Engine::new(Size { cols: 10, rows: 4 }, Options::default());
        engine.feed(b"\x1b_Ga=T,f=32,s=1,v=1,i=1,c=1,r=1,C=1,q=2;AQID/w==\x1b\\");
        let placement = engine.graphics_placements().pop().unwrap();
        let image = placement.image.clone();
        (RemoteReply::Graphics(7, vec![placement; 10]), image)
    }

    #[test]
    fn legacy_graphics_response_encodes_once_across_bincode_passes() {
        let (reply, image) = shared_image_reply();
        let mut wire = Vec::new();
        write_response(&mut wire, &Response::Reply(reply)).unwrap();
        assert_eq!(image.png_encoding_count(), 1);
        let decoded: Response = read_message(&mut wire.as_slice()).unwrap();
        let Response::Reply(RemoteReply::Graphics(revision, placements)) = decoded else {
            panic!("graphics response should keep its legacy wire shape");
        };
        assert_eq!(revision, 7);
        assert_eq!(placements.len(), 10);
        assert!(
            placements
                .iter()
                .all(|p| p.image.png().starts_with(b"\x89PNG"))
        );
    }

    #[test]
    fn direct_legacy_serialization_shares_exports_within_each_pass() {
        let (reply, image) = shared_image_reply();
        let wire = bincode::serialize(&reply).unwrap();
        // Two passes encode twice, not once for each of the ten placements.
        assert_eq!(image.png_encoding_count(), 2);
        let decoded: RemoteReply = bincode::deserialize(&wire).unwrap();
        assert!(matches!(decoded, RemoteReply::Graphics(7, placements) if placements.len() == 10));
    }

    /// Audit B13: a reply that cannot be sent becomes an error reply, and
    /// the connection stays usable.
    #[test]
    fn oversized_replies_are_answered_with_an_error() {
        let huge = Response::Layout {
            revision: 1,
            layout: Some("x".repeat(4096)),
        };
        let mut wire = Vec::new();
        write_response_limited(&mut wire, &huge, 1024).unwrap();
        write_response(&mut wire, &Response::Ok).unwrap();
        let mut reader = wire.as_slice();
        let Response::Error(error) = read_message(&mut reader).unwrap() else {
            panic!("expected an error reply");
        };
        assert!(error.contains("too large"), "{error}");
        assert!(matches!(read_message(&mut reader).unwrap(), Response::Ok));
    }

    fn queue(events: impl IntoIterator<Item = TerminalEvent>) -> Vec<TerminalEvent> {
        let mut queue = Vec::new();
        for event in events {
            if coalesce_event(&mut queue, &event) {
                queue.push(event);
            }
        }
        queue
    }

    #[test]
    fn command_finish_and_next_prompt_in_one_write_both_stay_queued() {
        // The bundled integrations print `133;D;1` and `133;A` back to back.
        let queued = queue([
            TerminalEvent::ShellCommandExecuting,
            TerminalEvent::ShellCommandFinished(Some(1)),
            TerminalEvent::ShellPromptStart,
        ]);
        assert!(
            matches!(
                queued.as_slice(),
                [
                    TerminalEvent::ShellCommandExecuting,
                    TerminalEvent::ShellCommandFinished(Some(1)),
                    TerminalEvent::ShellPromptStart,
                ]
            ),
            "{queued:?}"
        );
    }

    #[test]
    fn prompt_floods_keep_the_latest_shell_events_and_other_state() {
        let mut events = vec![TerminalEvent::Title("title".into()), TerminalEvent::Bell];
        for code in 0..10_000 {
            events.extend([
                TerminalEvent::ShellPromptStart,
                TerminalEvent::ShellCommandStart,
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(code)),
            ]);
        }
        let queued = queue(events);
        assert!(queued.len() <= 2 + MAX_QUEUED_SHELL_EVENTS, "{queued:?}");
        assert!(matches!(&queued[0], TerminalEvent::Title(title) if title == "title"));
        assert!(matches!(queued[1], TerminalEvent::Bell));
        assert!(matches!(
            queued.last(),
            Some(TerminalEvent::ShellCommandFinished(Some(9_999)))
        ));
    }

    #[test]
    fn eviction_drops_whole_command_cycles() {
        // The integrations print `C` on submit, then `D` and `A` together.
        let mut queued = Vec::new();
        for code in 0..1_000 {
            for event in [
                TerminalEvent::ShellCommandExecuting,
                TerminalEvent::ShellCommandFinished(Some(code)),
                TerminalEvent::ShellPromptStart,
            ] {
                if coalesce_event(&mut queued, &event) {
                    queued.push(event);
                }
                assert!(queued.len() <= MAX_QUEUED_SHELL_EVENTS);
                // Every queued finish follows the start that timed it.
                for (index, event) in queued.iter().enumerate() {
                    if matches!(event, TerminalEvent::ShellCommandFinished(_)) {
                        assert!(
                            index > 0
                                && matches!(
                                    queued[index - 1],
                                    TerminalEvent::ShellCommandExecuting
                                ),
                            "orphan finish in {queued:?}"
                        );
                    }
                }
            }
        }
        // Without prompt starts, the oldest event goes first.
        let queued = queue((0..100).map(|_| TerminalEvent::ShellCommandExecuting));
        assert_eq!(queued.len(), MAX_QUEUED_SHELL_EVENTS);
    }

    #[test]
    fn command_finish_is_not_sticky_but_the_shell_phase_is() {
        assert_eq!(
            sticky_key(&TerminalEvent::ShellCommandFinished(Some(0))),
            None
        );
        assert_eq!(sticky_key(&TerminalEvent::ShellCommandExecuting), Some(3));
        assert_eq!(coalesce_key(&TerminalEvent::ShellCommandExecuting), None);
    }

    #[test]
    fn hmac_matches_rfc_4231_and_proofs_are_direction_bound() {
        // RFC 4231 test case 2.
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        let hex: String = mac.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(
            hex,
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let (client, server) = ([1; 32], [2; 32]);
        assert_ne!(
            host_proof("token", &client, &server),
            client_proof("token", &client, &server)
        );
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"sane"));
        assert!(!constant_time_eq(b"same", b"same "));
    }
}
