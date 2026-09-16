//! Small GitHub-flavored markdown subset for overlay dialogs.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Strong(String),
    Emphasis(String),
    Code(String),
    Link { label: String, url: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        children: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    List {
        ordered: bool,
        items: Vec<Vec<Inline>>,
    },
    Quote(Vec<Inline>),
    Code {
        language: Option<String>,
        code: String,
    },
    Rule,
}

pub fn parse_markdown(source: &str) -> Vec<Block> {
    let source = strip_html_comments(source);
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            index += 1;
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("```") {
            let language = {
                let lang = rest.trim();
                (!lang.is_empty()).then(|| lang.to_string())
            };
            index += 1;
            let mut code = String::new();
            while index < lines.len() && lines[index].trim() != "```" {
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(lines[index]);
                index += 1;
            }
            if index < lines.len() {
                index += 1;
            }
            blocks.push(Block::Code { language, code });
            continue;
        }

        if is_rule(trimmed) {
            blocks.push(Block::Rule);
            index += 1;
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            blocks.push(Block::Heading {
                level,
                children: parse_inlines(text),
            });
            index += 1;
            continue;
        }

        if let Some(text) = trimmed.strip_prefix("> ") {
            let mut combined = text.to_string();
            index += 1;
            while index < lines.len() {
                let next = lines[index].trim();
                if let Some(rest) = next.strip_prefix("> ") {
                    combined.push(' ');
                    combined.push_str(rest);
                    index += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::Quote(parse_inlines(&combined)));
            continue;
        }

        if let Some((ordered, item)) = parse_list_item(trimmed) {
            let mut items = vec![parse_inlines(item)];
            index += 1;
            while index < lines.len() {
                let next = lines[index].trim();
                match parse_list_item(next) {
                    Some((next_ordered, next_item)) if next_ordered == ordered => {
                        items.push(parse_inlines(next_item));
                        index += 1;
                    }
                    _ => break,
                }
            }
            blocks.push(Block::List { ordered, items });
            continue;
        }

        let mut paragraph = trimmed.to_string();
        index += 1;
        while index < lines.len() {
            let next = lines[index].trim();
            if next.is_empty()
                || next.starts_with("```")
                || is_rule(next)
                || parse_heading(next).is_some()
                || next.starts_with("> ")
                || parse_list_item(next).is_some()
            {
                break;
            }
            paragraph.push(' ');
            paragraph.push_str(next);
            index += 1;
        }
        blocks.push(Block::Paragraph(parse_inlines(&paragraph)));
    }

    blocks
}

fn strip_html_comments(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("<!--") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 4..];
        match rest.find("-->") {
            Some(end) => rest = &rest[end + 3..],
            None => return output,
        }
    }
    output.push_str(rest);
    output
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    chars.len() >= 3
        && chars
            .iter()
            .all(|ch| *ch == '-' || *ch == '*' || *ch == '_')
        && chars.windows(2).all(|pair| pair[0] == pair[1])
}

fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let bytes = line.as_bytes();
    let mut level = 0usize;
    while level < bytes.len() && bytes[level] == b'#' && level < 6 {
        level += 1;
    }
    if level == 0 || level >= bytes.len() || bytes[level] != b' ' {
        return None;
    }
    Some((level as u8, line[level + 1..].trim()))
}

fn parse_list_item(line: &str) -> Option<(bool, &str)> {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        return Some((false, rest.trim()));
    }
    let bytes = trimmed.as_bytes();
    let mut digits = 0usize;
    while digits < bytes.len() && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits == 0 || digits + 1 >= bytes.len() {
        return None;
    }
    if (bytes[digits] == b'.' || bytes[digits] == b')') && bytes[digits + 1] == b' ' {
        return Some((true, trimmed[digits + 2..].trim()));
    }
    None
}

pub fn parse_inlines(input: &str) -> Vec<Inline> {
    let chars: Vec<char> = input.chars().collect();
    let mut inlines = Vec::new();
    let mut index = 0;
    let mut text = String::new();

    while index < chars.len() {
        if chars[index] == '`' {
            flush_text(&mut text, &mut inlines);
            index += 1;
            let mut code = String::new();
            while index < chars.len() && chars[index] != '`' {
                code.push(chars[index]);
                index += 1;
            }
            if index < chars.len() {
                index += 1;
            }
            inlines.push(Inline::Code(code));
            continue;
        }

        if starts_with(&chars, index, "**")
            && let Some((end, content)) = find_closing(&chars, index + 2, "**")
        {
            flush_text(&mut text, &mut inlines);
            inlines.push(Inline::Strong(content));
            index = end;
            continue;
        }

        if chars[index] == '*'
            && let Some((end, content)) = find_closing(&chars, index + 1, "*")
        {
            flush_text(&mut text, &mut inlines);
            inlines.push(Inline::Emphasis(content));
            index = end;
            continue;
        }

        if chars[index] == '['
            && let Some((label, url, end)) = parse_markdown_link(&chars, index)
        {
            flush_text(&mut text, &mut inlines);
            inlines.push(Inline::Link { label, url });
            index = end;
            continue;
        }

        if let Some((url, end)) = parse_autolink(&chars, index) {
            flush_text(&mut text, &mut inlines);
            inlines.push(Inline::Link {
                label: url.clone(),
                url,
            });
            index = end;
            continue;
        }

        text.push(chars[index]);
        index += 1;
    }

    flush_text(&mut text, &mut inlines);
    inlines
}

fn flush_text(text: &mut String, inlines: &mut Vec<Inline>) {
    if !text.is_empty() {
        inlines.push(Inline::Text(std::mem::take(text)));
    }
}

fn starts_with(chars: &[char], index: usize, token: &str) -> bool {
    token.chars().enumerate().all(|(offset, ch)| {
        chars
            .get(index + offset)
            .is_some_and(|candidate| *candidate == ch)
    })
}

fn find_closing(chars: &[char], start: usize, token: &str) -> Option<(usize, String)> {
    let mut index = start;
    while index < chars.len() {
        if starts_with(chars, index, token) {
            let content: String = chars[start..index].iter().collect();
            if !content.is_empty() {
                return Some((index + token.chars().count(), content));
            }
        }
        index += 1;
    }
    None
}

fn parse_markdown_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut index = start + 1;
    let label_start = index;
    while index < chars.len() && chars[index] != ']' {
        index += 1;
    }
    if index >= chars.len() || index + 1 >= chars.len() || chars[index + 1] != '(' {
        return None;
    }
    let label: String = chars[label_start..index].iter().collect();
    index += 2;
    let url_start = index;
    while index < chars.len() && chars[index] != ')' {
        index += 1;
    }
    if index >= chars.len() {
        return None;
    }
    let url: String = chars[url_start..index].iter().collect();
    if label.is_empty() || url.is_empty() {
        return None;
    }
    Some((label, url, index + 1))
}

fn parse_autolink(chars: &[char], start: usize) -> Option<(String, usize)> {
    const HTTP: &str = "http://";
    const HTTPS: &str = "https://";
    let prefix = if starts_with(chars, start, HTTPS) {
        HTTPS
    } else if starts_with(chars, start, HTTP) {
        HTTP
    } else {
        return None;
    };
    let mut end = start + prefix.chars().count();
    if end >= chars.len() {
        return None;
    }
    while end < chars.len() && is_url_char(chars[end]) {
        end += 1;
    }
    let mut url: String = chars[start..end].iter().collect();
    while url
        .chars()
        .last()
        .is_some_and(|ch| matches!(ch, '.' | ',' | ';' | ':' | ')'))
    {
        url.pop();
        end -= 1;
    }
    if url.len() <= prefix.len() {
        return None;
    }
    Some((url, end))
}

fn is_url_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '-' | '.'
                | '_'
                | '~'
                | ':'
                | '/'
                | '?'
                | '#'
                | '['
                | ']'
                | '@'
                | '!'
                | '$'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | '+'
                | ','
                | ';'
                | '='
                | '%'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_release_notes_subset() {
        let blocks = parse_markdown(
            "<!-- generated -->\n## What's Changed\n* feat: tabs by @dev in https://example.com/pull/1\n\n**Full Changelog**: [compare](https://example.com/compare)\n",
        );

        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 2,
                    children: vec![Inline::Text("What's Changed".to_string())],
                },
                Block::List {
                    ordered: false,
                    items: vec![vec![
                        Inline::Text("feat: tabs by @dev in ".to_string()),
                        Inline::Link {
                            label: "https://example.com/pull/1".to_string(),
                            url: "https://example.com/pull/1".to_string(),
                        },
                    ]],
                },
                Block::Paragraph(vec![
                    Inline::Strong("Full Changelog".to_string()),
                    Inline::Text(": ".to_string()),
                    Inline::Link {
                        label: "compare".to_string(),
                        url: "https://example.com/compare".to_string(),
                    },
                ]),
            ]
        );
    }

    #[test]
    fn parses_fenced_code_and_inline_code() {
        let blocks = parse_markdown("Use `gh`:\n\n```bash\ngh release view\n```\n");
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph(vec![
                    Inline::Text("Use ".to_string()),
                    Inline::Code("gh".to_string()),
                    Inline::Text(":".to_string()),
                ]),
                Block::Code {
                    language: Some("bash".to_string()),
                    code: "gh release view".to_string(),
                },
            ]
        );
    }
}
