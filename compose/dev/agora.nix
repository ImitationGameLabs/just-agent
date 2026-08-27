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

  # The stack shape switch: KALLIP_TLS=on (default) keeps the Caddy-fronted
  # https+domain topology below; KALLIP_TLS=off is the plain-http direct
  # shape -- no Caddy, host defaults to localhost (set KALLIP_DOMAIN to a
  # LAN host for multi-machine access; see docs/development.md).
  tlsOff = envOrDefault "KALLIP_TLS" "on" == "off";
  # The dev domain (registrable domain + subdomain parent, or the plain
  # host when TLS is off). The code default is the prod domain
  # (kallipai.com) with TLS on, localhost with TLS off; .env overrides
  # (kallipai.lan for the https dev shape) -- direnv's dotenv puts .env in
  # the shell, so this builtins.getEnv sees it at eval time. Everything
  # below (WebAuthn RP id/origin, CORS, cookie domain, Caddyfile, the web
  # app's API URLs) derives from these bindings.
  devDomain =
    let
      v = builtins.getEnv "KALLIP_DOMAIN";
    in
    if v == "" then (if tlsOff then "localhost" else "kallipai.com") else v;
  # The browser-facing web origin: the Caddy subdomain face when TLS is
  # on, the plain vite origin (:5173) when off.
  webOrigin = if tlsOff then "http://${devDomain}:5173" else "https://web.${devDomain}";

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

  # Bind helper (mirrors compose/dev/tagma.nix): an absolute, colon-free
  # host path -> "<path>:<target>" bind-mount. The named env var wins; unset
  # falls back to the daemon-standard host dir. Both paths go through the
  # same shape check, so a malformed HOME or override fails fast at eval
  # time instead of half-working at up time.
  bindOverride =
    name: target: fallback:
    let
      v = builtins.getEnv name;
      src = if v == "" then fallback else v;
    in
    if src == "/" || !(lib.hasPrefix "/" src) || lib.hasInfix ":" src then
      throw "arion: ${name} must resolve to an absolute, colon-free host path other than '/' (got '${src}')"
    else
      "${src}:${target}";
  # Unset defaults: the HOST daemon's standard dirs, so a plain arion up
  # has the service manage the SAME real daemon + instance tree a
  # host-side kallipctl sees (the daemon's own code defaults -- see
  # crates/daemon/kallip-daemon/src/main.rs; the data default is
  # ~/.local/share/kallip).
  homeDir = builtins.getEnv "HOME";
  daemonDir =
    if homeDir == "" then
      throw "arion: HOME unset; defaulting the instances binds needs it (or set KALLIP_ARION_INSTANCES_STATE/DATA_PATH)"
    else
      sub: "${homeDir}/.local/${sub}";
  instancesStateBind = bindOverride "KALLIP_ARION_INSTANCES_STATE_PATH" "/state" (
    daemonDir "state/kallip-daemon"
  );
  instancesDataBind = bindOverride "KALLIP_ARION_INSTANCES_DATA_PATH" "/data" (
    daemonDir "share/kallip"
  );

  # Parallel-stack overrides (the same env pattern as bindOverride above):
  # unset -> the default single stack; set -> a second, independent dev stack
  # with its own project name (own containers, networks, and volumes), for the
  # dual-agora acceptance flow:
  #   KALLIP_ARION_PROJECT_NAME=kallipai-dev2 \
  #   KALLIP_ARION_AGORA_PORT=7101 KALLIP_ARION_LESCHE_PORT=7201 \
  #   arion up -d agora lesche
  # Only the HOST side of each publish is overridden (container ports and the
  # compose-internal URLs stay fixed), and caddy keeps routing the
  # agora2./lesche2. subdomains to the host ports -- so the second stack is
  # reachable exactly where the old inline pair was. Bring up only agora +
  # lesche (plus their deps): caddy would fight the first stack for :80/:443,
  # and instances owns the loopback-only 7300.
  envOrDefault =
    name: default:
    let
      v = builtins.getEnv name;
    in
    if v == "" then default else v;
  projectName = envOrDefault "KALLIP_ARION_PROJECT_NAME" "kallipai-dev";
  agoraHostPort = envOrDefault "KALLIP_ARION_AGORA_PORT" "7100";
  lescheHostPort = envOrDefault "KALLIP_ARION_LESCHE_PORT" "7200";
  instancesHostPort = envOrDefault "KALLIP_ARION_INSTANCES_PORT" "7300";
in
{
  config = {
    project.name = projectName;

    # Named volumes must be declared at the compose top level (compose rejects
    # a reference to an undeclared named volume). The project name
    # (`kallipai-dev` by default) prefixes every volume, so the internal
    # name only carries the meaningful suffix.
    docker-compose.volumes = {
      agora_pgdata = { };
      lesche_pgdata = { };
    };

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
    # {$KALLIP_DOMAIN} substitution; see it for the routing + the
    # streaming flush on lesche.
    services.caddy = lib.mkIf (!tlsOff) {
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
      # The domain for Caddyfile {$KALLIP_DOMAIN} substitution. Sourced
      # from the same nix `devDomain` as the agora/lesche env below so the
      # whole stack agrees on one name.
      service.environment.KALLIP_DOMAIN = devDomain;
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
      service.ports = [ "${agoraHostPort}:7100" ];
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
        # WebAuthn RP: the id is the registrable domain <devDomain> (the
        # plain host in the http shape); the origin is the browser-facing
        # webOrigin (https://web.<d> via Caddy, or http://<host>:5173 in
        # the http shape, whose non-standard port needs ALLOW_ANY_PORT).
        KALLIP_AGORA_WEBAUTHN_RP_ID = devDomain;
        KALLIP_AGORA_WEBAUTHN_RP_ORIGIN = webOrigin;
        KALLIP_AGORA_WEBAUTHN_RP_NAME = "kallipai";
        KALLIP_AGORA_WEBAUTHN_ALLOW_ANY_PORT = if tlsOff then "true" else "false";
        # Behind Caddy's TLS the session cookie is Secure; the plain-http
        # shape needs it off.
        KALLIP_AGORA_COOKIE_SECURE = if tlsOff then "false" else "true";
        KALLIP_AGORA_CORS_ORIGINS = webOrigin;
        # Share the session cookie across agora.<devDomain> and
        # lesche.<devDomain> (the per-subdomain topology). Single-origin
        # deploys -- and the http shape's host-only vite origin -- leave
        # this unset (host-only cookie); the httpShape merge below does.
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
      } // lib.optionalAttrs (!tlsOff) { KALLIP_AGORA_SESSION_COOKIE_DOMAIN = devDomain; } // lib.optionalAttrs tlsOff { KALLIP_AGORA_OAUTH_REDIRECT_BASE = webOrigin; };
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
      service.ports = [ "${lescheHostPort}:7200" ];
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
        KALLIP_LESCHE_CORS_ORIGINS = webOrigin;
        RUST_LOG = "info";
      };
    };

    # Instances: the local instance management service (kallip-instances),
    # proxying the HOST daemon's UDS socket. Both the socket dir and the
    # instance tree are host bind mounts: the KALLIP_ARION_INSTANCES_*
    # overrides, or unset the HOST daemon's standard dirs (the same real
    # daemon a host-side kallipctl sees). The browser reaches it via
    # https://instances.<devDomain>; host tooling uses the published
    # 127.0.0.1:7300. Platform mode: the agora's internal root + shared
    # secret verify the SPA's sk-admin- bearer (the local-platform
    # login key), replacing the standalone token. API-only: the SPA is
    # served by the host vite dev server (Caddy @web -> :5173); the
    # browser calls this service cross-origin from the web origin
    # (KALLIP_INSTANCES_CORS_ORIGINS below).
    services.instances = {
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-instances" ];
      # Loopback-tight publish in the https shape (the browser path is
      # Caddy); the http shape opens 7300 to the LAN so browsers on other
      # machines reach the instances API directly (token-gated +
      # host-allowlisted; treat the LAN as a trusted surface).
      service.ports = [ (if tlsOff then "${instancesHostPort}:7300" else "127.0.0.1:${instancesHostPort}:7300") ];
      service.env_file = [ ".env" ];
      service.volumes = [
        instancesStateBind
        instancesDataBind
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
        # Platform mode: the agora's internal face verifies the SPA's
        # sk-admin- bearer; the token must equal the agora's
        # KALLIP_AGORA_INTERNAL_TOKEN (dev fixture, same discipline).
        KALLIP_INSTANCES_AGORA_URL = "http://agora:7100";
        KALLIP_INSTANCES_AGORA_INTERNAL_TOKEN = "dev-internal-secret";
        KALLIP_INSTANCES_ALLOWED_HOSTS = if tlsOff then devDomain else "instances.${devDomain}";
        KALLIP_INSTANCES_CORS_ORIGINS = webOrigin;
        RUST_LOG = "info";
      };
    };
  };
}
