use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::terminal_engine::{Cell, Engine, Style};

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DetectedLink {
    pub start_col: usize,
    pub end_col: usize,
    pub target: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DetectedViewportLink {
    pub start_row: usize,
    pub start_col: usize,
    pub end_row: usize,
    pub end_col: usize,
    pub target: String,
}

const FILE_URL_CACHE_CAPACITY: usize = 128;
/// A path that didn't exist may be created any moment (a build output, a file
/// being written), so a failed lookup is only remembered briefly.
const FAILED_FILE_URL_TTL: Duration = Duration::from_secs(2);
/// How far a hover scans for the ends of a link, in cells each way. A longer
/// run without whitespace (minified code, base64) is not treated as a link,
/// so hovering it can't walk megabytes of soft-wrapped history.
const MAX_LINK_SCAN_CELLS: usize = 4096;

#[derive(Default)]
struct FileUrlLruCache {
    capacity: usize,
    entries: HashMap<String, (Option<String>, Instant)>,
    order: VecDeque<String>,
}

impl FileUrlLruCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &str, now: Instant) -> Option<Option<String>> {
        let (value, stored_at) = self.entries.get(key).cloned()?;
        if value.is_none() && now.saturating_duration_since(stored_at) >= FAILED_FILE_URL_TTL {
            return None;
        }
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: Option<String>, now: Instant) {
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), (value, now));
            self.touch(&key);
            return;
        }

        self.entries.insert(key.clone(), (value, now));
        self.order.push_back(key);

        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn touch(&mut self, key: &str) {
        if let Some(pos) = self.order.iter().position(|existing| existing == key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.to_string());
    }
}

fn file_url_cache() -> &'static Mutex<FileUrlLruCache> {
    static CACHE: OnceLock<Mutex<FileUrlLruCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(FileUrlLruCache::new(FILE_URL_CACHE_CAPACITY)))
}

fn lookup_cached_file_url(token: &str) -> Option<Option<String>> {
    let mut cache = file_url_cache().lock().ok()?;
    cache.get(token, Instant::now())
}

fn store_cached_file_url(token: &str, resolved: Option<String>) {
    if let Ok(mut cache) = file_url_cache().lock() {
        cache.insert(token.to_string(), resolved, Instant::now());
    }
}

/// The OSC 8 hyperlink under a viewport cell, if any.
///
/// Expands to the contiguous run of cells on that row carrying the same
/// hyperlink (matched by OSC 8 id + uri), so hover underlines cover the whole
/// link text even when it differs from the target URI. Takes priority over the
/// heuristic [`find_link_in_line`] detection.
pub(crate) fn hyperlink_at_viewport_cell(
    term: &Engine,
    row: usize,
    col: usize,
) -> Option<DetectedLink> {
    let grid = term;
    let columns = grid.size().cols;
    if row >= grid.size().rows || col >= columns {
        return None;
    }

    // Viewport rows map directly into the live/history grid after subtracting
    // display_offset. Index that one row instead of materializing hyperlink
    // metadata for every visible cell on every mouse-hover lookup.
    let line = row as i32 - grid.display_offset() as i32;
    let row_cells = grid.line(line)?;
    let target = row_cells.hyperlink(col)?;
    let matches_target = |candidate_col: usize| {
        row_cells
            .hyperlink(candidate_col)
            .is_some_and(|other| other == target)
    };

    let mut start_col = col;
    while start_col > 0 && matches_target(start_col - 1) {
        start_col -= 1;
    }
    let mut end_col = col;
    while end_col + 1 < columns && matches_target(end_col + 1) {
        end_col += 1;
    }

    Some(DetectedLink {
        start_col,
        end_col,
        target: target.uri.clone(),
    })
}

/// The link under a viewport cell, including links spanning soft-wrapped rows.
///
/// OSC 8 metadata takes priority over heuristic URL detection. The returned
/// range is clipped to the visible viewport while the target is built from the
/// complete logical link, including portions currently in scrollback.
#[cfg(test)]
pub(crate) fn link_at_viewport_cell(
    term: &Engine,
    row: usize,
    col: usize,
) -> Option<DetectedViewportLink> {
    link_candidate_at_viewport_cell(term, row, col)?.resolve()
}

/// A link under a viewport cell whose target may still need a filesystem
/// lookup. Finding it only reads the grid; [`Self::resolve`] does the lookup,
/// so a caller holding the engine lock can release it in between.
pub(crate) struct ViewportLinkCandidate {
    link: DetectedViewportLink,
    /// The token to classify into `link.target`, and the pane's working
    /// directory for relative paths. `None` when the target is already known.
    token: Option<(String, Option<PathBuf>)>,
}

impl ViewportLinkCandidate {
    pub(crate) fn resolve(self) -> Option<DetectedViewportLink> {
        let mut link = self.link;
        if let Some((token, base_dir)) = self.token {
            link.target = classify_link_token_in(&token, base_dir.as_deref())?;
        }
        Some(link)
    }
}

/// [`link_at_viewport_cell`] up to, not including, any filesystem lookup.
pub(crate) fn link_candidate_at_viewport_cell(
    term: &Engine,
    row: usize,
    col: usize,
) -> Option<ViewportLinkCandidate> {
    let grid = term;
    let columns = grid.size().cols;
    let screen_lines = grid.size().rows;
    if row >= screen_lines || col >= columns || columns == 0 {
        return None;
    }

    let display_offset = i32::try_from(grid.display_offset()).ok()?;
    let hovered = GridPosition {
        line: i32::try_from(row).ok()?.saturating_sub(display_offset),
        col,
    };
    let bounds = grid_line_bounds(grid)?;

    if let Some(target) = grid.line(hovered.line)?.hyperlink(hovered.col) {
        let target_uri = target.uri.clone();
        // An OSC 8 run longer than the scan window is clipped to it; the
        // target is already known.
        let mut start = hovered;
        for _ in 0..MAX_LINK_SCAN_CELLS {
            let Some(previous) = previous_wrapped_position(grid, start, bounds, columns) else {
                break;
            };
            if grid
                .line(previous.line)?
                .hyperlink(previous.col)
                .is_some_and(|candidate| candidate == target)
            {
                start = previous;
            } else {
                break;
            }
        }

        let mut end = hovered;
        for _ in 0..MAX_LINK_SCAN_CELLS {
            let Some(next) = next_wrapped_position(grid, end, bounds, columns) else {
                break;
            };
            if grid
                .line(next.line)?
                .hyperlink(next.col)
                .is_some_and(|candidate| candidate == target)
            {
                end = next;
            } else {
                break;
            }
        }

        let link = viewport_link_from_grid_range(
            start,
            end,
            target_uri,
            display_offset,
            screen_lines,
            columns,
        )?;
        return Some(ViewportLinkCandidate { link, token: None });
    }

    let hovered_char = grid_cell_char(grid, hovered);
    if hovered_char.is_whitespace() {
        return None;
    }

    let mut positions_before = Vec::new();
    let mut cursor = hovered;
    while let Some(previous) = previous_wrapped_position(grid, cursor, bounds, columns) {
        let candidate = grid_cell_char(grid, previous);
        if candidate.is_whitespace() {
            break;
        }
        if positions_before.len() == MAX_LINK_SCAN_CELLS {
            return None;
        }
        positions_before.push((previous, candidate));
        cursor = previous;
    }
    positions_before.reverse();

    let mut positions = Vec::with_capacity(positions_before.len().saturating_add(16));
    positions.extend(positions_before);
    let hovered_index = positions.len();
    positions.push((hovered, hovered_char));

    cursor = hovered;
    while let Some(next) = next_wrapped_position(grid, cursor, bounds, columns) {
        let candidate = grid_cell_char(grid, next);
        if candidate.is_whitespace() {
            break;
        }
        if positions.len() - hovered_index > MAX_LINK_SCAN_CELLS {
            return None;
        }
        positions.push((next, candidate));
        cursor = next;
    }

    let line: Vec<char> = positions.iter().map(|(_, character)| *character).collect();
    let (start_col, end_col, token) = link_token_in_line(&line, hovered_index)?;
    let start = positions.get(start_col)?.0;
    let end = positions.get(end_col)?.0;
    let link = viewport_link_from_grid_range(
        start,
        end,
        String::new(),
        display_offset,
        screen_lines,
        columns,
    )?;
    let base_dir = grid.working_directory().and_then(working_directory_base);
    Some(ViewportLinkCandidate {
        link,
        token: Some((token, base_dir)),
    })
}

/// The pane's OSC 7 directory as a base for relative links, when it is a local
/// absolute path.
fn working_directory_base(directory: &str) -> Option<PathBuf> {
    // `file://host/C:/dir` used to be reported as `/C:/dir`.
    #[cfg(windows)]
    let directory = match directory.strip_prefix('/') {
        Some(stripped) if has_windows_drive_prefix(stripped) => stripped,
        _ => directory,
    };
    Some(PathBuf::from(directory)).filter(|path| path.is_absolute())
}

#[derive(Clone, Copy)]
struct GridPosition {
    line: i32,
    col: usize,
}

fn grid_line_bounds(grid: &Engine) -> Option<(i32, i32)> {
    let screen_lines = i32::try_from(grid.size().rows).ok()?;
    let total_lines = screen_lines.checked_add(i32::try_from(grid.history_size()).ok()?)?;
    if screen_lines <= 0 || total_lines <= 0 {
        return None;
    }
    Some((-(total_lines - screen_lines), screen_lines - 1))
}

fn previous_wrapped_position(
    grid: &Engine,
    position: GridPosition,
    (min_line, _): (i32, i32),
    columns: usize,
) -> Option<GridPosition> {
    if position.col > 0 {
        return Some(GridPosition {
            line: position.line,
            col: position.col - 1,
        });
    }
    let previous_line = position.line.checked_sub(1)?;
    if previous_line < min_line {
        return None;
    }
    let previous_col = columns.checked_sub(1)?;
    grid.line_wrapped(previous_line).then_some(GridPosition {
        line: previous_line,
        col: previous_col,
    })
}

fn next_wrapped_position(
    grid: &Engine,
    position: GridPosition,
    (_, max_line): (i32, i32),
    columns: usize,
) -> Option<GridPosition> {
    if position.col + 1 < columns {
        return Some(GridPosition {
            line: position.line,
            col: position.col + 1,
        });
    }
    if position.line >= max_line || !grid.line_wrapped(position.line) {
        return None;
    }
    Some(GridPosition {
        line: position.line + 1,
        col: 0,
    })
}

fn grid_cell_char(grid: &Engine, position: GridPosition) -> char {
    let Some(cell) = grid
        .line(position.line)
        .and_then(|row| row.cells().get(position.col))
    else {
        return ' ';
    };
    if cell.flags & (Cell::WIDE_SPACER | Cell::LEADING_WIDE_SPACER) != 0
        || cell.style.attributes & Style::HIDDEN != 0
        || cell.character == '\0'
        || cell.character.is_control()
    {
        ' '
    } else {
        cell.character
    }
}

fn viewport_link_from_grid_range(
    start: GridPosition,
    end: GridPosition,
    target: String,
    display_offset: i32,
    screen_lines: usize,
    columns: usize,
) -> Option<DetectedViewportLink> {
    let viewport_min_line = -display_offset;
    let viewport_max_line = i32::try_from(screen_lines)
        .ok()?
        .checked_sub(1)?
        .saturating_sub(display_offset);
    let visible_start_line = start.line.max(viewport_min_line);
    let visible_end_line = end.line.min(viewport_max_line);
    if visible_start_line > visible_end_line {
        return None;
    }

    Some(DetectedViewportLink {
        start_row: usize::try_from(visible_start_line.saturating_add(display_offset)).ok()?,
        start_col: if start.line < visible_start_line {
            0
        } else {
            start.col
        },
        end_row: usize::try_from(visible_end_line.saturating_add(display_offset)).ok()?,
        end_col: if end.line > visible_end_line {
            columns.saturating_sub(1)
        } else {
            end.col
        },
        target,
    })
}

pub fn find_link_in_line(line: &[char], col: usize) -> Option<DetectedLink> {
    let (start_col, end_col, token) = link_token_in_line(line, col)?;
    Some(DetectedLink {
        start_col,
        end_col,
        target: classify_link_token(&token)?,
    })
}

/// The bounds of the whitespace-delimited token at `col`, minus edge
/// punctuation, and the text to classify.
fn link_token_in_line(line: &[char], col: usize) -> Option<(usize, usize, String)> {
    if col >= line.len() || line[col].is_whitespace() {
        return None;
    }

    let mut start = col;
    while start > 0 && !line[start - 1].is_whitespace() {
        start -= 1;
    }

    let mut end = col;
    while end + 1 < line.len() && !line[end + 1].is_whitespace() {
        end += 1;
    }

    // Leading dots are punctuation unless they start a relative path.
    while start <= end
        && edge_trim_char(line[start])
        && !starts_with_relative_prefix(&line[start..=end])
    {
        start += 1;
    }
    while end >= start && edge_trim_char(line[end]) {
        if end == 0 {
            break;
        }
        // Keep the closing parenthesis of an MSVC `file.cpp(12,3)` location.
        if cfg!(windows)
            && line[end] == ')'
            && msvc_location_start(&line[start..=end].iter().collect::<String>()).is_some()
        {
            break;
        }
        end -= 1;
    }

    if start > end {
        return None;
    }

    let token: String = line[start..=end].iter().collect();
    let token = token.trim_end_matches(':').to_string();
    Some((start, end, token))
}

fn starts_with_relative_prefix(token: &[char]) -> bool {
    matches!(token, ['.', '/', ..] | ['.', '.', '/', ..])
        || (cfg!(windows) && matches!(token, ['.', '\\', ..] | ['.', '.', '\\', ..]))
}

/// `./` and `../` paths, plus `.\` and `..\` on Windows.
fn is_relative_link_path(path: &str) -> bool {
    path.starts_with("./")
        || path.starts_with("../")
        || (cfg!(windows) && (path.starts_with(".\\") || path.starts_with("..\\")))
}

/// `~/` home paths, plus `~\` on Windows.
fn strip_home_prefix(path: &str) -> Option<&str> {
    path.strip_prefix("~/").or_else(|| {
        if cfg!(windows) {
            path.strip_prefix("~\\")
        } else {
            None
        }
    })
}

/// [`classify_link_token_in`] with relative paths resolved against the
/// process's working directory.
pub fn classify_link_token(token: &str) -> Option<String> {
    classify_link_token_in(token, None)
}

/// The link target a token stands for. Relative paths (`./`, `../`, and `.\`,
/// `..\` on Windows) resolve against `base_dir`, normally the pane's OSC 7
/// working directory, or the process's working directory without one.
pub fn classify_link_token_in(token: &str, base_dir: Option<&Path>) -> Option<String> {
    if token.is_empty() {
        return None;
    }

    let lower = token.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(token.to_string());
    }

    if lower.starts_with("www.") {
        return Some(format!("https://{token}"));
    }

    if lower.starts_with("file://") {
        return normalize_file_url_token(token);
    }

    if looks_like_file_path(token) {
        return canonicalize_path_to_file_url(token, base_dir);
    }

    if is_ipv4_with_optional_port_and_path(token) || looks_like_domain(token) {
        return Some(format!("http://{token}"));
    }

    None
}

fn normalize_file_url_token(token: &str) -> Option<String> {
    let raw_path = token.get("file://".len()..)?;
    let local_path = extract_local_path_from_file_url(raw_path)?;
    canonicalize_path_to_file_url(&local_path, None)
}

#[cfg(unix)]
fn extract_local_path_from_file_url(raw_path: &str) -> Option<String> {
    if raw_path.starts_with('/') {
        return Some(raw_path.to_string());
    }

    let (host, path) = raw_path.split_once('/')?;
    if host.eq_ignore_ascii_case("localhost") {
        return Some(format!("/{path}"));
    }

    None
}

#[cfg(windows)]
fn extract_local_path_from_file_url(raw_path: &str) -> Option<String> {
    if let Some(stripped) = raw_path.strip_prefix('/')
        && has_windows_drive_prefix(stripped)
    {
        return Some(stripped.to_string());
    }

    if has_windows_drive_prefix(raw_path) || Path::new(raw_path).is_absolute() {
        return Some(raw_path.to_string());
    }

    let (host, path) = raw_path.split_once('/')?;
    if !host.eq_ignore_ascii_case("localhost") {
        return None;
    }

    if let Some(stripped) = path.strip_prefix('/')
        && has_windows_drive_prefix(stripped)
    {
        return Some(stripped.to_string());
    }

    if has_windows_drive_prefix(path) || Path::new(path).is_absolute() {
        return Some(path.to_string());
    }

    None
}

#[cfg(not(any(unix, windows)))]
fn extract_local_path_from_file_url(_: &str) -> Option<String> {
    None
}

fn canonicalize_path_to_file_url(token: &str, base_dir: Option<&Path>) -> Option<String> {
    let path = strip_line_col_suffix(token);
    if path.is_empty() {
        return None;
    }
    // Joining first also keys the cache by directory for relative paths.
    let joined;
    let normalized_key = match base_dir {
        Some(base_dir) if is_relative_link_path(path) => {
            joined = base_dir.join(path).to_string_lossy().into_owned();
            joined.as_str()
        }
        _ => path,
    };

    if let Some(cached) = lookup_cached_file_url(normalized_key) {
        return cached;
    }

    let resolved = canonicalize_path_to_file_url_uncached(normalized_key);
    store_cached_file_url(normalized_key, resolved.clone());
    resolved
}

#[cfg(unix)]
fn canonicalize_path_to_file_url_uncached(token: &str) -> Option<String> {
    let raw_path = strip_line_col_suffix(token);
    if raw_path.is_empty() {
        return None;
    }

    let path = expand_tilde_path(raw_path).unwrap_or_else(|| PathBuf::from(raw_path));
    let canonical = std::fs::canonicalize(path).ok()?;
    let canonical = canonical.to_string_lossy().replace('\\', "/");
    if !canonical.starts_with('/') {
        return None;
    }

    Some(format!("file:///{}", canonical.trim_start_matches('/')))
}

#[cfg(any(unix, windows))]
fn expand_tilde_path(path: &str) -> Option<PathBuf> {
    let remainder = strip_home_prefix(path)?;
    let home = dirs::home_dir()?;
    Some(home.join(remainder))
}

#[cfg(windows)]
fn canonicalize_path_to_file_url_uncached(token: &str) -> Option<String> {
    let mut raw_path = strip_line_col_suffix(token);
    if raw_path.is_empty() {
        return None;
    }

    if let Some(stripped) = raw_path.strip_prefix('/')
        && has_windows_drive_prefix(stripped)
    {
        raw_path = stripped;
    }

    let path = expand_tilde_path(raw_path).unwrap_or_else(|| PathBuf::from(raw_path));
    // Relative paths without a pane directory resolve against the process's
    // one. A rooted path with no drive (`/usr/lib`) is MSYS or WSL output that
    // names another filesystem, so it is not a local link.
    if !path.is_absolute() && !is_relative_link_path(raw_path) {
        return None;
    }

    let canonical = dunce::canonicalize(path).ok()?;
    let canonical = canonical.to_string_lossy();
    // dunce keeps the verbatim prefix when dropping it would change the path
    // (long paths, reserved names); a file URL has no verbatim form anyway.
    let canonical = match canonical.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => canonical
            .strip_prefix(r"\\?\")
            .unwrap_or(&canonical)
            .to_string(),
    };
    let canonical = canonical.replace('\\', "/");

    // `\\server\share\dir` becomes `file://server/share/dir`.
    if let Some(share) = canonical.strip_prefix("//") {
        return (!share.is_empty()).then(|| format!("file://{share}"));
    }

    if !has_windows_drive_prefix(&canonical) {
        return None;
    }

    let drive = canonical.chars().next()?.to_ascii_uppercase();
    let path = canonical[2..].trim_start_matches('/');

    if path.is_empty() {
        Some(format!("file:///{drive}:/"))
    } else {
        Some(format!("file:///{drive}:/{path}"))
    }
}

#[cfg(not(any(unix, windows)))]
fn canonicalize_path_to_file_url_uncached(_: &str) -> Option<String> {
    None
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn edge_trim_char(c: char) -> bool {
    matches!(
        c,
        '\'' | '"'
            | '`'
            | ','
            | '.'
            | ';'
            | '!'
            | '?'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '<'
            | '>'
    )
}

fn is_ipv4_with_optional_port_and_path(input: &str) -> bool {
    let host_port = input.split('/').next().unwrap_or(input);
    let (host, port) = if let Some((host, port)) = host_port.rsplit_once(':') {
        (host, Some(port))
    } else {
        (host_port, None)
    };

    let octets: Vec<&str> = host.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    if octets
        .iter()
        .any(|octet| octet.is_empty() || octet.parse::<u8>().is_err())
    {
        return false;
    }

    if let Some(port) = port {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if port.parse::<u16>().is_err() {
            return false;
        }
    }

    true
}

fn looks_like_domain(input: &str) -> bool {
    let host_port = input.split('/').next().unwrap_or(input);
    let (host, port) = if let Some((host, port)) = host_port.rsplit_once(':') {
        (host, Some(port))
    } else {
        (host_port, None)
    };

    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }

    if !host.contains('.') {
        return false;
    }

    for label in host.split('.') {
        if label.is_empty() {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return false;
        }
    }

    if let Some(port) = port {
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
        if port.parse::<u16>().is_err() {
            return false;
        }
    }

    true
}

fn looks_like_file_path(input: &str) -> bool {
    // Strip optional line:col suffix (e.g., "file.rs:42" or "file.rs:42:10")
    let path = strip_line_col_suffix(input);

    if path.is_empty() {
        return false;
    }

    // Absolute Unix paths
    if path.starts_with('/') {
        return has_path_like_structure(path);
    }

    // Home directory paths
    if strip_home_prefix(path).is_some() {
        return has_path_like_structure(path);
    }

    // Relative paths starting with ./ or ../
    if is_relative_link_path(path) {
        return has_path_like_structure(path);
    }

    // UNC paths (\\server\share\file)
    if cfg!(windows) && is_unc_path(path) {
        return true;
    }

    // Windows absolute paths (C:\, D:\, etc.)
    if path.len() >= 3 {
        let bytes = path.as_bytes();
        if has_windows_drive_prefix(path) && (bytes[2] == b'\\' || bytes[2] == b'/') {
            return has_path_like_structure(path);
        }
    }

    false
}

/// `\\server\share` with a non-empty server and share, not a `\\?\` or `\\.\`
/// device path.
fn is_unc_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("\\\\") else {
        return false;
    };
    let mut parts = rest.split('\\');
    let server = parts.next().unwrap_or_default();
    let share = parts.next().unwrap_or_default();
    !server.is_empty() && server != "?" && server != "." && !share.is_empty()
}

/// Where an MSVC-style `(line)` or `(line,col)` location suffix starts, as in
/// `file.cpp(12,3)`.
fn msvc_location_start(input: &str) -> Option<usize> {
    let inner = input.strip_suffix(')')?;
    let open = inner.rfind('(')?;
    let location = &inner[open + 1..];
    let (line, col) = location.split_once(',').unwrap_or((location, "0"));
    let is_number = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
    (open > 0 && is_number(line) && is_number(col)).then_some(open)
}

fn strip_line_col_suffix(input: &str) -> &str {
    if cfg!(windows)
        && let Some(open) = msvc_location_start(input)
    {
        return &input[..open];
    }

    // Handle patterns like "file.rs:42" or "file.rs:42:10"
    let mut path = input;

    // Try to strip :col suffix first
    if let Some(colon_pos) = path.rfind(':') {
        let suffix = &path[colon_pos + 1..];
        if !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()) {
            path = &path[..colon_pos];
            // Try to strip :line suffix
            if let Some(colon_pos2) = path.rfind(':') {
                let suffix2 = &path[colon_pos2 + 1..];
                if !suffix2.is_empty() && suffix2.chars().all(|c| c.is_ascii_digit()) {
                    path = &path[..colon_pos2];
                }
            }
        }
    }

    path
}

fn has_path_like_structure(path: &str) -> bool {
    // Must contain at least one path separator or have a file extension
    let has_separator = path.contains('/') || path.contains('\\');
    let has_extension = path.rfind('.').is_some_and(|dot_pos| {
        let after_dot = &path[dot_pos + 1..];
        !after_dot.is_empty()
            && after_dot.len() <= 10
            && after_dot.chars().all(|c| c.is_ascii_alphanumeric())
    });

    has_separator || has_extension
}

#[cfg(test)]
mod tests {
    use super::{
        FAILED_FILE_URL_TTL, FileUrlLruCache, MAX_LINK_SCAN_CELLS, classify_link_token,
        classify_link_token_in, hyperlink_at_viewport_cell, link_at_viewport_cell,
    };
    use crate::terminal_engine::{Engine, Options, Size};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    fn term_with_input(cols: u16, rows: u16, input: &[u8]) -> Engine {
        let mut term = Engine::new(
            Size {
                cols: usize::from(cols),
                rows: usize::from(rows),
            },
            Options::default(),
        );
        term.feed(input);
        term
    }

    #[test]
    fn osc8_hyperlink_detected_under_each_link_cell() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\ after",
        );

        for col in 0..4 {
            let link = hyperlink_at_viewport_cell(&term, 0, col)
                .expect("hyperlinked cell should report the OSC 8 target");
            assert_eq!(link.start_col, 0);
            assert_eq!(link.end_col, 3);
            assert_eq!(link.target, "https://example.com");
        }
    }

    #[test]
    fn osc8_hyperlink_absent_outside_link_run() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://example.com\x1b\\docs\x1b]8;;\x1b\\ after",
        );

        assert_eq!(hyperlink_at_viewport_cell(&term, 0, 4), None);
        assert_eq!(hyperlink_at_viewport_cell(&term, 0, 6), None);
        assert_eq!(hyperlink_at_viewport_cell(&term, 1, 0), None);
    }

    #[test]
    fn osc8_adjacent_distinct_links_do_not_merge() {
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;;https://a.example\x1b\\aa\x1b]8;;https://b.example\x1b\\bb\x1b]8;;\x1b\\",
        );

        let first = hyperlink_at_viewport_cell(&term, 0, 1).expect("first link");
        assert_eq!((first.start_col, first.end_col), (0, 1));
        assert_eq!(first.target, "https://a.example");

        let second = hyperlink_at_viewport_cell(&term, 0, 2).expect("second link");
        assert_eq!((second.start_col, second.end_col), (2, 3));
        assert_eq!(second.target, "https://b.example");
    }

    #[test]
    fn osc8_same_id_split_runs_only_cover_contiguous_cells() {
        // Same link id interrupted by a plain cell: hover should only span the
        // contiguous run under the cursor, not jump across the gap.
        let term = term_with_input(
            20,
            2,
            b"\x1b]8;id=x;https://example.com\x1b\\ab\x1b]8;;\x1b\\-\x1b]8;id=x;https://example.com\x1b\\cd\x1b]8;;\x1b\\",
        );

        let left = hyperlink_at_viewport_cell(&term, 0, 0).expect("left run");
        assert_eq!((left.start_col, left.end_col), (0, 1));
        let right = hyperlink_at_viewport_cell(&term, 0, 3).expect("right run");
        assert_eq!((right.start_col, right.end_col), (3, 4));
    }

    #[test]
    fn plain_text_has_no_hyperlink() {
        let term = term_with_input(20, 2, b"https://example.com");
        assert_eq!(hyperlink_at_viewport_cell(&term, 0, 0), None);
    }

    #[test]
    fn detected_url_spans_soft_wrapped_rows() {
        let term = term_with_input(10, 4, b"go https://example.com/path");

        for (row, col) in [(0, 4), (1, 5), (2, 4)] {
            let link = link_at_viewport_cell(&term, row, col)
                .expect("each wrapped URL segment should resolve");
            assert_eq!((link.start_row, link.start_col), (0, 3));
            assert_eq!((link.end_row, link.end_col), (2, 6));
            assert_eq!(link.target, "https://example.com/path");
        }
    }

    #[test]
    fn detected_url_does_not_cross_hard_line_break() {
        let term = term_with_input(20, 3, b"https://a.co\r\nsecond.example");

        let second =
            link_at_viewport_cell(&term, 1, 3).expect("second line should be its own domain");
        assert_eq!((second.start_row, second.start_col), (1, 0));
        assert_eq!((second.end_row, second.end_col), (1, 13));
        assert_eq!(second.target, "http://second.example");
    }

    #[test]
    fn osc8_hyperlink_span_follows_soft_wrap() {
        let term = term_with_input(
            5,
            3,
            b"\x1b]8;;https://example.com\x1b\\read-more\x1b]8;;\x1b\\",
        );

        let link = link_at_viewport_cell(&term, 1, 2)
            .expect("wrapped OSC 8 text should resolve as one link");
        assert_eq!((link.start_row, link.start_col), (0, 0));
        assert_eq!((link.end_row, link.end_col), (1, 3));
        assert_eq!(link.target, "https://example.com");
    }

    fn unique_temp_path(file_name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("termy-links-{nonce}-{file_name}"))
    }

    #[test]
    fn absolute_file_paths_emit_well_formed_file_urls() {
        let file_path = unique_temp_path("sample.txt");
        fs::write(&file_path, "sample").expect("write temp file");

        let token = file_path.to_string_lossy();
        let link = classify_link_token(&token).expect("file path should produce a file URL");

        assert!(link.starts_with("file:///"));
        assert!(!link.contains('\\'));

        #[cfg(unix)]
        {
            let canonical = fs::canonicalize(&file_path).expect("canonicalize temp file");
            let canonical = canonical.to_string_lossy();
            assert_eq!(
                link,
                format!("file:///{}", canonical.trim_start_matches('/'))
            );
        }

        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn file_path_line_col_suffix_is_ignored_for_url_generation() {
        let file_path = unique_temp_path("with-line-col.rs");
        fs::write(&file_path, "fn main() {}").expect("write temp file");

        let token = file_path.to_string_lossy();
        let expected = classify_link_token(&token).expect("base file path should classify");
        let with_suffix = format!("{token}:42:10");

        assert_eq!(classify_link_token(&with_suffix), Some(expected));

        let _ = fs::remove_file(file_path);
    }

    #[test]
    fn malformed_file_urls_are_rejected() {
        assert_eq!(classify_link_token("file://relative/path.txt"), None);
    }

    #[test]
    fn non_canonicalizable_file_paths_are_rejected() {
        let missing_path = unique_temp_path("missing-file.txt");
        let token = missing_path.to_string_lossy();

        assert_eq!(classify_link_token(&token), None);
    }

    #[test]
    fn file_url_lru_cache_evicts_old_entries() {
        let now = Instant::now();
        let mut cache = FileUrlLruCache::new(2);
        cache.insert("a".to_string(), Some("file:///a".to_string()), now);
        cache.insert("b".to_string(), Some("file:///b".to_string()), now);
        assert_eq!(cache.get("a", now), Some(Some("file:///a".to_string())));
        cache.insert("c".to_string(), Some("file:///c".to_string()), now);

        assert_eq!(cache.get("a", now), Some(Some("file:///a".to_string())));
        assert_eq!(cache.get("b", now), None);
        assert_eq!(cache.get("c", now), Some(Some("file:///c".to_string())));
    }

    #[test]
    fn failed_file_url_lookups_expire() {
        let now = Instant::now();
        let mut cache = FileUrlLruCache::new(4);
        cache.insert("found".to_string(), Some("file:///found".to_string()), now);
        cache.insert("missing".to_string(), None, now);
        assert_eq!(cache.get("missing", now), Some(None));

        let later = now + FAILED_FILE_URL_TTL;
        assert_eq!(cache.get("missing", later), None);
        assert_eq!(
            cache.get("found", later),
            Some(Some("file:///found".to_string()))
        );
    }

    #[cfg(unix)]
    #[test]
    fn relative_paths_resolve_against_the_pane_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("notes.txt");
        fs::write(&file_path, "notes").expect("write temp file");
        let canonical = fs::canonicalize(&file_path).expect("canonicalize temp file");
        let expected = format!(
            "file:///{}",
            canonical.to_string_lossy().trim_start_matches('/')
        );

        assert_eq!(
            classify_link_token_in("./notes.txt", Some(dir.path())),
            Some(expected.clone())
        );

        // The engine's OSC 7 directory is the base for hovered links.
        let mut input = format!("\x1b]7;file://localhost{}\x1b\\", dir.path().display());
        input.push_str("see ./notes.txt");
        let term = term_with_input(40, 2, input.as_bytes());
        let link = link_at_viewport_cell(&term, 0, 6).expect("relative path link");
        assert_eq!(link.target, expected);
        assert_eq!((link.start_col, link.end_col), (4, 14));
    }

    #[cfg(windows)]
    #[test]
    fn windows_relative_paths_resolve_against_the_pane_working_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("notes.txt");
        fs::write(&file_path, "notes").expect("write temp file");
        let canonical = dunce::canonicalize(&file_path).expect("canonicalize temp file");
        let expected =
            classify_link_token(&canonical.to_string_lossy()).expect("absolute path link");
        assert!(expected.starts_with("file:///"), "{expected}");
        assert!(!expected.contains('?'), "{expected}");

        assert_eq!(
            classify_link_token_in(r".\notes.txt", Some(dir.path())),
            Some(expected.clone())
        );
        assert_eq!(
            classify_link_token_in("./notes.txt", Some(dir.path())),
            Some(expected.clone())
        );

        // A Windows OSC 7 URL (`file://host/C:/dir`) is the base for hovered links.
        let url_path = dir.path().to_string_lossy().replace('\\', "/");
        let mut input = format!("\x1b]7;file://localhost/{url_path}\x1b\\");
        input.push_str(r"see .\notes.txt");
        let term = term_with_input(40, 2, input.as_bytes());
        let link = link_at_viewport_cell(&term, 0, 6).expect("relative path link");
        assert_eq!(link.target, expected);
        assert_eq!((link.start_col, link.end_col), (4, 14));
    }

    #[cfg(windows)]
    #[test]
    fn msvc_locations_link_to_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("main.cpp");
        fs::write(&file_path, "int main() {}").expect("write temp file");
        let token = file_path.to_string_lossy();
        let expected = classify_link_token(&token).expect("base file path should classify");

        for suffix in ["(12)", "(12,3)", ":12", ":12:3"] {
            assert_eq!(
                classify_link_token(&format!("{token}{suffix}")),
                Some(expected.clone()),
                "{suffix}"
            );
        }

        let line: Vec<char> = r"C:\src\main.cpp(12,3): error C2065".chars().collect();
        assert_eq!(
            super::link_token_in_line(&line, 3).map(|(_, _, token)| token),
            Some(r"C:\src\main.cpp(12,3)".to_string())
        );
        let line: Vec<char> = r"at C:\src\main.cpp(12)".chars().collect();
        assert_eq!(
            super::link_token_in_line(&line, 5),
            Some((3, 21, r"C:\src\main.cpp(12)".to_string()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_forms_are_recognised() {
        assert!(super::looks_like_file_path(r"\\server\share\file.txt"));
        assert!(super::looks_like_file_path(r".\src\main.rs"));
        assert!(super::looks_like_file_path(r"..\lib.rs"));
        assert!(super::looks_like_file_path(r"~\notes.txt"));
        assert!(!super::looks_like_file_path(r"\\?\"));
        assert!(!super::looks_like_file_path(r"\\server"));

        let line: Vec<char> = r"(.\src\main.rs)".chars().collect();
        assert_eq!(
            super::link_token_in_line(&line, 3),
            Some((1, 13, r".\src\main.rs".to_string()))
        );
    }

    #[test]
    fn msvc_location_suffixes_are_parsed() {
        assert_eq!(super::msvc_location_start("main.cpp(12)"), Some(8));
        assert_eq!(super::msvc_location_start("main.cpp(12,3)"), Some(8));
        assert_eq!(super::msvc_location_start("main.cpp(x)"), None);
        assert_eq!(super::msvc_location_start("main.cpp(12,)"), None);
        assert_eq!(super::msvc_location_start("(12)"), None);
        assert_eq!(super::msvc_location_start("main.cpp"), None);
    }

    #[test]
    fn leading_dots_of_relative_paths_are_kept() {
        let line: Vec<char> = "(./src/main.rs) ../lib.rs ...end".chars().collect();
        assert_eq!(
            super::link_token_in_line(&line, 3),
            Some((1, 13, "./src/main.rs".to_string()))
        );
        assert_eq!(
            super::link_token_in_line(&line, 18),
            Some((16, 24, "../lib.rs".to_string()))
        );
        assert_eq!(
            super::link_token_in_line(&line, 29),
            Some((29, 31, "end".to_string()))
        );
    }

    #[test]
    fn hover_does_not_scan_unbounded_runs_without_whitespace() {
        let cols = 80;
        let url = "https://example.com/";
        // Longer than the scan window on both sides of any hovered cell.
        let long = format!("{url}{}", "a".repeat(2 * MAX_LINK_SCAN_CELLS + 10));
        let rows = long.len().div_ceil(cols) + 1;
        let term = term_with_input(cols as u16, rows as u16, long.as_bytes());
        for row in [0, rows / 2, rows - 2] {
            assert!(link_at_viewport_cell(&term, row, 0).is_none(), "{row}");
        }

        // A link that fits in the window is still found across wrapped rows.
        let fits = format!("{url}{}", "a".repeat(200));
        let term = term_with_input(cols as u16, 4, fits.as_bytes());
        let link = link_at_viewport_cell(&term, 2, 0).expect("wrapped link");
        assert_eq!(link.target, fits);
    }
}
