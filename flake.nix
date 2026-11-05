{
  description = "ntfy-dashboard: at-a-glance meal picker and diet history for Nice To Fit You";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        ntfy-dashboard = pkgs.callPackage ./nix/package.nix { };
        default = ntfy-dashboard;
      });

      # `nix develop`, then `cargo run -- --demo` (no account needed) or `cargo run`.
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer sqlite cargo-watch ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };
      });

      # The module builds the package with the host's nixpkgs; this pins it to the flake's build.
      nixosModules.default = { pkgs, ... }: {
        imports = [ ./nix/module.nix ];
        services.ntfy-dashboard.package =
          nixpkgs.lib.mkDefault self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      };

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });
    };
}
