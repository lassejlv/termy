#[derive(Clone, Debug, PartialEq, Eq)]
struct SemanticCell {
    character: char,
    combining: String,
    foreground: RawColor,
    background: RawColor,
    bold: bool,
    dim: bool,
    italic: bool,
    underline_style: RawUnderlineStyle,
    underline_color: Option<RawColor>,
    inverse: bool,
    hidden: bool,
    strikethrough: bool,
    hyperlink: bool,
    wide_spacer: bool,
    wrapped: bool,
}
const NORMALIZED_BOLD: u16 = 1 << 0;
const NORMALIZED_DIM: u16 = 1 << 1;
const NORMALIZED_ITALIC: u16 = 1 << 2;
const NORMALIZED_SINGLE_UNDERLINE: u16 = 1 << 3;
const NORMALIZED_INVERSE: u16 = 1 << 4;
const NORMALIZED_HIDDEN: u16 = 1 << 5;
const NORMALIZED_STRIKETHROUGH: u16 = 1 << 6;
const NORMALIZED_HYPERLINK: u16 = 1 << 7;
const NORMALIZED_WIDE_SPACER: u16 = 1 << 8;
const NORMALIZED_LEADING_WIDE_SPACER: u16 = 1 << 9;
const NORMALIZED_WRAPPED: u16 = 1 << 10;
const NORMALIZED_DOUBLE_UNDERLINE: u16 = 1 << 11;
const NORMALIZED_CURLY_UNDERLINE: u16 = 1 << 12;
const NORMALIZED_DOTTED_UNDERLINE: u16 = 1 << 13;
const NORMALIZED_DASHED_UNDERLINE: u16 = 1 << 14;
#[cfg(test)]
const NORMALIZED_ALL_UNDERLINES: u16 = NORMALIZED_SINGLE_UNDERLINE
    | NORMALIZED_DOUBLE_UNDERLINE
    | NORMALIZED_CURLY_UNDERLINE
    | NORMALIZED_DOTTED_UNDERLINE
    | NORMALIZED_DASHED_UNDERLINE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NormalizedCell {
    character: char,
    foreground: RawColor,
    background: RawColor,
    underline_color: Option<RawColor>,
    combining_start: u32,
    combining_len: u32,
    flags: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NormalizedCursor {
    col: usize,
    row: usize,
    line_style: bool,
}

#[derive(Debug, PartialEq, Eq)]
struct NormalizedFrame {
    cols: u16,
    rows: u16,
    cells: Vec<NormalizedCell>,
    combining: String,
    cursor: Option<NormalizedCursor>,
    display_offset: usize,
    history_size: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RendererCacheCell {
    character: char,
    combining: String,
    foreground: RawColor,
    background: RawColor,
    underline_color: Option<RawColor>,
    flags: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RendererCacheFrame {
    cols: u16,
    rows: u16,
    cell_rows: Arc<Vec<Arc<Vec<RendererCacheCell>>>>,
    cursor: Option<NormalizedCursor>,
    display_offset: usize,
    history_size: usize,
}

struct RendererCacheFixture {
    terminal: TmonTerminal,
    before: RendererCacheFrame,
    update: RenderDamageSnapshot,
    expected: RendererCacheFrame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RendererCachePath {
    FullRebuild,
    ScrollReplay,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RendererCacheSample {
    first: RendererCachePath,
    full_rebuild: f64,
    scroll_replay: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct RendererCacheStats {
    full_rebuild: Stats,
    scroll_replay: Stats,
    ratio: Stats,
}

fn setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    let Ok(raw) = env::var(name) else {
        return default;
    };
    let value = raw
        .parse::<usize>()
        .unwrap_or_else(|_| panic!("{name} must be an integer, got {raw:?}"));
    assert!(
        (min..=max).contains(&value),
        "{name} must be between {min} and {max}, got {value}"
    );
    value
}

fn require_even_sample_count(sample_count: usize) -> usize {
    assert!(
        sample_count.is_multiple_of(2),
        "TMON_BENCH_SAMPLES must be even so every workload has balanced Tmon-first and \
         Alacritty-first pairs, got {sample_count}"
    );
    sample_count
}

fn sample_order(index: usize, tmon_first: bool) -> [Engine; 2] {
    if index.is_multiple_of(2) == tmon_first {
        [Engine::Tmon, Engine::Alacritty]
    } else {
        [Engine::Alacritty, Engine::Tmon]
    }
}

fn validate_target_ratio(target: f64) -> Result<(), &'static str> {
    if !target.is_finite() {
        return Err("target ratio must be finite");
    }
    if target <= 0.0 {
        return Err("target ratio must be greater than zero");
    }
    Ok(())
}

fn meets_target(actual: f64, target: f64) -> bool {
    validate_target_ratio(target).unwrap_or_else(|reason| panic!("{reason}, got {target:?}"));
    actual.is_finite() && actual >= target
}

fn tmon_terminal() -> TmonTerminal {
    TmonTerminal::new_display(
        TmonSize {
            cols: COLS,
            rows: ROWS,
            ..TmonSize::default()
        },
        TmonConfig {
            scrollback_history: SCROLLBACK,
            ..TmonConfig::default()
        },
    )
}

fn alacritty_terminal() -> AlacrittyTerminal {
    let config = TerminalRuntimeConfig {
        scrollback_history: SCROLLBACK,
        ..TerminalRuntimeConfig::default()
    };
    let terminal = AlacrittyTerminal::new_display(
        AlacrittySize {
            cols: COLS,
            rows: ROWS,
            ..AlacrittySize::default()
        },
        Some(&config),
    );
    assert_eq!(
        terminal.engine_label(),
        "alacritty",
        "engine comparison requires TERMY_CORE_TEST_BACKEND=alacritty"
    );
    terminal
}

#[cfg(feature = "benchmark-allocations")]
fn feed_tmon_target(terminal: &TmonTerminal, payload: &[u8], target_bytes: usize) {
    for _ in 0..iterations_for(target_bytes, payload) {
        terminal.feed_output(black_box(payload));
    }
}

#[cfg(feature = "benchmark-allocations")]
fn feed_alacritty_target(terminal: &AlacrittyTerminal, payload: &[u8], target_bytes: usize) {
    for _ in 0..iterations_for(target_bytes, payload) {
        terminal.feed_output(black_box(payload));
    }
}

#[cfg(feature = "benchmark-allocations")]
fn warm_up_allocation_path(engine: Engine, workload: &Workload) {
    let payload = workload.payload.as_bytes();
    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            feed_tmon_target(&terminal, payload, ALLOCATION_WARMUP_BYTES);
            black_box(&terminal);
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            feed_alacritty_target(&terminal, payload, ALLOCATION_WARMUP_BYTES);
            black_box(&terminal);
        }
    }
}

#[cfg(feature = "benchmark-allocations")]
fn allocation_sample(
    engine: Engine,
    workload: &Workload,
    target_bytes: usize,
) -> allocation_counter::AllocationSnapshot {
    warm_up_allocation_path(engine, workload);
    let payload = workload.payload.as_bytes();

    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            let measurement =
                allocation_counter::AllocationMeasurement::begin().unwrap_or_else(|reason| {
                    panic!("could not start allocation measurement: {reason}")
                });
            feed_tmon_target(&terminal, payload, target_bytes);
            let allocations = measurement.finish();
            black_box(terminal.snapshot());
            allocations
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            let measurement =
                allocation_counter::AllocationMeasurement::begin().unwrap_or_else(|reason| {
                    panic!("could not start allocation measurement: {reason}")
                });
            feed_alacritty_target(&terminal, payload, target_bytes);
            let allocations = measurement.finish();
            black_box(terminal.snapshot());
            allocations
        }
    }
}

fn alacritty_color(color: TerminalRenderColor) -> RawColor {
    match color {
        TerminalRenderColor::Indexed(index) | TerminalRenderColor::DimIndexed(index) => {
            RawColor::Indexed(index)
        }
        TerminalRenderColor::Rgb(color) => RawColor::Rgb(color.r, color.g, color.b),
        TerminalRenderColor::DefaultForeground
        | TerminalRenderColor::DefaultBackground
        | TerminalRenderColor::Cursor
        | TerminalRenderColor::BrightForeground
        | TerminalRenderColor::DimForeground => RawColor::Default,
    }
}

fn tmon_color(color: termy_core::tmon::Color) -> RawColor {
    match color {
        termy_core::tmon::Color::Default => RawColor::Default,
        termy_core::tmon::Color::Indexed(index) => RawColor::Indexed(index),
        termy_core::tmon::Color::Rgb { r, g, b } => RawColor::Rgb(r, g, b),
    }
}

fn alacritty_underline_style(style: TerminalUnderlineStyle) -> RawUnderlineStyle {
    match style {
        TerminalUnderlineStyle::None => RawUnderlineStyle::None,
        TerminalUnderlineStyle::Single => RawUnderlineStyle::Single,
        TerminalUnderlineStyle::Double => RawUnderlineStyle::Double,
        TerminalUnderlineStyle::Curly => RawUnderlineStyle::Curly,
        TerminalUnderlineStyle::Dotted => RawUnderlineStyle::Dotted,
        TerminalUnderlineStyle::Dashed => RawUnderlineStyle::Dashed,
    }
}

const fn tmon_underline_style(style: TmonUnderlineStyle) -> RawUnderlineStyle {
    match style {
        TmonUnderlineStyle::None => RawUnderlineStyle::None,
        TmonUnderlineStyle::Single => RawUnderlineStyle::Single,
        TmonUnderlineStyle::Double => RawUnderlineStyle::Double,
        TmonUnderlineStyle::Curly => RawUnderlineStyle::Curly,
        TmonUnderlineStyle::Dotted => RawUnderlineStyle::Dotted,
        TmonUnderlineStyle::Dashed => RawUnderlineStyle::Dashed,
    }
}

fn alacritty_cell(cell: &TerminalRenderCell) -> SemanticCell {
    let mut chars = cell.text.chars();
    SemanticCell {
        character: chars.next().unwrap_or('\0'),
        combining: chars.collect(),
        foreground: alacritty_color(cell.foreground),
        background: alacritty_color(cell.background),
        bold: cell.bold,
        dim: cell.dim,
        italic: cell.italic,
        underline_style: alacritty_underline_style(cell.underline_style),
        underline_color: cell.underline_color.map(alacritty_color),
        inverse: cell.inverse,
        hidden: cell.hidden,
        strikethrough: cell.strikethrough,
        hyperlink: cell.hyperlink,
        wide_spacer: cell.wide_character_spacer || cell.leading_wide_character_spacer,
        wrapped: cell.line_wrapped,
    }
}

fn tmon_cell(cell: &termy_core::tmon::Cell, combining: Option<TmonCombining<'_>>) -> SemanticCell {
    SemanticCell {
        character: cell.character,
        combining: combining.map_or_else(String::new, TmonCombining::to_owned_string),
        foreground: tmon_color(cell.foreground),
        background: tmon_color(cell.background),
        bold: cell.attributes.bold(),
        dim: cell.attributes.dim(),
        italic: cell.attributes.italic(),
        underline_style: tmon_underline_style(cell.attributes.underline_style()),
        underline_color: cell.underline_color.map(tmon_color),
        inverse: cell.attributes.inverse(),
        hidden: cell.attributes.hidden(),
        strikethrough: cell.attributes.strikethrough(),
        hyperlink: cell.has_hyperlink(),
        wide_spacer: cell.wide_spacer() || cell.leading_wide_spacer(),
        wrapped: cell.wrapped(),
    }
}

const fn normalized_flag(enabled: bool, flag: u16) -> u16 {
    if enabled { flag } else { 0 }
}

const fn normalized_underline_flag(style: RawUnderlineStyle) -> u16 {
    match style {
        RawUnderlineStyle::None => 0,
        RawUnderlineStyle::Single => NORMALIZED_SINGLE_UNDERLINE,
        RawUnderlineStyle::Double => NORMALIZED_DOUBLE_UNDERLINE,
        RawUnderlineStyle::Curly => NORMALIZED_CURLY_UNDERLINE,
        RawUnderlineStyle::Dotted => NORMALIZED_DOTTED_UNDERLINE,
        RawUnderlineStyle::Dashed => NORMALIZED_DASHED_UNDERLINE,
    }
}

fn normalized_combining_range(start: usize, end: usize) -> (u32, u32) {
    let start = u32::try_from(start).expect("normalized combining arena exceeds 4 GiB");
    let len = u32::try_from(end.saturating_sub(start as usize))
        .expect("normalized combining text exceeds 4 GiB");
    (start, len)
}

fn normalized_alacritty_cell(cell: &TerminalRenderCell, combining: &mut String) -> NormalizedCell {
    let mut chars = cell.text.chars();
    let character = chars.next().unwrap_or('\0');
    let combining_start = combining.len();
    combining.extend(chars);
    let (combining_start, combining_len) =
        normalized_combining_range(combining_start, combining.len());
    let flags = normalized_flag(cell.bold, NORMALIZED_BOLD)
        | normalized_flag(cell.dim, NORMALIZED_DIM)
        | normalized_flag(cell.italic, NORMALIZED_ITALIC)
        | normalized_underline_flag(alacritty_underline_style(cell.underline_style))
        | normalized_flag(cell.inverse, NORMALIZED_INVERSE)
        | normalized_flag(cell.hidden, NORMALIZED_HIDDEN)
        | normalized_flag(cell.strikethrough, NORMALIZED_STRIKETHROUGH)
        | normalized_flag(cell.hyperlink, NORMALIZED_HYPERLINK)
        | normalized_flag(cell.wide_character_spacer, NORMALIZED_WIDE_SPACER)
        | normalized_flag(
            cell.leading_wide_character_spacer,
            NORMALIZED_LEADING_WIDE_SPACER,
        )
        | normalized_flag(cell.line_wrapped, NORMALIZED_WRAPPED);

    NormalizedCell {
        character,
        foreground: alacritty_color(cell.foreground),
        background: alacritty_color(cell.background),
        underline_color: cell.underline_color.map(alacritty_color),
        combining_start,
        combining_len,
        flags,
    }
}

fn normalized_tmon_cell(
    cell: &termy_core::tmon::Cell,
    cell_combining: Option<TmonCombining<'_>>,
    combining: &mut String,
) -> NormalizedCell {
    let combining_start = combining.len();
    if let Some(cell_combining) = cell_combining {
        cell_combining.append_to(combining);
    }
    let (combining_start, combining_len) =
        normalized_combining_range(combining_start, combining.len());
    let flags = normalized_flag(cell.attributes.bold(), NORMALIZED_BOLD)
        | normalized_flag(cell.attributes.dim(), NORMALIZED_DIM)
        | normalized_flag(cell.attributes.italic(), NORMALIZED_ITALIC)
        | normalized_underline_flag(tmon_underline_style(cell.attributes.underline_style()))
        | normalized_flag(cell.attributes.inverse(), NORMALIZED_INVERSE)
        | normalized_flag(cell.attributes.hidden(), NORMALIZED_HIDDEN)
        | normalized_flag(cell.attributes.strikethrough(), NORMALIZED_STRIKETHROUGH)
        | normalized_flag(cell.has_hyperlink(), NORMALIZED_HYPERLINK)
        | normalized_flag(cell.wide_spacer(), NORMALIZED_WIDE_SPACER)
        | normalized_flag(cell.leading_wide_spacer(), NORMALIZED_LEADING_WIDE_SPACER)
        | normalized_flag(cell.wrapped(), NORMALIZED_WRAPPED);

    NormalizedCell {
        character: cell.character,
        foreground: tmon_color(cell.foreground),
        background: tmon_color(cell.background),
        underline_color: cell.underline_color.map(tmon_color),
        combining_start,
        combining_len,
        flags,
    }
}

fn normalized_tmon_frame(terminal: &TmonTerminal) -> NormalizedFrame {
    let mut cells = Vec::with_capacity(usize::from(COLS) * usize::from(ROWS));
    let mut combining = String::new();
    let metadata = terminal.visit_viewport_cells(|_, _, _, cell, cell_combining| {
        cells.push(normalized_tmon_cell(cell, cell_combining, &mut combining));
    });
    debug_assert_eq!(
        cells.len(),
        usize::from(metadata.cols) * usize::from(metadata.rows)
    );
    NormalizedFrame {
        cols: metadata.cols,
        rows: metadata.rows,
        cells,
        combining,
        cursor: metadata.cursor.map(|cursor| NormalizedCursor {
            col: cursor.col,
            row: cursor.row,
            line_style: matches!(cursor.style, TmonCursorStyle::Line),
        }),
        display_offset: metadata.display_offset,
        history_size: metadata.history_size,
    }
}

fn normalized_tmon_cursor(cursor: Option<termy_core::tmon::CursorState>) -> Option<NormalizedCursor> {
    cursor.map(|cursor| NormalizedCursor {
        col: cursor.col,
        row: cursor.row,
        line_style: matches!(cursor.style, TmonCursorStyle::Line),
    })
}

fn renderer_cache_cell(
    cell: &termy_core::tmon::Cell,
    cell_combining: Option<TmonCombining<'_>>,
) -> RendererCacheCell {
    let mut combining = String::new();
    let normalized = normalized_tmon_cell(cell, cell_combining, &mut combining);
    debug_assert_eq!(normalized.combining_start, 0);
    debug_assert_eq!(normalized.combining_len as usize, combining.len());
    RendererCacheCell {
        character: normalized.character,
        combining,
        foreground: normalized.foreground,
        background: normalized.background,
        underline_color: normalized.underline_color,
        flags: normalized.flags,
    }
}

fn fresh_renderer_cache_frame(
    terminal: &TmonTerminal,
    expected_cols: u16,
    expected_rows: u16,
) -> RendererCacheFrame {
    let cols = usize::from(expected_cols);
    let rows = usize::from(expected_rows);
    let mut cell_rows = (0..rows)
        .map(|_| Arc::new(Vec::with_capacity(cols)))
        .collect::<Vec<_>>();
    let metadata = terminal.visit_viewport_cells(|offset, term_line, _, cell, cell_combining| {
        let row = usize::try_from(
            i64::from(term_line) + i64::try_from(offset).expect("viewport offset fits in i64"),
        )
        .expect("visible terminal line maps to a viewport row");
        let cache_row = cell_rows
            .get_mut(row)
            .expect("viewport visitor returned an out-of-range row");
        Arc::get_mut(cache_row)
            .expect("fresh renderer-cache rows are uniquely owned")
            .push(renderer_cache_cell(cell, cell_combining));
    });
    assert_eq!(
        (metadata.cols, metadata.rows),
        (expected_cols, expected_rows),
        "renderer-cache dimensions changed while rebuilding"
    );
    assert!(
        cell_rows.iter().all(|row| row.len() == cols),
        "renderer-cache rebuild did not visit every visible cell"
    );
    RendererCacheFrame {
        cols: metadata.cols,
        rows: metadata.rows,
        cell_rows: Arc::new(cell_rows),
        cursor: normalized_tmon_cursor(metadata.cursor),
        display_offset: metadata.display_offset,
        history_size: metadata.history_size,
    }
}

fn replay_renderer_scrolls<T>(rows: &mut [T], scrolls: &[ScrollDamage]) -> bool {
    if scrolls.iter().any(|scroll| {
        scroll.top > scroll.bottom
            || scroll.bottom >= rows.len()
            || scroll.count == 0
            || scroll.count > scroll.bottom - scroll.top + 1
    }) {
        return false;
    }

    for scroll in scrolls {
        let region = &mut rows[scroll.top..=scroll.bottom];
        match scroll.direction {
            ScrollDirection::Up => region.rotate_left(scroll.count),
            ScrollDirection::Down => region.rotate_right(scroll.count),
        }
    }
    true
}

fn apply_renderer_cache_update(
    cache: &mut RendererCacheFrame,
    terminal: &TmonTerminal,
    update: &RenderDamageSnapshot,
) -> bool {
    let DamageSnapshot::Partial(spans) = &update.damage else {
        return false;
    };
    let cell_rows = Arc::make_mut(&mut cache.cell_rows);
    if !replay_renderer_scrolls(cell_rows, &update.scrolls) {
        return false;
    }

    let expected_cells = spans.iter().try_fold(0usize, |total, span| {
        if span.row >= usize::from(cache.rows)
            || span.left_col > span.right_col
            || span.right_col >= usize::from(cache.cols)
        {
            return None;
        }
        total.checked_add(span.right_col - span.left_col + 1)
    });
    let Some(expected_cells) = expected_cells else {
        return false;
    };
    let ranges = spans
        .iter()
        .map(|span| (span.row, span.left_col, span.right_col));
    let mut patched_cells = 0usize;
    let Some(visited_offset) = terminal.for_each_viewport_range_at_generation(
        update.generation,
        ranges,
        |row, _, _, col, cell, cell_combining| {
            Arc::make_mut(&mut cell_rows[row])[col] = renderer_cache_cell(cell, cell_combining);
            patched_cells = patched_cells.saturating_add(1);
        },
    ) else {
        return false;
    };
    if patched_cells != expected_cells {
        return false;
    }

    let cursor = normalized_tmon_cursor(terminal.cursor_state());
    let (display_offset, history_size) = terminal.scroll_state();
    if display_offset != visited_offset
        || terminal.for_each_viewport_range_at_generation(
            update.generation,
            std::iter::empty(),
            |_, _, _, _, _, _| {},
        ) != Some(display_offset)
    {
        return false;
    }
    cache.cursor = cursor;
    cache.display_offset = display_offset;
    cache.history_size = history_size;
    true
}

fn prepare_renderer_cache_fixture() -> RendererCacheFixture {
    let terminal = tmon_terminal();
    prepare_normalized_tmon(&terminal, SNAPSHOT_PREFILL_BYTES);
    let before = fresh_renderer_cache_frame(&terminal, COLS, ROWS);
    assert!(matches!(
        terminal.take_render_damage_snapshot().damage,
        DamageSnapshot::Full
    ));

    terminal.feed_output(RENDERER_CACHE_UPDATE.as_bytes());
    let update = terminal.take_render_damage_snapshot();
    assert!(
        matches!(update.damage, DamageSnapshot::Partial(_)),
        "renderer-cache fixture unexpectedly requires a full rebuild"
    );
    assert_eq!(
        update
            .scrolls
            .iter()
            .map(|scroll| scroll.direction)
            .collect::<Vec<_>>(),
        [
            ScrollDirection::Up,
            ScrollDirection::Down,
            ScrollDirection::Up,
        ],
        "renderer-cache fixture did not preserve the intended scroll order"
    );
    let expected = fresh_renderer_cache_frame(&terminal, COLS, ROWS);
    RendererCacheFixture {
        terminal,
        before,
        update,
        expected,
    }
}

fn assert_renderer_cache_preflight(fixture: &RendererCacheFixture) {
    let mut actual = fixture.before.clone();
    assert!(
        apply_renderer_cache_update(&mut actual, &fixture.terminal, &fixture.update),
        "renderer-cache replay could not apply the captured generation"
    );
    assert_eq!(
        actual, fixture.expected,
        "scroll replay plus final-span patching differs from a fresh normalized rebuild"
    );
}

fn normalized_alacritty_frame(terminal: &AlacrittyTerminal) -> NormalizedFrame {
    let read = terminal.render_read(true);
    let mut cells = Vec::with_capacity(read.cells.len());
    let mut combining = String::new();
    for cell in &read.cells {
        cells.push(normalized_alacritty_cell(cell, &mut combining));
    }
    let cursor = read.metadata.cursor.map(|cursor| NormalizedCursor {
        col: cursor.col,
        row: cursor.row,
        line_style: matches!(cursor.style, TerminalCursorStyle::Line),
    });
    NormalizedFrame {
        cols: read.metadata.cols,
        rows: read.metadata.rows,
        cells,
        combining,
        cursor,
        display_offset: read.metadata.display_offset,
        history_size: read.metadata.history_size,
    }
}

fn normalized_snapshot_fixture() -> &'static str {
    static FIXTURE: OnceLock<String> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let mut fixture = String::from("\x1b[2J\x1b[H");
        for row in 1..=ROWS {
            fixture.push_str("\x1b[");
            fixture.push_str(&row.to_string());
            fixture.push_str(";1H");
            fixture.push_str(concat!(
                "\x1b[0mplain ",
                "\x1b[31mred ",
                "\x1b[38;5;200mindexed ",
                "\x1b[38;2;12;34;56mrgb ",
                "\x1b[48;2;65;43;21mbg\x1b[0m ",
                "\x1b[1mbold\x1b[0m ",
                "\x1b[2mdim\x1b[0m ",
                "\x1b[3mitalic\x1b[0m ",
                "\x1b[4munderline\x1b[0m ",
                "\x1b[7minverse\x1b[0m ",
                "\x1b[8mhidden\x1b[0m ",
                "\x1b[9mstrike\x1b[0m cafe\u{301} 界",
            ));
        }
        fixture.push_str(concat!(
            "\x1b[35;1H",
            "\x1b[58;2;12;34;56mR",
            "\x1b[58;5;200mI",
            "\x1b[58:2::65:43:21mC",
            "\x1b[58:5:123mX",
            "\x1b[59mN",
            "\x1b[58;2;1;2;3mZ",
            "\x1b[0mD",
        ));
        fixture.push_str(concat!(
            "\x1b[36;1H",
            "\x1b[4mS",
            "\x1b[4:2mD",
            "\x1b[4:3mC",
            "\x1b[4:4mO",
            "\x1b[4:5mH",
            "\x1b[4:0mN",
        ));
        fixture
            .push_str("\x1b[37;1H\x1b]8;;https://example.invalid/tmon\x1b\\linked\x1b]8;;\x1b\\");
        fixture.push_str("\x1b[38;120H界");
        fixture.push_str("\x1b[0m\x1b[20;60H");
        fixture
    })
}

fn prepare_normalized_tmon(terminal: &TmonTerminal, prefill_bytes: usize) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(prefill_bytes, payload) {
        terminal.feed_output(payload);
    }
    terminal.feed_output(normalized_snapshot_fixture().as_bytes());
}

fn prepare_normalized_alacritty(terminal: &AlacrittyTerminal, prefill_bytes: usize) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(prefill_bytes, payload) {
        terminal.feed_output(payload);
    }
    terminal.feed_output(normalized_snapshot_fixture().as_bytes());
}

fn normalized_combining_text<'a>(frame: &'a NormalizedFrame, cell: &NormalizedCell) -> &'a str {
    let start = cell.combining_start as usize;
    let end = start.saturating_add(cell.combining_len as usize);
    frame
        .combining
        .get(start..end)
        .expect("normalized combining ranges are valid UTF-8 boundaries")
}

fn assert_normalized_frames_match(
    tmon: &NormalizedFrame,
    alacritty: &NormalizedFrame,
    context: &str,
) {
    assert_eq!(
        (
            tmon.cols,
            tmon.rows,
            tmon.cursor,
            tmon.display_offset,
            tmon.history_size,
        ),
        (
            alacritty.cols,
            alacritty.rows,
            alacritty.cursor,
            alacritty.display_offset,
            alacritty.history_size,
        ),
        "{context}: normalized frame metadata differs"
    );
    assert_eq!(
        tmon.cells.len(),
        alacritty.cells.len(),
        "{context}: normalized frame cell counts differ"
    );
    if let Some((index, (tmon_cell, alacritty_cell))) = tmon
        .cells
        .iter()
        .zip(&alacritty.cells)
        .enumerate()
        .find(|(_, (tmon_cell, alacritty_cell))| tmon_cell != alacritty_cell)
    {
        panic!(
            "{context}: normalized cell {index} differs:\n  Tmon: {tmon_cell:?} combining={:?}\n  \
             Alacritty: {alacritty_cell:?} combining={:?}",
            normalized_combining_text(tmon, tmon_cell),
            normalized_combining_text(alacritty, alacritty_cell),
        );
    }
    assert_eq!(
        tmon.combining, alacritty.combining,
        "{context}: normalized combining arena differs"
    );
}

fn assert_normalized_fixture_equivalence(prefill_bytes: usize) {
    let tmon = tmon_terminal();
    let alacritty = alacritty_terminal();
    prepare_normalized_tmon(&tmon, prefill_bytes);
    prepare_normalized_alacritty(&alacritty, prefill_bytes);
    assert_normalized_frames_match(
        &normalized_tmon_frame(&tmon),
        &normalized_alacritty_frame(&alacritty),
        "mixed renderer-neutral fixture",
    );
}

fn assert_workload_equivalence(workload: &Workload, validation_bytes: usize) {
    let tmon = tmon_terminal();
    let alacritty = alacritty_terminal();
    let payload = workload.payload.as_bytes();
    for _ in 0..iterations_for(validation_bytes, payload) {
        tmon.feed_output(payload);
        alacritty.feed_output(payload);
    }

    let (tmon_first, tmon_last) = tmon.line_bounds();
    let (alacritty_first, alacritty_last) = alacritty.line_bounds();
    assert_eq!(
        (tmon_first, tmon_last),
        (alacritty_first, alacritty_last),
        "{}: full-grid bounds differ",
        workload.name
    );

    for line in alacritty_first..=alacritty_last {
        let mut alacritty_line = Vec::new();
        alacritty.visit_line_cells(line, line, |_, _, _, cell| {
            alacritty_line.push(alacritty_cell(cell));
        });
        let mut tmon_line = Vec::with_capacity(alacritty_line.len());
        assert!(
            tmon.for_each_line_cell(line, |_, cell, combining| {
                tmon_line.push(tmon_cell(cell, combining));
            }),
            "{}: Tmon is missing grid line {line}",
            workload.name
        );
        assert_eq!(
            tmon_line.len(),
            alacritty_line.len(),
            "{}: grid line {line} has a different width",
            workload.name
        );
        if let Some((col, (tmon_cell, alacritty_cell))) = tmon_line
            .iter()
            .zip(&alacritty_line)
            .enumerate()
            .find(|(_, (tmon_cell, alacritty_cell))| tmon_cell != alacritty_cell)
        {
            panic!(
                "{}: terminal state differs at line {line}, column {col}:\n\
                 Tmon: {tmon_cell:?}\nAlacritty: {alacritty_cell:?}",
                workload.name
            );
        }
    }

    let tmon_cursor = tmon.cursor_state().map(|cursor| {
        (
            cursor.col,
            cursor.row,
            matches!(cursor.style, TmonCursorStyle::Line),
        )
    });
    let alacritty_cursor = alacritty.cursor_state().map(|cursor| {
        (
            cursor.col,
            cursor.row,
            matches!(cursor.style, TerminalCursorStyle::Line),
        )
    });
    assert_eq!(
        tmon_cursor, alacritty_cursor,
        "{}: cursor state differs",
        workload.name
    );
    assert_eq!(
        tmon.scroll_state(),
        alacritty.scroll_state(),
        "{}: scroll state differs",
        workload.name
    );
    assert_eq!(
        tmon.alternate_screen_mode(),
        alacritty.alternate_screen_mode(),
        "{}: screen mode differs",
        workload.name
    );
    assert_eq!(
        tmon.bracketed_paste_mode(),
        alacritty.bracketed_paste_mode(),
        "{}: bracketed-paste mode differs",
        workload.name
    );
}
