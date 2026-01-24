# Static cloud-hypervisor binary from GitHub Releases
{ pkgs }:

let
  version = "50.0";
in

pkgs.stdenv.mkDerivation {
  pname = "cloud-hypervisor";
  inherit version;

  src = pkgs.fetchurl {
    url = "https://github.com/cloud-hypervisor/cloud-hypervisor/releases/download/v${version}/cloud-hypervisor-static";
    sha256 = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";  # Will be updated on first build
  };

  dontUnpack = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    mkdir -p $out/bin
    cp $src $out/bin/cloud-hypervisor
    chmod +x $out/bin/cloud-hypervisor

    runHook postInstall
  '';

  meta = with pkgs.lib; {
    description = "Open source Virtual Machine Monitor (VMM) for KVM";
    homepage = "https://github.com/cloud-hypervisor/cloud-hypervisor";
    license = licenses.asl20;
    platforms = [ "x86_64-linux" ];
    mainProgram = "cloud-hypervisor";
  };
}
