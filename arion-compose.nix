# Arion auto-discovery shim: arion only auto-loads `arion-compose.nix` at the
# repo root, so this re-exports compose/dev/polis.nix to keep `arion up`
# working. The actual dev polis composition lives there.
import ./compose/dev/polis.nix
