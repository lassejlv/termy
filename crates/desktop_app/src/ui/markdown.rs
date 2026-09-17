//! GitHub-flavored markdown subset for overlay dialogs.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Strong(String),
    Emphasis(String),
    Code(String),
    Link { label: String, url: String },
    Image { alt: String, url: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    pub task: Option<bool>,
    pub children: Vec<Inline>,
    pub nested: Vec<Block>,
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
        items: Vec<ListItem>,
    },
    Quote(Vec<Inline>),
    Code {
        language: Option<String>,
        code: String,
    },
    Table {
        headers: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
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

        if let Some((table, next_index)) = try_parse_table(&lines, index) {
            blocks.push(table);
            index = next_index;
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

        if parse_list_marker(line).is_some() {
            blocks.push(parse_list(&lines, &mut index));
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
                || parse_list_marker(lines[index]).is_some()
                || try_parse_table(&lines, index).is_some()
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

pub fn markdown_image_urls(source: &str) -> Vec<String> {
    let mut urls = Vec::new();
    collect_image_urls_from_blocks(&parse_markdown(source), &mut urls);
    urls
}

fn collect_image_urls_from_blocks(blocks: &[Block], urls: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Heading { children, .. }
            | Block::Paragraph(children)
            | Block::Quote(children) => collect_image_urls_from_inlines(children, urls),
            Block::List { items, .. } => {
                for item in items {
                    collect_image_urls_from_inlines(&item.children, urls);
                    collect_image_urls_from_blocks(&item.nested, urls);
                }
            }
            Block::Table { headers, rows } => {
                for cell in headers {
                    collect_image_urls_from_inlines(cell, urls);
                }
                for row in rows {
                    for cell in row {
                        collect_image_urls_from_inlines(cell, urls);
                    }
                }
            }
            Block::Code { .. } | Block::Rule => {}
        }
    }
}

fn collect_image_urls_from_inlines(inlines: &[Inline], urls: &mut Vec<String>) {
    for inline in inlines {
        if let Inline::Image { url, .. } = inline
            && is_http_url(url)
            && !urls.iter().any(|existing| existing == url)
        {
            urls.push(url.clone());
        }
    }
}

pub fn is_http_url(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .is_some_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
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

struct ListMarker {
    indent: usize,
    ordered: bool,
    task: Option<bool>,
    text: String,
}

fn leading_indent(line: &str) -> usize {
    let mut indent = 0usize;
    for ch in line.chars() {
        match ch {
            ' ' => indent += 1,
            '\t' => indent += 4,
            _ => break,
        }
    }
    indent
}

fn parse_task_prefix(rest: &str) -> (Option<bool>, &str) {
    if rest == "[ ]" {
        return (Some(false), "");
    }
    if rest == "[x]" || rest == "[X]" {
        return (Some(true), "");
    }
    if let Some(text) = rest.strip_prefix("[ ] ") {
        return (Some(false), text);
    }
    if let Some(text) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        return (Some(true), text);
    }
    (None, rest)
}

fn parse_list_marker(line: &str) -> Option<ListMarker> {
    if line.trim().is_empty() {
        return None;
    }
    let indent = leading_indent(line);
    let trimmed = line.trim_start();
    let (ordered, rest) = if let Some(rest) = trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
    {
        (false, rest)
    } else {
        let bytes = trimmed.as_bytes();
        let mut digits = 0usize;
        while digits < bytes.len() && bytes[digits].is_ascii_digit() {
            digits += 1;
        }
        if digits == 0 || digits + 1 >= bytes.len() {
            return None;
        }
        if (bytes[digits] == b'.' || bytes[digits] == b')') && bytes[digits + 1] == b' ' {
            (true, trimmed[digits + 2..].trim())
        } else {
            return None;
        }
    };
    let (task, text) = parse_task_prefix(rest.trim());
    Some(ListMarker {
        indent,
        ordered,
        task,
        text: text.to_string(),
    })
}

fn parse_list(lines: &[&str], index: &mut usize) -> Block {
    let first = parse_list_marker(lines[*index]).expect("list marker");
    let ordered = first.ordered;
    let base_indent = first.indent;
    let mut items: Vec<ListItem> = Vec::new();

    while *index < lines.len() {
        let line = lines[*index];
        if line.trim().is_empty() {
            let nested_after_blank = lines
                .get(*index + 1)
                .copied()
                .and_then(parse_list_marker)
                .is_some_and(|marker| marker.indent >= base_indent);
            if nested_after_blank {
                *index += 1;
                continue;
            }
            break;
        }

        if let Some(marker) = parse_list_marker(line) {
            if marker.indent < base_indent || marker.ordered != ordered {
                break;
            }
            if marker.indent > base_indent {
                if let Some(last) = items.last_mut() {
                    last.nested.push(parse_list(lines, index));
                    continue;
                }
                break;
            }

            *index += 1;
            let mut text = marker.text;
            while *index < lines.len() {
                let continuation = lines[*index];
                if continuation.trim().is_empty() || parse_list_marker(continuation).is_some() {
                    break;
                }
                if leading_indent(continuation) > base_indent {
                    text.push(' ');
                    text.push_str(continuation.trim());
                    *index += 1;
                } else {
                    break;
                }
            }
            items.push(ListItem {
                task: marker.task,
                children: parse_inlines(&text),
                nested: Vec::new(),
            });
            continue;
        }

        break;
    }

    Block::List { ordered, items }
}

fn looks_like_table_row(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.chars().filter(|ch| *ch == '|').count() >= 2
}

fn split_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let without_edges = trimmed
        .strip_prefix('|')
        .unwrap_or(trimmed)
        .strip_suffix('|')
        .unwrap_or(trimmed.strip_prefix('|').unwrap_or(trimmed));
    without_edges
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn is_table_separator(line: &str) -> bool {
    if !looks_like_table_row(line) {
        return false;
    }
    let cells = split_table_row(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let trimmed = cell.trim().trim_matches(':').trim();
            !trimmed.is_empty() && trimmed.chars().all(|ch| ch == '-')
        })
}

fn try_parse_table(lines: &[&str], index: usize) -> Option<(Block, usize)> {
    let header_line = lines.get(index)?;
    let separator_line = lines.get(index + 1)?;
    if !looks_like_table_row(header_line) || !is_table_separator(separator_line) {
        return None;
    }
    let headers = split_table_row(header_line);
    if headers.is_empty() {
        return None;
    }
    let column_count = headers.len();
    let mut rows = Vec::new();
    let mut next = index + 2;
    while next < lines.len() {
        let line = lines[next];
        if !looks_like_table_row(line) {
            break;
        }
        let mut cells = split_table_row(line);
        cells.resize(column_count, String::new());
        cells.truncate(column_count);
        rows.push(cells.into_iter().map(|cell| parse_inlines(&cell)).collect());
        next += 1;
    }
    Some((
        Block::Table {
            headers: headers
                .into_iter()
                .map(|cell| parse_inlines(&cell))
                .collect(),
            rows,
        },
        next,
    ))
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

        if chars[index] == '!'
            && chars.get(index + 1) == Some(&'[')
            && let Some((alt, url, end)) = parse_markdown_link(&chars, index + 1)
        {
            flush_text(&mut text, &mut inlines);
            inlines.push(Inline::Image { alt, url });
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
    if url.is_empty() {
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
                    items: vec![ListItem {
                        task: None,
                        children: vec![
                            Inline::Text("feat: tabs by @dev in ".to_string()),
                            Inline::Link {
                                label: "https://example.com/pull/1".to_string(),
                                url: "https://example.com/pull/1".to_string(),
                            },
                        ],
                        nested: Vec::new(),
                    }],
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

    #[test]
    fn parses_nested_lists_task_lists_tables_and_images() {
        let blocks = parse_markdown(
            "- parent\n  - child\n- [x] done\n- [ ] todo\n\n| Feature | Status |\n| --- | --- |\n| Tabs | shipped |\n\n![Hero](https://example.com/hero.png)\n",
        );

        assert_eq!(
            blocks,
            vec![
                Block::List {
                    ordered: false,
                    items: vec![
                        ListItem {
                            task: None,
                            children: vec![Inline::Text("parent".to_string())],
                            nested: vec![Block::List {
                                ordered: false,
                                items: vec![ListItem {
                                    task: None,
                                    children: vec![Inline::Text("child".to_string())],
                                    nested: Vec::new(),
                                }],
                            }],
                        },
                        ListItem {
                            task: Some(true),
                            children: vec![Inline::Text("done".to_string())],
                            nested: Vec::new(),
                        },
                        ListItem {
                            task: Some(false),
                            children: vec![Inline::Text("todo".to_string())],
                            nested: Vec::new(),
                        },
                    ],
                },
                Block::Table {
                    headers: vec![
                        vec![Inline::Text("Feature".to_string())],
                        vec![Inline::Text("Status".to_string())],
                    ],
                    rows: vec![vec![
                        vec![Inline::Text("Tabs".to_string())],
                        vec![Inline::Text("shipped".to_string())],
                    ]],
                },
                Block::Paragraph(vec![Inline::Image {
                    alt: "Hero".to_string(),
                    url: "https://example.com/hero.png".to_string(),
                }]),
            ]
        );
        assert_eq!(
            markdown_image_urls(
                "![Hero](https://example.com/hero.png)\n![skip](file:///tmp/x.png)"
            ),
            vec!["https://example.com/hero.png".to_string()]
        );
    }
}
