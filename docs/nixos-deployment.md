# NixOS deployment

This guide walks through deploying the kallipai platform form on a NixOS
host: the daemon as a system service, the four polis services behind the
host's reverse proxy, and the web app. The container-based deployments
(arion compose) are covered in [container.md](reference/container.md);
this guide is the step-by-step bring-up for the NixOS module form. The
module itself is `nix/nixos-modules.nix`, and its option descriptions are
the authoritative reference for everything this guide summarizes.

## Prerequisites

- A NixOS host with flakes enabled
  (`nix.settings.experimental-features = [ "nix-command" "flakes" ]`).
- For the proxied shape: DNS records pointing the subdomains (`archeion.`,
  `lesche.`, `files.`, `instances.`, and optionally `web.`) at the host,
  with ports 80 and 443 reachable — Caddy obtains public certificates
  automatically.
- Root access (the token file below is root-only).

## Import the module

The module ships as the flake output `nixosModules.kallipai` (also
exported as `nixosModules.default`). Add the repository as a flake input
and import the module into your host; the `specialArgs` line passes the
flake inputs through, so the configuration below can reference the
kallipai packages:

```nix
{
  inputs.kallipai.url = "github:ImitationGameLabs/kallipai";

  outputs = { nixpkgs, ... }@inputs: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      specialArgs = { inherit inputs; };
      modules = [
        inputs.kallipai.nixosModules.kallipai
        ./configuration.nix
      ];
    };
  };
}
```

## Create the internal token file

The polis services authenticate to each other with one shared secret.
`internalTokenFile` points at a root-only EnvironmentFile that must
define four keys carrying the same value:

```sh
KALLIP_ARCHEION_INTERNAL_TOKEN=<same-random-value>
KALLIP_LESCHE_ARCHEION_TOKEN=<same-random-value>
KALLIP_FILES_ARCHEION_TOKEN=<same-random-value>
KALLIP_INSTANCES_ARCHEION_INTERNAL_TOKEN=<same-random-value>
```

Create the file and paste the four lines (a fresh deployment writes all
four keys in one go; an existing deployment that already runs an earlier
token file adds the fourth key — the instances service fails closed and
refuses to start without it):

```sh
sudo install -m 0600 /dev/null /etc/kallipai/polis-internal-tokens
sudoedit /etc/kallipai/polis-internal-tokens
```

The format is systemd's line-based `KEY=value`. Avoid `#`, quotes, and
leading whitespace in the value — any of these breaks the parse.

## Minimal configuration

A minimal full-platform configuration — the daemon, the four polis
services, the reverse proxy on one domain, and the web app:

```nix
{ inputs, ... }:
{
  services.kallipai.daemon = {
    enable = true;
    package = inputs.kallipai.packages.x86_64-linux.kallip-daemon;
  };
  services.kallipai.polis = {
    enable = true;
    archeionPackage = inputs.kallipai.packages.x86_64-linux.kallip-archeion;
    leschePackage = inputs.kallipai.packages.x86_64-linux.kallip-lesche;
    filesPackage = inputs.kallipai.packages.x86_64-linux.kallip-files;
    instancesPackage = inputs.kallipai.packages.x86_64-linux.kallip-instances;
    internalTokenFile = "/etc/kallipai/polis-internal-tokens";
    proxy = {
      enable = true;
      domain = "example.com";
      # acmeEmail = "acme@example.com";  # optional: ACME recovery address
    };
  };
  services.kallipai.web = {
    enable = true;
    package = inputs.kallipai.packages.x86_64-linux.kallip-web-dist;
    domain = "example.com";
  };
}
```

The package options carry no default — pinning stays with the consumer
flake. With `adminTokenFile` unset, the archeion generates a fresh admin
token at every boot and prints it once to the journal; read it with
`journalctl -u kallip-archeion` for the first login. A permanent
deployment pins the file instead (see its option description for the
format and the OAuth client secrets it can carry).

## Ports

The four listeners bind localhost on 7100 (archeion), 7200 (lesche),
7400 (files), and 7300 (instances). Override any of them under
`services.kallipai.polis.ports.<service>` (1024-65535; the four values
must be distinct — the module fails evaluation otherwise). If you change
a port and serve the web app in its direct-connect form, pin the new
port for the UI with
`services.kallipai.web.runtimeConfig.services.<service>`.

## Deploy and verify

Switch into the new generation:

```sh
sudo nixos-rebuild switch
```

Check the five units:

```sh
systemctl status kallip-daemon kallip-archeion kallip-lesche \
  kallip-files kallip-instances
```

Then confirm each subdomain answers over https — `archeion.example.com`
for sign-up and login, `web.example.com` for the app, and the lesche,
files, and instances subdomains through the same proxy.

A missing or unreadable `internalTokenFile` keeps the affected unit from
starting (the unit fails when the EnvironmentFile cannot be read), and a
token file without the fourth key keeps `kallip-instances` from
starting — `journalctl -u kallip-instances` shows the failure.

## Further options

The full option surface — per-service tuning, the web `runtimeConfig`,
`adminTokenFile` and `notifyTokenFile` — is described in the option
declarations in `nix/nixos-modules.nix`, with an option-level summary in
the polis and web NixOS module sections of
[container.md](reference/container.md).
