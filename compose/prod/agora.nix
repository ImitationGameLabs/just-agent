# Arion composition for the prod-agora deploy (the server side): agora +
# lesche + files + agora-postgres + lesche-postgres + files-postgres. The
# agora (control plane) runs from packages.kallip-agora-image; the lesche
# (data-plane relay) runs from packages.kallip-lesche-image; files (content
# transfer) runs from packages.kallip-files-image; each postgres uses the
# official postgres:17.5 image for production parity and isolation.
#
# Invoke from the repo root (so .env resolves):
#   arion -f compose/prod/agora.nix up -d
#
# This is a single-purpose file: every service is declared directly, no mode
# switch or mkIf/mkMerge. Secret-bearing deploy env (DB url incl.
# password, WebAuthn RP, CORS, cookie domain, admin token, the internal
# shared secret, POSTGRES_PASSWORD) comes from the repo-root .env; each
# service's operational env (listen addr, blob root, internal hop URL)
# is pinned inline and overrides env_file.
# None of the three services is published -- all sit behind the
# operator's TLS-terminating edge proxy, which HOST-routes agora.<d> -> agora
# and lesche.<d> -> lesche / files.<d> -> files (the per-service subdomain
# topology). The lesche and the files service reach the agora's /internal
# ControlPlane surface over the private compose network (each with its own
# KALLIP_*_AGORA_INTERNAL_URL=http://agora:7100); the proxy must NOT route
# /internal publicly. See docs/reference/container.md.
{ lib, ... }:
let
  # Resolve the workspace flake. `toString ../..` is the repo root (two levels
  # up from this file); the git+file URL applies fetchGit's VCS filtering so the
  # packages match `nix build .#*` bit-for-bit.
  flake = builtins.getFlake "git+file://${toString ../..}";
  agora = flake.packages.x86_64-linux.kallip-agora;
  agoraImage = flake.packages.x86_64-linux.kallip-agora-image;
  lesche = flake.packages.x86_64-linux.kallip-lesche;
  lescheImage = flake.packages.x86_64-linux.kallip-lesche-image;
  files = flake.packages.x86_64-linux.kallip-files;
  filesImage = flake.packages.x86_64-linux.kallip-files-image;
in
{
  config = {
    project.name = "kallipai-agora";

    docker-compose.volumes = {
      agora_pgdata = { };
      lesche_pgdata = { };
      files_pgdata = { };
      files_blobs = { };
    };

    # POSTGRES_USER/PASSWORD/DB come from .env ONLY and are read by all
    # three postgres services -- do NOT set them in service.environment
    # (compose precedence would pin a weak default password on a public DB).
    services.agora-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "agora_pgdata:/var/lib/postgresql/data" ];
      service.env_file = [ ".env" ];
    };

    services.lesche-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "lesche_pgdata:/var/lib/postgresql/data" ];
      service.env_file = [ ".env" ];
    };

    services.files-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "files_pgdata:/var/lib/postgresql/data" ];
      service.env_file = [ ".env" ];
    };

    services.agora = {
      service.depends_on = [ "agora-postgres" ];
      # arion's image-builder option is `services.<name>.build.image` (a sibling
      # of `service`, not nested under it). mkForce replaces arion's own nix-image
      # builder (which would inject a nix-database layer).
      build.image = lib.mkForce agoraImage;
      service.command = [ "${agora}/bin/kallip-agora" ];
      service.env_file = [ ".env" ];
      service.environment = {
        KALLIP_AGORA_ADDR = "0.0.0.0:7100";
        RUST_LOG = "info";
        # KALLIP_AGORA_INTERNAL_TOKEN (the shared secret the lesche presents to
        # the /internal/* surface) comes from .env. When unset, the agora runs
        # standalone and the /internal nest is not mounted -- so the lesche
        # service below will fail its ControlPlane calls until it is set.
        # KALLIP_AGORA_SESSION_COOKIE_DOMAIN comes from .env: set to the parent
        # domain (e.g. kallipai.com) so the session cookie is shared across the
        # agora.<d> and lesche.<d> subdomains the edge routes here.
      };
      # No service.ports -- the agora sits behind the operator's TLS-terminating
      # edge proxy, which HOST-routes agora.<d> -> agora:7100 and lesche.<d> ->
      # lesche:7200 and sets X-Forwarded-For; configure
      # KALLIP_AGORA_TRUSTED_PROXIES to the proxy's CIDR (prod keeps its proxy,
      # unlike dev). /internal is reached by the lesche over the private compose
      # network, never via the public edge.
    };

    # Lesche: the data-plane relay (tagma relay tunnels, app SSE, envelope routing,
    # KEX, presence). Owns the chat domain in its own Postgres (rooms, membership,
    # message payloads); it authenticates requests and
    # attests identity through the agora's /internal ControlPlane API over the
    # private compose network. Not published -- the operator's edge host-routes
    # lesche.<d> here.
    services.lesche = {
      # No healthcheck; unlike the agora, lesche does not retry its DB connect.
      service.depends_on = [
        "agora"
        "lesche-postgres"
      ];
      build.image = lib.mkForce lescheImage;
      service.command = [ "${lesche}/bin/kallip-lesche" ];
      service.env_file = [ ".env" ];
      service.environment = {
        KALLIP_LESCHE_ADDR = "0.0.0.0:7200";
        # Private compose-network hop to the agora's /internal surface; never
        # routed through the public edge.
        KALLIP_LESCHE_AGORA_INTERNAL_URL = "http://agora:7100";
        RUST_LOG = "info";
        # KALLIP_LESCHE_DATABASE_URL (the chat schema), KALLIP_LESCHE_AGORA_TOKEN
        # (must equal the agora's KALLIP_AGORA_INTERNAL_TOKEN), and
        # KALLIP_LESCHE_CORS_ORIGINS come from .env.
      };
      # No service.ports -- like the agora, the lesche sits behind the
      # TLS-terminating reverse proxy.
    };

    # Files: the content-transfer service. Content-addressed blobs (local
    # volume) + record metadata in its own Postgres; identity and enrollment
    # facts stay in the agora, verified per request through the agora's
    # /internal ControlPlane surface over the private compose network. Not
    # published -- the operator's edge host-routes files.<d> here.
    services.files = {
      service.depends_on = [
        "agora"
        "files-postgres"
      ];
      build.image = lib.mkForce filesImage;
      service.command = [ "${files}/bin/kallip-files" ];
      service.env_file = [ ".env" ];
      service.volumes = [ "files_blobs:/data/blobs" ];
      service.environment = {
        KALLIP_FILES_ADDR = "0.0.0.0:7400";
        # The blob root INSIDE the container; must equal the files_blobs
        # volume mount target above.
        KALLIP_FILES_BLOB_ROOT = "/data/blobs";
        # Private compose-network hop to the agora's /internal surface;
        # never routed through the public edge.
        KALLIP_FILES_AGORA_INTERNAL_URL = "http://agora:7100";
        RUST_LOG = "info";
        # KALLIP_FILES_DATABASE_URL (the metadata schema) and
        # KALLIP_FILES_AGORA_TOKEN (must equal the agora's
        # KALLIP_AGORA_INTERNAL_TOKEN) come from .env.
      };
      # No service.ports -- like the agora and the lesche, the files service
      # sits behind the TLS-terminating reverse proxy.
    };
  };
}
