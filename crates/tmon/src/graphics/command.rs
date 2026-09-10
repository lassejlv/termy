use super::{MAX_CONTROL_BYTES, MAX_CONTROL_FIELDS};

#[derive(Debug)]
pub(crate) struct GraphicsCommand {
    pub(super) control: Vec<(char, String)>,
    pub(super) payload: Vec<u8>,
    pub(super) oversized: bool,
}

impl GraphicsCommand {
    pub(crate) fn parse(mut bytes: Vec<u8>, oversized: bool) -> Self {
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
