# Based on getworkloop/wlp-mono experimental/nix-smolvm/image.nix's
# dockerTools composition, simplified for synthetic workloads. See NOTICE.
{ pkgs }:
pkgs.dockerTools.buildLayeredImage {
  name = "rbdc-evals";
  tag = "synthetic-v1";
  architecture = "arm64";
  contents = [ pkgs.bashInteractive pkgs.coreutils pkgs.python3
    (pkgs.runCommand "rbdc-guest" {} ''
      install -Dm644 ${./guest.py} $out/share/rbdc/guest.py
    '') ];
  extraCommands = ''
    mkdir -p tmp
    chmod 1777 tmp
  '';
  config = {
    Cmd = [ "/bin/sleep" "infinity" ];
    Env = [ "PATH=/bin" ];
    WorkingDir = "/tmp";
  };
}
