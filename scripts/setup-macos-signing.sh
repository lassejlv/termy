#!/usr/bin/env bash
set -euo pipefail

# Import Termy's Developer ID certificate and notarization key on an ephemeral
# GitHub-hosted macOS runner. Never print the credential material.

die() { echo "Error: $*" >&2; exit 1; }

[[ -n "${GITHUB_ENV:-}" && -n "${RUNNER_TEMP:-}" ]] || die "This script requires GitHub Actions."
[[ "${APPLE_TEAM_ID:-}" =~ ^[A-Z0-9]{10}$ ]] || die "Set the APPLE_TEAM_ID repository variable to your 10-character Apple team ID."

for name in APPLE_CERTIFICATE_P12_BASE64 APPLE_CERTIFICATE_PASSWORD APPLE_NOTARY_KEY_P8_BASE64 TERMY_NOTARY_KEY_ID TERMY_NOTARY_ISSUER; do
  [[ -n "${!name:-}" ]] || die "Missing $name GitHub Actions credential."
done

umask 077
signing_dir="$(mktemp -d "$RUNNER_TEMP/termy-signing.XXXXXX")"
certificate_path="$signing_dir/developer-id.p12"
notary_key_path="$signing_dir/notary-key.p8"
keychain_path="$signing_dir/signing.keychain-db"
keychain_password="$(openssl rand -hex 32)"

printf '%s' "$APPLE_CERTIFICATE_P12_BASE64" | base64 -D > "$certificate_path"
printf '%s' "$APPLE_NOTARY_KEY_P8_BASE64" | base64 -D > "$notary_key_path"
openssl pkey -in "$notary_key_path" -noout >/dev/null \
  || die "The App Store Connect .p8 key is invalid."

security create-keychain -p "$keychain_password" "$keychain_path"
security set-keychain-settings -lut 21600 "$keychain_path"
security unlock-keychain -p "$keychain_password" "$keychain_path"
security import "$certificate_path" -k "$keychain_path" \
  -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security \
  || die "Could not import the Developer ID .p12; check the file and export password."
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$keychain_password" "$keychain_path" >/dev/null

identity_sha="$(security find-identity -v -p codesigning "$keychain_path" |
  awk -v team="($APPLE_TEAM_ID)" '$0 ~ /Developer ID Application:/ && index($0, team) {print $2}')"
[[ "$identity_sha" =~ ^[A-Fa-f0-9]{40}$ ]] \
  || die "Expected exactly one Developer ID Application identity for team $APPLE_TEAM_ID in the imported .p12."

security list-keychains -d user -s "$keychain_path"
security default-keychain -d user -s "$keychain_path"

printf 'TERMY_SIGN_IDENTITY=%s\n' "$identity_sha" >> "$GITHUB_ENV"
printf 'TERMY_NOTARY_KEY=%s\n' "$notary_key_path" >> "$GITHUB_ENV"
printf 'TERMY_SIGNING_DIR=%s\n' "$signing_dir" >> "$GITHUB_ENV"
echo "Developer ID identity for team $APPLE_TEAM_ID is ready."
