# Third-party license policy

Tmon's release dependency graph may use only the SPDX licenses allowlisted in `deny.toml`, and all
registry dependencies must come from crates.io. `cargo-deny 0.19.0` checks both rules as part of
every release candidate:

```sh
cargo-deny check licenses sources
```

Adding a license, source, exception, or manual clarification requires a focused review of the exact
crate version and its license files. Every exception must name the package version, link the
upstream license source in the change description, explain why distribution is compatible with
Tmon's `MIT OR Apache-2.0` terms, and have an owner and removal condition. Do not use a broad
package, source, or license bypass.

Before the first public artifact, generate and review a complete third-party notices file from the
locked dependency graph, include it in `Tmon.app/Contents/Resources`, and make bundle verification
fail when it is absent. The current internal-candidate gate checks compatibility and provenance; it
does not claim that public-distribution notices are complete.
