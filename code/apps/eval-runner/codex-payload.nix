# Verified upstream macOS executable; guests need no Nix installation or credentials.
{ pkgs }:
let
  pin = builtins.fromJSON (builtins.readFile ./codex-pin.json);
  archive = pkgs.fetchurl { inherit (pin) url sha256; };
  hostArchive = pkgs.fetchurl { inherit (pin.code_mode_host) url sha256; };
in pkgs.runCommand "rbdc-codex-payload-${pin.version}" {} ''
  mkdir -p $out
  cp ${archive} $out/codex.tar.gz
  cp ${hostArchive} $out/code-mode-host.tar.gz
  cp ${./codex-pin.json} $out/manifest.json
''
