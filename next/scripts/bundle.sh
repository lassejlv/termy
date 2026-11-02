#!/usr/bin/env bash
# Build Termy Next and wrap it in a .app with libtermy and the session host
# bundled, signed ad hoc for running locally (scripts/release.sh signs for
# distribution).
#
# Usage: scripts/bundle.sh [debug|release] [--universal] [--version X.Y.Z] [--build N]
# Output: next/build/Termy Next.app
set -euo pipefail

profile="release"
universal=0
version="0.1.0"
build_number="1"
while [[ $# -gt 0 ]]; do
  case "$1" in
    debug|release) profile="$1" ;;
    --universal) universal=1 ;;
    --version) version="$2"; shift ;;
    --build) build_number="$2"; shift ;;
    *) echo "usage: $0 [debug|release] [--universal] [--version X.Y.Z] [--build N]" >&2; exit 64 ;;
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
# Load libtermy from the bundle, not the SDK build directory.
install_name_tool -delete_rpath "$sdk_dir/lib" "$app/Contents/MacOS/TermyNext" 2>/dev/null || true
install_name_tool -add_rpath "@executable_path/../Frameworks" "$app/Contents/MacOS/TermyNext" 2>/dev/null || true

icon="$next_dir/../assets/termy.icns"
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

codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
echo "built $app ($version, $( [[ $universal -eq 1 ]] && echo universal || uname -m ))"
