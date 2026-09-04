# Changelog

All notable user-visible changes to Tmon are recorded here. The project follows semantic versions
for the application; the pre-1.0 C ABI remains experimental unless a release says otherwise.

## Unreleased

### Added

- Production roadmap and release-candidate contract.
- Pinned Rust toolchain and macOS CI/release-candidate workflows.
- Privacy-safe local diagnostics, a reviewable support-bundle command, and native recovery alerts.
- Exact raw-input, external-TUI stream, snapshot compatibility, font, IME, mux lifecycle, packaged
  runtime, sustained-soak, and N-1/N rollback fixtures or harnesses.
- Visible IME pre-edit and candidate-window positioning in terminal and search input.
- Manual checksummed update/rollback, accessibility-preview, support, and security contracts.

### Changed

- Native PTY ownership is consolidated under `engine::pty`.
- Terminal rendering preserves monochrome Claude Code markers, OpenCode spinner cell geometry, and
  a Tokyo Night-compatible opaque block cursor.
- Snapshot and multiplexer encoding uses bounded postcard data under protocol generation 3; older
  daemons remain discoverable and isolated until their sessions are deliberately drained.
- Closed mux tabs are removed while daemon state is locked, then their PTYs are destroyed on the
  originating client-handler thread only after the lock is released and the response is sent. This
  avoids per-close thread churn, queue deadlocks, and cross-thread allocator retention.
- The native Metal benchmark now measures scale rebuild and surface recreation, records source and
  protocol identity, and exits nonzero on incomplete coverage, budget failure, or hidden-window
  work.

### Security

- OSC 52 clipboard writes are disabled by default and can be enabled only for the active tab, with
  an optional focused-window requirement.
- Terminal metadata/actions, paste, snapshots, local protocol frames, socket ownership, and
  diagnostic records have explicit size, provenance, and privacy boundaries with adversarial tests.
- Pinned RustSec, license/source, unsafe-boundary, and decoder-fuzz gates are part of the candidate
  workflow. Signing, notarization, clean-install, and cross-machine evidence remain release blockers.

## 0.1.0 - Unreleased

Initial native macOS daily-driver candidate. No public production artifact has been declared.
