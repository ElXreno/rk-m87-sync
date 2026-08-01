{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      { moduleWithSystem, ... }:
      {
        systems = [
          "x86_64-linux"
          "aarch64-linux"
        ];

        perSystem =
          {
            config,
            pkgs,
            lib,
            ...
          }:
          {
            packages.default = pkgs.rustPlatform.buildRustPackage {
              pname = "rk-m87-sync";
              version = "0.1.1"; # x-release-please-version
              src = ./.;
              cargoLock.lockFile = ./Cargo.lock;
              nativeBuildInputs = with pkgs; [ pkg-config ];
              buildInputs = with pkgs; [
                libpulseaudio
              ];
              meta = {
                description = "Sync system time and volume to RK M87 keyboard LCD";
                license = lib.licenses.mit;
                mainProgram = "rk-m87-sync";
              };
            };

            devShells.default = pkgs.mkShell {
              inputsFrom = [ config.packages.default ];
              packages = with pkgs; [
                rust-analyzer
                clippy
                rustfmt
              ];
            };
          };

        flake.nixosModules.default = moduleWithSystem (
          { config, ... }: _: { imports = [ (import ./nix/module.nix config.packages.default) ]; }
        );
      }
    );
}
