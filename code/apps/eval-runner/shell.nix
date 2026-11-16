{ pkgs, rbdc }:
pkgs.mkShell {
  packages = [ pkgs.python3 pkgs.git pkgs.nix rbdc ];
}
