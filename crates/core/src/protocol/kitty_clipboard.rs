use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use rand_core::{OsRng, RngCore};

use super::TerminalReplyHost;

const MAX_ID_BYTES: usize = 512;
const MAX_MIME_BYTES: usize = 256;
const MAX_NAME_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 128;
const MAX_MIME_TYPES: usize = 64;
const MAX_CHUNK_BYTES: usize = 4096;
const MAX_WRITE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SESSION_GRANTS: usize = 32;
const MAX_PASTE_GRANTS: usize = 8;
const PASTE_GRANT_LIFETIME: Duration = Duration::from_secs(60);

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboardLocation {
    Clipboard,
    Primary,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TerminalClipboardContent {
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TerminalClipboardReadRequest {
    pub location: TerminalClipboardLocation,
    pub mime_types: Vec<String>,
    pub list_available: bool,
    pub name: Option<String>,
    pub permission_granted: bool,
    pub can_remember_permission: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum TerminalClipboardReadResult {
    Success {
        available_formats: Vec<String>,
        contents: Vec<TerminalClipboardContent>,
        remember_permission: bool,
    },
    Denied,
    Unsupported,
    Busy,
    IoError,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct TerminalClipboardWriteRequest {
    pub location: TerminalClipboardLocation,
    pub contents: Vec<TerminalClipboardContent>,
    pub name: Option<String>,
    pub permission_granted: bool,
    pub can_remember_permission: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClipboardWriteResult {
    Success { remember_permission: bool },
    Denied,
    Unsupported,
    Busy,
    InvalidData,
    IoError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyClipboardOscTerminator {
    Bell,
    StringTerminator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyClipboardOsc {
    body: Vec<u8>,
    terminator: KittyClipboardOscTerminator,
}

impl KittyClipboardOsc {
    pub fn from_osc_payload(
        payload: &[u8],
        terminator: KittyClipboardOscTerminator,
    ) -> Option<Self> {
        Some(Self {
            body: payload.strip_prefix(b"5522;")?.to_vec(),
            terminator,
        })
    }

    pub fn from_body(body: &[u8], terminator: KittyClipboardOscTerminator) -> Self {
        Self {
            body: body.to_vec(),
            terminator,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    Read,
    Write,
    WriteData,
    WriteAlias,
}

impl Operation {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "wdata" => Some(Self::WriteData),
            "walias" => Some(Self::WriteAlias),
            _ => None,
        }
    }
}

#[derive(Debug)]
enum MetadataError {
    Drop,
    InvalidValue(Operation),
}

#[derive(Debug)]
struct Metadata {
    operation: Operation,
    location: TerminalClipboardLocation,
    id: String,
    mime_type: Option<String>,
    password: Option<String>,
    name: Option<String>,
}

impl Metadata {
    fn parse(raw: &[u8]) -> Result<Self, MetadataError> {
        let raw = std::str::from_utf8(raw).map_err(|_| MetadataError::Drop)?;
        let mut operation = None;
        let mut location = TerminalClipboardLocation::Clipboard;
        let mut raw_id = "";
        let mut raw_mime = "";
        let mut raw_password = "";
        let mut raw_name = "";

        for record in raw.split(':') {
            let (key, value) = record.split_once('=').ok_or(MetadataError::Drop)?;
            match key {
                "type" => operation = Operation::parse(value),
                "loc" => {
                    location = if value == "primary" {
                        TerminalClipboardLocation::Primary
                    } else {
                        TerminalClipboardLocation::Clipboard
                    };
                }
                "id" => raw_id = value,
                "mime" => raw_mime = value,
                "pw" => raw_password = value,
                "name" => raw_name = value,
                _ => {}
            }
        }

        let operation = operation.ok_or(MetadataError::Drop)?;
        let mime_type = decode_text_metadata(raw_mime, MAX_MIME_BYTES)
            .map_err(|error| classify_metadata_error(error, operation))?;
        if mime_type
            .as_deref()
            .is_some_and(|mime_type| !valid_mime_type(mime_type))
        {
            return Err(MetadataError::InvalidValue(operation));
        }
        let password = match decode_text_metadata(raw_password, MAX_PASSWORD_BYTES) {
            Ok(password) => password,
            Err(DecodeMetadataError::TooLong) => None,
            Err(DecodeMetadataError::InvalidBase64) => return Err(MetadataError::Drop),
            Err(DecodeMetadataError::InvalidUtf8) => {
                return Err(MetadataError::InvalidValue(operation));
            }
        };
        let name = decode_text_metadata(raw_name, MAX_NAME_BYTES)
            .map_err(|error| classify_metadata_error(error, operation))?;

        Ok(Self {
            operation,
            location,
            id: sanitize_id(raw_id),
            mime_type,
            password,
            name,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodeMetadataError {
    InvalidBase64,
    InvalidUtf8,
    TooLong,
}

fn classify_metadata_error(error: DecodeMetadataError, operation: Operation) -> MetadataError {
    if error == DecodeMetadataError::InvalidBase64 {
        return MetadataError::Drop;
    }
    MetadataError::InvalidValue(operation)
}

fn decode_text_metadata(value: &str, limit: usize) -> Result<Option<String>, DecodeMetadataError> {
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > limit.saturating_add(2) / 3 * 4 {
        return Err(DecodeMetadataError::TooLong);
    }
    let decoded = BASE64
        .decode(value)
        .map_err(|_| DecodeMetadataError::InvalidBase64)?;
    if decoded.len() > limit {
        return Err(DecodeMetadataError::TooLong);
    }
    String::from_utf8(decoded)
        .map(Some)
        .map_err(|_| DecodeMetadataError::InvalidUtf8)
}

fn valid_mime_type(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn sanitize_id(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'+' | b'.'))
        .take(MAX_ID_BYTES)
        .map(char::from)
        .collect()
}

#[derive(Debug)]
struct ClipboardPacket {
    metadata: Metadata,
    payload: Option<Vec<u8>>,
    terminator: KittyClipboardOscTerminator,
}

impl ClipboardPacket {
    fn parse(osc: KittyClipboardOsc) -> Result<Self, MetadataError> {
        let (raw_metadata, payload) = match osc.body.iter().position(|byte| *byte == b';') {
            Some(index) => (&osc.body[..index], Some(osc.body[index + 1..].to_vec())),
            None => (osc.body.as_slice(), None),
        };
        Ok(Self {
            metadata: Metadata::parse(raw_metadata)?,
            payload,
            terminator: osc.terminator,
        })
    }
}

#[derive(Debug)]
struct WriteEntry {
    mime_type: String,
    data: Vec<u8>,
}

#[derive(Debug)]
struct WriteTransaction {
    location: TerminalClipboardLocation,
    id: String,
    password: Option<String>,
    name: Option<String>,
    entries: Vec<WriteEntry>,
    aliases: Vec<(String, String)>,
    total_bytes: usize,
}

impl WriteTransaction {
    fn new(metadata: Metadata) -> Self {
        let password = usable_password(&metadata);
        Self {
            location: metadata.location,
            id: metadata.id,
            password,
            name: metadata.name,
            entries: Vec::new(),
            aliases: Vec::new(),
            total_bytes: 0,
        }
    }

    fn push_data(&mut self, mime_type: String, payload: &[u8]) -> Result<(), WriteError> {
        let decoded = decode_payload(payload).map_err(|_| WriteError::Invalid)?;
        if decoded.len() > MAX_CHUNK_BYTES {
            return Err(WriteError::Invalid);
        }
        let new_total = self
            .total_bytes
            .checked_add(decoded.len())
            .ok_or(WriteError::TooLarge)?;
        if new_total > MAX_WRITE_BYTES {
            return Err(WriteError::TooLarge);
        }

        let index = if self
            .entries
            .last()
            .is_some_and(|entry| entry.mime_type == mime_type)
        {
            self.entries.len() - 1
        } else {
            if self
                .entries
                .iter()
                .any(|entry| entry.mime_type == mime_type)
                || self.entries.len() >= MAX_MIME_TYPES
            {
                return Err(WriteError::Invalid);
            }
            self.entries.push(WriteEntry {
                mime_type,
                data: Vec::new(),
            });
            self.entries.len() - 1
        };
        self.entries[index].data.extend_from_slice(&decoded);
        self.total_bytes = new_total;
        Ok(())
    }

    fn push_aliases(&mut self, target: &str, payload: &[u8]) -> Result<(), WriteError> {
        let decoded = decode_payload(payload).map_err(|_| WriteError::Invalid)?;
        let aliases = std::str::from_utf8(&decoded).map_err(|_| WriteError::Invalid)?;
        for alias in aliases.split_ascii_whitespace() {
            if alias.is_empty() || alias.len() > MAX_MIME_BYTES {
                return Err(WriteError::Invalid);
            }
            if alias != target
                && !self
                    .aliases
                    .iter()
                    .any(|(existing_target, existing_alias)| {
                        existing_target == target && existing_alias == alias
                    })
            {
                if self.aliases.len() >= MAX_MIME_TYPES {
                    return Err(WriteError::Invalid);
                }
                self.aliases.push((target.to_string(), alias.to_string()));
            }
        }
        Ok(())
    }

    fn into_contents(self) -> Result<Vec<TerminalClipboardContent>, WriteError> {
        if self
            .aliases
            .iter()
            .any(|(target, _)| !self.entries.iter().any(|entry| &entry.mime_type == target))
        {
            return Err(WriteError::Invalid);
        }
        let mut contents = Vec::new();
        let mut expanded_bytes = 0_usize;
        for entry in self.entries {
            let aliases = self
                .aliases
                .iter()
                .filter(|(target, _)| target == &entry.mime_type)
                .map(|(_, alias)| alias)
                .collect::<Vec<_>>();
            expanded_bytes = expanded_bytes
                .checked_add(
                    entry
                        .data
                        .len()
                        .checked_mul(aliases.len() + 1)
                        .ok_or(WriteError::TooLarge)?,
                )
                .ok_or(WriteError::TooLarge)?;
            if expanded_bytes > MAX_WRITE_BYTES {
                return Err(WriteError::TooLarge);
            }
            for alias in aliases {
                contents.push(TerminalClipboardContent {
                    mime_type: alias.clone(),
                    data: entry.data.clone(),
                });
            }
            contents.push(TerminalClipboardContent {
                mime_type: entry.mime_type,
                data: entry.data,
            });
        }
        Ok(contents)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteError {
    Invalid,
    TooLarge,
}

#[derive(Debug)]
struct Grant {
    location: TerminalClipboardLocation,
    password: String,
    kind: GrantKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantKind {
    Read,
    Write,
}

#[derive(Debug)]
struct PasteGrant {
    location: TerminalClipboardLocation,
    password: String,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct KittyClipboardHostState {
    write: Option<WriteTransaction>,
    ignore_write_packets: bool,
    session_grants: VecDeque<Grant>,
    paste_grants: VecDeque<PasteGrant>,
    paste_events_enabled: bool,
}

impl KittyClipboardHostState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn paste_events_enabled(&self) -> bool {
        self.paste_events_enabled
    }

    pub fn set_paste_events_enabled(&mut self, enabled: bool) {
        self.paste_events_enabled = enabled;
        if !enabled {
            self.paste_grants.clear();
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn handle_osc(
        &mut self,
        osc: KittyClipboardOsc,
        host: &mut impl TerminalReplyHost,
    ) -> Vec<Vec<u8>> {
        let terminator = osc.terminator;
        let packet = match ClipboardPacket::parse(osc) {
            Ok(packet) => packet,
            Err(MetadataError::InvalidValue(Operation::Write)) => {
                self.write = None;
                self.ignore_write_packets = true;
                return vec![response("write", "EINVAL", "", &[], None, terminator)];
            }
            Err(MetadataError::InvalidValue(operation))
                if matches!(operation, Operation::WriteData | Operation::WriteAlias)
                    && self.write.is_some() =>
            {
                let id = self.write.take().map(|write| write.id).unwrap_or_default();
                self.ignore_write_packets = true;
                return vec![response("write", "EINVAL", &id, &[], None, terminator)];
            }
            Err(_) => return Vec::new(),
        };

        match packet.metadata.operation {
            Operation::Read => self.handle_read(packet, host),
            Operation::Write => {
                self.write = Some(WriteTransaction::new(packet.metadata));
                self.ignore_write_packets = false;
                Vec::new()
            }
            Operation::WriteData => self.handle_write_data(packet, host),
            Operation::WriteAlias => self.handle_write_alias(packet),
        }
    }

    pub fn paste_notification(
        &mut self,
        location: TerminalClipboardLocation,
        available_formats: &[String],
    ) -> Option<Vec<u8>> {
        if !self.paste_events_enabled {
            return None;
        }

        self.expire_paste_grants();
        let mut random = [0_u8; 18];
        OsRng.fill_bytes(&mut random);
        let password = URL_SAFE_NO_PAD.encode(random);
        self.paste_grants.push_back(PasteGrant {
            location,
            password: password.clone(),
            expires_at: Instant::now() + PASTE_GRANT_LIFETIME,
        });
        while self.paste_grants.len() > MAX_PASTE_GRANTS {
            self.paste_grants.pop_front();
        }

        let mut extra = Vec::with_capacity(2);
        if location == TerminalClipboardLocation::Primary {
            extra.push(("loc", "primary"));
        }
        let encoded_password = BASE64.encode(password);
        extra.push(("pw", encoded_password.as_str()));
        let mime_list = available_formats
            .iter()
            .filter(|mime| {
                !mime.is_empty()
                    && mime.len() <= MAX_MIME_BYTES
                    && !mime.bytes().any(|byte| byte.is_ascii_whitespace())
            })
            .take(MAX_MIME_TYPES)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");

        let mut output = Vec::new();
        output.extend(response(
            "read",
            "OK",
            "",
            &extra,
            None,
            KittyClipboardOscTerminator::StringTerminator,
        ));
        let mut data_extra = vec![("mime", "Lg==")];
        data_extra.push(("pw", encoded_password.as_str()));
        output.extend(response(
            "read",
            "DATA",
            "",
            &data_extra,
            Some(BASE64.encode(mime_list).as_bytes()),
            KittyClipboardOscTerminator::StringTerminator,
        ));
        output.extend(response(
            "read",
            "DONE",
            "",
            &[("pw", encoded_password.as_str())],
            None,
            KittyClipboardOscTerminator::StringTerminator,
        ));
        Some(output)
    }

    fn handle_read(
        &mut self,
        packet: ClipboardPacket,
        host: &mut impl TerminalReplyHost,
    ) -> Vec<Vec<u8>> {
        let Some(payload) = packet.payload.as_deref() else {
            return Vec::new();
        };
        let Ok(decoded) = decode_payload(payload) else {
            return Vec::new();
        };
        let Ok(decoded) = std::str::from_utf8(&decoded) else {
            return Vec::new();
        };
        let mut list_available = false;
        let mut mime_types = Vec::new();
        for mime_type in decoded.split_ascii_whitespace() {
            if mime_type == "." {
                list_available = true;
            } else if mime_type.len() > MAX_MIME_BYTES || !valid_mime_type(mime_type) {
                return Vec::new();
            } else if !mime_types.iter().any(|existing| existing == mime_type) {
                if mime_types.len() >= MAX_MIME_TYPES {
                    return Vec::new();
                }
                mime_types.push(mime_type.to_string());
            }
        }
        if !list_available && mime_types.is_empty() {
            return Vec::new();
        }

        let password = usable_password(&packet.metadata);
        let permission_granted = if mime_types.is_empty() {
            true
        } else {
            password.as_deref().is_some_and(|password| {
                self.has_session_grant(packet.metadata.location, password, GrantKind::Read)
                    || self.use_paste_grant(packet.metadata.location, password)
            })
        };
        let request = TerminalClipboardReadRequest {
            location: packet.metadata.location,
            mime_types,
            list_available,
            name: packet.metadata.name.clone(),
            permission_granted,
            can_remember_permission: password.is_some(),
        };
        let result = host.read_clipboard(request);
        let id = packet.metadata.id;
        match result {
            TerminalClipboardReadResult::Success {
                available_formats,
                contents,
                remember_permission,
            } => {
                if remember_permission && let Some(password) = password {
                    self.remember_grant(packet.metadata.location, password, GrantKind::Read);
                }
                read_success_responses(
                    &id,
                    list_available,
                    available_formats,
                    contents,
                    packet.terminator,
                )
            }
            TerminalClipboardReadResult::Denied => {
                vec![response("read", "EPERM", &id, &[], None, packet.terminator)]
            }
            TerminalClipboardReadResult::Unsupported => {
                vec![response(
                    "read",
                    "ENOSYS",
                    &id,
                    &[],
                    None,
                    packet.terminator,
                )]
            }
            TerminalClipboardReadResult::Busy => {
                vec![response("read", "EBUSY", &id, &[], None, packet.terminator)]
            }
            TerminalClipboardReadResult::IoError => {
                vec![response("read", "EBUSY", &id, &[], None, packet.terminator)]
            }
        }
    }

    fn handle_write_data(
        &mut self,
        packet: ClipboardPacket,
        host: &mut impl TerminalReplyHost,
    ) -> Vec<Vec<u8>> {
        if self.ignore_write_packets {
            return Vec::new();
        }
        let Some(write) = self.write.as_mut() else {
            return Vec::new();
        };
        if packet.metadata.mime_type.is_none() {
            if packet
                .payload
                .as_deref()
                .is_some_and(|payload| !payload.is_empty())
            {
                return self.abort_write("EINVAL", packet.terminator);
            }
            let write = self.write.take().expect("write transaction checked above");
            return self.commit_write(write, host, packet.terminator);
        }
        let (Some(mime_type), Some(payload)) =
            (packet.metadata.mime_type, packet.payload.as_deref())
        else {
            return self.abort_write("EINVAL", packet.terminator);
        };
        match write.push_data(mime_type, payload) {
            Ok(()) => Vec::new(),
            Err(WriteError::Invalid) => self.abort_write("EINVAL", packet.terminator),
            Err(WriteError::TooLarge) => self.abort_write("EIO", packet.terminator),
        }
    }

    fn handle_write_alias(&mut self, packet: ClipboardPacket) -> Vec<Vec<u8>> {
        if self.ignore_write_packets {
            return Vec::new();
        }
        let Some(write) = self.write.as_mut() else {
            return Vec::new();
        };
        let (Some(target), Some(payload)) = (
            packet.metadata.mime_type.as_deref(),
            packet.payload.as_deref(),
        ) else {
            return self.abort_write("EINVAL", packet.terminator);
        };
        match write.push_aliases(target, payload) {
            Ok(()) => Vec::new(),
            Err(_) => self.abort_write("EINVAL", packet.terminator),
        }
    }

    fn commit_write(
        &mut self,
        write: WriteTransaction,
        host: &mut impl TerminalReplyHost,
        terminator: KittyClipboardOscTerminator,
    ) -> Vec<Vec<u8>> {
        let WriteTransaction {
            location,
            id,
            password,
            name,
            entries,
            aliases,
            ..
        } = write;
        let permission_granted = password
            .as_deref()
            .is_some_and(|password| self.has_session_grant(location, password, GrantKind::Write));
        let contents = match (WriteTransaction {
            location,
            id: String::new(),
            password: None,
            name: None,
            entries,
            aliases,
            total_bytes: 0,
        })
        .into_contents()
        {
            Ok(contents) => contents,
            Err(WriteError::Invalid) => {
                self.ignore_write_packets = true;
                return vec![response("write", "EINVAL", &id, &[], None, terminator)];
            }
            Err(WriteError::TooLarge) => {
                self.ignore_write_packets = true;
                return vec![response("write", "EIO", &id, &[], None, terminator)];
            }
        };
        let request = TerminalClipboardWriteRequest {
            location,
            contents,
            name,
            permission_granted,
            can_remember_permission: password.is_some(),
        };
        let (status, remember) = match host.write_clipboard(request) {
            TerminalClipboardWriteResult::Success {
                remember_permission,
            } => ("DONE", remember_permission),
            TerminalClipboardWriteResult::Denied => ("EPERM", false),
            TerminalClipboardWriteResult::Unsupported => ("ENOSYS", false),
            TerminalClipboardWriteResult::Busy => ("EBUSY", false),
            TerminalClipboardWriteResult::InvalidData => ("EINVAL", false),
            TerminalClipboardWriteResult::IoError => ("EIO", false),
        };
        if remember && let Some(password) = password {
            self.remember_grant(location, password, GrantKind::Write);
        }
        vec![response("write", status, &id, &[], None, terminator)]
    }

    fn abort_write(
        &mut self,
        status: &str,
        terminator: KittyClipboardOscTerminator,
    ) -> Vec<Vec<u8>> {
        let id = self.write.take().map(|write| write.id).unwrap_or_default();
        self.ignore_write_packets = true;
        vec![response("write", status, &id, &[], None, terminator)]
    }

    fn has_session_grant(
        &self,
        location: TerminalClipboardLocation,
        password: &str,
        kind: GrantKind,
    ) -> bool {
        self.session_grants.iter().any(|grant| {
            grant.location == location && grant.password == password && grant.kind == kind
        })
    }

    fn remember_grant(
        &mut self,
        location: TerminalClipboardLocation,
        password: String,
        kind: GrantKind,
    ) {
        if self.has_session_grant(location, &password, kind) {
            return;
        }
        self.session_grants.push_back(Grant {
            location,
            password,
            kind,
        });
        while self.session_grants.len() > MAX_SESSION_GRANTS {
            self.session_grants.pop_front();
        }
    }

    fn use_paste_grant(&mut self, location: TerminalClipboardLocation, password: &str) -> bool {
        self.expire_paste_grants();
        let Some(index) = self
            .paste_grants
            .iter()
            .position(|grant| grant.location == location && grant.password == password)
        else {
            return false;
        };
        self.paste_grants.remove(index);
        true
    }

    fn expire_paste_grants(&mut self) {
        let now = Instant::now();
        self.paste_grants.retain(|grant| grant.expires_at > now);
    }
}

fn usable_password(metadata: &Metadata) -> Option<String> {
    if metadata
        .name
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        metadata
            .password
            .clone()
            .filter(|password| !password.is_empty())
    } else {
        None
    }
}

fn decode_payload(payload: &[u8]) -> Result<Vec<u8>, ()> {
    BASE64.decode(payload).map_err(|_| ())
}

fn read_success_responses(
    id: &str,
    list_available: bool,
    available_formats: Vec<String>,
    contents: Vec<TerminalClipboardContent>,
    terminator: KittyClipboardOscTerminator,
) -> Vec<Vec<u8>> {
    let mut responses = vec![response("read", "OK", id, &[], None, terminator)];
    if list_available {
        let mime_list = available_formats
            .into_iter()
            .filter(|mime| mime.len() <= MAX_MIME_BYTES && valid_mime_type(mime))
            .take(MAX_MIME_TYPES)
            .collect::<Vec<_>>()
            .join(" ");
        let encoded = BASE64.encode(mime_list);
        responses.push(response(
            "read",
            "DATA",
            id,
            &[("mime", "Lg==")],
            Some(encoded.as_bytes()),
            terminator,
        ));
    }
    for content in contents {
        let mime = BASE64.encode(content.mime_type);
        if content.data.is_empty() {
            responses.push(response(
                "read",
                "DATA",
                id,
                &[("mime", mime.as_str())],
                Some(b""),
                terminator,
            ));
            continue;
        }
        for chunk in content.data.chunks(MAX_CHUNK_BYTES) {
            let encoded = BASE64.encode(chunk);
            responses.push(response(
                "read",
                "DATA",
                id,
                &[("mime", mime.as_str())],
                Some(encoded.as_bytes()),
                terminator,
            ));
        }
    }
    responses.push(response("read", "DONE", id, &[], None, terminator));
    responses
}

fn response(
    operation: &str,
    status: &str,
    id: &str,
    extra: &[(&str, &str)],
    payload: Option<&[u8]>,
    terminator: KittyClipboardOscTerminator,
) -> Vec<u8> {
    let mut output = format!("\x1b]5522;type={operation}:status={status}").into_bytes();
    if !id.is_empty() {
        output.extend_from_slice(b":id=");
        output.extend_from_slice(id.as_bytes());
    }
    for (key, value) in extra {
        output.push(b':');
        output.extend_from_slice(key.as_bytes());
        output.push(b'=');
        output.extend_from_slice(value.as_bytes());
    }
    if let Some(payload) = payload {
        output.push(b';');
        output.extend_from_slice(payload);
    }
    match terminator {
        KittyClipboardOscTerminator::Bell => output.push(0x07),
        KittyClipboardOscTerminator::StringTerminator => output.extend_from_slice(b"\x1b\\"),
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host {
        read: TerminalClipboardReadResult,
        written: Vec<TerminalClipboardContent>,
    }

    impl TerminalReplyHost for Host {
        fn load_clipboard(
            &mut self,
            _target: super::super::TerminalClipboardTarget,
        ) -> Option<String> {
            None
        }

        fn read_clipboard(
            &mut self,
            _request: TerminalClipboardReadRequest,
        ) -> TerminalClipboardReadResult {
            self.read.clone()
        }

        fn write_clipboard(
            &mut self,
            request: TerminalClipboardWriteRequest,
        ) -> TerminalClipboardWriteResult {
            self.written = request.contents;
            TerminalClipboardWriteResult::Success {
                remember_permission: false,
            }
        }
    }

    #[derive(Default)]
    struct PermissionHost {
        read_permissions: Vec<bool>,
    }

    impl TerminalReplyHost for PermissionHost {
        fn load_clipboard(
            &mut self,
            _target: super::super::TerminalClipboardTarget,
        ) -> Option<String> {
            None
        }

        fn read_clipboard(
            &mut self,
            request: TerminalClipboardReadRequest,
        ) -> TerminalClipboardReadResult {
            self.read_permissions.push(request.permission_granted);
            TerminalClipboardReadResult::Success {
                available_formats: Vec::new(),
                contents: vec![TerminalClipboardContent {
                    mime_type: "text/plain".to_string(),
                    data: b"paste".to_vec(),
                }],
                remember_permission: false,
            }
        }
    }

    fn osc(body: &str) -> KittyClipboardOsc {
        KittyClipboardOsc {
            body: body.as_bytes().to_vec(),
            terminator: KittyClipboardOscTerminator::StringTerminator,
        }
    }

    #[test]
    fn reads_and_chunks_multiple_mime_types() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Success {
                available_formats: Vec::new(),
                contents: vec![TerminalClipboardContent {
                    mime_type: "application/octet-stream".to_string(),
                    data: vec![7; MAX_CHUNK_BYTES + 1],
                }],
                remember_permission: false,
            },
            written: Vec::new(),
        };
        let replies = state.handle_osc(
            osc("type=read:id=a*!b;YXBwbGljYXRpb24vb2N0ZXQtc3RyZWFt"),
            &mut host,
        );

        assert_eq!(replies.len(), 4);
        assert!(replies[0].starts_with(b"\x1b]5522;type=read:status=OK:id=ab"));
        assert!(replies[3].starts_with(b"\x1b]5522;type=read:status=DONE:id=ab"));
    }

    #[test]
    fn lists_formats_without_clipboard_data() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Success {
                available_formats: vec!["text/plain".to_string(), "image/png".to_string()],
                contents: Vec::new(),
                remember_permission: false,
            },
            written: Vec::new(),
        };
        let replies = state.handle_osc(osc("type=read;Lg=="), &mut host);

        assert_eq!(replies.len(), 3);
        assert!(String::from_utf8_lossy(&replies[1]).contains("mime=Lg=="));
        assert!(String::from_utf8_lossy(&replies[1]).contains("dGV4dC9wbGFpbiBpbWFnZS9wbmc="));
    }

    #[test]
    fn assembles_write_data_and_aliases() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Denied,
            written: Vec::new(),
        };

        assert!(
            state
                .handle_osc(osc("type=write:id=w1"), &mut host)
                .is_empty()
        );
        assert!(
            state
                .handle_osc(
                    osc("type=walias:mime=dGV4dC9wbGFpbg==;dGV4dC91dGY4"),
                    &mut host,
                )
                .is_empty()
        );
        assert!(
            state
                .handle_osc(osc("type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8="), &mut host,)
                .is_empty()
        );
        let replies = state.handle_osc(osc("type=wdata;"), &mut host);

        assert_eq!(
            replies,
            vec![b"\x1b]5522;type=write:status=DONE:id=w1\x1b\\".to_vec()]
        );
        assert_eq!(host.written.len(), 2);
        assert_eq!(host.written[0].mime_type, "text/utf8");
        assert_eq!(host.written[1].data, b"hello");
    }

    #[test]
    fn invalid_write_aborts_until_the_next_write() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Denied,
            written: Vec::new(),
        };
        state.handle_osc(osc("type=write:id=bad"), &mut host);
        let error = state.handle_osc(osc("type=wdata:mime=dGV4dC9wbGFpbg==;!!!"), &mut host);
        assert!(String::from_utf8_lossy(&error[0]).contains("status=EINVAL:id=bad"));
        assert!(state.handle_osc(osc("type=wdata"), &mut host).is_empty());
    }

    #[test]
    fn write_terminator_rejects_nonempty_payload() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Denied,
            written: Vec::new(),
        };
        state.handle_osc(osc("type=write:id=bad-final"), &mut host);
        state.handle_osc(osc("type=wdata:mime=dGV4dC9wbGFpbg==;aGVsbG8="), &mut host);

        let replies = state.handle_osc(osc("type=wdata;bm90LWVtcHR5"), &mut host);

        assert_eq!(replies.len(), 1);
        assert!(String::from_utf8_lossy(&replies[0]).contains("status=EINVAL:id=bad-final"));
        assert!(host.written.is_empty());
    }

    #[test]
    fn write_rejects_control_bytes_in_mime_type() {
        let mut state = KittyClipboardHostState::new();
        let mut host = Host {
            read: TerminalClipboardReadResult::Denied,
            written: Vec::new(),
        };
        state.handle_osc(osc("type=write:id=bad-mime"), &mut host);

        let replies = state.handle_osc(osc("type=wdata:mime=dGV4dC8AcGxhaW4=;aGVsbG8="), &mut host);

        assert_eq!(replies.len(), 1);
        assert!(String::from_utf8_lossy(&replies[0]).contains("status=EINVAL:id=bad-mime"));
        assert!(host.written.is_empty());
    }

    #[test]
    fn paste_password_is_single_use_and_location_scoped() {
        let mut state = KittyClipboardHostState::new();
        state.set_paste_events_enabled(true);
        let notification = state
            .paste_notification(
                TerminalClipboardLocation::Clipboard,
                &["text/plain".to_string()],
            )
            .unwrap();
        let notification = String::from_utf8(notification).unwrap();
        let encoded_password = notification
            .split("pw=")
            .nth(1)
            .unwrap()
            .split([':', '\x1b'])
            .next()
            .unwrap();
        let password = String::from_utf8(BASE64.decode(encoded_password).unwrap()).unwrap();

        assert!(!state.use_paste_grant(TerminalClipboardLocation::Primary, &password));
        assert!(state.use_paste_grant(TerminalClipboardLocation::Clipboard, &password));
        assert!(!state.use_paste_grant(TerminalClipboardLocation::Clipboard, &password));
    }

    #[test]
    fn paste_password_authorizes_only_the_first_matching_read() {
        let mut state = KittyClipboardHostState::new();
        state.set_paste_events_enabled(true);
        let notification = String::from_utf8(
            state
                .paste_notification(
                    TerminalClipboardLocation::Clipboard,
                    &["text/plain".to_string()],
                )
                .unwrap(),
        )
        .unwrap();
        let encoded_password = notification
            .split("pw=")
            .nth(1)
            .unwrap()
            .split([':', '\x1b'])
            .next()
            .unwrap();
        let request =
            format!("type=read:pw={encoded_password}:name=UGFzdGUgZXZlbnQ=;dGV4dC9wbGFpbg==");
        let mut host = PermissionHost::default();

        state.handle_osc(osc(&request), &mut host);
        state.handle_osc(osc(&request), &mut host);

        assert_eq!(host.read_permissions, vec![true, false]);
    }
}
