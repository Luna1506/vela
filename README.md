<img alt="vela — launcher, control center and every Hyprland setting" src="docs/readme/hero-light.svg#gh-light-mode-only" width="100%">
<img alt="vela — launcher, control center and every Hyprland setting" src="docs/readme/hero-dark.svg#gh-dark-mode-only" width="100%">

<p align="center">
  <a href="https://github.com/Raindancer118/vela/releases"><img alt="release" src="https://img.shields.io/github/v/release/Raindancer118/vela?style=for-the-badge&labelColor=0d1330&color=7aa2f7&label=release"></a>
  <a href="https://raindancer118.github.io/vela/"><img alt="changelog" src="https://img.shields.io/badge/changelog-every_tag-b4a0f5?style=for-the-badge&labelColor=0d1330"></a>
  <a href="https://github.com/Raindancer118/vela/actions"><img alt="ci" src="https://img.shields.io/github/actions/workflow/status/Raindancer118/vela/ci.yml?branch=main&style=for-the-badge&labelColor=0d1330&color=9ece6a&label=ci"></a>
  <img alt="Hyprland 0.55+" src="https://img.shields.io/badge/hyprland-0.55%2B-a9c1ff?style=for-the-badge&labelColor=0d1330">
  <img alt="Rust" src="https://img.shields.io/badge/rust-2024-f7a072?style=for-the-badge&labelColor=0d1330&logo=rust&logoColor=f7a072">
  <a href="LICENSE"><img alt="MIT" src="https://img.shields.io/github/license/Raindancer118/vela?style=for-the-badge&labelColor=0d1330&color=d97757"></a>
</p>

<p align="center">
  <b>Tap Super.</b> Find any app, any file, any setting.<br>
  Change Hyprland with sliders instead of a config file, or just tell Claude what you want.
</p>

<p align="center">
  <a href="#the-launcher">Launcher</a> &nbsp;·&nbsp;
  <a href="#the-control-center">Control center</a> &nbsp;·&nbsp;
  <a href="#pulse">Pulse</a> &nbsp;·&nbsp;
  <a href="#quick-share">Quick Share</a> &nbsp;·&nbsp;
  <a href="#hyprland-without-the-config-file">Hyprland settings</a> &nbsp;·&nbsp;
  <a href="#just-say-it">Claude</a> &nbsp;·&nbsp;
  <a href="#get-it">Install</a> &nbsp;·&nbsp;
  <a href="#under-the-hood">How it works</a>
</p>

<br>

<a id="the-launcher"></a>
<img alt="The launcher" src="docs/readme/h-launcher-light.svg#gh-light-mode-only" width="100%">
<img alt="The launcher" src="docs/readme/h-launcher-dark.svg#gh-dark-mode-only" width="100%">

<img alt="The launcher: app grid, file results and Ask Claude" src="docs/readme/launcher.png" width="100%">

<table>
<tr>
<td width="33%" valign="top">

**Tap, don't hold.**
A tap on <kbd>Super</kbd> opens it. <kbd>Super</kbd>+<kbd>Q</kbd>, <kbd>Super</kbd>+<kbd>1</kbd> and
every other shortcut keep working and never open it by accident, even
combinations that have no bind.

</td>
<td width="33%" valign="top">

**One field, everything.**
Fuzzy app search with desktop actions, a file search whose index covers
100 000 files in about 50 ms, and your pinned apps as a grid while the field
is empty.

</td>
<td width="33%" valign="top">

**Ask Claude.**
Type a question and <kbd>Shift</kbd>+<kbd>Enter</kbd> hands it to Claude Code in
your terminal, as a single argument, never through a shell.

</td>
</tr>
</table>

<details>
<summary><b>Keys</b></summary>

| Key | Action |
| --- | --- |
| type | search apps, files and Claude |
| <kbd>←</kbd><kbd>↑</kbd><kbd>↓</kbd><kbd>→</kbd> | move in the grid; <kbd>↑</kbd><kbd>↓</kbd> / <kbd>Tab</kbd> in results |
| <kbd>Enter</kbd> | open the selected entry |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | send the whole input to Claude Code |
| <kbd>Ctrl</kbd>+<kbd>Enter</kbd> | show the selected file in its folder |
| <kbd>←</kbd> / <kbd>→</kbd> | open the selected result on the monitor left / right of this one |
| <kbd>Ctrl</kbd>+<kbd>,</kbd> | settings |
| <kbd>Esc</kbd> | close |
| right-click | pin / unpin / move tiles |

A tap on <kbd>Super</kbd> opens the launcher. *Settings → Shortcuts →
Launcher* adds a shortcut of your own for it (e.g. <kbd>Super</kbd>+<kbd>Space</kbd>)
and can switch the Super tap off; *Control center* there sets the control
center's shortcut (tap toggles, hold peeks), which wins over `panel_peek` in
`setup()`. Both work on NixOS too.

While the input reads like a prompt ("Explain RSA to me", or anything ending
in `?`), or while <kbd>Shift</kbd> is held, the search icon turns into the Claude
logo: <kbd>Enter</kbd> goes to Claude.

</details>

<br>

<a id="the-control-center"></a>
<img alt="The control center" src="docs/readme/h-center-light.svg#gh-light-mode-only" width="100%">
<img alt="The control center" src="docs/readme/h-center-dark.svg#gh-dark-mode-only" width="100%">

`vela shell` runs a control center built on Quickshell, in exactly the look
of the launcher and the settings: the same font, the same Adwaita icons, the
same accent selection, sliders and boxed lists.

<img alt="Notification popups, the sound detail view and the control center panel" src="docs/readme/control-center.png" width="100%">

<table>
<tr>
<td width="50%" valign="top">

- **Quick settings**: Wi-Fi, Bluetooth, sound, power profile, night light, do not disturb; each tile opens its details
- **Notifications**: popups, groups per app, compact rows that unfold when the pointer rests on them
- **Power & idle**: dim, lock, screen off and sleep, each with its own switch and delay (runs hypridle for you)

</td>
<td width="50%" valign="top">

- **Claude usage**: 5-hour and weekly limits of every Claude Code profile, with the plan as Anthropic has it now and the reset time
- **Workspace dots** when you switch workspaces
- **Updates**: a tile with what is pending (repositories, AUR, Flatpak) that opens Settings → Updates
- **Screen-share picker** for xdg-desktop-portal-hyprland: screens as they stand on your desk, windows and a region, all with live previews
- **Tap or hold** a shortcut (`panel_peek`, e.g. <kbd>Super</kbd>+<kbd>T</kbd>): a tap keeps it open, holding shows it until you let go
- Speaks your language (German above)

</td>
</tr>
</table>

<br>

<a id="pulse"></a>
<img alt="Pulse, the task manager" src="docs/readme/h-pulse-light.svg#gh-light-mode-only" width="100%">
<img alt="Pulse, the task manager" src="docs/readme/h-pulse-dark.svg#gh-dark-mode-only" width="100%">

<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Esc</kbd>, "Pulse" in the app menu, or `vela-pulse`: a task manager
in the same look, laid out like the ones you know (Windows 11, Mission Center) and a step further:
it doesn't just show numbers, it tells you what is wrong and fixes it with one click.

<img alt="Pulse: health, live tiles, top consumers and activity" src="docs/readme/pulse.png" width="100%">

<table>
<tr>
<td width="50%" valign="top">

- **Apps & processes**: grouped into apps with windows, background apps, commands running in terminal tabs, services and the system (from systemd's cgroups, so an app's 30 processes are one row); unfold into processes, or all processes as a list or tree; cells tinted by load; end, force quit, restart, pause, efficiency mode, signals, priority
- **Performance**: CPU (per core), memory with what it's made of, every GPU (NVIDIA only asked while awake, so it can sleep), disks, network, battery and every sensor and fan, with 1- or 5-minute graphs that glide or step
- **Services**: systemd user and system units, start, stop, restart, autostart

</td>
<td width="50%" valign="top">

- **Diagnosis**: overload, low memory, swapping, programs waiting for the disk, heat and throttling, a power profile that holds you back, full disks, apps that hang (Hyprland's "not responding"), crash loops (core dumps), failed services, runaway CPU, memory leaks, outdated code after an update or a new kernel (on NixOS: a store path the profile no longer uses); each with the fix as a button
- **Ask Claude**: hands Claude Code a snapshot to dig deeper; the `pulse_*` MCP tools let Claude read and act on it too
- **Activity**: what started, closed, crashed, hung or ran out of memory, also while the window was closed (the daemon keeps a light record; Settings → Pulse)

</td>
</tr>
</table>

<br>

<a id="quick-share"></a>

## Quick Share

Send and receive files, text and links with Android and Samsung phones nearby
(Quick Share, formerly Nearby Share). Files are sent uncompressed and saved as
they are. A tile in the control center opens it: the *Receive* tab shows how
this PC appears to phones, the *Send* tab lists the phones that are nearby.

- **Network**: in this version the PC and the phone must be on the same Wi-Fi network, or the PC on the phone's hotspot.
- **On the phone**: set Quick Share visibility to "Everyone" (German: "Alle") while you send to the PC or receive from it.
- **Receiving**: a window asks about every offer. Nothing is accepted automatically. Check that the PIN shown on the PC matches the one on the phone, then choose the folder (the first one is your Downloads folder) and accept.
- **Sending**: pick files in the *Send* tab, or run `vela quickshare send <files…>` (directories are not sent). Dolphin gets "Send with Quick Share" in the right-click menu (`share/kio/servicemenus/vela-quickshare.desktop`); other file managers may list "Quick Share" under "Open with", depending on how they treat hidden entries (`vela-quickshare.desktop`). `vela quickshare send` always works. Without a running control center the command prints an error.
- **Turning it off**: the switch in the Quick Share view. While it is off, vela runs no Quick Share process and opens no port. *Settings → Control center → Quick Share tile* hides the tile; the `quickshare` component (`./install.sh --without quickshare`) removes the feature.

Quick Share needs a TCP port (47392) for the transfer and UDP 5353 for mDNS
discovery. Open them in your firewall:

```sh
# NixOS
networking.firewall.allowedTCPPorts = [ 47392 ];
networking.firewall.allowedUDPPorts = [ 5353 ];

# ufw
sudo ufw allow 47392/tcp && sudo ufw allow 5353/udp

# firewalld
sudo firewall-cmd --add-port=47392/tcp --add-service=mdns --permanent && sudo firewall-cmd --reload
```

The name phones see and the port are set in the `[quickshare]` section of
[`data/config.example.toml`](data/config.example.toml) (`name`, empty = the hostname; `port`).

Known limitations, found with a Samsung Galaxy S25 (One UI 9):

- If *Share with Apple devices* is on in the phone's Quick Share settings, the phone looks for PCs over Bluetooth only.
- Some phones announce themselves without a name; vela then lists them as "Android phone".

For phones that look for PCs over Bluetooth only, vela also advertises over
Bluetooth while the PC's Bluetooth is on (not yet confirmed with every
phone). A transfer from such a phone starts over Bluetooth and switches to
Wi-Fi when the phone can reach the PC there; otherwise it continues over
Bluetooth, which is slow.

<br>

<a id="hyprland-without-the-config-file"></a>
<img alt="Hyprland, without the config file" src="docs/readme/h-hyprland-light.svg#gh-light-mode-only" width="100%">
<img alt="Hyprland, without the config file" src="docs/readme/h-hyprland-dark.svg#gh-dark-mode-only" width="100%">

The settings open on a single search field. Type what you are looking for, in
English or German, and the settings appear right below it, ready to use: no
need to find the page they live on.

<p align="center">
  <img alt="Typing into the settings search: 'Abstand zwischen Fenstern' finds the gap settings, 'blur' the blur sliders" src="docs/readme/demo-search.webp" width="88%">
</p>

Every option Hyprland has is in there. The ones you change most have their own
pages with sliders, switches and colour pickers, and each change shows up on
screen the moment you make it.

<img alt="Monitors, blur and effects, shortcuts and window rules" src="docs/readme/bento-hyprland.png" width="100%">

<table>
<tr>
<td width="50%" valign="top">

**Windows & gaps** · gaps to the screen edges on or off per edge, their width, gaps between windows, border width and gradient colours, layout, snapping

**Blur & effects** · blurriness, passes, brightness, contrast, vibrancy, noise, rounded corners, transparency, dimming, shadow, glow

**Animations** · on or off, duration, curve (beziers and springs, plus a few of vela's own) and style, for each part: opening, closing, workspaces, fades, layers

**Input** · keyboard layout and repeat, mouse speed and acceleration, touchpad, workspace swipe, cursor hiding and zoom

</td>
<td width="50%" valign="top">

**Monitors** · drag them into place, resolution, refresh rate, only scales that give sharp pixels, rotation, VRR, on and off, with a 15-second *keep or revert*

**Shortcuts** · record keys and pick from 22 actions or any command; take over or switch off the shortcuts from your hyprland.lua

**Window rules** · pick an open window, then float it, size it, send it to a workspace, pin it, make it opaque … (24 effects)

**Autostart · Layouts · Behaviour · All options** · the rest, down to the last of Hyprland's ~350 options, searchable

</td>
</tr>
</table>

**Updates** · update everything (pacman/paru or yay, Flatpak), only refresh
the package database, or tick single apps; the output runs live below. When
something fails, **Fix with Claude** opens Claude Code with the error and the
log in a terminal on your minimized windows (`special:minimized`, no focus
stolen) and it repairs the update there; *Show* brings it over.

> [!TIP]
> Your `hyprland.lua` is never rewritten. vela keeps what you change in
> `~/.config/vela/hyprland.toml`, applies it live with `hyprctl eval`, and
> loads it at the end of your config. Only what you touched is overridden;
> the ↶ next to a setting gives you your own value back.

<details>
<summary><b>All options, searchable</b></summary>
<br>
<img alt="Every Hyprland option on one page" src="docs/readme/shot-all-options.png" width="100%">
</details>

<br>

<a id="just-say-it"></a>
<img alt="Just say it" src="docs/readme/h-claude-light.svg#gh-light-mode-only" width="100%">
<img alt="Just say it" src="docs/readme/h-claude-dark.svg#gh-dark-mode-only" width="100%">

<table>
<tr>
<td width="55%" valign="middle">
<img alt="The settings search with an Ask Claude card above the matching settings" src="docs/readme/shot-search.png" width="100%">
</td>
<td width="45%" valign="middle">

Write what you want instead of looking for it:

> *Make the gaps between windows smaller and the blur a bit stronger.*

> *Float pavucontrol, centred, 900 by 600.*

> *Super+B should open Firefox.*

Press <kbd>Ctrl</kbd>+<kbd>Enter</kbd> and Claude Code does it through
**`vela mcp`**, vela's own MCP server. Every change goes the same way as from
the settings window: live, kept, and one click away from undone.

</td>
</tr>
</table>

<details>
<summary><b>The MCP tools</b></summary>

`install.sh` registers the server with Claude Code
(`claude mcp add --scope user vela -- vela mcp`), in the default profile and
in every ccacct profile (`~/.claude-accounts/*`).

| Tool | What it does |
| --- | --- |
| `search_hyprland_options` | find options by words in name or description, with value, default, range and choices |
| `set_hyprland_options` | set several options at once (numbers, colours, gradients, gaps, choices by name) |
| `reset_hyprland_options` | give options, animations or monitors back to hyprland.lua |
| `list_hyprland_animations` · `set_hyprland_animation` | what each animation does, and change it |
| `list_monitors` · `set_monitor` | modes, valid scales, position, rotation, VRR |
| `list_shortcuts` · `set_shortcut` · `remove_shortcut` | vela's shortcuts and the keys hyprland.lua uses |
| `list_window_rules` · `set_window_rule` · `remove_window_rule` | rules, their effects and the windows open now |
| `set_autostart` | commands started with Hyprland |
| `get_vela_settings` · `set_vela_setting` · `open_vela_settings` | vela itself |

</details>

<br>

<a id="get-it"></a>
<img alt="Get it" src="docs/readme/h-install-light.svg#gh-light-mode-only" width="100%">
<img alt="Get it" src="docs/readme/h-install-dark.svg#gh-dark-mode-only" width="100%">

On Arch Linux, with Hyprland 0.55 or newer (Lua config):

```sh
sudo pacman -S --needed gtk4 gtk4-layer-shell libadwaita rust   # plocate, quickshell: optional
git clone https://github.com/Raindancer118/vela && cd vela && ./install.sh --hyprland
```

That's all: tap <kbd>Super</kbd>. Everything lands in your home directory, and
`--hyprland` adds one line to the end of `~/.config/hypr/hyprland.lua` (after a
backup):

```lua
dofile(os.getenv("HOME") .. "/.config/hypr/vela.lua").setup()
```

<details>
<summary><b>Pick what you want: profiles and components</b></summary>

`install.sh` asks which profile to install, or lets you pick the parts one by
one. Running it again changes the selection; parts you drop are removed again
(settings stay). Settings and Appearance are always there.

| Profile | Contains |
| --- | --- |
| `full` | everything (default) |
| `minimal` | the launcher |
| `launcher` | launcher, Claude Code, Hyprland settings, updates, Pulse |
| `panel` | control center, idle, screen-share picker, updates, Pulse, Quick Share (no launcher) |

Components: `launcher`, `claude`, `hyprland`, `panel`, `idle`,
`share-picker`, `updates`, `pulse`, `quickshare` (needs `panel`) (`./install.sh --list` describes them).

```sh
./install.sh --profile panel -y                          # no questions
./install.sh --profile launcher --without claude --with idle
./install.sh -y                                          # update, keep the selection
```

The selection lives in `~/.local/share/vela/components.toml`; vela and
`vela.lua` leave out what isn't installed (`vela components` shows it, so
does *Settings → System*). Without that file, as with the package, everything
is on.

*Settings → System → Features* changes the selection too: pick a profile,
switch single parts on or off, **Apply** runs install.sh of the installed
version with that choice.

**Updating vela itself:** *Settings → System → Vela* shows whether a newer
release is out (checked every 12 hours, `[self_update]` in config.toml) and a
desktop notification says so once per version. **Update** downloads the
release and runs its install.sh with `-y`, so your selection and settings
stay; the daemon restarts at the end. This works for installs made by
install.sh; on NixOS it builds the release source with nix, with Home Manager
**Update & rebuild** runs `nix flake update vela` and your rebuild command in
a terminal.

</details>

<details>
<summary><b>What gets installed where</b></summary>

| File | Location |
| --- | --- |
| `vela`, `vela-daemon`, `vela-share-picker`, `vela-pulse` | `~/.local/bin/` |
| systemd user unit | `~/.config/systemd/user/vela.service` |
| Hyprland module | `~/.config/hypr/vela.lua` |
| control center and Pulse (QML) | `~/.local/share/vela/shell` |
| component selection | `~/.local/share/vela/components.toml` |
| desktop entries, icons | `~/.local/share/applications`, `~/.local/share/icons` |
| Quick Share service menu (Dolphin) | `~/.local/share/kio/servicemenus/vela-quickshare.desktop` |
| MCP server | registered with Claude Code, if installed |

Claude Code must be installed for the Claude features (`claude` in `PATH`, or
set its path under *Settings → Claude*).

**As a package**: `pkg/arch/PKGBUILD` builds a system package (binaries in
`/usr/bin`, the module in `/usr/share/vela/hyprland/vela.lua`):
`cd pkg/arch && makepkg -si`, then load
`dofile("/usr/share/vela/hyprland/vela.lua").setup()`.

**Uninstall**: `./uninstall.sh` (keeps `~/.config/vela`) or
`./uninstall.sh --purge`. It removes the `dofile` line too, after a backup.

</details>

<details>
<summary><b>NixOS (Home Manager)</b></summary>
<br>

The flake has a package and a Home Manager module. It needs
`wayland.windowManager.hyprland.configType = "lua"`.

```nix
# flake.nix
inputs.vela = {
  url = "github:Raindancer118/vela";
  inputs.nixpkgs.follows = "nixpkgs";
};

# home.nix
imports = [ inputs.vela.homeManagerModules.default ];
programs.vela = {
  enable = true;
  panelPeek = "SUPER + B";
  # Optional: parts to leave out or add (`vela components` lists them).
  components = { claude = false; share_picker = true; };
  # Optional: keep vela's settings in your NixOS repository.
  nixos.stateDir = "/home/me/nixos/home/me/vela";
  nixos.rebuildCommand = "sudo nixos-rebuild switch --flake /home/me/nixos#host";
};
```

The module installs vela, its user service and the fonts and icons of the
control center, and adds the `setup()` call at the end of your hyprland.lua.
`components` replaces install.sh's profiles: everything is on except
`share_picker` (writes `~/.config/hypr/xdph.conf`) and `updates` (pacman and
Flatpak; NixOS updates through your flake).

Without Home Manager, `./install.sh` from a git checkout works on NixOS too:
it builds the flake's package with `nix build` (GC root
`~/.local/share/vela/nix-package`) instead of cargo, leaves `updates` out of
every profile (`--with updates` adds it) and doesn't touch a hyprland.lua that
Home Manager manages.

With `nixos.stateDir` set, vela recognises NixOS and keeps `config.toml`,
`hyprland.toml` and `hyprland.lua` in that directory instead of
`~/.config/vela` and `~/.local/state/vela` (settings you already had there
are copied over once). Changes still apply the moment
you make them; while git reports uncommitted changes there, *Settings* shows
**Apply & rebuild**, which runs `nixos.rebuildCommand` in your terminal
(*Settings → System → Terminal*), in that directory. Without it, vela works as
everywhere else and says so in *Settings*. The pointer to the directory is
`~/.config/vela/nixos.toml`.

*Settings → System → Features* then works on NixOS too: Apply saves the choice
as `components.toml` in that directory (`components` above is only the default
until then), stops or starts the control center and idle handling, reloads
Hyprland and restarts vela; no rebuild needed. vela then keeps its picker in
`~/.config/hypr/xdph.conf` itself instead of Home Manager.

Register the MCP server yourself:
`claude mcp add --scope user vela -- vela mcp`.

</details>

<details>
<summary><b>What <code>setup()</code> does, and its options</b></summary>

1. **Super tap.** It listens to `input.keyboard.key`, which Hyprland emits for
   every key before binds run. Super arms the launcher; any other key while
   Super is down disarms it, as does holding it longer than `tap_ms`. Releasing
   an armed Super runs `vela toggle`.
2. **Layer rules**: blur behind the launcher, its backdrop and the control
   center; Hyprland's own layer animation off (vela animates itself).
3. **Autostart** of the daemon (`vela.service`, or the daemon directly).
4. **Control center**: `vela shell` (`shell = false` to skip). Don't also start
   `qs` yourself: two notification daemons would fight.
5. **Idle**: `vela idle` runs hypridle with the *Power & idle* settings
   (`idle = false` to skip). Don't also start hypridle yourself.
6. **Hyprland settings** made in vela (`settings = false` to skip). They
   override hyprland.lua, so call `setup()` at its **end**.

```lua
dofile(os.getenv("HOME") .. "/.config/hypr/vela.lua").setup({
    tap_ms    = 400,          -- max. duration of a tap
    keycodes  = { 133, 134 }, -- Super_L, Super_R (XKB keycodes)
    blur      = true,
    autostart = true,
    shell     = true,         -- start the control center
    idle      = true,         -- run hypridle with vela's settings
    settings  = true,         -- apply the Hyprland settings made in vela
    panel_peek = nil,         -- e.g. "SUPER + T": tap toggles the control center,
                              -- holding shows it until you let go (peek_ms = 280)
    binary    = nil,          -- path to `vela`, found automatically
    settings_file = nil,      -- Lua with vela's Hyprland settings
                              -- (default ~/.local/state/vela/hyprland.lua)
})
```

Still on `hyprland.conf`? A release bind (`bindr = SUPER, SUPER_L, exec, vela toggle`)
plus a blur `layerrule` for the namespace `vela` comes closest, but can't
tell combinations without a bind apart and isn't tested. The Hyprland settings
need the Lua config.

</details>

<br>

<a id="under-the-hood"></a>
<img alt="Under the hood" src="docs/readme/h-inside-light.svg#gh-light-mode-only" width="100%">
<img alt="Under the hood" src="docs/readme/h-inside-dark.svg#gh-dark-mode-only" width="100%">

```mermaid
flowchart LR
    tap(["Super tap"]) --> lua["vela.lua<br/><sub>in Hyprland</sub>"]
    lua -- "vela toggle" --> cli["vela<br/><sub>tiny client, no GTK</sub>"]
    cli -- "unix socket" --> daemon["vela-daemon<br/><sub>GTK 4 · libadwaita</sub>"]
    daemon --> launcher["Launcher"]
    daemon --> settings["Settings"]
    settings -- "hyprctl eval" --> hypr[("Hyprland")]
    settings --> toml["hyprland.toml"]
    toml --> gen["generated Lua"] --> lua
    claude["Claude Code"] -- "MCP · stdio" --> mcp["vela mcp"]
    mcp --> toml
    mcp -- "hyprctl eval" --> hypr
    daemon -. "vela shell-config --watch" .-> qs["Control center<br/><sub>Quickshell</sub>"]
```

<details>
<summary><b>How it works, in detail</b></summary>

- **`vela`** is a small client without GTK. It sends one line (`toggle`,
  `show`, …) to `$XDG_RUNTIME_DIR/vela.sock` and answers in about a millisecond.
- **`vela-daemon`** runs the GTK application: the launcher is a layer-shell
  overlay on the focused (or main) monitor, the settings a libadwaita window
  in the same process, which is why changes apply instantly.
- **Applications** come from the XDG `applications` directories, following the
  Desktop Entry Specification (`NoDisplay`, `Hidden`, `OnlyShowIn`, `TryExec`,
  `Terminal`, localized keys). `Exec` is split into arguments by the spec and
  started directly, never through a shell, in its own systemd scope.
- **Search** runs on worker threads: nucleo fuzzy matching weighted towards
  exact and prefix matches plus a launch history; file search debounced, a
  newer query cancels the running one.
- **File index**: per search root vela checks whether the plocate database
  covers it, else it builds its own in-memory index (a parallel walk on the
  same file system, ~50 ms for 100 000 files).
- **Hyprland settings**: the option catalogue comes from Hyprland itself
  (`j/descriptions`, `j/getoption`), so new Hyprland options show up without a
  vela update. Changes go out with `hyprctl eval` and into
  `~/.config/vela/hyprland.toml`; the Lua generated from it
  (`~/.local/state/vela/hyprland.lua`) wraps every entry in `pcall`, so one
  option an older Hyprland doesn't know can't break your config.
- **Shortcut recording** briefly enters an empty Hyprland submap, so the keys
  you press reach vela instead of their bind.
- **Settings search** borrows the real rows from their pages and puts them
  back afterwards: what you find is the setting itself, not a copy.
- **Control center**: `vela shell` runs Quickshell on the QML in `shell/`. It
  reads the settings through `vela shell-config --watch` (one JSON line per
  change). The QML started as the Quickshell config of
  [Luna1506/nixos](https://github.com/Luna1506/nixos).
- **Screen sharing**: `install.sh` sets `custom_picker_binary =
  …/vela-share-picker` in `~/.config/hypr/xdph.conf` (if no other picker is
  set) and restarts the portal. It runs `shell/share-picker.qml` as its own
  Quickshell instance and answers xdph on stdout; regions are drawn with slurp.
- **Pulse**: `vela pulse` runs `shell/pulse.qml` as its own Quickshell instance
  (a second call brings it to the front). Its backend `vela pulse serve` reads
  /proc and sysfs once per interval and writes one JSON frame per line; the
  window sends commands back on its stdin. Processes become apps by their
  systemd cgroup (`app-….scope`, services), the rest by process tree below
  Hyprland; windows come from Hyprland, "not responding" from the dialogs
  Hyprland opens. Pressure (PSI), throttle counters, core dumps, failed units
  and mapped libraries that were replaced on disk feed the diagnosis. GPU load
  per process comes from DRM fdinfo; nvidia-smi only runs while the GPU is
  awake. The daemon's light background record lives in
  `$XDG_RUNTIME_DIR/vela-pulse-backlog.json`.

</details>

<details>
<summary><b>Configuration reference</b> (<code>~/.config/vela/config.toml</code>)</summary>

Use the settings window: `vela settings`, the gear in the launcher, or
<kbd>Ctrl</kbd>+<kbd>,</kbd>. You can also edit the file by hand; vela notices and
reloads it, reports errors in the settings and keeps the last valid settings
(an unparsable file is backed up as `config.toml.broken-<time>`).
`data/config.example.toml` lists every option with its default.

| Section | Key | Meaning |
| --- | --- | --- |
| `general` | `width`, `max_height` | size of the launcher (logical px); content scrolls beyond `max_height` |
| | `vertical_position` | distance from the top of the monitor in % |
| | `horizontal_position` (`center`, `left`, `right`), `side_margin` | launcher centred or at an edge, `side_margin` px in |
| | `clock`, `clock_size`, `clock_font` | launcher clock on the blurred backdrop (needs `appearance.backdrop` + `backdrop_blur`): above a centred launcher, on the free side otherwise |
| | `opacity` | background opacity 0–1 |
| | `close_on_focus_loss` | hide on a click outside the panel |
| | `main_monitor` | always open here while connected: `"desc:<make model serial>"` or a connector like `"DP-7"`; empty = focused monitor |
| | `max_results` | rows in the result list |
| | `systemd_scope` | start apps in their own transient scope |
| `appearance` | `theme` | `dark`, `midnight`, `graphite`, `nord`, `light` |
| | `accent` | `#rrggbb` |
| | `tile_size`, `icon_size`, `spacing`, `border_radius`, `surface_opacity`, `font_scale` | |
| | `columns` | fixed column count, `0` = derived from width |
| | `show_labels`, `tile_background`, `tile_outline` | the app grid |
| | `animations`, `animation_speed` | motion on/off; speed factor |
| | `backdrop`, `backdrop_dim`, `backdrop_blur` | blur and darken the rest of the monitor while the launcher is open (`backdrop_blur = false`: only darken) |
| `apps` | `grid` | `pinned`, `pinned_then_all`, `all` |
| | `pinned`, `hidden` | desktop IDs (`firefox.desktop`), `custom:<id>` or `vela:settings` |
| | `desktop_actions` | offer desktop actions in search |
| | `[[apps.custom]]` | `id`, `name`, `command` (argv array), `icon`, `terminal`, `keywords` |
| `search` | `apps`, `files`, `claude` | result sources |
| | `max_app_results`, `max_file_results`, `min_file_query_len`, `debounce_ms`, `index_interval_minutes` | |
| | `file_backend` | `auto`, `plocate`, `builtin` |
| | `file_roots`, `exclude`, `include_hidden`, `include_directories` | what the file search covers |
| `claude` | `executable`, `args`, `working_dir` | runs `<terminal> <exec args> <executable> <args> -- "<prompt>"` |
| | `always_visible`, `shift_enter`, `prefer_for_questions` | when "Ask Claude" shows and wins |
| `panel` | `width`, `close_on_focus_loss`, `backdrop`, `backdrop_blur` | the control center |
| | `popup_timeout_secs`, `popup_max_visible`, `critical_popups_stay`, `group_collapsed_count`, `compact_notifications` | notifications |
| | `workspace_osd`, `night_light_temperature`, `clock_centered` | |
| | `clock_on_backdrop`, `backdrop_clock_size`, `clock_font` | clock large on the blurred backdrop (needs `backdrop` + `backdrop_blur`), its size in px (48–320); clock font, empty = UI font |
| | `claude_usage`, `claude_usage_subtle`, `claude_usage_only_default`, `claude_usage_hidden` | Claude plan usage at the bottom of the panel |
| | `media_player`, `media_player_any`, `media_player_position` (`tiles`, `top`, `bottom`; the last two need the clock on the blur), `media_player_cover`, `media_player_cover_background`, `media_player_cover_blur` (0–1), `media_player_progress`, `media_player_shuffle_repeat`, `media_player_scale` (0.75–1.75), `media_player_button_scale` (0.6–1.25), `media_player_button_background` | mini player (Spotify, or any MPRIS player) |
| `idle` | `dim`, `lock`, `screen_off`, `suspend` (+ `*_after_min`), `lock_before_sleep` | what `vela idle` runs hypridle with |
| `updates` | `check_interval_hours` (0 = never), `aur`, `flatpak` | background checks and sources |
| | `claude_workspace` | where "Fix with Claude" opens, silently; empty = a normal window |
| | `panel.updates_tile` | the tile in the control center |
| `terminal` | `executable`, `exec_args` | default `kitty`; arguments before the command |

From scripts: `vela set idle.suspend false` (checked against type and range).
The Hyprland side lives in `~/.config/vela/hyprland.toml` (`[options]`,
`[animations.<leaf>]`, `[monitors."desc:…"]`, `[[shortcuts]]`, `unbind`,
`[[rules]]`, `[[autostart]]`), also editable by hand.

</details>

<details>
<summary><b>Command line</b></summary>

```
vela                      toggle the launcher (starts the daemon if needed)
vela show | hide
vela settings [page]      home, general, apps, search, claude, panel, notifications, power,
                          hypr-windows, hypr-effects, hypr-animations, hypr-input, hypr-monitors,
                          hypr-shortcuts, hypr-rules, hypr-autostart, hypr-layouts,
                          hypr-behaviour, hypr-all, appearance, updates, system
vela set <section.key> <value>
vela reload               re-read config, apps and file index
vela shell | panel [toggle|open|close] | idle
vela updates [list|check|watch]   pending updates as JSON | let the daemon look now | state for the panel
vela mcp                  MCP server for Claude Code (stdio)
vela quit | status | default-config
```

</details>

<details>
<summary><b>Troubleshooting</b></summary>

**Tapping Super does nothing.** Run `vela` in a terminal: if the launcher
appears, the Hyprland side is the problem. Check `hyprctl configerrors`, and
that the `dofile` line comes last. If your keyboard sends other keycodes for
Super, find them with `wev` and set `keycodes`.

**A Hyprland setting doesn't stick after a restart.** `setup()` must be the
last line of hyprland.lua (later lines would override vela), and
`settings = false` must not be set.

**No blur.** Blur needs `decoration.blur.enabled` (Settings → Blur & effects)
and the layer rules from `vela.lua`.

**Files in my home folder are not found with plocate.** On btrfs, `/home` is a
subvolume that updatedb skips. `file_backend = "auto"` notices and uses the
built-in index; or set `PRUNE_BIND_MOUNTS = "no"` in `/etc/updatedb.conf` and
run `sudo updatedb`.

**"Claude executable … was not found".** vela adds `~/.local/bin`,
`~/.cargo/bin` and `~/bin` to PATH; elsewhere, set the full path under
*Settings → Claude*.

**Wrong terminal.** *Settings → General → Terminal*. For unknown terminals,
turn off "Default arguments" and enter the flag that runs a command (usually
`-e`).

**Testing in a nested Hyprland.** Pass `autostart = false` to `setup()` there,
or D-Bus activated apps of your real session open in the nested one.

**Logs.** `journalctl --user -u vela -f`, or run `vela-daemon` in a terminal
(`RUST_LOG=vela=debug`).

</details>

<details>
<summary><b>Development</b></summary>

```sh
cargo test                                  # unit and integration tests
cargo test --lib live_ -- --ignored         # against the running Hyprland (no-op changes)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
scripts/smoke-test.sh                       # start-up test on a Wayland display
scripts/shell-test.sh                       # control center (needs qs)
```

The README artwork is generated: `scripts/readme-art.py` (hero and headings,
text set with HarfBuzz and stored as outlines), `scripts/readme-shots.sh`
(screenshots from a throwaway vela that never takes the focus) and
`scripts/readme-frame.py` (frames, gallery, the search demo).

Releases: add the version to `docs/changelog.toml` (CI fails while a tag has
no entry), then `git tag X.Y.Z && git push origin X.Y.Z`; GitHub Actions builds
and publishes them with that entry as release notes, and rebuilds the
[changelog page](https://raindancer118.github.io/vela/) (`scripts/changelog.py`).

</details>

<br>

<p align="center">
  <img alt="" src="docs/readme/vela-mark.svg" width="44"><br>
  <sub><i>vela</i> — Latin for <i>sails</i>, and the constellation of the sails of the ship Argo.<br>
  MIT licensed · made for Hyprland</sub>
</p>
