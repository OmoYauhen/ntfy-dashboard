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
      description = ''
        Address to listen on. There is no login, so keep it local and put a reverse proxy in front
        that only your private network (e.g. Tailscale) can reach.
      '';
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8440;
      description = "Port to listen on.";
    };

    credentialsFile = lib.mkOption {
      type = lib.types.path;
      example = lib.literalExpression "./secrets/ntfy-dashboard.cred";
      description = ''
        NTFY login as `email:password`, handed to the service as the systemd credential
        `ntfy-credentials`. By default this is a `systemd-creds encrypt` blob (see
        `encryptedCredentials`), which is safe to keep in git and in the Nix store. Pipe
        `email:password` into `sudo systemd-creds encrypt --with-key=tpm2
        --name=ntfy-credentials - ntfy-dashboard.cred` (read the password with `read -s`, so it
        doesn't end up in shell history). With `--with-key=tpm2`, only this machine can decrypt it.
      '';
    };

    encryptedCredentials = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Whether `credentialsFile` is encrypted with `systemd-creds` (LoadCredentialEncrypted=).
        Set to false for a plain file outside the Nix store, e.g. from agenix or sops-nix.
      '';
    };

    readOnly = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Show the plan but never send changes to NTFY.";
    };
  };

  config = lib.mkIf cfg.enable {
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

      environment = {
        NTFY_BIND = "${cfg.listenAddress}:${toString cfg.port}";
        NTFY_DATA_DIR = stateDir;
        NTFY_IMAGE_CACHE = "${stateDir}/cache/images";
      } // lib.optionalAttrs cfg.readOnly { NTFY_READONLY = "1"; };

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        # systemd decrypts the login into a private, memory-only directory ($CREDENTIALS_DIRECTORY)
        # that only this service can read.
        ${if cfg.encryptedCredentials then "LoadCredentialEncrypted" else "LoadCredential"} =
          "ntfy-credentials:${cfg.credentialsFile}";
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
