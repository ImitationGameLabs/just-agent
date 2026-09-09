{
  pkgs,
  common,
  workspace,
  advisory-db,
  lib,
}:
let
  inherit (common)
    craneLib
    src
    commonArgs
    cargoArtifacts
    ;

  project = "kallip";
in
{
  # Run clippy (and deny all warnings) on the workspace source
  "${project}-clippy" = craneLib.cargoClippy (
    commonArgs
    // {
      inherit cargoArtifacts;
      cargoClippyExtraArgs = "--all-targets -- --deny warnings";
    }
  );

  # Build docs (default features)
  "${project}-doc" = craneLib.cargoDoc (
    commonArgs
    // {
      inherit cargoArtifacts;
      env.RUSTDOCFLAGS = "--deny warnings";
    }
  );

  # Docs with all features: catches broken intra-doc links inside feature-gated
  # modules (e.g. mock behind `testutils`), which the default-feature
  # check above can't see (those modules aren't compiled then). Keep both: the
  # default check catches links in always-compiled code that point to gated
  # items; this one catches links inside the gated modules.
  "${project}-doc-all-features" = craneLib.cargoDoc (
    commonArgs
    // {
      inherit cargoArtifacts;
      # Repeat `--locked`: overriding cargoExtraArgs replaces crane's default
      # ("--locked"), so it must be re-stated here. --locked asserts Cargo.lock
      # is current (fails instead of silently updating it) for hermetic builds.
      cargoExtraArgs = "--locked --all-features";
      env.RUSTDOCFLAGS = "--deny warnings";
    }
  );

  # Check formatting
  "${project}-fmt" = craneLib.cargoFmt {
    inherit src;
  };

  # TOML formatting
  "${project}-toml-fmt" = craneLib.taploFmt {
    src = pkgs.lib.sources.sourceFilesBySuffices src [ ".toml" ];
  };

  # Audit dependencies for security issues
  "${project}-audit" = craneLib.cargoAudit {
    inherit src advisory-db;
  };

  # Audit licenses
  "${project}-deny" = craneLib.cargoDeny {
    inherit src;
  };

  # Run the test suite. Sandbox env deps are scoped here (not in commonArgs,
  # which the package build and buildDepsOnly also consume):
  # - procps: provides `pgrep` for the process-group reap tests (kill is already
  #   in coreutils).
  # - cacert + SSL_CERT_FILE: reqwest's rustls-platform-verifier loads the system
  #   CA store at client construction; the sandbox has none, so point it at the
  #   nix bundle.
  "${project}-nextest" = craneLib.cargoNextest (
    commonArgs
    // {
      inherit cargoArtifacts;
      nativeBuildInputs = (commonArgs.nativeBuildInputs or [ ]) ++ [
        pkgs.cacert
        pkgs.procps
      ];
      SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      partitions = 1;
      partitionType = "count";
      cargoNextestPartitionsExtraArgs = "--no-tests=pass";
    }
  );

  # Evaluate the NixOS module (pure eval): a stub host with
  # every switch on must typecheck, pass its assertions, and stay
  # warning-free; a drifted port must fire the L1.5 direct-connect
  # warning; and the merged site root must carry the baked runtime
  # config. Builds here: a stub bundle plus the real workspace (pulled
  # in by the bin/ assertion, seconds on a warm store).
  "${project}-nixos-module-eval" =
    let
      stubPackages = {
        ${pkgs.stdenv.hostPlatform.system} = builtins.listToAttrs (
          map
            (name: {
              inherit name;
              # The web stub ships a config.js shell and a page sentinel:
              # the plain site root must pass both through; a baked root
              # must overwrite the former and carry the latter.
              value = pkgs.runCommand "${name}-stub" { } (
                if name == "kallip-web-dist" then
                  ''
                    mkdir $out
                    echo bundle-shell > $out/config.js
                    echo bundle-page > $out/index.html
                  ''
                else
                  "mkdir $out"
              );
            })
            [
              "kallip-daemon"
              "kallip-archeion"
              "kallip-lesche"
              "kallip-files"
              "kallip-instances"
              "kallip-web-dist"
              "workspace"
            ]
        );
      };
      kallipaiModule = import ./nixos-modules.nix { packages = stubPackages; };
      evalHost =
        extraModules:
        lib.nixosSystem {
          system = pkgs.stdenv.hostPlatform.system;
          modules = [
            kallipaiModule
            # Keep the eval quiet and the base assertions green: the stub
            # host pins a stateVersion and a dummy boot layout.
            {
              system.stateVersion = "26.11";
              boot.loader.grub.device = "/dev/null";
              fileSystems."/" = {
                device = "/dev/disk/by-label/stub";
                fsType = "ext4";
              };
            }
            extraModules
          ];
        };
      aligned = evalHost {
        services.kallipai = {
          daemon.enable = true;
          polis.enable = true;
        };
      };
      # A non-default port evaluates clean and warning-free.
      drifted = evalHost {
        services.kallipai = {
          daemon.enable = true;
          polis.enable = true;
          polis.ports.lesche = 7250;
        };
      };
      failedAssertions = attrs: builtins.filter (a: !a.assertion) attrs.config.assertions;
      # The daemon block installs the whole workspace build on PATH.
      workspaceOnPath =
        builtins.elem stubPackages.${pkgs.stdenv.hostPlatform.system}.workspace
          aligned.config.environment.systemPackages;
      # The daemon unit's text must carry the system path on PATH: the
      # daemon resolves its helpers by bare name, and a NixOS unit's
      # PATH is empty unless the unit lists `path` explicitly. Dropping
      # the unit's path line loses the Environment line and this check
      # goes red.
      daemonUnitFile =
        pkgs.writeText "kallip-daemon.service-test"
          aligned.config.systemd.units."kallip-daemon.service".text;
      # The stub only proves module wiring (which attr lands on PATH);
      # the real workspace build must actually ship the binaries the
      # daemon resolves by bare name. Referencing it here puts the real
      # build in this check's closure - the one heavyweight artifact.
      realWorkspace = workspace;
      inherit (import ./lib.nix) bakeRuntimeConfig;
      stubDist = stubPackages.${pkgs.stdenv.hostPlatform.system}."kallip-web-dist";
      # No runtime keys: the site root is the bundle itself.
      webPlain = evalHost {
        services.kallipai.web.enable = true;
      };
      webCustom = evalHost {
        services.kallipai.web = {
          enable = true;
          runtimeConfig = {
            offlineLogin = false;
            domain = "kallipai.lan";
          };
        };
      };
      # An unknown runtimeConfig key must fail the evaluation itself.
      webBogusKey = evalHost {
        services.kallipai.web = {
          enable = true;
          runtimeConfig.oflineLogin = false;
        };
      };
      bogusRejected =
        !(builtins.tryEval webBogusKey.config.services.kallipai.web.distWithRuntimeConfig.outPath).success;
      # The L1.5 drift warning fires for a drifted port with tlsOff
      # pinned and the service unpinned, and stays silent once the
      # service is pinned in runtimeConfig.services.
      driftedWeb = evalHost {
        services.kallipai = {
          daemon.enable = true;
          polis.enable = true;
          polis.ports.lesche = 7250;
          web.enable = true;
          web.runtimeConfig.tlsOff = true;
        };
      };
      pinnedWeb = evalHost {
        services.kallipai = {
          daemon.enable = true;
          polis.enable = true;
          polis.ports.lesche = 7250;
          web = {
            enable = true;
            runtimeConfig = {
              tlsOff = true;
              services.lesche = "http://lesche.example.com";
            };
          };
        };
      };
      # The baking helper, called directly (no module), writes exactly
      # the payload it is given.
      directBake = bakeRuntimeConfig {
        inherit pkgs;
        package = stubDist;
        runtimeConfig = {
          tlsOff = true;
        };
      };
    in
    pkgs.runCommand "${project}-nixos-module-eval" { } ''
      # Forcing leaf options typechecks the module; assertions are
      # checked explicitly (they only throw in the toplevel activation).
      test "${toString aligned.config.services.kallipai.polis.ports.archeion}" = "7100"
      test "${toString aligned.config.services.kallipai.polis.ports.lesche}" = "7200"
      test "${toString (builtins.length (failedAssertions aligned))}" = "0"
      test "${toString (builtins.length aligned.config.warnings)}" = "0"
      test "${toString workspaceOnPath}" = "1"
      # The real workspace build must actually ship the helpers; a stub
      # mistakenly passed here would fail these two lines immediately.
      test -x "${realWorkspace}/bin/kallip-tagma"
      test -x "${realWorkspace}/bin/kallip-daemon-spawn"
      # The daemon unit rides the system path on PATH: bare-name
      # helper resolution depends on it (see daemonUnitFile).
      grep -q "${aligned.config.system.path}/bin" "${daemonUnitFile}"
      # The daemon socket literal lives in two places: the module
      # binding (unit env) and the client constant (the probe chain's
      # last leg). Pin both definition lines: editing either side
      # alone turns this check red.
      grep -q 'daemonSocket = "/run/kallipai/daemon.sock";' "${./nixos-modules.nix}"
      grep -q 'SYSTEM_DAEMON_SOCKET: &str = "/run/kallipai/daemon.sock";' "${../crates/daemon/kallip-daemon-common/src/socket.rs}"
      # A polis-only drifted host stays warning-free: the L1.5 drift
      # warning fires at evaluation time only on hosts with the web
      # enabled (driftedWeb below asserts the firing side).
      test "${toString (builtins.length (failedAssertions drifted))}" = "0"
      test "${toString (builtins.length drifted.config.warnings)}" = "0"
      # No runtime keys: the stub bundle passes straight through, shell
      # config.js and page sentinel both untouched.
      plain="${webPlain.config.services.kallipai.web.distWithRuntimeConfig}"
      test "$plain" = "${stubDist}"
      grep -q bundle-shell "$plain/config.js"
      grep -q bundle-page "$plain/index.html"
      # Custom runtime keys bake over the shell, user keys only; the
      # page sentinel still comes through.
      custom="${webCustom.config.services.kallipai.web.distWithRuntimeConfig}"
      grep -q '"offlineLogin":false' "$custom/config.js"
      grep -q '"domain":"kallipai.lan"' "$custom/config.js"
      grep -q bundle-page "$custom/index.html"
      # An unknown runtimeConfig key failed the evaluation itself.
      test "${toString bogusRejected}" = "1"
      # The drift warning fires unpinned and stays silent once the
      # service is pinned.
      test "${toString (builtins.length driftedWeb.config.warnings)}" = "1"
      test "${toString (builtins.length pinnedWeb.config.warnings)}" = "0"
      # The baking helper, called directly, writes exactly the payload.
      direct="${directBake}"
      grep -q '"tlsOff":true' "$direct/config.js"
      test -z "$(grep bundle-shell "$direct/config.js")"
      grep -q bundle-page "$direct/index.html"
      printf %s ok > "$out"
    '';
}
