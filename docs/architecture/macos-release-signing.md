# macOS Release Signing

This workflow prepares Termy's DMG for Developer ID distribution outside the Mac App Store. GitHub Actions runs the existing packaging script for both Apple silicon and Intel and publishes only the resulting `-signed.dmg` files. Apple requires a Developer ID Application certificate, hardened runtime, secure timestamp, and notarization for this distribution path. See [Apple's notarization requirements](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution) and [Mac software packaging guidance](https://developer.apple.com/documentation/xcode/packaging-mac-software-for-distribution).

## Create the Apple credentials

1. In [Apple Developer Certificates](https://developer.apple.com/account/resources/certificates/list), create a **Developer ID Application** certificate for Termy's team. Generate the signing request on the Mac that will export the certificate. Install the downloaded `.cer` so it appears in Keychain Access under **My Certificates** with its private key, then export that identity as a password protected `.p12`. A Mac Development or Mac App Distribution certificate cannot be used for this workflow. [Apple's certificate instructions](https://developer.apple.com/help/account/certificates/create-developer-id-certificates).
2. In [App Store Connect → Users and Access → Integrations → App Store Connect API](https://appstoreconnect.apple.com/access/integrations/api), generate a **Team Key** for notarization. Record its Key ID and Issuer ID, and download its `.p8` private key once. An Individual API Key cannot use `notarytool`. [Apple's API key instructions](https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api).
3. Record the 10-character Team ID for the same team as the certificate. Keep the `.p12`, its password, and `.p8` private key out of the repository.

## Configure GitHub Actions

In the repository's **Settings → Secrets and variables → Actions**, add this **variable**:

| Name | Value |
| --- | --- |
| `APPLE_TEAM_ID` | The Team ID of the Developer ID Application certificate |

Add these **repository secrets**:

| Name | Value |
| --- | --- |
| `APPLE_CERTIFICATE_P12_BASE64` | Base64 encoding of the complete `.p12` file, with no line breaks |
| `APPLE_CERTIFICATE_PASSWORD` | Password chosen when exporting the `.p12` |
| `APPLE_NOTARY_KEY_P8_BASE64` | Base64 encoding of the complete `.p8` file, with no line breaks |
| `APPLE_NOTARY_KEY_ID` | Key ID shown beside the App Store Connect Team Key |
| `APPLE_NOTARY_ISSUER_ID` | Issuer ID shown on the App Store Connect API page |

On macOS, `base64 < /path/to/file | tr -d '\n'` produces the value for either file secret. Avoid putting these values in shell history, logs, issue comments, or PRs. GitHub secrets have a 48 KB value limit; these files should fit when encoded. [GitHub's secrets documentation](https://docs.github.com/en/actions/reference/security/secrets).

The release job checks that the imported certificate belongs to `APPLE_TEAM_ID`, signs the bundled CLI and app with hardened runtime, signs the DMG, submits the DMG to Apple's notary service, staples the accepted ticket, and verifies the result. Missing or invalid credentials fail the macOS job. The stable release finalization also waits for both signed macOS DMGs before marking the release latest.

After adding or rotating credentials, run the manual **Verify macOS signing credentials** workflow on GitHub. It imports the `.p12` into a temporary keychain and checks that the Team API key can reach Apple's notary service, without publishing a release.

## Validate the first signed release

Publish a prerelease after the workflow and credentials are on `main`. Download one DMG per architecture and inspect each with:

```sh
for dmg in Termy-*-signed.dmg; do
  codesign --verify --verbose=2 "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
done
```

Mount each DMG, copy `Termy.app` to `/Applications`, and launch it on matching Apple silicon and Intel Macs. Inspect the app with `codesign --verify --deep --strict --verbose=2 /Applications/Termy.app` and `codesign -dv --verbose=4 /Applications/Termy.app`. Inspect the bundled CLI with `codesign --verify --verbose=2 /Applications/Termy.app/Contents/MacOS/termy-cli`. The reported Team ID must match the GitHub variable. After a signed release is published and this install check passes, update the unsigned-build notices on the website and in `README.md`.

If notarization fails, inspect the submission log with `xcrun notarytool log <submission-id> --key <p8-file> --key-id <key-id> --issuer <issuer-id>`. Do not print private keys in CI logs. [Apple's notarization troubleshooting](https://developer.apple.com/documentation/security/resolving-common-notarization-issues).
