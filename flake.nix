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

    colmena = {
      url = "github:zhaofengli/colmena";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, rust-overlay, crane, disko, colmena }:
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

        # Node VM disk image (4 GB raw)
        node-image = self.nixosConfigurations.node.config.system.build.image;

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

      # Node VM image configuration
      nixosConfigurations.node = nixpkgs.lib.nixosSystem {
        inherit system;
        specialArgs = {
          inherit self;
          mvirtPkgs = self.packages.${system};
        };
        modules = [
          self.nixosModules.mvirt
          ./nix/images/node.nix
          ({ config, lib, pkgs, modulesPath, ... }: {
            imports = [ "${modulesPath}/image/repart.nix" ];

            # GRUB config now in node.nix

            image.repart = {
              name = "mvirt-node";
              partitions = {
                "00-esp" = {
                  contents =
                    let
                      toplevel = config.system.build.toplevel;
                      grubCfg = pkgs.writeText "grub.cfg" ''
                        serial --speed=115200
                        terminal_output serial console
                        terminal_input serial console

                        set timeout=3
                        search --set=root --label nixos

                        menuentry "NixOS" {
                          linux ${toplevel}/kernel init=${toplevel}/init ${builtins.concatStringsSep " " config.boot.kernelParams}
                          initrd ${toplevel}/initrd
                        }
                      '';
                      grubEfi = pkgs.runCommand "grub-efi-image" { nativeBuildInputs = [ pkgs.grub2_efi ]; } ''
                        mkdir -p $out
                        grub-mkimage \
                          -o $out/BOOTX64.EFI \
                          -p /EFI/BOOT \
                          -O x86_64-efi \
                          part_gpt fat ext2 normal boot linux search search_label configfile serial terminal
                      '';
                    in
                    {
                      "/EFI/BOOT/BOOTX64.EFI".source = "${grubEfi}/BOOTX64.EFI";
                      "/EFI/BOOT/grub.cfg".source = grubCfg;
                    };
                  repartConfig = {
                    Type = "esp";
                    Format = "vfat";
                    SizeMinBytes = "256M";
                    SizeMaxBytes = "256M";
                  };
                };
                "10-root" = {
                  storePaths = [ config.system.build.toplevel ];
                  repartConfig = {
                    Type = "root";
                    Format = "ext4";
                    SizeMinBytes = "3500M";
                    Label = "nixos";
                  };
                };
              };
            };
          })
        ];
      };

      # Colmena deployment
      colmenaHive = colmena.lib.makeHive self.colmena;
      colmena = {
        meta = {
          nixpkgs = pkgs;
          specialArgs = {
            inherit self;
            mvirtPkgs = self.packages.${system};
          };
        };

        defaults = { ... }: {
          imports = [
            self.nixosModules.mvirt
            ./nix/images/node.nix
          ];
          services.mvirt.node = {
            enable = true;
            apiEndpoint = "http://10.0.0.1:50056";
          };
        };

        node-1 = { ... }: {
          deployment = {
            targetHost = "10.0.0.11";
            targetUser = "root";
          };
          networking.hostName = "mvirt-node-1";
          networking.hostId = "a1b2c3d1";
        };

        node-2 = { ... }: {
          deployment = {
            targetHost = "10.0.0.12";
            targetUser = "root";
          };
          networking.hostName = "mvirt-node-2";
          networking.hostId = "a1b2c3d2";
        };

        node-3 = { ... }: {
          deployment = {
            targetHost = "10.0.0.13";
            targetUser = "root";
          };
          networking.hostName = "mvirt-node-3";
          networking.hostId = "a1b2c3d3";
        };
      };

      # Colmena app for `nix run .#colmena`
      apps.${system}.colmena = {
        type = "app";
        program = "${colmena.packages.${system}.colmena}/bin/colmena";
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

          # Deployment
          colmena.packages.${system}.colmena

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
