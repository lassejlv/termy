/// Encode pasted text with Enter line endings, unsafe controls removed, and
/// optional bracketed-paste markers. Hosts decode their text before calling.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let mut body = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                body.push('\r');
            }
            '\n' => body.push('\r'),
            '\t' => body.push('\t'),
            // Other C0 controls, DEL and C1 controls could run commands or
            // end the bracket early. Dropping ESC also defeats nested end
            // markers such as `ESC[20 ESC[201~ 1~`.
            '\0'..='\x1f' | '\x7f'..='\u{9f}' => {}
            character => body.push(character),
        }
    }
    if !bracketed {
        return body.into_bytes();
    }
    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::encode_paste;

    #[test]
    fn paste_keeps_unicode_tabs_and_enter_line_endings() {
        assert_eq!(
            encode_paste("雪\t🙂\r\nx\ny\rz", false),
            "雪\t🙂\rx\ry\rz".as_bytes()
        );
    }

    #[test]
    fn controls_cannot_terminate_a_bracketed_paste() {
        let controls: String = (0..=0x9f)
            .filter(|code| *code <= 0x1f || *code >= 0x7f)
            .filter_map(char::from_u32)
            .collect();
        assert_eq!(encode_paste(&controls, true), b"\x1b[200~\t\r\r\x1b[201~");
        assert_eq!(
            encode_paste("x\x1b[20\x1b[201~1~rm\u{9b}201~", true),
            b"\x1b[200~x[20[201~1~rm201~\x1b[201~"
        );
    }
}
