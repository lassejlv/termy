# Current Goal

## Objective
Build every committed phase in `ROADMAP.md` into a verified production release path for Tmon.

## Status
active

## Definition of done
- [ ] Phase 1: a clean, automated release-candidate gate, CI, pinned toolchain, product/release contract, licensing, and release evidence are implemented and verified.
- [x] Phase 2: terminal and mux trust boundaries, diagnostics, recovery, daemon/session lifecycle, unsafe-code review, and dependency gates are implemented and verified.
- [ ] Phase 3: packaged daily-driver fixtures, raw input contracts, font/TUI regressions, IME pre-edit, accessibility baseline, display/lifecycle coverage, and sustained soak evidence are implemented and verified.
- [ ] Phase 4: the exact release artifact is Developer ID signed, notarized, quarantine-installed, and runtime-validated on every declared OS/architecture, with honest release documentation and rollback evidence.
- [ ] Phase 5: manual/automatic update policy, N-1 upgrade and rollback with live sessions, privacy-safe support diagnostics, release/security operations, and a repeatable second candidate are implemented and verified.
- [ ] Every committed `ROADMAP.md` checkbox and exit criterion has authoritative evidence; unresolved external requirements are not relabeled as complete.

## Constraints
- Preserve the existing engine/render/app boundaries and unrelated working-tree changes.
- Keep unsafe code confined to the native PTY and FFI boundaries.
- Do not claim performance, compatibility, signing, notarization, accessibility, or upgrade behavior without matching live evidence.
- Do not expose credentials, terminal contents, clipboard data, commands, environment secrets, or private paths in release diagnostics.
- Do not publish artifacts or terminate users' daemon-owned sessions without explicit authorization.

## Progress
- Created and validated the repository-grounded five-phase production roadmap.
- Reconciled the stale completed PTY goal with the active roadmap implementation objective.
- Implemented the Phase 1 deterministic candidate command, macOS CI and manual candidate workflows,
  pinned Rust toolchain, release contract/checklist, dual-license texts, and dependency provenance
  policy.
- Produced a build-specific universal ad hoc local candidate whose evidence is deliberately dirty
  and non-publishable; the clean-tree publication guard was separately exercised.
- Added a schema-v3 native Metal gate with source identity, complete workload coverage, detected
  refresh budgets, scale/surface lifecycle work, and enforced idle/occlusion/inactive behavior.
- Completed the Phase 2 trust-boundary implementation: safe OSC 52 defaults, bounded terminal and
  protocol inputs, private peer-checked sockets, privacy-safe diagnostics and alerts, explicit
  lifecycle/upgrade behavior, unsafe-code enforcement, and an expiring dependency-advisory gate.
- Replaced unmaintained direct `bincode` use with bounded `postcard` decoding and bumped the mux
  protocol generation without touching older daemons.
- Ran all three decoder fuzz targets under libFuzzer for 30 seconds each without a crash, timeout,
  or RSS breach; added a pinned scheduled fuzz workflow and reproducible smoke script.
- Built Phase 3 fixtures with exact raw-PTY inputs, minimized Claude Code/OpenCode/cursor streams,
  retained reference screenshots, a Menlo/Monaco/Courier New font matrix, and complete IME pre-edit
  state/render/candidate-window handling.
- Added the documented accessibility-preview boundary, packaged daily-driver matrix, exact archive
  runtime smoke, privacy-safe support export, display/lifecycle workloads, and a resource-bounded
  30-minute soak gate.
- Diagnosed the soak's tab-close growth through per-close threads and then cross-thread reaper
  allocator retention. PTYs now tear down on the originating client-handler thread after daemon
  state is unlocked and the response is sent; a five-minute probe passes the steady-state policy.
- Added versioned snapshot fixtures and a live protocol-v2 to v3 coexistence/rollback harness that
  preserves detached sessions and avoids duplicate PTYs.

## Evidence
- `ROADMAP.md` contains five outcome-based phases, dependencies, observable exit criteria, risks, open decisions, and repository evidence.
- Current tree previously passed workspace tests, strict Clippy, formatting, diff checks, and the C ABI smoke test after the PTY consolidation.
- `bash script/release_candidate.sh` refused the dirty tree before building.
- `bash script/release_candidate.sh --allow-dirty` passed cargo-deny license/source checks,
  formatting, strict Clippy, 242 release tests (two manual renderer benchmarks ignored), the C ABI
  smoke, universal arm64/x86_64 packaging, metadata/resource/signature verification, and archive
  checksum verification.
- `dist/Tmon-0.1.0-10-release-evidence.json` records revision
  `1f57984df61b2ca54b069e3cf84d0b1286674e87`, Rust 1.96.0, archive SHA-256
  `6523addf45118fd072d846f205fc822c7dc1fe254eed77b8764d9593bfab210f`, `dirty: true`,
  ad hoc signing, and `publishable: false`.
- Both workflow files parse as YAML; an actual hosted GitHub Actions run remains unobserved.
- `performance/results/lifecycle-gate-smoke.json` and `lifecycle-gate-smoke2.json` are consecutive
  passing schema-v3 live Metal smokes under matching 120 Hz conditions; the final 30-sample pair is
  intentionally deferred until the authoritative soak releases the host.
- `release/evidence/packaged-smoke-build10.json` proves the exact checksum-verified archive starts a
  PTY, survives detach, reattaches without duplication, exports private diagnostics, and terminates
  explicitly. `release/evidence/upgrade-rollback-v2-v3-build10.json` proves internal generation
  coexistence/rollback while accurately leaving signed-production status false.
- The deterministic workspace suite passed 242 tests (two manual renderer benchmarks ignored), and
  focused mux, diagnostics, unsafe, dependency, fuzz compile, terminal-stream, raw-input, font, and
  IME checks pass.
- `security/fuzz-smoke-2026-09-04.md` records 161,618 terminal-feed, 2,732,270 snapshot-decode, and
  2,584,459 mux-frame inputs with zero detected failures.
- `performance/results/soak-same-thread-5m.json` records 2,496 tab-churn cycles, 9,600 KiB
  post-settle RSS growth, and zero descriptor/thread growth. The authoritative 30-minute run is in
  progress; the earlier rejected reports remain as diagnostic evidence.

## Next action
Finish the authoritative 30-minute soak, then run two uncontended 30-sample native Metal gates and
refresh the roadmap evidence. After that, the remaining work requires release identity/signing,
hosted CI, clean-install access, physical target Macs/displays, and recorded manual interaction.

## Blocker
None.
