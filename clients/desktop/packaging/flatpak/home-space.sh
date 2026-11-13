#!/bin/sh
# Flatpak points XDG_DATA_HOME into the sandbox (~/.var/app/<id>/data). Files that sync moves
# to the trash belong in the user's own trash (~/.local/share/Trash), where the file manager
# shows them, so the app sees the host's data folder; it keeps its own data under
# ~/.local/share/home-space either way.
export XDG_DATA_HOME="$HOME/.local/share"
exec /app/bin/HomeSpace "$@"
