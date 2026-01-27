# NixOS Hypervisor Image Configuration
{ config, lib, pkgs, modulesPath, self, mvirtPkgs, disko, ... }:

let
  # Custom GRUB theme for mvirt (directory must be named grub-theme for NixOS ISO)
  mvirtGrubTheme = pkgs.runCommand "mvirt-grub-theme" {} ''
    mkdir -p $out/grub-theme

    # Copy background image
    cp ${./assets/splash.png} $out/grub-theme/background.png

    # Create theme.txt with simple config (no missing image references)
    cat > $out/grub-theme/theme.txt << 'EOF'
# mvirt GRUB Theme
desktop-image: "background.png"
desktop-color: "#0d0d1a"

title-text: ""

message-font: "Unifont Regular 16"
message-color: "#ffffff"
message-bg-color: "#1a1a2e"

terminal-font: "Unifont Regular 16"

+ boot_menu {
  left = 25%
  top = 55%
  width = 50%
  height = 40%
  item_font = "Unifont Regular 16"
  item_color = "#ffffff"
  selected_item_font = "Unifont Regular 16"
  selected_item_color = "#00ffff"
  item_height = 28
  item_padding = 8
  item_spacing = 8
}
EOF
  '';

in
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

    # Load virtio modules early for boot
    initrd.kernelModules = [
      "virtio_pci"
      "virtio_blk"
    ];

    # Reserve hugepages in stage 1 (before memory fragmentation)
    initrd.postDeviceCommands = ''
      # Get total memory in kB
      total_kb=$(awk '/MemTotal/ {print $2}' /proc/meminfo)

      # Option 1: Total minus 4 GiB (4194304 kB)
      minus_4g_kb=$((total_kb - 4194304))

      # Option 2: 95% of total
      pct95_kb=$((total_kb * 95 / 100))

      # Take the minimum of both (ensures at least 4 GiB free, but max 95%)
      if [ $minus_4g_kb -lt $pct95_kb ]; then
        hugepages_kb=$minus_4g_kb
      else
        hugepages_kb=$pct95_kb
      fi

      # Don't go negative
      if [ $hugepages_kb -lt 0 ]; then
        hugepages_kb=0
      fi

      # Each 2MB hugepage = 2048 kB
      nr_hugepages=$((hugepages_kb / 2048))

      echo "Reserving $nr_hugepages x 2MB hugepages for VMs"
      echo $nr_hugepages > /proc/sys/vm/nr_hugepages
    '';

    # Kernel parameters for KVM
    kernelParams = [
      "intel_iommu=on"
      "amd_iommu=on"
      "transparent_hugepage=never"  # Disable THP, we use explicit hugepages
    ];

    # 6.12 LTS - latest with ZFS support (6.18 not yet compatible)
    kernelPackages = pkgs.linuxPackages_6_12;

  };

  # ISO image settings
  isoImage = {
    makeEfiBootable = true;
    makeUsbBootable = true;
    isoName = "mvirt-hypervisor-${config.system.nixos.label}-${pkgs.stdenv.hostPlatform.system}.iso";
    volumeID = "MVIRT_HV";

    # Replace NixOS branding with mvirt (1024x768 for bootloader)
    splashImage = ./assets/splash.png;
    efiSplashImage = ./assets/splash.png;

    # Custom GRUB theme with mvirt branding
    grubTheme = "${mvirtGrubTheme}/grub-theme";

    # Syslinux theme for BIOS boot (fix colors to be visible)
    syslinuxTheme = ''
      MENU TITLE mvirt
      MENU RESOLUTION 1024 768
      MENU CLEAR
      MENU ROWS 6
      MENU CMDLINEROW -4
      MENU TIMEOUTROW -3
      MENU TABMSGROW  -2
      MENU HELPMSGROW -1
      MENU HELPMSGENDROW -1
      MENU MARGIN 0

      #                                FG:AARRGGBB  BG:AARRGGBB   shadow
      MENU COLOR BORDER       30;44      #00000000    #00000000   none
      MENU COLOR SCREEN       37;40      #FF000000    #00000000   none
      MENU COLOR TABMSG       31;40      #80FFFFFF    #00000000   none
      MENU COLOR TIMEOUT      1;37;40    #FFFFFFFF    #00000000   none
      MENU COLOR TIMEOUT_MSG  37;40      #FFFFFFFF    #00000000   none
      MENU COLOR CMDMARK      1;36;40    #FF00FFFF    #00000000   none
      MENU COLOR CMDLINE      37;40      #FFFFFFFF    #00000000   none
      MENU COLOR TITLE        1;36;44    #00000000    #00000000   none
      MENU COLOR UNSEL        37;44      #FFFFFFFF    #00000000   none
      MENU COLOR SEL          7;37;40    #FFFFFFFF    #FF5277C3   std
    '';
  };

  # Override boot entry name and set resolution to match splash image
  boot.loader.grub = {
    memtest86.enable = false;
    splashImage = ./assets/splash.png;
    gfxmodeBios = "1024x768";
    gfxmodeEfi = "1024x768";
    gfxpayloadBios = "keep";
    gfxpayloadEfi = "keep";
  };

  # Change system name displayed in boot
  system.nixos.distroName = "mvirt";

  # CPU microcode updates
  hardware.cpu.intel.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;

  # Networking
  networking = {
    useNetworkd = true;
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
    firmware = mvirtPkgs.edk2-cloudhv;

    vmm.enable = true;
    log.enable = true;
    net.enable = true;
    zfs.enable = true;
  };

  # NTP for time synchronization
  services.timesyncd.enable = true;

  # SSH access
  services.openssh = {
    enable = true;
    settings = {
      PermitRootLogin = "prohibit-password";
      PasswordAuthentication = false;
    };
  };

  # Root user (no password on live ISO, set password after install)
  users.users.root = {
    initialHashedPassword = "";  # Empty password for live ISO auto-login
    openssh.authorizedKeys.keys = [
      # Add your SSH public key here or override in your configuration
      # "ssh-ed25519 AAAA..."
    ];
  };

  # System packages
  environment.systemPackages = with pkgs; [
    vim
    htop
    tmux

    # Networking tools
    bridge-utils
    iproute2
    iptables
    tcpdump
    ethtool
    curl
    net-tools      # netstat
    mtr            # mtr
    dnsutils       # dig, nslookup

    # Disk tools
    parted
    util-linux
    smartmontools

    # Debugging tools
    pciutils
    lsof

    # mvirt CLI
    mvirtPkgs.mvirt-cli

    # Disk partitioning
    disko.packages.${pkgs.system}.disko
  ];

  # Auto-login root on tty1 (live ISO only)
  services.getty.autologinUser = "root";

  # Welcome message with install instructions
  users.motd = ''

    Welcome to mvirt Hypervisor

    Installation guide: https://mvirt.malte.io/install

  '';

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
