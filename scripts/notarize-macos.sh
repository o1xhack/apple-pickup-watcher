#!/bin/sh
# Run after pnpm tauri build --bundles app. Credentials stay in the caller's environment.
set -eu
APW_APP="${1:?Pass the built .app path}"
APW_NOTARY_DIR="${2:?Pass a new output directory}"
: "${APW_SIGNING_IDENTITY:?Select a Developer ID Application identity from your keychain}"
: "${APP_STORE_CONNECT_API_KEY_FILE:?Set the private API-key file path}"
: "${APP_STORE_CONNECT_KEY_ID:?Set the API key ID}"
: "${APP_STORE_CONNECT_ISSUER_ID:?Set the API issuer ID}"
mkdir -p "$APW_NOTARY_DIR"
codesign --force --deep --options runtime --timestamp --sign "$APW_SIGNING_IDENTITY" "$APW_APP"
codesign --verify --deep --strict "$APW_APP"
ditto -c -k --keepParent "$APW_APP" "$APW_NOTARY_DIR/submission.zip"
xcrun notarytool submit "$APW_NOTARY_DIR/submission.zip" --key "$APP_STORE_CONNECT_API_KEY_FILE" --key-id "$APP_STORE_CONNECT_KEY_ID" --issuer "$APP_STORE_CONNECT_ISSUER_ID" --wait --output-format json > "$APW_NOTARY_DIR/result.json"
python3 - "$APW_NOTARY_DIR/result.json" <<'CHECK'
import json,sys
result=json.load(open(sys.argv[1]))
assert result.get('status')=='Accepted', 'Notarization was not accepted; inspect the result and Apple log'
CHECK
xcrun stapler staple "$APW_APP"
xcrun stapler validate "$APW_APP"
spctl --assess --type execute --verbose=2 "$APW_APP"
