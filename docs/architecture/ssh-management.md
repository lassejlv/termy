# SSH Management

Termy's Rust/GPUI desktop app owns the saved-host workflow. The headless
`termy_core::ssh_core` crate owns validated data, atomic persistence, OpenSSH
arguments, and keychain credential lifecycle.

## Data and secrets

Non-secret host metadata lives beside `config.txt` in `ssh_hosts.json`:

- stable UUID
- display name
- hostname, port, and username
- authentication type
- identity-file path for key authentication

Passwords and private-key passphrases never enter that file. They are stored by
the operating-system credential store under
`com.lassevestergaard.termy.ssh`, keyed by the stable host UUID and secret
kind. Private keys remain in their original files.

## Launch boundary

Saved hosts launch the system OpenSSH client as a typed program plus argv. SSH
host fields and identity paths never pass through `/bin/sh -c`.

Interactive credentials remain terminal prompts. Remembered credentials use a
restricted `SSH_ASKPASS` invocation of the Termy executable:

- the environment contains only the helper path and non-secret routing metadata
  (host ID, credential kind, and expected Termy process ID), never the credential;
- the helper validates that its parent is the OpenSSH process launched by the
  expected Termy process;
- only password, key-passphrase, and host-key confirmation prompts are handled;
- the credential is read from the system keychain only after those checks.

OpenSSH continues to own `known_hosts` and host-key verification. Termy does
not set `StrictHostKeyChecking=no` or alter the user's SSH configuration.

## Validation

```sh
cargo test -p termy_core
cargo test -p termy
cargo check --workspace
just check-boundaries
```
