#!/usr/bin/env bash
# Build Termy Next and wrap it in a .app with libtermy and the session host
# bundled, signed ad hoc for running locally (scripts/release.sh signs for
# distribution).
#
# Usage: scripts/bundle.sh [debug|release] [--universal] [--version X.Y.Z] [--build N] [--update-feed URL]
#   --update-feed  the Sparkle appcast this build updates from (scripts/release.sh
#                  sets it); without one the app never checks for updates
# Output: next/build/Termy Next.app
set -euo pipefail

profile="release"
universal=0
version="0.1.0"
build_number="1"
update_feed=""
# The public half of the EdDSA key that signs Termy's and Termy Next's
# releases (the SPARKLE_ED_PRIVATE_KEY Actions secret); see scripts/build-dmg.sh.
sparkle_public_key="${TERMY_SPARKLE_PUBLIC_ED_KEY:-1hTfFqyrg+XaxfZ5ywbvd20aT5YXvL15BschCg2L57o=}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    debug|release) profile="$1" ;;
    --universal) universal=1 ;;
    --version) version="$2"; shift ;;
    --build) build_number="$2"; shift ;;
    --update-feed) update_feed="$2"; shift ;;
    *) echo "usage: $0 [debug|release] [--universal] [--version X.Y.Z] [--build N] [--update-feed URL]" >&2; exit 64 ;;
  esac
  shift
done

next_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_dir="$(cd "$next_dir/../sdk/swift" && pwd)"

if [[ "$universal" -eq 1 ]]; then
  "$sdk_dir/scripts/build-libtermy.sh" "$profile" --universal
  universal_args=(-c "$profile" --package-path "$next_dir" --arch arm64 --arch x86_64)
  swift build "${universal_args[@]}"
  # Where multi-architecture products land differs between SwiftPM versions.
  executable="$(swift build "${universal_args[@]}" --show-bin-path)/TermyNext"
else
  "$sdk_dir/scripts/build-libtermy.sh" "$profile"
  swift build -c "$profile" --package-path "$next_dir"
  executable="$next_dir/.build/$profile/TermyNext"
fi

app="$next_dir/build/Termy Next.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources" "$app/Contents/Helpers"

cp "$executable" "$app/Contents/MacOS/TermyNext"
cp "$sdk_dir/lib/libtermy_core.dylib" "$app/Contents/Frameworks/"
# The session host that runs every pane's program, so they outlive the app.
cp "$sdk_dir/lib/termy-session-host" "$app/Contents/Helpers/"
# Sparkle, for automatic updates. SwiftPM puts it next to the executable.
# Its XPC services are only for sandboxed apps; Termy Next isn't sandboxed.
ditto "$(dirname "$executable")/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
rm -rf "$app/Contents/Frameworks/Sparkle.framework/Versions/B/XPCServices" "$app/Contents/Frameworks/Sparkle.framework/XPCServices"
# Load libtermy and Sparkle from the bundle, never from the build directories,
# whose copies on this Mac are signed differently.
while read -r rpath; do
  case "$rpath" in
    "$sdk_dir"/*|"$sdk_dir"|"$next_dir"/*) install_name_tool -delete_rpath "$rpath" "$app/Contents/MacOS/TermyNext" ;;
  esac
# A universal binary lists each rpath once per architecture.
done < <(otool -l "$app/Contents/MacOS/TermyNext" | awk '/cmd LC_RPATH/ { getline; getline; print $2 }' | sort -u)
install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/TermyNext" 2>/dev/null || true

icon="$next_dir/Resources/icon/TermyNext.icns"
if [[ -f "$icon" ]]; then
  cp "$icon" "$app/Contents/Resources/AppIcon.icns"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Termy Next</string>
  <key>CFBundleDisplayName</key><string>Termy Next</string>
  <key>CFBundleIdentifier</key><string>sh.termy.next</string>
  <key>CFBundleExecutable</key><string>TermyNext</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$build_number</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
if [[ -n "$update_feed" ]]; then
  plist=(/usr/libexec/PlistBuddy "$app/Contents/Info.plist")
  "${plist[@]}" -c "Add :SUFeedURL string $update_feed"
  "${plist[@]}" -c "Add :SUPublicEDKey string $sparkle_public_key"
  "${plist[@]}" -c "Add :SUEnableAutomaticChecks bool true"
  "${plist[@]}" -c "Add :SUScheduledCheckInterval integer 21600"
fi

codesign --force --sign - "$app/Contents/Frameworks/Sparkle.framework" >/dev/null 2>&1 || true
codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app ($version, $( [[ $universal -eq 1 ]] && echo universal || uname -m ))"
