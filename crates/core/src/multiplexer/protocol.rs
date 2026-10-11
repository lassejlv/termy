use crate::{remote::*, *};
use anyhow::{Context, ensure};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::sync::Arc;

// Version 2 adds the OSC 7501 snapshot event to the bincode event enum.
pub(crate) const VERSION: u32 = 2;
const MAX_MESSAGE_BYTES: u64 = 128 * 1024 * 1024;

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

#[derive(Serialize, Deserialize)]
pub(crate) enum Request {
    Hello {
        version: u32,
        token: String,
    },
    Create(Box<PaneLaunch>),
    Attach(String),
    Subscribe(String),
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
    Shutdown,
    CompareAndSetLayout {
        expected: Option<String>,
        replacement: String,
    },
    SubscribeGraphics(String),
    /// Scroll or clear the viewport, publish the new frame immediately and
    /// reply with its generation. Gated by `Endpoint::viewport_replies`.
    ViewportCommand {
        pane: String,
        command: RemoteCommand,
    },
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Ok,
    Attached(PaneInfo, Arc<RemoteState>),
    Reply(RemoteReply),
    Panes(Vec<PaneInfo>),
    Layout(Option<String>),
    Error(String),
    LayoutUpdated(bool),
    Viewport { changed: bool, generation: u64 },
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Update {
    State(Arc<RemoteState>),
    Events(Vec<TerminalEvent>),
    Host { id: u64, request: RemoteHostRequest },
    GraphicsState(Arc<RemoteState>, graphics::GraphicsUpdate),
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
    // Bincode performs sizing and writing passes. Keep legacy PNG exports alive
    // across both; the negotiated graphics stream sends raw image generations.
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
}
