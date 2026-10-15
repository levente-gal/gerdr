#!/bin/sh
# Build gerdr in release mode and install it as a user-level Linux app:
# binary in ~/.local/bin, desktop entry and icon under ~/.local/share, so
# app launchers list it. Re-run after changes to update the install.
set -eu
cd "$(dirname "$0")"

bin_dir="${HOME}/.local/bin"
data_dir="${XDG_DATA_HOME:-${HOME}/.local/share}"
apps_dir="${data_dir}/applications"
icon_dir="${data_dir}/icons/hicolor/scalable/apps"

cargo build --release

mkdir -p "$bin_dir" "$apps_dir" "$icon_dir"
install -m 755 target/release/gerdr "$bin_dir/gerdr"
install -m 644 dist/gerdr.svg "$icon_dir/gerdr.svg"
sed "s|@BIN@|$bin_dir/gerdr|g" dist/gerdr.desktop > "$apps_dir/gerdr.desktop"

command -v update-desktop-database >/dev/null && update-desktop-database "$apps_dir" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "${data_dir}/icons/hicolor" 2>/dev/null || true

echo "installed: $bin_dir/gerdr, $apps_dir/gerdr.desktop"
