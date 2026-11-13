#!/bin/bash
# Builds dist/Skill-Construct-<version>-macos.zip: a universal, Developer ID signed app.
#
#   scripts/release-macos.sh                      build and sign only
#   NOTARY_PROFILE=name scripts/release-macos.sh  also notarize and staple
#
# Signing uses the first "Developer ID Application" identity in the keychain, or $SIGN_IDENTITY.
# Create the notary profile once with: xcrun notarytool store-credentials <name>
set -euo pipefail
cd "$(dirname "$0")/.."

NAME="Skill Construct"
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP="dist/$NAME.app"
ZIP="dist/Skill-Construct-$VERSION-macos.zip"
IDENTITY=${SIGN_IDENTITY:-$(security find-identity -v -p codesigning | sed -n 's/.*"\(Developer ID Application: .*\)"/\1/p' | head -1)}
[ -n "$IDENTITY" ] || { echo "no Developer ID Application certificate found" >&2; exit 1; }

# Keep this machine's home directory and username out of the shipped binary's panic messages.
# (Encoded form: flags are separated by 0x1f so paths with spaces survive.)
export CARGO_ENCODED_RUSTFLAGS="--remap-path-prefix=$HOME=~"$'\x1f'"--remap-path-prefix=$PWD=."
for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target" >/dev/null
  MACOSX_DEPLOYMENT_TARGET=11.0 cargo build --release --target "$target"
done

rm -rf dist && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
lipo -create -output "$APP/Contents/MacOS/skillconstruct" \
  target/aarch64-apple-darwin/release/skillconstruct target/x86_64-apple-darwin/release/skillconstruct
cp assets/icon.icns "$APP/Contents/Resources/icon.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>io.github.justin-r-wright.skillconstruct</string>
  <key>CFBundleExecutable</key><string>skillconstruct</string>
  <key>CFBundleIconFile</key><string>icon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

codesign --force --options runtime --timestamp --sign "$IDENTITY" "$APP"
codesign --verify --strict --verbose=2 "$APP"
ditto -c -k --keepParent "$APP" "$ZIP"

if [ -n "${NOTARY_PROFILE:-}" ]; then
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  ditto -c -k --keepParent "$APP" "$ZIP"   # re-zip so the download carries the stapled ticket
  spctl --assess --type execute --verbose "$APP"
else
  echo "Signed but NOT notarized (set NOTARY_PROFILE). Gatekeeper will still warn on other Macs."
fi
echo "built $ZIP"
