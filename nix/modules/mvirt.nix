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
      # mvirt-log runs alongside mvirt-cplane (it reads CA + server cert
      # material that cplane mints in raft state and publishes on disk).
      # Default tracks cplane.enable so node-only hosts don't try to start
      # it without the cert files.
      enable = mkOption {
        type = types.bool;
        default = cfg.cplane.enable;
        description = "Enable mvirt-log (centralized logging service). Defaults to cfg.cplane.enable.";
      };

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

    cplane = {
      enable = mkEnableOption "mvirt-cplane (Raft consensus, REST API, scheduler, reconciler, tunnel acceptor)";

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

      tunnelListen = mkOption {
        type = types.str;
        default = "[::]:50056";
        description = "Listen address for the reverse-tunnel (mvirt-node agents dial here)";
      };

      nodeId = mkOption {
        type = types.int;
        default = 1;
        description = "Raft node ID";
      };

      extraArgs = mkOption {
        type = types.listOf types.str;
        default = [];
        description = "Extra arguments to pass to mvirt-cplane";
      };
    };

    node = {
      enable = mkEnableOption "mvirt-node (Node agent for reconciliation)";

      apiEndpoint = mkOption {
        type = types.str;
        default = "[::1]:50056";
        description = "mvirt-cplane reverse-tunnel endpoint (host:port; the node TCP-dials here)";
      };

      nodeId = mkOption {
        type = types.str;
        default = "";
        example = "1";
        description = "Stable node id sent in the Identify RPC. Required.";
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

    # mvirt-log service.
    # Runs on cplane hosts only — node-only deployments should set
    # `services.mvirt.log.enable = false`. Depends on mvirt-cplane having
    # bootstrapped the CA and written TLS material to disk; AuditLogger
    # clients on node hosts dial in via mTLS using their node certs.
    systemd.services.mvirt-log = mkIf cfg.log.enable {
      description = "mvirt Logging Service";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" "mvirt-cplane.service" ];
      wants = [ "mvirt-cplane.service" ];

      serviceConfig = {
        Type = "simple";
        # Root because the cert files cplane writes are root-owned; rather
        # than chowning to a shared group, keep the file permissions tight
        # and have both services run as the same user.
        User = "root";
        ExecStart = "${mvirtPkgs}/bin/mvirt-log --listen [::]:${toString cfg.log.port} --data-dir ${cfg.dataDir}/log --tls-ca ${cfg.dataDir}/cplane/log-tls/ca.pem --tls-cert ${cfg.dataDir}/cplane/log-tls/cert.pem --tls-key ${cfg.dataDir}/cplane/log-tls/key.pem ${concatStringsSep " " cfg.log.extraArgs}";
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

    # mvirt-shipper service.
    # Stays on every node — ships journald entries to the cplane-hosted
    # mvirt-log over mTLS. Picks up endpoints + TLS paths from
    # /var/lib/mvirt-node/env which mvirt-node writes during onboarding.
    systemd.services.mvirt-shipper = mkIf cfg.shipper.enable {
      description = "mvirt Journald Log Shipper";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      serviceConfig = {
        Type = "simple";
        User = "root";
        # `-` makes the env file optional: shipper falls back to defaults
        # before onboarding has run.
        EnvironmentFile = "-/var/lib/mvirt-node/env";
        ExecStart = "${mvirtPkgs}/bin/mvirt-shipper --units ${concatStringsSep "," cfg.shipper.units} --cursor-dir ${cfg.dataDir}/shipper ${concatStringsSep " " cfg.shipper.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        # Hardening
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [ cfg.dataDir ];
        ReadOnlyPaths = [ "/var/lib/mvirt-node" ];
        PrivateTmp = true;
      };
    };

    # mvirt-vmm service.
    # Audit logger endpoints + TLS paths come from /var/lib/mvirt-node/env
    # (written by mvirt-node onboarding). Dependency on mvirt-log dropped:
    # AuditLogger reconnects lazily once the remote becomes reachable.
    systemd.services.mvirt-vmm = mkIf cfg.vmm.enable {
      description = "mvirt Virtual Machine Manager";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      # cdrkit provides genisoimage, which mvirt-vmm shells out to in
      # `create_cloudinit_iso` to build the NoCloud CIDATA ISO attached
      # as a second disk for cloud-init's datasource.
      path = [ cfg.cloudHypervisor pkgs.cdrkit ];

      environment = {
        MVIRT_DATA_DIR = cfg.dataDir;
        HYPERVISOR_FW = "${cfg.firmware}/share/firmware/CLOUDHV.fd";
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for KVM/TAP access
        EnvironmentFile = "-/var/lib/mvirt-node/env";
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
      after = [ "network.target" "zfs.target" ];
      requires = [ "zfs.target" ];

      path = [ config.boot.zfs.package pkgs.qemu-utils ];

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for ZFS operations
        EnvironmentFile = "-/var/lib/mvirt-node/env";
        ExecStart = "${mvirtPkgs}/bin/mvirt-zfs --listen [::1]:${toString cfg.zfs.port} --pool ${cfg.zfs.pool} ${concatStringsSep " " cfg.zfs.extraArgs}";
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
      after = [ "network.target" ];

      path = [ pkgs.nftables pkgs.iproute2 ];

      environment = {
        MVIRT_DATA_DIR = cfg.dataDir;
      };

      serviceConfig = {
        Type = "simple";
        User = "root";  # Needs root for eBPF and TUN device
        EnvironmentFile = "-/var/lib/mvirt-node/env";
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
        ExecStart = "${mvirtPkgs}/bin/mvirt-node --api-endpoint ${cfg.node.apiEndpoint} --node-id ${cfg.node.nodeId} --vmm-endpoint http://[::1]:${toString cfg.vmm.port} --zfs-endpoint http://[::1]:${toString cfg.zfs.port} --net-endpoint http://[::1]:${toString cfg.ebpf.port} ${concatStringsSep " " cfg.node.extraArgs}";
        Restart = "on-failure";
        RestartSec = "5s";

        NoNewPrivileges = false;
        ProtectSystem = "full";
        PrivateTmp = true;
      };
    };

    # mvirt-cplane service.
    # mvirt-log is the OTHER way round now: log waits for cplane to come
    # up because cplane writes the cert files mvirt-log reads. Cplane's
    # own AuditLogger reconnects lazily.
    systemd.services.mvirt-cplane = mkIf cfg.cplane.enable {
      description = "mvirt control plane";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = let
          devFlag = if cfg.cplane.dev then " --dev" else "";
          nodeIdFlag = " --node-id ${toString cfg.cplane.nodeId}";
        in "${mvirtPkgs}/bin/mvirt-cplane --listen [::]:${toString cfg.cplane.port} --tunnel-listen ${cfg.cplane.tunnelListen} --data-dir ${cfg.dataDir}/cplane --log-endpoint https://[::1]:${toString cfg.log.port}${devFlag}${nodeIdFlag} ${concatStringsSep " " cfg.cplane.extraArgs}";
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
