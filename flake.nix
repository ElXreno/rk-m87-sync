{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "rk-m87-sync";
            version = "0.1.0"; # x-release-please-version
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
        }
      );

      nixosModules.default = import ./nix/module.nix self;

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.default ];
            packages = with pkgs; [
              rust-analyzer
              clippy
              rustfmt
            ];
          };
        }
      );
    };
}
