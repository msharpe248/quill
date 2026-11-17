#!/bin/bash
# Makes the Quill.app that is given out, and packs it: a release build of
# the shell and the core, zipped, with the checksum a Homebrew cask wants.
#
#   app/release.sh
#   QUILL_SIGN_IDENTITY="Developer ID Application: NAME (TEAM)" app/release.sh
#   QUILL_SIGN_IDENTITY="..." QUILL_NOTARY_PROFILE=quill app/release.sh
#
# What comes out is target/dist/Quill-VERSION-ARCH.zip, with the version
# from app/Support/Info.plist and the kind of Mac it was built on.
#
# A build to give out is one that signs in to Google as Quill, so this
# stops if app/build-app.sh would have no client to build with (see there).
# QUILL_ALLOW_NO_GOOGLE=1 makes one without all the same.
#
# Signed ad hoc, which is what happens with no identity named, the app
# works but macOS stops it when someone else downloads it: they have to
# allow it by hand in System Settings, under Privacy & Security. Signed
# with a Developer ID and notarized, it simply opens. Notarizing is done
# here when QUILL_NOTARY_PROFILE names a profile saved in the Keychain with
# `xcrun notarytool store-credentials`. That part has not been run yet: it
# needs a developer account.
set -euo pipefail

unset CDPATH
root="$(cd "$(dirname "$0")/.." >/dev/null && pwd)"
dist="$root/target/dist"
app="$dist/Quill.app"

client="${QUILL_GOOGLE_CLIENT_FILE:-$root/google-client.json}"
if [ -z "${QUILL_GOOGLE_CLIENT_ID:-}" ] && [ ! -f "$client" ] && [ "${QUILL_ALLOW_NO_GOOGLE:-0}" != "1" ]; then
    echo "release.sh: there is no google-client.json, so this build would not sign in to Google as Quill." >&2
    echo "Put the file there (see app/build-app.sh), or say QUILL_ALLOW_NO_GOOGLE=1." >&2
    exit 1
fi

version="$(/usr/bin/plutil -extract CFBundleShortVersionString raw -o - "$root/app/Support/Info.plist")"
arch="$(uname -m)"
zip="$dist/Quill-$version-$arch.zip"

mkdir -p "$dist"
QUILL_APP_OUT="$app" "$root/app/build-app.sh" release

pack() {
    rm -f "$zip"
    # As the Finder zips an app: its links and attributes kept.
    /usr/bin/ditto -c -k --sequesterRsrc --keepParent "$app" "$zip"
}

echo "==> pack"
pack

if [ -n "${QUILL_NOTARY_PROFILE:-}" ]; then
    echo "==> notarize"
    xcrun notarytool submit "$zip" --keychain-profile "$QUILL_NOTARY_PROFILE" --wait
    # The ticket goes into the app, and the zip is made again around it.
    xcrun stapler staple "$app"
    pack
fi

echo "==> done"
echo "$zip"
echo "sha256: $(/usr/bin/shasum -a 256 "$zip" | cut -d' ' -f1)"
