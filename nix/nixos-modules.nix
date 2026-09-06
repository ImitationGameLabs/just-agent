# NixOS module for the kallipai platform-hosting form, phase one: the
# daemon as a system service plus the declared tagma users it launches
# instances as.
#
# Identity is externalized by design: this module declares the users and
# the daemon only consumes passwd entries — it never creates users or
# touches the uid ledger. The three directories split the daemon's world
# (config under /etc, runtime under /run, record area under /var/lib),
# and linger gives every declared user the standard logind runtime
# directory (/run/user/<uid>), the same semantics a human user gets.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.kallipai.daemon;
in
{
  options.services.kallipai.daemon = {
    enable = lib.mkEnableOption "the kallipai daemon as a system service";

    package = lib.mkOption {
      type = lib.types.package;
      description = ''
        The kallipai daemon package. It must ship `kallip-daemon`
        together with `kallip-daemon-spawn`: the daemon resolves the
        helper as a sibling of its own binary first.
      '';
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "kallip";
      description = ''
        The group gating access to the control socket (0660
        root:<group>) and, as @<group>, to the nix daemon: one
        declaration primitive shared by both access paths. Deliberately
        not the generic `users` group — that would hand the control
        socket and nix to every human account on the host.
      '';
    };

    tagmaUsers = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = ''
        Pre-declared system users that tagma instances may run as (the
        dedicated-user form of `kallipctl spawn --user`). Each gets a
        home directory and linger, so a spawned instance finds the
        standard XDG runtime directory and can drive flake + direnv
        natively.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    # One group per declared user plus the shared access gate group
    # (socket 0660 + nix @group below). Primary groups are per user:
    # a shared primary group would let the tagma users read each
    # other's homes, undoing the uid isolation the dedicated-user
    # form exists for. The gate group rides as an extra group —
    # kernel group checks accept supplementary membership, so the
    # socket and nix gates work unchanged.
    users.groups = lib.listToAttrs (
      map (name: lib.nameValuePair name { }) (cfg.tagmaUsers ++ [ cfg.group ])
    );

    users.users = lib.listToAttrs (
      map (
        name:
        lib.nameValuePair name {
          isSystemUser = true;
          group = name;
          extraGroups = [ cfg.group ];
          home = "/home/${name}";
          createHome = true;
          # logind pre-creates /run/user/<uid> at boot: the spawned
          # instance's XDG_RUNTIME_DIR, no per-instance setup.
          linger = true;
          shell = pkgs.bashInteractive;
        }
      ) cfg.tagmaUsers
    );

    # The upstream default is [ "*" ] — everyone. Concatenating with a
    # default would defeat the wiring, so this module owns the list:
    # the declared users (by name) and the tagma group. This replaces
    # anything an operator configured elsewhere — extra entries belong
    # in tagmaUsers, not in a competing declaration.
    nix.settings.allowed-users = lib.mkForce (lib.unique (cfg.tagmaUsers ++ [ "@${cfg.group}" ]));

    systemd.services.kallip-daemon = {
      description = "kallipai instance daemon";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      environment = {
        KALLIP_DAEMON_SOCKET = "/run/kallipai/daemon.sock";
        KALLIP_DAEMON_RECORD_DIR = "/var/lib/kallipai/daemon/instances";
        KALLIP_DAEMON_SOCKET_GROUP = cfg.group;
        # NixOS has no /bin/bash; the login-environment harvest needs a
        # fixed administrative bash, never the caller's shell.
        KALLIP_HARVEST_BASH = "${pkgs.bash}/bin/bash";
      };

      serviceConfig = {
        ExecStart = "${cfg.package}/bin/kallip-daemon";
        # Dedicated-user launches fork+setuid to arbitrary declared
        # users: that needs real root, not a capability subset.
        User = "root";
        StateDirectory = "kallipai/daemon";
        # Records enumerate the slugs, uids, and workspaces on the
        # host; 0700 keeps that a root-and-daemon-only view (clients
        # read through the socket, not the files).
        StateDirectoryMode = "0700";
        RuntimeDirectory = "kallipai";
        ConfigurationDirectory = "kallipai";
        Restart = "on-failure";
      };
    };
  };
}
