# Engineering docs

Contributor-facing plans for codebase quality, CI parity, and long-term maintainability.

| Document | Purpose |
|----------|---------|
| [roadmap.md](roadmap.md) | Engineering track (E0–E4): phases, exit criteria, quarterly plan |
| [quality-scorecard.md](quality-scorecard.md) | Measurable gates for a 10/10 codebase; update monthly |
| [terminal-view-decomposition.md](terminal-view-decomposition.md) | Module extraction plan for `terminal_view/` |


## Quick commands

```sh
just validate          # Local pass closest to target CI (see roadmap E0)
just check-boundaries  # Crate deps + generated docs
just test              # Desktop app tests (release)
just test-workspace    # All workspace crate tests (release)
```
