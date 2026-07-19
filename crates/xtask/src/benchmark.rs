use anyhow::{Context, Result, bail};
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    env,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use termy_core::monotonic_now_ns;

const DEFAULT_DURATION_SECS: u64 = 13;
// Give launched apps enough room to finish the benchmark command, flush metrics,
// and quit before xctrace force-terminates them at the trace time limit.
const TRACE_PADDING_SECS: u64 = 5;
// xctrace occasionally ignores its own time limit while finalizing a trace.
// Keep a hard outer deadline so one wedged recording cannot consume the job.
const XCTRACE_FINALIZATION_GRACE_SECS: u64 = 45;
const XCTRACE_ATTEMPTS: usize = 2;
const DRIVER_START_TIMEOUT: Duration = Duration::from_secs(15);
const DRIVER_START_POLL_INTERVAL: Duration = Duration::from_millis(50);
const BENCHMARK_EVENTS_PATH_ENV: &str = "TERMY_BENCHMARK_EVENTS_PATH";
const DRIVER_START_MARKER: &str = "driver_start";
const IDLE_BURST_PRE_IDLE: Duration = Duration::from_millis(1500);
const ECHO_TRAIN_PRE_IDLE: Duration = Duration::from_millis(1500);
const ECHO_TRAIN_INTERVAL: Duration = Duration::from_millis(250);
const ECHO_TRAIN_POST_IDLE: Duration = Duration::from_millis(1000);
const ECHO_TRAIN_DEFAULT_ITERATIONS: u64 = 40;
const MIN_DISPLAYED_FRAME_SAMPLES: u64 = 100;

pub(crate) fn run(mut args: impl Iterator<Item = String>) -> Result<()> {
    let Some(command) = args.next() else {
        bail!(
            "usage: cargo run -p xtask -- <benchmark-driver|benchmark-compare|benchmark-gate> [options]"
        );
    };

    match command.as_str() {
        "benchmark-driver" => run_driver(args),
        "benchmark-compare" => run_compare(args),
        "benchmark-gate" => run_gate(args),
        other => bail!("unknown benchmark command `{other}`"),
    }
}

fn run_driver(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut scenario = None;
    let mut duration_secs = DEFAULT_DURATION_SECS;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scenario" => {
                let value = args.next().context("missing value for --scenario")?;
                scenario = Some(Scenario::parse(&value)?);
            }
            "--duration-secs" => {
                let value = args.next().context("missing value for --duration-secs")?;
                duration_secs = value
                    .parse()
                    .with_context(|| format!("invalid --duration-secs `{value}`"))?;
            }
            other => bail!(
                "unknown benchmark-driver argument `{other}`; expected --scenario or --duration-secs"
            ),
        }
    }

    let scenario = scenario.context("missing required --scenario")?;
    let mut marker_writer = BenchmarkMarkerWriter::new_from_env()?;
    marker_writer.record(DRIVER_START_MARKER, None)?;
    marker_writer.flush()?;
    scenario.run(Duration::from_secs(duration_secs))
}

fn run_compare(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut baseline_spec = None;
    let mut candidate_spec = None;
    let mut baseline_root = None;
    let mut candidate_root = None;
    let mut output_root = None;
    let mut duration_secs = DEFAULT_DURATION_SECS;
    let mut selected_scenarios = Vec::new();
    let mut collect_animation = true;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--baseline" => {
                let value = args.next().context("missing value for --baseline")?;
                baseline_spec = Some(value);
            }
            "--candidate" => {
                let value = args.next().context("missing value for --candidate")?;
                candidate_spec = Some(value);
            }
            "--baseline-root" => {
                baseline_root = Some(PathBuf::from(
                    args.next().context("missing value for --baseline-root")?,
                ));
            }
            "--candidate-root" => {
                candidate_root = Some(PathBuf::from(
                    args.next().context("missing value for --candidate-root")?,
                ));
            }
            "--output" => {
                output_root = Some(PathBuf::from(
                    args.next().context("missing value for --output")?,
                ));
            }
            "--duration-secs" => {
                let value = args.next().context("missing value for --duration-secs")?;
                duration_secs = value
                    .parse()
                    .with_context(|| format!("invalid --duration-secs `{value}`"))?;
            }
            "--scenario" => {
                let value = args.next().context("missing value for --scenario")?;
                selected_scenarios.push(Scenario::parse(&value)?);
            }
            "--skip-animation-trace" => {
                collect_animation = false;
            }
            other => bail!(
                "unknown benchmark-compare argument `{other}`; expected --baseline, --candidate, --baseline-root, --candidate-root, --output, --duration-secs, --scenario, or --skip-animation-trace"
            ),
        }
    }

    if baseline_spec.is_some() && baseline_root.is_some() {
        bail!("use either --baseline or --baseline-root, not both");
    }
    if candidate_spec.is_some() && candidate_root.is_some() {
        bail!("use either --candidate or --candidate-root, not both");
    }

    let baseline = match (baseline_spec, baseline_root) {
        (Some(spec), None) => BenchmarkTargetSpec::parse("baseline", &spec)?,
        (None, Some(root)) => BenchmarkTargetSpec::from_termy_root("baseline", root)?,
        (None, None) => bail!("missing --baseline or --baseline-root"),
        (Some(_), Some(_)) => unreachable!(),
    };
    let candidate = match (candidate_spec, candidate_root) {
        (Some(spec), None) => BenchmarkTargetSpec::parse("candidate", &spec)?,
        (None, Some(root)) => BenchmarkTargetSpec::from_termy_root("candidate", root)?,
        (None, None) => bail!("missing --candidate or --candidate-root"),
        (Some(_), Some(_)) => unreachable!(),
    };
    let output_root = output_root.context("missing --output")?;
    if output_root.exists() {
        fs::remove_dir_all(&output_root)
            .with_context(|| format!("failed to clear {}", output_root.display()))?;
    }
    fs::create_dir_all(&output_root)
        .with_context(|| format!("failed to create {}", output_root.display()))?;
    // xctrace does not preserve the caller's working directory for launched
    // applications. Keep every path forwarded through the environment
    // absolute so app metrics and marker files cannot escape the report tree.
    let output_root = canonicalize_root(output_root)?;

    let driver = BenchmarkDriverSpec::current()?;
    build_release_driver(&driver)?;
    prepare_target(&baseline)?;
    prepare_target(&candidate)?;

    let scenarios = if selected_scenarios.is_empty() {
        Scenario::all().to_vec()
    } else {
        selected_scenarios
    };
    let mut runs = Vec::with_capacity(scenarios.len() * 2);
    for build in [&baseline, &candidate] {
        for scenario in &scenarios {
            runs.push(run_single_benchmark(
                build,
                &driver,
                *scenario,
                duration_secs,
                &output_root,
                collect_animation,
            )?);
        }
    }

    let summary = ComparisonSummary::from_runs(&baseline, &candidate, &scenarios, runs)?;
    write_report_artifacts(&output_root, &summary)?;
    println!("wrote benchmark report to {}", output_root.display());
    Ok(())
}

fn run_gate(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut summary_path = None;
    let mut thresholds = BenchmarkGateThresholds::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--summary" => {
                summary_path = Some(PathBuf::from(
                    args.next().context("missing value for --summary")?,
                ));
            }
            "--max-cpu-delta-percent" => {
                thresholds.max_cpu_delta_percent = parse_gate_value(&mut args, &arg)?;
            }
            "--max-memory-delta-mib" => {
                thresholds.max_memory_delta_mib = parse_gate_value(&mut args, &arg)?;
            }
            "--max-frame-p95-delta-ms" => {
                thresholds.max_frame_p95_delta_ms = parse_gate_value(&mut args, &arg)?;
            }
            "--max-hitch-count-delta" => {
                thresholds.max_hitch_count_delta = parse_gate_value(&mut args, &arg)?;
            }
            "--max-idle-wakeup-delta" => {
                thresholds.max_idle_wakeup_delta = parse_gate_value(&mut args, &arg)?;
            }
            "--max-echo-p95-delta-ms" => {
                thresholds.max_echo_p95_delta_ms = parse_gate_value(&mut args, &arg)?;
            }
            "--max-echo-missed-delta" => {
                thresholds.max_echo_missed_delta = parse_gate_value(&mut args, &arg)?;
            }
            "--min-displayed-frames" => {
                thresholds.min_displayed_frames = parse_gate_value(&mut args, &arg)?;
            }
            other => bail!(
                "unknown benchmark-gate argument `{other}`; expected --summary or a --max-* threshold"
            ),
        }
    }

    let summary_path = summary_path.context("missing --summary")?;
    let summary: ComparisonSummary = read_json(&summary_path)?;
    let failures = thresholds.failures(&summary);
    if failures.is_empty() {
        println!(
            "benchmark gates passed for {} scenarios ({})",
            summary.scenarios.len(),
            summary_path.display()
        );
        return Ok(());
    }

    bail!("benchmark gates failed:\n- {}", failures.join("\n- "))
}

fn parse_gate_value<T: std::str::FromStr>(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    let value = args
        .next()
        .with_context(|| format!("missing value for {flag}"))?;
    value
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid {flag} value `{value}`: {error}"))
}

#[derive(Clone, Copy, Debug)]
struct BenchmarkGateThresholds {
    max_cpu_delta_percent: f32,
    max_memory_delta_mib: f32,
    max_frame_p95_delta_ms: f32,
    max_hitch_count_delta: i64,
    max_idle_wakeup_delta: i64,
    max_echo_p95_delta_ms: f32,
    max_echo_missed_delta: i64,
    min_displayed_frames: u64,
}

impl Default for BenchmarkGateThresholds {
    fn default() -> Self {
        Self {
            max_cpu_delta_percent: 15.0,
            max_memory_delta_mib: 20.0,
            max_frame_p95_delta_ms: 8.0,
            max_hitch_count_delta: 3,
            max_idle_wakeup_delta: 75,
            max_echo_p95_delta_ms: 10.0,
            max_echo_missed_delta: 0,
            min_displayed_frames: MIN_DISPLAYED_FRAME_SAMPLES,
        }
    }
}

impl BenchmarkGateThresholds {
    fn failures(self, summary: &ComparisonSummary) -> Vec<String> {
        let mut failures = Vec::new();
        let max_memory_delta_bytes = (self.max_memory_delta_mib * 1024.0 * 1024.0) as i64;

        if summary.scenarios.is_empty() {
            failures.push("benchmark summary contains no scenarios".to_string());
        }

        for scenario in &summary.scenarios {
            if self.min_displayed_frames > 0 {
                for (label, run) in [
                    ("baseline", &scenario.baseline),
                    ("candidate", &scenario.candidate),
                ] {
                    if let Some(animation) = run.animation_summary.as_ref()
                        && matches!(
                            animation.displayed_frame_capture_status,
                            FrameCaptureStatus::ParserError
                        )
                    {
                        failures.push(format!(
                            "{}: {label} displayed-frame trace parser failed{}",
                            scenario.scenario,
                            animation
                                .displayed_frame_capture_detail
                                .as_ref()
                                .map(|detail| format!(": {detail}"))
                                .unwrap_or_default()
                        ));
                    }
                }
            }
            let minimum_displayed_frames = if self.min_displayed_frames == 0 {
                0
            } else {
                match scenario.scenario.as_str() {
                    "steady-scroll" | "alt-screen-anim" => self.min_displayed_frames,
                    // These scenarios actively produce terminal output, but may
                    // legitimately render fewer than the cadence sample floor.
                    // Still require proof that both targets displayed something.
                    "idle-burst" | "echo-train" => 1,
                    _ => 0,
                }
            };
            if minimum_displayed_frames > 0 {
                for (label, run) in [
                    ("baseline", &scenario.baseline),
                    ("candidate", &scenario.candidate),
                ] {
                    let count = run
                        .animation_summary
                        .as_ref()
                        .map_or(0, |animation| animation.displayed_frame_count);
                    if count < minimum_displayed_frames {
                        failures.push(format!(
                            "{}: {label} displayed frame count {count} is below minimum {}",
                            scenario.scenario, minimum_displayed_frames
                        ));
                    }
                }
            }
            check_required_f32(
                &mut failures,
                &scenario.scenario,
                "activity-monitor CPU delta",
                scenario.deltas.activity_monitor_cpu_percent,
                self.max_cpu_delta_percent,
                "%",
            );
            check_required_i64(
                &mut failures,
                &scenario.scenario,
                "memory delta",
                scenario.deltas.memory_bytes,
                max_memory_delta_bytes,
                "bytes",
            );
            if scenario.scenario != "idle-blink" {
                check_optional_f32(
                    &mut failures,
                    &scenario.scenario,
                    "frame p95 delta",
                    scenario.deltas.frame_p95_ms,
                    self.max_frame_p95_delta_ms,
                    "ms",
                );
            }
            if self.min_displayed_frames == 0 {
                check_optional_i64(
                    &mut failures,
                    &scenario.scenario,
                    "hitch count delta",
                    scenario.deltas.hitch_count,
                    self.max_hitch_count_delta,
                    "hitches",
                );
            } else {
                check_required_i64(
                    &mut failures,
                    &scenario.scenario,
                    "hitch count delta",
                    scenario.deltas.hitch_count,
                    self.max_hitch_count_delta,
                    "hitches",
                );
            }
            if matches!(scenario.scenario.as_str(), "idle-blink" | "idle-burst") {
                check_required_i64(
                    &mut failures,
                    &scenario.scenario,
                    "idle wakeup delta",
                    scenario.deltas.idle_wakeups,
                    self.max_idle_wakeup_delta,
                    "wakeups",
                );
            }
            check_optional_f32(
                &mut failures,
                &scenario.scenario,
                "echo first-frame p95 delta",
                scenario.deltas.echo_first_frame_ms_p95,
                self.max_echo_p95_delta_ms,
                "ms",
            );
            check_optional_i64(
                &mut failures,
                &scenario.scenario,
                "echo missed count delta",
                scenario.deltas.echo_missed_count,
                self.max_echo_missed_delta,
                "misses",
            );
        }

        failures
    }
}

fn check_required_f32(
    failures: &mut Vec<String>,
    scenario: &str,
    metric: &str,
    value: Option<f32>,
    max_allowed: f32,
    unit: &str,
) {
    let Some(value) = value else {
        failures.push(format!("{scenario}: required {metric} is missing"));
        return;
    };
    if value > max_allowed {
        failures.push(format!(
            "{scenario}: {metric} {value:.2}{unit} exceeded {max_allowed:.2}{unit}"
        ));
    }
}

fn check_required_i64(
    failures: &mut Vec<String>,
    scenario: &str,
    metric: &str,
    value: Option<i64>,
    max_allowed: i64,
    unit: &str,
) {
    let Some(value) = value else {
        failures.push(format!("{scenario}: required {metric} is missing"));
        return;
    };
    if value > max_allowed {
        failures.push(format!(
            "{scenario}: {metric} {value}{unit} exceeded {max_allowed}{unit}"
        ));
    }
}

fn check_optional_f32(
    failures: &mut Vec<String>,
    scenario: &str,
    metric: &str,
    value: Option<f32>,
    max_allowed: f32,
    unit: &str,
) {
    let Some(value) = value else {
        return;
    };
    if value > max_allowed {
        failures.push(format!(
            "{scenario}: {metric} {value:.2}{unit} exceeded {max_allowed:.2}{unit}"
        ));
    }
}

fn check_optional_i64(
    failures: &mut Vec<String>,
    scenario: &str,
    metric: &str,
    value: Option<i64>,
    max_allowed: i64,
    unit: &str,
) {
    let Some(value) = value else {
        return;
    };
    if value > max_allowed {
        failures.push(format!(
            "{scenario}: {metric} {value}{unit} exceeded {max_allowed}{unit}"
        ));
    }
}

fn canonicalize_root(path: PathBuf) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))
}

#[derive(Clone, Debug)]
struct BenchmarkDriverSpec {
    root: PathBuf,
    xtask_binary: PathBuf,
}

impl BenchmarkDriverSpec {
    fn current() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .context("failed to compute xtask workspace root")?;
        Ok(Self {
            xtask_binary: root.join("target/release/xtask"),
            root,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BenchmarkTargetKind {
    Termy,
    Native,
    Ghostty,
}

impl BenchmarkTargetKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "termy" => Ok(Self::Termy),
            "native" => Ok(Self::Native),
            "ghostty" => Ok(Self::Ghostty),
            other => bail!("unknown benchmark target kind `{other}`"),
        }
    }

    fn display_name(self) -> &'static str {
        match self {
            Self::Termy => "Termy",
            Self::Native => "Termy Native",
            Self::Ghostty => "Ghostty",
        }
    }
}

#[derive(Clone, Debug)]
struct BenchmarkTargetSpec {
    label: &'static str,
    kind: BenchmarkTargetKind,
    source_path: PathBuf,
    executable_path: PathBuf,
    git_sha: Option<String>,
}

impl BenchmarkTargetSpec {
    fn parse(label: &'static str, value: &str) -> Result<Self> {
        let (kind, path) = value
            .split_once(':')
            .with_context(|| format!("invalid target spec `{value}`; expected kind:/path"))?;
        let kind = BenchmarkTargetKind::parse(kind)?;
        match kind {
            BenchmarkTargetKind::Termy => Self::from_termy_root(label, PathBuf::from(path)),
            BenchmarkTargetKind::Native => Self::from_native_path(label, PathBuf::from(path)),
            BenchmarkTargetKind::Ghostty => Self::from_ghostty_path(label, PathBuf::from(path)),
        }
    }

    fn from_termy_root(label: &'static str, root: PathBuf) -> Result<Self> {
        let root = canonicalize_root(root)?;
        Ok(Self {
            label,
            kind: BenchmarkTargetKind::Termy,
            executable_path: root.join("target/release/termy"),
            git_sha: Some(git_rev_parse_short(&root)?),
            source_path: root,
        })
    }

    fn from_ghostty_path(label: &'static str, path: PathBuf) -> Result<Self> {
        let source_path = canonicalize_root(path)?;
        let executable_path = resolve_ghostty_executable(&source_path)?;
        Ok(Self {
            label,
            kind: BenchmarkTargetKind::Ghostty,
            source_path,
            executable_path,
            git_sha: None,
        })
    }

    fn from_native_path(label: &'static str, path: PathBuf) -> Result<Self> {
        let source_path = canonicalize_root(path)?;
        let executable_path = resolve_native_executable(&source_path)?;
        Ok(Self {
            label,
            kind: BenchmarkTargetKind::Native,
            source_path,
            executable_path,
            git_sha: None,
        })
    }

    fn display_name(&self) -> &'static str {
        self.kind.display_name()
    }

    fn metrics_supported(&self) -> bool {
        matches!(self.kind, BenchmarkTargetKind::Termy)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct GhosttyVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl GhosttyVersion {
    const MIN_SUPPORTED: Self = Self {
        major: 1,
        minor: 2,
        patch: 0,
    };
}

#[derive(Clone, Debug)]
struct GhosttyLaunchArtifacts {
    config_path: PathBuf,
}

fn git_rev_parse_short(root: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--short")
        .arg("HEAD")
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run git rev-parse in {}", root.display()))?;
    if !output.status.success() {
        bail!(
            "git rev-parse failed in {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn resolve_ghostty_executable(path: &Path) -> Result<PathBuf> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        let executable = path.join("Contents/MacOS/ghostty");
        if !executable.is_file() {
            bail!(
                "failed to locate Ghostty executable at {}",
                executable.display()
            );
        }
        return Ok(executable);
    }

    if !path.is_file() {
        bail!(
            "ghostty target must point to an executable or .app bundle: {}",
            path.display()
        );
    }
    Ok(path.to_path_buf())
}

fn resolve_native_executable(path: &Path) -> Result<PathBuf> {
    let executable = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
    {
        path.join("Contents/MacOS/Termy")
    } else {
        path.to_path_buf()
    };

    if !executable.is_file() {
        bail!(
            "native target must point to the Termy executable or .app bundle: {}",
            executable.display()
        );
    }
    Ok(executable)
}

fn ghostty_version(executable_path: &Path) -> Result<GhosttyVersion> {
    let output = Command::new(executable_path)
        .arg("+version")
        .output()
        .with_context(|| format!("failed to launch Ghostty at {}", executable_path.display()))?;
    if !output.status.success() {
        bail!(
            "Ghostty version probe failed for {}: {}",
            executable_path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut version_output = String::from_utf8_lossy(&output.stdout).to_string();
    if version_output.trim().is_empty() {
        version_output = String::from_utf8_lossy(&output.stderr).to_string();
    }
    parse_ghostty_version(&version_output).with_context(|| {
        format!(
            "failed to parse Ghostty version from `{}`",
            version_output.trim()
        )
    })
}

fn parse_ghostty_version(output: &str) -> Option<GhosttyVersion> {
    output
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '-'))
        .find_map(parse_ghostty_version_token)
}

fn parse_ghostty_version_token(token: &str) -> Option<GhosttyVersion> {
    let token = token.trim_start_matches('v');
    let token = token.split('-').next().unwrap_or(token);
    let mut parts = token.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    Some(GhosttyVersion {
        major,
        minor,
        patch,
    })
}

fn build_release_driver(driver: &BenchmarkDriverSpec) -> Result<()> {
    run_command(
        Command::new("cargo")
            .arg("build")
            .arg("--release")
            .arg("-p")
            .arg("xtask")
            .current_dir(&driver.root),
        format!(
            "cargo build --release -p xtask in {}",
            driver.root.display()
        ),
    )
}

fn prepare_target(build: &BenchmarkTargetSpec) -> Result<()> {
    match build.kind {
        BenchmarkTargetKind::Termy => run_command(
            Command::new("cargo")
                .arg("build")
                .arg("--release")
                .arg("-p")
                .arg("termy")
                .current_dir(&build.source_path),
            format!(
                "cargo build --release -p termy in {}",
                build.source_path.display()
            ),
        ),
        BenchmarkTargetKind::Native => Ok(()),
        BenchmarkTargetKind::Ghostty => {
            let version = ghostty_version(&build.executable_path)?;
            if version < GhosttyVersion::MIN_SUPPORTED {
                bail!(
                    "Ghostty {}.{}.{} is unsupported for benchmark mode; require >= {}.{}.{} because the harness uses `initial-command = direct:...`",
                    version.major,
                    version.minor,
                    version.patch,
                    GhosttyVersion::MIN_SUPPORTED.major,
                    GhosttyVersion::MIN_SUPPORTED.minor,
                    GhosttyVersion::MIN_SUPPORTED.patch,
                );
            }
            Ok(())
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Scenario {
    IdleBlink,
    IdleBurst,
    EchoTrain,
    SteadyScroll,
    AltScreenAnim,
}

impl Scenario {
    fn all() -> &'static [Scenario] {
        &[
            Scenario::IdleBlink,
            Scenario::IdleBurst,
            Scenario::EchoTrain,
            Scenario::SteadyScroll,
            Scenario::AltScreenAnim,
        ]
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "idle-blink" => Ok(Self::IdleBlink),
            "idle-burst" => Ok(Self::IdleBurst),
            "echo-train" => Ok(Self::EchoTrain),
            "steady-scroll" => Ok(Self::SteadyScroll),
            "alt-screen-anim" => Ok(Self::AltScreenAnim),
            other => bail!("unknown benchmark scenario `{other}`"),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::IdleBlink => "idle-blink",
            Self::IdleBurst => "idle-burst",
            Self::EchoTrain => "echo-train",
            Self::SteadyScroll => "steady-scroll",
            Self::AltScreenAnim => "alt-screen-anim",
        }
    }

    fn run(self, duration: Duration) -> Result<()> {
        match self {
            Self::IdleBlink => {
                thread::sleep(duration);
                Ok(())
            }
            Self::IdleBurst => run_idle_burst(duration),
            Self::EchoTrain => run_echo_train(duration),
            Self::SteadyScroll => run_steady_scroll(duration),
            Self::AltScreenAnim => run_alt_screen_anim(duration),
        }
    }
}

fn run_idle_burst(duration: Duration) -> Result<()> {
    let start = Instant::now();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut marker_writer = BenchmarkMarkerWriter::new_from_env()?;

    sleep_for_remaining(start, duration, IDLE_BURST_PRE_IDLE);
    marker_writer.record("burst_start", None)?;

    let mut burst = String::new();
    for line in 0..16u64 {
        burst.push_str(&format!(
            "burst line {line:02} 0123456789 abcdefghijklmnopqrstuvwxyz\n"
        ));
    }
    out.write_all(burst.as_bytes())?;
    out.flush()?;
    marker_writer.record("burst_end", None)?;

    thread::sleep(duration.saturating_sub(start.elapsed()));
    marker_writer.flush()?;
    Ok(())
}

fn run_echo_train(duration: Duration) -> Result<()> {
    let start = Instant::now();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut marker_writer = BenchmarkMarkerWriter::new_from_env()?;

    sleep_for_remaining(start, duration, ECHO_TRAIN_PRE_IDLE);

    let remaining = duration.saturating_sub(start.elapsed());
    let reserved_post_idle = ECHO_TRAIN_POST_IDLE.min(remaining);
    let emit_budget = remaining.saturating_sub(reserved_post_idle);
    let budget_iterations = (emit_budget.as_millis() / ECHO_TRAIN_INTERVAL.as_millis()) as u64;
    let iterations = budget_iterations.clamp(1, ECHO_TRAIN_DEFAULT_ITERATIONS);
    let glyphs = b"abcdefghijklmnopqrstuvwxyz0123456789";

    for seq in 0..iterations {
        marker_writer.record("echo_start", Some(seq))?;
        let glyph = glyphs[(seq as usize) % glyphs.len()] as char;
        write!(out, "{glyph}")?;
        out.flush()?;
        if seq + 1 != iterations {
            thread::sleep(ECHO_TRAIN_INTERVAL);
        }
    }
    writeln!(out)?;
    out.flush()?;

    thread::sleep(duration.saturating_sub(start.elapsed()));
    marker_writer.flush()?;
    Ok(())
}

fn run_steady_scroll(duration: Duration) -> Result<()> {
    let start = Instant::now();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut line = 0u64;
    while start.elapsed() < duration {
        writeln!(
            out,
            "scroll {line:06}  the quick brown fox jumps over the lazy dog 0123456789"
        )?;
        out.flush()?;
        line = line.saturating_add(1);
        thread::sleep(Duration::from_millis(6));
    }
    Ok(())
}

fn sleep_for_remaining(start: Instant, total_duration: Duration, requested_sleep: Duration) {
    let remaining = total_duration.saturating_sub(start.elapsed());
    thread::sleep(requested_sleep.min(remaining));
}

struct BenchmarkMarkerWriter {
    writer: Option<io::BufWriter<fs::File>>,
}

impl BenchmarkMarkerWriter {
    fn new_from_env() -> Result<Self> {
        let Some(path) = env::var_os(BENCHMARK_EVENTS_PATH_ENV) else {
            return Ok(Self { writer: None });
        };
        let path = PathBuf::from(path);
        let parent = path
            .parent()
            .context("benchmark events path is missing a parent directory")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        Ok(Self {
            writer: Some(io::BufWriter::new(file)),
        })
    }

    fn record(&mut self, kind: &str, seq: Option<u64>) -> Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        let marker = MarkerEvent {
            kind: kind.to_string(),
            seq,
            monotonic_ns: monotonic_now_ns(),
        };
        serde_json::to_writer(&mut *writer, &marker)
            .context("failed to serialize benchmark marker")?;
        writer
            .write_all(b"\n")
            .context("failed to write benchmark marker newline")?;
        writer.flush().context("failed to flush benchmark marker")?;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(());
        };
        writer.flush().context("failed to flush benchmark marker")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MarkerEvent {
    kind: String,
    seq: Option<u64>,
    monotonic_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FrameEvent {
    monotonic_ns: u64,
    elapsed_ms: u64,
    total_frames: u64,
    terminal_redraws: u64,
    view_wake_signals: u64,
    runtime_wakeups: u64,
}

fn run_alt_screen_anim(duration: Duration) -> Result<()> {
    let start = Instant::now();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    write!(out, "\x1b[?1049h\x1b[?25l")?;
    out.flush()?;

    let rows = 24usize;
    let cols = 80usize;
    let mut frame = 0usize;
    while start.elapsed() < duration {
        write!(out, "\x1b[H")?;
        for row in 0..rows {
            let band = (row + frame) % cols;
            let mut line = String::with_capacity(cols);
            for col in 0..cols {
                let ch = if col == band || col == (band + 1) % cols {
                    '#'
                } else if (col + row + frame).is_multiple_of(7) {
                    '.'
                } else {
                    ' '
                };
                line.push(ch);
            }
            writeln!(out, "{line}")?;
        }
        write!(
            out,
            "\x1b[2;2Hframe {:05} elapsed {:.2}s\x1b[{};{}H",
            frame,
            start.elapsed().as_secs_f32(),
            (frame % rows) + 1,
            ((frame * 3) % cols) + 1
        )?;
        out.flush()?;
        frame = frame.saturating_add(1);
        thread::sleep(Duration::from_millis(16));
    }

    write!(out, "\x1b[?25h\x1b[?1049l")?;
    out.flush()?;
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AppSummary {
    build_label: Option<String>,
    git_sha: Option<String>,
    scenario: String,
    duration_ms: u64,
    sample_count: u64,
    total_frames: u64,
    fps_avg: f32,
    frame_p50_ms: f32,
    frame_p95_ms: f32,
    frame_p99_ms: f32,
    cpu_avg_percent: f32,
    cpu_max_percent: f32,
    memory_max_bytes: u64,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EnergySummary {
    trace_template: String,
    cpu_total_ns: Option<u64>,
    cpu_percent: Option<f32>,
    idle_wakeups: Option<u64>,
    memory_bytes: Option<u64>,
    disk_bytes_read: Option<u64>,
    disk_bytes_written: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FrameCaptureStatus {
    Parsed,
    #[default]
    NoFrames,
    ParserError,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AnimationSummary {
    trace_template: String,
    launched_pid: Option<u32>,
    displayed_frame_capture_status: FrameCaptureStatus,
    displayed_frame_capture_detail: Option<String>,
    displayed_frame_count: u64,
    fps_avg: Option<f32>,
    frame_p50_ms: Option<f32>,
    frame_p95_ms: Option<f32>,
    frame_p99_ms: Option<f32>,
    hitch_count: u64,
    hitch_max_ms: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RunResult {
    build_label: String,
    target_name: String,
    git_sha: Option<String>,
    scenario: String,
    app_summary: Option<AppSummary>,
    energy_summary: EnergySummary,
    animation_summary: Option<AnimationSummary>,
    micro_latency: MicroLatencySummary,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct MicroLatencySummary {
    idle_burst: Option<IdleBurstLatencySummary>,
    echo_train: Option<EchoTrainLatencySummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdleBurstLatencySummary {
    first_frame_after_burst_ms: Option<f32>,
    last_frame_after_burst_ms: Option<f32>,
    frames_until_settle: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct EchoTrainLatencySummary {
    echo_first_frame_ms_p50: Option<f32>,
    echo_first_frame_ms_p95: Option<f32>,
    echo_first_frame_ms_p99: Option<f32>,
    echo_first_frame_ms_max: Option<f32>,
    echo_missed_count: u64,
    echo_sample_count: u64,
}

fn run_single_benchmark(
    build: &BenchmarkTargetSpec,
    driver: &BenchmarkDriverSpec,
    scenario: Scenario,
    duration_secs: u64,
    output_root: &Path,
    collect_animation: bool,
) -> Result<RunResult> {
    let raw_dir = output_root
        .join("raw")
        .join(build.label)
        .join(scenario.as_str());
    let energy_dir = output_root
        .join("energy")
        .join(build.label)
        .join(scenario.as_str());
    let animation_dir = output_root
        .join("animation")
        .join(build.label)
        .join(scenario.as_str());
    let config_root = raw_dir.join("config");
    let metrics_dir = raw_dir.join("app");
    let driver_dir = raw_dir.join("driver");
    if raw_dir.exists() {
        fs::remove_dir_all(&raw_dir)
            .with_context(|| format!("failed to clear {}", raw_dir.display()))?;
    }
    if energy_dir.exists() {
        fs::remove_dir_all(&energy_dir)
            .with_context(|| format!("failed to clear {}", energy_dir.display()))?;
    }
    if animation_dir.exists() {
        fs::remove_dir_all(&animation_dir)
            .with_context(|| format!("failed to clear {}", animation_dir.display()))?;
    }
    fs::create_dir_all(config_root.join("termy"))
        .with_context(|| format!("failed to create {}", config_root.display()))?;
    fs::create_dir_all(&metrics_dir)
        .with_context(|| format!("failed to create {}", metrics_dir.display()))?;
    fs::create_dir_all(&driver_dir)
        .with_context(|| format!("failed to create {}", driver_dir.display()))?;
    fs::create_dir_all(&energy_dir)
        .with_context(|| format!("failed to create {}", energy_dir.display()))?;
    fs::create_dir_all(&animation_dir)
        .with_context(|| format!("failed to create {}", animation_dir.display()))?;

    let config_path = config_root.join("termy/config.txt");
    fs::write(&config_path, benchmark_config_contents(scenario))
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    let ghostty_launch = if matches!(build.kind, BenchmarkTargetKind::Ghostty) {
        Some(create_ghostty_launch_artifacts(
            &config_root,
            driver,
            scenario,
            duration_secs,
        )?)
    } else {
        None
    };

    let trace_path = energy_dir.join("activity-monitor.trace");
    let activity_markers_path = driver_dir.join("activity-markers.ndjson");
    let animation_markers_path = driver_dir.join("animation-markers.ndjson");
    let time_limit_secs = duration_secs.saturating_add(TRACE_PADDING_SECS);
    let _activity_pid = match build.kind {
        BenchmarkTargetKind::Termy | BenchmarkTargetKind::Native => {
            let command = benchmark_driver_command(driver, scenario, duration_secs);
            run_attached_termy_trace(
                build,
                "Activity Monitor",
                &trace_path,
                &config_root,
                &metrics_dir,
                &activity_markers_path,
                scenario,
                &command,
                duration_secs,
                time_limit_secs,
                &raw_dir.join("activity-target.log"),
            )?
        }
        BenchmarkTargetKind::Ghostty => {
            let mut activity_command = activity_monitor_ghostty_command(
                build,
                &trace_path,
                &activity_markers_path,
                ghostty_launch
                    .as_ref()
                    .expect("ghostty launch artifacts must exist"),
                time_limit_secs,
            );
            run_xctrace_record_command(
                &mut activity_command,
                format!(
                    "xctrace benchmark run for {} ({}) {}",
                    build.label,
                    build.display_name(),
                    scenario.as_str()
                ),
                &trace_path,
                xctrace_timeout(time_limit_secs),
            )?;
            0
        }
    };

    let app_summary = if build.metrics_supported() {
        let summary_path = metrics_dir.join("summary.json");
        Some(read_json(&summary_path)?)
    } else {
        None
    };
    let micro_latency = if build.metrics_supported() {
        let frames_path = metrics_dir.join("frames.ndjson");
        summarize_micro_latency(scenario, &activity_markers_path, &frames_path)?
    } else {
        MicroLatencySummary::default()
    };

    let toc_path = energy_dir.join("toc.xml");
    let live_path = energy_dir.join("activity-monitor-process-live.xml");
    let ledger_path = energy_dir.join("activity-monitor-process-ledger.xml");
    export_xctrace_table(&trace_path, None, &toc_path)?;
    export_xctrace_table(
        &trace_path,
        Some("/trace-toc/run[@number=\"1\"]/data/table[@schema=\"activity-monitor-process-live\"]"),
        &live_path,
    )?;
    export_xctrace_table(
        &trace_path,
        Some(
            "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"activity-monitor-process-ledger\"]",
        ),
        &ledger_path,
    )?;

    let energy_summary = parse_activity_monitor_summary(&live_path, &ledger_path)?;
    let energy_json_path = energy_dir.join("energy.json");
    write_json(&energy_json_path, &energy_summary)?;

    let animation_summary = if collect_animation {
        let animation_trace_path = animation_dir.join("animation-hitches.trace");
        let animation_metrics_dir = raw_dir.join("animation-app");
        let attached_animation_pid = match build.kind {
            BenchmarkTargetKind::Termy | BenchmarkTargetKind::Native => {
                let command = benchmark_driver_command(driver, scenario, duration_secs);
                run_attached_termy_trace(
                    build,
                    "Animation Hitches",
                    &animation_trace_path,
                    &config_root,
                    &animation_metrics_dir,
                    &animation_markers_path,
                    scenario,
                    &command,
                    duration_secs,
                    time_limit_secs,
                    &raw_dir.join("animation-target.log"),
                )?
            }
            BenchmarkTargetKind::Ghostty => {
                let mut animation_command = animation_hitches_ghostty_command(
                    build,
                    &animation_trace_path,
                    &animation_markers_path,
                    ghostty_launch
                        .as_ref()
                        .expect("ghostty launch artifacts must exist"),
                    time_limit_secs,
                );
                run_xctrace_record_command(
                    &mut animation_command,
                    format!(
                        "xctrace Animation Hitches run for {} ({}) {}",
                        build.label,
                        build.display_name(),
                        scenario.as_str()
                    ),
                    &animation_trace_path,
                    xctrace_timeout(time_limit_secs),
                )?;
                0
            }
        };

        let animation_toc_path = animation_dir.join("toc.xml");
        export_xctrace_table(&animation_trace_path, None, &animation_toc_path)?;
        let launched_pid = if attached_animation_pid > 0 {
            attached_animation_pid
        } else {
            parse_trace_launched_process_pid(&animation_toc_path)?
        };
        let displayed_frames_path = animation_dir.join("displayed-surfaces-interval.xml");
        let hitches_path = animation_dir.join("hitches.xml");
        export_xctrace_table(
            &animation_trace_path,
            Some(
                "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"displayed-surfaces-interval\"]",
            ),
            &displayed_frames_path,
        )?;
        export_xctrace_table(
            &animation_trace_path,
            Some("/trace-toc/run[@number=\"1\"]/data/table[@schema=\"hitches\"]"),
            &hitches_path,
        )?;
        let summary = parse_animation_summary(&displayed_frames_path, &hitches_path, launched_pid)?;
        write_json(&animation_dir.join("animation-summary.json"), &summary)?;
        Some(summary)
    } else {
        None
    };

    Ok(RunResult {
        build_label: build.label.to_string(),
        target_name: build.display_name().to_string(),
        git_sha: build.git_sha.clone(),
        scenario: scenario.as_str().to_string(),
        app_summary,
        energy_summary,
        animation_summary,
        micro_latency,
    })
}

fn benchmark_config_contents(scenario: Scenario) -> String {
    let cursor_blink = matches!(scenario, Scenario::IdleBlink);
    format!(
        "tmux_enabled = false\nbackground_blur = false\nbackground_opacity = 1.0\ncursor_blink = {cursor_blink}\nwindow_width = 1280\nwindow_height = 820\nshow_debug_overlay = false\n"
    )
}

fn benchmark_driver_command(
    driver: &BenchmarkDriverSpec,
    scenario: Scenario,
    duration_secs: u64,
) -> String {
    format!(
        "{} benchmark-driver --scenario {} --duration-secs {}",
        shell_escape_path(&driver.xtask_binary),
        scenario.as_str(),
        duration_secs
    )
}

fn create_ghostty_launch_artifacts(
    config_root: &Path,
    driver: &BenchmarkDriverSpec,
    scenario: Scenario,
    duration_secs: u64,
) -> Result<GhosttyLaunchArtifacts> {
    let ghostty_dir = config_root.join("ghostty");
    fs::create_dir_all(&ghostty_dir)
        .with_context(|| format!("failed to create {}", ghostty_dir.display()))?;

    let script_path = env::temp_dir().join(format!(
        "termy-ghostty-benchmark-{}-{}.sh",
        scenario.as_str(),
        monotonic_now_ns()
    ));
    let script_contents = format!(
        "#!/bin/sh\nexec {} benchmark-driver --scenario {} --duration-secs {}\n",
        shell_escape_path(&driver.xtask_binary),
        scenario.as_str(),
        duration_secs
    );
    fs::write(&script_path, script_contents)
        .with_context(|| format!("failed to write {}", script_path.display()))?;
    #[cfg(unix)]
    {
        let permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&script_path, permissions)
            .with_context(|| format!("failed to chmod {}", script_path.display()))?;
    }

    let config_path = ghostty_dir.join("config");
    let config_contents = format!(
        "window-width = 128\nwindow-height = 48\nquit-after-last-window-closed = true\ninitial-command = direct:{}\n",
        script_path.display()
    );
    fs::write(&config_path, config_contents)
        .with_context(|| format!("failed to write {}", config_path.display()))?;

    Ok(GhosttyLaunchArtifacts { config_path })
}

#[allow(clippy::too_many_arguments)]
fn run_attached_termy_trace(
    build: &BenchmarkTargetSpec,
    template: &str,
    trace_path: &Path,
    config_root: &Path,
    metrics_dir: &Path,
    markers_path: &Path,
    scenario: Scenario,
    benchmark_command: &str,
    duration_secs: u64,
    time_limit_secs: u64,
    target_log_path: &Path,
) -> Result<u32> {
    for attempt in 1..=XCTRACE_ATTEMPTS {
        remove_file_if_present(markers_path)?;
        remove_xctrace_output_if_present(trace_path)?;
        let mut child = spawn_termy_benchmark_target(
            build,
            config_root,
            metrics_dir,
            markers_path,
            scenario,
            benchmark_command,
            duration_secs,
            target_log_path,
        )?;
        let pid = child.id();
        if let Err(error) =
            wait_for_driver_start(&mut child, markers_path, build, scenario, target_log_path)
        {
            stop_benchmark_target(&mut child);
            return Err(error);
        }

        let mut trace_command = Command::new("xctrace");
        trace_command
            .arg("record")
            .arg("--template")
            .arg(template)
            .arg("--time-limit")
            .arg(format!("{time_limit_secs}s"))
            .arg("--output")
            .arg(trace_path)
            .arg("--attach")
            .arg(pid.to_string());

        let trace_result = run_xctrace_record_command(
            &mut trace_command,
            format!(
                "xctrace {template} attach for {} ({}) {}",
                build.label,
                build.display_name(),
                scenario.as_str()
            ),
            trace_path,
            xctrace_timeout(time_limit_secs),
        );
        stop_benchmark_target(&mut child);
        match trace_result {
            Ok(()) => return Ok(pid),
            Err(error) if attempt < XCTRACE_ATTEMPTS => {
                eprintln!(
                    "{error:#}; retrying xctrace {template} once for {} ({}) {}",
                    build.label,
                    build.display_name(),
                    scenario.as_str()
                );
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("xctrace attempt loop always returns")
}

fn remove_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn remove_xctrace_output_if_present(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    } else {
        remove_file_if_present(path)?;
    }
    Ok(())
}

fn wait_for_driver_start(
    child: &mut Child,
    markers_path: &Path,
    build: &BenchmarkTargetSpec,
    scenario: Scenario,
    target_log_path: &Path,
) -> Result<()> {
    let deadline = Instant::now() + DRIVER_START_TIMEOUT;
    loop {
        if marker_file_contains(markers_path, DRIVER_START_MARKER)? {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .context("failed to poll benchmark target")?
        {
            bail!(
                "{} ({}) exited with {status} before the {} driver started; see {} and {}",
                build.label,
                build.display_name(),
                scenario.as_str(),
                target_log_path.display(),
                markers_path.display()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for the {} ({}) {} driver to start; see {} and {}",
                build.label,
                build.display_name(),
                scenario.as_str(),
                target_log_path.display(),
                markers_path.display()
            );
        }
        thread::sleep(DRIVER_START_POLL_INTERVAL);
    }
}

fn marker_file_contains(path: &Path, kind: &str) -> Result<bool> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    Ok(contents.lines().any(|line| {
        serde_json::from_str::<MarkerEvent>(line).is_ok_and(|marker| marker.kind == kind)
    }))
}

#[allow(clippy::too_many_arguments)]
fn spawn_termy_benchmark_target(
    build: &BenchmarkTargetSpec,
    config_root: &Path,
    metrics_dir: &Path,
    markers_path: &Path,
    scenario: Scenario,
    benchmark_command: &str,
    duration_secs: u64,
    target_log_path: &Path,
) -> Result<Child> {
    let log = fs::File::create(target_log_path)
        .with_context(|| format!("failed to create {}", target_log_path.display()))?;
    let error_log = log
        .try_clone()
        .with_context(|| format!("failed to clone {}", target_log_path.display()))?;
    Command::new(&build.executable_path)
        .env("XDG_CONFIG_HOME", config_root)
        .env("TERMY_BENCHMARK_COMMAND", benchmark_command)
        .env("TERMY_BENCHMARK_SCENARIO", scenario.as_str())
        .env("TERMY_BENCHMARK_METRICS_PATH", metrics_dir)
        .env("TERMY_BENCHMARK_DURATION_SECS", duration_secs.to_string())
        .env(BENCHMARK_EVENTS_PATH_ENV, markers_path)
        .env("TERMY_BENCHMARK_EXIT_ON_COMPLETE", "1")
        .env("TERMY_BENCHMARK_BUILD_LABEL", build.label)
        .env(
            "TERMY_BENCHMARK_GIT_SHA",
            build.git_sha.as_deref().unwrap_or("unknown"),
        )
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log))
        .spawn()
        .with_context(|| {
            format!(
                "failed to launch {} benchmark target at {}",
                build.display_name(),
                build.executable_path.display()
            )
        })
}

fn stop_benchmark_target(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn activity_monitor_ghostty_command(
    build: &BenchmarkTargetSpec,
    trace_path: &Path,
    markers_path: &Path,
    launch: &GhosttyLaunchArtifacts,
    time_limit_secs: u64,
) -> Command {
    let mut command = Command::new("xctrace");
    command
        .arg("record")
        .arg("--template")
        .arg("Activity Monitor")
        .arg("--time-limit")
        .arg(format!("{time_limit_secs}s"))
        .arg("--output")
        .arg(trace_path)
        .arg("--env")
        .arg(format!(
            "{BENCHMARK_EVENTS_PATH_ENV}={}",
            markers_path.display()
        ))
        .arg("--launch")
        .arg("--")
        .arg(&build.executable_path)
        .arg("--config-default-files=false")
        .arg(format!("--config-file={}", launch.config_path.display()));
    command
}

fn animation_hitches_ghostty_command(
    build: &BenchmarkTargetSpec,
    trace_path: &Path,
    markers_path: &Path,
    launch: &GhosttyLaunchArtifacts,
    time_limit_secs: u64,
) -> Command {
    let mut command = Command::new("xctrace");
    command
        .arg("record")
        .arg("--template")
        .arg("Animation Hitches")
        .arg("--time-limit")
        .arg(format!("{time_limit_secs}s"))
        .arg("--output")
        .arg(trace_path)
        .arg("--env")
        .arg(format!(
            "{BENCHMARK_EVENTS_PATH_ENV}={}",
            markers_path.display()
        ))
        .arg("--launch")
        .arg("--")
        .arg(&build.executable_path)
        .arg("--config-default-files=false")
        .arg(format!("--config-file={}", launch.config_path.display()));
    command
}

fn export_xctrace_table(trace_path: &Path, xpath: Option<&str>, output_path: &Path) -> Result<()> {
    let mut command = Command::new("xctrace");
    command.arg("export").arg("--input").arg(trace_path);
    if let Some(xpath) = xpath {
        command.arg("--xpath").arg(xpath);
    } else {
        command.arg("--toc");
    }
    command.arg("--output").arg(output_path);
    run_command(
        &mut command,
        format!("xctrace export {}", output_path.display()),
    )
}

fn parse_activity_monitor_summary(live_path: &Path, ledger_path: &Path) -> Result<EnergySummary> {
    let live_xml = fs::read_to_string(live_path)
        .with_context(|| format!("failed to read {}", live_path.display()))?;
    let ledger_xml = fs::read_to_string(ledger_path)
        .with_context(|| format!("failed to read {}", ledger_path.display()))?;

    let live_rows = parse_table_rows(&live_xml)?;
    let ledger_row = parse_single_row_table(&ledger_xml)?;

    let mut weighted_cpu = 0.0f64;
    let mut cpu_duration = 0u64;
    let mut memory_bytes = None;
    for row in &live_rows {
        if let (Some(cpu), Some(duration)) = (
            row.get("cpu-percent")
                .and_then(|value| value.parse::<f64>().ok()),
            row.get("duration")
                .and_then(|value| value.parse::<u64>().ok()),
        ) {
            weighted_cpu += cpu * duration as f64;
            cpu_duration = cpu_duration.saturating_add(duration);
        }
        if let Some(memory) = row
            .get("memory-physical-footprint")
            .and_then(|value| value.parse::<u64>().ok())
        {
            memory_bytes = Some(memory_bytes.map_or(memory, |current: u64| current.max(memory)));
        }
    }

    Ok(EnergySummary {
        trace_template: "Activity Monitor".to_string(),
        cpu_total_ns: ledger_row
            .get("cpu-total")
            .and_then(|value| value.parse::<u64>().ok()),
        cpu_percent: (cpu_duration > 0).then(|| (weighted_cpu / cpu_duration as f64) as f32),
        idle_wakeups: ledger_row
            .get("idle-wakeups")
            .and_then(|value| value.parse::<u64>().ok()),
        memory_bytes,
        disk_bytes_read: ledger_row
            .get("disk-bytes-read")
            .and_then(|value| value.parse::<u64>().ok()),
        disk_bytes_written: ledger_row
            .get("disk-bytes-written")
            .and_then(|value| value.parse::<u64>().ok()),
    })
}

fn parse_trace_launched_process_pid(toc_path: &Path) -> Result<u32> {
    let toc_xml = fs::read_to_string(toc_path)
        .with_context(|| format!("failed to read {}", toc_path.display()))?;
    let doc = Document::parse(&toc_xml).context("failed to parse xctrace toc xml")?;
    let launched_process = doc
        .descendants()
        .find(|node| {
            node.has_tag_name("process")
                && node
                    .attribute("type")
                    .is_some_and(|value| value == "launched")
        })
        .context("missing launched process in trace toc")?;
    let pid = launched_process
        .attribute("pid")
        .context("launched process pid missing from trace toc")?;
    pid.parse()
        .with_context(|| format!("invalid launched process pid `{pid}`"))
}

fn parse_animation_summary(
    displayed_frames_path: &Path,
    hitches_path: &Path,
    launched_pid: u32,
) -> Result<AnimationSummary> {
    let displayed_frames_xml = fs::read_to_string(displayed_frames_path)
        .with_context(|| format!("failed to read {}", displayed_frames_path.display()))?;
    let hitches_xml = fs::read_to_string(hitches_path)
        .with_context(|| format!("failed to read {}", hitches_path.display()))?;

    let (frame_starts, frame_capture_status, frame_capture_detail) =
        match parse_displayed_frame_starts(&displayed_frames_xml, launched_pid) {
            Ok(frame_starts) if frame_starts.is_empty() => (
                frame_starts,
                FrameCaptureStatus::NoFrames,
                Some(format!(
                    "no displayed frame rows matched launched pid {launched_pid}"
                )),
            ),
            Ok(frame_starts) => (frame_starts, FrameCaptureStatus::Parsed, None),
            Err(error) => (
                Vec::new(),
                FrameCaptureStatus::ParserError,
                Some(error.to_string()),
            ),
        };
    let hitch_durations = parse_hitch_durations(&hitches_xml, launched_pid)?;

    let mut frame_intervals = Vec::new();
    for window in frame_starts.windows(2) {
        let interval = window[1].saturating_sub(window[0]);
        if interval > 0 {
            frame_intervals.push(interval);
        }
    }
    let total_span = frame_starts
        .first()
        .zip(frame_starts.last())
        .map_or(0, |(first, last)| last.saturating_sub(*first));

    let mut sorted_intervals = frame_intervals;
    sorted_intervals.sort_unstable();
    let mut sorted_hitches = hitch_durations;
    sorted_hitches.sort_unstable();

    Ok(AnimationSummary {
        trace_template: "Animation Hitches".to_string(),
        launched_pid: Some(launched_pid),
        displayed_frame_capture_status: frame_capture_status,
        displayed_frame_capture_detail: frame_capture_detail,
        displayed_frame_count: frame_starts.len() as u64,
        fps_avg: if total_span > 0 && frame_starts.len() > 1 {
            Some(
                (frame_starts.len().saturating_sub(1)) as f32
                    / (total_span as f32 / 1_000_000_000.0),
            )
        } else {
            None
        },
        frame_p50_ms: (!sorted_intervals.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted_intervals, 50, 100)),
        frame_p95_ms: (!sorted_intervals.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted_intervals, 95, 100)),
        frame_p99_ms: (!sorted_intervals.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted_intervals, 99, 100)),
        hitch_count: sorted_hitches.len() as u64,
        hitch_max_ms: sorted_hitches.last().copied().map(nanos_to_millis),
    })
}

fn parse_displayed_frame_starts(xml: &str, launched_pid: u32) -> Result<Vec<u64>> {
    let doc = Document::parse(xml).context("failed to parse displayed surfaces xml")?;
    let node = doc
        .descendants()
        .find(|node| node.has_tag_name("node"))
        .context("missing displayed surfaces node")?;
    let schema = node
        .children()
        .find(|child| child.has_tag_name("schema"))
        .context("missing displayed surfaces schema")?;
    let mnemonics = schema
        .children()
        .filter(|child| child.has_tag_name("col"))
        .map(schema_mnemonic)
        .collect::<Result<Vec<_>>>()?;

    let mut numbered_frames = std::collections::BTreeMap::<u64, u64>::new();
    let mut fallback_starts = std::collections::BTreeSet::<u64>::new();
    let mut rows_with_start = 0usize;
    let mut matched_rows = 0usize;
    let references = doc
        .descendants()
        .filter_map(|candidate| {
            candidate
                .attribute("id")
                .map(|id| (id.to_string(), candidate))
        })
        .collect::<std::collections::HashMap<_, _>>();
    for row in node.children().filter(|child| child.has_tag_name("row")) {
        let mut parsed_row = DisplayedFrameRow::default();
        for cell in row.children().filter(Node::is_element) {
            let resolved_cell = resolve_xctrace_reference(cell, &references);
            let children = resolved_cell
                .children()
                .filter(Node::is_element)
                .collect::<Vec<_>>();
            for (index, child) in children.iter().enumerate() {
                let resolved_child = resolve_xctrace_reference(*child, &references);
                if !resolved_child.has_tag_name("process") {
                    continue;
                }
                parsed_row.has_structured_process = true;
                let process_pid = resolved_child
                    .descendants()
                    .find(|descendant| descendant.has_tag_name("pid"))
                    .map(|node| resolve_xctrace_reference(node, &references))
                    .and_then(|pid| node_value(pid))
                    .and_then(|pid| pid.parse::<u32>().ok());
                if process_pid != Some(launched_pid) {
                    continue;
                }
                parsed_row.matches_pid = true;
                let mut saw_frame_label = false;
                for sibling in children.iter().skip(index + 1) {
                    let resolved_sibling = resolve_xctrace_reference(*sibling, &references);
                    if resolved_sibling.has_tag_name("process") {
                        break;
                    }
                    if resolved_sibling.has_tag_name("narrative-text")
                        && resolved_sibling
                            .attribute("fmt")
                            .is_some_and(|value| value.contains("Frame"))
                    {
                        saw_frame_label = true;
                        continue;
                    }
                    if saw_frame_label && resolved_sibling.has_tag_name("uint64") {
                        parsed_row.frame_number = node_value(resolved_sibling)
                            .and_then(|value| value.parse::<u64>().ok());
                        break;
                    }
                }
            }
        }
        for (mnemonic, cell) in mnemonics
            .iter()
            .zip(row.children().filter(Node::is_element))
        {
            let cell = resolve_xctrace_reference(cell, &references);
            let cell_text = node_text_content(cell);
            if !cell_text.is_empty() {
                parsed_row.text_cells.push(cell_text.clone());
            }
            if parsed_row.start_ns.is_none() {
                parsed_row.start_ns = parse_displayed_frame_start(mnemonic, cell, &cell_text);
            }
            if parsed_row.frame_number.is_none() {
                parsed_row.frame_number =
                    parse_displayed_frame_number(mnemonic, cell, &cell_text, launched_pid);
            }
            if parsed_row.process_pid.is_none() {
                parsed_row.process_pid = parse_displayed_frame_pid(mnemonic, cell, &cell_text);
            }
        }

        let Some(start_ns) = parsed_row.start_ns else {
            continue;
        };
        rows_with_start = rows_with_start.saturating_add(1);
        if !displayed_frame_row_matches_pid(&parsed_row, launched_pid) {
            continue;
        }
        matched_rows = matched_rows.saturating_add(1);

        if let Some(frame_number) = parsed_row.frame_number {
            numbered_frames
                .entry(frame_number)
                .and_modify(|current| *current = (*current).min(start_ns))
                .or_insert(start_ns);
        } else {
            fallback_starts.insert(start_ns);
        }
    }

    if !numbered_frames.is_empty() {
        return Ok(numbered_frames.into_values().collect());
    }
    if !fallback_starts.is_empty() {
        return Ok(fallback_starts.into_iter().collect());
    }
    if rows_with_start == 0 {
        bail!("displayed surfaces table did not expose a parseable frame start timestamp");
    }
    if matched_rows == 0 {
        bail!(
            "displayed surfaces rows had timestamps but none matched launched pid {launched_pid}"
        );
    }
    Ok(Vec::new())
}

#[derive(Default)]
struct DisplayedFrameRow {
    start_ns: Option<u64>,
    frame_number: Option<u64>,
    process_pid: Option<u32>,
    matches_pid: bool,
    has_structured_process: bool,
    text_cells: Vec<String>,
}

fn parse_displayed_frame_start(mnemonic: &str, cell: Node<'_, '_>, cell_text: &str) -> Option<u64> {
    if mnemonic == "start" || mnemonic.ends_with("-start") || mnemonic.contains("timestamp") {
        return node_value(cell)
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| cell_text.parse::<u64>().ok());
    }
    None
}

fn parse_displayed_frame_number(
    mnemonic: &str,
    cell: Node<'_, '_>,
    cell_text: &str,
    launched_pid: u32,
) -> Option<u64> {
    if mnemonic.contains("frame") {
        if let Some(value) = node_value(cell).and_then(|value| value.parse::<u64>().ok()) {
            return Some(value);
        }
        if let Some(value) = first_ascii_number(cell_text) {
            return Some(value);
        }
    }
    narrative_frame_number(cell_text, launched_pid)
}

fn parse_displayed_frame_pid(mnemonic: &str, cell: Node<'_, '_>, cell_text: &str) -> Option<u32> {
    if mnemonic == "process" || mnemonic.contains("pid") {
        if let Some(pid) = cell
            .descendants()
            .find(|child| child.has_tag_name("pid"))
            .and_then(|pid| pid.text())
            .and_then(|pid| pid.trim().parse::<u32>().ok())
        {
            return Some(pid);
        }
        return first_ascii_number(cell_text).and_then(|pid| u32::try_from(pid).ok());
    }
    None
}

fn displayed_frame_row_matches_pid(row: &DisplayedFrameRow, launched_pid: u32) -> bool {
    if row.matches_pid {
        return true;
    }
    if row.has_structured_process {
        return false;
    }
    if row
        .text_cells
        .iter()
        .any(|text| text_mentions_pid(text, launched_pid))
    {
        return true;
    }
    if let Some(pid) = row.process_pid {
        return pid == launched_pid;
    }
    true
}

fn text_mentions_pid(text: &str, pid: u32) -> bool {
    text.contains(&format!("({pid})"))
        || text.contains(&format!("pid {pid}"))
        || text.contains(&format!("pid={pid}"))
}

fn narrative_frame_number(text: &str, launched_pid: u32) -> Option<u64> {
    let candidates = [
        format!("({launched_pid}) :Frame "),
        format!("({launched_pid}) Frame "),
        ":Frame ".to_string(),
        "Frame ".to_string(),
    ];
    for pattern in candidates {
        if let Some((_, suffix)) = text.split_once(pattern.as_str()) {
            let digits: String = suffix
                .chars()
                .skip_while(|ch| !ch.is_ascii_digit())
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

fn first_ascii_number(text: &str) -> Option<u64> {
    let mut digits = String::new();
    let mut saw_digit = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            saw_digit = true;
        } else if saw_digit {
            break;
        }
    }
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn node_text_content(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter_map(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_hitch_durations(xml: &str, launched_pid: u32) -> Result<Vec<u64>> {
    let doc = Document::parse(xml).context("failed to parse hitches xml")?;
    let Some(node) = doc.descendants().find(|node| node.has_tag_name("node")) else {
        return Ok(Vec::new());
    };
    let schema = node
        .children()
        .find(|child| child.has_tag_name("schema"))
        .context("missing hitches schema")?;
    let mnemonics = schema
        .children()
        .filter(|child| child.has_tag_name("col"))
        .map(schema_mnemonic)
        .collect::<Result<Vec<_>>>()?;

    let mut durations = Vec::new();
    for row in node.children().filter(|child| child.has_tag_name("row")) {
        let mut duration_ns = None;
        let mut process_pid = None;
        for (mnemonic, cell) in mnemonics
            .iter()
            .zip(row.children().filter(Node::is_element))
        {
            match mnemonic.as_str() {
                "duration" => {
                    duration_ns = node_value(cell).and_then(|value| value.parse::<u64>().ok());
                }
                "process" => {
                    process_pid = cell
                        .children()
                        .find(|child| child.has_tag_name("pid"))
                        .and_then(|pid| pid.text())
                        .and_then(|pid| pid.trim().parse::<u32>().ok());
                }
                _ => {}
            }
        }

        if process_pid.is_some_and(|pid| pid == launched_pid)
            && let Some(duration_ns) = duration_ns
        {
            durations.push(duration_ns);
        }
    }

    Ok(durations)
}

fn summarize_micro_latency(
    scenario: Scenario,
    markers_path: &Path,
    frames_path: &Path,
) -> Result<MicroLatencySummary> {
    match scenario {
        Scenario::IdleBurst => {
            let markers: Vec<MarkerEvent> = read_ndjson(markers_path)?;
            let frames: Vec<FrameEvent> = read_ndjson(frames_path)?;
            Ok(MicroLatencySummary {
                idle_burst: Some(summarize_idle_burst_latency(&markers, &frames)?),
                echo_train: None,
            })
        }
        Scenario::EchoTrain => {
            let markers: Vec<MarkerEvent> = read_ndjson(markers_path)?;
            let frames: Vec<FrameEvent> = read_ndjson(frames_path)?;
            Ok(MicroLatencySummary {
                idle_burst: None,
                echo_train: Some(summarize_echo_train_latency(&markers, &frames)?),
            })
        }
        Scenario::IdleBlink | Scenario::SteadyScroll | Scenario::AltScreenAnim => {
            Ok(MicroLatencySummary::default())
        }
    }
}

fn summarize_idle_burst_latency(
    markers: &[MarkerEvent],
    frames: &[FrameEvent],
) -> Result<IdleBurstLatencySummary> {
    let burst_start = markers
        .iter()
        .find(|marker| marker.kind == "burst_start")
        .context("missing burst_start marker")?
        .monotonic_ns;
    let burst_end = markers
        .iter()
        .find(|marker| marker.kind == "burst_end")
        .context("missing burst_end marker")?
        .monotonic_ns;
    let first_frame = frames
        .iter()
        .find(|frame| frame.monotonic_ns >= burst_start);
    let last_frame = frames
        .iter()
        .rev()
        .find(|frame| frame.monotonic_ns >= burst_end);
    let frames_before_burst = frames
        .iter()
        .rev()
        .find(|frame| frame.monotonic_ns < burst_start)
        .map_or(0, |frame| frame.total_frames);

    Ok(IdleBurstLatencySummary {
        first_frame_after_burst_ms: first_frame
            .map(|frame| nanos_to_millis(frame.monotonic_ns.saturating_sub(burst_start))),
        last_frame_after_burst_ms: last_frame
            .map(|frame| nanos_to_millis(frame.monotonic_ns.saturating_sub(burst_end))),
        frames_until_settle: last_frame
            .map(|frame| frame.total_frames.saturating_sub(frames_before_burst)),
    })
}

fn summarize_echo_train_latency(
    markers: &[MarkerEvent],
    frames: &[FrameEvent],
) -> Result<EchoTrainLatencySummary> {
    let echo_markers: Vec<&MarkerEvent> = markers
        .iter()
        .filter(|marker| marker.kind == "echo_start")
        .collect();
    if echo_markers.is_empty() {
        bail!("missing echo_start markers");
    }

    let mut latencies_ns = Vec::with_capacity(echo_markers.len());
    let mut missed_count = 0u64;
    for marker in echo_markers {
        if let Some(frame) = frames
            .iter()
            .find(|frame| frame.monotonic_ns >= marker.monotonic_ns)
        {
            latencies_ns.push(frame.monotonic_ns.saturating_sub(marker.monotonic_ns));
        } else {
            missed_count = missed_count.saturating_add(1);
        }
    }

    let mut sorted = latencies_ns;
    sorted.sort_unstable();
    Ok(EchoTrainLatencySummary {
        echo_first_frame_ms_p50: (!sorted.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted, 50, 100)),
        echo_first_frame_ms_p95: (!sorted.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted, 95, 100)),
        echo_first_frame_ms_p99: (!sorted.is_empty())
            .then(|| percentile_nanos_to_millis(&sorted, 99, 100)),
        echo_first_frame_ms_max: sorted.last().copied().map(nanos_to_millis),
        echo_missed_count: missed_count,
        echo_sample_count: sorted.len() as u64,
    })
}

fn parse_single_row_table(xml: &str) -> Result<std::collections::HashMap<String, String>> {
    parse_table_rows(xml)?
        .into_iter()
        .next()
        .context("missing row node")
}

fn parse_table_rows(xml: &str) -> Result<Vec<std::collections::HashMap<String, String>>> {
    let doc = Document::parse(xml).context("failed to parse xctrace xml")?;
    let node = doc
        .descendants()
        .find(|node| node.has_tag_name("node"))
        .context("missing trace-query-result node")?;
    let schema = node
        .children()
        .find(|child| child.has_tag_name("schema"))
        .context("missing schema node")?;
    let mnemonics = schema
        .children()
        .filter(|child| child.has_tag_name("col"))
        .map(schema_mnemonic)
        .collect::<Result<Vec<_>>>()?;
    let references = doc
        .descendants()
        .filter_map(|candidate| {
            candidate
                .attribute("id")
                .map(|id| (id.to_string(), candidate))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut rows = Vec::new();
    for row in node.children().filter(|child| child.has_tag_name("row")) {
        let mut values = std::collections::HashMap::new();
        for (mnemonic, cell) in mnemonics
            .iter()
            .zip(row.children().filter(Node::is_element))
        {
            let cell = cell
                .attribute("ref")
                .and_then(|reference| references.get(reference).copied())
                .unwrap_or(cell);
            if cell.has_tag_name("sentinel") {
                continue;
            }
            if let Some(value) = node_value(cell) {
                values.insert(mnemonic.clone(), value);
            }
        }
        rows.push(values);
    }
    if rows.is_empty() {
        bail!("missing row node");
    }
    Ok(rows)
}

fn schema_mnemonic(node: Node<'_, '_>) -> Result<String> {
    node.children()
        .find(|child| child.has_tag_name("mnemonic"))
        .and_then(|mnemonic| mnemonic.text())
        .map(ToOwned::to_owned)
        .context("missing schema mnemonic")
}

fn node_value(node: Node<'_, '_>) -> Option<String> {
    node.text()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn resolve_xctrace_reference<'a, 'input>(
    node: Node<'a, 'input>,
    references: &std::collections::HashMap<String, Node<'a, 'input>>,
) -> Node<'a, 'input> {
    node.attribute("ref")
        .and_then(|reference| references.get(reference).copied())
        .unwrap_or(node)
}

fn shell_escape_path(path: &Path) -> String {
    shell_escape(path.as_os_str())
}

fn shell_escape(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn run_command(command: &mut Command, description: String) -> Result<()> {
    let status = command
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("failed to start {description}"))?;
    if !status.success() {
        bail!("{description} failed with status {status}");
    }
    Ok(())
}

fn run_xctrace_record_command(
    command: &mut Command,
    description: String,
    trace_path: &Path,
    timeout: Duration,
) -> Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to start {description}"))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .with_context(|| format!("failed to poll {description}"))?
        {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{description} exceeded its {}s hard timeout",
                timeout.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(200));
    };
    if status.success() {
        return Ok(());
    }

    if status.code() == Some(54) && trace_path.exists() {
        eprintln!(
            "{description} returned status 54 after writing {}; continuing",
            trace_path.display()
        );
        return Ok(());
    }

    bail!("{description} failed with status {status}");
}

fn xctrace_timeout(time_limit_secs: u64) -> Duration {
    Duration::from_secs(time_limit_secs.saturating_add(XCTRACE_FINALIZATION_GRACE_SECS))
}

fn read_ndjson<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).context("failed to parse ndjson row"))
        .collect::<Result<Vec<_>>>()
        .with_context(|| format!("failed to parse {}", path.display()))
}

fn nanos_to_millis(nanos: u64) -> f32 {
    nanos as f32 / 1_000_000.0
}

fn percentile_nanos_to_millis(samples: &[u64], numerator: usize, denominator: usize) -> f32 {
    let Some(last_index) = samples.len().checked_sub(1) else {
        return 0.0;
    };
    let index =
        (last_index.saturating_mul(numerator) + denominator.saturating_sub(1)) / denominator;
    nanos_to_millis(samples[index])
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("failed to parse {}", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let contents = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    fs::write(path, contents).with_context(|| format!("failed to write {}", path.display()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ComparisonSummary {
    baseline: ComparedTargetSummary,
    candidate: ComparedTargetSummary,
    scenarios: Vec<ScenarioComparison>,
}

impl ComparisonSummary {
    fn from_runs(
        baseline: &BenchmarkTargetSpec,
        candidate: &BenchmarkTargetSpec,
        selected_scenarios: &[Scenario],
        runs: Vec<RunResult>,
    ) -> Result<Self> {
        let mut scenarios = Vec::new();
        for scenario in selected_scenarios {
            let baseline_run = runs
                .iter()
                .find(|run| run.build_label == baseline.label && run.scenario == scenario.as_str())
                .cloned()
                .with_context(|| format!("missing baseline run for {}", scenario.as_str()))?;
            let candidate_run = runs
                .iter()
                .find(|run| run.build_label == candidate.label && run.scenario == scenario.as_str())
                .cloned()
                .with_context(|| format!("missing candidate run for {}", scenario.as_str()))?;
            scenarios.push(ScenarioComparison::new(
                scenario.as_str().to_string(),
                baseline_run,
                candidate_run,
            ));
        }

        Ok(Self {
            baseline: ComparedTargetSummary::from_spec(baseline),
            candidate: ComparedTargetSummary::from_spec(candidate),
            scenarios,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ComparedTargetSummary {
    label: String,
    name: String,
    source_path: String,
    git_sha: Option<String>,
}

impl ComparedTargetSummary {
    fn from_spec(target: &BenchmarkTargetSpec) -> Self {
        Self {
            label: target.label.to_string(),
            name: target.display_name().to_string(),
            source_path: target.source_path.display().to_string(),
            git_sha: target.git_sha.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScenarioComparison {
    scenario: String,
    baseline: RunResult,
    candidate: RunResult,
    deltas: ScenarioDeltas,
}

impl ScenarioComparison {
    fn new(scenario: String, baseline: RunResult, candidate: RunResult) -> Self {
        let deltas = ScenarioDeltas {
            frame_p50_ms: option_animation_delta(&candidate, &baseline, |summary| {
                summary.frame_p50_ms
            }),
            frame_p95_ms: option_animation_delta(&candidate, &baseline, |summary| {
                summary.frame_p95_ms
            }),
            frame_p99_ms: option_animation_delta(&candidate, &baseline, |summary| {
                summary.frame_p99_ms
            }),
            fps_avg: option_animation_delta(&candidate, &baseline, |summary| summary.fps_avg),
            hitch_max_ms: option_animation_delta(&candidate, &baseline, |summary| {
                summary.hitch_max_ms
            }),
            activity_monitor_cpu_percent: option_f32_delta(
                candidate.energy_summary.cpu_percent,
                baseline.energy_summary.cpu_percent,
            ),
            idle_wakeups: option_delta(
                candidate.energy_summary.idle_wakeups,
                baseline.energy_summary.idle_wakeups,
            ),
            hitch_count: option_animation_i64_delta(&candidate, &baseline, |summary| {
                summary.hitch_count
            }),
            memory_bytes: option_i64_delta(
                candidate.energy_summary.memory_bytes,
                baseline.energy_summary.memory_bytes,
            ),
            disk_bytes_read: option_i64_delta(
                candidate.energy_summary.disk_bytes_read,
                baseline.energy_summary.disk_bytes_read,
            ),
            disk_bytes_written: option_i64_delta(
                candidate.energy_summary.disk_bytes_written,
                baseline.energy_summary.disk_bytes_written,
            ),
            idle_burst_first_frame_ms: option_f32_delta(
                candidate
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.first_frame_after_burst_ms),
                baseline
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.first_frame_after_burst_ms),
            ),
            idle_burst_last_frame_ms: option_f32_delta(
                candidate
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.last_frame_after_burst_ms),
                baseline
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.last_frame_after_burst_ms),
            ),
            idle_burst_frames_until_settle: option_i64_delta(
                candidate
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.frames_until_settle),
                baseline
                    .micro_latency
                    .idle_burst
                    .as_ref()
                    .and_then(|summary| summary.frames_until_settle),
            ),
            echo_first_frame_ms_p95: option_f32_delta(
                candidate
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .and_then(|summary| summary.echo_first_frame_ms_p95),
                baseline
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .and_then(|summary| summary.echo_first_frame_ms_p95),
            ),
            echo_first_frame_ms_max: option_f32_delta(
                candidate
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .and_then(|summary| summary.echo_first_frame_ms_max),
                baseline
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .and_then(|summary| summary.echo_first_frame_ms_max),
            ),
            echo_missed_count: option_i64_delta(
                candidate
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .map(|summary| summary.echo_missed_count),
                baseline
                    .micro_latency
                    .echo_train
                    .as_ref()
                    .map(|summary| summary.echo_missed_count),
            ),
        };
        Self {
            scenario,
            baseline,
            candidate,
            deltas,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ScenarioDeltas {
    frame_p50_ms: Option<f32>,
    frame_p95_ms: Option<f32>,
    frame_p99_ms: Option<f32>,
    fps_avg: Option<f32>,
    hitch_max_ms: Option<f32>,
    activity_monitor_cpu_percent: Option<f32>,
    idle_wakeups: Option<i64>,
    hitch_count: Option<i64>,
    memory_bytes: Option<i64>,
    disk_bytes_read: Option<i64>,
    disk_bytes_written: Option<i64>,
    idle_burst_first_frame_ms: Option<f32>,
    idle_burst_last_frame_ms: Option<f32>,
    idle_burst_frames_until_settle: Option<i64>,
    echo_first_frame_ms_p95: Option<f32>,
    echo_first_frame_ms_max: Option<f32>,
    echo_missed_count: Option<i64>,
}

fn option_delta(candidate: Option<u64>, baseline: Option<u64>) -> Option<i64> {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) => Some(candidate as i64 - baseline as i64),
        _ => None,
    }
}

fn option_i64_delta(candidate: Option<u64>, baseline: Option<u64>) -> Option<i64> {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) => Some(candidate as i64 - baseline as i64),
        _ => None,
    }
}

fn option_f32_delta(candidate: Option<f32>, baseline: Option<f32>) -> Option<f32> {
    match (candidate, baseline) {
        (Some(candidate), Some(baseline)) => Some(candidate - baseline),
        _ => None,
    }
}

fn option_animation_delta(
    candidate: &RunResult,
    baseline: &RunResult,
    metric: impl Fn(&AnimationSummary) -> Option<f32>,
) -> Option<f32> {
    match (
        candidate.animation_summary.as_ref(),
        baseline.animation_summary.as_ref(),
    ) {
        (Some(candidate), Some(baseline))
            if candidate.displayed_frame_count >= MIN_DISPLAYED_FRAME_SAMPLES
                && baseline.displayed_frame_count >= MIN_DISPLAYED_FRAME_SAMPLES =>
        {
            option_f32_delta(metric(candidate), metric(baseline))
        }
        _ => None,
    }
}

fn option_animation_i64_delta(
    candidate: &RunResult,
    baseline: &RunResult,
    metric: impl Fn(&AnimationSummary) -> u64,
) -> Option<i64> {
    match (
        candidate.animation_summary.as_ref(),
        baseline.animation_summary.as_ref(),
    ) {
        (Some(candidate), Some(baseline)) => {
            Some(metric(candidate) as i64 - metric(baseline) as i64)
        }
        _ => None,
    }
}

fn write_report_artifacts(output_root: &Path, summary: &ComparisonSummary) -> Result<()> {
    write_json(&output_root.join("summary.json"), summary)?;
    fs::write(output_root.join("report.md"), render_report(summary)).with_context(|| {
        format!(
            "failed to write {}",
            output_root.join("report.md").display()
        )
    })
}

fn render_report(summary: &ComparisonSummary) -> String {
    let mut report = String::new();
    report.push_str("# Termy Render Benchmark Report\n\n");
    report.push_str(&format!(
        "Baseline `{}` ({}) vs candidate `{}` ({}).\n\n",
        summary.baseline.name,
        summary.baseline.label,
        summary.candidate.name,
        summary.candidate.label
    ));
    report.push_str(&format!(
        "- Baseline source: `{}`{}\n",
        summary.baseline.source_path,
        summary
            .baseline
            .git_sha
            .as_ref()
            .map(|sha| format!(" (`{sha}`)"))
            .unwrap_or_default()
    ));
    report.push_str(&format!(
        "- Candidate source: `{}`{}\n\n",
        summary.candidate.source_path,
        summary
            .candidate
            .git_sha
            .as_ref()
            .map(|sha| format!(" (`{sha}`)"))
            .unwrap_or_default()
    ));

    for scenario in &summary.scenarios {
        let idle_blink = scenario.scenario == "idle-blink";
        report.push_str(&format!("## {}\n\n", scenario.scenario));
        report.push_str("### Shared external metrics\n\n");
        report.push_str("| Metric | Baseline | Candidate | Delta |\n");
        report.push_str("| --- | ---: | ---: | ---: |\n");
        report.push_str(&format!(
            "| Displayed frame p50 ms | {} | {} | {} |\n",
            format_option_f32(
                scenario
                    .baseline
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p50_ms)
            ),
            format_option_f32(
                scenario
                    .candidate
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p50_ms)
            ),
            format_option_f32(scenario.deltas.frame_p50_ms),
        ));
        report.push_str(&format!(
            "| Displayed frame p95 ms | {} | {} | {} |\n",
            format_option_f32(
                scenario
                    .baseline
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p95_ms)
            ),
            format_option_f32(
                scenario
                    .candidate
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p95_ms)
            ),
            format_option_f32(scenario.deltas.frame_p95_ms),
        ));
        report.push_str(&format!(
            "| Displayed frame p99 ms | {} | {} | {} |\n",
            format_option_f32(
                scenario
                    .baseline
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p99_ms)
            ),
            format_option_f32(
                scenario
                    .candidate
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.frame_p99_ms)
            ),
            format_option_f32(scenario.deltas.frame_p99_ms),
        ));
        report.push_str(&format!(
            "| Displayed FPS avg | {} | {} | {} |\n",
            format_option_f32(
                scenario
                    .baseline
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.fps_avg)
            ),
            format_option_f32(
                scenario
                    .candidate
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.fps_avg)
            ),
            format_option_f32(scenario.deltas.fps_avg),
        ));
        report.push_str(&format!(
            "| Displayed frame capture | {} | {} | {} |\n",
            format_animation_capture_status(scenario.baseline.animation_summary.as_ref()),
            format_animation_capture_status(scenario.candidate.animation_summary.as_ref()),
            "n/a",
        ));
        report.push_str(&format!(
            "| Hitches | {} | {} | {} |\n",
            scenario.baseline.animation_summary.as_ref().map_or_else(
                || "n/a".to_string(),
                |summary| summary.hitch_count.to_string()
            ),
            scenario.candidate.animation_summary.as_ref().map_or_else(
                || "n/a".to_string(),
                |summary| summary.hitch_count.to_string()
            ),
            format_option_i64(scenario.deltas.hitch_count),
        ));
        report.push_str(&format!(
            "| Hitch max ms | {} | {} | {} |\n",
            format_option_f32(
                scenario
                    .baseline
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.hitch_max_ms)
            ),
            format_option_f32(
                scenario
                    .candidate
                    .animation_summary
                    .as_ref()
                    .and_then(|summary| summary.hitch_max_ms)
            ),
            format_option_f32(scenario.deltas.hitch_max_ms),
        ));
        report.push_str(&format!(
            "| Activity Monitor CPU % | {} | {} | {} |\n",
            format_option_f32(scenario.baseline.energy_summary.cpu_percent),
            format_option_f32(scenario.candidate.energy_summary.cpu_percent),
            format_option_f32(scenario.deltas.activity_monitor_cpu_percent),
        ));
        report.push_str(&format!(
            "| Physical memory bytes | {} | {} | {} |\n",
            format_option_u64(scenario.baseline.energy_summary.memory_bytes),
            format_option_u64(scenario.candidate.energy_summary.memory_bytes),
            format_option_i64(scenario.deltas.memory_bytes),
        ));
        report.push_str(&format!(
            "| Idle wakeups | {} | {} | {} |\n",
            format_option_u64(scenario.baseline.energy_summary.idle_wakeups),
            format_option_u64(scenario.candidate.energy_summary.idle_wakeups),
            format_option_i64(scenario.deltas.idle_wakeups),
        ));
        report.push_str(&format!(
            "| Disk bytes read | {} | {} | {} |\n",
            format_option_u64(scenario.baseline.energy_summary.disk_bytes_read),
            format_option_u64(scenario.candidate.energy_summary.disk_bytes_read),
            format_option_i64(scenario.deltas.disk_bytes_read),
        ));
        report.push_str(&format!(
            "| Disk bytes written | {} | {} | {} |\n\n",
            format_option_u64(scenario.baseline.energy_summary.disk_bytes_written),
            format_option_u64(scenario.candidate.energy_summary.disk_bytes_written),
            format_option_i64(scenario.deltas.disk_bytes_written),
        ));

        match (
            scenario.baseline.app_summary.as_ref(),
            scenario.candidate.app_summary.as_ref(),
        ) {
            (Some(baseline), Some(candidate)) => {
                report.push_str("### App diagnostics\n\n");
                report.push_str(
                    "These values describe `TerminalView::render` callback cadence, not presented-frame latency.\n\n",
                );
                report.push_str("| Metric | Baseline | Candidate | Delta |\n");
                report.push_str("| --- | ---: | ---: | ---: |\n");
                report.push_str(&format!(
                    "| Render callback interval p50 ms | {:.2} | {:.2} | {:.2} |\n",
                    baseline.frame_p50_ms,
                    candidate.frame_p50_ms,
                    candidate.frame_p50_ms - baseline.frame_p50_ms
                ));
                report.push_str(&format!(
                    "| Render callback interval p95 ms | {:.2} | {:.2} | {:.2} |\n",
                    baseline.frame_p95_ms,
                    candidate.frame_p95_ms,
                    candidate.frame_p95_ms - baseline.frame_p95_ms,
                ));
                report.push_str(&format!(
                    "| Render callback interval p99 ms | {:.2} | {:.2} | {:.2} |\n",
                    baseline.frame_p99_ms,
                    candidate.frame_p99_ms,
                    candidate.frame_p99_ms - baseline.frame_p99_ms,
                ));
                report.push_str(&format!(
                    "| Render callbacks / sec | {:.2} | {:.2} | {:.2} |\n",
                    baseline.fps_avg,
                    candidate.fps_avg,
                    candidate.fps_avg - baseline.fps_avg,
                ));
                report.push_str(&format!(
                    "| Runtime wakeups | {} | {} | {} |\n",
                    baseline.runtime_wakeups,
                    candidate.runtime_wakeups,
                    candidate.runtime_wakeups as i64 - baseline.runtime_wakeups as i64
                ));
                report.push_str(&format!(
                    "| View wake signals | {} | {} | {} |\n",
                    baseline.view_wake_signals,
                    candidate.view_wake_signals,
                    candidate.view_wake_signals as i64 - baseline.view_wake_signals as i64
                ));
                report.push_str(&format!(
                    "| Drain passes | {} | {} | {} |\n",
                    baseline.terminal_event_drain_passes,
                    candidate.terminal_event_drain_passes,
                    candidate.terminal_event_drain_passes as i64
                        - baseline.terminal_event_drain_passes as i64
                ));
                report.push_str(&format!(
                    "| Redraws | {} | {} | {} |\n",
                    baseline.terminal_redraws,
                    candidate.terminal_redraws,
                    candidate.terminal_redraws as i64 - baseline.terminal_redraws as i64
                ));
                report.push_str(&format!(
                    "| Grid paints | {} | {} | {} |\n",
                    baseline.grid_paint_count,
                    candidate.grid_paint_count,
                    candidate.grid_paint_count as i64 - baseline.grid_paint_count as i64
                ));
                report.push_str(&format!(
                    "| shape_line calls | {} | {} | {} |\n",
                    baseline.shape_line_calls,
                    candidate.shape_line_calls,
                    candidate.shape_line_calls as i64 - baseline.shape_line_calls as i64
                ));
                report.push_str(&format!(
                    "| Shaped-line cache hits | {} | {} | {} |\n",
                    baseline.shaped_line_cache_hits,
                    candidate.shaped_line_cache_hits,
                    candidate.shaped_line_cache_hits as i64
                        - baseline.shaped_line_cache_hits as i64
                ));
                report.push_str(&format!(
                    "| Shaped-line cache misses | {} | {} | {} |\n",
                    baseline.shaped_line_cache_misses,
                    candidate.shaped_line_cache_misses,
                    candidate.shaped_line_cache_misses as i64
                        - baseline.shaped_line_cache_misses as i64
                ));
                report.push_str(&format!(
                    "| Alt-screen fallback redraws | {} | {} | {} |\n\n",
                    baseline.alt_screen_fallback_redraws,
                    candidate.alt_screen_fallback_redraws,
                    candidate.alt_screen_fallback_redraws as i64
                        - baseline.alt_screen_fallback_redraws as i64
                ));
            }
            (Some(summary), None) => {
                report.push_str("### App diagnostics\n\n");
                render_single_app_diagnostics(&mut report, "Baseline", summary);
            }
            (None, Some(summary)) => {
                report.push_str("### App diagnostics\n\n");
                render_single_app_diagnostics(&mut report, "Candidate", summary);
            }
            (None, None) => {
                report.push_str(
                    "### App diagnostics\n\nApp-internal frame and redraw metrics were unavailable for both targets in this scenario.\n\n",
                );
            }
        }

        if let (Some(baseline), Some(candidate)) = (
            scenario.baseline.micro_latency.idle_burst.as_ref(),
            scenario.candidate.micro_latency.idle_burst.as_ref(),
        ) {
            report.push_str("| Idle-burst metric | Baseline | Candidate | Delta |\n");
            report.push_str("| --- | ---: | ---: | ---: |\n");
            report.push_str(&format!(
                "| First frame after burst ms | {} | {} | {} |\n",
                format_option_f32(baseline.first_frame_after_burst_ms),
                format_option_f32(candidate.first_frame_after_burst_ms),
                format_option_f32(scenario.deltas.idle_burst_first_frame_ms),
            ));
            report.push_str(&format!(
                "| Last frame after burst ms | {} | {} | {} |\n",
                format_option_f32(baseline.last_frame_after_burst_ms),
                format_option_f32(candidate.last_frame_after_burst_ms),
                format_option_f32(scenario.deltas.idle_burst_last_frame_ms),
            ));
            report.push_str(&format!(
                "| Frames until settle | {} | {} | {} |\n\n",
                format_option_u64(baseline.frames_until_settle),
                format_option_u64(candidate.frames_until_settle),
                format_option_i64(scenario.deltas.idle_burst_frames_until_settle),
            ));
        }

        if let (Some(baseline), Some(candidate)) = (
            scenario.baseline.micro_latency.echo_train.as_ref(),
            scenario.candidate.micro_latency.echo_train.as_ref(),
        ) {
            report.push_str("| Echo-train metric | Baseline | Candidate | Delta |\n");
            report.push_str("| --- | ---: | ---: | ---: |\n");
            report.push_str(&format!(
                "| First frame p50 ms | {} | {} | {} |\n",
                format_option_f32(baseline.echo_first_frame_ms_p50),
                format_option_f32(candidate.echo_first_frame_ms_p50),
                format_option_f32(option_f32_delta(
                    candidate.echo_first_frame_ms_p50,
                    baseline.echo_first_frame_ms_p50
                )),
            ));
            report.push_str(&format!(
                "| First frame p95 ms | {} | {} | {} |\n",
                format_option_f32(baseline.echo_first_frame_ms_p95),
                format_option_f32(candidate.echo_first_frame_ms_p95),
                format_option_f32(scenario.deltas.echo_first_frame_ms_p95),
            ));
            report.push_str(&format!(
                "| First frame p99 ms | {} | {} | {} |\n",
                format_option_f32(baseline.echo_first_frame_ms_p99),
                format_option_f32(candidate.echo_first_frame_ms_p99),
                format_option_f32(option_f32_delta(
                    candidate.echo_first_frame_ms_p99,
                    baseline.echo_first_frame_ms_p99
                )),
            ));
            report.push_str(&format!(
                "| First frame max ms | {} | {} | {} |\n",
                format_option_f32(baseline.echo_first_frame_ms_max),
                format_option_f32(candidate.echo_first_frame_ms_max),
                format_option_f32(scenario.deltas.echo_first_frame_ms_max),
            ));
            report.push_str(&format!(
                "| Missed echoes | {} | {} | {} |\n",
                baseline.echo_missed_count,
                candidate.echo_missed_count,
                format_option_i64(scenario.deltas.echo_missed_count),
            ));
            report.push_str(&format!(
                "| Echo samples | {} | {} | {} |\n\n",
                baseline.echo_sample_count,
                candidate.echo_sample_count,
                candidate.echo_sample_count as i64 - baseline.echo_sample_count as i64,
            ));
        }

        report.push_str("Findings:\n");
        if let Some(delta) = scenario.deltas.activity_monitor_cpu_percent {
            report.push_str(&format!(
                "- Candidate {} Activity Monitor CPU by {:.2}%.\n",
                if delta < 0.0 { "reduces" } else { "increases" },
                delta.abs()
            ));
        }
        if !idle_blink && let Some(delta) = scenario.deltas.fps_avg {
            report.push_str(&format!(
                "- Candidate {} displayed FPS by {:.2}.\n",
                if delta > 0.0 { "improves" } else { "regresses" },
                delta.abs()
            ));
        }
        if !idle_blink && let Some(delta) = scenario.deltas.frame_p95_ms {
            report.push_str(&format!(
                "- Candidate {} displayed frame p95 by {:.2} ms.\n",
                if delta < 0.0 { "improves" } else { "regresses" },
                delta.abs()
            ));
        }
        if idle_blink {
            report.push_str(
                "- Idle-blink presentation cadence is diagnostic only; fewer displayed frames or longer intervals can reflect less idle work.\n",
            );
        }
        if let Some(delta) = scenario.deltas.idle_burst_first_frame_ms {
            report.push_str(&format!(
                "- Candidate {} idle-burst first-frame latency by {:.2} ms.\n",
                if delta < 0.0 { "improves" } else { "regresses" },
                delta.abs()
            ));
        }
        if let Some(delta) = scenario.deltas.echo_first_frame_ms_p95 {
            report.push_str(&format!(
                "- Candidate {} echo-train first-frame p95 by {:.2} ms.\n",
                if delta < 0.0 { "improves" } else { "regresses" },
                delta.abs()
            ));
        }
        if let Some(animation_summary) = scenario.baseline.animation_summary.as_ref()
            && !matches!(
                animation_summary.displayed_frame_capture_status,
                FrameCaptureStatus::Parsed
            )
        {
            report.push_str(&format!(
                "- Baseline displayed-frame capture is {}{}.\n",
                format_frame_capture_status_label(
                    &animation_summary.displayed_frame_capture_status
                ),
                animation_summary
                    .displayed_frame_capture_detail
                    .as_ref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            ));
        }
        if let Some(animation_summary) = scenario.candidate.animation_summary.as_ref()
            && !matches!(
                animation_summary.displayed_frame_capture_status,
                FrameCaptureStatus::Parsed
            )
        {
            report.push_str(&format!(
                "- Candidate displayed-frame capture is {}{}.\n",
                format_frame_capture_status_label(
                    &animation_summary.displayed_frame_capture_status
                ),
                animation_summary
                    .displayed_frame_capture_detail
                    .as_ref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            ));
        }
        if scenario.baseline.app_summary.is_some() || scenario.candidate.app_summary.is_some() {
            report.push_str(
                "- Termy-only redraw diagnostics are app-specific; use the shared external metrics table for apples-to-apples frame comparison, and the app diagnostics section to explain Termy-side churn.\n",
            );
        }
        report.push('\n');
    }

    report
}

fn render_single_app_diagnostics(report: &mut String, label: &str, summary: &AppSummary) {
    report.push_str(&format!(
        "Only `{label}` exposed in-app diagnostics for this scenario.\n\n"
    ));
    report.push_str(
        "These values describe `TerminalView::render` callback cadence, not presented-frame latency.\n\n",
    );
    report.push_str(&format!("| Metric | {label} |\n"));
    report.push_str("| --- | ---: |\n");
    report.push_str(&format!(
        "| Render callback interval p50 ms | {:.2} |\n",
        summary.frame_p50_ms
    ));
    report.push_str(&format!(
        "| Render callback interval p95 ms | {:.2} |\n",
        summary.frame_p95_ms
    ));
    report.push_str(&format!(
        "| Render callback interval p99 ms | {:.2} |\n",
        summary.frame_p99_ms
    ));
    report.push_str(&format!(
        "| Render callbacks / sec | {:.2} |\n",
        summary.fps_avg
    ));
    report.push_str(&format!(
        "| Runtime wakeups | {} |\n",
        summary.runtime_wakeups
    ));
    report.push_str(&format!(
        "| View wake signals | {} |\n",
        summary.view_wake_signals
    ));
    report.push_str(&format!(
        "| Drain passes | {} |\n",
        summary.terminal_event_drain_passes
    ));
    report.push_str(&format!("| Redraws | {} |\n", summary.terminal_redraws));
    report.push_str(&format!("| Grid paints | {} |\n", summary.grid_paint_count));
    report.push_str(&format!(
        "| shape_line calls | {} |\n",
        summary.shape_line_calls
    ));
    report.push_str(&format!(
        "| Shaped-line cache hits | {} |\n",
        summary.shaped_line_cache_hits
    ));
    report.push_str(&format!(
        "| Shaped-line cache misses | {} |\n",
        summary.shaped_line_cache_misses
    ));
    report.push_str(&format!(
        "| Alt-screen fallback redraws | {} |\n\n",
        summary.alt_screen_fallback_redraws
    ));
}

fn format_animation_capture_status(summary: Option<&AnimationSummary>) -> String {
    let Some(summary) = summary else {
        return "n/a".to_string();
    };
    let mut label = format_frame_capture_status_label(&summary.displayed_frame_capture_status);
    if let Some(detail) = summary.displayed_frame_capture_detail.as_ref() {
        label.push_str(": ");
        label.push_str(detail);
    }
    label
}

fn format_frame_capture_status_label(status: &FrameCaptureStatus) -> String {
    match status {
        FrameCaptureStatus::Parsed => "parsed".to_string(),
        FrameCaptureStatus::NoFrames => "no_frames".to_string(),
        FrameCaptureStatus::ParserError => "parser_error".to_string(),
    }
}

fn format_option_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| value.to_string())
}

fn format_option_i64(value: Option<i64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| value.to_string())
}

fn format_option_f32(value: Option<f32>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| format!("{value:.2}"))
}

#[cfg(test)]
mod tests {
    use super::{
        BenchmarkDriverSpec, FrameCaptureStatus, FrameEvent, GhosttyVersion, MarkerEvent, Scenario,
        benchmark_config_contents, create_ghostty_launch_artifacts, marker_file_contains,
        parse_animation_summary, parse_displayed_frame_starts, parse_ghostty_version,
        parse_hitch_durations, parse_single_row_table, render_report, resolve_native_executable,
        summarize_echo_train_latency, summarize_idle_burst_latency,
    };
    use std::{fs, path::PathBuf};

    #[test]
    fn parses_scenario_names() {
        assert_eq!(Scenario::parse("idle-blink").unwrap(), Scenario::IdleBlink);
        assert_eq!(Scenario::parse("idle-burst").unwrap(), Scenario::IdleBurst);
        assert_eq!(Scenario::parse("echo-train").unwrap(), Scenario::EchoTrain);
        assert!(Scenario::parse("nope").is_err());
    }

    #[test]
    fn recognizes_driver_start_marker_in_partial_diagnostic_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("markers.ndjson");
        fs::write(
            &path,
            concat!(
                "{\"kind\":\"driver_start\",\"seq\":null,\"monotonic_ns\":1}\n",
                "{\"kind\":\"echo_start\""
            ),
        )
        .unwrap();

        assert!(marker_file_contains(&path, "driver_start").unwrap());
        assert!(!marker_file_contains(&path, "echo_start").unwrap());
    }

    #[test]
    fn idle_blink_is_the_only_scenario_that_enables_cursor_blink() {
        assert!(benchmark_config_contents(Scenario::IdleBlink).contains("cursor_blink = true"));
        assert!(benchmark_config_contents(Scenario::IdleBurst).contains("cursor_blink = false"));
    }

    #[test]
    fn parses_ghostty_version_output() {
        assert_eq!(
            parse_ghostty_version("Ghostty 1.2.3\n"),
            Some(GhosttyVersion {
                major: 1,
                minor: 2,
                patch: 3,
            })
        );
        assert_eq!(
            parse_ghostty_version("v1.2.0-dev"),
            Some(GhosttyVersion {
                major: 1,
                minor: 2,
                patch: 0,
            })
        );
    }

    #[test]
    fn resolves_native_app_executable() {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("Termy.app");
        let executable = app.join("Contents/MacOS/Termy");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, b"native").unwrap();

        assert_eq!(resolve_native_executable(&app).unwrap(), executable);
    }

    #[test]
    fn creates_ghostty_launch_artifacts_with_direct_initial_command() {
        let temp = tempfile::tempdir().unwrap();
        let driver = BenchmarkDriverSpec {
            root: PathBuf::from("/tmp"),
            xtask_binary: PathBuf::from("/Users/test/bin/xtask"),
        };
        let artifacts =
            create_ghostty_launch_artifacts(temp.path(), &driver, Scenario::IdleBurst, 13).unwrap();
        let config = fs::read_to_string(&artifacts.config_path).unwrap();
        assert!(config.contains("initial-command = direct:"));
        assert!(config.contains("quit-after-last-window-closed = true"));
        assert!(config.contains("window-width = 128"));
    }

    #[test]
    fn parses_single_row_xctrace_table() {
        let xml = r#"<?xml version="1.0"?>
<trace-query-result>
  <node>
    <schema name="example">
      <col><mnemonic>cpu-total</mnemonic></col>
      <col><mnemonic>idle-wakeups</mnemonic></col>
    </schema>
    <row>
      <duration-on-core>42</duration-on-core>
      <event-count>7</event-count>
    </row>
  </node>
</trace-query-result>"#;
        let parsed = parse_single_row_table(xml).unwrap();
        assert_eq!(parsed.get("cpu-total").unwrap(), "42");
        assert_eq!(parsed.get("idle-wakeups").unwrap(), "7");
    }

    #[test]
    fn parse_displayed_frame_starts_accepts_process_pid_and_numeric_frame_column() {
        let xml = r#"<?xml version="1.0"?>
<trace-query-result>
  <node>
    <schema name="displayed-surfaces-interval">
      <col><mnemonic>start</mnemonic></col>
      <col><mnemonic>process</mnemonic></col>
      <col><mnemonic>frame-number</mnemonic></col>
    </schema>
    <row>
      <start>100</start>
      <process><pid>42</pid></process>
      <frame>7</frame>
    </row>
    <row>
      <start>160</start>
      <process><pid>42</pid></process>
      <frame>8</frame>
    </row>
    <row>
      <start>220</start>
      <process><pid>99</pid></process>
      <frame>9</frame>
    </row>
  </node>
</trace-query-result>"#;

        let frame_starts = parse_displayed_frame_starts(xml, 42).unwrap();
        assert_eq!(frame_starts, vec![100, 160]);
    }

    #[test]
    fn parse_displayed_frame_starts_resolves_xcode_reference_nodes() {
        let xml = r#"<?xml version="1.0"?>
<trace-query-result>
  <node>
    <schema name="displayed-surfaces-interval">
      <col><mnemonic>start</mnemonic></col>
      <col><mnemonic>event-label</mnemonic></col>
    </schema>
    <row>
      <start-time id="1">100</start-time>
      <narrative id="2">
        <process id="3"><pid>42</pid></process>
        <narrative-text id="4" fmt=":Frame ">:Frame </narrative-text>
        <uint64 id="5">7</uint64>
      </narrative>
    </row>
    <row>
      <start-time id="6">160</start-time>
      <narrative>
        <process ref="3"/>
        <narrative-text ref="4"/>
        <uint64>8</uint64>
      </narrative>
    </row>
    <row>
      <start-time>220</start-time>
      <narrative>
        <process><pid>99</pid></process>
        <narrative-text ref="4"/>
        <uint64>9</uint64>
      </narrative>
    </row>
  </node>
</trace-query-result>"#;

        let frame_starts = parse_displayed_frame_starts(xml, 42).unwrap();
        assert_eq!(frame_starts, vec![100, 160]);
    }

    #[test]
    fn parse_animation_summary_preserves_hitches_when_frame_capture_parser_fails() {
        let displayed_frames = r#"<?xml version="1.0"?>
<trace-query-result>
  <node>
    <schema name="displayed-surfaces-interval">
      <col><mnemonic>event-label</mnemonic></col>
    </schema>
    <row>
      <event>(42) :Frame 7</event>
    </row>
  </node>
</trace-query-result>"#;
        let hitches = r#"<?xml version="1.0"?>
<trace-query-result>
  <node>
    <schema name="hitches">
      <col><mnemonic>duration</mnemonic></col>
      <col><mnemonic>process</mnemonic></col>
    </schema>
    <row>
      <duration>50000000</duration>
      <process><pid>42</pid></process>
    </row>
  </node>
</trace-query-result>"#;

        let temp = tempfile::tempdir().unwrap();
        let displayed_frames_path = temp.path().join("displayed.xml");
        let hitches_path = temp.path().join("hitches.xml");
        fs::write(&displayed_frames_path, displayed_frames).unwrap();
        fs::write(&hitches_path, hitches).unwrap();

        let summary = parse_animation_summary(&displayed_frames_path, &hitches_path, 42).unwrap();
        assert!(matches!(
            summary.displayed_frame_capture_status,
            FrameCaptureStatus::ParserError
        ));
        assert_eq!(summary.displayed_frame_count, 0);
        assert_eq!(summary.hitch_count, 1);
        assert_eq!(summary.hitch_max_ms, Some(50.0));
    }

    #[test]
    fn parse_hitch_durations_treats_empty_export_as_zero_hitches() {
        let xml = r#"<?xml version="1.0"?>
<trace-query-result>
</trace-query-result>"#;

        let hitches = parse_hitch_durations(xml, 42).unwrap();
        assert!(hitches.is_empty());
    }

    #[test]
    fn summarizes_idle_burst_latency() {
        let markers = vec![
            MarkerEvent {
                kind: "burst_start".to_string(),
                seq: None,
                monotonic_ns: 100,
            },
            MarkerEvent {
                kind: "burst_end".to_string(),
                seq: None,
                monotonic_ns: 150,
            },
        ];
        let frames = vec![
            FrameEvent {
                monotonic_ns: 90,
                elapsed_ms: 0,
                total_frames: 1,
                terminal_redraws: 0,
                view_wake_signals: 0,
                runtime_wakeups: 0,
            },
            FrameEvent {
                monotonic_ns: 130,
                elapsed_ms: 1,
                total_frames: 2,
                terminal_redraws: 1,
                view_wake_signals: 1,
                runtime_wakeups: 1,
            },
            FrameEvent {
                monotonic_ns: 180,
                elapsed_ms: 2,
                total_frames: 4,
                terminal_redraws: 2,
                view_wake_signals: 2,
                runtime_wakeups: 2,
            },
        ];

        let summary = summarize_idle_burst_latency(&markers, &frames).unwrap();
        assert!(
            (summary.first_frame_after_burst_ms.unwrap() - 0.00003).abs() < f32::EPSILON,
            "unexpected first frame latency: {:?}",
            summary.first_frame_after_burst_ms
        );
        assert!(
            (summary.last_frame_after_burst_ms.unwrap() - 0.00003).abs() < f32::EPSILON,
            "unexpected last frame latency: {:?}",
            summary.last_frame_after_burst_ms
        );
        assert_eq!(summary.frames_until_settle, Some(3));
    }

    #[test]
    fn summarizes_echo_train_latency() {
        let markers = vec![
            MarkerEvent {
                kind: "echo_start".to_string(),
                seq: Some(0),
                monotonic_ns: 100,
            },
            MarkerEvent {
                kind: "echo_start".to_string(),
                seq: Some(1),
                monotonic_ns: 200,
            },
        ];
        let frames = vec![
            FrameEvent {
                monotonic_ns: 120,
                elapsed_ms: 1,
                total_frames: 1,
                terminal_redraws: 1,
                view_wake_signals: 1,
                runtime_wakeups: 1,
            },
            FrameEvent {
                monotonic_ns: 260,
                elapsed_ms: 2,
                total_frames: 2,
                terminal_redraws: 2,
                view_wake_signals: 2,
                runtime_wakeups: 2,
            },
        ];

        let summary = summarize_echo_train_latency(&markers, &frames).unwrap();
        assert_eq!(summary.echo_missed_count, 0);
        assert_eq!(summary.echo_sample_count, 2);
        assert!((summary.echo_first_frame_ms_max.unwrap() - 0.00006).abs() < f32::EPSILON);
    }

    #[test]
    fn render_report_mentions_micro_latency_tables() {
        let report = render_report(&super::ComparisonSummary {
            baseline: super::ComparedTargetSummary {
                label: "baseline".to_string(),
                name: "Termy".to_string(),
                source_path: "/tmp/baseline".to_string(),
                git_sha: Some("abc".to_string()),
            },
            candidate: super::ComparedTargetSummary {
                label: "candidate".to_string(),
                name: "Ghostty".to_string(),
                source_path: "/Applications/Ghostty.app".to_string(),
                git_sha: None,
            },
            scenarios: vec![super::ScenarioComparison {
                scenario: "idle-burst".to_string(),
                baseline: super::RunResult {
                    build_label: "baseline".to_string(),
                    target_name: "Termy".to_string(),
                    git_sha: Some("abc".to_string()),
                    scenario: "idle-burst".to_string(),
                    app_summary: Some(super::AppSummary {
                        build_label: Some("baseline".to_string()),
                        git_sha: Some("abc".to_string()),
                        scenario: "idle-burst".to_string(),
                        duration_ms: 3000,
                        sample_count: 2,
                        total_frames: 4,
                        fps_avg: 1.0,
                        frame_p50_ms: 12.0,
                        frame_p95_ms: 20.0,
                        frame_p99_ms: 25.0,
                        cpu_avg_percent: 3.0,
                        cpu_max_percent: 5.0,
                        memory_max_bytes: 1,
                        runtime_wakeups: 1,
                        view_wake_signals: 1,
                        terminal_event_drain_passes: 1,
                        terminal_redraws: 1,
                        alt_screen_fallback_redraws: 0,
                        grid_paint_count: 1,
                        shape_line_calls: 1,
                        shaped_line_cache_hits: 4,
                        shaped_line_cache_misses: 1,
                    }),
                    energy_summary: super::EnergySummary {
                        trace_template: "Activity Monitor".to_string(),
                        cpu_total_ns: None,
                        cpu_percent: Some(3.0),
                        idle_wakeups: Some(10),
                        memory_bytes: Some(100),
                        disk_bytes_read: Some(5),
                        disk_bytes_written: Some(6),
                    },
                    animation_summary: Some(super::AnimationSummary {
                        trace_template: "Animation Hitches".to_string(),
                        launched_pid: Some(1),
                        displayed_frame_capture_status: super::FrameCaptureStatus::Parsed,
                        displayed_frame_capture_detail: None,
                        displayed_frame_count: 100,
                        fps_avg: Some(59.0),
                        frame_p50_ms: Some(16.7),
                        frame_p95_ms: Some(20.0),
                        frame_p99_ms: Some(25.0),
                        hitch_count: 2,
                        hitch_max_ms: Some(42.0),
                    }),
                    micro_latency: super::MicroLatencySummary {
                        idle_burst: Some(super::IdleBurstLatencySummary {
                            first_frame_after_burst_ms: Some(4.0),
                            last_frame_after_burst_ms: Some(12.0),
                            frames_until_settle: Some(2),
                        }),
                        echo_train: None,
                    },
                },
                candidate: super::RunResult {
                    build_label: "candidate".to_string(),
                    target_name: "Ghostty".to_string(),
                    git_sha: None,
                    scenario: "idle-burst".to_string(),
                    app_summary: None,
                    energy_summary: super::EnergySummary {
                        trace_template: "Activity Monitor".to_string(),
                        cpu_total_ns: None,
                        cpu_percent: Some(4.0),
                        idle_wakeups: Some(11),
                        memory_bytes: Some(101),
                        disk_bytes_read: Some(7),
                        disk_bytes_written: Some(9),
                    },
                    animation_summary: Some(super::AnimationSummary {
                        trace_template: "Animation Hitches".to_string(),
                        launched_pid: Some(2),
                        displayed_frame_capture_status: super::FrameCaptureStatus::Parsed,
                        displayed_frame_capture_detail: None,
                        displayed_frame_count: 105,
                        fps_avg: Some(61.0),
                        frame_p50_ms: Some(15.8),
                        frame_p95_ms: Some(18.0),
                        frame_p99_ms: Some(21.0),
                        hitch_count: 1,
                        hitch_max_ms: Some(28.0),
                    }),
                    micro_latency: super::MicroLatencySummary {
                        idle_burst: None,
                        echo_train: None,
                    },
                },
                deltas: super::ScenarioDeltas {
                    frame_p50_ms: Some(-0.9),
                    frame_p95_ms: Some(-2.0),
                    frame_p99_ms: Some(-4.0),
                    fps_avg: Some(2.0),
                    hitch_max_ms: Some(-14.0),
                    activity_monitor_cpu_percent: Some(1.0),
                    idle_wakeups: Some(1),
                    hitch_count: Some(-1),
                    memory_bytes: Some(1),
                    disk_bytes_read: Some(2),
                    disk_bytes_written: Some(3),
                    idle_burst_first_frame_ms: None,
                    idle_burst_last_frame_ms: None,
                    idle_burst_frames_until_settle: None,
                    echo_first_frame_ms_p95: None,
                    echo_first_frame_ms_max: None,
                    echo_missed_count: None,
                },
            }],
        });
        assert!(report.contains("Shared external metrics"));
        assert!(report.contains("Displayed frame p95 ms"));
        assert!(report.contains("Render callback interval p95 ms"));
        assert!(report.contains("callback cadence, not presented-frame latency"));
        assert!(report.contains("Only `Baseline` exposed in-app diagnostics"));
        assert!(report.contains("Shaped-line cache hits"));
    }

    #[test]
    fn benchmark_gates_fail_on_positive_regressions() {
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![super::ScenarioComparison::new(
                "idle-burst".to_string(),
                run_result("baseline", 3.0, 10, 10 * 1024 * 1024),
                run_result("candidate", 25.0, 200, 45 * 1024 * 1024),
            )],
        };

        let failures = super::BenchmarkGateThresholds::default().failures(&summary);

        assert!(failures.iter().any(|failure| failure.contains("CPU")));
        assert!(failures.iter().any(|failure| failure.contains("memory")));
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("idle wakeup"))
        );
    }

    #[test]
    fn benchmark_gates_fail_when_required_external_metrics_are_missing() {
        let mut scenario = super::ScenarioComparison::new(
            "idle-burst".to_string(),
            run_result("baseline", 3.0, 10, 10 * 1024 * 1024),
            run_result("candidate", 3.0, 10, 10 * 1024 * 1024),
        );
        scenario.deltas.activity_monitor_cpu_percent = None;
        scenario.deltas.memory_bytes = None;
        scenario.deltas.idle_wakeups = None;
        scenario.deltas.hitch_count = None;
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![scenario],
        };

        let failures = super::BenchmarkGateThresholds::default().failures(&summary);

        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("required activity-monitor CPU delta is missing"))
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("required memory delta is missing"))
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("required idle wakeup delta is missing"))
        );
        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("required hitch count delta is missing"))
        );
    }

    #[test]
    fn benchmark_gates_require_a_displayed_frame_for_active_output_scenarios() {
        for scenario_name in ["idle-burst", "echo-train"] {
            let summary = super::ComparisonSummary {
                baseline: compared_target("baseline"),
                candidate: compared_target("candidate"),
                scenarios: vec![super::ScenarioComparison::new(
                    scenario_name.to_string(),
                    run_result("baseline", 3.0, 10, 10 * 1024 * 1024),
                    run_result("candidate", 3.0, 10, 10 * 1024 * 1024),
                )],
            };

            let failures = super::BenchmarkGateThresholds::default().failures(&summary);

            assert!(failures.iter().any(|failure| {
                failure.contains(&format!(
                    "{scenario_name}: candidate displayed frame count 0 is below minimum 1"
                ))
            }));
        }
    }

    #[test]
    fn benchmark_gates_fail_when_summary_contains_no_scenarios() {
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: Vec::new(),
        };

        let failures = super::BenchmarkGateThresholds::default().failures(&summary);

        assert_eq!(failures, ["benchmark summary contains no scenarios"]);
    }

    #[test]
    fn benchmark_gates_fail_when_displayed_frame_trace_parser_fails() {
        let mut baseline = run_result("baseline", 3.0, 10, 10 * 1024 * 1024);
        baseline.animation_summary = Some(super::AnimationSummary::default());
        let mut candidate = run_result("candidate", 3.0, 10, 10 * 1024 * 1024);
        candidate.animation_summary = Some(super::AnimationSummary {
            displayed_frame_capture_status: super::FrameCaptureStatus::ParserError,
            displayed_frame_capture_detail: Some("unparseable timestamp".to_string()),
            ..super::AnimationSummary::default()
        });
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![super::ScenarioComparison::new(
                "idle-burst".to_string(),
                baseline,
                candidate,
            )],
        };

        let failures = super::BenchmarkGateThresholds::default().failures(&summary);

        assert!(failures.iter().any(|failure| {
            failure.contains(
                "idle-burst: candidate displayed-frame trace parser failed: unparseable timestamp",
            )
        }));
    }

    #[test]
    fn benchmark_gates_can_opt_out_when_hosted_runner_has_no_displayed_frames() {
        let mut baseline = run_result("baseline", 3.0, 10, 10 * 1024 * 1024);
        baseline.animation_summary = Some(super::AnimationSummary::default());
        let mut candidate = run_result("candidate", 3.0, 10, 10 * 1024 * 1024);
        candidate.animation_summary = Some(super::AnimationSummary {
            displayed_frame_capture_status: super::FrameCaptureStatus::ParserError,
            displayed_frame_capture_detail: Some("no timestamps on hosted runner".to_string()),
            ..super::AnimationSummary::default()
        });
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![super::ScenarioComparison::new(
                "idle-burst".to_string(),
                baseline,
                candidate,
            )],
        };
        let mut thresholds = super::BenchmarkGateThresholds::default();
        thresholds.min_displayed_frames = 0;

        let failures = thresholds.failures(&summary);

        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    }

    #[test]
    fn benchmark_gates_can_opt_out_when_animation_trace_is_not_collected() {
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![super::ScenarioComparison::new(
                "idle-burst".to_string(),
                run_result("baseline", 3.0, 10, 10 * 1024 * 1024),
                run_result("candidate", 3.0, 10, 10 * 1024 * 1024),
            )],
        };
        let mut thresholds = super::BenchmarkGateThresholds::default();
        thresholds.min_displayed_frames = 0;

        let failures = thresholds.failures(&summary);

        assert!(failures.is_empty(), "unexpected failures: {failures:?}");
    }

    #[test]
    fn idle_blink_gates_wakeups_but_not_presentation_interval() {
        let mut scenario = super::ScenarioComparison::new(
            "idle-blink".to_string(),
            run_result("baseline", 3.0, 10, 10 * 1024 * 1024),
            run_result("candidate", 3.0, 200, 10 * 1024 * 1024),
        );
        scenario.deltas.frame_p95_ms = Some(500.0);
        let summary = super::ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![scenario],
        };

        let failures = super::BenchmarkGateThresholds::default().failures(&summary);

        assert!(
            failures
                .iter()
                .any(|failure| failure.contains("idle wakeup"))
        );
        assert!(!failures.iter().any(|failure| failure.contains("frame p95")));
    }

    fn compared_target(label: &str) -> super::ComparedTargetSummary {
        super::ComparedTargetSummary {
            label: label.to_string(),
            name: "Termy".to_string(),
            source_path: format!("/tmp/{label}"),
            git_sha: None,
        }
    }

    fn run_result(
        build_label: &str,
        cpu_percent: f32,
        idle_wakeups: u64,
        memory_bytes: u64,
    ) -> super::RunResult {
        super::RunResult {
            build_label: build_label.to_string(),
            target_name: "Termy".to_string(),
            git_sha: None,
            scenario: "idle-burst".to_string(),
            app_summary: None,
            energy_summary: super::EnergySummary {
                trace_template: "Activity Monitor".to_string(),
                cpu_total_ns: None,
                cpu_percent: Some(cpu_percent),
                idle_wakeups: Some(idle_wakeups),
                memory_bytes: Some(memory_bytes),
                disk_bytes_read: None,
                disk_bytes_written: None,
            },
            animation_summary: None,
            micro_latency: super::MicroLatencySummary::default(),
        }
    }
}
