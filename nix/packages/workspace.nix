{
  pkgs,
  sharedSkills,
  common,
}:
let
  inherit (common)
    craneLib
    commonArgs
    cargoArtifacts
    ;

  # One crane build recipe for a chosen cargo command, on the shared workspace
  # deps cache. `workspace` builds every binary in one derivation; `agora` /
  # `tagma` build per-crate subsets so the purpose-built docker images carry
  # only the binaries they need (lighter closures -- see docker-images/).
  #
  # NB: doCheck = false so `nix build .#default` doesn't run `cargo test` (whose
  # sandbox-env deps -- CA roots, pgrep/kill -- would pollute the package).
  # NB: do NOT hoist doCheck into commonArgs. crane's cargoNextest and
  # buildDepsOnly both do `args.doCheck or true`, so a shared doCheck = false
  # would silently skip nextest's checkPhase AND drop buildDepsOnly's dev-dep
  # caching. Keep it on this buildPackage call only.
  buildCrate =
    cargoBuildCommand:
    craneLib.buildPackage (
      commonArgs
      // {
        inherit cargoArtifacts cargoBuildCommand;
        doCheck = false;
      }
    );
in
{
  # The full workspace: every kallip binary. This is `packages.default` and the
  # single source of truth consumed by the tarball + dev compose. It is also
  # the only install shape where the daemon resolves the tagma as a
  # same-directory sibling (kallip-daemon/src/bins.rs), so the skills-seed
  # wrapper lives here rather than on the per-crate tagma build.
  workspace = (buildCrate "cargo build --release").overrideAttrs (old: {
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [
      pkgs.makeWrapper
    ];
    postInstall = (old.postInstall or "") + ''
      # Seed the shared skills by default on nix installs: the wrapper
      # exports KALLIP_SKILLS_SEED only when unset, so an explicit env
      # value (or the container image Env) still wins. Only kallip-tagma
      # reads the seed -- the agent-side kallip CLI sharing its bin/
      # directory never does, so it stays unwrapped.
      wrapProgram $out/bin/kallip-tagma \
        --set-default KALLIP_SKILLS_SEED ${sharedSkills}/share/kallip/skills
    '';
  });
  # The agora control-plane server (pure HTTP/Postgres; no shell-out deps).
  agora = buildCrate "cargo build --release -p kallip-agora";
  # The headless agora admin CLI (HTTP client; runs on the operator host). A
  # separate attr so it can be built/deployed without the server. Not baked into
  # the agora image -- the image is deliberately minimal; operators run this
  # against any reachable agora.
  admin = buildCrate "cargo build --release -p kallip-admin";
  # The lesche data-plane relay (tagma relay tunnels, app SSE, envelope
  # routing; pure HTTP, no shell-out deps). Its own image so the agora and
  # lesche services deploy independently -- see
  # nix/packages/docker-images/lesche.nix.
  lesche = buildCrate "cargo build --release -p kallip-lesche";
  # The files transfer service (content-addressed blobs, ACL'd spaces;
  # pure HTTP/Postgres, no shell-out deps). Its own image so it deploys
  # independently of the agora/lesche pair -- see
  # nix/packages/docker-images/files.nix.
  files = buildCrate "cargo build --release -p kallip-files";
  # The host/"tagma" side: the tagma service (agent host + in-process relay
  # connector) and the `kallip` CLI (whose `lesche send` subcommand the agent
  # invokes to address the user) share most of their closure, so one build
  # beats many. The container image (docker-images/tagma.nix) and the dev
  # compose inject KALLIP_SKILLS_SEED explicitly, so this build stays bare
  # -- the seed wrapper lives on `workspace` above.
  # Excludes agora.
  tagma = buildCrate "cargo build --release -p kallip-tagma -p kallip";
  # The timer/notification daemon: fires schedules and injects them into agent
  # conversations via the tagma HTTP API. Separate attrs for the daemon and its
  # management CLI so each can be built/deployed independently.
  cron-daemon = buildCrate "cargo build --release -p kallip-cron-daemon";
  cron = buildCrate "cargo build --release -p kallip-cron";
  # Local daemon family: the stateless instance manager, its
  # kallipctl CLI, and the setuid-candidate spawn helper -- separate attrs
  # so the daemon (operator host) and helper (root-owned install path)
  # never share a deployment unit.
  daemon = buildCrate "cargo build --release -p kallip-daemon";
  ctl = buildCrate "cargo build --release -p kallipctl";
  daemon-spawn = buildCrate "cargo build --release -p kallip-daemon-spawn";
}
