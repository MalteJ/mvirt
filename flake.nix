{
  description = "mvirt - Virtual Machine Manager with NixOS Hypervisor Image";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    crane = {
      url = "github:ipetkov/crane";
    };

    disko = {
      url = "github:nix-community/disko/latest";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, crane, disko }:
    let
      system = "x86_64-linux";

      pkgs = import nixpkgs {
        inherit system;
        overlays = [ rust-overlay.overlays.default ];
      };

      # Rust toolchain with musl target for static linking
      rustToolchain = pkgs.rust-bin.stable.latest.default.override {
        targets = [ "x86_64-unknown-linux-musl" ];
      };

      # Crane library configured with our toolchain
      craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

      # Import package definitions
      mvirtPackages = import ./nix/packages/mvirt.nix {
        inherit pkgs craneLib;
      };

      cloudHypervisor = import ./nix/packages/cloud-hypervisor.nix {
        inherit pkgs;
      };

      hypervisorFw = import ./nix/packages/hypervisor-fw.nix {
        inherit pkgs;
      };

    in {
      # Individual packages
      packages.${system} = {
        # Main mvirt package (all binaries)
        mvirt = mvirtPackages.mvirt;

        # Individual components
        mvirt-cli = mvirtPackages.mvirt-cli;
        mvirt-vmm = mvirtPackages.mvirt-vmm;
        mvirt-zfs = mvirtPackages.mvirt-zfs;
        mvirt-ebpf = mvirtPackages.mvirt-ebpf;
        mvirt-log = mvirtPackages.mvirt-log;

        # External dependencies
        cloud-hypervisor = cloudHypervisor;
        hypervisor-fw = hypervisorFw.hypervisor-fw;
        edk2-cloudhv = hypervisorFw.edk2-cloudhv;

        # Hypervisor ISO image
        hypervisor-image = self.nixosConfigurations.hypervisor.config.system.build.isoImage;

        default = mvirtPackages.mvirt;
      };

      # NixOS module
      nixosModules.default = import ./nix/modules/mvirt.nix;
      nixosModules.mvirt = import ./nix/modules/mvirt.nix;

      # NixOS configurations
      nixosConfigurations.hypervisor = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = {
          inherit self disko;
          mvirtPkgs = self.packages.${system};
        };
        modules = [
          self.nixosModules.mvirt
          disko.nixosModules.disko
          ./nix/images/hypervisor.nix
        ];
      };

      # Development shell
      devShells.${system}.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          # Rust toolchain
          rustToolchain
          rust-analyzer

          # Build dependencies
          protobuf
          pkg-config
          openssl

          # musl for static linking
          musl
          musl.dev

          # Additional tools
          sqlite

          # For testing
          qemu
        ];

        PROTOC = "${pkgs.protobuf}/bin/protoc";

        # For musl static builds
        CARGO_BUILD_TARGET = "x86_64-unknown-linux-musl";
        CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${pkgs.musl.dev}/bin/musl-gcc";

        shellHook = ''
          echo "mvirt development shell"
          echo "  cargo build --release  # Build all packages"
          echo "  nix build .#mvirt      # Build with Nix"
          echo "  nix build .#hypervisor-image  # Build ISO"
        '';
      };
    };
}
