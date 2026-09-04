# Tmon security policy

## Release status and supported versions

Tmon is pre-release until an exact archive has passed the signed, notarized, quarantined-install,
compatibility, performance, and rollback gates in `ROADMAP.md`. No current ad hoc build is a
supported public security release.

After the first public release, the newest patch release is the supported line. N-1 is retained only
as a signed rollback/session-recovery client until its live daemon generation is drained; it does
not silently receive an extended security-support promise. A security fix that changes snapshot or
mux semantics must ship a versioned protocol and retain an explicit old-session recovery path.

## Reporting

Do not report a suspected vulnerability in a public issue or include terminal contents, commands,
credentials, clipboard data, private paths, or customer data. The intended private channel is the
repository host's private security-advisory flow. The release owner must enable it, submit a private
test report, and publish the final monitored URL/contact before the first public release. Until that
is verified, the missing live security contact is a release blocker.

Local crashes are not uploaded. A reporter may generate the JSON described in `SUPPORT.md`, review
it locally, and attach it only through the verified private channel.

## Triage and severity

- **Critical:** arbitrary code execution, cross-user access, signing/update compromise, or silent
  destructive control of unrelated sessions. Stop distribution and rotate/revoke affected release
  material as applicable.
- **High:** terminal-output-triggered sensitive action outside policy, local privilege boundary
  bypass, reliable secret disclosure, or session destruction/corruption. Prepare an urgent patch or
  rollback and warn affected users through the verified release channel.
- **Moderate:** bounded denial of service, persistent crash, unsafe default with meaningful user
  interaction, or material privacy metadata leak. Patch in the next supported release unless
  exploitation raises severity.
- **Low:** defense-in-depth weakness without a demonstrated boundary crossing. Track it with a test
  and resolve through normal maintenance.

Initial acknowledgement, reproduction, severity, affected versions, and an owner are recorded
privately. Do not promise a public timeline before reproduction. Disclosure is coordinated after a
fix and exact signed artifacts are available; reporters receive credit when they want it and it is
safe to provide.

## Hotfix and rollback procedure

1. Freeze unrelated release changes and preserve the report privately.
2. Reproduce with a minimized fixture that contains no reporter data; add a regression test.
3. Decide whether to stop distribution or roll back using the criteria in `UPDATE.md`.
4. Implement and review the smallest boundary-preserving fix. Run dependency, unsafe, fuzz,
   deterministic, packaged-runtime, Metal, soak, and N-1/N rollback gates affected by the change.
5. Build from a clean immutable revision, Developer ID sign, notarize, staple, quarantine-install,
   and verify the exact archive on every still-supported target.
6. Publish checksums, affected/fixed versions, mitigations, session/update implications, and credit;
   retain private evidence according to the repository owner's incident policy.

Release credentials remain only in the login keychain or reviewed CI secret store. They are never
placed in source, logs, support bundles, command-line arguments, or issue forms.
