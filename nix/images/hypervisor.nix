# NixOS Hypervisor Image Configuration
{ config, lib, pkgs, modulesPath, self, mvirtPkgs, disko, ... }:

{
  imports = [
    # Include the ISO image generator (without base.nix bloat)
    "${modulesPath}/installer/cd-dvd/iso-image.nix"
  ];

  # System identification
  system.stateVersion = "24.11";
  networking.hostName = "mvirt-hypervisor";
  networking.hostId = "deadbeef";  # Required for ZFS

  # Boot configuration
  boot = {
    # KVM modules
    kernelModules = [
      "kvm-intel"
      "kvm-amd"
      "vhost_net"
      "vhost_vsock"
      "tun"
      "bridge"
    ];

    # Load vhost_net early
    initrd.kernelModules = [ "virtio_pci" "virtio_blk" ];

    # Kernel parameters for KVM
    kernelParams = [
      "intel_iommu=on"
      "amd_iommu=on"
    ];

    # Use LTS kernel for stability and ZFS compatibility
    kernelPackages = pkgs.linuxPackages_6_6;
  };

  # ISO image settings
  isoImage = {
    makeEfiBootable = true;
    makeUsbBootable = true;
    isoName = "mvirt-hypervisor-${config.system.nixos.label}-${pkgs.stdenv.hostPlatform.system}.iso";
    volumeID = "MVIRT_HV";
  };

  # Enable KVM
  virtualisation.libvirtd.enable = false;  # We use cloud-hypervisor, not libvirt
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;

  # Networking
  networking = {
    useDHCP = false;

    # Bridge for VM networking
    bridges.br0 = {
      interfaces = [];  # Will be configured with physical interfaces manually
    };

    interfaces.br0.useDHCP = true;

    # Firewall configuration
    firewall = {
      enable = true;
      allowedTCPPorts = [
        22      # SSH
        50051   # mvirt-vmm gRPC
        50052   # mvirt-log gRPC
        50053   # mvirt-zfs gRPC
        50054   # mvirt-net gRPC
      ];
    };

    # Enable IP forwarding for VM traffic
    nat.enable = false;
  };

  # Enable IP forwarding
  boot.kernel.sysctl = {
    "net.ipv4.ip_forward" = 1;
    "net.ipv6.conf.all.forwarding" = 1;
    "net.bridge.bridge-nf-call-iptables" = 0;
    "net.bridge.bridge-nf-call-ip6tables" = 0;
  };

  # mvirt services
  services.mvirt = {
    enable = true;
    package = mvirtPkgs.mvirt;
    cloudHypervisor = mvirtPkgs.cloud-hypervisor;
    firmware = mvirtPkgs.hypervisor-fw;

    vmm.enable = true;
    log.enable = true;
    net.enable = true;
    zfs.enable = true;
  };

  # SSH access
  services.openssh = {
    enable = true;
    settings = {
      PermitRootLogin = "prohibit-password";
      PasswordAuthentication = false;
    };
  };

  # Allow root SSH with key
  users.users.root = {
    openssh.authorizedKeys.keys = [
      # Add your SSH public key here or override in your configuration
      # "ssh-ed25519 AAAA..."
    ];
  };

  # Admin user
  users.users.admin = {
    isNormalUser = true;
    initialPassword = "admin";
    extraGroups = [ "wheel" "mvirt" ];
    openssh.authorizedKeys.keys = [
      # Add your SSH public key here or override in your configuration
    ];
  };

  # Sudo without password for wheel group (for live ISO convenience)
  security.sudo.wheelNeedsPassword = false;

  # System packages
  environment.systemPackages = with pkgs; [
    vim
    htop
    tmux
    git

    # Networking tools
    bridge-utils
    iproute2
    iptables
    tcpdump

    # Disk tools
    parted
    util-linux

    # mvirt CLI
    mvirtPkgs.mvirt-cli

    # NixOS installer
    mvirtPkgs.nixos-wizard
    disko.packages.${pkgs.system}.disko
  ];

  # Enable serial console for headless operation
  systemd.services."serial-getty@ttyS0" = {
    enable = true;
    wantedBy = [ "multi-user.target" ];
  };

  # Disable documentation (saves ~50 MB)
  documentation.enable = false;
  documentation.nixos.enable = false;

  # Disable nix flake registry (saves ~200 MB nixpkgs source)
  nix.registry = lib.mkForce {};
  nix.channel.enable = false;

  # Enable ZFS
  boot.supportedFilesystems.zfs = true;
}
