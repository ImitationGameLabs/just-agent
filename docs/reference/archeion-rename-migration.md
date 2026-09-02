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
   empty database. To keep the old data, copy it before bringing the new
   stack up (same Postgres image generation, so a physical copy is safe):

   ```sh
   docker run --rm -v agora_pgdata:/from -v archeion_pgdata:/to alpine \
     sh -c 'cp -a /from/. /to/'
   ```

   The old volume is left in place either way; delete it manually once the
   new stack is confirmed good. Prefer a `pg_dump`/restore round-trip if
   the volumes were ever created under different Postgres versions.
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
