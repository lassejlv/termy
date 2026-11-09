//! Repeated, alternating baseline/candidate runs.
//!
//! One run per side lets a single scheduler stall on a hosted runner decide a
//! latency gate: echo-train's p95 is the second-slowest of 40 echoes, so two
//! slow frames on one side are enough. Pairs alternate which side runs first,
//! so warm-up or thermal drift cannot favor one side, and the gated deltas are
//! the per-metric median of the per-pair deltas, so one bad pair cannot fail
//! (or pass) the comparison on its own.
use super::*;

/// Which side runs first in the zero-based `pair`: the baseline in even pairs,
/// the candidate in odd ones.
pub(super) fn pair_order<'a, T>(pair: usize, baseline: &'a T, candidate: &'a T) -> [&'a T; 2] {
    if pair.is_multiple_of(2) {
        [baseline, candidate]
    } else {
        [candidate, baseline]
    }
}

/// Raw-output subdirectory for a run. A single pair keeps the historical
/// `raw/<label>/<scenario>` layout; repeated pairs get `pair-NN` below it.
pub(super) fn pair_dir(pair: usize, pair_count: usize) -> Option<String> {
    (pair_count > 1).then(|| format!("pair-{:02}", pair + 1))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ScenarioPair {
    pub(super) baseline: RunResult,
    pub(super) candidate: RunResult,
    pub(super) deltas: ScenarioDeltas,
}

impl ScenarioComparison {
    /// Compares `pairs` (baseline, candidate) runs of one scenario. With more
    /// than one pair, `deltas` holds per-metric medians, `baseline` and
    /// `candidate` show the first pair, and every pair is kept in `pairs`.
    pub(super) fn from_pairs(scenario: String, mut runs: Vec<(RunResult, RunResult)>) -> Self {
        assert!(!runs.is_empty(), "a scenario comparison needs a run pair");
        if runs.len() == 1 {
            let (baseline, candidate) = runs.remove(0);
            return Self::new(scenario, baseline, candidate);
        }
        let pairs: Vec<ScenarioPair> = runs
            .into_iter()
            .map(|(baseline, candidate)| {
                let deltas =
                    Self::new(scenario.clone(), baseline.clone(), candidate.clone()).deltas;
                ScenarioPair {
                    baseline,
                    candidate,
                    deltas,
                }
            })
            .collect();
        let deltas = median_deltas(pairs.iter().map(|pair| &pair.deltas));
        Self {
            scenario,
            baseline: pairs[0].baseline.clone(),
            candidate: pairs[0].candidate.clone(),
            deltas,
            pairs,
        }
    }

    /// Every run behind this comparison, labeled for gate messages.
    pub(super) fn labeled_runs(&self) -> Vec<(String, &RunResult)> {
        if self.pairs.is_empty() {
            return vec![
                ("baseline".to_string(), &self.baseline),
                ("candidate".to_string(), &self.candidate),
            ];
        }
        self.pairs
            .iter()
            .enumerate()
            .flat_map(|(index, pair)| {
                [
                    (format!("baseline (pair {})", index + 1), &pair.baseline),
                    (format!("candidate (pair {})", index + 1), &pair.candidate),
                ]
            })
            .collect()
    }
}

fn median_deltas<'a>(deltas: impl Iterator<Item = &'a ScenarioDeltas> + Clone) -> ScenarioDeltas {
    let f = |metric: fn(&ScenarioDeltas) -> Option<f32>| median_f32(deltas.clone().map(metric));
    let i = |metric: fn(&ScenarioDeltas) -> Option<i64>| median_i64(deltas.clone().map(metric));
    ScenarioDeltas {
        frame_p50_ms: f(|d| d.frame_p50_ms),
        frame_p95_ms: f(|d| d.frame_p95_ms),
        frame_p99_ms: f(|d| d.frame_p99_ms),
        fps_avg: f(|d| d.fps_avg),
        hitch_max_ms: f(|d| d.hitch_max_ms),
        activity_monitor_cpu_percent: f(|d| d.activity_monitor_cpu_percent),
        idle_wakeups: i(|d| d.idle_wakeups),
        hitch_count: i(|d| d.hitch_count),
        memory_bytes: i(|d| d.memory_bytes),
        disk_bytes_read: i(|d| d.disk_bytes_read),
        disk_bytes_written: i(|d| d.disk_bytes_written),
        idle_burst_first_frame_ms: f(|d| d.idle_burst_first_frame_ms),
        idle_burst_last_frame_ms: f(|d| d.idle_burst_last_frame_ms),
        idle_burst_frames_until_settle: i(|d| d.idle_burst_frames_until_settle),
        echo_first_frame_ms_p95: f(|d| d.echo_first_frame_ms_p95),
        echo_first_frame_ms_max: f(|d| d.echo_first_frame_ms_max),
        echo_missed_count: i(|d| d.echo_missed_count),
    }
}

/// Median of the values, or `None` if any pair lacks the metric, so a
/// required gate still reports a missing measurement instead of passing.
fn median_f32(values: impl Iterator<Item = Option<f32>>) -> Option<f32> {
    let mut values: Vec<f32> = values.collect::<Option<_>>()?;
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}

fn median_i64(values: impl Iterator<Item = Option<i64>>) -> Option<i64> {
    let mut values: Vec<i64> = values.collect::<Option<_>>()?;
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]).div_euclid(2)
    } else {
        values[mid]
    })
}

/// Report section listing each pair's gated deltas next to their medians.
pub(super) fn render_pairs(scenario: &ScenarioComparison) -> String {
    if scenario.pairs.is_empty() {
        return String::new();
    }
    let mut report = format!(
        "Deltas are medians over {} alternating pairs (odd pairs run the baseline first); \
         the Baseline and Candidate columns show pair 1.\n\n",
        scenario.pairs.len()
    );
    report.push_str("| Pair | First | CPU delta % | Memory delta | Echo p95 delta ms |\n");
    report.push_str("| ---: | --- | ---: | ---: | ---: |\n");
    for (index, pair) in scenario.pairs.iter().enumerate() {
        report.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            index + 1,
            pair_order(index, &"baseline", &"candidate")[0],
            format_option_f32(pair.deltas.activity_monitor_cpu_percent),
            format_option_i64(pair.deltas.memory_bytes),
            format_option_f32(pair.deltas.echo_first_frame_ms_p95),
        ));
    }
    report.push_str(&format!(
        "| median | | {} | {} | {} |\n\n",
        format_option_f32(scenario.deltas.activity_monitor_cpu_percent),
        format_option_i64(scenario.deltas.memory_bytes),
        format_option_f32(scenario.deltas.echo_first_frame_ms_p95),
    ));
    report
}

#[cfg(test)]
mod tests {
    use super::super::tests::{compared_target, run_result};
    use super::super::{
        BenchmarkGateThresholds, ComparisonSummary, EchoTrainLatencySummary, RunResult,
        ScenarioComparison, render_report,
    };
    use super::{median_f32, median_i64, pair_dir, pair_order};

    fn echo_run(label: &str, p95: f32) -> RunResult {
        let mut run = run_result(label, 3.0, 10, 10 * 1024 * 1024);
        run.scenario = "echo-train".to_string();
        run.micro_latency.echo_train = Some(EchoTrainLatencySummary {
            echo_first_frame_ms_p50: Some(12.0),
            echo_first_frame_ms_p95: Some(p95),
            echo_first_frame_ms_p99: Some(p95),
            echo_first_frame_ms_max: Some(p95),
            echo_missed_count: 0,
            echo_sample_count: 40,
        });
        run
    }

    fn echo_failures(pairs: &[(f32, f32)]) -> Vec<String> {
        let runs = pairs
            .iter()
            .map(|(baseline, candidate)| {
                (
                    echo_run("baseline", *baseline),
                    echo_run("candidate", *candidate),
                )
            })
            .collect();
        let summary = ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![ScenarioComparison::from_pairs("echo-train".into(), runs)],
        };
        BenchmarkGateThresholds::default()
            .failures(&summary)
            .into_iter()
            .filter(|failure| failure.contains("echo first-frame p95"))
            .collect()
    }

    #[test]
    fn the_gate_uses_the_median_pair() {
        // #455 attempt 1: the candidate drew two stalls, so its p95 was 32 ms.
        let stalled = (19.0, 32.1);
        assert_eq!(echo_failures(&[stalled]).len(), 1);
        let others = [(18.1, 18.4), (18.6, 18.8), (17.3, 18.0), (18.9, 18.2)];
        let mut five = vec![stalled];
        five.extend(others);
        assert!(
            echo_failures(&five).is_empty(),
            "{:?}",
            echo_failures(&five)
        );
        // A regression present in every pair still fails.
        let regressed: Vec<_> = five.iter().map(|(b, _)| (*b, b + 15.0)).collect();
        assert_eq!(echo_failures(&regressed).len(), 1);
    }

    #[test]
    fn the_report_lists_each_pair() {
        let runs = [(19.0, 32.1), (18.1, 18.4), (18.6, 18.8)]
            .iter()
            .map(|(baseline, candidate)| {
                (
                    echo_run("baseline", *baseline),
                    echo_run("candidate", *candidate),
                )
            })
            .collect();
        let scenario = ScenarioComparison::from_pairs("echo-train".into(), runs);
        assert_eq!(scenario.labeled_runs().len(), 6);
        let report = render_report(&ComparisonSummary {
            baseline: compared_target("baseline"),
            candidate: compared_target("candidate"),
            scenarios: vec![scenario],
        });
        assert!(report.contains("medians over 3 alternating pairs"));
        assert!(report.contains("| 2 | candidate |"));
        assert!(report.contains("| median | |"));
    }

    #[test]
    fn pairs_alternate_which_side_runs_first() {
        let orders: Vec<_> = (0..4)
            .map(|pair| *pair_order(pair, &"b", &"c")[0])
            .collect();
        assert_eq!(orders, ["b", "c", "b", "c"]);
    }

    #[test]
    fn a_single_pair_keeps_the_historical_raw_layout() {
        assert_eq!(pair_dir(0, 1), None);
        assert_eq!(pair_dir(0, 5).as_deref(), Some("pair-01"));
        assert_eq!(pair_dir(4, 5).as_deref(), Some("pair-05"));
    }

    #[test]
    fn one_stalled_pair_does_not_move_the_median() {
        // #455's echo-train: identical code, one pair with two stalled echoes.
        let p95_deltas = [13.13, -0.4, 0.7, 1.1, -0.9].map(Some);
        assert_eq!(median_f32(p95_deltas.into_iter()), Some(0.7));
        // A real regression shows up in every pair and still fails a 10 ms gate.
        let regression = [18.2, 19.5, 17.9, 21.0, 18.8].map(Some);
        assert!(median_f32(regression.into_iter()).unwrap() > 10.0);
    }

    #[test]
    fn a_missing_measurement_stays_missing() {
        assert_eq!(median_f32([Some(1.0), None, Some(2.0)].into_iter()), None);
        assert_eq!(median_i64([Some(1), None].into_iter()), None);
        assert_eq!(median_i64(std::iter::empty()), None);
    }

    #[test]
    fn even_counts_average_the_middle_values() {
        assert_eq!(
            median_f32([4.0, 1.0, 3.0, 2.0].map(Some).into_iter()),
            Some(2.5)
        );
        assert_eq!(median_i64([4, 1, 3, 2].map(Some).into_iter()), Some(2));
        assert_eq!(median_i64([-3, -2].map(Some).into_iter()), Some(-3));
    }
}
