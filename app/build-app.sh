#!/bin/bash
# Builds Quill.app from the Swift shell and the Rust core, signed ad hoc with
# the App Sandbox on. For development: a build for the App Store is an Xcode
# archive signed with a real identity.
#
#   app/build-app.sh            debug shell, release core
#   app/build-app.sh release    release shell, release core
#   SANDBOX=0 app/build-app.sh  sign without the sandbox, to tell a sandbox
#                               denial from an ordinary bug
#
# The bundle lands in target/app/Quill.app. There is no Vim in it: mail is
# written in the Vim that comes with macOS, or in the user's own by way of a
# script of theirs.
set -euo pipefail

configuration="${1:-debug}"
# With CDPATH set, `cd` prints the directory it lands in; keep that out of $root.
unset CDPATH
root="$(cd "$(dirname "$0")/.." >/dev/null && pwd)"
out="$root/target/app/Quill.app"

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
# Quill's side of writing a message in Vim.
mkdir -p "$out/Contents/Resources/vim"
cp "$root/app/Resources/vim/quill.vim" "$out/Contents/Resources/vim/quill.vim"
# For a user who writes in their own Vim through a script of their own: the
# helper that script is handed, and a script to start from.
cp "$root/app/Resources/vim/on-terminal" "$out/Contents/Resources/vim/on-terminal"
cp "$root/app/Resources/vim/editor-command.sh" "$out/Contents/Resources/vim/editor-command.sh"
chmod 755 "$out/Contents/Resources/vim/on-terminal" "$out/Contents/Resources/vim/editor-command.sh"

echo "==> sign"
if [ "${SANDBOX:-1}" = "1" ]; then
    codesign --force --sign - --options runtime \
        --entitlements "$root/app/Support/Helper.entitlements" "$out/Contents/Helpers/quill"
    codesign --force --sign - --options runtime \
        --entitlements "$root/app/Support/Quill.entitlements" "$out"
else
    codesign --force --sign - "$out/Contents/Helpers/quill"
    codesign --force --sign - "$out"
fi
codesign --verify --strict "$out"

echo "$out"
