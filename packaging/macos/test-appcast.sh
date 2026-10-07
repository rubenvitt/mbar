#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
out=$(./appcast.sh 0.2.0 2000 https://x/mbar-0.2.0.zip 'sparkle:edSignature="SIG" length="42"' https://x/notes)
grep -q '<sparkle:version>2000</sparkle:version>' <<<"$out"
grep -q '<sparkle:shortVersionString>0.2.0</sparkle:shortVersionString>' <<<"$out"
grep -q '<sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>' <<<"$out"
grep -q 'url="https://x/mbar-0.2.0.zip" sparkle:edSignature="SIG" length="42"' <<<"$out"
command -v xmllint >/dev/null && xmllint --noout - <<<"$out"
echo ok
