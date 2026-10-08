use std::io;

use super::{ChildExit, PtySize, SpawnConfig};

pub(crate) struct Transport;

fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "native terminal transport is unavailable on this operating system",
    )
}

impl Transport {
    pub(crate) fn spawn(
        _config: SpawnConfig,
        _size: PtySize,
        _on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        _on_exit: impl FnOnce() + Send + 'static,
    ) -> io::Result<Self> {
        Err(unsupported())
    }

    pub(crate) fn spawn_with_exit(
        _config: SpawnConfig,
        _size: PtySize,
        _inherit_environment: bool,
        _on_output: impl FnMut(&[u8]) -> Vec<u8> + Send + 'static,
        _on_exit: impl FnOnce(ChildExit) + Send + 'static,
    ) -> io::Result<Self> {
        Err(unsupported())
    }

    pub(crate) fn write(&self, _input: &[u8]) -> io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn write_owned(&self, _input: Vec<u8>) -> io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn write_protocol_reply_owned(&self, _input: Vec<u8>) -> io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn resize(&self, _size: PtySize) -> io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn child_pid(&self) -> u32 {
        0
    }
    pub(crate) fn signal(&self, _signal: i32) -> io::Result<()> {
        Err(unsupported())
    }
    pub(crate) fn terminate(&self) {}
    pub(crate) fn foreground_process_name(&self) -> Option<String> {
        None
    }
}
