# Dependency advisory policy

Every release candidate runs `script/dependency_audit.sh`. The script pins `cargo-audit 0.22.2`,
refreshes the RustSec database, and denies vulnerabilities, unsoundness, yanked crates,
unmaintained crates, and every other warning. It also expires the two exceptions below before
2026-10-01 so they cannot become permanent silent policy.

## Current reviewed exceptions

### RUSTSEC-2026-0192 — `ttf-parser` unmaintained

`ttf-parser 0.25.1` is pulled through `glyphon 0.12.0 -> cosmic-text 0.19.0 -> fontdb 0.23.0`.
RustSec classifies this as informational/unmaintained and reports no vulnerability or patched
release. Tmon does parse untrusted fonts installed by the current user, so this is accepted only as
a short migration window—not as a claim that unmaintained parsing code is harmless.

Upstream `fontdb` has removed the dependency on its main branch, but the published `cosmic-text`
and `glyphon` versions do not yet accept that release. Recheck for compatible published releases at
every candidate and remove the exception as soon as the supported text stack can upgrade.

### RUSTSEC-2026-0253 — `lru` panic-safety unsoundness

`lru 0.16.4` is pulled only by `glyphon 0.12.0`. The advisory requires `LruCache::pop()`, a key type
whose `Drop` panics, panic unwinding, and subsequent cache reuse. Glyphon's source does not call
`pop`; its cache key is an owned value without a custom panicking destructor; and Tmon does not
catch and resume renderer panics. The advised path is therefore unreachable in the current locked
consumer, while `lru >= 0.18.2` contains the upstream fix.

This is still an unsafe transitive implementation. Recheck glyphon first; if no compatible release
lands before the review deadline, vendor a minimal reviewed glyphon dependency update to `lru
>= 0.18.2` or replace the text-cache integration. Do not extend the exception without repeating the
source-path analysis and recording an owner and new deadline.

## Resolved during hardening

`bincode 2.0.1` was reported as permanently unmaintained by RUSTSEC-2025-0141. Tmon directly used it
for terminal snapshots and mux messages, so it was replaced with maintained `postcard 1.1.3`.
Encoded sizes remain directionally bounded before transport/allocation, trailing bytes are rejected,
and the protocol generation was advanced to prevent incompatible clients from attaching.
