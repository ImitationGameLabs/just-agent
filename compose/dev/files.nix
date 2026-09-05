# Dev files-side composition fragment: files + files-postgres. Imported by
# compose/dev/polis.nix (the default dev stack) -- the files service belongs
# to the archeion-side stack, not its own single-purpose composition, because it
# leans on the archeion's /internal ControlPlane surface over the compose
# network (the same dependency shape as the lesche).
#
# Shape follows the stack's existing services: the workspace `files` binary
# via useHostStore, its own postgres:17.5 with a files_pgdata named volume,
# and the dev shared internal secret ("dev-internal-secret") presented as
# KALLIP_FILES_ARCHEION_TOKEN -- it must equal the archeion's
# KALLIP_ARCHEION_INTERNAL_TOKEN (same discipline as the lesche/instances pair).
{ pkgs, lib, ... }:
let
  # Load via git+file URL (not a bare path) so getFlake applies fetchGit's VCS
  # filtering and the resolved packages match `nix build .#*` bit-for-bit --
  # the same resolution compose/dev/polis.nix performs.
  flake = builtins.getFlake "git+file://${toString ../..}";
  workspace = flake.packages.x86_64-linux.default;

  # The files service builds a reqwest Client at startup (the archeion /internal
  # calls), and the rustls platform verifier loads the system trust store
  # EAGERLY at .build() -- so it needs the CA bundle at the standard paths
  # (the shared `cacert` wrapper) even though its /internal calls are plain
  # HTTP. Same reason the lesche service carries the CA layer.
  shared = import ../../nix/packages/container-shared.nix { inherit pkgs; };
  inherit (shared) cacert;

  # Host-side publish override, the same env pattern as polis.nix's
  # envOrDefault (the lesche convention): unset -> the default
  # all-interfaces publish on 7400; set -> a second-stack files instance
  # can live beside the first.
  envOrDefault =
    name: default:
    let
      v = builtins.getEnv name;
    in
    if v == "" then default else v;
  filesHostPort = envOrDefault "KALLIP_ARION_FILES_PORT" "7400";
in
{
  config = {
    # Named volumes must be declared at the compose top level (compose rejects
    # a reference to an undeclared named volume); declaring them HERE (not in
    # polis.nix) keeps the files-side storage self-contained. The project name
    # prefixes every volume, so the internal name carries the suffix only.
    docker-compose.volumes = {
      files_pgdata = { };
      files_blobs = { };
    };

    services.files-postgres = {
      service.image = "postgres:17.5";
      service.volumes = [ "files_pgdata:/var/lib/postgresql/data" ];
      service.environment = {
        POSTGRES_USER = "kallip";
        POSTGRES_PASSWORD = "kallip";
        POSTGRES_DB = "kallip";
      };
    };

    # Files: the content-transfer service. Content-addressed blobs (local
    # volume) + record metadata in its own Postgres; identity and enrollment
    # facts stay in the archeion, reached through the /internal ControlPlane
    # surface over the compose network. Reached by the `kallip file` CLI and,
    # since the files page landed, by the browser: published on all host
    # interfaces (the lesche pattern); the Caddy files.<devDomain> route in
    # the TLS shape fronts the same port.
    services.files = {
      service.depends_on = [
        "archeion"
        "files-postgres"
      ];
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-files" ];
      # Open publish in both TLS shapes (the lesche pattern -- a platform
      # microservice the browser reaches directly); the https shape's
      # Caddy route fronts the same port from the host network namespace.
      service.ports = [ "${filesHostPort}:7400" ];
      service.env_file = [ ".env" ];
      image.contents = [
        workspace
      ]
      ++ cacert;
      service.environment = {
        KALLIP_FILES_ADDR = "0.0.0.0:7400";
        KALLIP_FILES_DATABASE_URL = "postgres://kallip:kallip@files-postgres:5432/kallip";
        # Blob root inside the container, backed by the named volume below.
        # Service-owned data, not shared with the host daemon tree (unlike
        # the instances binds).
        KALLIP_FILES_BLOB_ROOT = "/data/blobs";
        # Private compose-network hop to the archeion's /internal surface; never
        # routed through the public edge.
        KALLIP_FILES_ARCHEION_INTERNAL_URL = "http://archeion:7100";
        # Must equal the archeion's KALLIP_ARCHEION_INTERNAL_TOKEN (dev
        # fixture, same discipline as the lesche's KALLIP_LESCHE_ARCHEION_TOKEN).
        KALLIP_FILES_ARCHEION_TOKEN = "dev-internal-secret";
        KALLIP_FILES_NOTIFY_URL = "http://lesche:7200";
        # Same dev shared secret discipline: must equal the lesche's
        # KALLIP_LESCHE_INTERNAL_TOKEN.
        KALLIP_FILES_NOTIFY_TOKEN = "dev-internal-secret";
        RUST_LOG = "info";
      };
      service.volumes = [ "files_blobs:/data/blobs" ];
    };
  };
}
