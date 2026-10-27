#!/usr/bin/env bash
# Installer test: runs install.sh / uninstall.sh into a throwaway home with
# each profile and checks what lands on disk, what is removed again when the
# selection changes, and that vela reads the selection. Needs the release
# binaries (cargo build --release).
#
#   scripts/install-test.sh
set -euo pipefail

cd "$(dirname "$0")/.."
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
export HOME="$tmp/home" XDG_CONFIG_HOME="$tmp/home/.config" XDG_DATA_HOME="$tmp/home/.local/share" XDG_STATE_HOME="$tmp/home/.local/state"
export XDG_CACHE_HOME="$tmp/home/.cache"
# Never talk to the real daemon (uninstall.sh runs `vela quit`).
export VELA_SOCKET="$tmp/vela.sock"
unset HYPRLAND_INSTANCE_SIGNATURE PREFIX VELA_COMPONENTS
mkdir -p "$HOME/.config/hypr" "$tmp/fake"
echo '-- hyprland.lua' >"$HOME/.config/hypr/hyprland.lua"

# Fakes: systemctl does nothing, claude keeps its MCP servers in a file.
log="$tmp/calls.log"
printf '#!/bin/sh\necho "systemctl $*" >>%s\n' "$log" >"$tmp/fake/systemctl"
cat >"$tmp/fake/claude" <<EOF
#!/bin/sh
echo "claude \$*" >>$log
reg="\${CLAUDE_CONFIG_DIR:-\$HOME}/.mcp-registered"
case "\$1 \$2" in
    "mcp get") [ -f "\$reg" ] ;;
    "mcp add") touch "\$reg" ;;
    "mcp remove") [ -f "\$reg" ] && rm "\$reg" ;;
esac
EOF
chmod +x "$tmp/fake/"*
export PATH="$tmp/fake:$PATH"

BIN="$HOME/.local/bin"
DATA="$XDG_DATA_HOME"
fails=0
check() {
    local name="$1"
    shift
    if "$@"; then :; else
        echo "FAIL: $name"
        fails=$((fails + 1))
    fi
}
no() { ! "$@"; }
key() { grep -qx "$1 = $2" "$DATA/vela/components.toml"; }
installed() {
    echo "--- install.sh $*" >>"$log"
    ./install.sh --no-build "$@" >>"$tmp/out.log" 2>&1 || { cat "$tmp/out.log"; echo "FAIL: install.sh $* exited non-zero"; exit 1; }
}

# minimal: launcher only.
installed --profile minimal -y
check "minimal: vela" test -x "$BIN/vela"
check "minimal: daemon" test -x "$BIN/vela-daemon"
check "minimal: no share picker" test ! -e "$BIN/vela-share-picker"
check "minimal: no pulse" test ! -e "$BIN/vela-pulse"
check "minimal: no pulse menu entry" test ! -e "$DATA/applications/vela-pulse.desktop"
check "minimal: pulse off" key pulse false
check "minimal: no quickshare menu entry" test ! -e "$DATA/applications/vela-quickshare.desktop"
check "minimal: no quickshare service menu" test ! -e "$DATA/kio/servicemenus/vela-quickshare.desktop"
check "minimal: quickshare off" key quickshare false
check "minimal: no shell" test ! -e "$DATA/vela/shell"
check "minimal: vela.lua" test -f "$HOME/.config/hypr/vela.lua"
check "minimal: unit" test -f "$HOME/.config/systemd/user/vela.service"
check "minimal: profile" key profile '"minimal"'
check "minimal: launcher on" key launcher true
check "minimal: panel off" key panel false
check "minimal: share_picker off" key share_picker false
check "minimal: no MCP" test ! -e "$HOME/.mcp-registered"
check "minimal: no xdph.conf" test ! -e "$HOME/.config/hypr/xdph.conf"
check "minimal: vela reads it" sh -c "'$BIN/vela' components | grep -q '\[ \] panel'"
check "components: closed pipe is no panic" sh -c "! ( '$BIN/vela' components | head -1 >/dev/null ) 2>&1 | grep -q panicked"
check "minimal: vela sees the launcher" sh -c "'$BIN/vela' components | grep -q '\[x\] launcher'"

# full: everything.
installed --profile full -y
check "full: share picker" test -x "$BIN/vela-share-picker"
check "full: shell" test -f "$DATA/vela/shell/shell.qml"
check "full: share picker qml" test -f "$DATA/vela/shell/share-picker.qml"
check "full: share picker js" test -f "$DATA/vela/shell/sharepicker/logic.js"
check "full: pulse" test -x "$BIN/vela-pulse"
check "full: pulse qml" test -f "$DATA/vela/shell/pulse.qml"
check "full: pulse js" test -f "$DATA/vela/shell/pulse/Fmt.js"
# Entry files are written last with a fresh stamp, so a running instance
# reloads once more after a copy it may have caught half done.
for e in shell.qml pulse.qml share-picker.qml; do
    check "full: $e written last" grep -q '^// installed [0-9]*$' "$DATA/vela/shell/$e"
done
check "full: pulse menu entry" grep -qx "Exec=$BIN/vela-pulse" "$DATA/applications/vela-pulse.desktop"
check "full: pulse icon" test -f "$DATA/icons/hicolor/scalable/apps/vela-pulse.svg"
check "full: pulse on" key pulse true
check "full: quickshare menu entry" grep -qx "Exec=$BIN/vela quickshare send %F" "$DATA/applications/vela-quickshare.desktop"
check "full: quickshare service menu" grep -qx "Exec=$BIN/vela quickshare send %F" "$DATA/kio/servicemenus/vela-quickshare.desktop"
check "full: quickshare service menu is executable" test -x "$DATA/kio/servicemenus/vela-quickshare.desktop"
check "full: quickshare on" key quickshare true
check "full: firewall hint" grep -q "Quick Share: open TCP 47392 and UDP 5353" "$tmp/out.log"
check "full: no test files" sh -c "! ls '$DATA/vela/shell' | grep -q '^test-'"
check "full: xdph.conf" grep -q "custom_picker_binary = $BIN/vela-share-picker" "$HOME/.config/hypr/xdph.conf"
check "full: MCP registered" test -e "$HOME/.mcp-registered"
check "full: profile" key profile '"full"'
check "full: vela reads it" sh -c "'$BIN/vela' components | grep -q 'profile: full'"

# A rerun without options keeps the selection.
installed -y
check "rerun: still full" key profile '"full"'

# A component added in a newer version: on as in the profile, and new.
sed -i '/^pulse = /d' "$DATA/vela/components.toml"
rm -f "$BIN/vela-pulse"
installed -y
check "new component: pulse on" key pulse true
check "new component: installed" test -x "$BIN/vela-pulse"
check "new component: announced" grep -q "Pulse: Ctrl+Shift+Esc" "$tmp/out.log"
check "new component: still full" key profile '"full"'

# panel, adjusted: custom selection; the share picker and launcher go away.
installed --profile panel --with claude --without share-picker
check "panel: profile is custom" key profile '"custom"'
check "panel: launcher off" key launcher false
check "panel: claude on" key claude true
check "panel: idle on" key idle true
check "panel: share picker removed" test ! -e "$BIN/vela-share-picker"
check "panel: xdph block removed" test ! -e "$HOME/.config/hypr/xdph.conf"
check "panel: shell kept" test -f "$DATA/vela/shell/shell.qml"
check "panel: MCP still registered" test -e "$HOME/.mcp-registered"

# Dropping Claude unregisters the MCP server; Pulse alone keeps the shell
# files, dropping it too removes them.
installed --without claude,panel -y
check "drop: MCP removed" test ! -e "$HOME/.mcp-registered"
check "drop: pulse keeps the shell" test -f "$DATA/vela/shell/pulse.qml"
installed --without pulse -y
check "drop: shell removed" test ! -e "$DATA/vela/shell"
check "drop: pulse removed" test ! -e "$BIN/vela-pulse"
check "drop: pulse menu entry removed" test ! -e "$DATA/applications/vela-pulse.desktop"
check "drop: idle kept" key idle true

# The menu: 5 = custom, toggle 1 (launcher) and 4 (panel) on from the last selection.
echo "--- menu" >>"$log"
printf '5\n1 4\n\n' | ./install.sh --no-build --ask >>"$tmp/out.log" 2>&1 || { cat "$tmp/out.log"; exit 1; }
check "menu: custom" key profile '"custom"'
check "menu: launcher on" key launcher true
check "menu: panel on" key panel true
check "menu: claude still off" key claude false
printf '2\n' | ./install.sh --no-build --ask >>"$tmp/out.log" 2>&1 || { cat "$tmp/out.log"; exit 1; }
check "menu: profile 2 = minimal" key profile '"minimal"'

# Bad input fails before anything changes.
check "unknown profile fails" no ./install.sh --no-build --profile huge -y 2>/dev/null
check "unknown component fails" no ./install.sh --no-build --with teleport -y 2>/dev/null
check "selection unchanged" key profile '"minimal"'

# Quick Share alone, on top of the minimal profile, and off again.
installed --profile minimal --with quickshare -y
check "no panel: warned" grep -q "Quick Share needs the panel component" "$tmp/out.log"
check "no panel: no quickshare menu entry" test ! -e "$DATA/applications/vela-quickshare.desktop"
check "no panel: no quickshare service menu" test ! -e "$DATA/kio/servicemenus/vela-quickshare.desktop"
installed --profile minimal --with panel,quickshare -y
check "with: quickshare menu entry" grep -qx "Exec=$BIN/vela quickshare send %F" "$DATA/applications/vela-quickshare.desktop"
check "with: quickshare service menu" test -x "$DATA/kio/servicemenus/vela-quickshare.desktop"
installed --profile full --without quickshare -y
check "without: quickshare menu entry removed" test ! -e "$DATA/applications/vela-quickshare.desktop"
check "without: quickshare service menu removed" test ! -e "$DATA/kio/servicemenus/vela-quickshare.desktop"
installed --profile full -y

# Settings survive the uninstall, the rest is gone.
mkdir -p "$XDG_CONFIG_HOME/vela" && echo '# mine' >"$XDG_CONFIG_HOME/vela/config.toml"
./uninstall.sh >>"$tmp/out.log" 2>&1
check "uninstall: vela gone" test ! -e "$BIN/vela"
check "uninstall: pulse gone" test ! -e "$BIN/vela-pulse"
check "uninstall: quickshare menu entry gone" test ! -e "$DATA/applications/vela-quickshare.desktop"
check "uninstall: quickshare service menu gone" test ! -e "$DATA/kio/servicemenus/vela-quickshare.desktop"
check "uninstall: selection gone" test ! -e "$DATA/vela/components.toml"
check "uninstall: config kept" test -f "$XDG_CONFIG_HOME/vela/config.toml"
check "uninstall: hyprland.lua kept" test -f "$HOME/.config/hypr/hyprland.lua"

# NixOS: built with nix (a fake one here), no pacman updates by default.
printf 'NAME=NixOS\nID=nixos\n' >"$tmp/os-release-nixos"
mkdir -p "$tmp/nixfake"
cat >"$tmp/nixfake/nix" <<EOF
#!/bin/sh
echo "nix \$*" >>"$log"
while [ \$# -gt 0 ]; do [ "\$1" = --out-link ] && link="\$2"; shift; done
mkdir -p "$tmp/store/bin"
for b in vela vela-daemon vela-share-picker vela-pulse; do printf '#!/bin/sh\necho nix-%s\n' "\$b" >"$tmp/store/bin/\$b"; chmod +x "$tmp/store/bin/\$b"; done
ln -sfn "$tmp/store" "\$link"
EOF
chmod +x "$tmp/nixfake/nix"
nixos() { VELA_OS_RELEASE="$tmp/os-release-nixos" PATH="$tmp/nixfake:$PATH" "$@"; }
echo "--- nixos install.sh" >>"$log"
nixos ./install.sh --profile full -y >>"$tmp/out.log" 2>&1 || { echo "FAIL: nixos: install.sh exited non-zero"; fails=$((fails + 1)); }
check "nixos: built with nix" grep -q "^nix .*build path:.*#default --out-link $DATA/vela/nix-package" "$log"
check "nixos: no cargo build" no grep -q "building (release)" "$tmp/out.log"
check "nixos: binary from the nix package" grep -q nix-vela "$BIN/vela"
check "nixos: full without updates" key updates false
check "nixos: full keeps the rest" key share_picker true
check "nixos: profile stays full" grep -qx 'profile = "full"' "$DATA/vela/components.toml"
nixos ./install.sh --no-build --with updates -y >>"$tmp/out.log" 2>&1
check "nixos: --with updates adds them" key updates true
check "nixos: --no-build reuses the nix package" grep -q nix-vela "$BIN/vela"
./uninstall.sh >>"$tmp/out.log" 2>&1
check "nixos: uninstall drops the gc root" test ! -e "$DATA/vela/nix-package"

# The release archive, installed the way self-update does it (no source, -y).
./uninstall.sh --purge >>"$tmp/out.log" 2>&1 || true
scripts/package.sh 0.0.0-test "$tmp/dist" >>"$tmp/out.log" 2>&1 || { cat "$tmp/out.log"; echo "FAIL: package.sh"; exit 1; }
mkdir -p "$tmp/release" && tar -xzf "$tmp/dist/vela-0.0.0-test-$(uname -m)-linux.tar.gz" -C "$tmp/release" --strip-components=1
check "release: install.sh from the archive" sh -c "cd '$tmp/release' && ./install.sh -y >>'$tmp/out.log' 2>&1"
for b in vela vela-daemon vela-share-picker vela-pulse; do
    check "release: installs $b" test -x "$BIN/$b"
done

if ((fails)); then
    echo "--- output"
    cat "$tmp/out.log"
    exit 1
fi
echo "install.sh: all checks passed"
