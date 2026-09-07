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
#
# The polis section (services.kallipai.polis) brings up the four platform
# services -- archeion, lesche, files, instances -- on one switch:
# localhost-only listeners behind the host's reverse proxy, one shared
# PostgreSQL for the three stateful services over unix-socket peer auth,
# and one self-managed secret: the platform-internal token is generated
# by the archeion into its own state directory on first boot and only
# read afterwards. Operator token knobs stay paths (pin an admin token,
# or let the unit mint a short-lived one); no secret hits the store.
{ packages }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.kallipai.daemon;
  polisCfg = config.services.kallipai.polis;
  webCfg = config.services.kallipai.web;

  # The flake's own build for this host: the package options default to
  # it, so enabling a service needs no package reference; setting an
  # option explicitly pins a specific build.
  hostPackages = packages.${pkgs.stdenv.hostPlatform.system};

  # Single source of truth for the polis port defaults: the option
  # defaults and the direct-connect warning both read this one
  # binding, so a default change happens here and nowhere else. The
  # web UI's compiled-in copies of these numbers are reconciled by
  # the config.ports schema work (handover note), not here.
  defaultPolisPorts = {
    archeion = 7100;
    lesche = 7200;
    files = 7400;
    instances = 7300;
  };

  # The daemon's control socket: the daemon unit sets it and the
  # instances proxy reads it back, so the path lives in one binding.
  daemonSocket = "/run/kallipai/daemon.sock";

  # Inject an environment key only when the option carries a value: null
  # means "let the service's own default govern", keeping the code default
  # the single source of truth -- a mirrored default here would drift from
  # it. Bools render via boolToString because Nix's toString gives "1".
  envOpt =
    name: value:
    lib.optionalAttrs (value != null) {
      ${name} = if lib.isBool value then lib.boolToString value else toString value;
    };
  # The polis listeners' localhost ports, configured per service under
  # services.kallipai.polis.ports and shared by the env and caddy routes.
  polisPorts = polisCfg.ports;
in
{
  options.services.kallipai = {
    daemon = {
      enable = lib.mkEnableOption "the kallipai daemon as a system service";

      package = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-daemon;
        description = ''
          The kallipai daemon package, defaulting to this flake's build
          (set it explicitly to pin a specific version). It must ship
          `kallip-daemon` together with `kallip-daemon-spawn`: the
          daemon resolves the helper as a sibling of its own binary
          first.
        '';
      };

      group = lib.mkOption {
        type = lib.types.str;
        default = "kallipai-polis";
        description = ''
          The group gating @<group> access to the nix daemon and read
          access to the archeion's provisioned internal-token file: one
          declaration primitive shared by every platform-internal access
          path. The daemon's control socket is gated separately, by the
          dedicated kallipai-daemon group, so socket admission does
          not ride the platform gate. Deliberately not the generic
          `users` group -- that would hand access to every account on
          the host.
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
    polis = {
      enable = lib.mkEnableOption "the polis platform services (archeion, lesche, files, instances) as system services";

      archeionPackage = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-archeion;
        description = "The kallip-archeion package; defaults to this flake's build.";
      };
      leschePackage = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-lesche;
        description = "The kallip-lesche package; defaults to this flake's build.";
      };
      filesPackage = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-files;
        description = "The kallip-files package; defaults to this flake's build.";
      };
      instancesPackage = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-instances;
        description = "The kallip-instances package; defaults to this flake's build.";
      };

      adminTokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a root-only EnvironmentFile defining
          KALLIP_ARCHEION_ADMIN_TOKEN (the provisioning authority and the
          admin-login exchange), plus any archeion-only extra keys --
          notably the OAuth client secrets
          (KALLIP_ARCHEION_OAUTH_GITHUB_CLIENT_SECRET,
          KALLIP_ARCHEION_OAUTH_GOOGLE_CLIENT_SECRET; a provider enables
          only when its id option and secret are both set).
          This is the pin form: a stable credential the operator owns, so
          it lives under /etc like other admin assets. Default null means
          the archeion mints the token itself into its runtime directory
          (/run/kallipai/archeion/admin-token.env, 0600) on every start --
          a short-lived bootstrap credential, rewritten on each restart
          and valid until the next one, never written to the
          journal. Pin the file to keep a stable token; leave it unset to
          accept a short-lived one.
        '';
      };
      notifyTokenFile = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = ''
          Path to a root-only EnvironmentFile carrying the files-to-lesche
          event-push secret; the file must define two keys with the same value:
          KALLIP_LESCHE_INTERNAL_TOKEN and KALLIP_FILES_NOTIFY_TOKEN. Default
          null: the lesche leaves its internal surface unmounted and the push
          stays disabled (the safe standalone posture).
        '';
      };

      ports = {
        archeion = lib.mkOption {
          type = lib.types.port;
          default = defaultPolisPorts.archeion;
          description = ''
            Listening port of the archeion service. Must be 1024-65535;
            pick a port outside the system's ephemeral range and unused by
            other services on this host — a conflict surfaces at service
            start as an address-in-use error.
          '';
        };
        lesche = lib.mkOption {
          type = lib.types.port;
          default = defaultPolisPorts.lesche;
          description = ''
            Listening port of the lesche service. Must be 1024-65535;
            pick a port outside the system's ephemeral range and unused by
            other services on this host — a conflict surfaces at service
            start as an address-in-use error.
          '';
        };
        files = lib.mkOption {
          type = lib.types.port;
          default = defaultPolisPorts.files;
          description = ''
            Listening port of the files service. Must be 1024-65535;
            pick a port outside the system's ephemeral range and unused by
            other services on this host — a conflict surfaces at service
            start as an address-in-use error.
          '';
        };
        instances = lib.mkOption {
          type = lib.types.port;
          default = defaultPolisPorts.instances;
          description = ''
            Listening port of the instances service. Must be 1024-65535;
            pick a port outside the system's ephemeral range and unused by
            other services on this host — a conflict surfaces at service
            start as an address-in-use error.
          '';
        };
      };

      proxy = {
        enable = lib.mkEnableOption "caddy virtual hosts exposing the polis services on their subdomains";

        domain = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            The deployment domain the subdomains hang off: the virtual hosts
            archeion.<domain>, lesche.<domain> and files.<domain> route to
            the localhost listeners. Required when the proxy is enabled.
          '';
        };

        acmeEmail = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = ''
            ACME account email, forwarded to services.caddy.email. Default
            null: Caddy then registers with Let's Encrypt without a
            recovery address, which small deployments accept.
          '';
        };
      };

      archeion = {
        webauthnRpId = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "WebAuthn relying-party id (the registrable domain passkeys bind to); changing it invalidates every bound passkey.";
        };
        webauthnRpOrigin = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "WebAuthn relying-party origin; must have the rp id as its effective domain.";
        };
        webauthnRpName = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Human-readable WebAuthn relying-party name shown in the browser prompt.";
        };
        webauthnAllowAnyPort = lib.mkOption {
          type = lib.types.nullOr lib.types.bool;
          default = null;
          description = "Allow non-standard ports on the WebAuthn origin (local HTTP dev only).";
        };
        sessionTtlSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Session cookie lifetime in seconds.";
        };
        cookieSecure = lib.mkOption {
          type = lib.types.nullOr lib.types.bool;
          default = null;
          description = "Mark the session cookie Secure (disable only for plain-HTTP dev).";
        };
        cookieDomain = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Cookie Domain attribute; set to the parent domain when the lesche shares a subdomain of it.";
        };
        authRateCapacity = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Per-IP token-bucket capacity guarding /v1/auth/*.";
        };
        authRateRefillPerSec = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Per-IP auth bucket refill rate, requests per second.";
        };
        pairRateCapacity = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Shared pairing-bucket capacity: the real brute-force bound on the pairing code (per-IP limiting is bypassable by source-IP diversity).";
        };
        pairRateRefillPerSec = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Shared pairing-bucket refill rate, requests per second.";
        };
        trustedProxies = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated CIDRs trusted to set X-Forwarded-For; the code default already trusts loopback for the same-box proxy.";
        };
        maxBodySizeKb = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Max HTTP request body size in kilobytes (0 = axum default).";
        };
        corsOrigins = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated CORS allow-list origins; never a wildcard on a public deploy.";
        };
        enrollmentCodeTtlSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Single-use enrollment-code lifetime in seconds.";
        };
        signupEnabled = lib.mkOption {
          type = lib.types.nullOr lib.types.bool;
          default = null;
          description = "Whether open signup is allowed (the incident-time kill switch).";
        };
        oauthRedirectBase = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Web origin the OAuth flow redirects into; required only when an OAuth provider is configured.";
        };
        oauthGithubClientId = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "GitHub OAuth client id; the provider enables only when id and secret are both present. Client secrets go in the token files, never here.";
        };
        oauthGoogleClientId = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Google OAuth client id; same enable rule and secret rule as GitHub.";
        };
        adminUserLogin = lib.mkOption {
          type = lib.types.nullOr lib.types.bool;
          default = null;
          description = "Mount POST /v1/auth/admin-login (admin token exchanged for a local session). When on, a hand-set admin token shorter than 32 chars fails at boot.";
        };
      };

      lesche = {
        proofSkewSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.int;
          default = null;
          description = "Acceptable clock skew (both directions) on a tunnel reconnect proof, in seconds.";
        };
        keyExchangeTimeoutSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "How long a synchronous key exchange waits for the tagma before failing with 504.";
        };
        maxBodySizeKb = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Max HTTP request body size in kilobytes (0 = axum default).";
        };
        corsOrigins = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated CORS allow-list origins.";
        };
      };

      files = {
        maxBodySizeMb = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Maximum accepted upload body in megabytes; larger streams get 413.";
        };
        corsOrigins = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated CORS allow-list origins.";
        };
        degrade = lib.mkOption {
          type = lib.types.nullOr (
            lib.types.enum [
              "closed"
              "soft"
            ]
          );
          default = null;
          description = "Archeion degrade posture: closed fails authorization with 503 when the registry is unreachable, soft denies with 403 from an empty fact set.";
        };
        gcIntervalSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Delay between GC passes, in seconds.";
        };
        gcGraceSecs = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "How long a freed zero-refcount row must age before the GC reclaims it.";
        };
        gcBatch = lib.mkOption {
          type = lib.types.nullOr lib.types.ints.unsigned;
          default = null;
          description = "Maximum catalog rows reclaimed per GC pass.";
        };
      };
      instances = {
        corsOrigins = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated CORS allow-list origins; the web bundle calls this service cross-origin (web.<domain> to instances.<domain>), so a proxied deployment serving the web app lists that origin here.";
        };
        allowedHosts = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Comma-separated extra Host values the host guard admits (IP literals and localhost always pass). The proxied shape receives instances.<domain>; a direct-connect LAN shape names the domain browsers use.";
        };
      };
    };

    web = {
      enable = lib.mkEnableOption "the kallip-web static site behind caddy";

      package = lib.mkOption {
        type = lib.types.package;
        default = hostPackages.kallip-web-dist;
        description = ''
          The kallip-web bundle (this flake's kallip-web-dist build);
          set it explicitly to pin a specific build, as with the polis
          packages.
        '';
      };

      domain = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          The deployment domain: the virtual host web.<domain> serves the
          SPA with a fallback to its index.html so client-side routes
          resolve. Required when the module is enabled.
        '';
      };

      runtimeConfig = lib.mkOption {
        type = lib.types.attrsOf lib.types.anything;
        default = {
          offlineLogin = true;
        };
        description = ''
          Payload for the web app's runtime config (/config.js), serialized
          as JSON into a window.KALLIP_CONFIG assignment. Keys mirror the
          app's Window.KALLIP_CONFIG type: domain, tlsOff, offlineLogin,
          services. The default keeps the self-hosted default behavior:
          the operator-key login branch shows because the operator is the
          owner. Set offlineLogin = false to hide it (a cloud-facing
          deployment), or add domain/tlsOff/services to pin values the
          app would otherwise derive from the browser location.
          The file is served publicly by Caddy, so anything placed here is
          readable by anyone who can reach the site — keep it to values the
          browser is meant to see; secrets belong in environment files or
          credential stores, never in this option.
        '';
      };

      acmeEmail = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = ''
          ACME account email, forwarded to services.caddy.email. Default
          null: Caddy then registers with Let's Encrypt without a
          recovery address, which small deployments accept.
        '';
      };
    };
  };
  config = lib.mkMerge [
    {
      assertions = [
        {
          assertion = polisCfg.proxy.enable -> polisCfg.proxy.domain != null;
          message = "services.kallipai.polis.proxy.domain must be set when the proxy is enabled.";
        }
        {
          assertion = webCfg.enable -> webCfg.domain != null;
          message = "services.kallipai.web.domain must be set when services.kallipai.web is enabled.";
        }
      ]
      ++ lib.optionals polisCfg.enable (
        # The four listeners must not collide: a shared port is always a
        # misconfiguration, so fail at eval time with the pair and value.
        (map
          (pair: {
            assertion = polisCfg.ports.${builtins.elemAt pair 0} != polisCfg.ports.${builtins.elemAt pair 1};
            message = "services.kallipai.polis.ports.${builtins.elemAt pair 0} and services.kallipai.polis.ports.${builtins.elemAt pair 1} are both ${
              toString polisCfg.ports.${builtins.elemAt pair 0}
            }; the four polis listeners must use distinct ports — set one of them to a free port.";
          })
          [
            [
              "archeion"
              "lesche"
            ]
            [
              "lesche"
              "files"
            ]
            [
              "archeion"
              "files"
            ]
            [
              "archeion"
              "instances"
            ]
            [
              "lesche"
              "instances"
            ]
            [
              "files"
              "instances"
            ]
          ]
        )
        ++ (map
          (svc: {
            assertion = polisCfg.ports.${svc} >= 1024 && polisCfg.ports.${svc} <= 65535;
            message = "services.kallipai.polis.ports.${svc} is ${toString polisCfg.ports.${svc}}; it must be 1024-65535 — the polis services do not hold CAP_NET_BIND_SERVICE, so a lower port cannot be bound. Set it to a port in that range.";
          })
          [
            "archeion"
            "lesche"
            "files"
            "instances"
          ]
        )
      );
    }
    (lib.mkIf cfg.enable {
      # One group per declared user plus the shared access gate group
      # (nix @group below, and the polis token gate in the polis
      # block). Primary groups are per user: a shared primary group
      # would let the tagma users read each other's homes, undoing the
      # uid isolation the dedicated-user form exists for. The gate
      # group rides as an extra group — kernel group checks accept
      # supplementary membership, so the nix gate works unchanged. The
      # daemon's control socket is gated separately (kallipai-daemon
      # below): that group admits only actual socket consumers.
      users.groups = lib.listToAttrs (
        map (name: lib.nameValuePair name { }) (
          cfg.tagmaUsers
          ++ [
            cfg.group
            "kallipai-daemon"
          ]
        )
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
          KALLIP_DAEMON_SOCKET = daemonSocket;
          KALLIP_DAEMON_RECORD_DIR = "/var/lib/kallipai/daemon/instances";
          # Dedicated socket gate, not the shared platform gate: the group
          # admits only actual socket consumers. The tagma users keep the
          # nix gate (which the platform gate group carries) untouched.
          KALLIP_DAEMON_SOCKET_GROUP = "kallipai-daemon";
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
    })
    (lib.mkIf polisCfg.enable {
      # Dedicated users, per-user primary groups. The stateful services
      # stay out of the daemon-socket gate group (kallipai-daemon):
      # none of them consumes the socket. They ride the platform gate
      # group only where the archeion token requires it.
      users.groups = builtins.listToAttrs (
        map (name: lib.nameValuePair name { }) [
          "kallip-archeion"
          "kallip-lesche"
          "kallip-files"
          "kallip-instances"
        ]
      );
      users.users =
        builtins.listToAttrs (
          map
            (
              name:
              lib.nameValuePair name {
                isSystemUser = true;
                group = name;
                # Consumers ride the platform gate group for the archeion's
                # 0640 internal-token file. The daemon socket is gated
                # separately (kallipai-daemon) and none of these users
                # joins it. The archeion keeps its primary group private
                # and joins the gate only inside its unit, where it needs
                # membership to chgrp the token file.
                extraGroups = lib.optionals (name != "kallip-archeion") [ cfg.group ];
              }
            )
            [
              "kallip-archeion"
              "kallip-lesche"
              "kallip-files"
            ]
        )
        // {
          # Explicit for symmetry with the map above; see its comment for
          # the gate-group rationale.
          kallip-instances = {
            isSystemUser = true;
            group = "kallip-instances";
            # The one polis user that dials the daemon's control socket:
            # platform gate for the token, socket gate for the proxy.
            extraGroups = [
              cfg.group
              "kallipai-daemon"
            ];
          };
        };

      # One shared PostgreSQL over the unix socket: each service connects as
      # its own system user (peer auth), so no password exists to leak and
      # no TCP surface exists. Database name = role name = OS user name (one
      # name, hyphenated: peer maps the OS user to the role, and
      # ensureDBOwnership runs ALTER DATABASE on the role name). Peer
      # authentication is pinned explicitly so it does not depend on the
      # channel's implicit default.
      services.postgresql = {
        enable = lib.mkDefault true;
        authentication = lib.mkDefault "local all all peer";
        ensureDatabases = lib.mkDefault [
          "kallip-archeion"
          "kallip-lesche"
          "kallip-files"
        ];
        ensureUsers = lib.mkDefault [
          {
            name = "kallip-archeion";
            ensureDBOwnership = true;
          }
          {
            name = "kallip-lesche";
            ensureDBOwnership = true;
          }
          {
            name = "kallip-files";
            ensureDBOwnership = true;
          }
        ];
      };

      systemd.services = {
        kallip-archeion = {
          description = "kallipai archeion control plane";
          wantedBy = [ "multi-user.target" ];
          # The archeion retries its DB connect with a capped backoff, so
          # wants (not requires): a slow postgres must not tear it down.
          after = [
            "network.target"
            "postgresql.service"
          ];
          wants = [ "postgresql.service" ];
          environment = {
            KALLIP_ARCHEION_ADDR = "127.0.0.1:${toString polisPorts.archeion}";
            KALLIP_ARCHEION_DATABASE_URL = "postgresql:///kallip-archeion?host=/run/postgresql";
            KALLIP_ARCHEION_LOG_DIR = "/var/log/kallipai/archeion";
            # The internal token is state the archeion owns: generated into
            # its state dir on first boot, read (never rewritten) after.
            KALLIP_ARCHEION_INTERNAL_TOKEN_FILE = "/var/lib/kallipai/archeion/internal-token";
            KALLIP_ARCHEION_INTERNAL_TOKEN_GROUP = cfg.group;
            # Runtime state: the admin bootstrap token, rewritten every start.
            KALLIP_ARCHEION_ADMIN_TOKEN_OUT_FILE = "/run/kallipai/archeion/admin-token.env";
          }
          // envOpt "KALLIP_ARCHEION_WEBAUTHN_RP_ID" polisCfg.archeion.webauthnRpId
          // envOpt "KALLIP_ARCHEION_WEBAUTHN_RP_ORIGIN" polisCfg.archeion.webauthnRpOrigin
          // envOpt "KALLIP_ARCHEION_WEBAUTHN_RP_NAME" polisCfg.archeion.webauthnRpName
          // envOpt "KALLIP_ARCHEION_WEBAUTHN_ALLOW_ANY_PORT" polisCfg.archeion.webauthnAllowAnyPort
          // envOpt "KALLIP_ARCHEION_SESSION_TTL_SECS" polisCfg.archeion.sessionTtlSecs
          // envOpt "KALLIP_ARCHEION_COOKIE_SECURE" polisCfg.archeion.cookieSecure
          // envOpt "KALLIP_ARCHEION_SESSION_COOKIE_DOMAIN" polisCfg.archeion.cookieDomain
          // envOpt "KALLIP_ARCHEION_AUTH_RATE_CAPACITY" polisCfg.archeion.authRateCapacity
          // envOpt "KALLIP_ARCHEION_AUTH_RATE_REFILL_PER_SEC" polisCfg.archeion.authRateRefillPerSec
          // envOpt "KALLIP_ARCHEION_PAIR_RATE_CAPACITY" polisCfg.archeion.pairRateCapacity
          // envOpt "KALLIP_ARCHEION_PAIR_RATE_REFILL_PER_SEC" polisCfg.archeion.pairRateRefillPerSec
          // envOpt "KALLIP_ARCHEION_TRUSTED_PROXIES" polisCfg.archeion.trustedProxies
          // envOpt "KALLIP_ARCHEION_MAX_BODY_SIZE_KB" polisCfg.archeion.maxBodySizeKb
          // envOpt "KALLIP_ARCHEION_CORS_ORIGINS" polisCfg.archeion.corsOrigins
          // envOpt "KALLIP_ARCHEION_ENROLLMENT_CODE_TTL_SECS" polisCfg.archeion.enrollmentCodeTtlSecs
          // envOpt "KALLIP_ARCHEION_SIGNUP_ENABLED" polisCfg.archeion.signupEnabled
          // envOpt "KALLIP_ARCHEION_OAUTH_REDIRECT_BASE" polisCfg.archeion.oauthRedirectBase
          // envOpt "KALLIP_ARCHEION_OAUTH_GITHUB_CLIENT_ID" polisCfg.archeion.oauthGithubClientId
          // envOpt "KALLIP_ARCHEION_OAUTH_GOOGLE_CLIENT_ID" polisCfg.archeion.oauthGoogleClientId
          // envOpt "KALLIP_ARCHEION_ADMIN_USER_LOGIN" polisCfg.archeion.adminUserLogin;
          serviceConfig = {
            ExecStart = "${polisCfg.archeionPackage}/bin/kallip-archeion";
            User = "kallip-archeion";
            Group = "kallip-archeion";
            # Gate-group membership lives on the unit (not the user): only
            # this unit chgrps the provisioned token file to the gate group.
            SupplementaryGroups = [ cfg.group ];
            StateDirectory = "kallipai/archeion";
            # Runtime sibling: the admin bootstrap token lives here --
            # rewritten on every start, gone when the unit stops.
            RuntimeDirectory = "kallipai/archeion";
            LogsDirectory = "kallipai/archeion";
            # 0750 (not 0700): gate-group consumers traverse this directory
            # to read the provisioned internal token.
            StateDirectoryMode = "0750";
            RuntimeDirectoryMode = "0700";
            LogsDirectoryMode = "0750";
            Restart = "on-failure";
            EnvironmentFile = lib.optional (polisCfg.adminTokenFile != null) (toString polisCfg.adminTokenFile);
          };
        };

        kallip-lesche = {
          description = "kallipai lesche data-plane relay";
          wantedBy = [ "multi-user.target" ];
          # Hard dependency: the lesche reads the archeion's provisioned
          # internal-token file at boot, so the archeion must be up (and
          # the file present) first. Operator-accepted cost: an archeion
          # stop cascades here.
          after = [
            "network.target"
            "kallip-archeion.service"
          ];
          requires = [ "kallip-archeion.service" ];
          environment = {
            KALLIP_LESCHE_ADDR = "127.0.0.1:${toString polisPorts.lesche}";
            KALLIP_LESCHE_ARCHEION_INTERNAL_URL = "http://127.0.0.1:${toString polisPorts.archeion}";
            KALLIP_LESCHE_DATABASE_URL = "postgresql:///kallip-lesche?host=/run/postgresql";
            KALLIP_LESCHE_LOG_DIR = "/var/log/kallipai/lesche";
            KALLIP_POLIS_INTERNAL_TOKEN_FILE = "/var/lib/kallipai/archeion/internal-token";
          }
          // envOpt "KALLIP_LESCHE_PROOF_SKEW_SECS" polisCfg.lesche.proofSkewSecs
          // envOpt "KALLIP_LESCHE_KEY_EXCHANGE_TIMEOUT_SECS" polisCfg.lesche.keyExchangeTimeoutSecs
          // envOpt "KALLIP_LESCHE_MAX_BODY_SIZE_KB" polisCfg.lesche.maxBodySizeKb
          // envOpt "KALLIP_LESCHE_CORS_ORIGINS" polisCfg.lesche.corsOrigins;
          serviceConfig = {
            ExecStart = "${polisCfg.leschePackage}/bin/kallip-lesche";
            User = "kallip-lesche";
            Group = "kallip-lesche";
            SupplementaryGroups = [ cfg.group ];
            StateDirectory = "kallipai/lesche";
            LogsDirectory = "kallipai/lesche";
            StateDirectoryMode = "0700";
            LogsDirectoryMode = "0750";
            Restart = "on-failure";
            EnvironmentFile = lib.optional (polisCfg.notifyTokenFile != null) (
              toString polisCfg.notifyTokenFile
            );
          };
        };

        kallip-files = {
          description = "kallipai files content-transfer service";
          wantedBy = [ "multi-user.target" ];
          after = [
            "network.target"
            "kallip-archeion.service"
          ];
          # Hard dependency: the internal-token file must be provisioned first.
          requires = [ "kallip-archeion.service" ];
          environment = {
            KALLIP_FILES_ADDR = "127.0.0.1:${toString polisPorts.files}";
            KALLIP_FILES_ARCHEION_INTERNAL_URL = "http://127.0.0.1:${toString polisPorts.archeion}";
            KALLIP_FILES_DATABASE_URL = "postgresql:///kallip-files?host=/run/postgresql";
            KALLIP_FILES_LOG_DIR = "/var/log/kallipai/files";
            KALLIP_FILES_BLOB_ROOT = "/var/lib/kallipai/files/blobs";
            KALLIP_FILES_NOTIFY_URL = "http://127.0.0.1:${toString polisPorts.lesche}";
            KALLIP_POLIS_INTERNAL_TOKEN_FILE = "/var/lib/kallipai/archeion/internal-token";
          }
          // envOpt "KALLIP_FILES_MAX_BODY_SIZE_MB" polisCfg.files.maxBodySizeMb
          // envOpt "KALLIP_FILES_CORS_ORIGINS" polisCfg.files.corsOrigins
          // envOpt "KALLIP_FILES_DEGRADE" polisCfg.files.degrade
          // envOpt "KALLIP_FILES_GC_INTERVAL_SECS" polisCfg.files.gcIntervalSecs
          // envOpt "KALLIP_FILES_GC_GRACE_SECS" polisCfg.files.gcGraceSecs
          // envOpt "KALLIP_FILES_GC_BATCH" polisCfg.files.gcBatch;
          serviceConfig = {
            ExecStart = "${polisCfg.filesPackage}/bin/kallip-files";
            User = "kallip-files";
            Group = "kallip-files";
            StateDirectory = "kallipai/files";
            LogsDirectory = "kallipai/files";
            StateDirectoryMode = "0700";
            LogsDirectoryMode = "0750";
            Restart = "on-failure";
            EnvironmentFile = lib.optional (polisCfg.notifyTokenFile != null) (
              toString polisCfg.notifyTokenFile
            );
          };
        };
        kallip-instances = {
          description = "kallipai instances management proxy";
          wantedBy = [ "multi-user.target" ];
          # Two dependencies, two postures: the daemon is soft (a restart
          # answers 503 daemon_unreachable, not a failed unit); the
          # archeion is hard -- its provisioned internal-token file is read
          # at boot, so it must be up first.
          after = [
            "network.target"
            "kallip-daemon.service"
            "kallip-archeion.service"
          ];
          wants = [ "kallip-daemon.service" ];
          requires = [ "kallip-archeion.service" ];
          environment = {
            KALLIP_INSTANCES_ADDR = "127.0.0.1:${toString polisPorts.instances}";
            KALLIP_DAEMON_SOCKET = daemonSocket;
            KALLIP_INSTANCES_ARCHEION_URL = "http://127.0.0.1:${toString polisPorts.archeion}";
            KALLIP_POLIS_INTERNAL_TOKEN_FILE = "/var/lib/kallipai/archeion/internal-token";
            # Relay defaults must follow the configured polis ports, not the
            # binary's compiled-in 7100/7200 (the files NOTIFY_URL pattern).
            KALLIP_INSTANCES_RELAY_ARCHEION_URL = "http://127.0.0.1:${toString polisPorts.archeion}";
            KALLIP_INSTANCES_RELAY_LESCHE_URL = "http://127.0.0.1:${toString polisPorts.lesche}";
          }
          // envOpt "KALLIP_INSTANCES_CORS_ORIGINS" polisCfg.instances.corsOrigins
          // envOpt "KALLIP_INSTANCES_ALLOWED_HOSTS" polisCfg.instances.allowedHosts;
          serviceConfig = {
            ExecStart = "${polisCfg.instancesPackage}/bin/kallip-instances";
            User = "kallip-instances";
            Group = "kallip-instances";
            # A pure UDS proxy: no state or log directory of its own --
            # the daemon owns both sides of that split.
            Restart = "on-failure";
          };
        };
      };
    })
    (lib.mkIf (polisCfg.proxy.enable && polisCfg.proxy.domain != null) {
      # The public edge: bring up caddy and route the four polis
      # subdomains to their localhost listeners, the NixOS form of the
      # dev Caddyfile's host routing. The lesche route flushes
      # immediately (the event stream must not buffer); the other three
      # are plain request/response.
      services.caddy = {
        enable = true;
        email = lib.mkIf (polisCfg.proxy.acmeEmail != null) polisCfg.proxy.acmeEmail;
        virtualHosts = {
          "archeion.${polisCfg.proxy.domain}".extraConfig = ''
            reverse_proxy 127.0.0.1:${toString polisPorts.archeion}
          '';
          "lesche.${polisCfg.proxy.domain}".extraConfig = ''
            reverse_proxy 127.0.0.1:${toString polisPorts.lesche} {
              flush_interval -1
            }
          '';
          "files.${polisCfg.proxy.domain}".extraConfig = ''
            reverse_proxy 127.0.0.1:${toString polisPorts.files}
          '';
          "instances.${polisCfg.proxy.domain}".extraConfig = ''
            reverse_proxy 127.0.0.1:${toString polisPorts.instances}
          '';
        };
      };
    })
    (lib.mkIf (webCfg.enable && webCfg.domain != null) {
      # The SPA's virtual host: serve the bundle's files, falling back
      # to index.html so client-side routes resolve on hard reload. The
      # runtimeConfig payload (offline-login branch on by default) is
      # served instead of the bundle's empty config.js shell (handle
      # blocks are mutually exclusive and take precedence over the
      # catch-all file serving).
      services.caddy = {
        enable = true;
        email = lib.mkIf (webCfg.acmeEmail != null) webCfg.acmeEmail;
        virtualHosts."web.${webCfg.domain}".extraConfig = ''
            handle /config.js {
              root * ${pkgs.writeTextDir "config.js" "window.KALLIP_CONFIG = ${builtins.toJSON webCfg.runtimeConfig};"}
              file_server
            }
          handle {
            root * ${webCfg.package}
            try_files {path} /index.html
            file_server
          }
        '';
      };
    })
    (lib.mkIf (webCfg.enable && webCfg.domain != null && polisCfg.enable) {
      # L1.5 direct-connect drift warning: the module can only see an
      # explicit runtimeConfig.tlsOff — a browser that derives tlsOff from
      # an http location is outside this module's visibility, so the
      # warning is best-effort by design (documented gap until the
      # config.ports schema lands).
      warnings =
        let
          userServices = webCfg.runtimeConfig.services or { };
          unpinnedChanged =
            builtins.filter
              (
                svc:
                (webCfg.runtimeConfig.tlsOff or false) == true
                && polisCfg.ports.${svc} != defaultPolisPorts.${svc}
                && !(builtins.isAttrs userServices && userServices ? ${svc})
              )
              [
                "archeion"
                "lesche"
                "files"
                "instances"
              ];
        in
        map (
          svc:
          "services.kallipai.polis.ports.${svc} is set to ${toString polisCfg.ports.${svc}}, but the web UI's direct-connect derivation still targets the default port ${
            toString defaultPolisPorts.${svc}
          } for ${svc}. Set services.kallipai.web.runtimeConfig.services.${svc} to pin the new port."
        ) unpinnedChanged;
    })
  ];
}
