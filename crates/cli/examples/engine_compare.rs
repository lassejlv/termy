use std::{
    env,
    hint::black_box,
    mem::size_of,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

#[cfg(feature = "benchmark-allocations")]
mod allocation_counter {
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        sync::atomic::{AtomicU8, AtomicU64, Ordering},
    };

    const DISABLED: u8 = 0;
    const RESETTING: u8 = 1;
    const ENABLED: u8 = 2;
    const STOPPING: u8 = 3;

    const HOOK_ENABLED: u64 = 1 << 63;
    const HOOK_COUNT_MASK: u64 = u32::MAX as u64;
    const HOOK_GENERATION_STEP: u64 = 1 << 32;
    const HOOK_GENERATION_MASK: u64 = !(HOOK_ENABLED | HOOK_COUNT_MASK);

    static STATE: AtomicU8 = AtomicU8::new(DISABLED);
    // The high bit enables registration, the next 31 bits distinguish
    // measurement generations, and the low 32 bits count registered hooks.
    // Keeping generation and count in one atomic prevents an old hook from
    // registering into a later measurement after the gate briefly closed.
    static HOOK_GATE: AtomicU64 = AtomicU64::new(0);
    static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
    static REALLOC_OLD_BYTES: AtomicU64 = AtomicU64::new(0);
    static REALLOC_NEW_BYTES: AtomicU64 = AtomicU64::new(0);

    struct CountingSystem;

    #[global_allocator]
    static GLOBAL_ALLOCATOR: CountingSystem = CountingSystem;

    fn register_hook_from(mut gate: u64) -> bool {
        let generation = gate & HOOK_GENERATION_MASK;
        loop {
            if gate & HOOK_ENABLED == 0 {
                return false;
            }
            if gate & HOOK_GENERATION_MASK != generation {
                return false;
            }
            if gate & HOOK_COUNT_MASK == HOOK_COUNT_MASK {
                return false;
            }
            match HOOK_GATE.compare_exchange_weak(
                gate,
                gate + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => gate = current,
            }
        }
    }

    fn register_hook() -> bool {
        register_hook_from(HOOK_GATE.load(Ordering::Acquire))
    }

    fn unregister_hook() {
        HOOK_GATE.fetch_sub(1, Ordering::AcqRel);
    }

    fn disable_and_wait_for_hooks() {
        STATE.store(STOPPING, Ordering::Release);
        HOOK_GATE.fetch_and(!HOOK_ENABLED, Ordering::AcqRel);
        while HOOK_GATE.load(Ordering::Acquire) & HOOK_COUNT_MASK != 0 {
            std::hint::spin_loop();
        }
        STATE.store(DISABLED, Ordering::Release);
    }

    // SAFETY: Every hook delegates to `System` with the caller-provided pointer,
    // layout, and size unchanged. The additional bookkeeping is lock-free atomic
    // arithmetic and cannot call back into the allocator. Hooks register before
    // delegating, and measurement shutdown waits for every registered hook before
    // reading the counters.
    unsafe impl GlobalAlloc for CountingSystem {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::alloc` guarantees that `layout` is valid, and
            // it is forwarded to the system allocator without modification.
            let pointer = unsafe { System.alloc(layout) };
            if counted && !pointer.is_null() {
                ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::alloc_zeroed` guarantees that `layout` is
            // valid, and it is forwarded to `System` without modification.
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if counted && !pointer.is_null() {
                ALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                ALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            pointer
        }

        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::dealloc` guarantees that `pointer` and
            // `layout` describe a live allocation from this allocator. Both are
            // forwarded to `System` without modification.
            unsafe { System.dealloc(pointer, layout) };
            if counted {
                DEALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                DEALLOC_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
                unregister_hook();
            }
        }

        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            let counted = register_hook();
            // SAFETY: `GlobalAlloc::realloc` guarantees that `pointer`, `layout`,
            // and `new_size` satisfy its contract. All three arguments are
            // forwarded to `System` without modification.
            let new_pointer = unsafe { System.realloc(pointer, layout, new_size) };
            if counted && !new_pointer.is_null() {
                REALLOC_CALLS.fetch_add(1, Ordering::Relaxed);
                REALLOC_OLD_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
                REALLOC_NEW_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
            }
            if counted {
                unregister_hook();
            }
            new_pointer
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub(super) struct AllocationSnapshot {
        pub(super) alloc_calls: u64,
        pub(super) alloc_bytes: u64,
        pub(super) dealloc_calls: u64,
        pub(super) dealloc_bytes: u64,
        pub(super) realloc_calls: u64,
        pub(super) realloc_old_bytes: u64,
        pub(super) realloc_new_bytes: u64,
    }

    impl AllocationSnapshot {
        pub(super) fn net_requested_bytes(self) -> i128 {
            self.alloc_bytes as i128 + self.realloc_new_bytes as i128
                - self.dealloc_bytes as i128
                - self.realloc_old_bytes as i128
        }
    }

    fn reset() {
        ALLOC_CALLS.store(0, Ordering::Relaxed);
        ALLOC_BYTES.store(0, Ordering::Relaxed);
        DEALLOC_CALLS.store(0, Ordering::Relaxed);
        DEALLOC_BYTES.store(0, Ordering::Relaxed);
        REALLOC_CALLS.store(0, Ordering::Relaxed);
        REALLOC_OLD_BYTES.store(0, Ordering::Relaxed);
        REALLOC_NEW_BYTES.store(0, Ordering::Relaxed);
    }

    fn snapshot() -> AllocationSnapshot {
        AllocationSnapshot {
            alloc_calls: ALLOC_CALLS.load(Ordering::Relaxed),
            alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
            dealloc_calls: DEALLOC_CALLS.load(Ordering::Relaxed),
            dealloc_bytes: DEALLOC_BYTES.load(Ordering::Relaxed),
            realloc_calls: REALLOC_CALLS.load(Ordering::Relaxed),
            realloc_old_bytes: REALLOC_OLD_BYTES.load(Ordering::Relaxed),
            realloc_new_bytes: REALLOC_NEW_BYTES.load(Ordering::Relaxed),
        }
    }

    pub(super) struct AllocationMeasurement {
        active: bool,
    }

    impl AllocationMeasurement {
        pub(super) fn begin() -> Result<Self, &'static str> {
            STATE
                .compare_exchange(DISABLED, RESETTING, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| "an allocation measurement is already active")?;
            let gate = HOOK_GATE.load(Ordering::Acquire);
            if gate & (HOOK_ENABLED | HOOK_COUNT_MASK) != 0 {
                STATE.store(DISABLED, Ordering::Release);
                return Err("allocator hooks from the previous measurement are still active");
            }
            reset();
            let generation = gate & HOOK_GENERATION_MASK;
            let next_generation =
                generation.wrapping_add(HOOK_GENERATION_STEP) & HOOK_GENERATION_MASK;
            HOOK_GATE.store(next_generation | HOOK_ENABLED, Ordering::Release);
            STATE.store(ENABLED, Ordering::Release);
            Ok(Self { active: true })
        }

        pub(super) fn finish(mut self) -> AllocationSnapshot {
            disable_and_wait_for_hooks();
            self.active = false;
            snapshot()
        }
    }

    impl Drop for AllocationMeasurement {
        fn drop(&mut self) {
            if self.active {
                disable_and_wait_for_hooks();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn register_hook_for_test() -> bool {
        register_hook()
    }

    #[cfg(test)]
    pub(super) fn unregister_hook_for_test() {
        unregister_hook();
    }

    #[cfg(test)]
    pub(super) fn gate_for_test() -> u64 {
        HOOK_GATE.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn register_hook_from_for_test(gate: u64) -> bool {
        register_hook_from(gate)
    }

    #[cfg(test)]
    pub(super) fn stopping_for_test() -> bool {
        STATE.load(Ordering::Acquire) == STOPPING
            && HOOK_GATE.load(Ordering::Acquire) & HOOK_ENABLED == 0
    }
}

use termy_core::tmon::{
    Combining as TmonCombining, Config as TmonConfig, CursorStyle as TmonCursorStyle,
    DamageSnapshot, RenderDamageSnapshot, ScrollDamage, ScrollDirection, Size as TmonSize,
    Terminal as TmonTerminal, UnderlineStyle as TmonUnderlineStyle,
};
use termy_core::{
    Terminal as AlacrittyTerminal, TerminalCursorStyle, TerminalRenderCell, TerminalRenderColor,
    TerminalRuntimeConfig, TerminalSize as AlacrittySize, TerminalUnderlineStyle,
};

const MIB: usize = 1024 * 1024;
const COLS: u16 = 120;
const ROWS: u16 = 40;
const SCROLLBACK: usize = 10_000;
const SNAPSHOT_PREFILL_BYTES: usize = 2 * MIB;
const NORMALIZED_SNAPSHOT_MIN_DURATION: Duration = Duration::from_millis(250);
const RENDERER_CACHE_MIN_DURATION: Duration = NORMALIZED_SNAPSHOT_MIN_DURATION;
const RENDERER_CACHE_UPDATE: &str = concat!(
    "\x1b[40;1Htail-A\r\n",
    "tail-B",
    "\x1b[5;30r\x1b[5;1H\x1bM",
    "\x1b[30;1H\n",
    "\x1b[r",
    "\x1b[12;10Hpatched cafe\u{301} \x1b[38;2;9;8;7mcolor\x1b[0m",
);
const VALIDATION_BYTES_MAX: usize = 32 * MIB;
const MAX_BENCHMARK_MIB_PER_ENGINE: usize = 2_048;
#[cfg(feature = "benchmark-allocations")]
const MAX_ALLOCATION_MIB_TOTAL: usize = 1_024;
const SUPPLIED_SNAPSHOT_RATIO_BASELINE: f64 = 19.88;
const TMON_RUNTIME_DESCRIPTION: &str = "termy_core::tmon::Terminal display engine (direct)";
const ALACRITTY_RUNTIME_DESCRIPTION: &str =
    "termy_core::Terminal display engine (Alacritty-backed)";
#[cfg(feature = "benchmark-allocations")]
const ALLOCATION_WARMUP_BYTES: usize = 2 * MIB;

struct Workload {
    id: &'static str,
    name: &'static str,
    payload: &'static str,
    minimum_ratio: f64,
}

const WORKLOADS: [Workload; 4] = [
    Workload {
        id: "plain",
        name: "plain scrollback",
        payload: "the quick brown fox jumps over the lazy dog 0123456789\r\n",
        minimum_ratio: 1.05,
    },
    Workload {
        id: "styled",
        name: "styled TUI redraw",
        payload: concat!(
            "\x1b[H\x1b[38;2;120;180;255;48;5;234;1mstatus\x1b[0m",
            "\x1b[2;1H\x1b[Kbuild complete\x1b[3;1Hprogress: 100%",
        ),
        minimum_ratio: 1.05,
    },
    Workload {
        id: "unicode",
        name: "Unicode + wide cells",
        payload: "ASCII café Ελληνικά 日本語 한글 🙂界\r\n",
        minimum_ratio: 1.00,
    },
    Workload {
        id: "edits",
        name: "cursor + line edits",
        payload: "\x1b[10;20Habcdef\x1b[3DXYZ\x1b[K\x1b[4;1H\x1b[2Linserted\x1b[1M\x1b[2J\x1b[H",
        minimum_ratio: 1.00,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Engine {
    Tmon,
    Alacritty,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Stats {
    median: f64,
    min: f64,
    max: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PairedSample {
    first: Engine,
    tmon: f64,
    alacritty: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PairedStats {
    tmon: Stats,
    alacritty: Stats,
    ratio: Stats,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RawUnderlineStyle {
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

include!("engine_compare/normalization.rs");

fn iterations_for(target_bytes: usize, payload: &[u8]) -> usize {
    target_bytes.div_ceil(payload.len())
}

fn parse_sample(engine: Engine, workload: &Workload, target_bytes: usize) -> f64 {
    let payload = workload.payload.as_bytes();
    let iterations = iterations_for(target_bytes, payload);
    let bytes = iterations * payload.len();

    let elapsed = match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            let started = Instant::now();
            for _ in 0..iterations {
                terminal.feed_output(black_box(payload));
            }
            let elapsed = started.elapsed();
            black_box(terminal.snapshot());
            elapsed
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            let started = Instant::now();
            for _ in 0..iterations {
                terminal.feed_output(black_box(payload));
            }
            let elapsed = started.elapsed();
            black_box(terminal.snapshot());
            elapsed
        }
    };

    bytes as f64 / MIB as f64 / elapsed.as_secs_f64()
}

fn prefill_tmon(terminal: &TmonTerminal) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(SNAPSHOT_PREFILL_BYTES, payload) {
        terminal.feed_output(payload);
    }
}

fn prefill_alacritty(terminal: &AlacrittyTerminal) {
    let payload = WORKLOADS[0].payload.as_bytes();
    for _ in 0..iterations_for(SNAPSHOT_PREFILL_BYTES, payload) {
        terminal.feed_output(payload);
    }
}

fn snapshot_sample(engine: Engine, snapshots: usize) -> f64 {
    let elapsed = match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            prefill_tmon(&terminal);
            let started = Instant::now();
            for _ in 0..snapshots {
                black_box(terminal.snapshot());
            }
            started.elapsed()
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            prefill_alacritty(&terminal);
            let started = Instant::now();
            for _ in 0..snapshots {
                black_box(terminal.snapshot());
            }
            started.elapsed()
        }
    };
    snapshots as f64 / elapsed.as_secs_f64()
}

fn warm_normalized_snapshot(engine: Engine) {
    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            prepare_normalized_tmon(&terminal, SNAPSHOT_PREFILL_BYTES);
            black_box(normalized_tmon_frame(&terminal));
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            prepare_normalized_alacritty(&terminal, SNAPSHOT_PREFILL_BYTES);
            black_box(normalized_alacritty_frame(&terminal));
        }
    }
}

fn measure_normalized_snapshots(
    minimum_snapshots: usize,
    mut snapshot: impl FnMut() -> NormalizedFrame,
) -> f64 {
    const BATCH_SIZE: usize = 64;

    let started = Instant::now();
    let mut completed = 0usize;
    loop {
        for _ in 0..BATCH_SIZE {
            black_box(snapshot());
        }
        completed = completed.saturating_add(BATCH_SIZE);
        let elapsed = started.elapsed();
        if completed >= minimum_snapshots && elapsed >= NORMALIZED_SNAPSHOT_MIN_DURATION {
            return completed as f64 / elapsed.as_secs_f64();
        }
    }
}

fn normalized_snapshot_sample(engine: Engine, minimum_snapshots: usize) -> f64 {
    match engine {
        Engine::Tmon => {
            let terminal = tmon_terminal();
            prepare_normalized_tmon(&terminal, SNAPSHOT_PREFILL_BYTES);
            measure_normalized_snapshots(minimum_snapshots, || normalized_tmon_frame(&terminal))
        }
        Engine::Alacritty => {
            let terminal = alacritty_terminal();
            prepare_normalized_alacritty(&terminal, SNAPSHOT_PREFILL_BYTES);
            measure_normalized_snapshots(minimum_snapshots, || {
                normalized_alacritty_frame(&terminal)
            })
        }
    }
}

fn stats(samples: &[f64]) -> Stats {
    assert!(
        !samples.is_empty(),
        "cannot calculate statistics without samples"
    );
    assert!(
        samples.iter().all(|sample| sample.is_finite()),
        "benchmark samples must be finite"
    );
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    } else {
        sorted[middle]
    };
    Stats {
        median,
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

impl PairedSample {
    fn ratio(self) -> f64 {
        assert!(
            self.tmon.is_finite() && self.tmon >= 0.0,
            "Tmon sample must be finite and non-negative, got {:?}",
            self.tmon
        );
        assert!(
            self.alacritty.is_finite() && self.alacritty > 0.0,
            "Alacritty sample must be finite and positive, got {:?}",
            self.alacritty
        );
        let ratio = self.tmon / self.alacritty;
        assert!(ratio.is_finite(), "paired throughput ratio must be finite");
        ratio
    }

    fn order_label(self) -> &'static str {
        match self.first {
            Engine::Tmon => "T/A",
            Engine::Alacritty => "A/T",
        }
    }
}

fn measure_pair(order: [Engine; 2], mut measure: impl FnMut(Engine) -> f64) -> PairedSample {
    let first = order[0];
    let (tmon, alacritty) = match order {
        [Engine::Tmon, Engine::Alacritty] => (measure(Engine::Tmon), measure(Engine::Alacritty)),
        [Engine::Alacritty, Engine::Tmon] => {
            let alacritty = measure(Engine::Alacritty);
            let tmon = measure(Engine::Tmon);
            (tmon, alacritty)
        }
        _ => panic!("a sample pair must measure each engine exactly once"),
    };
    PairedSample {
        first,
        tmon,
        alacritty,
    }
}

fn paired_stats(samples: &[PairedSample]) -> PairedStats {
    let tmon = samples.iter().map(|sample| sample.tmon).collect::<Vec<_>>();
    let alacritty = samples
        .iter()
        .map(|sample| sample.alacritty)
        .collect::<Vec<_>>();
    let ratios = samples
        .iter()
        .copied()
        .map(PairedSample::ratio)
        .collect::<Vec<_>>();
    PairedStats {
        tmon: stats(&tmon),
        alacritty: stats(&alacritty),
        ratio: stats(&ratios),
    }
}

fn format_paired_ratios(samples: &[PairedSample]) -> String {
    samples
        .iter()
        .copied()
        .map(|sample| format!("{} {:.3}x", sample.order_label(), sample.ratio()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_parse_samples(
    workload: &Workload,
    target_bytes: usize,
    sample_count: usize,
    tmon_first: bool,
) -> Vec<PairedSample> {
    let warmup_bytes = target_bytes.min(2 * MIB);
    for engine in sample_order(0, tmon_first) {
        black_box(parse_sample(engine, workload, warmup_bytes));
    }

    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        samples.push(measure_pair(sample_order(index, tmon_first), |engine| {
            parse_sample(engine, workload, target_bytes)
        }));
    }
    samples
}

fn collect_snapshot_samples(
    snapshot_count: usize,
    sample_count: usize,
    tmon_first: bool,
) -> Vec<PairedSample> {
    for engine in sample_order(0, tmon_first) {
        black_box(snapshot_sample(engine, 10));
    }

    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        samples.push(measure_pair(sample_order(index, tmon_first), |engine| {
            snapshot_sample(engine, snapshot_count)
        }));
    }
    samples
}

fn collect_normalized_snapshot_samples(
    minimum_snapshot_count: usize,
    sample_count: usize,
    tmon_first: bool,
) -> Vec<PairedSample> {
    for engine in sample_order(0, tmon_first) {
        warm_normalized_snapshot(engine);
    }

    let mut samples = Vec::with_capacity(sample_count);
    for index in 0..sample_count {
        samples.push(measure_pair(sample_order(index, tmon_first), |engine| {
            normalized_snapshot_sample(engine, minimum_snapshot_count)
        }));
    }
    samples
}

fn renderer_cache_report_enabled() -> bool {
    env::var_os("CI").is_some()
}

fn renderer_cache_order(index: usize) -> [RendererCachePath; 2] {
    if index.is_multiple_of(2) {
        [
            RendererCachePath::FullRebuild,
            RendererCachePath::ScrollReplay,
        ]
    } else {
        [
            RendererCachePath::ScrollReplay,
            RendererCachePath::FullRebuild,
        ]
    }
}

impl RendererCacheSample {
    fn ratio(self) -> f64 {
        assert!(
            self.full_rebuild.is_finite() && self.full_rebuild > 0.0,
            "full renderer-cache rebuild sample must be finite and positive"
        );
        assert!(
            self.scroll_replay.is_finite() && self.scroll_replay >= 0.0,
            "renderer-cache replay sample must be finite and non-negative"
        );
        let ratio = self.scroll_replay / self.full_rebuild;
        assert!(ratio.is_finite(), "renderer-cache ratio must be finite");
        ratio
    }

    fn order_label(self) -> &'static str {
        match self.first {
            RendererCachePath::FullRebuild => "F/R",
            RendererCachePath::ScrollReplay => "R/F",
        }
    }
}

fn measure_renderer_cache_path(
    fixture: &RendererCacheFixture,
    path: RendererCachePath,
    minimum_updates: usize,
) -> f64 {
    let mut elapsed = Duration::ZERO;
    let mut completed = 0usize;
    loop {
        match path {
            RendererCachePath::FullRebuild => {
                let started = Instant::now();
                let frame = fresh_renderer_cache_frame(
                    &fixture.terminal,
                    fixture.expected.cols,
                    fixture.expected.rows,
                );
                elapsed = elapsed.saturating_add(started.elapsed());
                black_box(frame);
            }
            RendererCachePath::ScrollReplay => {
                // A live renderer already owns the prior frame. Cloning this
                // benchmark cache copies only its root Arc here. The timed
                // update copy-on-writes the outer row table, and then each
                // actually patched row.
                let mut frame = black_box(&fixture.before).clone();
                let started = Instant::now();
                let applied =
                    apply_renderer_cache_update(&mut frame, &fixture.terminal, &fixture.update);
                elapsed = elapsed.saturating_add(started.elapsed());
                assert!(applied, "captured renderer-cache generation became stale");
                black_box(frame);
            }
        }
        completed = completed.saturating_add(1);
        if completed >= minimum_updates && elapsed >= RENDERER_CACHE_MIN_DURATION {
            return completed as f64 / elapsed.as_secs_f64();
        }
    }
}

fn measure_renderer_cache_pair(
    fixture: &RendererCacheFixture,
    order: [RendererCachePath; 2],
    minimum_updates: usize,
) -> RendererCacheSample {
    let first = order[0];
    let (full_rebuild, scroll_replay) = match order {
        [
            RendererCachePath::FullRebuild,
            RendererCachePath::ScrollReplay,
        ] => (
            measure_renderer_cache_path(fixture, RendererCachePath::FullRebuild, minimum_updates),
            measure_renderer_cache_path(fixture, RendererCachePath::ScrollReplay, minimum_updates),
        ),
        [
            RendererCachePath::ScrollReplay,
            RendererCachePath::FullRebuild,
        ] => {
            let scroll_replay = measure_renderer_cache_path(
                fixture,
                RendererCachePath::ScrollReplay,
                minimum_updates,
            );
            let full_rebuild = measure_renderer_cache_path(
                fixture,
                RendererCachePath::FullRebuild,
                minimum_updates,
            );
            (full_rebuild, scroll_replay)
        }
        _ => panic!("a renderer-cache pair must measure each path exactly once"),
    };
    RendererCacheSample {
        first,
        full_rebuild,
        scroll_replay,
    }
}

fn renderer_cache_stats(samples: &[RendererCacheSample]) -> RendererCacheStats {
    let full_rebuild = samples
        .iter()
        .map(|sample| sample.full_rebuild)
        .collect::<Vec<_>>();
    let scroll_replay = samples
        .iter()
        .map(|sample| sample.scroll_replay)
        .collect::<Vec<_>>();
    let ratios = samples
        .iter()
        .copied()
        .map(RendererCacheSample::ratio)
        .collect::<Vec<_>>();
    RendererCacheStats {
        full_rebuild: stats(&full_rebuild),
        scroll_replay: stats(&scroll_replay),
        ratio: stats(&ratios),
    }
}

fn format_renderer_cache_ratios(samples: &[RendererCacheSample]) -> String {
    samples
        .iter()
        .copied()
        .map(|sample| format!("{} {:.3}x", sample.order_label(), sample.ratio()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_renderer_cache_samples(
    fixture: &RendererCacheFixture,
    minimum_updates: usize,
    sample_count: usize,
) -> Vec<RendererCacheSample> {
    assert_renderer_cache_preflight(fixture);
    for path in renderer_cache_order(0) {
        match path {
            RendererCachePath::FullRebuild => {
                black_box(fresh_renderer_cache_frame(
                    &fixture.terminal,
                    fixture.expected.cols,
                    fixture.expected.rows,
                ));
            }
            RendererCachePath::ScrollReplay => {
                let mut frame = fixture.before.clone();
                assert!(apply_renderer_cache_update(
                    &mut frame,
                    &fixture.terminal,
                    &fixture.update,
                ));
                black_box(frame);
            }
        }
    }

    (0..sample_count)
        .map(|index| {
            measure_renderer_cache_pair(fixture, renderer_cache_order(index), minimum_updates)
        })
        .collect()
}

fn run_profile_mode(target_bytes: usize) -> bool {
    let Ok(workload_id) = env::var("TMON_PROFILE_WORKLOAD") else {
        return false;
    };
    let engine = match env::var("TMON_PROFILE_ENGINE").as_deref() {
        Ok("alacritty") => Engine::Alacritty,
        Ok("tmon") | Err(_) => Engine::Tmon,
        Ok(other) => panic!("TMON_PROFILE_ENGINE must be tmon or alacritty, got {other:?}"),
    };
    let repetitions = setting("TMON_PROFILE_REPETITIONS", 1, 1, 100);
    if workload_id == "normalized" {
        let snapshots = setting("TMON_BENCH_SNAPSHOTS", 250, 10, 10_000);
        println!(
            "Profiling {} normalized full-frame construction for {repetitions} repetitions",
            match engine {
                Engine::Tmon => "Tmon",
                Engine::Alacritty => "Alacritty",
            },
        );
        for _ in 0..repetitions {
            black_box(normalized_snapshot_sample(engine, snapshots));
        }
        return true;
    }
    let workload = WORKLOADS
        .iter()
        .find(|workload| workload.id == workload_id)
        .unwrap_or_else(|| {
            panic!("TMON_PROFILE_WORKLOAD must be plain, styled, unicode, edits, or normalized")
        });
    println!(
        "Profiling {} with {} MiB of {} output per repetition",
        match engine {
            Engine::Tmon => "Tmon",
            Engine::Alacritty => "Alacritty",
        },
        target_bytes / MIB,
        workload.id,
    );
    for _ in 0..repetitions {
        black_box(parse_sample(engine, workload, target_bytes));
    }
    true
}

#[cfg(feature = "benchmark-allocations")]
fn run_allocation_mode() {
    let target_mib = setting("TMON_BENCH_TARGET_MIB", 32, 1, 1024);
    let total_mib = target_mib
        .checked_mul(WORKLOADS.len())
        .and_then(|mib| mib.checked_mul(2))
        .expect("allocation workload size overflowed usize");
    assert!(
        total_mib <= MAX_ALLOCATION_MIB_TOTAL,
        "requested allocation accounting parses {total_mib} MiB across both engines; \
         the CI-safe limit is {MAX_ALLOCATION_MIB_TOTAL} MiB. Reduce \
         TMON_BENCH_TARGET_MIB"
    );
    let target_bytes = target_mib * MIB;

    println!("Tmon vs Alacritty allocation request accounting");
    println!("===============================================");
    println!("Platform: {}/{}", env::consts::OS, env::consts::ARCH);
    println!("Tmon runtime: {TMON_RUNTIME_DESCRIPTION}");
    println!("Alacritty runtime: {ALACRITTY_RUNTIME_DESCRIPTION}");
    println!(
        "Engine selection: direct Tmon plus forced core Alacritty comparison; desktop engine environment is ignored"
    );
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK} lines");
    println!("Parse target per engine/workload: {target_bytes} bytes ({target_mib} MiB)");
    println!(
        "Warmup per engine/workload: {} bytes ({} MiB, uncounted)",
        ALLOCATION_WARMUP_BYTES,
        ALLOCATION_WARMUP_BYTES / MIB
    );
    println!();

    for workload in &WORKLOADS {
        let payload = workload.payload.as_bytes();
        let processed_bytes = iterations_for(target_bytes, payload) * payload.len();
        println!("{}", workload.name);
        for engine in [Engine::Tmon, Engine::Alacritty] {
            let allocations = allocation_sample(engine, workload, target_bytes);
            let engine_name = match engine {
                Engine::Tmon => "Tmon",
                Engine::Alacritty => "Alacritty",
            };
            println!("  {engine_name}");
            println!("    processed bytes:       {processed_bytes}");
            println!(
                "    alloc:                 {} successful calls, {} requested bytes",
                allocations.alloc_calls, allocations.alloc_bytes
            );
            println!(
                "    dealloc:               {} calls, {} requested bytes",
                allocations.dealloc_calls, allocations.dealloc_bytes
            );
            println!(
                "    realloc:               {} successful calls, {} old requested bytes, {} new requested bytes",
                allocations.realloc_calls,
                allocations.realloc_old_bytes,
                allocations.realloc_new_bytes
            );
            println!(
                "    net requested change:  {:+} bytes",
                allocations.net_requested_bytes()
            );
        }
        println!();
    }

    println!("Notes");
    println!("  - This mode is report-only and has no performance or allocation gates.");
    println!(
        "  - A fresh terminal is constructed before counting; only its feed_output loop is counted."
    );
    println!("  - Terminal snapshots and terminal destruction happen after counting stops.");
    println!("  - alloc includes successful alloc_zeroed calls and requested bytes.");
    println!("  - Net requested change = alloc + realloc-new - dealloc - realloc-old bytes.");
    println!("  - Counts cover all process allocator traffic while the feed guard is active.");
    println!(
        "  - Requested-byte accounting is not RSS, retained/usable heap, allocator metadata, or physical memory."
    );
}

#[cfg_attr(feature = "benchmark-allocations", allow(dead_code))]
fn run_timed_mode() {
    let target_mib = setting("TMON_BENCH_TARGET_MIB", 32, 1, 1024);
    let target_bytes = target_mib * MIB;
    if run_profile_mode(target_bytes) {
        return;
    }
    let sample_count = require_even_sample_count(setting("TMON_BENCH_SAMPLES", 8, 3, 25));
    let snapshot_count = setting("TMON_BENCH_SNAPSHOTS", 250, 10, 10_000);
    let renderer_cache_fixture =
        renderer_cache_report_enabled().then(prepare_renderer_cache_fixture);
    for workload in &WORKLOADS {
        validate_target_ratio(workload.minimum_ratio).unwrap_or_else(|reason| {
            panic!(
                "{} has an invalid target ratio {:?}: {reason}",
                workload.name, workload.minimum_ratio
            )
        });
    }
    let benchmark_mib_per_engine = target_mib
        .checked_mul(sample_count)
        .and_then(|mib| mib.checked_mul(WORKLOADS.len()))
        .expect("benchmark workload size overflowed usize");
    assert!(
        benchmark_mib_per_engine <= MAX_BENCHMARK_MIB_PER_ENGINE,
        "requested benchmark parses {benchmark_mib_per_engine} MiB per engine; \
         the CI-safe limit is {MAX_BENCHMARK_MIB_PER_ENGINE} MiB. Reduce \
         TMON_BENCH_TARGET_MIB or TMON_BENCH_SAMPLES"
    );

    println!("Tmon vs Alacritty terminal engine benchmark");
    println!("============================================");
    println!("Platform: {}/{}", env::consts::OS, env::consts::ARCH);
    println!("Tmon runtime: {TMON_RUNTIME_DESCRIPTION}");
    println!("Alacritty runtime: {ALACRITTY_RUNTIME_DESCRIPTION}");
    println!(
        "Engine selection: direct Tmon plus forced core Alacritty comparison; desktop engine environment is ignored"
    );
    println!("Grid: {COLS}x{ROWS}, scrollback: {SCROLLBACK} lines");
    println!("Samples: {sample_count}, parse target: {target_mib} MiB/sample");
    println!("Legacy raw snapshots: {snapshot_count}/sample");
    println!(
        "Normalized snapshots: at least {snapshot_count}/sample and {:.0} ms/sample",
        NORMALIZED_SNAPSHOT_MIN_DURATION.as_secs_f64() * 1000.0
    );
    if renderer_cache_fixture.is_some() {
        println!(
            "Renderer-cache updates: at least {snapshot_count}/sample and {:.0} ms/sample (CI only)",
            RENDERER_CACHE_MIN_DURATION.as_secs_f64() * 1000.0
        );
    }
    println!();
    println!(
        "Correctness preflight (untimed full-grid comparison, up to {} MiB/workload)",
        VALIDATION_BYTES_MAX / MIB
    );
    for workload in &WORKLOADS {
        assert_workload_equivalence(workload, target_bytes.min(VALIDATION_BYTES_MAX));
        println!("  {:<24} matched", workload.name);
    }
    assert_normalized_fixture_equivalence(SNAPSHOT_PREFILL_BYTES);
    println!("  {:<24} matched", "normalized mixed frame");
    if let Some(fixture) = &renderer_cache_fixture {
        assert_renderer_cache_preflight(fixture);
        println!("  {:<24} matched", "scroll replay cache");
    }
    println!();
    println!("Static grid cell size (excludes optional heap allocations):");
    println!(
        "  Tmon:       {} bytes",
        size_of::<termy_core::tmon::Cell>()
    );
    println!(
        "  Alacritty:  {} bytes",
        size_of::<alacritty_terminal::term::cell::Cell>()
    );
    println!();
    println!(
        "Integrated Termy backend throughput (median MiB/s and paired ratio; higher is better)"
    );
    println!(
        "{:<24} {:>12} {:>12} {:>14} {:>10}",
        "Workload", "Tmon", "Alacritty", "Tmon/Alac", "Target"
    );
    println!("{}", "-".repeat(91));

    let mut ratios = Vec::with_capacity(WORKLOADS.len());
    let mut failed_targets = Vec::new();
    for (index, workload) in WORKLOADS.iter().enumerate() {
        let samples = collect_parse_samples(
            workload,
            target_bytes,
            sample_count,
            index.is_multiple_of(2),
        );
        let measurements = paired_stats(&samples);
        let ratio = measurements.ratio.median;
        ratios.push(ratio);
        if !meets_target(ratio, workload.minimum_ratio) {
            failed_targets.push((workload.name, ratio, workload.minimum_ratio));
        }
        println!(
            "{:<24} {:>12.1} {:>12.1} {:>13.3}x {:>9.3}x",
            workload.name,
            measurements.tmon.median,
            measurements.alacritty.median,
            ratio,
            workload.minimum_ratio
        );
        println!(
            "  range                  {:>7.1}-{:<7.1} {:>7.1}-{:<7.1} {:>7.3}-{:<7.3}x",
            measurements.tmon.min,
            measurements.tmon.max,
            measurements.alacritty.min,
            measurements.alacritty.max,
            measurements.ratio.min,
            measurements.ratio.max
        );
        println!(
            "  paired ratios          {}",
            format_paired_ratios(&samples)
        );
    }

    let parse_geomean =
        (ratios.iter().map(|ratio| ratio.ln()).sum::<f64>() / ratios.len() as f64).exp();
    let normalized_snapshot_samples =
        collect_normalized_snapshot_samples(snapshot_count, sample_count, true);
    let normalized_snapshot = paired_stats(&normalized_snapshot_samples);
    let normalized_snapshot_ratio = normalized_snapshot.ratio.median;
    let snapshot_samples = collect_snapshot_samples(snapshot_count, sample_count, false);
    let snapshot = paired_stats(&snapshot_samples);
    let snapshot_ratio = snapshot.ratio.median;
    let snapshot_baseline_retention = snapshot_ratio / SUPPLIED_SNAPSHOT_RATIO_BASELINE * 100.0;
    let renderer_cache = renderer_cache_fixture.as_ref().map(|fixture| {
        let samples = collect_renderer_cache_samples(fixture, snapshot_count, sample_count);
        let measurements = renderer_cache_stats(&samples);
        (samples, measurements)
    });

    println!();
    println!("Renderer-neutral normalized full-frame throughput (equivalent work; calls/s)");
    println!(
        "  Tmon:                       {:>12.1}",
        normalized_snapshot.tmon.median
    );
    println!(
        "  Alacritty:                  {:>12.1}",
        normalized_snapshot.alacritty.median
    );
    println!("  Median paired Tmon/Alac:    {normalized_snapshot_ratio:>11.3}x");
    println!(
        "  Range:                      {:>7.3}-{:<7.3}x",
        normalized_snapshot.ratio.min, normalized_snapshot.ratio.max
    );
    println!(
        "  Paired ratios:              {}",
        format_paired_ratios(&normalized_snapshot_samples)
    );
    println!("  Gate:                        report-only until the first CI baseline");

    if let Some((samples, measurements)) = &renderer_cache {
        println!();
        println!("Tmon normalized renderer-cache throughput (equivalent final cache; calls/s)");
        println!(
            "  Fresh visible-frame rebuild:{:>12.1}",
            measurements.full_rebuild.median
        );
        println!(
            "  Scroll replay + span patch: {:>12.1}",
            measurements.scroll_replay.median
        );
        println!(
            "  Median replay/rebuild:       {:>11.3}x",
            measurements.ratio.median
        );
        println!(
            "  Range:                       {:>7.3}-{:<7.3}x",
            measurements.ratio.min, measurements.ratio.max
        );
        println!(
            "  Paired ratios:               {}",
            format_renderer_cache_ratios(samples)
        );
        println!("  Gate:                         report-only");
    }

    println!();
    println!("Legacy public snapshot API throughput (different result work; calls/s)");
    println!(
        "  Tmon:                       {:>12.1}",
        snapshot.tmon.median
    );
    println!(
        "  Alacritty:                  {:>12.1}",
        snapshot.alacritty.median
    );
    println!("  Median paired Tmon/Alac:    {snapshot_ratio:>11.3}x");
    println!("  Supplied baseline:          {SUPPLIED_SNAPSHOT_RATIO_BASELINE:>11.3}x");
    println!("  Supplied-baseline retention:{snapshot_baseline_retention:>11.1}%");
    println!(
        "  Paired ratios:              {}",
        format_paired_ratios(&snapshot_samples)
    );
    println!();
    println!("Summary");
    println!("  Paired parse-ratio geometric mean: {parse_geomean:.3}x");
    println!("  Normalized full-frame ratio:       {normalized_snapshot_ratio:.3}x");
    if let Some((_, measurements)) = &renderer_cache {
        println!(
            "  Renderer-cache replay speedup:     {:.3}x",
            measurements.ratio.median
        );
    }
    println!("  Legacy public snapshot API ratio:  {snapshot_ratio:.3}x");
    if failed_targets.is_empty() {
        println!("  Parser/grid targets:             PASS");
    } else {
        println!("  Parser/grid targets:             FAIL");
        for (name, actual, minimum) in &failed_targets {
            println!("    {name}: {actual:.3}x < {minimum:.3}x");
        }
    }
    println!();
    println!("Notes");
    println!("  - Ratios above 1.000x favor Tmon; below 1.000x favor Alacritty.");
    println!(
        "  - The selected runtimes are printed above and do not depend on the desktop engine environment."
    );
    println!("  - Alacritty is measured through Termy's termy_core wrapper.");
    println!("  - Even sample counts balance T/A and A/T starting order within every workload.");
    println!("  - Parser targets use the median of per-sample paired ratios.");
    println!(
        "  - Parser calls use each workload's small repeated payload, matching the saved baseline."
    );
    println!(
        "  - Tmon snapshots copy raw Cells; termy_core converts Alacritty cells into TermyFrame."
    );
    println!(
        "    Snapshot ratios measure the current public APIs, not equivalent raw engine work."
    );
    println!("  - Snapshot ratio is report-only: a 19.880x * 0.95 hosted-CI gate is not sound");
    println!("    because the APIs differ; retention is only a supplied-baseline trend.");
    println!(
        "  - The normalized frame uses one shared owned cell/metadata contract for both engines."
    );
    println!(
        "    It includes raw colors, styles, combining text, hyperlinks, wide roles, and wraps."
    );
    println!(
        "  - Every normalized snapshot sample runs for at least 250 ms to reduce timer noise."
    );
    if renderer_cache.is_some() {
        println!(
            "  - Renderer-cache paths consume one shared parsed terminal generation; parsing and damage capture are untimed for both."
        );
        println!(
            "  - Timed replay copy-on-writes the outer Arc row table, rotates row handles, then clones and patches only final dirty rows."
        );
        println!("  - Resetting replay samples clones only the root Arc outside the timed update.");
        println!(
            "  - The exact-result preflight compares the complete owned cache, cursor, and scroll metadata."
        );
    }
    println!("  - This excludes PTY I/O, GPUI rendering, input latency, RSS, and feature parity.");
    println!("  - Compare trends across runs; shared GitHub runners have timing noise.");

    if env::var("TMON_BENCH_ENFORCE_TARGETS").as_deref() == Ok("1") && !failed_targets.is_empty() {
        eprintln!("Tmon missed one or more required parser/grid throughput targets");
        std::process::exit(2);
    }
}

#[cfg(not(feature = "benchmark-allocations"))]
fn main() {
    assert!(
        env::var("TMON_BENCH_ALLOCATIONS_ONLY").as_deref() != Ok("1"),
        "TMON_BENCH_ALLOCATIONS_ONLY=1 requires rebuilding engine_compare with \
         --features benchmark-allocations"
    );
    run_timed_mode();
}

#[cfg(feature = "benchmark-allocations")]
fn main() {
    assert!(
        env::var("TMON_BENCH_ALLOCATIONS_ONLY").as_deref() == Ok("1"),
        "engine_compare was built with benchmark-allocations and refuses timed mode; \
         set TMON_BENCH_ALLOCATIONS_ONLY=1"
    );
    run_allocation_mode();
}

#[cfg(test)]
#[path = "engine_compare/tests.rs"]
mod tests;
