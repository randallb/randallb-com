{ pkgs, smolvm }:
let
  codexPayload = if pkgs.stdenv.hostPlatform.system == "aarch64-darwin"
    then import ./codex-payload.nix { inherit pkgs; } else null;
  source = pkgs.lib.fileset.toSource {
    root = ./.;
    fileset = pkgs.lib.fileset.unions [ ./cli.py ./lifecycle.py ./builder.py ./tart_backend.py ./codex_payload.py ./codex-pin.json ];
  };
in pkgs.writeShellApplication {
  name = "rbdc";
  runtimeInputs = [ pkgs.python3 pkgs.git ] ++ pkgs.lib.optional (smolvm != null) smolvm;
  text = ''
    ${pkgs.lib.optionalString (codexPayload != null) ''export RBDC_CODEX_PAYLOAD="''${RBDC_CODEX_PAYLOAD:-${codexPayload}}"''}
    exec python3 ${source}/cli.py "$@"
  '';
}
