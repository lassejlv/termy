# Manual update and rollback policy

Tmon v0.1 uses manual, checksum-verified updates. There is no automatic update feed and the app
does not download or execute updates. An updater may be added only after Developer ID signing,
notarization, authenticated feed metadata, rollback, and live-session migration have all been
exercised with two public releases.

Release engineering exercises two installed app bundles with:

```sh
bash script/upgrade_rollback_smoke.sh \
  --previous-app /path/to/Tmon-N-1.app \
  --current-app /path/to/Tmon-N.app \
  --output release/evidence/upgrade-rollback.json
```

The test uses a private temporary home, creates one real PTY in each versioned daemon, detaches both
apps, checks coexistence through N's read-only status view, launches N-1 to prove rollback reattach
without a duplicate session, and proves N's explicit termination does not touch N-1. Both inputs
must be distinct Developer ID signed releases for production evidence. `--allow-adhoc` records only
internal protocol evidence and leaves `production_upgrade_rollback_passed` false.

## Before updating

1. Download the new versioned zip and its `.sha256` file from the final release location.
2. Verify it in the download directory with
   `shasum -a 256 -c Tmon-<version>-<build>-macos-universal.zip.sha256`.
3. Run the installed app's `--session-status`. Closing all windows detaches; it does not terminate
   daemon-owned PTYs.
4. Keep the previous signed zip and checksum. Do not delete an older app while it is the only client
   capable of reconnecting to an older protocol daemon.
5. Extract the new app next to the old app first. Confirm its bundle identifier, version/build,
   Developer ID Team ID, notarization ticket, and Gatekeeper assessment before replacing anything.

## Versioned live sessions

The socket filename includes the mux protocol version. A newer incompatible app starts its own
daemon and leaves the older daemon and its PTYs untouched. `--session-status` reports current and
older daemons without connecting to an unsafe path. Tmon does not claim transparent cross-version
snapshot migration.

If an older daemon is listed as running, use the matching older signed application to finish or
save those sessions. Terminate a generation only through that version's explicit termination action
and only after reviewing the affected sessions. Never delete a live socket or kill a daemon merely
to make an update appear clean.

## Rollback

1. Detach from the new application and retain its signed archive and checksum.
2. Re-verify and launch the retained N-1 signed application. Its versioned socket reconnects only to
   the N-1 daemon; it cannot attach to or terminate the newer generation.
3. Verify shell input/output, tabs, resize, and session identity before replacing the installed app.
4. If the rollback changes a protocol and no compatible daemon remains, state clearly that sessions
   cannot be preserved and require explicit user confirmation before termination.

Rollback is required for startup failure, data/input corruption, session loss/duplication,
unexpected clipboard action, sustained resource growth beyond the release gate, invalid signing or
notarization, or a high/critical security regression. Cosmetic issues may use a normal patch only
when the release owner records why continued distribution is safe.

## Uninstall

Before deleting Tmon, close or save work and explicitly terminate any sessions the user no longer
wants. Removing the app alone intentionally leaves detached same-login sessions alive until their
daemon exits or the login session ends. Configuration, local fixed-code diagnostics, and runtime
directories are separate user-owned data and must be shown before optional removal.
