{
  lib,
  rustPlatform,
  pkg-config,
  wrapGAppsHook4,
  gtk4,
  libadwaita,
  gtk4-layer-shell,
  quickshell,
  hypridle,
  hyprsunset,
  brightnessctl,
  slurp,
  xdg-utils,
  curl,
  gnutar,
  gzip,
  lxqt,
}:
let
  cargo = (builtins.fromTOML (builtins.readFile ../Cargo.toml)).package;
in
rustPlatform.buildRustPackage {
  pname = "vela";
  inherit (cargo) version;
  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [
    pkg-config
    wrapGAppsHook4
  ];
  buildInputs = [
    gtk4
    libadwaita
    gtk4-layer-shell
  ];

  # The tests need a Wayland compositor and Hyprland; CI runs them on Arch.
  doCheck = false;

  postInstall = ''
    for icon in data/icons/*.svg; do
      install -Dm644 "$icon" "$out/share/icons/hicolor/scalable/apps/$(basename "$icon")"
    done
    sed "s|^Exec=vela|Exec=$out/bin/vela|" data/vela.desktop \
      | install -Dm644 /dev/stdin "$out/share/applications/vela.desktop"
    sed "s|^Exec=vela-pulse|Exec=$out/bin/vela-pulse|" data/vela-pulse.desktop \
      | install -Dm644 /dev/stdin "$out/share/applications/vela-pulse.desktop"
    sed "s|^Exec=vela|Exec=$out/bin/vela|" data/vela-quickshare.desktop \
      | install -Dm644 /dev/stdin "$out/share/applications/vela-quickshare.desktop"
    # Dolphin ignores service menus that are not executable.
    sed "s|^Exec=vela|Exec=$out/bin/vela|" data/vela-quickshare-servicemenu.desktop \
      | install -Dm755 /dev/stdin "$out/share/kio/servicemenus/vela-quickshare.desktop"
    install -Dm644 contrib/hyprland/vela.lua "$out/share/vela/vela.lua"
    (cd shell && find . -type f \( -name '*.qml' -o -name '*.js' -o -name '*.svg' \) ! -name 'test-*' \
      -exec install -Dm644 {} "$out/share/vela/shell/{}" \;)
  '';

  # vela shell / vela idle start these by name.
  preFixup = ''
    gappsWrapperArgs+=(--prefix PATH : ${
      lib.makeBinPath [
        quickshell
        hypridle
        hyprsunset
        brightnessctl
        slurp
        xdg-utils
        # Self-update (Settings → System) and the Claude usage.
        curl
        gnutar
        gzip
      ]
    })
    # A user service may start without the system's PATH: Pulse needs the
    # host's setuid sudo, systemctl, journalctl, nvidia-smi, powerprofilesctl.
    # vela adds the per-user profiles itself (paths::complete_nixos_env).
    # lxqt-openssh-askpass is the password dialog of last resort.
    gappsWrapperArgs+=(
      --suffix PATH : /run/wrappers/bin:/run/current-system/sw/bin:${lib.getBin lxqt.lxqt-openssh-askpass}/bin
      --suffix XDG_DATA_DIRS : /run/current-system/sw/share
    )
  '';

  meta = {
    description = "Launcher, control center and every Hyprland setting";
    homepage = "https://github.com/Raindancer118/vela";
    license = lib.licenses.mit;
    mainProgram = "vela";
    platforms = lib.platforms.linux;
  };
}
