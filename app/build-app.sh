#!/bin/bash
# Builds Quill.app from the Swift shell and the Rust core, signed ad hoc
# with the hardened runtime. For development, and for a build to hand out:
# one to hand out is signed with a Developer ID and notarized afterwards.
#
#   app/build-app.sh            debug shell, release core
#   app/build-app.sh release    release shell, release core
#   QUILL_SIGN_IDENTITY="Name" app/build-app.sh
#                               sign with that code-signing identity from
#                               your Keychain instead of ad hoc
#
# The app is not sandboxed. It is given out by itself (a download, Homebrew)
# and not through the App Store, and it runs the user's own editor on their
# own settings, which a sandboxed app cannot. It keeps its settings and
# mail where a terminal's Quill does: ~/Library/Application Support/Quill.
#
# Signed ad hoc, each build is a different program to macOS: the Keychain
# asks again about every saved password after every build, because "Always
# Allow" is granted to that one build. Signed with an identity of your own
# (a Developer ID or Apple Development certificate, or a self-signed
# code-signing certificate made in Keychain Access), every build is the same
# program to the Keychain and it asks once. This script never makes an
# identity: that is for whoever owns the Keychain.
#
# The bundle lands in target/app/Quill.app, or where QUILL_APP_OUT says: the
# bundle of a Quill that is running must not be built over, so a build made
# to check something while one is open goes somewhere else.
set -euo pipefail

configuration="${1:-debug}"
# "-" is ad hoc: no identity at all.
identity="${QUILL_SIGN_IDENTITY:--}"
# With CDPATH set, `cd` prints the directory it lands in; keep that out of $root.
unset CDPATH
root="$(cd "$(dirname "$0")/.." >/dev/null && pwd)"
out="${QUILL_APP_OUT:-$root/target/app/Quill.app}"

# Until the Xcode licence is accepted, `swift` under Xcode refuses to run.
# The Command Line Tools carry the same compiler and can build the package.
if ! swift --version 2>&1 | grep -q "Swift version"; then
    export DEVELOPER_DIR=/Library/Developer/CommandLineTools
fi

echo "==> core"
(cd "$root" && cargo build --release -p quill-cli)

echo "==> shell ($configuration)"
(cd "$root/app" && swift build -c "$configuration")
products="$(cd "$root/app" && swift build -c "$configuration" --show-bin-path)"

echo "==> bundle"
rm -rf "$out"
mkdir -p "$out/Contents/MacOS" "$out/Contents/Helpers" "$out/Contents/Resources"
cp "$root/app/Support/Info.plist" "$out/Contents/Info.plist"
cp "$products/QuillApp" "$out/Contents/MacOS/Quill"
cp "$root/target/release/quill" "$out/Contents/Helpers/quill"
# SwiftPM resource bundles. A signed app cannot carry them at its root, which
# is where SwiftPM looks by default; libghostty-spm looks in Resources first.
find "$products" -maxdepth 1 -name '*.bundle' -exec cp -R {} "$out/Contents/Resources/" \;
# The app's icon, made from the pictures of it at each size.
iconutil --convert icns --output "$out/Contents/Resources/Quill.icns" \
    "$root/app/Resources/icon/Quill.iconset"

echo "==> sign"
# The hardened runtime, which a notarized build must have. No entitlements:
# nothing here needs an exception to it.
codesign --force --sign "$identity" --options runtime "$out/Contents/Helpers/quill"
codesign --force --sign "$identity" --options runtime "$out"
codesign --verify --strict "$out"

echo "$out"
