{
  description = "randallb.com development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    # Match the verified Deno pin used by wlp-mono.
    nixpkgs-deno.url = "github:NixOS/nixpkgs/6003a8c9f2b2e1f822758ed29a517049348325e3";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, nixpkgs-deno, rust-overlay, ... }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "aarch64-linux" "x86_64-linux" ];
    in {
      devShells = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ (import rust-overlay) ];
          };
          denoPkgs = import nixpkgs-deno { inherit system; };
          deno = assert denoPkgs.lib.versionAtLeast denoPkgs.deno.version "2.9.7";
            denoPkgs.deno;
          rust = pkgs.rust-bin.fromRustupToolchainFile ./code/rust-toolchain.toml;
        in {
          default = pkgs.mkShell {
            packages = [
              rust
              deno
              pkgs.nodejs_24
              pkgs.python3
              pkgs.pkg-config
              pkgs.git
              pkgs.ripgrep
              pkgs.jq
              pkgs.tmux
            ];
          };
        });
    };
}
