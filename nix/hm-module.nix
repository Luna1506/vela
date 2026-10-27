self:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.vela;
  inherit (lib) mkOption mkEnableOption mkIf types;
  setupArgs = {
    binary = lib.getExe cfg.package;
  }
  // lib.optionalAttrs (cfg.nixos.stateDir != null) {
    settings_file = "${cfg.nixos.stateDir}/hyprland.lua";
    components_file = "${cfg.nixos.stateDir}/components.toml";
  }
  // lib.optionalAttrs (cfg.panelPeek != null) { panel_peek = cfg.panelPeek; };
  # Keys of components.toml (data/components.txt, dashes as underscores).
  # updates runs pacman, so it is off by default here.
  defaultComponents = {
    launcher = true;
    claude = true;
    hyprland = true;
    panel = true;
    idle = true;
    share_picker = false;
    updates = false;
    pulse = true;
    quickshare = true;
  };
  components = defaultComponents // cfg.components;
in
{
  options.programs.vela = {
    enable = mkEnableOption "vela, launcher and control center for Hyprland";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "vela.packages.\${system}.default";
      description = "The vela package.";
    };

    panelPeek = mkOption {
      type = types.nullOr types.str;
      default = null;
      example = "SUPER + B";
      description = ''
        Shortcut for the control center: a tap toggles it, holding it peeks.
        One set in Settings → Shortcuts → Control center takes precedence.
      '';
    };

    components = mkOption {
      type = types.attrsOf types.bool;
      default = { };
      example = {
        claude = false;
        share_picker = true;
      };
      description = ''
        Parts of vela to use (see `vela components`): launcher, claude,
        hyprland, panel, idle, share_picker (sets vela's picker in
        ~/.config/hypr/xdph.conf), updates (pacman/Flatpak, off by default) and
        pulse (the task manager, Ctrl+Shift+Esc) and quickshare (send and
        receive files with Android and Samsung phones nearby; needs panel).
        Unset ones keep their default; all but share_picker and updates are on.
        Quick Share needs open ports, which the module does not touch; on NixOS:
        `networking.firewall.allowedTCPPorts = [ 47392 ];` and
        `networking.firewall.allowedUDPPorts = [ 5353 ];`.
        With nixos.stateDir set this is only the default: Settings → System →
        Features saves its choice as components.toml there, and vela keeps
        xdph.conf itself.
      '';
    };

    nixos = {
      stateDir = mkOption {
        type = types.nullOr (types.strMatching "/.*");
        default = null;
        example = "/home/me/nixos/home/me/vela";
        description = ''
          Absolute path of a directory in your NixOS repository. vela then keeps
          config.toml, its Hyprland settings and the choice of components
          (Settings → System → Features) there instead of ~/.config/vela and
          ~/.local/state/vela, and Settings offers "Apply & rebuild" while
          git reports uncommitted changes in it. A string, not a Nix path:
          vela writes to it.
        '';
      };

      rebuildCommand = mkOption {
        type = types.str;
        default = "sudo nixos-rebuild switch --flake .";
        example = "rebuild";
        description = ''
          Run in a terminal, in stateDir, by "Apply & rebuild". Split into
          words like a shell would, but not run through one.
        '';
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = config.wayland.windowManager.hyprland.configType == "lua";
        message = ''programs.vela needs wayland.windowManager.hyprland.configType = "lua" (Hyprland 0.55+).'';
      }
      {
        assertion = lib.all (k: lib.hasAttr k defaultComponents) (lib.attrNames cfg.components);
        message = "programs.vela.components: unknown ${lib.concatStringsSep ", " (lib.subtractLists (lib.attrNames defaultComponents) (lib.attrNames cfg.components))} (known: ${lib.concatStringsSep ", " (lib.attrNames defaultComponents)})";
      }
    ];

    warnings = lib.optional (components.quickshare && !components.panel)
      "programs.vela.components: quickshare needs the panel component (the control center); it does nothing without it";

    # The fonts below are only found with fontconfig on.
    fonts.fontconfig.enable = lib.mkDefault true;

    home.packages = [
      cfg.package
    ]
    ++ (with pkgs; [
      # Control center font and icons.
      material-symbols
      rubik
      papirus-icon-theme
    ]);

    # Same unit as contrib/systemd/vela.service.in; vela.lua (re)starts it.
    systemd.user.services.vela = {
      Unit = {
        Description = "vela application launcher";
        Documentation = "https://github.com/Raindancer118/vela";
        PartOf = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };
      Service = {
        Type = "simple";
        ExecStart = "${cfg.package}/bin/vela-daemon";
        Restart = "on-abnormal";
        RestartSec = 1;
        KillMode = "process";
        Environment = [ "RUST_LOG=vela=info" ];
      };
    };

    # Same format install.sh writes; vela and vela.lua read it.
    xdg.dataFile."vela/components.toml".text = ''
      profile = "custom"
    ''
    + lib.concatStrings (lib.mapAttrsToList (k: v: "${k} = ${lib.boolToString v}\n") components);

    # With a state directory the selection can change without a rebuild;
    # the daemon keeps the picker in xdph.conf then.
    xdg.configFile."hypr/xdph.conf" = mkIf (components.share_picker && cfg.nixos.stateDir == null) {
      text = lib.mkDefault ''
        screencopy {
            custom_picker_binary = ${cfg.package}/bin/vela-share-picker
        }
      '';
    };

    xdg.configFile."vela/nixos.toml" = mkIf (cfg.nixos.stateDir != null) {
      text = ''
        state_dir = ${builtins.toJSON cfg.nixos.stateDir}
        rebuild = ${builtins.toJSON cfg.nixos.rebuildCommand}
      '';
    };

    # Appended at the end of hyprland.lua, so vela's settings win.
    wayland.windowManager.hyprland.extraConfig = ''
      -- vela: Super tap, control center, idle and vela's Hyprland settings
      dofile("${cfg.package}/share/vela/vela.lua").setup(${lib.generators.toLua { } setupArgs})
    '';
  };
}
