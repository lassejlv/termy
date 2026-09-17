use anyhow::{Context, ensure};
use bincode::Options;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{Read, Write};
use std::sync::Arc;
use termy_core::{remote::*, *};

pub(crate) const VERSION: u32 = 1;
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
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Response {
    Ok,
    Attached(PaneInfo, Arc<RemoteState>),
    Reply(RemoteReply),
    Panes(Vec<PaneInfo>),
    Layout(Option<String>),
    Error(String),
}

#[derive(Serialize, Deserialize)]
pub(crate) enum Update {
    State(Arc<RemoteState>),
    Events(Vec<TerminalEvent>),
    Host { id: u64, request: RemoteHostRequest },
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
