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

    api = {
      enable = mkEnableOption "mvirt-api (REST API control plane)";

      port = mkOption {
        type = types.port;
        default = 8080;
        description = "REST API port";
      };

      dev = mkOption {
        type = types.bool;
        default = false;
        description = "Run in development mode (single-node, ephemeral storage)";
      };

      grpcListen = mkOption {
        type = types.str;
        default = "[::1]:50056";
        description = "gRPC listen address for node agents";
      };

      nodeId = mkOption {
        type = types.int;
        default = 1;
        description = "Raft node ID";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-api";
      };
    };

    node = {
      enable = mkEnableOption "mvirt-node (Node agent for reconciliation)";

      apiEndpoint = mkOption {
        type = types.str;
        default = "http://[::1]:50056";
        description = "mvirt-api gRPC endpoint for node registration and spec streaming";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-node";
      };
    };

    shipper = {
      enable = mkEnableOption "mvirt-shipper (Journald log shipper)" // { default = true; };

      units = mkOption {
        type = types.listOf types.str;
        default = [ "mvirt-vmm" "mvirt-zfs" "mvirt-ebpf" "mvirt-node" ];
        description = "Systemd units to ship logs from";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-shipper";
      };
    };

    ebpf = {
      enable = mkEnableOption "mvirt-ebpf (eBPF network management)" // { default = true; };

      port = mkOption {
        type = types.port;
        default = 50054;
        description = "gRPC port for mvirt-ebpf";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-ebpf";
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
      "d ${cfg.dataDir}/cp 0755 root root -"
      "d ${cfg.dataDir}/ebpf 0755 root root -"
      "d ${cfg.dataDir}/shipper 0750 mvirt mvirt -"
      "d /run/mvirt 0755 root root -"
      "d /run/mvirt/ebpf 0755 root root -"
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

    # mvirt-shipper service
    systemd.services.mvirt-shipper = mkIf cfg.shipper.enable {
      description = "mvirt Journald Log Shipper";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" ];
      wants = [ "mvirt-log.service" ];

      serviceConfig = {
        Type = "simple";
        User = "mvirt";
        Group = "mvirt";
        ExecStart = "${mvirtPkgs}/bin/mvirt-shipper --units ${concatStringsSep "," cfg.shipper.units} --log-endpoint http://[::1]:${toString cfg.log.port} --cursor-dir ${cfg.dataDir}/shipper ${concatStringsSep " " cfg.shipper.extraArgs}";
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

      path = [ config.boot.zfs.package pkgs.qemu-utils ];

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

    # mvirt-ebpf service
    systemd.services.mvirt-ebpf = mkIf cfg.ebpf.enable {
      description = "mvirt eBPF Network Manager";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" ];
      wants = [ "mvirt-log.service" ];

      path = [ pkgs.nftables pkgs.iproute2 ];

      environment = {
        MVIRT_DATA_DIR = cfg.dataDir;
        MVIRT_LOG_ENDPOINT = "http://[::1]:${toString cfg.log.port}";
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for eBPF and TUN device
        ExecStart = "${mvirtPkgs}/bin/mvirt-ebpf ${concatStringsSep " " cfg.ebpf.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening (limited due to root requirement for eBPF)
        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # mvirt-node agent
    systemd.services.mvirt-node = mkIf cfg.node.enable {
      description = "mvirt Node Agent";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" "mvirt-vmm.service" "mvirt-zfs.service" "mvirt-ebpf.service" ];
      wants = [ "mvirt-log.service" "mvirt-vmm.service" "mvirt-zfs.service" "mvirt-ebpf.service" ];

      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = "${mvirtPkgs}/bin/mvirt-node --api-endpoint ${cfg.node.apiEndpoint} --vmm-endpoint http://[::1]:${toString cfg.vmm.port} --zfs-endpoint http://[::1]:${toString cfg.zfs.port} --net-endpoint http://[::1]:${toString cfg.ebpf.port} --log-endpoint http://[::1]:${toString cfg.log.port} ${concatStringsSep " " cfg.node.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # mvirt-api service
    systemd.services.mvirt-api = mkIf cfg.api.enable {
      description = "mvirt API Server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-log.service" "mvirt-vmm.service" "mvirt-zfs.service" "mvirt-ebpf.service" ];
      wants = [ "mvirt-log.service" "mvirt-vmm.service" "mvirt-zfs.service" "mvirt-ebpf.service" ];

      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = let
          devFlag = if cfg.api.dev then " --dev" else "";
          nodeIdFlag = " --node-id ${toString cfg.api.nodeId}";
        in "${mvirtPkgs}/bin/mvirt-api --listen [::]:${toString cfg.api.port} --grpc-listen ${cfg.api.grpcListen} --data-dir ${cfg.dataDir}/cp --log-endpoint http://[::1]:${toString cfg.log.port}${devFlag}${nodeIdFlag} ${concatStringsSep " " cfg.api.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

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
