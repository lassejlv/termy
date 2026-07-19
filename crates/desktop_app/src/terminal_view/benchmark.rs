use serde::Serialize;
use std::{
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use sysinfo::{ProcessesToUpdate, System, get_current_pid};
use termy_terminal_ui::{
    monotonic_now_ns, terminal_ui_render_metrics_reset, terminal_ui_render_metrics_snapshot,
};

pub(super) const BENCHMARK_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

const COMMAND_ENV: &str = "TERMY_BENCHMARK_COMMAND";
const SCENARIO_ENV: &str = "TERMY_BENCHMARK_SCENARIO";
const METRICS_PATH_ENV: &str = "TERMY_BENCHMARK_METRICS_PATH";
const DURATION_SECS_ENV: &str = "TERMY_BENCHMARK_DURATION_SECS";
const EXIT_ON_COMPLETE_ENV: &str = "TERMY_BENCHMARK_EXIT_ON_COMPLETE";
const BUILD_LABEL_ENV: &str = "TERMY_BENCHMARK_BUILD_LABEL";
const GIT_SHA_ENV: &str = "TERMY_BENCHMARK_GIT_SHA";
// Benchmark timing starts before the PTY child has necessarily launched. Give
// cold CI runners enough startup room; a normally exiting driver still closes
// the benchmark immediately through the terminal-exit path.
const COMPLETION_DEADLINE_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BenchmarkConfig {
    pub command: String,
    pub scenario: String,
    pub metrics_path: PathBuf,
    pub duration: Option<Duration>,
    pub exit_on_complete: bool,
    pub build_label: Option<String>,
    pub git_sha: Option<String>,
}

impl BenchmarkConfig {
    pub fn from_env() -> Result<Option<Self>, String> {
        let command = env::var(COMMAND_ENV).ok();
        let scenario = env::var(SCENARIO_ENV).ok();
        let metrics_path = env::var(METRICS_PATH_ENV).ok();

        if command.is_none() && scenario.is_none() && metrics_path.is_none() {
            return Ok(None);
        }

        let command = command
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{COMMAND_ENV} is required when benchmark mode is enabled"))?;
        let scenario = scenario
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{SCENARIO_ENV} is required when benchmark mode is enabled"))?;
        let metrics_path = metrics_path
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!("{METRICS_PATH_ENV} is required when benchmark mode is enabled")
            })?;

        Ok(Some(Self {
            command,
            scenario,
            metrics_path: PathBuf::from(metrics_path),
            duration: parse_optional_duration()?,
            exit_on_complete: env_flag(EXIT_ON_COMPLETE_ENV),
            build_label: optional_env(BUILD_LABEL_ENV),
            git_sha: optional_env(GIT_SHA_ENV),
        }))
    }
}

fn optional_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_optional_duration() -> Result<Option<Duration>, String> {
    let Some(value) = optional_env(DURATION_SECS_ENV) else {
        return Ok(None);
    };
    let seconds = value
        .parse::<u64>()
        .map_err(|error| format!("{DURATION_SECS_ENV} must be a positive integer: {error}"))?;
    if seconds == 0 {
        return Err(format!("{DURATION_SECS_ENV} must be greater than zero"));
    }
    Ok(Some(Duration::from_secs(seconds)))
}

fn env_flag(key: &str) -> bool {
    env::var(key).ok().is_some_and(|value| {
        matches!(value.trim(), "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
}

#[derive(Clone, Debug, Default)]
pub(super) struct BenchmarkCounters {
    pub view_wake_signals: u64,
    pub terminal_event_drain_passes: u64,
    pub terminal_redraws: u64,
    pub alt_screen_fallback_redraws: u64,
}

#[derive(Debug)]
pub(super) struct BenchmarkSession {
    config: BenchmarkConfig,
    start_at: Instant,
    sample_started_at: Instant,
    last_frame_at: Option<Instant>,
    frame_intervals_micros: Vec<u32>,
    frames_in_sample: u64,
    total_frames: u64,
    samples: Vec<BenchmarkSample>,
    frame_events: Vec<BenchmarkFrameEvent>,
    counters: BenchmarkCounters,
    system: System,
    pid: Option<sysinfo::Pid>,
    finished: bool,
}

impl BenchmarkSession {
    pub fn new(config: BenchmarkConfig) -> Self {
        terminal_ui_render_metrics_reset();
        Self {
            config,
            start_at: Instant::now(),
            sample_started_at: Instant::now(),
            last_frame_at: None,
            frame_intervals_micros: Vec::with_capacity(2048),
            frames_in_sample: 0,
            total_frames: 0,
            samples: Vec::new(),
            frame_events: Vec::with_capacity(2048),
            counters: BenchmarkCounters::default(),
            system: System::new(),
            pid: get_current_pid().ok(),
            finished: false,
        }
    }

    pub fn record_frame(&mut self, now: Instant) {
        self.frames_in_sample = self.frames_in_sample.saturating_add(1);
        self.total_frames = self.total_frames.saturating_add(1);
        if let Some(previous) = self.last_frame_at {
            let micros = now.saturating_duration_since(previous).as_micros();
            let micros = micros.min(u128::from(u32::MAX)) as u32;
            self.frame_intervals_micros.push(micros);
        }
        self.last_frame_at = Some(now);

        let terminal_ui = terminal_ui_render_metrics_snapshot();
        self.frame_events.push(BenchmarkFrameEvent {
            monotonic_ns: monotonic_now_ns(),
            elapsed_ms: duration_millis(now.saturating_duration_since(self.start_at)),
            total_frames: self.total_frames,
            terminal_redraws: self.counters.terminal_redraws,
            view_wake_signals: self.counters.view_wake_signals,
            runtime_wakeups: terminal_ui.runtime_wakeup_count,
        });
    }

    pub fn record_view_wakeup(&mut self) {
        self.counters.view_wake_signals = self.counters.view_wake_signals.saturating_add(1);
    }

    pub fn record_terminal_event_drain_pass(&mut self) {
        self.counters.terminal_event_drain_passes =
            self.counters.terminal_event_drain_passes.saturating_add(1);
    }

    pub fn record_terminal_redraw(&mut self) {
        self.counters.terminal_redraws = self.counters.terminal_redraws.saturating_add(1);
    }

    pub fn sample_if_due(&mut self, now: Instant) {
        if now.saturating_duration_since(self.sample_started_at) < BENCHMARK_SAMPLE_INTERVAL {
            return;
        }
        self.push_sample(now);
    }

    pub fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        self.push_sample(Instant::now());
        self.write_metrics()?;
        self.finished = true;
        Ok(())
    }

    pub fn exit_on_complete(&self) -> bool {
        self.config.exit_on_complete
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn completion_deadline_reached(&self, now: Instant) -> bool {
        if self.finished {
            return false;
        }
        let Some(duration) = self.config.duration else {
            return false;
        };
        let deadline = duration.saturating_add(COMPLETION_DEADLINE_GRACE);
        now.saturating_duration_since(self.start_at) >= deadline
    }

    fn push_sample(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.sample_started_at);
        let elapsed_secs = elapsed.as_secs_f32();
        let fps = if elapsed_secs > f32::EPSILON {
            self.frames_in_sample as f32 / elapsed_secs
        } else {
            0.0
        };
        self.sample_started_at = now;
        self.frames_in_sample = 0;

        let (cpu_percent, memory_bytes) = self.refresh_process_metrics();
        let terminal_ui = terminal_ui_render_metrics_snapshot();
        let (frame_p50_ms, frame_p95_ms, frame_p99_ms) =
            frame_percentiles(&self.frame_intervals_micros);
        self.samples.push(BenchmarkSample {
            elapsed_ms: duration_millis(now.saturating_duration_since(self.start_at)),
            fps,
            frame_p50_ms,
            frame_p95_ms,
            frame_p99_ms,
            cpu_percent,
            memory_bytes,
            runtime_wakeups: terminal_ui.runtime_wakeup_count,
            view_wake_signals: self.counters.view_wake_signals,
            terminal_event_drain_passes: self.counters.terminal_event_drain_passes,
            terminal_redraws: self.counters.terminal_redraws,
            alt_screen_fallback_redraws: self.counters.alt_screen_fallback_redraws,
            grid_paint_count: terminal_ui.grid_paint_count,
            shape_line_calls: terminal_ui.shape_line_calls,
            shaped_line_cache_hits: terminal_ui.shaped_line_cache_hits,
            shaped_line_cache_misses: terminal_ui.shaped_line_cache_misses,
        });
    }

    fn refresh_process_metrics(&mut self) -> (f32, u64) {
        let Some(pid) = self.pid else {
            return (0.0, 0);
        };

        let _ = self
            .system
            .refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        self.system
            .process(pid)
            .map_or((0.0, 0), |process| (process.cpu_usage(), process.memory()))
    }

    fn write_metrics(&self) -> Result<(), String> {
        fs::create_dir_all(&self.config.metrics_path).map_err(|error| {
            format!(
                "failed to create benchmark metrics dir {}: {error}",
                self.config.metrics_path.display()
            )
        })?;

        let timeline_path = self.config.metrics_path.join("timeline.ndjson");
        let timeline_tmp_path = temp_path(&timeline_path);
        let timeline_file = File::create(&timeline_tmp_path).map_err(|error| {
            format!("failed to create {}: {error}", timeline_tmp_path.display())
        })?;
        let mut timeline_writer = BufWriter::new(timeline_file);
        for sample in &self.samples {
            serde_json::to_writer(&mut timeline_writer, sample).map_err(|error| {
                format!(
                    "failed to serialize benchmark sample to {}: {error}",
                    timeline_path.display()
                )
            })?;
            timeline_writer
                .write_all(b"\n")
                .map_err(|error| format!("failed to write {}: {error}", timeline_path.display()))?;
        }
        timeline_writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", timeline_path.display()))?;
        drop(timeline_writer);
        fs::rename(&timeline_tmp_path, &timeline_path).map_err(|error| {
            format!(
                "failed to replace {} from {}: {error}",
                timeline_path.display(),
                timeline_tmp_path.display()
            )
        })?;

        let frames_path = self.config.metrics_path.join("frames.ndjson");
        let frames_tmp_path = temp_path(&frames_path);
        let frames_file = File::create(&frames_tmp_path)
            .map_err(|error| format!("failed to create {}: {error}", frames_tmp_path.display()))?;
        let mut frames_writer = BufWriter::new(frames_file);
        for frame in &self.frame_events {
            serde_json::to_writer(&mut frames_writer, frame).map_err(|error| {
                format!(
                    "failed to serialize benchmark frame to {}: {error}",
                    frames_path.display()
                )
            })?;
            frames_writer
                .write_all(b"\n")
                .map_err(|error| format!("failed to write {}: {error}", frames_path.display()))?;
        }
        frames_writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", frames_path.display()))?;
        drop(frames_writer);
        fs::rename(&frames_tmp_path, &frames_path).map_err(|error| {
            format!(
                "failed to replace {} from {}: {error}",
                frames_path.display(),
                frames_tmp_path.display()
            )
        })?;

        let summary = self.build_summary();
        let summary_path = self.config.metrics_path.join("summary.json");
        let summary_tmp_path = temp_path(&summary_path);
        let summary_file = File::create(&summary_tmp_path)
            .map_err(|error| format!("failed to create {}: {error}", summary_tmp_path.display()))?;
        let mut summary_writer = BufWriter::new(summary_file);
        serde_json::to_writer_pretty(&mut summary_writer, &summary)
            .map_err(|error| format!("failed to serialize {}: {error}", summary_path.display()))?;
        summary_writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", summary_path.display()))?;
        drop(summary_writer);
        fs::rename(&summary_tmp_path, &summary_path).map_err(|error| {
            format!(
                "failed to replace {} from {}: {error}",
                summary_path.display(),
                summary_tmp_path.display()
            )
        })?;
        Ok(())
    }

    fn build_summary(&self) -> BenchmarkSummary {
        let elapsed = duration_millis(self.start_at.elapsed());
        let cpu_avg_percent = mean_f32(self.samples.iter().map(|sample| sample.cpu_percent));
        let cpu_max_percent = self
            .samples
            .iter()
            .map(|sample| sample.cpu_percent)
            .fold(0.0, f32::max);
        let memory_max_bytes = self
            .samples
            .iter()
            .map(|sample| sample.memory_bytes)
            .max()
            .unwrap_or(0);
        let terminal_ui = terminal_ui_render_metrics_snapshot();
        let (frame_p50_ms, frame_p95_ms, frame_p99_ms) =
            frame_percentiles(&self.frame_intervals_micros);

        BenchmarkSummary {
            build_label: self.config.build_label.clone(),
            git_sha: self.config.git_sha.clone(),
            scenario: self.config.scenario.clone(),
            duration_ms: elapsed,
            sample_count: self.samples.len() as u64,
            total_frames: self.total_frames,
            fps_avg: if elapsed > 0 {
                self.total_frames as f32 / (elapsed as f32 / 1000.0)
            } else {
                0.0
            },
            frame_p50_ms,
            frame_p95_ms,
            frame_p99_ms,
            cpu_avg_percent,
            cpu_max_percent,
            memory_max_bytes,
            runtime_wakeups: terminal_ui.runtime_wakeup_count,
            view_wake_signals: self.counters.view_wake_signals,
            terminal_event_drain_passes: self.counters.terminal_event_drain_passes,
            terminal_redraws: self.counters.terminal_redraws,
            alt_screen_fallback_redraws: self.counters.alt_screen_fallback_redraws,
            grid_paint_count: terminal_ui.grid_paint_count,
            shape_line_calls: terminal_ui.shape_line_calls,
            shaped_line_cache_hits: terminal_ui.shaped_line_cache_hits,
            shaped_line_cache_misses: terminal_ui.shaped_line_cache_misses,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct BenchmarkSample {
    elapsed_ms: u64,
    fps: f32,
    frame_p50_ms: f32,
    frame_p95_ms: f32,
    frame_p99_ms: f32,
    cpu_percent: f32,
    memory_bytes: u64,
    runtime_wakeups: u64,
    view_wake_signals: u64,
    terminal_event_drain_passes: u64,
    terminal_redraws: u64,
    alt_screen_fallback_redraws: u64,
    grid_paint_count: u64,
    shape_line_calls: u64,
    shaped_line_cache_hits: u64,
    shaped_line_cache_misses: u64,
}

#[derive(Clone, Debug, Serialize)]
struct BenchmarkFrameEvent {
    monotonic_ns: u64,
    elapsed_ms: u64,
    total_frames: u64,
    terminal_redraws: u64,
    view_wake_signals: u64,
    runtime_wakeups: u64,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct BenchmarkSummary {
    pub build_label: Option<String>,
    pub git_sha: Option<String>,
    pub scenario: String,
    pub duration_ms: u64,
    pub sample_count: u64,
    pub total_frames: u64,
    pub fps_avg: f32,
    pub frame_p50_ms: f32,
    pub frame_p95_ms: f32,
    pub frame_p99_ms: f32,
    pub cpu_avg_percent: f32,
    pub cpu_max_percent: f32,
    pub memory_max_bytes: u64,
    pub runtime_wakeups: u64,
    pub view_wake_signals: u64,
    pub terminal_event_drain_passes: u64,
    pub terminal_redraws: u64,
    pub alt_screen_fallback_redraws: u64,
    pub grid_paint_count: u64,
    pub shape_line_calls: u64,
    pub shaped_line_cache_hits: u64,
    pub shaped_line_cache_misses: u64,
}

fn duration_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    millis.min(u128::from(u64::MAX)) as u64
}

fn temp_path(path: &Path) -> PathBuf {
    let mut temp = path.to_path_buf();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map_or_else(|| "tmp".to_string(), |extension| format!("{extension}.tmp"));
    temp.set_extension(extension);
    temp
}

fn percentile_millis(samples_micros: &[u32], numerator: usize, denominator: usize) -> f32 {
    let Some(last_index) = samples_micros.len().checked_sub(1) else {
        return 0.0;
    };
    let index =
        (last_index.saturating_mul(numerator) + denominator.saturating_sub(1)) / denominator;
    samples_micros[index] as f32 / 1000.0
}

fn frame_percentiles(samples_micros: &[u32]) -> (f32, f32, f32) {
    if samples_micros.is_empty() {
        return (0.0, 0.0, 0.0);
    }

    let mut sorted = samples_micros.to_vec();
    sorted.sort_unstable();
    (
        percentile_millis(&sorted, 50, 100),
        percentile_millis(&sorted, 95, 100),
        percentile_millis(&sorted, 99, 100),
    )
}

fn mean_f32(values: impl Iterator<Item = f32>) -> f32 {
    let mut sum = 0.0;
    let mut count = 0u64;
    for value in values {
        sum += value;
        count = count.saturating_add(1);
    }
    if count == 0 { 0.0 } else { sum / count as f32 }
}

#[cfg(test)]
mod tests {
    use super::BenchmarkConfig;

    #[test]
    fn benchmark_config_is_disabled_without_required_env() {
        assert_eq!(BenchmarkConfig::from_env().unwrap(), None);
    }
}
