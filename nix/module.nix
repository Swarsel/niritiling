{ config, lib, ... }:

let
  cfg = config.services.niritiling;
in
{
  options.services.niritiling = {
    enable = lib.mkEnableOption "automatic window tiling for the first window in Niri";
    columnTargets = lib.mkOption {
      default = { };
      description = "Per-output column targets (\"make model serial\"): columns are sized to fit up to N side by side. If 1, stacks newly opened windows into the one column (\"vertical stacking\")";
      example."LG Electronics LG ULTRAFINE 510NTEPPU036" = 3;
      type = lib.types.attrsOf lib.types.ints.positive;
    };
    defaultColumns = lib.mkOption {
      default = 2;
      description = "Column target applied when a workspace moves to an output that has no entry in columnTargets.";
      type = lib.types.ints.positive;
    };
    resizeColumns = lib.mkOption {
      default = false;
      description = "When a column is resized, adjust the other column to compensate (only works with exactly two columns).";
      type = lib.types.bool;
    };
    systemdTarget = lib.mkOption {
      default = "graphical-session.target";
      description = "The systemd target to bind the niritiling service to.";
      type = lib.types.str;
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.niritiling = {
      after = [ cfg.systemdTarget ];
      description = "niritiling - first-window tiling service for Niri";
      partOf = [ cfg.systemdTarget ];
      serviceConfig = {
        CapabilityBoundingSet = "";
        ExecStart = "${cfg.package}/bin/niritiling${lib.optionalString cfg.resizeColumns " --resize-columns"}${
          lib.optionalString (cfg.defaultColumns != 2) " --default-columns ${toString cfg.defaultColumns}"
        }${
          lib.concatStrings (
            lib.mapAttrsToList (
              identity: n: " --columns ${lib.escapeShellArg "${identity}=${toString n}"}"
            ) cfg.columnTargets
          )
        }";
        IPAddressDeny = "any";
        KeyringMode = "private";
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        NoNewPrivileges = true;
        PrivateDevices = true;
        PrivateNetwork = true;
        PrivateTmp = true;
        PrivateUsers = true;
        ProcSubset = "pid";
        ProtectClock = true;
        ProtectControlGroups = true;
        ProtectHome = "read-only";
        ProtectHostname = true;
        ProtectKernelLogs = true;
        ProtectKernelModules = true;
        ProtectKernelTunables = true;
        ProtectProc = "invisible";
        ProtectSystem = "strict";
        Restart = "on-failure";
        RestartSec = 2;
        RestrictAddressFamilies = [ "AF_UNIX" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = "@system-service";
        UMask = "0077";
      };
      wantedBy = [ cfg.systemdTarget ];
    };
  };
}
