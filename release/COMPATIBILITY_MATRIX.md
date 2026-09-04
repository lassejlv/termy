# Tmon release compatibility record

Copy this template for each public candidate and replace every `not-run` cell with `pass`, `fail`,
`blocked`, or `unsupported`. A cross-build is never a runtime pass. Link only reviewed evidence that
belongs to the exact archive named below.

## Candidate identity

| Field | Value |
| --- | --- |
| Application version/build | not-recorded |
| Source revision / clean | not-recorded |
| Archive / SHA-256 | not-recorded |
| Bundle identifier | not-recorded |
| Developer ID Team ID | not-recorded |
| Notary submission / staple / Gatekeeper | not-recorded |

## Runtime targets

| OS build | Architecture | Hardware | Package launch | Core PTY/tabs | Evidence | Status |
| --- | --- | --- | --- | --- | --- | --- |
| Current reference macOS | arm64 | Current M-series reference | not-run | not-run | none | not-run |
| macOS 15 latest patch | arm64 | named host | not-run | not-run | none | not-run |
| macOS 14 latest patch | arm64 | named host | not-run | not-run | none | not-run |
| macOS 14+ | x86_64 | named Intel host | not-run | not-run | none | not-run |

Delete or mark unsupported any row the release owner cannot rerun. Record the exact OS build,
hardware model, and host architecture when executing it.

## Daily-driver matrix

| Exercise | Target/versions | Font, scale, refresh, layout/IME | Evidence | Status |
| --- | --- | --- | --- | --- |
| Automated archive/PTY detach/reattach/support/terminate smoke | exact archive | isolated defaults | none | not-run |
| Login zsh and bash | not-recorded | not-recorded | none | not-run |
| SSH disposable host | not-recorded | not-recorded | none | not-run |
| tmux split/copy/resize/detach | not-recorded | not-recorded | none | not-run |
| Vim and Neovim insert/search/split/scroll | not-recorded | not-recorded | none | not-run |
| `less -R` wrapped Unicode/search | not-recorded | not-recorded | none | not-run |
| Full-screen mouse TUI | not-recorded | not-recorded | none | not-run |
| Claude Code record marker/stream/interrupt | not-recorded | not-recorded | none | not-run |
| OpenCode animated footer spinner | not-recorded | not-recorded | none | not-run |
| IME terminal/search compose/cancel/commit | not-recorded | not-recorded | none | not-run |
| Clipboard, bracketed paste, OSC 52, HTTPS link | not-recorded | not-recorded | none | not-run |
| Native tab create/switch/reorder/close/restore | not-recorded | not-recorded | none | not-run |

## Display and lifecycle matrix

| Event | Displays/system setup | Evidence | Status |
| --- | --- | --- | --- |
| Enforced 30-sample native Metal run 1 | not-recorded | none | not-run |
| Enforced 30-sample native Metal run 2 | not-recorded | none | not-run |
| Retina to non-Retina move and return | not-recorded | none | not-run |
| 60 Hz to highest refresh and return | not-recorded | none | not-run |
| 30-second sleep/wake with active output | not-recorded | none | not-run |
| 30-second minimize/occlusion with active output | not-recorded | none | not-run |
| External-display disconnect/reconnect | not-recorded | none | not-run |
| Rapid full-screen/surface reconfiguration | not-recorded | none | not-run |
| 30-minute sustained soak | current reference host | none | not-run |

## Release operations

| Check | Evidence | Status |
| --- | --- | --- |
| Clean standard-user quarantine install | none | not-run |
| Signed N-1 to N live-session upgrade and rollback | none | not-run |
| Uninstall and user-approved data cleanup | none | not-run |
| Support channel smoke | none | not-run |
| Private security-channel smoke | none | not-run |

Failures must include the smallest sanitized reproduction and move into a deterministic fixture when
possible. `blocked` must name the missing hardware, credential, host, or application; it is not a
pass. Review all screenshots, streams, support JSON, and logs before sharing them.
