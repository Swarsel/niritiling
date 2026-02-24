{
  description = "niritiling - automatic window tiling for the first window in Niri";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks-nix = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs: inputs.flake-parts.lib.mkFlake { inherit inputs; } {
    imports = [
      inputs.treefmt-nix.flakeModule
      inputs.git-hooks-nix.flakeModule
    ];

    systems = [ "x86_64-linux" "aarch64-linux" ];

    perSystem = { config, self', pkgs, ... }: {
      treefmt = {
        programs = {
          nixpkgs-fmt.enable = true;
          rustfmt.enable = true;
          deadnix.enable = true;
          statix.enable = true;
        };
      };

      pre-commit.settings = {
        settings = {
          rust = {
            cargoManifestPath = "./Cargo.toml";
            check.cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
          };
        };
        hooks = {
          treefmt.enable = true;
          clippy.enable = true;
        };
      };

      packages.default = pkgs.rustPlatform.buildRustPackage {
        pname = "niritiling";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
      };

      devShells.default = pkgs.mkShell {
        inputsFrom = [ self'.packages.default ];
        nativeBuildInputs = with pkgs; [
          rust-analyzer
          clippy
        ];
        shellHook = config.pre-commit.installationScript;
      };
    };

    flake = {
      nixosModules.default = { pkgs, ... }: {
        imports = [ ./nix/module.nix ];
        services.niritiling.package = inputs.nixpkgs.lib.mkDefault inputs.self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      };
    };
  };
}
