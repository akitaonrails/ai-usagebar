{
  description = "AI plan usage for Waybar, Omarchy, and the terminal";

  # Keep updates on the maintained final release branch for x86_64-darwin.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-26.05-darwin";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
      packageFor = system: (pkgsFor system).callPackage ./nix/package.nix { };
    in
    {
      packages = forAllSystems (
        system:
        let
          package = packageFor system;
        in
        {
          default = package;
          ai-usagebar = package;
        }
      );

      apps = forAllSystems (
        system:
        let
          package = packageFor system;
        in
        {
          default = {
            type = "app";
            program = nixpkgs.lib.getExe package;
          };
          tui = {
            type = "app";
            program = nixpkgs.lib.getExe' package "ai-usagebar-tui";
          };
        }
      );

      overlays.default = final: _prev: {
        ai-usagebar = final.callPackage ./nix/package.nix { };
      };

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                actionlint
                cargo
                cargo-machete
                clippy
                gnumake
                nasm
                nixfmt-rfc-style
                nodejs
                rustc
                rustfmt
              ]
              ++ pkgs.lib.optional pkgs.stdenv.hostPlatform.isLinux mold;
            env = pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
              "CARGO_TARGET_${pkgs.stdenv.hostPlatform.rust.cargoEnvVarTarget}_RUSTFLAGS" =
                "-C link-arg=-fuse-ld=mold";
            };
          };
        }
      );

      checks = forAllSystems (system: {
        default = self.packages.${system}.default;
      });
    };
}
