{ pkgs, smolvm }:
let
  source = pkgs.lib.fileset.toSource {
    root = ./.;
    fileset = pkgs.lib.fileset.unions [ ./cli.py ./lifecycle.py ./builder.py ];
  };
in pkgs.writeShellApplication {
  name = "rbdc";
  runtimeInputs = [ pkgs.python3 pkgs.git ] ++ pkgs.lib.optional (smolvm != null) smolvm;
  text = ''exec python3 ${source}/cli.py "$@"'';
}
