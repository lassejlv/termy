# Tmon Production Roadmap

## Project snapshot

Tmon is a native macOS terminal for developers who spend most of their time in shells, full-screen
TUIs, and agent tools. It is a Rust workspace with six current packages: privacy-safe diagnostics,
the headless terminal and native PTY engine, a per-user session multiplexer, a Metal renderer, a
C/Swift FFI, and the `tmon` application.

The project is a feature-complete daily-driver candidate rather than a prototype. It already has a
native PTY, bounded output and scrollback, persistent tabs within a login session, layout-aware
keyboard input, search and selection, a retained Metal renderer, a versioned FFI, universal bundle
creation, notarization tooling, and a measured performance gate. The matched M4/macOS 26 reports in
`performance/final-report.md` put every warm workload inside the current 120 Hz budget and show
stable bounded work for idle, occluded, and inactive tabs.

The implementation now has a pinned toolchain, macOS CI and candidate workflows, deterministic
release gates, license and notice policy, safe OSC 52 defaults, message-specific mux bounds,
privacy-safe diagnostics/support export, explicit daemon generations and termination, exact input
and TUI regression fixtures, IME pre-edit, an accessibility-preview contract, packaged runtime and
N-1 protocol smoke tests, and an enforced native Metal timing/lifecycle gate.

Production readiness is still blocked at the distribution and hardware boundary:

- The working tree contains the roadmap implementation plus the PTY consolidation and renderer
  fixes. The complete local gate passes, but the evidence is deliberately dirty and non-publishable;
  no clean tagged candidate or hosted CI run has been recorded.
- The universal internal archive is ad hoc signed. No Developer ID signed, notarized, stapled,
  Gatekeeper-accepted, quarantined clean-account install has been recorded.
- Runtime proof covers only the current Apple Silicon reference Mac. Intel and the older declared
  macOS targets remain unverified and must either gain evidence or be removed from support.
- The automated packaged smoke proves a real PTY, detach/reattach, support export, and deliberate
  termination, but the named shell/TUI/IME/VoiceOver and physical display rows still require
  recorded interaction with the exact candidate.
- Versioned protocol coexistence and rollback are proven with internal v2/v3 apps, but production
  upgrade evidence still requires two distinct Developer ID signed releases.
- Public release ownership remains undecided: permanent bundle/repository/download identity, Apple
  Team ID, support contact, and a live private security-report channel.

The release target assumed by this roadmap is a directly distributed, Developer ID signed macOS
application. If the intended target is the Mac App Store or private personal use, resolve that in
Phase 1 because it changes signing, sandboxing, update, support, and accessibility requirements.

## Where to start

### Close the signed-release and compatibility evidence gap

**Why this first:** The code-side production gates and internal lifecycle evidence now exist. The
remaining risk is treating an ad hoc, single-machine build as a supported download. Signing,
quarantine, physical runtime compatibility, and permanent release ownership cannot be inferred from
the passing local suite.

Closing this gap turns the existing candidate machinery into one authentic, supportable release
rather than adding more features to an artifact that still cannot be distributed truthfully.

**First actions**

1. Confirm the direct-download model, permanent bundle/repository/download identity, Apple Team ID,
   support/security contacts, minimum macOS target, and whether Intel remains supported.
2. Commit the implementation, run the hosted candidate workflow from that immutable revision, and
   Developer ID sign, notarize, staple, checksum, and quarantine-install the exact resulting app.
3. Execute the packaged daily-driver, display, accessibility-preview, OS/architecture, performance,
   soak, and signed N-1/N rollback matrices; remove every unproved target from the release claim.

**Done when**

- One immutable clean revision passes hosted CI and produces the exact candidate used for every
  downstream check.
- The archive is signed by the intended Team ID, notarized/stapled, accepted under quarantine, and
  tied to its checksum, source revision, build number, reports, compatibility matrix, and rollback
  record.
- Every claimed OS/architecture/display row has runtime evidence; every unavailable row is removed
  or explicitly unsupported.
- The public installation, update, rollback, privacy, accessibility, security, and support promises
  match the behavior of that exact artifact.

## Roadmap

### Phase 1 — Establish the production acceptance contract

**Outcome:** Every prospective Tmon release starts from a clean, identified revision and is judged
against one automated, evidence-producing gate.

**Why now:** Tests, benchmarks, and packaging scripts exist, but they are separate commands and
local reports. Consolidating their contract is the foundation for all remaining production work.

**Scope**

- [ ] Decide and document direct download versus Mac App Store, final `CFBundleIdentifier`, product
      and repository naming, deployment target, architectures, and release versioning.
- [x] Decide whether `crates/ffi` is a stable supported API or an explicitly experimental surface;
      add an ABI compatibility rule appropriate to that decision.
- [x] Reconcile the tracked completed goal with the new in-engine PTY ownership and remove stale
      standalone-`pty` documentation.
- [x] Pin the supported Rust toolchain and make clean-checkout dependency resolution deterministic.
- [x] Add macOS CI for formatting, Clippy with warnings denied, workspace tests, the C smoke test,
      and a release build.
- [x] Add a release-candidate command/workflow that runs deterministic checks, packages the app,
      validates the bundle, and records versioned machine-readable evidence.
- [x] Keep the live Metal gate manual or dedicated-runner-only; require two consecutive comparable
      runs using the conditions in `performance/README.md`.
- [x] Add the declared MIT and Apache license texts and define third-party license/notice checks.
- [x] Add a short release checklist covering revision, changelog, known limitations, archive,
      checksum, signature, notarization, installation, smoke test, and rollback.

**Dependencies**

- A decision on public versus private distribution.
- Access to a macOS CI runner; a visible dedicated Mac is required for the Metal gate.
- Final ownership of the Tmon name, bundle identifier, and remote repository.

**Exit criteria**

- A clean clone can produce an internal release candidate without undocumented local files.
- Deterministic gates pass in CI and the current reference Mac produces two passing comparable Metal
  reports after the PTY and rendering changes.
- The workspace version, Info.plist values, archive name, checksum, reports, and release notes all
  identify the same candidate.
- The support matrix contains only targets that later phases can actually validate.

### Phase 2 — Harden trust boundaries, recovery, and session lifecycle

**Outcome:** Untrusted terminal output and local protocol input remain bounded, sensitive actions
follow explicit policy, and failures are recoverable and diagnosable from a Finder-launched app.

**Why now:** A terminal parses attacker-controlled output and owns long-lived shell processes. These
risks must be resolved before inviting external users or automating updates.

**Scope**

- [x] Add an OSC 52 policy with a safe production default, explicit config, and tests for active and
      inactive tabs. Never silently broaden clipboard read/write behavior.
- [x] Review OSC 7/8/22, titles, dynamic colors, replies, paste, and URL opening as one terminal-output
      trust boundary; add size and action-policy tests for adversarial sequences.
- [x] Replace the multiplexer-wide 1 GiB frame allowance with measured request/response-specific
      limits and reject oversized lengths before allocation.
- [x] Add malformed/truncated/oversized protocol, snapshot, and parser tests; add fuzz targets for
      `Terminal::feed`, snapshot decode, and multiplexer frame decode.
- [x] Make the private-socket ownership contract explicit and test directory permissions, peer user,
      stale-socket replacement, symlink/race behavior, and malformed-client isolation.
- [x] Replace invisible `stderr`-only failures with macOS unified logging and a user-visible startup,
      renderer, and multiplexer recovery path. Keep logs local and redact terminal contents and
      command arguments by default.
- [x] Define deliberate actions for detach, quit-and-terminate, stale daemon cleanup, and recovering
      from a dead daemon without accidentally killing unrelated sessions.
- [x] Define protocol-upgrade behavior: discover old daemons, migrate or offer an explicit fallback,
      and clean them up only after their PTYs are closed or the user confirms termination.
- [x] Audit the two allowed unsafe boundaries (`engine::pty` native syscalls and `ffi`) and add a
      dependency vulnerability/license gate appropriate for releases.

**Dependencies**

- Phase 1 acceptance gate.
- Product decisions for OSC 52 behavior, diagnostics/privacy, and upgrade persistence.

**Exit criteria**

- Adversarial parser and local-protocol fixtures cannot trigger unbounded allocation, process-wide
  crashes, silent sensitive actions outside policy, or cross-session state corruption.
- A Finder-launched startup/render/multiplexer failure produces an actionable user message and a
  bounded, privacy-safe local diagnostic trail.
- Automated lifecycle tests cover attach, detach, reconnect, daemon death, stale sockets, upgrades,
  explicit termination, and cleanup while proving which PTY process groups survive or stop.
- The unsafe and dependency audits have no unresolved release-blocking finding.

### Phase 3 — Prove daily-driver compatibility and accessibility

**Outcome:** The packaged application works predictably across the shells, TUIs, fonts, input
methods, displays, and interaction patterns used by its intended audience.

**Why now:** Unit tests cover terminal semantics well, but the recent Claude Code emoji, OpenCode
spinner, and cursor fixes show that real font fallback and TUI rendering can fail despite correct
cell data. Production confidence needs exact end-to-end fixtures and real packaged-app smoke tests.

**Scope**

- [x] Create a repeatable packaged-app smoke matrix for login shells, `ssh`, `tmux`, editors,
      pagers, full-screen mouse TUIs, Claude Code, and OpenCode.
- [x] Record exact raw PTY byte fixtures for localized Option characters, legacy xterm keys, Kitty
      keyboard negotiation/releases, paste, focus, and mouse modes.
- [x] Promote the current Claude Code record-symbol and OpenCode spinner regressions into a small
      corpus of captured terminal streams with reference screenshots and cell-alignment assertions.
- [x] Test Menlo plus a small supported font set across ASCII, combining marks, emoji variation
      selectors, CJK/wide cells, box drawing, Braille, and geometric block characters.
- [ ] Exercise Retina/non-Retina scaling, multiple displays, refresh-rate changes, sleep/wake,
      occlusion, rapid resize, native-tab switching, display disconnect, and renderer surface loss.
- [x] Implement and verify IME pre-edit composition instead of supporting only committed text.
- [x] Define a VoiceOver/accessibility baseline for the terminal, search field, tabs, selection, and
      process-exit/error state; implement it or clearly scope the first release as an accessibility
      preview rather than claiming support.
- [ ] Run a long-lived soak with output, idle periods, detach/reattach, tab churn, scrollback fill,
      resize, and sleep/wake while sampling CPU, RSS, descriptors, threads, and daemon count.

**Dependencies**

- Phase 2 failure reporting, so matrix failures leave useful evidence.
- Access to the target shells/TUIs, keyboard layouts, displays, and assistive technology.

**Exit criteria**

- The package passes the documented daily-driver matrix with no text-column drift, lost input,
  stuck cursor, stale frame, session loss, or runaway CPU/RSS.
- Every fixed external-app regression has a durable byte/cell/geometry test and a recorded visual
  smoke result.
- IME composition and the declared accessibility baseline work in the packaged app, or unsupported
  behavior is explicitly disclosed and accepted for the release tier.
- A sustained soak finishes within the established memory/CPU/descriptor/thread bounds.

### Phase 4 — Validate, sign, notarize, and publish the first supported release

**Outcome:** Users can download one authentic Tmon archive, pass Gatekeeper, install it from a
quarantined download, use it on every declared target, and roll back safely.

**Why now:** Signing cannot compensate for unresolved correctness or lifecycle problems. Once the
candidate is hardened and compatible, the existing packaging scripts can become a trustworthy
release path.

**Scope**

- [ ] Validate native runtime behavior on Apple Silicon macOS 14, 15, and 26, plus Intel macOS 14+
      if Intel remains supported. Narrow the matrix instead of inferring support from a cross-build.
- [ ] Build the universal archive with a Developer ID Application identity and hardened runtime;
      notarize, staple, and verify it with `codesign`, `notarytool`, `stapler`, and `spctl`.
- [ ] Test the zipped artifact after download/quarantine from a clean standard user account without
      a Rust toolchain or repository present.
- [ ] Smoke startup, shell environment, config creation, tabs, detach/reattach, input, clipboard
      policy, hyperlinks, search, resize, sleep/wake, and uninstall/cleanup from that artifact.
- [ ] Verify version/build metadata, architectures, deployment target, icon/resources, signature
      identity, Team ID, notarization ticket, archive checksum, and update/rollback compatibility.
- [ ] Publish release notes, supported/unsupported behavior, security/privacy posture, license and
      third-party notices, installation/update/rollback steps, and a support channel.
- [ ] Retain the exact release artifact, checksum, source revision, CI results, performance reports,
      notarization result, compatibility matrix, and known-issues list.

**Dependencies**

- Phases 1-3 complete.
- Apple Developer membership, Developer ID identity, and notary credentials.
- Physical or hosted access to every OS/architecture still claimed.

**Exit criteria**

- The published archive is signed by the intended Team ID, notarized and stapled, passes Gatekeeper,
  matches its checksum, and installs/launches from quarantine on a clean account.
- Every row in the public compatibility matrix has recorded runtime evidence from that release
  artifact; untested rows are removed or marked unsupported.
- Core terminal, session, and upgrade/rollback smoke tests pass from the installed application.
- Release documentation accurately describes session lifetime, clipboard behavior, diagnostics,
  missing protocols, accessibility, updates, and cleanup.

### Phase 5 — Make releases supportable and upgrade-safe

**Outcome:** Production releases can be updated, diagnosed, rolled back, and maintained without
stranding sessions or relying on the developer's local checkout.

**Why now:** The first release can use a manual checksummed update, but recurring releases need an
explicit compatibility and support loop before feature expansion.

**Scope**

- [x] Start with a documented manual update path; add an automatic updater only after signing,
      rollback, release-feed authentication, and mux migration semantics are proven.
- [x] Add versioned snapshot/protocol compatibility fixtures and an N-1 to N installed-app upgrade
      test that includes live detached sessions.
- [x] Provide a local, user-reviewable support bundle with app/daemon versions, OS/GPU/display,
      signature state, config validation, socket/daemon state, and bounded logs—but no terminal
      contents, clipboard, environment secrets, or command history by default.
- [x] Define crash and security-report handling, severity, supported versions, rollback criteria,
      and a release hotfix procedure.
- [x] Track post-release regressions by exact terminal stream, environment, font, scale, and app
      version; turn confirmed issues into deterministic tests before closing them.
- [ ] Re-run deterministic gates on every change and the full signed-package, compatibility,
      performance, and upgrade matrix for each release candidate.

**Dependencies**

- A published Phase 4 release and an authenticated distribution location.
- Decisions on opt-in crash reporting versus local-only diagnostics.

**Exit criteria**

- Updating and rolling back between two signed releases is documented and tested without silently
  losing, orphaning, or duplicating daemon-owned sessions.
- A user can collect useful diagnostics without exposing terminal contents or secrets.
- Release ownership, supported-version policy, security contact, regression intake, and hotfix
  procedure are documented and exercised once.
- A second release candidate can complete the process without undocumented machine state.

## Good ideas

### Now

- **Freeze release ownership (small, external):** Confirm the direct-download model, permanent
  bundle/repository/download identity, Team ID, support contact, and private security channel.
- **Create the clean signed candidate (medium, external):** Commit the validated tree, observe hosted
  CI, Developer ID sign and notarize it, then test the quarantined checksum-verified archive.
- **Run the physical compatibility matrix (medium-large):** Exercise every claimed OS, architecture,
  display event, shell/TUI, IME, and accessibility-preview row from that exact archive.
- **Prove signed upgrade/rollback (medium):** Repeat the passing v2/v3 protocol test with distinct,
  retained N-1 and N Developer ID releases.

### Next

- **Authenticated automatic updates:** Consider only after two signed manual releases have proved
  feed identity, rollback, and protocol/session behavior.
- **Semantic VoiceOver terminal tree (large):** Move beyond the documented accessibility preview
  with bounded visible text, caret/selection, tab/search/status roles, and notifications.
- **Older-daemon management UI (medium):** Discovery is safe and cleanup is explicit; a future UI
  can help users reopen or drain older protocol generations without hiding their PTYs.
- **Continuous compatibility lab (medium-large):** Keep only the older-mac/Intel targets that can be
  rerun for every release rather than relying on a one-time launch.

### Later

- **Automatic updates:** Valuable after manual signed upgrades, feed authentication, rollback, and
  live-session migrations are proven.
- **Reboot-persistent sessions:** Promising, but it adds launch-agent, state durability, stale-process,
  and recovery complexity beyond the current within-login promise.
- **Kitty graphics, sixel, bidi shaping, rectangular selection, and ligature controls:** Useful
  compatibility additions documented in `README.md`, but none should delay a truthful, reliable
  first production release for the existing developer workflow.
- **Mac App Store distribution:** Revisit only if the product accepts App Sandbox and store-policy
  constraints around shells, PTYs, downloads, and updates.

## Project guidelines

- Keep `engine` independent of windowing and GPU code; PTY/emulator behavior belongs there, while
  Metal and native-window behavior remain in `render` and `app`.
- Keep unsafe code confined to the audited native PTY and FFI boundaries, with every unsafe operation
  carrying a concrete ownership/lifetime/system-call justification.
- Treat keyboard behavior as a byte-level wire contract. Verify localized layouts and negotiated
  protocols through raw PTY bytes, not screenshots alone.
- Treat renderer fixes as cell-geometry contracts. Preserve exact terminal columns across font
  fallback, emoji presentation, geometric glyphs, scale factors, and animation frames.
- Preserve bounded PTY queues, GUI pending output, scrollback, reusable buffers, coalesced wakeups,
  display pacing, and zero-work idle/occluded/inactive states.
- Never claim performance from compile success or headless tests. Use deterministic work counters
  plus comparable visible Metal CPU/RSS/latency evidence.
- Never claim an OS/architecture from a successful cross-build. Launch the exact signed artifact and
  exercise PTY, tabs, input, renderer, detach/reconnect, and Gatekeeper on that target.
- Never silently kill or strand daemon-owned sessions during upgrades, cleanup, protocol changes,
  app quit, or error recovery.
- Keep diagnostics local, bounded, and secret-safe by default. Terminal text, clipboard contents,
  commands, environment, and working paths require explicit user review before sharing.
- Ship only from a clean identified revision; ignored files under `dist/` are outputs, not proof of
  what the current source produces.

## Risks and dependencies

- **Distribution ownership is unconfirmed:** The implemented path is direct Developer ID download;
  the owner still must accept it and freeze its permanent identities and contacts.
- **Product identity is not final:** The bundle uses `com.tmon.app` while the Git remote is still
  named `termy`; changing identity after release affects preferences, sessions, signing, and updates.
- **Current checkout is dirty:** PTY ownership and renderer fixes are validated locally but not tied
  to an immutable release revision; stale ignored bundles can mask this distinction.
- **Single-machine performance proof:** Current reports cover one Apple M4 reference host. Older
  systems and Intel may expose different font, AppKit, Metal, timing, and memory behavior.
- **Long-lived daemon upgrades:** Old protocol generations are discoverable and protected from new
  clients, but users still need the matching old app to drain their sessions.
- **Terminal output is untrusted:** OSC, hyperlinks, clipboard requests, titles, snapshots, and local
  mux frames must remain bounded and policy-controlled even when generated by remote applications.
- **Font fallback varies by OS:** Emoji, CJK, block characters, and absent glyphs can change face and
  metrics across macOS versions despite stable engine cells.
- **Public FFI increases compatibility cost:** A supported C ABI needs versioning, symbol/layout
  checks, ownership documentation, and deprecation policy; otherwise mark it experimental.
- **Apple credentials and hardware are external dependencies:** Notarization and runtime-matrix proof
  cannot be completed solely from CI or the current M4 machine.

## Open questions

- Is the first production release a public direct download, a private daily-driver build, or a Mac
  App Store product?
- What final bundle identifier, Team ID, repository name, download domain, and support contact should
  own the release?
- Is Intel a real supported target, and is macOS 14 the genuine minimum? Which physical or hosted
  machines will continuously prove those claims?
- Will the owner accept the implemented v0.1 decisions: disabled-by-default OSC 52, experimental C
  ABI, local-only diagnostics, accessibility-preview tier, checksummed manual updates, and
  within-login session persistence?
- Which disposable SSH host and exact Claude Code/OpenCode versions will be retained for the manual
  packaged-app matrix?
- Is reboot restoration a future product commitment, or is within-login persistence the long-term
  contract?

## Current implementation evidence

- `script/release_candidate.sh --allow-dirty` passed dependency/unsafe policy, formatting, strict
  Clippy, 242 release tests, the C ABI consumer, universal packaging, bundle validation, and archive
  checksum verification for internal build 10. The build-specific archive is
  `dist/Tmon-0.1.0-10-macos-universal.zip` with SHA-256
  `6523addf45118fd072d846f205fc822c7dc1fe254eed77b8764d9593bfab210f`; its evidence records
  `dirty: true`, ad hoc signing, and `publishable: false`, so it is not public-release proof.
- The native report schema records source/protocol identity, covers 16 real Metal workloads
  including scale rebuild and surface recreation, enforces detected-refresh timing budgets, and
  fails on incomplete coverage or nonzero idle/occluded/inactive work. A short live schema-v3 gate
  passes; two final 30-sample candidate reports remain required after the soak completes.
- `release/evidence/packaged-smoke-build10.json` records a passing checksum-verified isolated PTY,
  detach/reattach without duplication, private support export, and explicit termination from the
  exact ad hoc archive. It deliberately records `distribution_ready: false` and leaves the visual
  matrix manual.
- `release/evidence/upgrade-rollback-v2-v3-build10.json` records live v2/v3 daemon coexistence,
  N-1 rollback reattach, and generation-scoped termination. It deliberately records production
  upgrade/rollback as false because the pair is not Developer ID signed.
- The rejected soak reports retain the initial ambiguous memory policy, the thread-per-tab-close
  growth diagnosis, and the later cross-thread reaper result. Non-blocking queue admission fixed the
  reaper shutdown deadlock, but its completed 30-minute run still retained 27,552 KiB after settle.
  PTY teardown now runs on the originating client-handler thread after daemon state is unlocked and
  the close response is sent. Its focused five-minute run completed 2,496 cycles with 9,600 KiB
  post-settle RSS growth and zero fd/thread growth; the authoritative 30-minute rerun is in progress.
- `fixtures/`, `security/`, `UNSAFE_AUDIT.md`, `DEPENDENCY_AUDIT.md`, `SESSION_LIFECYCLE.md`,
  `ACCESSIBILITY.md`, `PACKAGED_SMOKE.md`, `SOAK.md`, `SUPPORT.md`, `UPDATE.md`, and `SECURITY.md`
  hold the deterministic contracts and explicit unsupported boundaries.

## Planning evidence reviewed

- `git status --short --branch` — confirmed branch `fresh-start`, the PTY consolidation, renderer
  fixes, and an otherwise dirty pre-release checkout.
- `git log -12 --oneline --stat` and `git tag --list` — showed two source commits, no release tags,
  and active work concentrated in the initial source import and app icon.
- `Cargo.toml`, `Cargo.lock`, and crate manifests — established the five-package Rust workspace,
  version `0.1.0`, Rust 1.96 floor, dependency graph, and lint boundaries.
- `README.md` — established the product workflow, implemented protocol surface, persistent-session
  promise, unsupported features, packaging contract, and unverified support-matrix rows.
- `.use-goal/current.md` — documented completed native PTY work but still references the removed
  standalone `tmon-pty` crate and its old unsafe boundary.
- `crates/app/src/main.rs`, `config.rs`, and `session.rs` — established the Finder app lifecycle,
  native tabs, config/input paths, committed-IME handling, clipboard behavior, hyperlink allowlist,
  and `stderr`-based runtime errors.
- `crates/engine/src/emulator.rs`, `event.rs`, and engine tests — established parser coverage, the
  bounded 1 MiB OSC 52 payload, automatic clipboard event, and byte/cell-level test style.
- `crates/engine/src/pty.rs` and `crates/engine/src/pty/` — established bounded asynchronous PTY I/O
  and the isolated native syscall boundary after consolidation.
- `crates/mux/src/lib.rs` and `crates/mux/tests/detach_reconnect.rs` — established private socket
  permissions, versioned daemon sockets, bounded pending GUI output, snapshot resync, the 1 GiB frame
  ceiling, and tested detach/reconnect behavior.
- `crates/render/src/lib.rs` and `palette.rs` — established retained rendering, performance counters,
  exact Claude Code/OpenCode regression fixtures, and the current cursor changes.
- `performance/README.md`, `performance/final-report.md`, and `performance/results/phase5-final-*.json`
  — established the M4/macOS 26 performance contract, two-run final measurements, bounded idle
  behavior, and the lack of Intel/older-mac and GPU-timestamp proof.
- `script/performance_gate.sh`, `benchmark_metal.sh`, `package_macos.sh`, `notarize_macos.sh`,
  `verify_macos_bundle.sh`, and `packaging/Info.plist.in` — established the existing test,
  universal-build, signing, notarization, Gatekeeper, bundle metadata, and checksum workflow.
- Initial repository inventory — found no tracked CI workflow, Rust toolchain pin, release
  configuration, license texts, changelog, security policy, or contribution guide; the roadmap
  implementation added the release-relevant omissions.
- `cargo test --workspace` — passed on the initial working tree, including native PTY, mux, FFI,
  renderer, and app tests (208 passed; two manual renderer benchmarks ignored).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`,
  `git diff --check`, and `bash script/test_ffi_c.sh` — passed on the current working tree.
