# The kallip-web static site (SvelteKit adapter-static SPA), built with the
# repo's deno-first toolchain inside the sandbox.
#
# Two derivations, per the standard Nix pattern for npm-style dependency
# trees (cf. fetchNpmDeps): a fixed-output "deps" derivation runs
# `deno install` (networked sandbox, output pinned by hash against
# deno.lock) and a pure build derivation reuses its node_modules offline.
#
# The deps derivation unpacks the cleaned source for its manifests and
# i18n messages, and a deno.lock change re-locks the hash.
{
  pkgs,
  deno,
  # The repository source (flake `self`), cleaned of gitignored build state.
  src,
  # One universal dist: deployment values (domain, TLS shape, offline
  # login) are runtime config — see the web app's /config.js and the
  # services.kallipai.web module's runtimeConfig option.
}:
let
  # Local build state must not leak into the source closure: the deps
  # derivation produces its own node_modules.
  filteredSrc = pkgs.lib.cleanSourceWith {
    inherit src;
    filter =
      path: type:
      !(
        type == "directory"
        && builtins.elem (baseNameOf path) [
          "node_modules"
          "build"
          ".svelte-kit"
          ".git"
          "target"
          "dev-certs"
        ]
      );
  };

  # Both derivations unpack the same cleaned source tree; the deps one
  # only consumes the manifests from it.
  nodeModules = pkgs.stdenvNoCC.mkDerivation {
    pname = "kallip-web-node-modules";
    version = "0.0.1";

    impureEnvVars = pkgs.lib.fetchers.proxyImpureEnvVars;

    outputHashMode = "recursive";
    outputHash = "sha256-8uZLoX6sCldIHxBIw1rYln5bESp98dzQv8JoQZnDJLI=";
    outputHashAlgo = "sha256";

    nativeBuildInputs = [ deno ];

    src = filteredSrc;
    # The output is an intermediate: keep the link graph exactly as deno
    # wrote it (fixup's shebang patching would materialize .bin symlinks).
    dontFixup = true;

    buildPhase = ''
      export HOME=$TMPDIR
      # hoisted: npm's classic flat layout, which the bundler's node-style
      # dependency resolution handles without deno-specific link magic.
      deno install --frozen --node-modules-linker=hoisted
      # The paraglide messages are compiled in the dist derivation, not
      # here: a fixed-output derivation reruns only when its declared
      # hash changes, so i18n edits would silently keep the stale
      # messages. Compiling downstream puts them in the dist derivation's
      # regular input graph instead.
    '';

    installPhase = ''
      mkdir $out
      cp -a node_modules $out/node_modules
    '';
  };
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "kallip-web-dist";
  version = "0.0.1";

  src = filteredSrc;

  nativeBuildInputs = [ deno ];

  configurePhase = ''
    export HOME=$TMPDIR
    cp -a ${nodeModules}/node_modules node_modules
    chmod -R u+w node_modules
    # Point the workspace entries back at this tree's live sources (the
    # copies from the deps derivation point into its own store path).
    mkdir -p node_modules/@kallipai
    for pkg in packages/*; do
      name="$(basename "$pkg")"
      rm -rf "node_modules/@kallipai/$name"
      ln -s "../../$pkg" "node_modules/@kallipai/$name"
    done
    # .bin launchers are plain shims whose relative imports only resolve at
    # the package's real location; re-point the one the build invokes.
    ln -sf ../vite/bin/vite.js node_modules/.bin/vite
  '';

  buildPhase = ''
    export HOME=$TMPDIR
    # Compile the paraglide messages first: the inlang plugins load from
    # this tree's node_modules per the project's settings.json, and the
    # compile sits in this derivation's regular input graph, so i18n
    # edits retrigger the bundle.
    (cd packages/kallip-web
    deno run -A --frozen ../../node_modules/@inlang/paraglide-js/bin/run.js compile \
      --project ../kallip-ui/i18n/project.inlang \
      --strategy cookie preferredLanguage baseLocale \
      --emit-ts-declarations \
      --output-structure message-modules \
      --outdir ../kallip-ui/src/paraglide
    )
    deno task build -- --configLoader runner
  '';

  installPhase = ''
    cp -r packages/kallip-web/build $out
  '';
}
