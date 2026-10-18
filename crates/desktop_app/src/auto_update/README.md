# termy::auto_update

Update-checking and installer handoff logic for Termy.

## Owner

This module owns release discovery, artifact verification, platform update decisions, and OS handoff points. It may depend on `termy_core::release_core` for release metadata, but it should not own desktop rendering or update UI.

Use this module when changing how Termy finds, validates, or launches updates.

## Platforms

- **macOS** uses [Sparkle](https://sparkle-project.org) (`sparkle.rs`). `scripts/build-dmg.sh` embeds `Sparkle.framework` and sets `SUFeedURL` (one appcast per architecture, served from the latest GitHub release) and `SUPublicEDKey`. The app loads the framework at runtime, so development builds without it still start; Sparkle owns the check schedule, update UI, and install/relaunch. The release workflow signs each DMG with the `SPARKLE_ED_PRIVATE_KEY` secret and attaches `appcast-<arch>.xml` (see `scripts/sparkle-appcast.py`). The private key also lives in the maintainer's login keychain under the Sparkle account `termy`.
- **Windows** uses the in-app updater (`AutoUpdater`): it finds the release asset for the running architecture on GitHub, downloads it to `%LOCALAPPDATA%\Termy\Cache\update-<version>.<ext>.part` (all requests time out on stalled connections), verifies it against `checksums.txt`, and only then renames it into place. The installer is started with a plain `open`, never `runas`, plus `/CURRENTUSER` or `/ALLUSERS` to match where the running `termy.exe` is installed; Setup elevates itself only for all-users installs, so the relaunched Termy is not elevated. Old installers and partial downloads in the cache are removed at startup and before each download.
- **Linux** has no in-app installer.

## Validation

```sh
cargo test -p termy
```

## Boundaries

Application state must stay in the desktop app shell; shared headless logic belongs in core.
