#!/bin/sh
# Package an already signed/stapled app. All staging stays in the caller's output directory.
set -eu
APW_DMG_ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
APW_APP="${1:?Pass the signed app path}"
APW_OUTPUT="${2:?Pass a new output directory}"
APW_DMG_STAGE="$APW_OUTPUT/dmg-content"
[ ! -e "$APW_DMG_STAGE" ] || exit 1
mkdir -p "$APW_DMG_STAGE"
ditto "$APW_APP" "$APW_DMG_STAGE/Apple 取货监控.app"
ln -s /Applications "$APW_DMG_STAGE/Applications"
for APW_FILE in LICENSE NOTICE THIRD_PARTY_NOTICES.md README.md README.en.md; do
 cp "$APW_DMG_ROOT/$APW_FILE" "$APW_DMG_STAGE/$APW_FILE"
done
APW_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$APW_APP/Contents/Info.plist")"
APW_DMG="$APW_OUTPUT/Apple-Pickup-Watcher_${APW_VERSION}_$(uname -m).dmg"
hdiutil create -volname 'Apple Pickup Watcher' -srcfolder "$APW_DMG_STAGE" -format UDZO "$APW_DMG" >/dev/null
if [ -n "${APW_SIGNING_IDENTITY:-}" ]; then
 codesign --force --timestamp --sign "$APW_SIGNING_IDENTITY" "$APW_DMG"
fi
printf '%s\n' "$APW_DMG"
