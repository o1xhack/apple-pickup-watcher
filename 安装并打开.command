#!/bin/sh
set -eu
[ "$(uname -s)" = Darwin ] || { echo "仅适用于 macOS"; exit 1; }
APW_INSTALL_ROOT="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)"
APW_SOURCE="$APW_INSTALL_ROOT/Apple 取货监控.app"
APW_DEST="/Applications/Apple 取货监控.app"
[ -d "$APW_SOURCE" ] || { echo "请完整解压安装包后再打开此文件。"; exit 1; }
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$APW_SOURCE/Contents/Info.plist")" = com.o1xhack.inventory-rush ] || exit 1
/usr/bin/codesign --verify --deep "$APW_SOURCE"
if [ "${1:-}" = --check ]; then echo "安装包结构与签名检查通过"; exit 0; fi
[ -w /Applications ] || { echo "应用程序文件夹不可写，请手动拖入并按系统提示授权。"; exit 1; }
# Quit only the same fork; never touch user settings, browsers or network settings.
if /usr/bin/pgrep -f '^/Applications/(Apple 取货监控|库存抢东西).app/Contents/MacOS/apw-app$' >/dev/null; then
 /usr/bin/osascript -e 'tell application id "com.o1xhack.inventory-rush" to quit'
fi
if [ -d "$APW_DEST" ]; then
 mv "$APW_DEST" "$APW_DEST.backup-$(date +%Y%m%d-%H%M%S)"
fi
/usr/bin/ditto "$APW_SOURCE" "$APW_DEST"
/usr/bin/open "$APW_DEST"
if /usr/bin/xcrun stapler validate "$APW_DEST" >/dev/null 2>&1; then
 echo "已安装到应用程序，并验证 Apple 公证票据。首次打开可能需要确认下载来源。"
else
 echo "已安装到应用程序。这是未附公证票据的构建，请按系统安全提示处理。"
fi
