use super::{
    ECHO_TRAIN_DEFAULT_ITERATIONS, FrameEvent, MarkerEvent, echo_train_offset,
    summarize_echo_train_latency,
};

// Model a display that presents on its next refresh. Shifting an otherwise
// identical app's launch must not turn display phase into a regression.
fn simulated_p95(refresh_hz: u64, launch_phase_ns: u64, processing_ns: u64) -> f32 {
    let frame_ns = 1_000_000_000 / refresh_hz;
    let markers: Vec<_> = (0..ECHO_TRAIN_DEFAULT_ITERATIONS)
        .map(|seq| MarkerEvent {
            kind: "echo_start".into(),
            seq: Some(seq),
            monotonic_ns: launch_phase_ns + echo_train_offset(seq).as_nanos() as u64,
        })
        .collect();
    let frames: Vec<_> = markers
        .iter()
        .enumerate()
        .map(|(index, marker)| FrameEvent {
            monotonic_ns: (marker.monotonic_ns + processing_ns).div_ceil(frame_ns) * frame_ns,
            elapsed_ms: 0,
            total_frames: index as u64 + 1,
            terminal_redraws: index as u64 + 1,
            view_wake_signals: index as u64 + 1,
            runtime_wakeups: index as u64 + 1,
        })
        .collect();
    summarize_echo_train_latency(&markers, &frames)
        .unwrap()
        .echo_first_frame_ms_p95
        .unwrap()
}

#[test]
fn echo_sampling_covers_display_phases() {
    for refresh_hz in [60, 120] {
        let mut minimum = f32::MAX;
        let mut maximum = f32::MIN;
        for phase in (0..1_000_000_000 / refresh_hz).step_by(100_000) {
            let p95 = simulated_p95(refresh_hz, phase, 100_000);
            minimum = minimum.min(p95);
            maximum = maximum.max(p95);
        }
        assert!(
            maximum - minimum < 2.0,
            "{refresh_hz} Hz launch phase changed p95 by {:.2} ms",
            maximum - minimum,
        );
    }
}

#[test]
fn phase_coverage_still_detects_a_real_latency_regression() {
    for refresh_hz in [60, 120] {
        let baseline = simulated_p95(refresh_hz, 0, 100_000);
        let candidate = simulated_p95(refresh_hz, 7_000_000, 20_100_000);
        assert!(candidate - baseline > 18.0);
    }
}
