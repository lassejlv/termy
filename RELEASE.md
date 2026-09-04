# Tmon release contract

This document is the operational contract for release candidates. `ROADMAP.md` remains the source
of truth for production-readiness scope; this file describes what the current tooling can enforce.

## Current release model

- Distribution is a direct-download macOS `.app` in a versioned zip.
- Public artifacts must use a Developer ID Application signature, hardened runtime, Apple
  notarization, and a stapled ticket. Ad hoc signatures are internal-only.
- The current bundle identifier is `com.tmon.app`. It must be confirmed as the permanent identifier
  before the first public signed release.
- The declared deployment target is macOS 14.0. Runtime support is proven only on machines listed in
  the release's compatibility record; a universal cross-build alone is not support evidence.
- The default artifact contains arm64 and x86_64 slices while Intel remains in the declared matrix.
- The workspace version is the user-facing semantic version. `TMON_BUILD_NUMBER` supplies the
  monotonic `CFBundleVersion` and must be set deliberately for public candidates.
- Updates are manual and checksum-verified for the first release. No automatic update feed is
  currently trusted.
- `crates/ffi` is experimental before 1.0. Its explicit ABI version and tests prevent accidental
  layout drift, but semantic compatibility is not promised until a stable tier is declared.
- Sessions persist across app detach/relaunch within one login session. Each incompatible protocol
  uses a different socket, so a newer app never kills an older daemon. Read-only discovery and
  explicit current-generation termination are documented in `SESSION_LIFECYCLE.md`; automatic
  cross-protocol migration and reboot restoration are not promised.

## Deterministic candidate gate

Run from a clean revision:

```sh
TMON_BUILD_NUMBER=1 bash script/release_candidate.sh
```

The gate refuses a dirty worktree unless `--allow-dirty` is passed for local development. An
allow-dirty artifact is recorded as dirty evidence and must never be published.

The gate performs:

1. RustSec advisory checks with `cargo-audit 0.22.2` plus dependency license and source-policy
   checks with `cargo-deny 0.19.0`;
2. formatting and strict Clippy checks;
3. release-mode workspace tests;
4. the real C ABI consumer smoke test;
5. a universal release build and bundle verification;
6. archive checksum verification; and
7. a machine-readable evidence report under `dist/`.

Bundle verification checks both Mach-O slices' real deployment target rather than trusting only the
plist, requires the hardened-runtime signature flag, and validates architecture, version/build,
bundle identifier, icon, licenses, and release documents.
Archive and checksum filenames contain both semantic version and `CFBundleVersion`, so evidence for
two candidates at the same version cannot silently alias an overwritten zip. The candidate command
also refuses to reuse an existing version/build archive, checksum, or evidence path; increment
`TMON_BUILD_NUMBER` instead of replacing prior evidence.

Install the pinned release-only tools once with
`cargo install cargo-audit --version 0.22.2 --locked` and
`cargo install cargo-deny --version 0.19.0 --locked`. CI installs the same versions. The complete
publication checklist is in `RELEASE_CHECKLIST.md`; `DEPENDENCY_AUDIT.md` records temporary,
expiring RustSec exceptions; and `THIRD_PARTY_LICENSES.md` defines the dependency and notice policy.

CI uses an ad hoc signature because it has no signing credentials. Signing and notarization are a
separate public-release gate:

```sh
TMON_BUILD_NUMBER=1 \
TMON_SIGN_IDENTITY="Developer ID Application: ..." \
bash script/release_candidate.sh

APPLE_NOTARY_PROFILE=tmon bash script/notarize_macos.sh
```

Never print, copy into the repository, or place Apple credentials in command arguments. The notary
profile belongs in the login keychain and CI secrets require a separately reviewed workflow.
The notarization command refuses to overwrite an existing version/build record and retains both
Apple's raw JSON response and a distribution record containing the source revision/dirty state,
Team ID, accepted submission ID, staple/Gatekeeper result, archive name, and final SHA-256 under
`release/evidence` (or the directory named by `TMON_RELEASE_EVIDENCE_DIR`).

## Hardware-dependent gate

Run two consecutive visible, unobscured checks on the reference machine after the deterministic
candidate gate:

```sh
BUILD_NUMBER=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' \
  dist/Tmon.app/Contents/Info.plist)
bash script/performance_gate.sh --samples 30 \
  --output "performance/results/release-candidate-build-$BUILD_NUMBER-run1.json"
bash script/performance_gate.sh --samples 30 \
  --output "performance/results/release-candidate-build-$BUILD_NUMBER-run2.json"
bash script/compare_metal_reports.sh \
  "performance/results/release-candidate-build-$BUILD_NUMBER-run1.json" \
  "performance/results/release-candidate-build-$BUILD_NUMBER-run2.json"
```

Do not put live compositor timing thresholds on a generic shared CI runner. Keep the JSON reports
with the candidate evidence and compare only like-for-like conditions documented in
`performance/README.md`. The comparison command rejects failed, out-of-order, dirty, differently
sourced, differently built, or differently configured reports. The gate runs the packaged
`dist/Tmon.app` executable, and reports carry its compile-time source and bundle-build identity.
`--allow-dirty` is available only for internal development evidence.

## Public-release blockers

A candidate must not be published until all of these are resolved:

- permanent bundle identifier, Apple Team ID, repository/download identity, and support contact;
- OSC 52 clipboard policy and other trust-boundary hardening in Roadmap Phase 2;
- visible app/daemon diagnostics and tested daemon recovery/upgrade behavior;
- packaged daily-driver, IME, accessibility, sleep/wake, display, and sustained-soak evidence;
- runtime proof for every OS/architecture claimed in the public compatibility matrix;
- Developer ID signature, notarization, stapling, Gatekeeper, quarantine-install, upgrade, and
  rollback evidence for the exact published archive; and
- complete release notes, licenses/notices, security/privacy posture, and known limitations.

## Candidate evidence

Keep these items together for every candidate:

- immutable source revision and source-status cleanliness;
- workspace version, build number, bundle identifier, deployment target, and architectures;
- Rust toolchain and Cargo lockfile;
- deterministic gate results and C ABI smoke result;
- two comparable Metal reports when performance-sensitive code changed;
- archive name and SHA-256 checksum;
- signature identity, Team ID, notarization submission, staple, and Gatekeeper result for public
  candidates;
- runtime compatibility matrix and clean-account quarantine-install result;
- release notes, known issues, upgrade/rollback result, and support contact.

Start the per-candidate compatibility record from `release/COMPATIBILITY_MATRIX.md`. A `blocked` or
`not-run` target is not part of the supported release set; either execute it or mark it unsupported
before publication.

## Rollback rule

Retain the previous signed archive and checksum. A rollback is acceptable only when its mux and
snapshot protocol can safely attach to existing sessions; otherwise tell the user which sessions
cannot be preserved and require explicit confirmation before termination or cleanup.
