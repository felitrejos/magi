#!/bin/sh
set -eu
dir=$1
curl -fsSL -o "$dir/xcodegen.zip" https://github.com/yonaskolb/XcodeGen/releases/download/2.46.0/xcodegen.zip
echo "4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806  $dir/xcodegen.zip" | shasum -a 256 -c
unzip -q "$dir/xcodegen.zip" -d "$dir"
echo "$dir/xcodegen/bin" >> "$GITHUB_PATH"
