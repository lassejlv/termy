# Fuzz smoke evidence — 2026-09-04

This is local working-tree evidence, not publishable release evidence. It proves the current three
targets executed under instrumentation without a detected crash, per-input timeout, or RSS-limit
breach. The clean release candidate must repeat the same gate.

## Environment

- `cargo-fuzz 0.13.2`
- `rustc 1.100.0-nightly (a69a63265 2026-09-03)`
- Apple Silicon macOS host
- 30 seconds per target, five-second per-input timeout, 2 GiB RSS limit
- libFuzzer default maximum generated input: 4096 bytes; deterministic unit tests separately cover
  the exact larger parser, snapshot, and protocol limits

## Results

| Target | Executed inputs | Added corpus units | Peak RSS | Result |
| --- | ---: | ---: | ---: | --- |
| `terminal-feed` | 161,618 | 1,950 | 362 MiB | pass |
| `snapshot-decode` | 2,732,270 | 3,168 | 571 MiB | pass |
| `mux-frame-decode` | 2,584,459 | 197 | 470 MiB | pass |

The runs used the exact limits encoded by `script/fuzz_smoke.sh`. Generated corpora and build
artifacts are ignored so an unreviewed random input cannot silently become part of the release
contract. Any future crash must first be minimized and promoted to a deterministic regression test.
