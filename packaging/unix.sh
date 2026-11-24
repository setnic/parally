#!/usr/bin/env bash
set -euo pipefail

mkdir -p dist
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

case "$(uname -s)" in
  Darwin)
    app="$stage/Parally.app/Contents"
    mkdir -p "$app/MacOS" "$app/Resources/browser"
    install -m 755 target/release/parallel-downloader "$app/MacOS/Parally"
    install -m 644 assets/parally.icns "$app/Resources/Parally.icns"
    cp -R browser/extension "$app/Resources/browser/"
    cat >"$app/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key><string>Parally</string>
  <key>CFBundleExecutable</key><string>Parally</string>
  <key>CFBundleIconFile</key><string>Parally</string>
  <key>CFBundleIdentifier</key><string>com.parally.app</string>
  <key>CFBundleName</key><string>Parally</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.5.1</string>
  <key>CFBundleVersion</key><string>0.5.1</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    plutil -lint "$app/Info.plist"
    codesign --force --sign - "$stage/Parally.app"
    codesign --verify "$stage/Parally.app"
    ln -s /Applications "$stage/Applications"
    hdiutil create -ov -fs HFS+ -volname Parally -format UDZO -srcfolder "$stage" \
      "dist/Parally-0.5.1-macos-$(uname -m).dmg"
    hdiutil verify "dist/Parally-0.5.1-macos-$(uname -m).dmg"
    ;;
  Linux)
    arch=$(dpkg --print-architecture)
    install -d "$stage/DEBIAN" "$stage/usr/bin" "$stage/usr/share/parally/browser" \
      "$stage/usr/share/applications" "$stage/usr/share/icons/hicolor/512x512/apps"
    install -m 755 target/release/parallel-downloader "$stage/usr/bin/parally"
    install -m 644 assets/parally.png "$stage/usr/share/icons/hicolor/512x512/apps/com.parally.app.png"
    cp -R browser/extension "$stage/usr/share/parally/browser/"
    cat >"$stage/usr/share/applications/com.parally.app.desktop" <<'DESKTOP'
[Desktop Entry]
Name=Parally
Comment=Download files over parallel HTTP connections
Exec=parally
Terminal=false
Type=Application
Icon=com.parally.app
Categories=Network;FileTransfer;
DESKTOP
    cat >"$stage/DEBIAN/control" <<CONTROL
Package: parally
Version: 0.5.1
Architecture: $arch
Maintainer: Parally contributors <198968127+setnic@users.noreply.github.com>
Depends: libc6 (>= 2.35), libegl1, libgl1, libxkbcommon0, libxkbcommon-x11-0, libwayland-client0, xdg-utils, zenity
Section: net
Priority: optional
Description: Desktop HTTP(S) download manager
 Parallel range downloads with saved checkpoints and browser handoff.
CONTROL
    dpkg-deb --root-owner-group --build "$stage" "dist/Parally-0.5.1-linux-$arch.deb"
    dpkg-deb --info "dist/Parally-0.5.1-linux-$arch.deb"
    ;;
esac
