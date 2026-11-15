#!/bin/bash
# Builds the Vim that ships inside Quill.app, for writing mail in Vim
# (`editor = "vim"` under `[compose]`). The app runs it in a terminal pane of
# its own, inside the app's sandbox.
#
#   app/build-vim.sh
#
# It fetches Vim's source at one pinned commit, builds a terminal-only Vim
# with no scripting languages, GUI, networking or clipboard in it, and leaves
# the binary and the runtime files it needs in target/vim/. `build-app.sh`
# puts them in the bundle when they are there; without them the app is built
# without the Vim option, and says so when it is asked for.
#
# This downloads and compiles other people's code, so it is run by hand and
# never by the other scripts or the checks. It has not been run yet: see
# DECISIONS.md under M9.
#
# Vim's licence (charityware, GPL-compatible) allows shipping it unchanged:
# see target/vim/LICENSE, which goes into the bundle with it.
set -euo pipefail

# The release this is, and exactly which commit: a tag can be moved, a
# commit cannot.
tag="v9.2.1200"
commit="8cd9a0686d2c07e061eb3210227b07e5d29f9720"

unset CDPATH
root="$(cd "$(dirname "$0")/.." >/dev/null && pwd)"
src="$root/target/vim-src"
stage="$root/target/vim-stage"
out="$root/target/vim"

echo "==> source ($tag)"
if [ ! -d "$src/.git" ]; then
    rm -rf "${src:?}"
    git init --quiet "$src"
    git -C "$src" remote add origin https://github.com/vim/vim.git
fi
git -C "$src" fetch --quiet --depth 1 origin "refs/tags/$tag"
got="$(git -C "$src" rev-parse FETCH_HEAD)"
if [ "$got" != "$commit" ]; then
    echo "The tag $tag is now $got, not $commit. Not building it." >&2
    exit 1
fi
git -C "$src" checkout --quiet --force --detach "$commit"
git -C "$src" clean --quiet -fdx

echo "==> configure"
# The prefix is never used to find anything at run time: the app sets
# VIMRUNTIME. It only has to be somewhere that is nobody's real directory.
(cd "$src" && ./configure \
    --prefix=/quill-vim \
    --with-features=huge \
    --enable-multibyte \
    --disable-gui --without-x \
    --disable-nls \
    --disable-netbeans \
    --disable-channel \
    --disable-terminal \
    --disable-canberra \
    --disable-libsodium \
    --disable-gpm --disable-sysmouse \
    --disable-selinux --disable-smack \
    --disable-darwin \
    --with-tlib=ncurses \
    --with-compiledby="Quill" \
    >"$root/target/vim-configure.log" 2>&1) || {
    tail -20 "$root/target/vim-configure.log" >&2
    exit 1
}

echo "==> build"
(cd "$src" && make -j"$(sysctl -n hw.ncpu)" >"$root/target/vim-build.log" 2>&1) || {
    tail -30 "$root/target/vim-build.log" >&2
    exit 1
}

echo "==> stage"
rm -rf "${stage:?}" "${out:?}"
(cd "$src" && make install DESTDIR="$stage" >>"$root/target/vim-build.log" 2>&1)
runtime="$(find "$stage/quill-vim/share/vim" -maxdepth 1 -type d -name 'vim[0-9]*' | head -1)"
if [ -z "$runtime" ] || [ ! -x "$stage/quill-vim/bin/vim" ]; then
    echo "The build did not leave a vim and its runtime where expected." >&2
    exit 1
fi
mkdir -p "$out"
cp "$stage/quill-vim/bin/vim" "$out/vim"
cp -R "$runtime" "$out/runtime"
cp "$src/LICENSE" "$out/LICENSE"
# What a mail editor has no use for: the manual and tutor, other languages'
# messages and keymaps, spelling word lists (macOS checks spelling), and
# the bundled plugins that fetch from the network or read archives.
for extra in doc tutor lang spell keymap macros print tools pack \
    plugin/netrwPlugin.vim plugin/getscriptPlugin.vim plugin/gzip.vim \
    plugin/tarPlugin.vim plugin/zipPlugin.vim plugin/vimballPlugin.vim \
    plugin/tohtml.vim plugin/rrhelper.vim plugin/spellfile.vim \
    autoload/netrw.vim autoload/netrwSettings.vim autoload/netrwFileHandlers.vim \
    autoload/getscript.vim autoload/tar.vim autoload/zip.vim autoload/vimball.vim \
    autoload/gzip.vim; do
    rm -rf "${out:?}/runtime/$extra"
done
rm -rf "${stage:?}"

"$out/vim" --version | head -3
du -sh "$out" | cut -f1
echo "$out"
