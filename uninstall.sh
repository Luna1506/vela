#!/usr/bin/env bash
# Removes a user installation made by install.sh.
#
#   ./uninstall.sh          remove program files; keep settings and history
#   ./uninstall.sh --purge  also delete ~/.config/vela, ~/.local/state/vela
#                           and ~/.cache/vela
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
DATADIR="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFDIR="${XDG_CONFIG_HOME:-$HOME/.config}"
HYPRDIR="$CONFDIR/hypr"
say() { printf '\033[1;34m::\033[0m %s\n' "$*"; }

"$PREFIX/bin/vela" quit 2>/dev/null || true
systemctl --user stop vela.service 2>/dev/null || true

pkill -f "qs -p $DATADIR/vela/shell/pulse.qml" 2>/dev/null || true
rm -f "$PREFIX/bin/vela" "$PREFIX/bin/vela-daemon" "$PREFIX/bin/vela-share-picker" "$PREFIX/bin/vela-pulse" \
    "$DATADIR/applications/vela.desktop" "$DATADIR/applications/vela-pulse.desktop" \
    "$DATADIR/applications/vela-quickshare.desktop" "$DATADIR/kio/servicemenus/vela-quickshare.desktop" \
    "$DATADIR/icons/hicolor/scalable/apps/vela-pulse.svg" \
    "$DATADIR/icons/hicolor/scalable/apps/vela.svg" \
    "$DATADIR/icons/hicolor/scalable/apps/vela-claude-symbolic.svg" \
    "$DATADIR/icons/hicolor/scalable/apps/vela-drag-handle-symbolic.svg" \
    "$CONFDIR/systemd/user/vela.service" \
    "$HYPRDIR/vela.lua"
rm -rf "$DATADIR/vela/shell"
rm -f "$DATADIR/vela/components.toml"
# NixOS: GC root of the nix-built package (install.sh).
rm -f "$DATADIR/vela/nix-package"
rmdir "$DATADIR/vela" 2>/dev/null || true
if command -v claude >/dev/null; then
    env -u CLAUDE_CONFIG_DIR claude mcp remove --scope user vela >/dev/null 2>&1 || true
    for d in "$HOME"/.claude-accounts/*/; do
        [[ -f "$d.claude.json" ]] && env CLAUDE_CONFIG_DIR="${d%/}" claude mcp remove --scope user vela >/dev/null 2>&1 || true
    done
fi
systemctl --user daemon-reload 2>/dev/null || true
if grep -qs '# vela share picker' "$HYPRDIR/xdph.conf"; then
    sed -i '/# vela share picker/,/^}/d' "$HYPRDIR/xdph.conf"
    [[ -s "$HYPRDIR/xdph.conf" ]] || rm -f "$HYPRDIR/xdph.conf"
    systemctl --user try-restart xdg-desktop-portal-hyprland.service 2>/dev/null || true
    say "screen sharing uses xdph's own picker again"
fi

if [[ -f "$HYPRDIR/hyprland.lua" ]] && grep -qF 'hypr/vela.lua' "$HYPRDIR/hyprland.lua"; then
    cp "$HYPRDIR/hyprland.lua" "$HYPRDIR/hyprland.lua.bak-vela-$(date +%s)"
    sed -i '/-- vela launcher: tap Super to open/d; /hypr\/vela\.lua/d' "$HYPRDIR/hyprland.lua"
    say "removed vela from hyprland.lua (backup created)"
fi

if [[ "${1:-}" == "--purge" ]]; then
    rm -rf "$CONFDIR/vela" "${XDG_STATE_HOME:-$HOME/.local/state}/vela" "${XDG_CACHE_HOME:-$HOME/.cache}/vela"
    say "settings, history and cache deleted"
else
    say "settings kept in $CONFDIR/vela (use --purge to delete them)"
fi
say "vela uninstalled"
