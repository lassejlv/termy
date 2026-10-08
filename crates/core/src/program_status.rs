//! OSC 7501 Program Status Protocol, revision 0.3.
//!
//! Reports are validated atomically; stored records retain only their own app.
//! Snapshots resolve app inheritance at read time, including missing parents.

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgramStatusKind {
    Permission,
    Question,
    Auth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramStatusRecord {
    /// Empty for the root record.
    pub id: String,
    pub state: ProgramState,
    pub kind: Option<ProgramStatusKind>,
    pub progress: Option<u8>,
    /// Snapshots include the nearest ancestor's app when this record omits it.
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

#[derive(Default)]
pub(crate) struct ProgramStatus {
    // Least recently updated first. Bounded to 256 records by the protocol.
    records: Vec<ProgramStatusRecord>,
    changed: bool,
}

impl ProgramStatus {
    pub(crate) fn report(&mut self, body: &str) {
        let Some(report) = parse(body) else { return };
        match report {
            Report::Clear(id) => {
                let prefix = format!("{id}/");
                self.retain(|record| {
                    !id.is_empty() && record.id != id && !record.id.starts_with(&prefix)
                });
            }
            Report::Set(record) => {
                self.records.retain(|old| old.id != record.id);
                if self.records.len() == 256 {
                    self.records.remove(0);
                }
                self.records.push(record);
                self.changed = true;
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.retain(|_| false);
    }

    pub(crate) fn finish(&mut self) {
        self.retain(|record| {
            !matches!(record.state, ProgramState::Working | ProgramState::Blocked)
        });
    }

    fn retain(&mut self, keep: impl FnMut(&ProgramStatusRecord) -> bool) {
        let before = self.records.len();
        self.records.retain(keep);
        self.changed |= before != self.records.len();
    }

    pub(crate) fn has_changes(&self) -> bool {
        self.changed
    }

    pub(crate) fn take_changed(&mut self) -> Option<Vec<ProgramStatusRecord>> {
        std::mem::take(&mut self.changed).then(|| self.snapshot())
    }

    pub(crate) fn snapshot(&self) -> Vec<ProgramStatusRecord> {
        self.records
            .iter()
            .cloned()
            .map(|mut record| {
                let mut id = record.id.as_str();
                while record.app.is_none() && !id.is_empty() {
                    id = id.rsplit_once('/').map_or("", |(parent, _)| parent);
                    record.app = self
                        .records
                        .iter()
                        .find(|parent| parent.id == id)
                        .and_then(|parent| parent.app.clone());
                }
                record
            })
            .collect()
    }
}

enum Report {
    Clear(String),
    Set(ProgramStatusRecord),
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
}

fn decode(value: &str, limit: usize) -> Option<String> {
    let bytes = STANDARD
        .decode(value)
        .or_else(|_| STANDARD_NO_PAD.decode(value))
        .ok()?;
    if bytes.len() > limit {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

fn parse(body: &str) -> Option<Report> {
    // Use the longer ST framing, also for BEL, as a permitted lower limit.
    if body.len() + 9 > 4096 {
        return None;
    }
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;
    for pair in body.split(':') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let (key, value) = (key.trim(), value.trim());
        // All limits apply to every occurrence, even overwritten fields.
        if key.len() > 16 {
            return None;
        }
        match key {
            "msg" if value.len() > 2732 => return None,
            "title" if value.len() > 256 => return None,
            "app" if value.len() > 32 => return None,
            "id" if value.len() > 128
                || value.split('/').count() > 8
                || value.split('/').any(|part| part.len() > 32) =>
            {
                return None;
            }
            _ => {}
        }
        if !pair.contains('=')
            || key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
        {
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => id = Some(value),
            "kind" => {
                kind = match value {
                    "permission" => Some(ProgramStatusKind::Permission),
                    "question" => Some(ProgramStatusKind::Question),
                    "auth" => Some(ProgramStatusKind::Auth),
                    _ => None,
                }
            }
            "progress" => {
                progress = if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
                    value.parse::<u8>().ok().filter(|v| *v <= 100)
                } else {
                    None
                }
            }
            "app" => app = segment(value).then(|| value.to_owned()),
            "title" => title = Some(decode(value, 192)?),
            "msg" => msg = Some(decode(value, 2048)?),
            _ => {}
        }
    }
    if id.is_some_and(|id| !id.split('/').all(segment)) {
        return None;
    }
    let id = id.unwrap_or("").to_owned();
    let state = match state? {
        "clear" => return Some(Report::Clear(id)),
        "idle" => ProgramState::Idle,
        "working" => ProgramState::Working,
        "done" => ProgramState::Done,
        "blocked" => ProgramState::Blocked,
        "error" => ProgramState::Error,
        _ => return None,
    };
    Some(Report::Set(ProgramStatusRecord {
        id,
        state,
        kind: (state == ProgramState::Blocked).then_some(kind).flatten(),
        progress: matches!(state, ProgramState::Working | ProgramState::Blocked)
            .then_some(progress)
            .flatten(),
        app,
        title,
        msg,
    }))
}

#[cfg(test)]
mod tests;
