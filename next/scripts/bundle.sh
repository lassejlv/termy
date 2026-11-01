#!/usr/bin/env bash
# Build Termy Next and wrap it in a .app with libtermy bundled.
#
# Usage: scripts/bundle.sh [debug|release]   (default: release)
# Output: next/build/Termy Next.app
set -euo pipefail

profile="${1:-release}"
next_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
sdk_dir="$(cd "$next_dir/../sdk/swift" && pwd)"

"$sdk_dir/scripts/build-libtermy.sh" "$profile"
swift build -c "$profile" --package-path "$next_dir"

app="$next_dir/build/Termy Next.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources" "$app/Contents/Helpers"

cp "$next_dir/.build/$profile/TermyNext" "$app/Contents/MacOS/TermyNext"
cp "$sdk_dir/lib/libtermy_core.dylib" "$app/Contents/Frameworks/"
# The session host that runs every pane's program, so they outlive the app.
cp "$sdk_dir/lib/termy-session-host" "$app/Contents/Helpers/"
# Load libtermy from the bundle, not the SDK build directory.
install_name_tool -delete_rpath "$sdk_dir/lib" "$app/Contents/MacOS/TermyNext" 2>/dev/null || true
install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/TermyNext" 2>/dev/null || true

icon="$next_dir/../assets/termy.icns"
if [[ -f "$icon" ]]; then
  cp "$icon" "$app/Contents/Resources/AppIcon.icns"
fi

cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Termy Next</string>
  <key>CFBundleDisplayName</key><string>Termy Next</string>
  <key>CFBundleIdentifier</key><string>sh.termy.next</string>
  <key>CFBundleExecutable</key><string>TermyNext</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app"
