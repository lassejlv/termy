use crate::{remote::*, *};
use anyhow::{Context, ensure};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::sync::Arc;

// Version 2 adds the OSC 7501 snapshot event to the bincode event enum.
// Version 3 adds TERM_PROGRAM identity to PaneLaunch's binary runtime config.
// Version 4 requires mutual authentication, retires the legacy capability
// fallbacks and freezes the shutdown handshake (see `Request`).
pub(crate) const VERSION: u32 = 4;
const MAX_MESSAGE_BYTES: u64 = 128 * 1024 * 1024;

/// Hash of the encoding of every wire type, checked by the `wire_golden`
/// test. When it changes, the wire changed: bump [`VERSION`] (unless the
/// unreleased change already did) and update this value.
pub(crate) const WIRE_FINGERPRINT: u64 = 0xd4e3_aba4_febf_8f2d;

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
    pub working_directory: Option<String>,
    pub shell_integration: Option<TabTitleShellIntegration>,
    pub config: TerminalRuntimeConfig,
    pub launch: Option<TerminalLaunch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: String,
    pub child_pid: Option<u32>,
    pub title: Option<String>,
    pub working_directory: Option<String>,
    pub exited: bool,
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
    GetLayout,
    SetLayout(String),
    /// Frozen (index 10).
    Shutdown,
    CompareAndSetLayout {
        expected: Option<String>,
        replacement: String,
    },
    SubscribeGraphics(String),
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
}

/// See [`Request`] for the wire rules and the frozen variants.
#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    /// Frozen (index 0).
    Ok,
    Attached(PaneInfo, Arc<RemoteState>),
    Reply(RemoteReply),
    Panes(Vec<PaneInfo>),
    Layout(Option<String>),
    /// Frozen (index 5).
    Error(String),
    LayoutUpdated(bool),
    Viewport {
        changed: bool,
        generation: u64,
    },
    /// Frozen (index 8).
    Challenge {
        server_nonce: [u8; 32],
        server_proof: [u8; 32],
    },
}

/// See [`Request`] for the wire rules.
#[derive(Serialize, Deserialize)]
pub(crate) enum Update {
    /// A pane's new state inside the host; clients receive `GraphicsState`.
    State(Arc<RemoteState>),
    Events(Vec<TerminalEvent>),
    Host {
        id: u64,
        request: RemoteHostRequest,
    },
    GraphicsState(Arc<RemoteState>, graphics::GraphicsUpdate),
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

/// Events whose latest value replaces earlier ones, so a reattaching client
/// can be given the current title, directory, progress and status.
pub(crate) fn sticky_key(event: &TerminalEvent) -> Option<u8> {
    match event {
        TerminalEvent::Title(_) | TerminalEvent::ResetTitle => Some(0),
        TerminalEvent::WorkingDirectory(_) => Some(1),
        TerminalEvent::Progress(_) => Some(2),
        TerminalEvent::ProgramStatus(_) => Some(4),
        TerminalEvent::ShellPromptStart
        | TerminalEvent::ShellCommandStart
        | TerminalEvent::ShellCommandExecuting
        | TerminalEvent::ShellCommandFinished(_) => Some(3),
        _ => None,
    }
}

/// Prepares `queue`, the undelivered events of a client that may not be
/// draining, for `event`: a newer state replaces the queued one and only the
/// latest clipboard store matters. Returns false when `event` repeats one
/// that is already queued (a bell rings once until the client catches up).
/// This keeps the queue bounded by the number of event kinds.
pub(crate) fn coalesce_event(queue: &mut Vec<TerminalEvent>, event: &TerminalEvent) -> bool {
    if let Some(key) = sticky_key(event) {
        queue.retain(|old| sticky_key(old) != Some(key));
    }
    match event {
        TerminalEvent::Bell | TerminalEvent::InputDropped | TerminalEvent::Exit => !queue
            .iter()
            .any(|old| std::mem::discriminant(old) == std::mem::discriminant(event)),
        TerminalEvent::ClipboardStore(_) => {
            queue.retain(|old| !matches!(old, TerminalEvent::ClipboardStore(_)));
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

pub(crate) fn write_message(
    writer: &mut impl Write,
    message: &impl Serialize,
) -> anyhow::Result<()> {
    let bytes = codec()
        .serialize(message)
        .context("encode multiplexer message")?;
    ensure!(
        bytes.len() as u64 <= MAX_MESSAGE_BYTES,
        "multiplexer message too large"
    );
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn write_response(writer: &mut impl Write, response: &Response) -> anyhow::Result<()> {
    // Bincode performs sizing and writing passes. Keep the PNG exports of a
    // `RemoteReply::Graphics` snapshot alive across both, so each image is
    // encoded once; the graphics stream sends raw image generations instead.
    let _exports = match response {
        Response::Reply(RemoteReply::Graphics(_, placements)) => {
            Some(crate::remote::serde_image::retain_png_exports(placements))
        }
        _ => None,
    };
    write_message(writer, response)
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
