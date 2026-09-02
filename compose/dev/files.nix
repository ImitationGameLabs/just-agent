# Dev files-side composition fragment: files + files-postgres. Imported by
# compose/dev/agora.nix (the default dev stack) -- the files service belongs
# to the agora-side stack, not its own single-purpose composition, because it
# leans on the agora's /internal ControlPlane surface over the compose
# network (the same dependency shape as the lesche).
#
# Shape follows the stack's existing services: the workspace `files` binary
# via useHostStore, its own postgres:17.5 with a files_pgdata named volume,
# and the dev shared internal secret ("dev-internal-secret") presented as
# KALLIP_FILES_AGORA_TOKEN -- it must equal the agora's
# KALLIP_AGORA_INTERNAL_TOKEN (same discipline as the lesche/instances pair).
{ pkgs, lib, ... }:
let
  # Load via git+file URL (not a bare path) so getFlake applies fetchGit's VCS
  # filtering and the resolved packages match `nix build .#*` bit-for-bit --
  # the same resolution compose/dev/agora.nix performs.
  flake = builtins.getFlake "git+file://${toString ../..}";
  workspace = flake.packages.x86_64-linux.default;

  # The files service builds a reqwest Client at startup (the agora /internal
  # calls), and the rustls platform verifier loads the system trust store
  # EAGERLY at .build() -- so it needs the CA bundle at the standard paths
  # (the shared `cacert` wrapper) even though its /internal calls are plain
  # HTTP. Same reason the lesche service carries the CA layer.
  shared = import ../../nix/packages/container-shared.nix { inherit pkgs; };
  inherit (shared) cacert;
in
{
  config = {
    # Named volumes must be declared at the compose top level (compose rejects
    # a reference to an undeclared named volume); declaring them HERE (not in
    # agora.nix) keeps the files-side storage self-contained. The project name
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
    # facts stay in the agora, reached through the /internal ControlPlane
    # surface over the compose network. Reached by the `kallip file` CLI and,
    # since the files page (F1) by the browser: published on all host
    # interfaces (the lesche pattern); the Caddy files.<devDomain> route in
    # the TLS shape fronts the same port.
    services.files = {
      service.depends_on = [
        "agora"
        "files-postgres"
      ];
      service.useHostStore = true;
      service.command = [ "${workspace}/bin/kallip-files" ];
      # Open publish in both TLS shapes (the lesche pattern -- a platform
      # microservice the browser reaches directly); the https shape's
      # Caddy route fronts the same port from the host network namespace.
      service.ports = [ "7400:7400" ];
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
        # Private compose-network hop to the agora's /internal surface; never
        # routed through the public edge.
        KALLIP_FILES_AGORA_INTERNAL_URL = "http://agora:7100";
        # Must equal the agora's KALLIP_AGORA_INTERNAL_TOKEN (dev
        # fixture, same discipline as the lesche's KALLIP_LESCHE_AGORA_TOKEN).
        KALLIP_FILES_AGORA_TOKEN = "dev-internal-secret";
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
