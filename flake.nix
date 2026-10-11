{
  description = "rCore Environment";

  inputs = {
    flake-compat = {
      url = "https://git.lix.systems/lix-project/flake-compat/archive/main.tar.gz";
    };

    flake-parts = {
      url = "github:hercules-ci/flake-parts/main";
      inputs.nixpkgs-lib.follows = "nixpkgs";
    };

    nixpkgs = {
      url = "github:NixOS/nixpkgs/nixos-unstable";
    };

    nixpkgs-qemu7 = {
      url = "github:NixOS/nixpkgs/444208798aefb1787b8ef0851f5c3d49113bd014";
    };

    rust-overlay = {
      url = "github:oxalica/rust-overlay/1775eafa1879ac098ee436849bc9c3d963206f89";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      perSystem =
        { system, pkgs, ... }:
        {
          _module.args.pkgs = (import inputs.nixpkgs) {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };

          devShells.default = pkgs.mkShellNoCC {
            packages = [
              ((pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
                targets = [ "riscv64gc-unknown-none-elf" ];
              })
              inputs.nixpkgs-qemu7.legacyPackages.${system}.qemu
              pkgs.cargo-binutils
              pkgs.gdb
              pkgs.nixfmt
              pkgs.nixfmt-tree
              pkgs.python3
            ];
          };
        };
    };
}
