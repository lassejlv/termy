# Tmon support and diagnostics

Tmon v0.1 keeps diagnostics local. It does not upload crashes, usage, terminal data, or identifiers.
When something fails, first run the read-only daemon view from the installed application:

```sh
/Applications/Tmon.app/Contents/MacOS/tmon --session-status
```

To create a support file in an existing directory, choose a new filename:

```sh
/Applications/Tmon.app/Contents/MacOS/tmon \
  --support-bundle "$HOME/Desktop/tmon-support.json"
```

The command refuses to overwrite a file and creates it with mode `0600`. Open and review the JSON
before sharing it. It contains only:

- Tmon, snapshot/mux, bundle, build, and binary-architecture versions;
- macOS/build and a whitelist of GPU, resolution, scale/refresh information when available;
- signature type, Team ID, hardened-runtime state, and Gatekeeper assessment;
- config validity without config values or paths;
- versioned daemon state without socket paths or process command lines; and
- at most 200 validated fixed-code events from the bounded private local log.

It excludes terminal contents, command history and arguments, clipboard data, environment values,
configuration contents, usernames, home paths, socket paths, and filesystem paths by design. A
malformed, oversized, symlinked, non-private, or differently owned log is omitted rather than read.

For a non-security rendering/input regression, use the repository's terminal-regression issue form
and include the smallest reproducible sequence. Sanitize screenshots and terminal streams. Secrets,
tokens, private hostnames, commands, and customer data do not belong in an issue or support bundle.

The permanent public support contact and private security-report channel are still release identity
decisions. They must be made live and verified before a public release; an internal candidate must
not imply that a placeholder channel is monitored.
