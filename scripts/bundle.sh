#!/bin/sh
# Builds a release binary and wraps it in "dist/Ferrous Commander.app".
# Usage: scripts/bundle.sh [--install]   (--install copies the app to /Applications)
set -eu
cd "$(dirname "$0")/.."

APP="dist/Ferrous Commander.app"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

cargo build --release -p fc-app

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/ferrous-commander "$APP/Contents/MacOS/ferrous-commander"

# App icon: render the SVG with Quick Look, then build an .icns.
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"
RENDER=$(mktemp -d)
qlmanage -t -s 1024 -o "$RENDER" crates/fc-app/resources/icon.svg >/dev/null 2>&1 || true
if [ -f "$RENDER/icon.svg.png" ]; then
  for size in 16 32 128 256 512; do
    sips -z $size $size "$RENDER/icon.svg.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double "$RENDER/icon.svg.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Ferrous Commander</string>
  <key>CFBundleDisplayName</key><string>Ferrous Commander</string>
  <key>CFBundleIdentifier</key><string>dev.ferrouscommander.app</string>
  <key>CFBundleExecutable</key><string>ferrous-commander</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSAppleEventsUsageDescription</key><string>Ferrous Commander asks Finder and Terminal to show Get Info windows, make aliases and run commands.</string>
</dict>
</plist>
PLIST

# Ad-hoc signature so macOS runs the bundle locally.
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
echo "Built $APP"

if [ "${1:-}" = "--install" ]; then
  rm -rf "/Applications/Ferrous Commander.app"
  cp -R "$APP" /Applications/
  echo "Installed to /Applications/Ferrous Commander.app"
fi
