#!/bin/bash
set -euo pipefail

SCRIPT_DIRECTORY="$(cd "$(dirname "$0")" && pwd)"
REPOSITORY_ROOT="$(cd "$SCRIPT_DIRECTORY/../.." && pwd)"
PRODUCT_VERSION="$(tr -d '[:space:]' < "$REPOSITORY_ROOT/VERSION")"
DIST_DIRECTORY="$REPOSITORY_ROOT/dist/macos"
APP_DIRECTORY="$DIST_DIRECTORY/WeChatSendGuard.app"
APP_CONTENTS="$APP_DIRECTORY/Contents"
APP_EXECUTABLE="$APP_CONTENTS/MacOS/WeChatSendGuard"
BUILD_TOOL="${CARGO:-cargo}"
SIGNING_IDENTITY="${MACOS_CODESIGN_IDENTITY:--}"
BUILD_KIND="${MACOS_BUILD_KIND:-apple-silicon}"
TEMPORARY_DIRECTORY="$(mktemp -d /tmp/WeChatSendGuard-macos.XXXXXX)"
trap 'rm -rf "$TEMPORARY_DIRECTORY"' EXIT

if [[ ! "$PRODUCT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "VERSION must be SemVer, got: $PRODUCT_VERSION" >&2
    exit 1
fi

case "$BUILD_KIND" in
    apple-silicon)
        PACKAGE_ARCHITECTURE="arm64"
        ;;
    universal)
        PACKAGE_ARCHITECTURE="universal"
        ;;
    *)
        echo "MACOS_BUILD_KIND must be apple-silicon or universal, got: $BUILD_KIND" >&2
        exit 1
        ;;
esac

DISK_IMAGE="$DIST_DIRECTORY/WeChatSendGuard-${PRODUCT_VERSION}-${PACKAGE_ARCHITECTURE}.dmg"

cd "$REPOSITORY_ROOT"
"$BUILD_TOOL" fmt --all -- --check
"$BUILD_TOOL" clippy --workspace --all-targets -- -D warnings
"$BUILD_TOOL" test --workspace
"$BUILD_TOOL" build --release --target aarch64-apple-darwin -p wechat-send-guard
if [[ "$BUILD_KIND" == "universal" ]]; then
    "$BUILD_TOOL" build --release --target x86_64-apple-darwin -p wechat-send-guard
fi

rm -rf "$APP_DIRECTORY"
mkdir -p "$APP_CONTENTS/MacOS" "$APP_CONTENTS/Resources"
if [[ "$BUILD_KIND" == "universal" ]]; then
    lipo -create \
        "$REPOSITORY_ROOT/target/aarch64-apple-darwin/release/WeChatSendGuard" \
        "$REPOSITORY_ROOT/target/x86_64-apple-darwin/release/WeChatSendGuard" \
        -output "$APP_EXECUTABLE"
else
    cp "$REPOSITORY_ROOT/target/aarch64-apple-darwin/release/WeChatSendGuard" \
        "$APP_EXECUTABLE"
fi
sed "s/__VERSION__/$PRODUCT_VERSION/g" "$SCRIPT_DIRECTORY/Info.plist.in" > "$APP_CONTENTS/Info.plist"
chmod 755 "$APP_EXECUTABLE"

if [[ "$SIGNING_IDENTITY" == "-" ]]; then
    codesign --force --deep --options runtime --sign - "$APP_DIRECTORY"
else
    codesign --force --deep --options runtime --timestamp \
        --sign "$SIGNING_IDENTITY" "$APP_DIRECTORY"
fi
codesign --verify --deep --strict --verbose=2 "$APP_DIRECTORY"

mkdir -p "$TEMPORARY_DIRECTORY/image"
ditto "$APP_DIRECTORY" "$TEMPORARY_DIRECTORY/image/WeChatSendGuard.app"
ln -s /Applications "$TEMPORARY_DIRECTORY/image/Applications"
rm -f "$DISK_IMAGE" "$DISK_IMAGE.sha256"
hdiutil create -volname WeChatSendGuard \
    -srcfolder "$TEMPORARY_DIRECTORY/image" \
    -ov -format UDZO "$DISK_IMAGE"
if [[ "$SIGNING_IDENTITY" == "-" ]]; then
    codesign --force --sign - "$DISK_IMAGE"
else
    codesign --force --timestamp --sign "$SIGNING_IDENTITY" "$DISK_IMAGE"
fi

if [[ -n "${MACOS_NOTARY_KEYCHAIN_PROFILE:-}" && "$SIGNING_IDENTITY" != "-" ]]; then
    xcrun notarytool submit "$DISK_IMAGE" \
        --keychain-profile "$MACOS_NOTARY_KEYCHAIN_PROFILE" --wait
    xcrun stapler staple "$DISK_IMAGE"
fi

shasum -a 256 "$DISK_IMAGE" > "$DISK_IMAGE.sha256"
echo "Created $DISK_IMAGE"
