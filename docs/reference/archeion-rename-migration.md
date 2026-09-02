# Archeion rename: runtime migration notes

The 2026-09-02 rename moved the `agora` service to `archeion` and the
deployment compositions to `polis`. Code, packages, deployment files, and
tracked config carry the new names as of `0c09af66^..88e2eb34`; the naming
background and the full before/after table live in `docs/naming.md`. This
page lists the runtime and state side that a commit cannot rename.

## Dev stack

1. **`.env`** — the eight `agora` entries (admin token, OAuth pair, relay
   URL, and friends) were renamed in place during the batch. A private copy
   needs the same keys updated: `KALLIP_AGORA_*` → `KALLIP_ARCHEION_*`,
   `KALLIP_TAGMA_RELAY_AGORA_URL` → `KALLIP_TAGMA_RELAY_ARCHEION_URL`,
   `KALLIP_ARION_AGORA_PORT` → `KALLIP_ARION_ARCHEION_PORT`,
   `VITE_AGORA_URL` → `VITE_ARCHEION_URL`.
2. **Postgres volume (optional)** — the compose volume is now
   `archeion_pgdata`, so the first boot after the rename starts from an
   empty database. To keep the old data, run a dump/restore round-trip
   before the new stack's first boot (image tag matches the compose
   file, `postgres:17.5`):

   ```sh
   # 1. Stop the old stack so the dump is consistent.
   arion down
   # 2. Back up: dump the old volume via a throwaway postgres.
   docker run -d --name rename-dump -v agora_pgdata:/var/lib/postgresql/data \
     -e POSTGRES_PASSWORD=x postgres:17.5
   docker exec rename-dump pg_isready -U postgres  # wait: accepting connections
   docker exec rename-dump pg_dumpall -U postgres > agora-backup.sql
   docker rm -f rename-dump
   # 3. Restore into the new volume and verify the data landed.
   docker run -d --name rename-restore -v archeion_pgdata:/var/lib/postgresql/data \
     -e POSTGRES_PASSWORD=x postgres:17.5
   docker exec rename-restore pg_isready -U postgres  # wait: accepting connections
   docker exec -i rename-restore psql -U postgres < agora-backup.sql
   docker exec rename-restore psql -U postgres -c '\l'  # kallip listed = restored
   docker rm -f rename-restore
   ```

   The old volume is left in place either way; delete it manually once the
   new stack is confirmed good. A physical `cp -a` between the two volumes
   is equally safe when both were created under the same Postgres version.
3. **Tagma credentials (optional)** — the stored enrollment origin file was
   renamed `agora.url` → `archeion.url`. Doing nothing is safe: the loader
   ignores the old file, the origin reads as absent, and the next stored
   boot backfills it under the new name. To preserve the recorded origin
   instead, rename it:

   ```sh
   mv <credentials_dir>/agora.url <credentials_dir>/archeion.url
   ```

## Operator checklist (outside the repo)

- **DNS**: add `archeion.<domain>` (and `archeion2.<domain>` if the
  dual-instance dev acceptance is used in prod-like setups); retire
  `agora.<domain>` after cutover.
- **Edge proxy**: the HOST-route rule `agora.<d> → agora:7100` becomes
  `archeion.<d> → archeion:7100`.
- **TLS**: wildcard `*.<domain>` certificates need no action; per-name
  certificates must be reissued for the new subdomain.
- **Enrolled tagmata**: each relay-connected tagma remembers its enrollment
  origin. Update `KALLIP_TAGMA_RELAY_ARCHEION_URL` and re-enroll; a tagma
  that keeps the old origin keeps failing to reach the agora until it is
  re-enrolled against `archeion`.
- **Image references**: pull by `kallip-archeion` (the image name changed
  with the flake attr `kallip-archeion-image`).
