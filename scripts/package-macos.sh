#!/bin/sh
# Build first with pnpm tauri build --bundles app. No private settings are copied.
set -eu
APW_PACKAGE_ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd -P)"
APW_APP="${1:?Pass the built .app path}"
APW_OUT="${2:?Pass an output directory}"
[ "$(uname -s)" = Darwin ] || exit 1
[ -f "$APW_APP/Contents/Info.plist" ] || { echo "Built app not found: $APW_APP" >&2; exit 1; }
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APW_APP/Contents/Info.plist")" = com.o1xhack.inventory-rush ] || exit 1
APW_ARCH="$(uname -m)"
APW_STAGE="$APW_OUT/Apple-Pickup-Watcher-macOS-$APW_ARCH"
[ ! -e "$APW_STAGE" ] || { echo "Staging path already exists"; exit 1; }
mkdir -p "$APW_STAGE"
/usr/bin/ditto "$APW_APP" "$APW_STAGE/Apple 取货监控.app"
# Preserve Developer ID signatures and stapled tickets; never replace them with ad-hoc.
if /usr/bin/codesign -dvv "$APW_STAGE/Apple 取货监控.app" 2>&1 | /usr/bin/grep -q '^Authority=Developer ID Application:'; then
 /usr/bin/codesign --verify --deep --strict "$APW_STAGE/Apple 取货监控.app"
else
 /usr/bin/codesign --force --deep --sign - "$APW_STAGE/Apple 取货监控.app"
fi
for APW_FILE in LICENSE NOTICE THIRD_PARTY_NOTICES.md README.md README.en.md 安装并打开.command; do
 cp "$APW_PACKAGE_ROOT/$APW_FILE" "$APW_STAGE/$APW_FILE"
done
"$APW_STAGE/安装并打开.command" --check
/usr/bin/ditto -c -k --sequesterRsrc --keepParent "$APW_STAGE" "$APW_STAGE.zip"
(cd "$APW_OUT" && shasum -a 256 "Apple-Pickup-Watcher-macOS-$APW_ARCH.zip" > SHA256SUMS)
echo "$APW_STAGE.zip"
