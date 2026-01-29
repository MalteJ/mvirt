# NixOS Node VM Image - bootable raw disk image for mvirt cluster nodes
{ config, lib, pkgs, modulesPath, self, mvirtPkgs, ... }:

{
  imports = [
    "${modulesPath}/profiles/qemu-guest.nix"
  ];

  # System identification
  system.stateVersion = "24.11";
  networking.hostId = lib.mkDefault "aabbccdd";  # Overridden per-node

  # Boot configuration
  boot = {
    loader.systemd-boot.enable = lib.mkDefault false;
    loader.efi.canTouchEfiVariables = false;
    loader.grub = {
      enable = lib.mkDefault true;
      efiSupport = true;
      efiInstallAsRemovable = true;
      device = "nodev";
    };

    supportedFilesystems = [ "zfs" ];
    zfs.devNodes = "/dev/disk/by-path";

    kernelModules = [
      "kvm-intel"
      "kvm-amd"
      "vhost_net"
      "vhost_vsock"
      "tun"
      "bridge"
    ];

    initrd.kernelModules = [
      "virtio_pci"
      "virtio_blk"
      "virtio_net"
    ];

    kernelParams = [
      "console=ttyS0"
      "intel_iommu=on"
      "transparent_hugepage=never"
    ];

    kernelPackages = pkgs.linuxPackages_6_12;

    growPartition = true;
  };

  # Filesystem layout (for image build)
  fileSystems."/" = {
    device = "/dev/vda2";
    fsType = "ext4";
    autoResize = true;
  };

  fileSystems."/boot" = {
    device = "/dev/vda1";
    fsType = "vfat";
  };

  # Networking - DHCP by default, overridden per-node
  networking = {
    useNetworkd = true;
    useDHCP = true;

    bridges.br0 = {
      interfaces = [];
    };

    firewall = {
      enable = true;
      allowedTCPPorts = [
        22
        50051  # mvirt-vmm
        50052  # mvirt-log
        50053  # mvirt-zfs
        50054  # mvirt-ebpf
        8080   # mvirt-api
        50056  # mvirt-api grpc (node agents)
      ];
    };
  };

  boot.kernel.sysctl = {
    "net.ipv4.ip_forward" = 1;
    "net.ipv6.conf.all.forwarding" = 1;
  };

  # mvirt services
  services.mvirt = {
    enable = true;
    package = mvirtPkgs.mvirt;
    cloudHypervisor = mvirtPkgs.cloud-hypervisor;
    firmware = mvirtPkgs.edk2-cloudhv;

    vmm.enable = true;
    log.enable = true;
    ebpf.enable = true;
    zfs.enable = true;
  };

  # Services
  services.chrony.enable = true;

  services.openssh = {
    enable = true;
    settings = {
      PermitRootLogin = "prohibit-password";
      PasswordAuthentication = false;
    };
  };

  # Users
  users.users.malte = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    openssh.authorizedKeys.keys = [
      "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCa9y9UIvZud2ktO7nlyK/kBZYaBw1pfQjznf93Q/XIw2/ciAgTvM+0OayNqzYrhqcoYVmil+xN8K+z96ABECVvB1SiTTjoddXw9BhZm3e5qUSyEBWmb3lbF4rhZKfQ9EiqZ1W7EAvfWDFAsy8eo47ZDOf4+c0ud4vI2zmlX3RbnAz2aNsatLnW48vRxmh+DNmkvQIJfi0vd2V2BXCF1a2++Wu/5NQrYGcSNyaZpeBuczAuGGhqp2t6ielDMDF6dyoSn+OMTnmRY4AqYPDGWD2MeQg6G3YUSkkIrVxvAj525OqqMOAQogT7JLmwXxHoXXzByTobFcIt3MNnFX+tGV6n"
    ];
  };

  users.users.root.openssh.authorizedKeys.keys = [
    "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCa9y9UIvZud2ktO7nlyK/kBZYaBw1pfQjznf93Q/XIw2/ciAgTvM+0OayNqzYrhqcoYVmil+xN8K+z96ABECVvB1SiTTjoddXw9BhZm3e5qUSyEBWmb3lbF4rhZKfQ9EiqZ1W7EAvfWDFAsy8eo47ZDOf4+c0ud4vI2zmlX3RbnAz2aNsatLnW48vRxmh+DNmkvQIJfi0vd2V2BXCF1a2++Wu/5NQrYGcSNyaZpeBuczAuGGhqp2t6ielDMDF6dyoSn+OMTnmRY4AqYPDGWD2MeQg6G3YUSkkIrVxvAj525OqqMOAQogT7JLmwXxHoXXzByTobFcIt3MNnFX+tGV6n"
  ];

  security.sudo.wheelNeedsPassword = false;

  # Packages
  environment.systemPackages = with pkgs; [
    vim htop tmux git nftables
    bridge-utils iproute2 tcpdump curl
    parted
    mvirtPkgs.mvirt-cli
  ];

  # Serial console
  systemd.services."serial-getty@ttyS0" = {
    enable = true;
    wantedBy = [ "multi-user.target" ];
  };

  # Minimal
  documentation.enable = false;
  nix.registry = lib.mkForce {};
  nix.channel.enable = false;
}
