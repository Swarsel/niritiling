{
  description = "niritiling - automatic window tiling for the first window in Niri";

  inputs = {
    flake-parts.url = "github:hercules-ci/flake-parts";

    git-hooks-nix = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:cachix/git-hooks.nix";
    };

    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    pedantix = {
      inputs = {
        flake-parts.follows = "flake-parts";
        git-hooks-nix.follows = "git-hooks-nix";
        nixpkgs.follows = "nixpkgs";
        treefmt-nix.follows = "treefmt-nix";
      };
      url = "github:Swarsel/pedantix";
    };

    treefmt-nix = {
      inputs.nixpkgs.follows = "nixpkgs";
      url = "github:numtide/treefmt-nix";
    };
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      flake =
        let
          inherit (inputs.nixpkgs) lib;
        in
        {
          nixosModules = rec {
            default = niritiling;
            niritiling = { pkgs, ... }: {
              imports = [ ./nix/module.nix ];

              options.services.niritiling.package = lib.mkOption {
                default = inputs.self.packages.${pkgs.stdenv.hostPlatform.system}.niritiling;
                description = "The niritiling package to use.";
                type = lib.types.package;
              };
            };
          };
        };
      imports = [
        inputs.treefmt-nix.flakeModule
        inputs.git-hooks-nix.flakeModule
        inputs.pedantix.flakeModules.default
      ];
      perSystem =
        { config
        , pkgs
        , self'
        , ...
        }:
        {
          devShells.default = pkgs.mkShell {
            inputsFrom = [ self'.packages.default ];
            nativeBuildInputs = with pkgs; [
              rust-analyzer
              clippy
            ];
            shellHook = config.pre-commit.installationScript;
          };
          packages = rec {
            default = niritiling;
            niritiling = pkgs.rustPlatform.buildRustPackage {
              cargoLock.lockFile = ./Cargo.lock;
              pname = "niritiling";
              src = ./.;
              version = "0.3.0";
              meta.mainProgram = "niritiling";
            };
          };
          pre-commit.settings = {
            hooks = {
              clippy.enable = true;
              treefmt.enable = true;
            };
            settings.rust = {
              cargoManifestPath = "./Cargo.toml";
              check.cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
            };
          };
          treefmt.programs = {
            deadnix.enable = true;
            pedantix.enable = true;
            rustfmt.enable = true;
            statix.enable = true;
          };
        };
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    };
}
