# Crane-based Rust derivation for mvirt packages
{ pkgs, craneLib }:

let
  # Source filtering - include Rust, proto, and SQL migration files
  src = pkgs.lib.cleanSourceWith {
    src = ../..;
    filter = path: type:
      (craneLib.filterCargoSources path type) ||
      (builtins.match ".*\.proto$" path != null) ||
      (builtins.match ".*/proto/.*" path != null) ||
      (builtins.match ".*\.sql$" path != null) ||
      (builtins.match ".*/migrations/.*" path != null);
  };

  # Common arguments for all builds
  commonArgs = {
    inherit src;

    pname = "mvirt";
    version = "0.1.0";

    # Build for musl target (static linking)
    CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";

    # Static linking flags
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS = "-C target-feature=+crt-static";

    # Linker for musl target
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${pkgs.pkgsCross.musl64.stdenv.cc}/bin/x86_64-unknown-linux-musl-gcc";

    # C compiler for musl (needed for -sys crates like libsqlite3-sys)
    CC_x86_64_unknown_linux_musl = "${pkgs.pkgsCross.musl64.stdenv.cc}/bin/x86_64-unknown-linux-musl-gcc";
    CXX_x86_64_unknown_linux_musl = "${pkgs.pkgsCross.musl64.stdenv.cc}/bin/x86_64-unknown-linux-musl-g++";
    AR_x86_64_unknown_linux_musl = "${pkgs.pkgsCross.musl64.stdenv.cc}/bin/x86_64-unknown-linux-musl-ar";

    # Protobuf compiler
    PROTOC = "${pkgs.protobuf}/bin/protoc";

    nativeBuildInputs = with pkgs; [
      protobuf
      pkg-config
      pkgsCross.musl64.stdenv.cc
    ];

    buildInputs = with pkgs; [
      # OpenSSL for reqwest (mvirt-cli, mvirt-zfs)
      pkgsCross.musl64.openssl
    ];

    # Environment for static OpenSSL (musl version)
    OPENSSL_STATIC = "1";
    OPENSSL_LIB_DIR = "${pkgs.pkgsCross.musl64.openssl.out}/lib";
    OPENSSL_INCLUDE_DIR = "${pkgs.pkgsCross.musl64.openssl.dev}/include";

    # Disable running tests during build (they require runtime environment)
    doCheck = false;

    # Use thin LTO to reduce memory usage during link (full LTO can OOM in sandbox)
    cargoExtraArgs = "--config 'profile.release.lto=\"thin\"'";
  };

  # Build only dependencies (for caching)
  cargoArtifacts = craneLib.buildDepsOnly (commonArgs // {
    # Dummy src to build deps
    src = craneLib.cleanCargoSource src;
  });

  # Build all workspace members
  mvirt = craneLib.buildPackage (commonArgs // {
    inherit cargoArtifacts;

    # Install all binaries
    postInstall = ''
      # Rename mvirt-cli binary to mvirt if needed
      if [ -f $out/bin/mvirt-cli ]; then
        mv $out/bin/mvirt-cli $out/bin/mvirt
      fi
    '';

    meta = with pkgs.lib; {
      description = "mvirt Virtual Machine Manager";
      homepage = "https://github.com/mvirt/mvirt";
      license = licenses.asl20;
      platforms = [ "x86_64-linux" ];
    };
  });

  # Build individual packages
  buildPackage = name: binName: craneLib.buildPackage (commonArgs // {
    inherit cargoArtifacts;

    pname = name;
    cargoExtraArgs = "-p ${name} --config 'profile.release.lto=\"thin\"'";

    postInstall = pkgs.lib.optionalString (binName != null) ''
      # Keep only the specific binary
      find $out/bin -type f ! -name "${binName}" -delete 2>/dev/null || true
    '';

    meta = with pkgs.lib; {
      description = "mvirt ${name}";
      homepage = "https://github.com/mvirt/mvirt";
      license = licenses.asl20;
      platforms = [ "x86_64-linux" ];
    };
  });

in {
  inherit mvirt cargoArtifacts;

  # Individual packages
  mvirt-cli = buildPackage "mvirt-cli" "mvirt";
  mvirt-vmm = buildPackage "mvirt-vmm" "mvirt-vmm";
  mvirt-zfs = buildPackage "mvirt-zfs" "mvirt-zfs";
  mvirt-net = buildPackage "mvirt-net" "mvirt-net";
  mvirt-log = buildPackage "mvirt-log" "mvirt-log";
}
