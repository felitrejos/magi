#!/bin/sh
set -eu
version=$1 keychain=$2 out=$3
cd "$(dirname "$0")"
requirement=$(cat ../remote/app.requirement)
xcodegen generate
xcodebuild -project magi.xcodeproj -scheme magi -configuration Release -derivedDataPath build/release MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$version" build
app=build/release/Build/Products/Release/magi.app
codesign --force --sign "magi Code Signing" --keychain "$keychain" --options runtime --entitlements magi.entitlements --timestamp=none "$app"
codesign --verify --strict --deep "$app"
[ "$(codesign -d -r- "$app" 2>/dev/null)" = "designated => $requirement" ] || { echo "magi.app's designated requirement isn't the pinned one in remote/app.requirement" >&2; exit 1; }
mkdir -p "$out"
ditto -c -k --keepParent "$app" "$out/magi.app.zip"
