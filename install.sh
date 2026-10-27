#!/usr/bin/env bash
# Builds vela and installs it for the current user (no root needed).
#
#   ./install.sh                    asks which profile / parts to install
#   ./install.sh --profile NAME     full, minimal, launcher or panel
#   ./install.sh --with a,b --without c
#                                   adjust the profile (or the last selection)
#   ./install.sh -y                 no questions: the last selection, else full
#   ./install.sh --ask              always show the menu
#   ./install.sh --list             show profiles and components
#   ./install.sh --hyprland         also add the dofile() line to
#                                   ~/.config/hypr/hyprland.lua (a backup is made)
#   ./install.sh --no-build         install the binaries in target/release as they are
#
# On NixOS it builds with `nix build` (the flake) instead of cargo and leaves
# out the pacman-based updates; the Home Manager module is the declarative way.
#
# Running it again changes the selection: parts no longer selected are removed
# (settings in ~/.config/vela are always kept).
set -euo pipefail

cd "$(dirname "$0")"
PREFIX="${PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
DATADIR="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFDIR="${XDG_CONFIG_HOME:-$HOME/.config}"
HYPRDIR="$CONFDIR/hypr"
MANIFEST="$DATADIR/vela/components.toml"
NIXOS=0
grep -qsxE 'ID="?nixos"?' "${VELA_OS_RELEASE:-/etc/os-release}" && NIXOS=1
# GC root of the package `nix build` made; the installed binaries run from it.
NIX_PACKAGE="$DATADIR/vela/nix-package"

say() { printf '\033[1;34m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*"; }
die() { printf '\033[1;31m!!\033[0m %s\n' "$*" >&2; exit 1; }

# ------------------------------------------------------------ catalog
# data/components.txt is shared with vela (src/components.rs).
PROFILES=() COMPONENTS=()
declare -A PROFILE_DESC=() COMP_DESC=() COMP_PROFILES=()
while read -r kind name rest; do
    case "$kind" in
        profile) PROFILES+=("$name") PROFILE_DESC[$name]="$rest" ;;
        component)
            read -r profiles desc <<<"$rest"
            COMPONENTS+=("$name") COMP_PROFILES[$name]=",$profiles," COMP_DESC[$name]="$desc"
            ;;
    esac
done < <(grep -v '^\s*\(#\|$\)' data/components.txt)

key_of() { printf '%s' "${1//-/_}"; }
# On NixOS no profile has the updates (pacman); --with updates still adds them.
in_profile() {
    [[ "${COMP_PROFILES[$2]}" == *",$1,"* ]] || return 1
    ! { ((NIXOS)) && [[ "$2" == updates ]]; }
}
is_component() { [[ -n "${COMP_DESC[$1]:-}" ]]; }

list() {
    echo "Profiles:"
    for p in "${PROFILES[@]}"; do printf '  %-9s %s\n' "$p" "${PROFILE_DESC[$p]}"; done
    echo "Components:"
    for c in "${COMPONENTS[@]}"; do
        local in=()
        for p in "${PROFILES[@]}"; do in_profile "$p" "$c" && in+=("$p"); done
        printf '  %-13s %s  [%s]\n' "$c" "${COMP_DESC[$c]}" "${in[*]}"
    done
}

# ------------------------------------------------------------ arguments
EDIT_HYPR=0 BUILD=1 ASK=auto PROFILE="" WITH="" WITHOUT=""
while (($#)); do
    case "$1" in
        --hyprland) EDIT_HYPR=1 ;;
        --no-build) BUILD=0 ;;
        -y | --yes) ASK=no ;;
        --ask) ASK=yes ;;
        --profile) PROFILE="${2:?--profile needs a name}"; shift ;;
        --profile=*) PROFILE="${1#*=}" ;;
        --with) WITH+=",${2:?--with needs components}"; shift ;;
        --with=*) WITH+=",${1#*=}" ;;
        --without) WITHOUT+=",${2:?--without needs components}"; shift ;;
        --without=*) WITHOUT+=",${1#*=}" ;;
        --list) list; exit 0 ;;
        -h | --help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1 (see --help)" ;;
    esac
    shift
done
[[ -z "$PROFILE" || " ${PROFILES[*]} " == *" $PROFILE "* ]] || die "unknown profile '$PROFILE' (${PROFILES[*]})"
for c in ${WITH//,/ } ${WITHOUT//,/ }; do is_component "$c" || die "unknown component '$c' (${COMPONENTS[*]})"; done

# ------------------------------------------------------------ selection
declare -A ON=() BEFORE=() NEW=()
OLD_PROFILE=""
if [[ -f "$MANIFEST" ]]; then
    OLD_PROFILE="$(sed -n 's/^profile *= *"\(.*\)"/\1/p' "$MANIFEST")"
    for c in "${COMPONENTS[@]}"; do
        v="$(sed -n "s/^$(key_of "$c") *= *\(true\|false\).*/\1/p" "$MANIFEST")"
        # Added in a later version: selected as in the chosen profile, but
        # not installed yet (so it counts as new below).
        if [[ -z "$v" ]]; then
            in_profile "$OLD_PROFILE" "$c" && NEW[$c]=1
            continue
        fi
        [[ "$v" == true ]] && BEFORE[$c]=1
    done
elif [[ -x "$BINDIR/vela" ]]; then
    # Installed before there was a selection: that was everything.
    for c in "${COMPONENTS[@]}"; do BEFORE[$c]=1; done
fi

use_profile() {
    ON=()
    for c in "${COMPONENTS[@]}"; do in_profile "$1" "$c" && ON[$c]=1; done
    SELECTED_PROFILE="$1"
}

SELECTED_PROFILE=""
if [[ -n "$PROFILE" ]]; then
    use_profile "$PROFILE"
elif [[ -f "$MANIFEST" ]]; then
    for c in "${!BEFORE[@]}" "${!NEW[@]}"; do ON[$c]=1; done
    SELECTED_PROFILE="$OLD_PROFILE"
else
    use_profile full
fi

menu() {
    local default=1 i=1 choice
    echo
    say "What should be installed?"
    for p in "${PROFILES[@]}"; do
        [[ "$p" == "$SELECTED_PROFILE" ]] && default=$i
        printf '   %d) %-9s %s\n' "$i" "$p" "${PROFILE_DESC[$p]}"
        i=$((i + 1))
    done
    printf '   %d) %-9s %s\n' "$i" custom "Pick the parts yourself"
    [[ "$SELECTED_PROFILE" == custom ]] && default=$i
    read -rp "   choice [$default]: " choice || true
    choice="${choice:-$default}"
    if [[ "$choice" =~ ^[0-9]+$ ]] && ((choice >= 1 && choice <= ${#PROFILES[@]})); then
        use_profile "${PROFILES[choice - 1]}"
        return
    fi
    if [[ " ${PROFILES[*]} " == *" $choice "* ]]; then
        use_profile "$choice"
        return
    fi
    [[ "$choice" == "$i" || "$choice" == custom ]] || die "no such choice: $choice"
    while true; do
        echo
        i=1
        for c in "${COMPONENTS[@]}"; do
            printf '   [%s] %d) %-13s %s\n' "$([[ -n "${ON[$c]:-}" ]] && echo x || echo ' ')" "$i" "$c" "${COMP_DESC[$c]}"
            i=$((i + 1))
        done
        read -rp "   toggle (numbers, e.g. \"2 5\"), Enter when done: " choice || choice=""
        [[ -z "$choice" ]] && break
        for n in $choice; do
            if [[ "$n" =~ ^[0-9]+$ ]] && ((n >= 1 && n <= ${#COMPONENTS[@]})); then
                c="${COMPONENTS[n - 1]}"
                if [[ -n "${ON[$c]:-}" ]]; then unset "ON[$c]"; else ON[$c]=1; fi
            else
                warn "no such number: $n"
            fi
        done
    done
    SELECTED_PROFILE=custom
}

if [[ "$ASK" == yes || ("$ASK" == auto && -z "$PROFILE$WITH$WITHOUT" && -t 0) ]]; then
    menu
fi
for c in ${WITH//,/ }; do ON[$c]=1; done
for c in ${WITHOUT//,/ }; do unset "ON[$c]"; done
# A changed profile is a hand-picked selection.
if [[ "$SELECTED_PROFILE" != custom ]]; then
    base="$SELECTED_PROFILE"
    for c in "${COMPONENTS[@]}"; do
        in_profile "$base" "$c" && p=1 || p=""
        [[ "$p" == "${ON[$c]:-}" ]] || SELECTED_PROFILE=custom
    done
fi
has() { [[ -n "${ON[$1]:-}" ]]; }
had() { [[ -n "${BEFORE[$1]:-}" ]]; }
removed() { had "$1" && ! has "$1"; }

picked=()
for c in "${COMPONENTS[@]}"; do has "$c" && picked+=("$c"); done
say "profile: $SELECTED_PROFILE (${picked[*]:-core only})"

# ------------------------------------------------------------ dependencies
# No sources (release archive): the binaries are already there.
[[ -d src ]] || BUILD=0
if command -v pacman >/dev/null; then
    missing=()
    need=(gtk4 gtk4-layer-shell libadwaita)
    ((BUILD)) && need+=(rust)
    for p in "${need[@]}"; do
        pacman -Qq "$p" >/dev/null 2>&1 || missing+=("$p")
    done
    if ((${#missing[@]})); then
        warn "missing packages: ${missing[*]}"
        warn "install them with: sudo pacman -S --needed ${missing[*]}"
        exit 1
    fi
    optional=(xdg-utils)
    has launcher && optional+=(plocate)
    { has launcher || has claude; } && optional+=(kitty)
    { has panel || has share-picker || has pulse; } && optional+=(quickshell)
    has idle && optional+=(hypridle)
    has share-picker && optional+=(slurp xdg-desktop-portal-hyprland)
    for p in "${optional[@]}"; do
        pacman -Qq "$p" >/dev/null 2>&1 || warn "optional package '$p' is not installed"
    done
fi
if has claude; then
    command -v claude >/dev/null || [[ -x "$HOME/.local/bin/claude" ]] || warn "Claude Code (claude) not found — Ask Claude will report that"
fi

# NixOS has no system GTK for a cargo build or the prebuilt binaries: the
# flake's package brings its own, kept alive by the out-link.
BUILT=target/release
if ((NIXOS)); then
    if ((BUILD)); then
        [[ -f flake.nix ]] || die "NixOS needs the source tree (git clone) to build with nix"
        command -v nix >/dev/null || die "nix not found"
        say "building with nix (flake package)"
        mkdir -p "$(dirname "$NIX_PACKAGE")"
        nix --extra-experimental-features 'nix-command flakes' build "path:$PWD#default" --out-link "$NIX_PACKAGE"
    fi
    if [[ -x "$NIX_PACKAGE/bin/vela" ]]; then
        BUILT="$NIX_PACKAGE/bin"
    else
        warn "no nix build at $NIX_PACKAGE; installing $BUILT, which may not find GTK on NixOS"
    fi
elif ((BUILD)); then
    say "building (release)"
    cargo build --release --locked
fi
for b in vela vela-daemon vela-share-picker vela-pulse; do
    [[ -x "$BUILT/$b" ]] || die "$BUILT/$b is missing (build first or drop --no-build)"
done

# ------------------------------------------------------------ core
say "installing to $PREFIX"
install -Dm755 "$BUILT/vela" "$BINDIR/vela"
install -Dm755 "$BUILT/vela-daemon" "$BINDIR/vela-daemon"
for icon in data/icons/*.svg; do
    install -Dm644 "$icon" "$DATADIR/icons/hicolor/scalable/apps/$(basename "$icon")"
done
sed "s|^Exec=vela|Exec=$BINDIR/vela|" data/vela.desktop | install -Dm644 /dev/stdin "$DATADIR/applications/vela.desktop"
sed "s|@BINDIR@|$BINDIR|" contrib/systemd/vela.service.in | install -Dm644 /dev/stdin "$CONFDIR/systemd/user/vela.service"
install -Dm644 contrib/hyprland/vela.lua "$HYPRDIR/vela.lua"

# Stops a process started by vela.lua (`<bin> shell`, `<bin> idle`); the
# pattern is anchored so it can't hit this script.
stop() { pkill -f "^$BINDIR/vela $1\$" 2>/dev/null || true; }

# ------------------------------------------------------------ control center files
# The panel, the share picker and Pulse all run from the shell directory.
if has panel || has share-picker || has pulse; then
    # Overwrite in place (cp keeps the files) so a running shell notices the
    # change and reloads itself; drop files that no longer exist.
    mkdir -p "$DATADIR/vela/shell"
    entries=(shell.qml pulse.qml share-picker.qml)
    (cd shell && find . -type f \( -name '*.qml' -o -name '*.js' -o -name '*.svg' \) ! -name 'test-*' ! -path ./shell.qml ! -path ./pulse.qml ! -path ./share-picker.qml -exec sh -c 'mkdir -p "$1/$(dirname "$2")" && cp "$2" "$1/$2"' _ "$DATADIR/vela/shell" {} \;)
    # The entry files last, after a pause and always different: a running shell,
    # Pulse or share picker may have reloaded mid-copy (old and new files mixed,
    # or a file read while empty — Pulse then lost its window for good); this
    # separate change makes each reload once more with every file in place.
    pgrep -f "qs -p $DATADIR/vela/shell" >/dev/null 2>&1 && sleep 1
    for e in "${entries[@]}"; do
        { cat "shell/$e"; printf '// installed %s\n' "$(date +%s)"; } >"$DATADIR/vela/shell/$e"
    done
    (cd "$DATADIR/vela/shell" && find . -type f \( -name '*.qml' -o -name '*.js' -o -name '*.svg' \) | while read -r f; do [[ -f "$OLDPWD/shell/$f" ]] || rm -f "$f"; done)
    command -v qs >/dev/null || warn "Quickshell (qs) not found — the control center, the share picker and Pulse need it"
else
    if removed panel; then
        stop shell
        say "control center removed"
    fi
    rm -rf "$DATADIR/vela/shell"
fi
if has panel && ! had panel; then
    say "control center: starts with Hyprland (now: setsid $BINDIR/vela shell)"
fi

# Panel dropped but the shell files stay (share picker or Pulse).
if removed panel && { has share-picker || has pulse; }; then
    stop shell
    say "control center removed"
fi

# ------------------------------------------------------------ Pulse
# The task manager: its own program (menu entry, vela-pulse), Ctrl+Shift+Esc.
if has pulse; then
    install -Dm755 "$BUILT/vela-pulse" "$BINDIR/vela-pulse"
    sed "s|^Exec=vela-pulse|Exec=$BINDIR/vela-pulse|" data/vela-pulse.desktop | install -Dm644 /dev/stdin "$DATADIR/applications/vela-pulse.desktop"
    had pulse || say "Pulse: Ctrl+Shift+Esc, \"Pulse\" in the app menu, or: vela-pulse"
else
    rm -f "$BINDIR/vela-pulse" "$DATADIR/applications/vela-pulse.desktop"
    if removed pulse; then
        pkill -f "qs -p $DATADIR/vela/shell/pulse.qml" 2>/dev/null || true
        say "Pulse removed"
    fi
fi

# ------------------------------------------------------------ Quick Share
# "Send with Quick Share" in file managers: a menu entry (Nautilus, "Open
# with") and a service menu (Dolphin; must be executable or it stays hidden).
if has quickshare && ! has panel; then
    warn "Quick Share needs the panel component; skipping its file manager entries"
fi
if has quickshare && has panel; then
    sed "s|^Exec=vela|Exec=$BINDIR/vela|" data/vela-quickshare.desktop | install -Dm644 /dev/stdin "$DATADIR/applications/vela-quickshare.desktop"
    sed "s|^Exec=vela|Exec=$BINDIR/vela|" data/vela-quickshare-servicemenu.desktop | install -Dm755 /dev/stdin "$DATADIR/kio/servicemenus/vela-quickshare.desktop"
    had quickshare || say "Quick Share: open TCP 47392 and UDP 5353 in your firewall (README → Quick Share)"
else
    rm -f "$DATADIR/applications/vela-quickshare.desktop" "$DATADIR/kio/servicemenus/vela-quickshare.desktop"
fi

# ------------------------------------------------------------ idle
if removed idle; then
    stop idle
    say "idle handling removed (start hypridle yourself if you want it)"
elif has idle && ! had idle; then
    say "idle handling: starts with Hyprland; don't also start hypridle yourself"
fi

# ------------------------------------------------------------ Claude Code
# The vela MCP server lets Claude Code change Hyprland/vela settings, in the
# default profile and every ccacct profile (~/.claude-accounts/<name>, each
# has its own .claude.json; *.lock are ccacct's leftovers).
if command -v claude >/dev/null && { has claude || removed claude; }; then
    profiles=("")
    for d in "$HOME"/.claude-accounts/*/; do
        [[ -d "$d" && "$d" != *.lock/ && -f "$d.claude.json" ]] && profiles+=("${d%/}")
    done
    for dir in "${profiles[@]}"; do
        name="${dir:+$(basename "$dir")}"
        name="${name:-default}"
        run=(env -u CLAUDE_CONFIG_DIR)
        [[ -n "$dir" ]] && run=(env CLAUDE_CONFIG_DIR="$dir")
        if ! has claude; then
            "${run[@]}" claude mcp remove --scope user vela >/dev/null 2>&1 && say "removed the vela MCP server from Claude Code ($name)"
        elif "${run[@]}" claude mcp get vela >/dev/null 2>&1; then
            say "Claude Code ($name) already knows the vela MCP server"
        elif "${run[@]}" claude mcp add --scope user vela -- "$BINDIR/vela" mcp >/dev/null 2>&1; then
            say "registered the vela MCP server with Claude Code ($name)"
        else
            warn "could not register the MCP server for $name; run: claude mcp add --scope user vela -- $BINDIR/vela mcp"
        fi
    done
fi

# ------------------------------------------------------------ screen sharing
# vela's picker for xdg-desktop-portal-hyprland, unless another one is
# configured. xdph reads its config only at start.
XDPH="$HYPRDIR/xdph.conf"
if has share-picker; then
    install -Dm755 "$BUILT/vela-share-picker" "$BINDIR/vela-share-picker"
    if grep -qs 'custom_picker_binary' "$XDPH" && ! grep -qs 'vela-share-picker' "$XDPH"; then
        warn "$XDPH already sets another share picker; vela's: custom_picker_binary = $BINDIR/vela-share-picker"
    elif ! grep -qs "custom_picker_binary = $BINDIR/vela-share-picker" "$XDPH"; then
        [[ -f "$XDPH" ]] && sed -i '/# vela share picker/,/^}/d' "$XDPH"
        printf '# vela share picker (removed by uninstall.sh)\nscreencopy {\n    custom_picker_binary = %s\n}\n' "$BINDIR/vela-share-picker" >>"$XDPH"
        say "screen sharing now uses vela's picker ($XDPH)"
        systemctl --user try-restart xdg-desktop-portal-hyprland.service 2>/dev/null || true
    fi
else
    rm -f "$BINDIR/vela-share-picker"
    if grep -qs '# vela share picker' "$XDPH"; then
        sed -i '/# vela share picker/,/^}/d' "$XDPH"
        [[ -s "$XDPH" ]] || rm -f "$XDPH"
        systemctl --user try-restart xdg-desktop-portal-hyprland.service 2>/dev/null || true
        say "screen sharing uses xdph's own picker again"
    fi
fi

# ------------------------------------------------------------ selection file
# Read by vela (src/components.rs) and vela.lua.
{
    echo "# Written by install.sh; run it again to change the selection."
    printf 'profile = "%s"\n' "$SELECTED_PROFILE"
    for c in "${COMPONENTS[@]}"; do
        printf '%s = %s\n' "$(key_of "$c")" "$(has "$c" && echo true || echo false)"
    done
} | install -Dm644 /dev/stdin "$MANIFEST"

systemctl --user daemon-reload 2>/dev/null || true
gtk-update-icon-cache -q -t "$DATADIR/icons/hicolor" 2>/dev/null || true

# ------------------------------------------------------------ Hyprland
LINE='dofile(os.getenv("HOME") .. "/.config/hypr/vela.lua").setup()'
if [[ -f "$HYPRDIR/hyprland.lua" ]]; then
    if grep -qF 'hypr/vela.lua' "$HYPRDIR/hyprland.lua"; then
        say "hyprland.lua already loads vela.lua"
    elif [[ "$(readlink -f "$HYPRDIR/hyprland.lua")" == /nix/store/* ]]; then
        say "hyprland.lua comes from Home Manager; add to wayland.windowManager.hyprland.extraConfig:"
        echo "    $LINE"
    elif ((EDIT_HYPR)); then
        cp "$HYPRDIR/hyprland.lua" "$HYPRDIR/hyprland.lua.bak-vela-$(date +%s)"
        printf '\n-- vela launcher: tap Super to open\n%s\n' "$LINE" >>"$HYPRDIR/hyprland.lua"
        say "added vela to hyprland.lua (backup created)"
    else
        say "add this line to $HYPRDIR/hyprland.lua (or rerun with --hyprland):"
        echo "    $LINE"
    fi
    # vela.lua reads the selection when the config loads.
    lua_changed=0
    for c in launcher panel idle hyprland pulse; do
        [[ "$(has "$c" && echo 1)" == "$(had "$c" && echo 1)" ]] || lua_changed=1
    done
    if ((lua_changed)) && [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]] && grep -qF 'hypr/vela.lua' "$HYPRDIR/hyprland.lua"; then
        hyprctl reload config-only >/dev/null 2>&1 && say "Hyprland reloaded vela.lua"
    fi
else
    warn "no Lua Hyprland config found; see README → Hyprland for hyprland.conf users"
fi

# Restart a running daemon so the new binary and selection are used.
if [[ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]]; then
    systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP XDG_SESSION_TYPE DISPLAY
    systemctl --user restart vela.service && say "vela daemon (re)started"
elif systemctl --user -q is-active vela.service 2>/dev/null; then
    systemctl --user restart vela.service
fi

case ":$PATH:" in *":$BINDIR:"*) ;; *) warn "$BINDIR is not in PATH" ;; esac
if has launcher; then
    say "done — tap Super in Hyprland, or run: vela"
else
    say "done — settings: vela settings"
fi
