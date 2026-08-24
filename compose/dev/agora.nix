# Dev agora-side composition: caddy + agora + lesche + agora-postgres +
# lesche-postgres + instances. The default
# dev stack -- a plain `arion up` brings it up via the `arion-compose.nix` shim
# at the repo root (which just re-exports this module); invoke directly with
# `arion -f compose/dev/agora.nix ...` for the same result.
#
# The dev tagma (compose/dev/tagma.nix) and the integration-test runner
# (compose/dev/test.nix) are NOT here: each is its own single-purpose
# composition under compose/dev/, sharing nothing with the agora side.
# Prod-tagma / prod-agora are standalone under compose/prod/.
#
# Consumes the flake's pre-built `packages.default` directly -- arion does no
# Rust/crane building. `useHostStore` shares the host /nix/store into the
# containers, so a rebuild is picked up without an in-compose bake. See
# docs/development.md for the bring-up commands and flow.
{ pkgs, lib, ... }:
let
  # Load via git+file URL (not a bare path) so getFlake applies fetchGit's VCS
  # filtering and the resolved packages match `nix build .#*` bit-for-bit.
  flake = builtins.getFlake "git+file://${toString ../..}";
  workspace = flake.packages.x86_64-linux.default;

  # Shared toolset + certs + aifed + PATH. Reused by the tagma docker image
  # (nix/packages/docker-images/tagma.nix) and here in the dev compose, so the
  # two cannot drift.
  shared = import ../../nix/packages/container-shared.nix { inherit pkgs; };
  inherit (shared)
    toolEnv
    cacert
    aifed
    binPath
    skillsSeed
    ;

  # Dev topology note: a Caddy edge proxy (services.caddy) terminates TLS for
  # `*.<devDomain>` (default `*.kallipai.com`) with an mkcert certificate and
  # host-routes web/agora/lesche subdomains to vite (on the host) / agora /
  # lesche. This makes the dev stack reachable cross-machine on the LAN
  # (browsers only allow WebAuthn in a secure context, so plain-HTTP + raw LAN
  # IP cannot work). The session cookie carries `Domain=<devDomain>` (see the
  # agora service env) so it is shared across the agora/lesche subdomains. agora
  # and lesche still publish 7100/7200 for host-side tooling (kallip-admin,
  # curl) AND for the dev tagma (compose/dev/tagma.nix, host network), which
  # reaches them at 127.0.0.1:7100 / :7200 rather than via compose DNS.

  # The dev domain (registrable domain + subdomain parent). The code default is
  # the prod domain (kallipai.com); dev overrides it to kallipai.lan via .env
  # (see .env.example) -- direnv's dotenv puts .env in the shell, so this
  # builtins.getEnv sees it at eval time. Everything below (WebAuthn RP
  # id/origin, CORS, cookie domain, Caddyfile, the web app's API URLs) derives
  # from it.
  devDomain =
    let
      v = builtins.getEnv "KALLIP_DEV_DOMAIN";
    in
    if v == "" then "kallipai.com" else v;

  # Path to the mkcert leaf cert dir (cert.pem + key.pem). Defaults to
  # <repo>/compose/dev/.certs -- where the mkcert command in docs/development.md
  # writes -- so no env var is needed for the common case; override
  # KALLIP_ARION_CERT_PATH only to point elsewhere (e.g. a shared dir across
  # worktrees). If the dir is missing, Caddy fails at runtime with a clear "cert
  # not found" -- the mkcert step in docs/development.md is the prerequisite.
  certDir =
    let
      v = builtins.getEnv "KALLIP_ARION_CERT_PATH";
    in
    if v == "" then
      "${toString ../..}/compose/dev/.certs"
    else if !(lib.hasPrefix "/" v) || lib.hasInfix ":" v then
      throw "arion: KALLIP_ARION_CERT_PATH must be an absolute, colon-free path (got '${v}')"
    else
      v;

  # Bind-override helpers (mirrors compose/dev/tagma.nix): unset -> docker
  # named volume; set to an absolute, colon-free host path -> bind-mount.
  # The instances service mounts the HOST daemon's socket dir + instance
  # tree this way, so the containerized service manages the same real
  # daemon a host-side kallipctl sees.
  bindOverride =
    name: target:
    let
      v = builtins.getEnv name;
    in
    if v == "" then
      null
    else if v == "/" || !(lib.hasPrefix "/" v) || lib.hasInfix ":" v then
      throw "arion: ${name} must be an absolute, colon-free host path other than '/' (got '${v}')"
    else
      "${v}:${target}";
  instancesStateBind = bindOverride "KALLIP_ARION_INSTANCES_STATE_PATH" "/state";
  instancesDataBind = bindOverride "KALLIP_ARION_INSTANCES_DATA_PATH" "/data";
  instancesStateVolume =
    if instancesStateBind != null then instancesStateBind else "instances_state:/state";
  instancesDataVolume =
    if instancesDataBind != null then instancesDataBind else "instances_data:/data";

in
{
  config = {
    project.name = "kallipai-dev";

    # Named volumes must be declared at the compose top level (compose rejects
    # a reference to an undeclared named volume). The `kallipai-dev` project
    # name prefixes every volume, so the internal name only carries the
    # meaningful suffix.
    docker-compose.volumes = {
      agora_pgdata = { };
      lesche_pgdata = { };
    }
    // lib.optionalAttrs (instancesStateBind == null) { instances_state = { }; }
    // lib.optionalAttrs (instancesDataBind == null) { instances_data = { }; };

    # Dev-only hardcoded creds (prod reads them from .env).
    services.agora-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "agora_pgdata:/var/lib/postgresql/data" ];
      service.environment = {
        POSTGRES_USER = "kallip";
        POSTGRES_PASSWORD = "kallip";
        POSTGRES_DB = "kallip";
      };
    };

    services.lesche-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "lesche_pgdata:/var/lib/postgresql/data" ];
      service.environment = {
        POSTGRES_USER = "kallip";
        POSTGRES_PASSWORD = "kallip";
        POSTGRES_DB = "kallip";
      };
    };

    # Caddy edge proxy: terminates TLS for *.<devDomain> (default
    # *.kallipai.com) with the mkcert leaf cert and host-routes the three
    # subdomains to 127.0.0.1: web.<devDomain> -> the host vite dev server
    # (:5173); agora/lesche -> their host-published ports (:7100/:7200).
    # Runs on the host network (`network_mode: host`) so it can reach the
    # host's vite directly -- under rootless docker the bridge cannot reach
    # host services (host-gateway resolves to a non-routable IP and the host
    # firewall drops the LAN IP). With the host netns, caddy binds :80/:443
    # straight on the host (requires net.ipv4.ip_unprivileged_port_start<=80
    # under rootless), so no `ports:` mapping (ignored under host net anyway)
    # and no extra_hosts. The Caddyfile (mounted below) uses
    # {$KALLIP_DEV_DOMAIN} substitution; see it for the routing + the
    # streaming flush on lesche.
    services.caddy = {
      service.image = "caddy:2.8";
      service.depends_on = [
        "agora"
        "lesche"
      ];
      service.network_mode = "host";
      service.volumes = [
        "${./Caddyfile.dev}:/etc/caddy/Caddyfile:ro"
        "${certDir}:/certs:ro"
      ];
      # The domain for Caddyfile {$KALLIP_DEV_DOMAIN} substitution. Sourced
      # from the same nix `devDomain` as the agora/lesche env below so the
      # whole stack agrees on one name.
      service.environment.KALLIP_DEV_DOMAIN = devDomain;
      service.command = [
        "caddy"
        "run"
        "--config"
        "/etc/caddy/Caddyfile"
      ];
    };

    # Agora: run from the workspace via the host store; publish 7100 for
    # host-side tooling (kallip-admin, curl). The browser reaches it via Caddy
    # at https://agora.<devDomain>. dev WebAuthn / CORS / cookie values all
    # derive from the `devDomain` nix binding. (prod-agora is its own
    # composition: compose/prod/agora.nix, behind the operator's TLS
    # reverse proxy, no published port.)
    services.agora = {
      service.depends_on = [ "agora-postgres" ];
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-agora" ];
      service.ports = [ "7100:7100" ];
      # Optional stable admin token (else generated per boot, printed to
      # `arion logs agora`).
      service.env_file = [ ".env" ];
      # cacert: the reqwest oauth client (rustls) loads the system trust
      # store at startup.
      image.contents = [
        workspace
      ]
      ++ cacert;
      service.environment = {
        PATH = "${workspace}/bin";
        KALLIP_AGORA_ADDR = "0.0.0.0:7100";
        KALLIP_AGORA_DATABASE_URL = "postgres://kallip:kallip@agora-postgres:5432/kallip";
        # The browser loads the SPA at https://web.<devDomain> (Caddy
        # terminates TLS), so the WebAuthn RP id is the registrable domain
        # <devDomain> and the RP origin is the web subdomain. Standard 443
        # port -> ALLOW_ANY_PORT stays false.
        KALLIP_AGORA_WEBAUTHN_RP_ID = devDomain;
        KALLIP_AGORA_WEBAUTHN_RP_ORIGIN = "https://web.${devDomain}";
        KALLIP_AGORA_WEBAUTHN_RP_NAME = "kallip";
        KALLIP_AGORA_WEBAUTHN_ALLOW_ANY_PORT = "false";
        # Behind Caddy's TLS -> the session cookie is Secure.
        KALLIP_AGORA_COOKIE_SECURE = "true";
        KALLIP_AGORA_CORS_ORIGINS = "https://web.${devDomain}";
        # Share the session cookie across agora.<devDomain> and
        # lesche.<devDomain> (the per-subdomain topology). Single-origin
        # deploys leave this unset (host-only cookie).
        KALLIP_AGORA_SESSION_COOKIE_DOMAIN = devDomain;
        # Caddy runs on the host network and proxies to agora at 127.0.0.1,
        # so trust loopback for X-Forwarded-For. agora binds 0.0.0.0:7100
        # (non-loopback), so the boot guard would otherwise clear the trusted
        # set and log every client as 127.0.0.1 (collapsing per-client rate
        # limiting).
        KALLIP_AGORA_TRUSTED_PROXIES = "127.0.0.0/8, ::1/128";
        # Dev shared secret mounting the /internal ControlPlane surface for
        # the lesche. Hardcoded like the dev DB creds (prod reads it from
        # .env). The lesche presents the SAME value as
        # KALLIP_LESCHE_AGORA_TOKEN.
        KALLIP_AGORA_INTERNAL_TOKEN = "dev-internal-secret";
        # The local-platform login refuses to boot with an operator-set
        # admin token shorter than 32 chars, so the dev fixture pins a
        # compliant one HERE (service.environment overrides the shorter
        # KALLIP_AGORA_ADMIN_TOKEN a legacy .env may still carry).
        KALLIP_AGORA_ADMIN_TOKEN = "sk-admin-dev-0123456789abcdef0123456789abcdef";
        # Local-platform operator login: exchange the admin token for a User
        # session (POST /v1/auth/admin-login) on a fixed local account. Dev
        # fixture, paired with the compliant token above; prod leaves it off.
        KALLIP_AGORA_ADMIN_USER_LOGIN = "true";
        RUST_LOG = "info";
      };
    };

    # Lesche: the data-plane relay. Owns the chat domain in its own Postgres
    # (rooms, membership, message payloads) and
    # authenticates + attests identity through the agora's /internal surface
    # over the compose network. Reached by the browser via Caddy at
    # https://lesche.<devDomain> and by the tagma's relay connector via
    # compose DNS (lesche:7200).
    services.lesche = {
      # No healthcheck; unlike the agora, lesche does not retry its DB connect.
      service.depends_on = [
        "agora"
        "lesche-postgres"
      ];
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-lesche" ];
      service.ports = [ "7200:7200" ];
      service.env_file = [ ".env" ];
      # reqwest (HttpControlPlane -> agora /internal) builds its Client at
      # startup and the rustls platform verifier loads the system trust store
      # EAGERLY at .build() -- so the lesche needs the CA bundle at the
      # standard paths (the shared `cacert` wrapper) even though its /internal
      # calls are plain HTTP. Same reason the tagma service carries the CA
      # layer (its in-process relay connector builds a reqwest client at
      # startup).
      image.contents = [
        workspace
      ]
      ++ cacert;
      service.environment = {
        KALLIP_LESCHE_ADDR = "0.0.0.0:7200";
        KALLIP_LESCHE_DATABASE_URL = "postgres://kallip:kallip@lesche-postgres:5432/kallip";
        KALLIP_LESCHE_AGORA_INTERNAL_URL = "http://agora:7100";
        # Must equal the agora's KALLIP_AGORA_INTERNAL_TOKEN above.
        KALLIP_LESCHE_AGORA_TOKEN = "dev-internal-secret";
        # Allow the web app origin (https://web.<devDomain> via Caddy) to
        # make credentialed cross-origin calls to lesche.<devDomain>.
        KALLIP_LESCHE_CORS_ORIGINS = "https://web.${devDomain}";
        RUST_LOG = "info";
      };
    };

    # Instances: the local instance management service (kallip-instances),
    # proxying the HOST daemon's UDS socket. Both the socket dir and the
    # instance tree come from the host via bind overrides (unset -> empty
    # named volumes, i.e. an unreachable daemon and an empty list -- enough
    # for a pure UI bring-up). The browser reaches it via Caddy at
    # https://instances.<devDomain>; host tooling uses the published
    # 127.0.0.1:7300. Token mode with a dev fixture token (the agora
    # admin-token pattern); prod reads it from .env.
    services.instances = {
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-instances" ];
      # Loopback-tight publish: only host-side tooling (curl, the vite-less
      # dev flow) needs the direct port; the browser path is Caddy.
      service.ports = [ "127.0.0.1:7300:7300" ];
      service.env_file = [ ".env" ];
      service.volumes = [
        instancesStateVolume
        instancesDataVolume
      ];
      image.contents = [
        workspace
      ]
      ++ cacert;
      service.environment = {
        PATH = "${workspace}/bin";
        KALLIP_INSTANCES_ADDR = "0.0.0.0:7300";
        # The mounted host dirs carry the daemon's socket + instance tree;
        # both point INTO the container mounts, never at host paths.
        KALLIP_DAEMON_SOCKET = "/state/control.sock";
        KALLIP_DAEMON_DATA_DIR = "/data";
        KALLIP_INSTANCES_TOKEN = "sk-instances-dev-0123456789abcdef0123456789abcdef";
        KALLIP_INSTANCES_ALLOWED_HOSTS = "instances.${devDomain}";
        KALLIP_INSTANCES_CORS_ORIGINS = "https://web.${devDomain}";
        RUST_LOG = "info";
      };
    };
  };
}
