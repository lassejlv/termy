# Engineering quality roadmap

**Purpose:** Make Termy cheap to change, safe to ship, and easy for contributors—without blocking v1.0 on perfection.

**Companion docs:** [quality-scorecard.md](quality-scorecard.md) · [terminal-view-decomposition.md](terminal-view-decomposition.md)

---

## Principles

1. **Enforce, don’t hope** — If it matters, CI or a script checks it (same philosophy as `check-boundaries.sh`).
2. **Parallel tracks** — Product phases ship features; engineering phases reduce tax on every feature.
3. **Sustainable cadence** — ~20% capacity on engineering; one decomposition tranche or test hardening item per release.
4. **v1.0 bar** — P0 engineering (E0) before tag; E1–E3 continue through and after v1.

---

## Timeline overview

```text
2026 Q2          2026 Q3          2026 Q4          2027 Q1
|----------------|----------------|----------------|----------------|
 E0 Pipeline     E1 modularity   E2 confidence   v1.0 tag
 trust            tranches 1–3     tmux/FFI        E3 operability
                  + file budgets   contracts       E4 scale (opt)
 Product:         Product:         Product:
 Phase 1          Phase 2–3        Phase 4–5
 blockers         parity/features  hardening/launch
```

---

## Phase E0 — Pipeline trust (P0, target: 2026 Q2)

**Outcome:** `just validate` ≈ CI; contributors cannot merge what CI would reject.

| ID | Initiative | Work | Exit criterion | Scorecard | Status |
|----|------------|------|----------------|-----------|--------|
| E0.1 | Workspace tests in CI | `workspace-tests` job in `architecture-checks.yml` | Green on every PR to `main` | G3 | Done |
| E0.2 | Format gate | `fmt` job + `just fmt-check` | No unformatted Rust on `main` | G4 | Done |
| E0.3 | Local parity | `just validate` | Documented in CONTRIBUTING | G5 | Done |
| E0.4 | PR definition of done | PR template checklist | Matches CI jobs by name | G12 | Done |
| E0.5 | Agent/contributor doc sync | `CLAUDE.md` paths | G11 review passes | G11 | Done |

**Status:** Merged to `main` ([#317](https://github.com/lassejlv/termy/pull/317)). Monitor CI for two weeks; revert gates if regressions appear.

**Explicitly not in E0:** 100% coverage, rewriting `render.rs`, new ADR process.

---

## Phase E1 — Modularity budget (P1, 2026 Q2–Q4)

**Outcome:** `terminal_view/` stops growing; god-files shrink on a schedule.

| ID | Initiative | Work | Exit criterion | Scorecard | Status |
|----|------------|------|----------------|-----------|--------|
| E1.1 | File size policy | `scripts/check-file-sizes.sh` + CI | G6: no regressions; allowlist shrinks | G6 | Done (allowlist=16) |
| E1.2 | Decomposition tranches | See [terminal-view-decomposition.md](terminal-view-decomposition.md) | `mod.rs` &lt; 1,500 lines; `render/` directory | G6 | Partial |
| E1.3 | Complexity discipline | No new `#[allow(clippy::cognitive_complexity)]` without issue | Review guideline in CONTRIBUTING | — | Done |

**Capacity:** One tranche per month maximum; never combine tranche 4 with a large product feature in the same PR.

---

## Phase E2 — Confidence layer (P1–P2, 2026 Q3–2027 Q1)

**Outcome:** Failures in tmux, FFI, and config surface in automation—not after release.

| ID | Initiative | Work | Exit criterion | Scorecard | Status |
|----|------------|------|----------------|-----------|--------|
| E2.1 | Tmux CI reliability | macOS job: `brew install tmux`; fail if &lt; 3.3; always run `just test-tmux-integration` | G7: job fails if tests fail, not skip silently | G7 | Done |
| E2.2 | Test pyramid doc | `docs/engineering/testing.md`: unit → integration → manual | Linked from CONTRIBUTING | — | Done |
| E2.3 | FFI contract tests | Minimal C API round-trips in `crates/core/src/ffi` tests | Run on Linux + macOS in CI | — | Partial |
| E2.5 | Ignore audit | Every `#[ignore]` has issue URL; quarterly cleanup | ≤10 ignored tests repo-wide | — | Partial |
| E2.6 | Stress harness | Scripted tab storm + scrollback (product Phase 4) | Documented scenario; optional CI nightly | — | Planned |

**Aligns with product roadmap Phase 4** (stress tests, scrollback validation).

---

## Phase E3 — Operability (P2, 2026 Q4–2027 Q1)

**Outcome:** Crashes and perf regressions are visible and attributable.

| ID | Initiative | Work | Exit criterion | Scorecard | Status |
|----|------------|------|----------------|-----------|--------|
| E3.1 | Crash log on panic | File + path documented for support | G10 | G10 | Done |
| E3.2 | Startup error UI | Replace startup `.unwrap()` with dialog (product Phase 4) | No silent exit on window create failure | G10 | Done |
| E3.3 | Perf CI budgets | `macos-performance.yml` fails on regression % | G9: documented thresholds | G9 | Planned |
| E3.4 | Render metrics smoke | Optional job: cursor-blink scenario, `full ≈ 0` | Documented in `docs/development.md` | — | Done |
| E3.5 | Dependency audit | `cargo deny` or license check in release workflow | Product Phase 5 item | — | Done |

---

## Phase E4 — Scale the team (P3, post-v1 or high PR volume)

| ID | Initiative | Work | Exit criterion | Status |
|----|------------|------|----------------|--------|
| E4.1 | ADRs | `docs/architecture/adr/` for GPUI pin, FFI boundary, tmux model | Template + 3 ADRs | Planned |
| E4.2 | CODEOWNERS | `terminal_view/`, `config_core/`, `ffi/` | Auto-review requests | Planned |
| E4.3 | Crate onboarding | Each crate README: owner, test command, forbidden deps | 100% workspace members | Done |
| E4.4 | Issue taxonomy | Labels: `area/*`, `risk/*`, `quality-gate` | Used in roadmap reviews | Planned |

---

## Test commands (target state)

| Scope | Command |
|-------|---------|
| Desktop app | `just test` → `cargo test -p termy --release` |
| Workspace | `just test-workspace` → `cargo test --workspace --release` |
| Tmux integration | `just test-tmux-integration` |
| Full local gate | `just validate` |

---

## Operating rhythm

| Cadence | Activity |
|---------|----------|
| **Per PR** | Run smallest subset from CONTRIBUTING; never increase allowlisted file sizes without tranche plan |
| **Per release** | One E1 tranche *or* one E2 hardening item |
| **Monthly** | Update [quality-scorecard.md](quality-scorecard.md) (15 min) |
| **Quarterly** | Roadmap review: defer stale items; sync with the product milestones product phases |

---

## Capacity model

| Track | Suggested share | Notes |
|-------|-----------------|-------|
| Product (features, bugs, platforms) | ~70–80% | Drives revenue and v1 narrative |
| Engineering (E0–E4) | ~20–30% | Spikes before multi-window, OSC, agent workspace |
| **Rule** | E0 complete before v1.0 tag | E1 can continue through v1.1 |

---

## Issue labels (recommended)

Create GitHub labels for tracking:

- `roadmap:engineering` — ties to Ex.y in this doc
- `quality-gate` — scorecard regression

---

## Related

- [Quality scorecard](quality-scorecard.md)
- [Development / render metrics](../development.md)
