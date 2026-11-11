#!/usr/bin/env bash
# Build OKPlayer.app with Homebrew's libmpv and all its dylibs bundled,
# then wrap it in a .dmg. Usage: scripts/macos-bundle.sh <arch-label>
set -euo pipefail
ARCH="${1:-$(uname -m)}"
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
APP="dist/OKPlayer.app"
BREW="$(brew --prefix)"

rm -rf dist && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources"
cp target/release/ok-player "$APP/Contents/MacOS/OKPlayer"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>OK Player</string>
  <key>CFBundleDisplayName</key><string>OK Player</string>
  <key>CFBundleIdentifier</key><string>com.hq970123.okplayer</string>
  <key>CFBundleExecutable</key><string>OKPlayer</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Not used.</string>
  <key>CFBundleDocumentTypes</key><array><dict>
    <key>CFBundleTypeName</key><string>Media</string>
    <key>CFBundleTypeRole</key><string>Viewer</string>
    <key>LSItemContentTypes</key><array>
      <string>public.movie</string><string>public.audiovisual-content</string><string>public.audio</string>
    </array>
    <key>LSHandlerRank</key><string>Alternate</string>
  </dict><dict>
    <key>CFBundleTypeName</key><string>Video file</string>
    <key>CFBundleTypeRole</key><string>Viewer</string>
    <key>LSHandlerRank</key><string>Alternate</string>
    <key>CFBundleTypeExtensions</key><array>
      <string>mkv</string><string>mp4</string><string>m4v</string><string>mov</string><string>avi</string><string>wmv</string>
      <string>flv</string><string>webm</string><string>ts</string><string>m2ts</string><string>mts</string><string>mpg</string>
      <string>mpeg</string><string>vob</string><string>3gp</string><string>ogv</string><string>rmvb</string><string>rm</string>
      <string>asf</string><string>f4v</string><string>mp3</string><string>flac</string><string>m4a</string><string>wav</string>
      <string>ogg</string><string>opus</string>
    </array>
  </dict></array>
</dict></plist>
PLIST

# libmpv is dlopen()ed from Contents/Frameworks; copy it and every dylib it needs.
cp "$BREW/lib/libmpv.2.dylib" "$APP/Contents/Frameworks/"
chmod u+w "$APP/Contents/Frameworks/libmpv.2.dylib"
install_name_tool -id @executable_path/../Frameworks/libmpv.2.dylib "$APP/Contents/Frameworks/libmpv.2.dylib"
dylibbundler -of -b \
  -x "$APP/Contents/Frameworks/libmpv.2.dylib" \
  -d "$APP/Contents/Frameworks/" \
  -p @executable_path/../Frameworks/ \
  -s "$BREW/lib"

# Ad-hoc sign everything (install_name_tool invalidates signatures on arm64).
find "$APP/Contents/Frameworks" -name '*.dylib' -exec codesign --force -s - {} \;
codesign --force --deep -s - "$APP"

cp README.md THIRD_PARTY.md dist/
hdiutil create -volname "OK Player" -srcfolder "$APP" -ov -format UDZO "dist/OKPlayer-macos-${ARCH}.dmg"
ls -la dist
