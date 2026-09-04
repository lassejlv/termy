# Sustained soak gate

`bash script/soak_gate.sh` runs an isolated release-mode multiplexer and real native PTYs for 30
minutes by default. It continuously fills scrollback, creates and terminates secondary tab process
groups, changes PTY geometry, detaches and reattaches the client, and inserts short idle periods.

The harness first fills the 5,000-row scrollback at its largest exercised geometry, performs one tab
close and detach/reattach, and only then records its steady-state baseline. The JSON report records
cycles, bytes, detach/reattach, tab churn, resize count, long idle periods, CPU, RSS, open file
descriptors, threads, daemon count, one-second resource samples, and the exact applied limits without
recording terminal text, commands, environment, or paths. The gate allows at most five minutes (or
the first quarter of a shorter run) for allocator/serialization caches to settle. It fails if total
RSS exceeds 256 MiB, total post-warmup RSS growth exceeds 64 MiB, post-settle RSS growth exceeds 16
MiB, sampled process CPU exceeds 100% of one core, descriptors grow by more than 16, threads grow by
more than eight, or any lifecycle/output invariant fails. The total and steady-state rules prevent
both a large bounded footprint and a slow leak from being hidden by a single baseline.

Use a short run only to validate harness mechanics:

```sh
bash script/soak_gate.sh --seconds 15 --output performance/results/soak-smoke.json
```

A production candidate requires the default 30-minute run plus the manual packaged-app sleep/wake
and display checks in `PACKAGED_SMOKE.md`. The headless soak cannot truthfully simulate macOS sleep,
display disconnect, compositor occlusion, or a Metal surface loss.

The rejected evidence is retained because it drove two lifecycle fixes. The original tab-close
path spawned one OS thread per PTY cleanup and accumulated macOS thread/allocator pages under
thousands of closes; `performance/results/release-candidate-soak-thread-churn-failed.json` records
that failure. A bounded single reaper reduced the focused five-minute post-settle slope to 864 KiB,
but its first 30-minute run deadlocked during final shutdown when a full cleanup queue performed a
blocking send while holding daemon state. A read-only stack sample confirmed the wait chain; the
interrupted run intentionally produced no passing JSON.

Non-blocking queue admission removed that deadlock, but the completed follow-up in
`performance/results/release-candidate-soak-cross-thread-reaper-failed.json` still failed the
steady-state policy: PTYs were allocated on client-handler threads and freed by the reaper, causing
27,552 KiB of post-settle allocator retention despite zero descriptor or thread growth. The final
design removes the reaper. A close now removes and reindexes the tab while holding daemon state,
sends the response after releasing that lock, and destroys the returned PTY on the originating
client-handler thread. The real-PTY test
`closed_tab_cleanup_is_returned_after_releasing_daemon_state` protects the lock boundary. The
focused same-thread run in `performance/results/soak-same-thread-5m.json` completed 2,496 cycles
with 9,600 KiB post-settle RSS growth and zero descriptor or thread growth; the default 30-minute
candidate gate remains the authoritative acceptance result.
