use super::{MAX_COMMAND_BYTES, MAX_CONTROL_BYTES, MAX_CONTROL_FIELDS};

#[derive(Clone, Debug)]
pub struct KittyGraphicsCommand {
    pub(super) control: Vec<(char, String)>,
    pub(super) payload: Vec<u8>,
    pub(super) oversized: bool,
}

impl KittyGraphicsCommand {
    /// Effective action, allowing hosts to collect grid coordinates only for deletion.
    pub fn action(&self) -> char {
        self.char_value('a').unwrap_or('t')
    }

    pub(super) fn parse(mut bytes: Vec<u8>, oversized: bool) -> Self {
        // A missing separator must not turn a payload-sized command into a control string.
        // Inspect one byte past the limit so a separator at the exact boundary is accepted.
        let separator = bytes[..bytes.len().min(MAX_CONTROL_BYTES + 1)]
            .iter()
            .position(|byte| *byte == b';');
        let control_oversized = separator.is_none() && bytes.len() > MAX_CONTROL_BYTES;
        let control_end = separator.unwrap_or(bytes.len().min(MAX_CONTROL_BYTES));
        let field_count = bytes[..control_end]
            .split(|byte| *byte == b',')
            .take(MAX_CONTROL_FIELDS + 1)
            .count();
        let fields_oversized = field_count > MAX_CONTROL_FIELDS;
        let control = bytes[..control_end]
            .split(|byte| *byte == b',')
            .take(MAX_CONTROL_FIELDS)
            .filter_map(|field| {
                let (&key, value) = field.split_first()?;
                let value = value.strip_prefix(b"=")?;
                if !key.is_ascii() {
                    return None;
                }
                let value = std::str::from_utf8(value).ok()?;
                Some((char::from(key), value.to_owned()))
            })
            .collect();
        let oversized = oversized || control_oversized || fields_oversized;
        let payload = if oversized {
            // Drop an already-rejected payload with its original allocation instead of
            // shifting up to MAX_COMMAND_BYTES only for `apply` to discard it.
            Vec::new()
        } else if let Some(index) = separator {
            // Keep the potentially large payload in the parser's allocation. Draining the
            // small control prefix moves it in place instead of cloning it into another Vec.
            bytes.drain(..=index);
            bytes
        } else {
            Vec::new()
        };
        Self {
            control,
            payload,
            oversized,
        }
    }

    pub(super) fn validate(&self) -> Result<(), &'static str> {
        for (key, value) in &self.control {
            let valid = match key {
                'a' => matches!(
                    value.as_str(),
                    "t" | "T" | "q" | "p" | "d" | "f" | "a" | "c"
                ),
                't' => matches!(value.as_str(), "d" | "f" | "t" | "s"),
                'o' => value == "z",
                'd' => value.len() == 1 && "aAiInNcCfFpPqQrRxXyYzZ".contains(value),
                'z' | 'H' | 'V' => value.parse::<i32>().is_ok(),
                'f' | 's' | 'v' | 'i' | 'I' | 'p' | 'q' | 'm' | 'x' | 'y' | 'w' | 'h' | 'X'
                | 'Y' | 'c' | 'r' | 'C' | 'U' | 'P' | 'Q' | 'S' | 'O' | 'N' => {
                    value.parse::<u32>().is_ok()
                }
                _ => true,
            };
            if !valid {
                return Err("EINVAL:invalid graphics control value");
            }
        }
        if self.u32_value('q').is_some_and(|n| n > 2)
            || self.u32_value('m').is_some_and(|n| n > 1)
            || self.u32_value('C').is_some_and(|n| n > 1)
            || self.u32_value('U').is_some_and(|n| n > 1)
        {
            return Err("EINVAL:invalid graphics control value");
        }
        Ok(())
    }

    pub(super) fn value(&self, key: char) -> Option<&str> {
        self.control
            .iter()
            .rev()
            .find_map(|(candidate, value)| (*candidate == key).then_some(value.as_str()))
    }

    pub(super) fn char_value(&self, key: char) -> Option<char> {
        let mut chars = self.value(key)?.chars();
        let value = chars.next()?;
        chars.next().is_none().then_some(value)
    }

    pub(super) fn u32_value(&self, key: char) -> Option<u32> {
        self.value(key)?.parse().ok()
    }

    pub(super) fn i32_value(&self, key: char) -> Option<i32> {
        self.value(key)?.parse().ok()
    }
}

#[derive(Clone, Debug)]
pub enum KittyGraphicsItem {
    Text(Vec<u8>),
    Command(KittyGraphicsCommand),
}

#[derive(Clone, Copy, Debug, Default)]
enum InterceptorState {
    #[default]
    Ground,
    Escape,
    ApcStart {
        c1: bool,
    },
    OtherApc,
    OtherApcEscape,
    Kitty,
    KittyEscape,
}

#[derive(Default)]
pub struct KittyGraphicsInterceptor {
    state: InterceptorState,
    command: Vec<u8>,
    pub(super) oversized: bool,
    utf8_remaining: u8,
}

impl KittyGraphicsInterceptor {
    pub fn process(&mut self, bytes: &[u8]) -> Vec<KittyGraphicsItem> {
        let mut items = Vec::new();
        let mut text = Vec::with_capacity(bytes.len().min(8192));

        let flush_text = |items: &mut Vec<KittyGraphicsItem>, text: &mut Vec<u8>| {
            if !text.is_empty() {
                items.push(KittyGraphicsItem::Text(std::mem::take(text)));
            }
        };

        for &byte in bytes {
            if matches!(self.state, InterceptorState::Ground) {
                if self.utf8_remaining > 0 && byte & 0xc0 == 0x80 {
                    self.utf8_remaining -= 1;
                    text.push(byte);
                    continue;
                }
                self.utf8_remaining = match byte {
                    0xc2..=0xdf => 1,
                    0xe0..=0xef => 2,
                    0xf0..=0xf4 => 3,
                    _ => 0,
                };
            }
            match self.state {
                InterceptorState::Ground => match byte {
                    0x1b => self.state = InterceptorState::Escape,
                    0x9f => self.state = InterceptorState::ApcStart { c1: true },
                    _ => text.push(byte),
                },
                InterceptorState::Escape => {
                    if byte == b'_' {
                        self.state = InterceptorState::ApcStart { c1: false };
                    } else if byte == 0x1b {
                        text.push(0x1b);
                        self.state = InterceptorState::Escape;
                    } else {
                        text.push(0x1b);
                        text.push(byte);
                        self.state = InterceptorState::Ground;
                    }
                }
                InterceptorState::ApcStart { c1 } => {
                    if byte == b'G' {
                        flush_text(&mut items, &mut text);
                        self.command.clear();
                        self.oversized = false;
                        self.state = InterceptorState::Kitty;
                    } else {
                        if c1 {
                            text.push(0x9f);
                        } else {
                            text.extend_from_slice(b"\x1b_");
                        }
                        text.push(byte);
                        self.state = InterceptorState::OtherApc;
                    }
                }
                InterceptorState::OtherApc => {
                    text.push(byte);
                    if byte == 0x9c {
                        self.state = InterceptorState::Ground;
                    } else if byte == 0x1b {
                        self.state = InterceptorState::OtherApcEscape;
                    }
                }
                InterceptorState::OtherApcEscape => {
                    text.push(byte);
                    if byte == b'\\' || byte == 0x9c {
                        self.state = InterceptorState::Ground;
                    } else if byte != 0x1b {
                        self.state = InterceptorState::OtherApc;
                    }
                }
                InterceptorState::Kitty => {
                    if byte == 0x1b {
                        self.state = InterceptorState::KittyEscape;
                    } else if byte == 0x9c {
                        flush_text(&mut items, &mut text);
                        items.push(KittyGraphicsItem::Command(KittyGraphicsCommand::parse(
                            std::mem::take(&mut self.command),
                            self.oversized,
                        )));
                        self.state = InterceptorState::Ground;
                    } else if self.command.len() < MAX_COMMAND_BYTES {
                        self.command.push(byte);
                    } else {
                        self.oversized = true;
                    }
                }
                InterceptorState::KittyEscape => {
                    if byte == b'\\' {
                        flush_text(&mut items, &mut text);
                        items.push(KittyGraphicsItem::Command(KittyGraphicsCommand::parse(
                            std::mem::take(&mut self.command),
                            self.oversized,
                        )));
                        self.state = InterceptorState::Ground;
                    } else {
                        if self.command.len() < MAX_COMMAND_BYTES {
                            self.command.push(0x1b);
                            self.command.push(byte);
                        } else {
                            self.oversized = true;
                        }
                        self.state = InterceptorState::Kitty;
                    }
                }
            }
        }
        flush_text(&mut items, &mut text);
        items
    }
}
