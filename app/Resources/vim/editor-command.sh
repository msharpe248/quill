#!/bin/sh
# Quill runs this script when a message is to be written in your own Vim:
# `editor = "script"` under [compose] in Quill's config.
#
# macOS runs it outside Quill's sandbox, as you. The Vim it starts is the one
# you use in a terminal, with your vimrc and your plugins, and it appears in
# Quill's pane.
#
# Where it goes: ~/Library/Application Scripts/dev.quill.Quill/, and it must
# be executable. Only you can put it there. Quill cannot write to that
# folder; "Show Editor Script Folder" in Settings > Composing opens it.
#
# What it is given: first the path of a helper inside Quill.app, which makes
# Quill's pane the terminal of whatever it is told to run; then what Vim is
# to be started with, ending in the draft file. Those arguments switch
# modelines off for the draft and load Quill's commands (:QuillSend and the
# rest), so pass them on.
#
# To use another Vim, change the lines below.

vim=/usr/bin/vim
for candidate in /opt/homebrew/bin/vim /usr/local/bin/vim; do
    if [ -x "$candidate" ]; then
        vim="$candidate"
        break
    fi
done

on_terminal="$1"
shift
exec "$on_terminal" "$vim" "$@"
