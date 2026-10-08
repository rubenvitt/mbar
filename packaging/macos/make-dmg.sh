#!/usr/bin/env bash
# dist/mbar.app → notarized app (stapled) → update zip + signature → DMG (notarized,
# stapled) → appcast.xml. NOTARIZE=0 skips Apple's service (local testing).
set -euo pipefail
cd "$(dirname "$0")/../.."
DIST=dist; APP=$DIST/mbar.app
VERSION=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APP/Contents/Info.plist")
BUILD=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$APP/Contents/Info.plist")
IDENTITY="${MBAR_SIGN_IDENTITY:?set MBAR_SIGN_IDENTITY to a Developer ID identity}"
ZIP=$DIST/mbar-$VERSION.zip; DMG=$DIST/mbar-$VERSION.dmg
NOTARIZE=${NOTARIZE:-1}

# Sparkle 2 refuses to start without a public EdDSA key, so a release bundle without
# one would silently never update.
KEY=$(/usr/libexec/PlistBuddy -c 'Print :SUPublicEDKey' "$APP/Contents/Info.plist" 2>/dev/null || true)
[ -n "$KEY" ] || { echo "SUPublicEDKey is empty: set SPARKLE_PUBLIC_KEY in packaging/macos/sparkle.env and rebuild" >&2; exit 1; }

notarize() {
  if [ -n "${ASC_KEY_PATH:-}" ]; then
    xcrun notarytool submit "$1" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" --wait
  else
    xcrun notarytool submit "$1" --keychain-profile "${NOTARY_PROFILE:-mbar-notary}" --wait
  fi
}

# 1. Notarize the app (via a temporary zip) and staple it.
if [ "$NOTARIZE" = 1 ]; then
  rm -f "$DIST/notarize.zip"; ditto -c -k --keepParent "$APP" "$DIST/notarize.zip"
  notarize "$DIST/notarize.zip"; rm -f "$DIST/notarize.zip"
  xcrun stapler staple "$APP"
fi

# 2. Update zip for Sparkle (sign_update prints `sparkle:edSignature="…" length="…"`).
rm -f "$ZIP"; ditto -c -k --keepParent "$APP" "$ZIP"
SIGN_UPDATE="$DIST/.cache/sparkle/bin/sign_update"
[ -x "$SIGN_UPDATE" ] || { echo "$SIGN_UPDATE not found: run 'make app' first" >&2; exit 1; }
if [ -n "${SPARKLE_KEY_FILE:-}" ]; then SIG=$("$SIGN_UPDATE" --ed-key-file "$SPARKLE_KEY_FILE" "$ZIP")
else SIG=$("$SIGN_UPDATE" --account "${SPARKLE_ACCOUNT:-mbar}" "$ZIP"); fi

# 3. DMG with an /Applications link.
STAGE=$DIST/dmg-stage; rm -rf "$STAGE" "$DMG"; mkdir -p "$STAGE"
ditto "$APP" "$STAGE/mbar.app"; ln -s /Applications "$STAGE/Applications"
hdiutil create -volname mbar -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
codesign -f -s "$IDENTITY" --timestamp "$DMG"
if [ "$NOTARIZE" = 1 ]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
fi

# 4. Appcast.
BASE="https://github.com/rubenvitt/mbar/releases/download/v$VERSION"
packaging/macos/appcast.sh "$VERSION" "$BUILD" "$BASE/mbar-$VERSION.zip" "$SIG" \
  "https://github.com/rubenvitt/mbar/releases/tag/v$VERSION" > "$DIST/appcast.xml"
echo "$DMG"; echo "$ZIP"; echo "$DIST/appcast.xml"
