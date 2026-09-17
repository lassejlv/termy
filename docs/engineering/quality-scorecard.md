# Quality scorecard

**North star:** Every standard that matters is enforced by CI or a script—not by memory.

Update the **Status** and **Last verified** columns at the start of each month (or when a gate flips). A gate counts as **met** only after it has stayed green for two weeks on `main`.

Baseline audit: **2026-06-01** (app version **0.3.0**).

---

## Scorecard

| ID | Gate | Target | Status | Last verified | Enforced by |
|----|------|--------|--------|---------------|-------------|
| G1 | Workspace Clippy | `-D warnings` on all targets | Met | 2026-06-01 | `architecture-checks.yml` |
| G2 | Crate boundaries | No forbidden deps; generated docs in sync | Met | 2026-06-01 | `scripts/check-boundaries.sh` |
| G3 | Workspace unit tests in CI | `cargo test --workspace` on every PR | Met | 2026-06-01 | `architecture-checks.yml` `workspace-tests` |
| G4 | Formatting | `cargo fmt --check` on every PR | Met | 2026-06-01 | `architecture-checks.yml` `fmt` |
| G5 | Local CI parity | `just validate` matches PR CI | Met | 2026-06-01 | `justfile` |
| G6 | Max file size | No `.rs` file &gt; 1,500 lines; no new files &gt; 800 without ADR | Partial | 2026-08-20 | `scripts/check-file-sizes.sh` (allowlist is the source of truth; shrink over time) |
| G7 | Tmux integration | CI installs tmux ≥ 3.3; ignored tests run on macOS | Met | 2026-06-01 | `architecture-checks.yml` (fail if tmux &lt; 3.3) |
| G9 | Perf regression | Benchmark gates on macOS perf workflow | Partial | 2026-06-01 | `macos-performance.yml` |
| G10 | Crash visibility | Panic writes crash log; user-visible startup failure | Partial | 2026-06-13 | Runtime panic hook + startup alert; watch `main` before Met |
| G11 | Contributor docs | `CLAUDE.md` + `CONTRIBUTING.md` match `crates/desktop_app` layout | Partial | 2026-06-13 | Manual review + `scripts/check-boundaries.sh` README metadata |
| G12 | PR definition of done | Template checklist mirrors CI | Met | 2026-06-01 | `.github/PULL_REQUEST_TEMPLATE.md` |

**Current score:** 7/11 met · **M0 (pipeline trust)** reached with merge of #317 · **Target for v1.0:** 9/11 met

---

## Pillar targets (qualitative)

| Pillar | 10/10 looks like |
|--------|------------------|
| Correctness | Regressions caught in CI before merge; tmux/FFI not optional luck |
| Architecture | Boundaries stay enforced; extractions go to the right crate |
| Maintainability | Median UI PR touches &lt;3 files; no growing god-files |
| Contributor UX | Clone → `just validate` green in &lt;30 minutes on a typical laptop |
| Operability | Perf and crash issues produce artifacts, not anecdotes |
| Documentation | Product vs contributor vs generated docs have one owner each |

---

## When to flip a gate

1. Implement the enforcement (workflow, script, or `xtask`).
2. Land on `main` and watch for two weeks without exemption.
3. Update **Status** → Met and **Last verified** date.
4. If the gate regresses, revert to **Not met** and open a tracking issue labeled `quality-gate`.

---

## Related

- [Engineering roadmap](roadmap.md)
- [Project layout](../architecture/project-layout.md)
