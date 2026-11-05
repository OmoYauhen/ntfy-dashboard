# NixOS module for ntfy-dashboard. Works with and without flakes:
#   imports = [ "${src}/nix/module.nix" ];   or   inputs.ntfy-dashboard.nixosModules.default
{ config, lib, pkgs, ... }:

let
  cfg = config.services.ntfy-dashboard;
  stateDir = "/var/lib/ntfy-dashboard";
in
{
  options.services.ntfy-dashboard = {
    enable = lib.mkEnableOption "ntfy-dashboard, a meal picker for Nice To Fit You";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The ntfy-dashboard package.";
    };

    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address to listen on. Keep it local and put a TLS reverse proxy in front.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8440;
      description = "Port to listen on.";
    };

    allowedUsers = lib.mkOption {
      type = lib.types.listOf lib.types.ints.unsigned;
      default = [ ];
      example = [ 4242 ];
      description = ''
        NTFY user ids allowed to log in (the `id` of the account, shown in the server log on
        login). Empty allows any NTFY account, which you only want on a private network.
      '';
    };

    readOnly = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Show the plan but never send changes to NTFY.";
    };
  };

  config = lib.mkIf cfg.enable {
    warnings = lib.optional (cfg.allowedUsers == [ ])
      "services.ntfy-dashboard.allowedUsers is empty: any NTFY account can log in.";

    users.users.ntfy-dashboard = {
      isSystemUser = true;
      group = "ntfy-dashboard";
      home = stateDir;
    };
    users.groups.ntfy-dashboard = { };

    systemd.services.ntfy-dashboard = {
      description = "ntfy-dashboard meal picker";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      # No credentials here: users log in on the web page, and the server keeps them in memory.
      environment = {
        NTFY_BIND = "${cfg.listenAddress}:${toString cfg.port}";
        NTFY_DATA_DIR = stateDir;
        NTFY_IMAGE_CACHE = "${stateDir}/cache/images";
        NTFY_ALLOWED_USERS = lib.concatMapStringsSep "," toString cfg.allowedUsers;
      } // lib.optionalAttrs cfg.readOnly { NTFY_READONLY = "1"; };

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        User = "ntfy-dashboard";
        Group = "ntfy-dashboard";
        # Holds the per-account archives (ntfy-<user id>.db) and the photo cache.
        StateDirectory = "ntfy-dashboard";
        StateDirectoryMode = "0750";
        WorkingDirectory = stateDir;
        Restart = "on-failure";
        RestartSec = 5;

        # It only needs outbound HTTPS, its listening socket and its state directory.
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectKernelModules = true;
        ProtectKernelLogs = true;
        ProtectControlGroups = true;
        ProtectClock = true;
        ProtectHostname = true;
        RestrictAddressFamilies = [ "AF_INET" "AF_INET6" ];
        RestrictNamespaces = true;
        RestrictRealtime = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
        MemoryDenyWriteExecute = true;
        SystemCallArchitectures = "native";
        SystemCallFilter = [ "@system-service" "~@privileged" ];
        CapabilityBoundingSet = "";
        UMask = "0027";
      };
    };
  };
}
