#!/usr/bin/env bash
# Builds dist/mbar.app: universal mbar + mbar-ui, Sparkle, Info.plist, agent plist,
# command-line symlinks, then signs inside-out. MBAR_SIGN_IDENTITY=- (default) signs
# ad-hoc without hardened runtime; a Developer ID signs with runtime + timestamp.
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$PWD
# shellcheck source=packaging/macos/sparkle.env
source packaging/macos/sparkle.env

IDENTITY="${MBAR_SIGN_IDENTITY:--}"
UNIVERSAL="${MBAR_UNIVERSAL:-1}"
CARGO="${CARGO:-cargo}"
DIST="$ROOT/dist"
APP="$DIST/mbar.app"
C="$APP/Contents"

target_dir() { (cd "$1" && "$CARGO" metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p'); }
VERSION=$("$CARGO" metadata --format-version 1 --no-deps | sed -n 's/.*"name":"mbar","version":"\([^"]*\)".*/\1/p')
IFS=. read -r MAJ MIN PAT <<<"$VERSION"
BUILD=$((MAJ * 1000000 + MIN * 1000 + PAT))

if [ "$UNIVERSAL" = 1 ]; then TRIPLES=(aarch64-apple-darwin x86_64-apple-darwin); else TRIPLES=("$(rustc -vV | sed -n 's/^host: //p')"); fi
for t in "${TRIPLES[@]}"; do
  "$CARGO" build --release -p mbar --target "$t"
  (cd crates/mbar-ui && "$CARGO" build --release --target "$t")
done
TD=$(target_dir "$ROOT"); UTD=$(target_dir "$ROOT/crates/mbar-ui")

rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Resources/bin" "$C/Library/LaunchAgents" "$C/Frameworks"
DAEMONS=(); UIS=()
for t in "${TRIPLES[@]}"; do DAEMONS+=("$TD/$t/release/mbar"); UIS+=("$UTD/$t/release/mbar-ui"); done
lipo -create "${DAEMONS[@]}" -output "$C/MacOS/mbar"
lipo -create "${UIS[@]}" -output "$C/MacOS/mbar-ui"
ln -s ../../MacOS/mbar "$C/Resources/bin/mbar"
ln -s ../../MacOS/mbar "$C/Resources/bin/sketchybar"
cp packaging/macos/AppIcon.icns "$C/Resources/AppIcon.icns"
cp lua/mbar.d.lua "$C/Resources/mbar.d.lua"
cp packaging/macos/dev.rubeen.mbar.plist "$C/Library/LaunchAgents/"
sed -e "s|@VERSION@|$VERSION|" -e "s|@BUILD@|$BUILD|" -e "s|@SPARKLE_PUBLIC_KEY@|$SPARKLE_PUBLIC_KEY|" \
  packaging/macos/Info.plist.in > "$C/Info.plist"
plutil -lint "$C/Info.plist" >/dev/null

# Sparkle (cached per version, checksum-verified).
CACHE="$DIST/.cache"; mkdir -p "$CACHE"
TARBALL="$CACHE/Sparkle-$SPARKLE_VERSION.tar.xz"
if [ ! -f "$TARBALL" ]; then
  curl -fsSL -o "$TARBALL.part" "https://github.com/sparkle-project/Sparkle/releases/download/$SPARKLE_VERSION/Sparkle-$SPARKLE_VERSION.tar.xz"
  mv "$TARBALL.part" "$TARBALL"
fi
echo "$SPARKLE_SHA256  $TARBALL" | shasum -a 256 -c - >/dev/null || { rm -f "$TARBALL"; echo "Sparkle checksum mismatch" >&2; exit 1; }
rm -rf "$CACHE/sparkle" && mkdir -p "$CACHE/sparkle" && tar -xJf "$TARBALL" -C "$CACHE/sparkle"
ditto "$CACHE/sparkle/Sparkle.framework" "$C/Frameworks/Sparkle.framework"

sign() {
  if [ "$IDENTITY" = "-" ]; then codesign -f -s - "$@"
  else codesign -f -s "$IDENTITY" -o runtime --timestamp "$@"; fi
}
SP="$C/Frameworks/Sparkle.framework/Versions/B"
# Downloader.xpc carries entitlements (sandbox, network client) that re-signing must keep.
for x in "$SP"/XPCServices/*.xpc; do
  if [ "$(basename "$x")" = Downloader.xpc ]; then sign --preserve-metadata=entitlements "$x"; else sign "$x"; fi
done
sign "$SP/Autoupdate"
sign "$SP/Updater.app"
sign "$C/Frameworks/Sparkle.framework"
sign "$C/MacOS/mbar"
sign "$C/MacOS/mbar-ui"
sign "$APP"
echo "$APP"
