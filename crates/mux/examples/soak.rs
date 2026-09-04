use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use engine::pty::PtyCommand;
use mux::{Client, TerminalSize};
use serde::Serialize;

const DEFAULT_SECONDS: u64 = 30 * 60;
const MAX_RSS_KIB: u64 = 256 * 1024;
const MAX_RSS_GROWTH_KIB: u64 = 64 * 1024;
const MAX_STEADY_RSS_GROWTH_KIB: u64 = 16 * 1024;
const MAX_CPU_PERCENT: f64 = 100.0;
const MAX_FD_GROWTH: u64 = 16;
const MAX_THREAD_GROWTH: u64 = 8;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const CYCLE_IDLE: Duration = Duration::from_millis(100);
const LONG_IDLE: Duration = Duration::from_millis(500);
const TAB_CHURN_INTERVAL: u64 = 10;
const DETACH_INTERVAL: u64 = 50;
const LONG_IDLE_INTERVAL: u64 = 100;
const MAX_SETTLE_SECONDS: u64 = 5 * 60;

#[derive(Debug)]
struct Config {
    seconds: u64,
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ProcessSample {
    rss_kib: u64,
    cpu_percent: f64,
    file_descriptors: u64,
    threads: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct Limits {
    rss_kib: u64,
    rss_growth_kib: u64,
    steady_rss_growth_kib: u64,
    cpu_percent: f64,
    fd_growth: u64,
    thread_growth: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct TimedSample {
    elapsed_seconds: u64,
    process: ProcessSample,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u16,
    duration_seconds: u64,
    cycles: u64,
    bytes_written: u64,
    bytes_drained: u64,
    warmup_bytes_written: u64,
    warmup_bytes_drained: u64,
    detach_reattach_count: u64,
    tab_churn_count: u64,
    resize_count: u64,
    long_idle_count: u64,
    baseline: ProcessSample,
    maximum: ProcessSample,
    rss_growth_kib: u64,
    settle_seconds: u64,
    steady_state_baseline: ProcessSample,
    steady_state_maximum: ProcessSample,
    steady_state_rss_growth_kib: u64,
    fd_growth: u64,
    thread_growth: u64,
    daemon_count: u64,
    limits: Limits,
    samples: Vec<TimedSample>,
    passed: bool,
}

#[derive(Debug)]
struct IsolatedMultiplexer {
    root: PathBuf,
    socket: PathBuf,
    server: Option<thread::JoinHandle<Result<()>>>,
}

impl IsolatedMultiplexer {
    fn start() -> Result<Self> {
        let root = unique_directory();
        let socket = root.join("multiplexer.sock");
        let server_socket = socket.clone();
        let server = thread::Builder::new()
            .name("tmon-soak-daemon".to_owned())
            .spawn(move || mux::serve(&server_socket))
            .context("starting isolated soak daemon")?;
        let mut isolated = Self {
            root,
            socket,
            server: Some(server),
        };
        if let Err(error) = wait_for_socket(&isolated.socket) {
            isolated.shutdown_best_effort();
            return Err(error);
        }
        Ok(isolated)
    }

    fn shutdown(&mut self) -> Result<()> {
        if self.socket.exists() {
            let mut client = Client::connect_existing(&self.socket)
                .context("reconnecting to terminate the isolated soak daemon")?;
            client
                .terminate_all_sessions()
                .context("terminating isolated soak sessions")?;
        }
        if let Some(server) = self.server.take() {
            server
                .join()
                .map_err(|_| anyhow::anyhow!("isolated soak daemon panicked"))??;
        }
        if self.root.exists() {
            fs::remove_dir_all(&self.root).context("removing isolated soak directory")?;
        }
        Ok(())
    }

    fn shutdown_best_effort(&mut self) {
        let _ = self.shutdown();
    }
}

impl Drop for IsolatedMultiplexer {
    fn drop(&mut self) {
        self.shutdown_best_effort();
    }
}

fn main() -> Result<()> {
    let config = parse_config()?;
    let mut isolated = IsolatedMultiplexer::start()?;

    let command = PtyCommand::new("/bin/sh").with_arguments([
        "-c",
        "stty -echo; while IFS= read -r line; do printf 'soak:%s\\n' \"$line\"; done",
    ]);
    // Allocate the largest exercised geometry before the baseline so the growth check measures
    // steady state rather than the intentional cost of filling a 5,000-row scrollback.
    let mut size = TerminalSize::new(46, 140, 1_400, 920);
    let mut client = Client::connect_existing(&isolated.socket)?;
    let restore = client.attach(&command, size, 5_000, 1_000)?;
    let primary_tab = restore.active_tab_id;
    let warmup_payload = scrollback_warmup_payload();
    client.write(primary_tab, &warmup_payload)?;
    let warmup_bytes_drained = drain_until_quiet(&mut client, Duration::from_secs(10))?;

    let churn =
        PtyCommand::new("/bin/sh").with_arguments(["-c", "printf 'tab-ready\\n'; sleep 60"]);
    let tab = client.new_tab(&churn, size, 128)?;
    let _ = drain_until_quiet(&mut client, Duration::from_secs(2))?;
    client.close_tab(tab.id)?;
    drop(client);
    thread::sleep(Duration::from_millis(50));
    let mut client = Client::connect_existing(&isolated.socket)?;
    let restored = client.attach(&command, size, 5_000, 1_000)?;
    if restored.tabs.len() != 1 || restored.active_tab_id != primary_tab {
        bail!("warmed soak session did not restore exactly once");
    }

    let baseline = process_sample()?;
    let mut maximum = baseline;
    let started = Instant::now();
    let settle_seconds = config.seconds.div_ceil(4).clamp(1, MAX_SETTLE_SECONDS);
    let settle_at = Duration::from_secs(settle_seconds);
    let mut next_sample = started + SAMPLE_INTERVAL;
    let mut steady_state_baseline = None;
    let mut steady_state_maximum = None;
    let mut samples = vec![TimedSample {
        elapsed_seconds: 0,
        process: baseline,
    }];
    let mut cycles = 0_u64;
    let mut bytes_written = 0_u64;
    let mut bytes_drained = 0_u64;
    let mut detach_reattach_count = 0_u64;
    let mut tab_churn_count = 0_u64;
    let mut resize_count = 0_u64;
    let mut long_idle_count = 0_u64;

    while started.elapsed() < Duration::from_secs(config.seconds) {
        let payload = output_payload(cycles);
        client.write(primary_tab, &payload)?;
        bytes_written = bytes_written
            .saturating_add(u64::try_from(payload.len()).context("converting written byte count")?);
        bytes_drained = bytes_drained.saturating_add(
            u64::try_from(drain_until_output(&mut client, Duration::from_secs(2))?)
                .context("converting drained byte count")?,
        );

        if cycles.is_multiple_of(TAB_CHURN_INTERVAL) {
            let churn = PtyCommand::new("/bin/sh")
                .with_arguments(["-c", "printf 'tab-ready\\n'; sleep 60"]);
            let tab = client.new_tab(&churn, size, 128)?;
            bytes_drained = bytes_drained.saturating_add(
                u64::try_from(drain_until_output(&mut client, Duration::from_secs(2))?)
                    .context("converting drained byte count")?,
            );
            client.close_tab(tab.id)?;
            tab_churn_count = tab_churn_count.saturating_add(1);
        }

        let columns = 80 + u16::try_from(cycles % 61).unwrap_or(0);
        let rows = 24 + u16::try_from(cycles % 23).unwrap_or(0);
        size = TerminalSize::new(rows, columns, columns * 10, rows * 20);
        client.resize_all(size)?;
        resize_count = resize_count.saturating_add(1);

        if cycles % DETACH_INTERVAL == DETACH_INTERVAL - 1 {
            drop(client);
            thread::sleep(Duration::from_millis(50));
            client = Client::connect_existing(&isolated.socket)?;
            let restored = client.attach(&command, size, 5_000, 1_000)?;
            if restored.tabs.len() != 1 || restored.active_tab_id != primary_tab {
                bail!("detached soak session did not restore exactly once");
            }
            detach_reattach_count = detach_reattach_count.saturating_add(1);
        }

        if cycles % LONG_IDLE_INTERVAL == LONG_IDLE_INTERVAL - 1 {
            thread::sleep(LONG_IDLE);
            long_idle_count = long_idle_count.saturating_add(1);
        } else {
            thread::sleep(CYCLE_IDLE);
        }
        if Instant::now() >= next_sample {
            let sample = process_sample()?;
            let elapsed = started.elapsed();
            maximum = max_sample(maximum, sample);
            samples.push(TimedSample {
                elapsed_seconds: elapsed.as_secs(),
                process: sample,
            });
            if elapsed >= settle_at {
                steady_state_baseline.get_or_insert(sample);
                steady_state_maximum = Some(
                    steady_state_maximum.map_or(sample, |maximum| max_sample(maximum, sample)),
                );
            }
            next_sample = Instant::now() + SAMPLE_INTERVAL;
        }
        cycles = cycles.saturating_add(1);
    }

    let final_sample = process_sample()?;
    let final_elapsed = started.elapsed();
    maximum = max_sample(maximum, final_sample);
    samples.push(TimedSample {
        elapsed_seconds: final_elapsed.as_secs(),
        process: final_sample,
    });
    let steady_state_baseline = steady_state_baseline.unwrap_or(final_sample);
    let steady_state_maximum =
        steady_state_maximum.map_or(final_sample, |maximum| max_sample(maximum, final_sample));
    let rss_growth_kib = maximum.rss_kib.saturating_sub(baseline.rss_kib);
    let steady_state_rss_growth_kib = steady_state_maximum
        .rss_kib
        .saturating_sub(steady_state_baseline.rss_kib);
    let fd_growth = maximum
        .file_descriptors
        .saturating_sub(baseline.file_descriptors);
    let thread_growth = maximum.threads.saturating_sub(baseline.threads);
    let passed = cycles > 0
        && bytes_drained > 0
        && resize_count == cycles
        && tab_churn_count > 0
        && detach_reattach_count > 0
        && long_idle_count > 0
        && maximum.rss_kib <= MAX_RSS_KIB
        && rss_growth_kib <= MAX_RSS_GROWTH_KIB
        && steady_state_rss_growth_kib <= MAX_STEADY_RSS_GROWTH_KIB
        && maximum.cpu_percent <= MAX_CPU_PERCENT
        && fd_growth <= MAX_FD_GROWTH
        && thread_growth <= MAX_THREAD_GROWTH;
    let report = Report {
        schema_version: 3,
        duration_seconds: final_elapsed.as_secs(),
        cycles,
        bytes_written,
        bytes_drained,
        warmup_bytes_written: u64::try_from(warmup_payload.len())
            .context("converting warmup byte count")?,
        warmup_bytes_drained: u64::try_from(warmup_bytes_drained)
            .context("converting warmup drained byte count")?,
        detach_reattach_count,
        tab_churn_count,
        resize_count,
        long_idle_count,
        baseline,
        maximum,
        rss_growth_kib,
        settle_seconds,
        steady_state_baseline,
        steady_state_maximum,
        steady_state_rss_growth_kib,
        fd_growth,
        thread_growth,
        daemon_count: 1,
        limits: Limits {
            rss_kib: MAX_RSS_KIB,
            rss_growth_kib: MAX_RSS_GROWTH_KIB,
            steady_rss_growth_kib: MAX_STEADY_RSS_GROWTH_KIB,
            cpu_percent: MAX_CPU_PERCENT,
            fd_growth: MAX_FD_GROWTH,
            thread_growth: MAX_THREAD_GROWTH,
        },
        samples,
        passed,
    };

    drop(client);
    isolated.shutdown()?;

    if let Some(parent) = config.output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config.output, serde_json::to_vec_pretty(&report)?)?;
    println!("soak report: {}", config.output.display());
    println!(
        "cycles={} rss_growth={} KiB steady_growth={} KiB fd_growth={} thread_growth={} passed={}",
        report.cycles,
        report.rss_growth_kib,
        report.steady_state_rss_growth_kib,
        report.fd_growth,
        report.thread_growth,
        report.passed
    );
    if !report.passed {
        bail!("soak resource bounds failed");
    }
    Ok(())
}

fn scrollback_warmup_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(256 * 1024);
    for line in 0..6_000_u64 {
        payload.extend_from_slice(format!("warmup-line-{line:05}\n").as_bytes());
    }
    payload
}

fn parse_config() -> Result<Config> {
    let mut seconds = DEFAULT_SECONDS;
    let mut output = PathBuf::from("performance/results/soak.json");
    let mut arguments = env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--seconds") => {
                seconds = arguments
                    .next()
                    .context("--seconds requires a value")?
                    .to_str()
                    .context("--seconds must be UTF-8")?
                    .parse()
                    .context("parsing --seconds")?;
                if seconds == 0 {
                    bail!("--seconds must be positive");
                }
            }
            Some("--output") => {
                output = PathBuf::from(arguments.next().context("--output requires a path")?);
            }
            _ => bail!("usage: soak [--seconds N] [--output PATH]"),
        }
    }
    Ok(Config { seconds, output })
}

fn unique_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    env::temp_dir().join(format!("tmon-soak-{}-{nonce}", std::process::id()))
}

fn wait_for_socket(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if socket.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    bail!("isolated soak daemon did not create its socket")
}

fn output_payload(cycle: u64) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16 * 1024);
    for line in 0..200_u64 {
        payload.extend_from_slice(format!("cycle-{cycle:08}-line-{line:03}\n").as_bytes());
    }
    payload
}

fn drain_until_output(client: &mut Client, timeout: Duration) -> Result<usize> {
    let deadline = Instant::now() + timeout;
    let mut total = 0_usize;
    while Instant::now() < deadline {
        let batch = client.drain()?;
        total = total.saturating_add(
            batch
                .outputs
                .iter()
                .map(|output| output.bytes.len())
                .sum::<usize>(),
        );
        if total > 0 || !batch.resynchronized_tabs.is_empty() {
            return Ok(total);
        }
        thread::sleep(Duration::from_millis(2));
    }
    bail!("timed out waiting for soak PTY output")
}

fn drain_until_quiet(client: &mut Client, timeout: Duration) -> Result<usize> {
    let deadline = Instant::now() + timeout;
    let mut quiet_since = None;
    let mut total = 0_usize;
    while Instant::now() < deadline {
        let batch = client.drain()?;
        let drained = batch
            .outputs
            .iter()
            .map(|output| output.bytes.len())
            .sum::<usize>();
        total = total.saturating_add(drained);
        if drained > 0 || !batch.resynchronized_tabs.is_empty() {
            quiet_since = None;
        } else if quiet_since
            .get_or_insert_with(Instant::now)
            .elapsed()
            .ge(&Duration::from_millis(50))
        {
            return Ok(total);
        }
        thread::sleep(Duration::from_millis(2));
    }
    bail!("timed out waiting for the soak PTY to become quiet")
}

fn process_sample() -> Result<ProcessSample> {
    let pid = std::process::id().to_string();
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-o", "%cpu=", "-p", &pid])
        .output()
        .context("sampling soak process")?;
    if !output.status.success() {
        bail!("ps could not sample the soak process");
    }
    let fields = String::from_utf8(output.stdout)?;
    let mut fields = fields.split_whitespace();
    let rss_kib = fields.next().context("missing RSS sample")?.parse()?;
    let cpu_percent = fields.next().context("missing CPU sample")?.parse()?;

    let output = Command::new("/bin/ps")
        .args(["-M", "-p", &pid])
        .output()
        .context("sampling soak threads")?;
    if !output.status.success() {
        bail!("ps could not sample soak threads");
    }
    let threads = u64::try_from(String::from_utf8(output.stdout)?.lines().skip(1).count())
        .context("converting thread count")?;
    if threads == 0 {
        bail!("ps returned no soak threads");
    }

    let output = Command::new("/usr/sbin/lsof")
        .args(["-p", &pid, "-Fn"])
        .output()
        .context("sampling soak file descriptors")?;
    if !output.status.success() {
        bail!("lsof could not sample soak file descriptors");
    }
    let file_descriptors = u64::try_from(
        String::from_utf8(output.stdout)?
            .lines()
            .filter(|line| line.starts_with('f'))
            .count(),
    )
    .context("converting file descriptor count")?;
    Ok(ProcessSample {
        rss_kib,
        cpu_percent,
        file_descriptors,
        threads,
    })
}

fn max_sample(left: ProcessSample, right: ProcessSample) -> ProcessSample {
    ProcessSample {
        rss_kib: left.rss_kib.max(right.rss_kib),
        cpu_percent: left.cpu_percent.max(right.cpu_percent),
        file_descriptors: left.file_descriptors.max(right.file_descriptors),
        threads: left.threads.max(right.threads),
    }
}
