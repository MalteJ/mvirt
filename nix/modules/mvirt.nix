# NixOS module for mvirt services
{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.mvirt;

  # Get mvirt packages from the flake or use provided packages
  mvirtPkgs = cfg.package;

in {
  options.services.mvirt = {
    enable = mkEnableOption "mvirt virtual machine manager";

    package = mkOption {
      type = types.package;
      description = "The mvirt package to use";
    };

    cloudHypervisor = mkOption {
      type = types.package;
      description = "The cloud-hypervisor package to use";
    };

    firmware = mkOption {
      type = types.package;
      description = "The UEFI firmware package (EDK2 CLOUDHV.fd) for VM boot";
    };

    dataDir = mkOption {
      type = types.path;
      default = "/var/lib/mvirt";
      description = "Directory for mvirt data (database, images, etc.)";
    };

    # Individual service enables
    vmm = {
      enable = mkEnableOption "mvirt-vmm (VM Manager daemon)" // { default = true; };

      port = mkOption {
        type = types.port;
        default = 50051;
        description = "gRPC port for mvirt-vmm";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-vmm";
      };
    };

    log = {
      enable = mkEnableOption "mvirt-log (Logging service)" // { default = true; };

      port = mkOption {
        type = types.port;
        default = 50052;
        description = "gRPC port for mvirt-log";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-log";
      };
    };

    zfs = {
      enable = mkEnableOption "mvirt-zfs (ZFS storage management)";

      port = mkOption {
        type = types.port;
        default = 50053;
        description = "gRPC port for mvirt-zfs";
      };

      pool = mkOption {
        type = types.str;
        default = "mvirt";
        description = "ZFS pool to use for VM storage";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-zfs";
      };
    };

    net = {
      enable = mkEnableOption "mvirt-net (Network management)" // { default = true; };

      port = mkOption {
        type = types.port;
        default = 50054;
        description = "gRPC port for mvirt-net";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-net";
      };
    };
  };

  config = mkIf cfg.enable {
    # Create mvirt user and group
    users.users.mvirt = {
      isSystemUser = true;
      group = "mvirt";
      home = cfg.dataDir;
      description = "mvirt service user";
    };

    users.groups.mvirt = {};

    # Ensure data directory exists
    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0755 root root -"
      "d ${cfg.dataDir}/vmm 0755 root root -"
      "d ${cfg.dataDir}/log 0750 mvirt mvirt -"
      "d ${cfg.dataDir}/net 0755 root root -"
      "d /run/mvirt 0755 root root -"
      "d /run/mvirt/net 0755 root root -"
    ];

    # mvirt-log service (starts first, others depend on it)
    systemd.services.mvirt-log = mkIf cfg.log.enable {
      description = "mvirt Logging Service";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      serviceConfig = {
        Type = "simple";
        User = "mvirt";
        Group = "mvirt";
        ExecStart = "${mvirtPkgs}/bin/mvirt-log --listen [::1]:${toString cfg.log.port} --data-dir ${cfg.dataDir}/log ${concatStringsSep " " cfg.log.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [ cfg.dataDir ];
        PrivateTmp = true;
      };
    };

    # mvirt-vmm service
    systemd.services.mvirt-vmm = mkIf cfg.vmm.enable {
      description = "mvirt Virtual Machine Manager";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" ];
      wants = [ "mvirt-log.service" ];

      path = [ cfg.cloudHypervisor ];

      environment = {
        MVIRT_DATA_DIR = cfg.dataDir;
        MVIRT_LOG_ENDPOINT = "http://[::1]:${toString cfg.log.port}";
        HYPERVISOR_FW = "${cfg.firmware}/share/firmware/CLOUDHV.fd";
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for KVM/TAP access
        ExecStart = "${mvirtPkgs}/bin/mvirt-vmm --listen [::1]:${toString cfg.vmm.port} --data-dir ${cfg.dataDir}/vmm ${concatStringsSep " " cfg.vmm.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening (limited due to root requirement)
        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # mvirt-zfs service
    systemd.services.mvirt-zfs = mkIf cfg.zfs.enable {
      description = "mvirt ZFS Storage Manager";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" "zfs.target" ];
      wants = [ "mvirt-log.service" ];
      requires = [ "zfs.target" ];

      environment = {
        MVIRT_LOG_ENDPOINT = "http://[::1]:${toString cfg.log.port}";
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for ZFS operations
        ExecStart = "${mvirtPkgs}/bin/mvirt-zfs --listen [::1]:${toString cfg.zfs.port} --pool ${cfg.zfs.pool} --log-endpoint http://[::1]:${toString cfg.log.port} ${concatStringsSep " " cfg.zfs.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening
        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # mvirt-net service
    systemd.services.mvirt-net = mkIf cfg.net.enable {
      description = "mvirt Network Manager";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" ];
      wants = [ "mvirt-log.service" ];

      environment = {
        MVIRT_DATA_DIR = cfg.dataDir;
        MVIRT_LOG_ENDPOINT = "http://[::1]:${toString cfg.log.port}";
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for TUN device
        ExecStart = "${mvirtPkgs}/bin/mvirt-net ${concatStringsSep " " cfg.net.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening (limited due to root requirement for TUN)
        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # Add cloud-hypervisor to system path
    environment.systemPackages = [
      mvirtPkgs
      cfg.cloudHypervisor
    ];
  };
}
