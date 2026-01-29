# Hypervisor firmware files
{ pkgs }:

let
  # rust-hypervisor-firmware version
  hypervisorFwVersion = "0.4.2";

  # EDK2 version (from cloud-hypervisor/edk2 releases)
  edk2Version = "a54f262b09";

in {
  # rust-hypervisor-firmware - lightweight UEFI firmware
  hypervisor-fw = pkgs.stdenv.mkDerivation {
    pname = "hypervisor-fw";
    version = hypervisorFwVersion;

    src = pkgs.fetchurl {
      url = "https://github.com/cloud-hypervisor/rust-hypervisor-firmware/releases/download/${hypervisorFwVersion}/hypervisor-fw";
      sha256 = "sha256-WMFGE7xmBnI/GBJNAPujRk+vMx1ssGp//lbeYtgHEkA=";
    };

    dontUnpack = true;
    dontBuild = true;

    installPhase = ''
      runHook preInstall

      mkdir -p $out/share/firmware
      cp $src $out/share/firmware/hypervisor-fw

      runHook postInstall
    '';

    meta = with pkgs.lib; {
      description = "Simple KVM firmware for cloud-hypervisor";
      homepage = "https://github.com/cloud-hypervisor/rust-hypervisor-firmware";
      license = licenses.asl20;
      platforms = [ "x86_64-linux" ];
    };
  };

  # EDK2 CLOUDHV.fd - full UEFI firmware for cloud-hypervisor
  edk2-cloudhv = pkgs.stdenv.mkDerivation {
    pname = "edk2-cloudhv";
    version = edk2Version;

    src = pkgs.fetchurl {
      url = "https://github.com/cloud-hypervisor/edk2/releases/download/ch-${edk2Version}/CLOUDHV.fd";
      sha256 = "sha256-BiTAbF0Hy47+OIBokM5wdsQcCQLy/NWyN28QcDPjIis=";
    };

    dontUnpack = true;
    dontBuild = true;

    installPhase = ''
      runHook preInstall

      mkdir -p $out/share/firmware
      cp $src $out/share/firmware/CLOUDHV.fd

      runHook postInstall
    '';

    meta = with pkgs.lib; {
      description = "EDK2 UEFI firmware for cloud-hypervisor";
      homepage = "https://github.com/cloud-hypervisor/edk2";
      license = licenses.bsd2;
      platforms = [ "x86_64-linux" ];
    };
  };
}
