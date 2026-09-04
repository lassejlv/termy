# Tmon release checklist

No public release is ready until every applicable item below has evidence for the exact archive.

## Candidate identity

- [ ] The working tree is clean and the source revision is immutable and pushed.
- [ ] `CHANGELOG.md` and release notes identify the same semantic version and build number.
- [ ] Bundle identifier, minimum macOS version, architectures, support contact, and known
      limitations are final.
- [ ] `bash script/release_candidate.sh` passes and its JSON evidence matches the source revision.
- [ ] Two live Metal gates execute the exact packaged candidate, pass on the reference Mac, and
      `script/compare_metal_reports.sh RUN1.json RUN2.json` accepts them as ordered, clean, and
      comparable.

## Distribution artifact

- [ ] The versioned zip's SHA-256 checksum verifies after copying it away from the build tree.
- [ ] The app has the intended Developer ID Application identity and Team ID, not an ad hoc
      signature.
- [ ] Apple notarization succeeds, the ticket is stapled, and Gatekeeper accepts the app.
- [ ] The downloaded zip retains the expected checksum and quarantine attribute.
- [ ] A clean standard user account installs and launches the app without the repository or Rust.

## Runtime and recovery

- [ ] The packaged-app smoke suite passes on every declared OS and architecture.
- [ ] A copy of `release/COMPATIBILITY_MATRIX.md` names every exact target and evidence item, with no
      unexplained `not-run` or `blocked` row in the claimed support set.
- [ ] Session detach, reattach, upgrade, rollback, and explicit termination match the documented
      lifecycle contract.
- [ ] Clipboard, diagnostics, privacy, accessibility, IME, and unsupported protocol behavior match
      the release notes.
- [ ] The prior signed archive and checksum remain available, and rollback was exercised without
      silently losing live sessions.

## Publication record

- [ ] Licenses and reviewed third-party notices are present in the app bundle.
- [ ] Archive, checksum, JSON evidence, CI results, performance reports, compatibility results,
      notarization result, known issues, and rollback result are retained together.
- [ ] The support and security-report channels are live and named in the release notes.
