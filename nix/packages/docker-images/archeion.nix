{
  pkgs,
  common,
  archeion,
  admin,
}:
let
  inherit (common) gitVersion;
in
# The minimal archeion image: the binary + the CA trust store + the `kallip-admin`
# CLI for in-container operator tasks. No shell toolset; archeion reads everything
# else from its env at runtime. The compose service (compose/prod/polis.nix)
# supplies the command + environment.
pkgs.dockerTools.buildImage {
  name = "kallip-archeion";
  tag = gitVersion;
  copyToRoot = [
    archeion
    admin
    pkgs.cacert
  ];
  config = {
    Cmd = [ "${archeion}/bin/kallip-archeion" ];
    Env = [ "PATH=${admin}/bin" ];
    ExposedPorts = {
      "7100/tcp" = { };
    };
  };
}
