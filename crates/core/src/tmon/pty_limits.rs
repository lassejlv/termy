//! Shared bounds for Tmon's Unix PTY and Windows ConPTY write paths.

pub(crate) const MAX_WRITE_BACKLOG_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_WRITE_BACKLOG_ENTRIES: usize = 4096;
pub(crate) const MAX_PROTOCOL_REPLY_BACKLOG_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_PROTOCOL_REPLY_BACKLOG_ENTRIES: usize = 256;
pub(crate) const MAX_WRITE_CHUNK_BYTES: usize = 16 * 1024;
