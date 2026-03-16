rk-m87-sync:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.rk-m87-sync;
in
{
  options.services.rk-m87-sync = {
    enable = lib.mkEnableOption "RK M87 keyboard time/volume sync daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = rk-m87-sync;
      description = "The rk-m87-sync package to use.";
    };

    device = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      description = "Hidraw device path. Auto-detected if null.";
    };

    noPing = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Skip echo ping check (dongle mode only).";
    };
  };

  config = lib.mkIf cfg.enable {
    services.udev.packages = [
      (pkgs.writeTextFile {
        name = "rk-m87-udev-rules";
        destination = "/lib/udev/rules.d/70-rk-m87.rules";
        text = ''
          # RK M87 keyboard (USB cable)
          SUBSYSTEM=="hidraw", ATTRS{idVendor}=="258a", ATTRS{idProduct}=="01a2", MODE="0660", TAG+="uaccess"
          # RK M87 dongle
          SUBSYSTEM=="hidraw", ATTRS{idVendor}=="258a", ATTRS{idProduct}=="0150", MODE="0660", TAG+="uaccess"
        '';
      })
    ];

    systemd.user.services.rk-m87-sync = {
      description = "RK M87 keyboard time/volume sync";
      wantedBy = [ "default.target" ];
      after = [
        "pipewire.service"
        "pulseaudio.service"
      ];
      serviceConfig = {
        ExecStart = lib.concatStringsSep " " (
          [
            (lib.getExe cfg.package)
            "--daemon"
          ]
          ++ lib.optionals (cfg.device != null) [
            "--device"
            cfg.device
          ]
          ++ lib.optional cfg.noPing "--no-ping"
        );
        Restart = "on-failure";
        RestartSec = 5;
      };
    };
  };
}
