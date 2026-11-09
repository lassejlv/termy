//! Displayed-surface intervals, not application render callbacks.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct FrameBudget {
    target_hz: u32,
    budget_ms: f64,
    intervals: usize,
    intervals_over_budget: usize,
    estimated_unfilled_refresh_slots: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SteadyPresentation {
    source: String,
    trim_each_end_ms: u32,
    displayed_frames: usize,
    fps: f64,
    p50_ms: f32,
    p95_ms: f32,
    p99_ms: f32,
    budgets: Vec<FrameBudget>,
}

pub(super) fn summarize(starts: &[u64]) -> Option<SteadyPresentation> {
    // Continuous benchmark scenarios run throughout this interior window.
    // Exclude launch/exit frames; report no samples for short/empty captures.
    let start = starts.first()?.checked_add(1_000_000_000)?;
    let end = starts.last()?.checked_sub(1_000_000_000)?;
    let frames: Vec<_> = starts
        .iter()
        .copied()
        .filter(|time| *time >= start && *time <= end)
        .collect();
    let span = frames.last()?.checked_sub(*frames.first()?)?;
    if span == 0 {
        return None;
    }
    let mut intervals: Vec<_> = frames
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .filter(|interval| *interval > 0)
        .collect();
    if intervals.is_empty() {
        return None;
    }
    intervals.sort_unstable();
    let budgets = [60, 120]
        .into_iter()
        .map(|hz| {
            let budget_ns = 1_000_000_000.0 / f64::from(hz);
            FrameBudget {
                target_hz: hz,
                budget_ms: budget_ns / 1_000_000.0,
                intervals: intervals.len(),
                // Allow 1 ms of compositor/timestamp jitter around a refresh.
                intervals_over_budget: intervals
                    .iter()
                    .filter(|&&ns| ns as f64 > budget_ns + 1_000_000.0)
                    .count(),
                estimated_unfilled_refresh_slots: intervals
                    .iter()
                    .map(|&ns| ((ns as f64 / budget_ns).round() as u64).saturating_sub(1))
                    .sum(),
            }
        })
        .collect();
    Some(SteadyPresentation {
        source: "xctrace/displayed-surfaces-interval".into(),
        trim_each_end_ms: 1000,
        displayed_frames: frames.len(),
        fps: (frames.len() - 1) as f64 * 1_000_000_000.0 / span as f64,
        p50_ms: percentile_nanos_to_millis(&intervals, 50, 100),
        p95_ms: percentile_nanos_to_millis(&intervals, 95, 100),
        p99_ms: percentile_nanos_to_millis(&intervals, 99, 100),
        budgets,
    })
}

pub(super) fn record(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut target = None;
    let mut output = None;
    let mut scenarios = Vec::new();
    let mut duration_secs = DEFAULT_DURATION_SECS;
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--target" => target = Some(BenchmarkTargetSpec::parse("candidate", &value)?),
            "--output" => output = Some(PathBuf::from(value)),
            "--scenario" => scenarios.push(Scenario::parse(&value)?),
            "--duration-secs" => duration_secs = value.parse().context("invalid duration")?,
            _ => bail!("unknown benchmark-record argument {arg}"),
        }
    }
    if duration_secs < 5 {
        bail!("presentation captures need at least 5 seconds");
    }
    let target = target.context("missing --target kind:/path")?;
    let output = output.context("missing --output")?;
    if output.exists() {
        bail!("output already exists: {}", output.display());
    }
    fs::create_dir_all(&output)?;
    let output = canonicalize_root(output)?;
    let driver = BenchmarkDriverSpec::current()?;
    build_release_driver(&driver)?;
    prepare_target(&target)?;
    if scenarios.is_empty() {
        scenarios = vec![
            Scenario::CjkScroll,
            Scenario::HeavyTui,
            Scenario::Resize,
            Scenario::Graphics,
        ];
    }
    let mut runs = Vec::new();
    for scenario in scenarios {
        runs.push(run_single_benchmark(
            &target,
            &driver,
            scenario,
            duration_secs,
            &output,
            None,
            true,
        )?);
        write_json(&output.join("presentation.json"), &runs)?;
    }
    if runs.iter().any(|run| {
        run.animation_summary.as_ref().is_none_or(|summary| {
            !matches!(
                summary.displayed_frame_capture_status,
                FrameCaptureStatus::Parsed
            ) || summary.displayed_frame_count < 2
                || (Scenario::parse(&run.scenario).is_ok_and(Scenario::is_continuous)
                    && summary.steady_presentation.is_none())
        })
    }) {
        bail!(
            "native presented-frame capture failed; diagnostics saved in {}",
            output.display()
        );
    }
    println!(
        "wrote native presentation measurements to {}",
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budgets_count_only_presented_intervals_and_trim_startup() {
        let starts: Vec<_> = (0..=300).map(|frame| frame * 16_666_667).collect();
        let summary = summarize(&starts).unwrap();
        assert!((summary.fps - 60.0).abs() < 0.01);
        assert_eq!(summary.budgets[0].intervals_over_budget, 0);
        assert_eq!(
            summary.budgets[1].intervals_over_budget,
            summary.budgets[1].intervals
        );
        assert_eq!(
            summary.budgets[1].estimated_unfilled_refresh_slots,
            summary.budgets[1].intervals as u64
        );
    }
    #[test]
    fn missing_and_short_captures_never_fabricate_fps() {
        assert!(summarize(&[]).is_none());
        assert!(summarize(&[0, 16_666_667]).is_none());
        assert!(summarize(&[0, 5_000_000_000]).is_none());
    }
}
