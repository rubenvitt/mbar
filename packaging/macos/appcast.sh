#!/usr/bin/env bash
# Prints a one-item Sparkle appcast. Args: version build zip-url signature-attrs notes-url
set -euo pipefail
v=$1 build=$2 url=$3 sig=$4 notes=$5
cat <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>mbar</title>
    <item>
      <title>Version $v</title>
      <pubDate>$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>$build</sparkle:version>
      <sparkle:shortVersionString>$v</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>
      <sparkle:releaseNotesLink>$notes</sparkle:releaseNotesLink>
      <enclosure url="$url" $sig type="application/octet-stream"/>
    </item>
  </channel>
</rss>
EOF
