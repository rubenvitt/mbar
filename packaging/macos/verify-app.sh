#!/usr/bin/env bash
# Structural checks for dist/mbar.app (CI runs this on every PR).
set -euo pipefail
APP="${1:-dist/mbar.app}"; C="$APP/Contents"; fail=0
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
check "codesign" "codesign --verify --deep --strict '$APP' 2>/dev/null"
for k in CFBundleIdentifier CFBundleExecutable CFBundleShortVersionString CFBundleVersion LSMinimumSystemVersion SUFeedURL SUPublicEDKey SUEnableAutomaticChecks; do
  check "Info.plist $k" "/usr/libexec/PlistBuddy -c 'Print :$k' '$C/Info.plist' >/dev/null 2>&1"
done
check "bundle id" "[ \"\$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' '$C/Info.plist')\" = dev.rubeen.mbar ]"
for n in mbar sketchybar borders; do
  check "bin/$n relative symlink" "[ \"\$(readlink '$C/Resources/bin/$n')\" = ../../MacOS/mbar ]"
done
check "daemon universal or native" "lipo -info '$C/MacOS/mbar' | grep -Eq 'arm64|x86_64'"
check "agent plist" "plutil -lint '$C/Library/LaunchAgents/dev.rubeen.mbar.plist' >/dev/null"
check "agent label" "[ \"\$(/usr/libexec/PlistBuddy -c 'Print :Label' '$C/Library/LaunchAgents/dev.rubeen.mbar.plist')\" = dev.rubeen.mbar ]"
check "Sparkle" "[ -d '$C/Frameworks/Sparkle.framework' ]"
check "mbar --version" "'$C/Resources/bin/mbar' --version | grep -q '^mbar-v'"
exit $fail
