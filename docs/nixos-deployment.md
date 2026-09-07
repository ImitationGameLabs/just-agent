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
  `lesche.`, `files.`, `instances.`, and optionally `web.`) at the host.
  On a public domain with ports 80 and 443 reachable, Caddy obtains
  certificates automatically; on a private domain like the `kallipai.lan`
  example below, ACME cannot issue and Caddy falls back to its local CA
  (see the TLS section).
- Root access (to rebuild the host and to read the generated admin
  token at first login).

## Import the module

The module ships as the flake output `nixosModules.kallipai` (also
exported as `nixosModules.default`). Add the repository as a flake input
and import the module into your host:

```nix
{
  inputs.kallipai.url = "github:ImitationGameLabs/kallipai";

  outputs = { nixpkgs, ... }@inputs: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        inputs.kallipai.nixosModules.kallipai
        ./configuration.nix
      ];
    };
  };
}
```

## The internal token (self-managed)

The polis services authenticate to each other with one shared secret,
and the archeion owns its lifecycle: on first boot it generates the
secret into its state directory
(`/var/lib/kallipai/archeion/internal-token`, mode 0640, readable by
the `kallipai-polis` group the module creates), and on every later
boot it reads the existing value — a token that exists is never
rewritten. The lesche, files, and instances services read the same
file, so all four agree on one value for the lifetime of the
deployment.

Secret state is filed by lifetime: `/etc` holds administrator-owned
static configuration (a pinned admin token), `/run` holds volatile
runtime state (the auto-generated admin token, reset on every service
restart), and `/var/lib` holds service-owned persistent state (the
internal token, stable across restarts).

To rotate the internal token: stop the four polis services, delete the
file, start the archeion (a fresh value is generated), then start the
other three. The group restart keeps every service on the same
generation.

## Minimal configuration

A minimal full-platform configuration — the daemon, the four polis
services, the reverse proxy on one domain, and the web app. This is
the `configuration.nix` the flake above imports:

```nix
{
  services.kallipai.daemon.enable = true;
  services.kallipai.polis = {
    enable = true;
    proxy = {
      enable = true;
      domain = "kallipai.lan";
      # acmeEmail = "you@example.org";  # optional: ACME recovery address,
      # only meaningful on a public domain (a .lan domain uses the local CA)
    };
  };
  services.kallipai.web = {
    enable = true;
    domain = "kallipai.lan";
  };
}
```

Both `domain` values are the base domain (`kallipai.lan`): the proxy
derives the four service subdomains (`archeion.`, `lesche.`, `files.`,
`instances.`) from it, and the web app serves on `web.<domain>`. The
module provisions its own PostgreSQL — one database per stateful
service (archeion, lesche, files), peer-authenticated over the unix
socket — so no database setup is needed. The daemon supervises tagma
instances over a local control socket and is consumed by `kallipctl`
and the instances proxy.

The package options default to this flake's build for the host system;
set one explicitly to pin a specific version. With `adminTokenFile`
unset, the archeion mints a fresh admin
token on every start into its runtime directory
(`/run/kallipai/archeion/admin-token.env`, mode 0600) and logs only the
path — the value never appears in the journal. Read it for the first
login with:

```sh
sudo cat /run/kallipai/archeion/admin-token.env
```

That token is a short-lived bootstrap credential: rewritten on every
restart, and sessions minted with it live in the database and survive
token rotation. A permanent deployment pins the
file instead (the `adminTokenFile` option description covers the file
format and the OAuth client secrets it can carry) — pin for a stable
token, leave unset to accept a short-lived one.

## Ports

The four listeners bind localhost on 7100 (archeion), 7200 (lesche),
7400 (files), and 7300 (instances). Override any of them under
`services.kallipai.polis.ports.<service>` (1024-65535; the four values
must be distinct — the module fails evaluation otherwise). If you
change a port and serve the web app in its direct-connect form —
browsers reach the polis services directly instead of through the
proxy, the other of the app's two serving shapes (see the
`runtimeConfig` option description) — pin the new port for the UI with
`services.kallipai.web.runtimeConfig.services.<service>`.

## HTTPS on a LAN or home network

Without reachable ports 80 and 443, ACME cannot issue certificates. On
a private network, Caddy's `tls internal` directive issues certificates
from Caddy's own local CA instead; browsers show a warning until the
host trusts that CA.

The module writes each virtual host's site block. Append the directive
by setting `extraConfig` for the same host in your own configuration —
the option is a `types.lines` value, so both definitions concatenate
into one site block:

```nix
services.caddy.virtualHosts."archeion.kallipai.lan".extraConfig = ''
  tls internal
'';
```

Repeat for the other subdomains — `lesche.`, `files.`, `instances.`,
and `web.` — if every subdomain is to serve https. The internal CA's
root certificate appears after Caddy's first start at
`/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt`
(confirm it with `ls` on the target host). Distribute trust from there:

- Firefox keeps its own trust store, separate from the system bundle —
  import the file manually as an authority (Privacy & Security →
  Certificates → View Certificates → Authorities → Import) and do not
  rely on OS-level trust reaching it.
- For command-line tools (curl, git), copy the root into your
  configuration tree and add it to the system trust:

```sh
cp /var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt \
  /etc/nixos/kallipai-root.crt
```

```nix
security.pki.certificateFiles = [ ./kallipai-root.crt ];
```

The copy exists because the CA generates its root at runtime, while
`security.pki.certificateFiles` is read while the system trust bundle
is built — a direct reference to the `/var/lib` path cannot resolve.

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

Then confirm each subdomain answers over https — `archeion.kallipai.lan`
for sign-up and login, `web.kallipai.lan` for the app, and the lesche,
files, and instances subdomains through the same proxy. A healthy
deployment: the app loads, you can sign up and sign in, and you can
create a first agent.

A missing internal-token file is not an error on the archeion's first
boot — it generates one. The lesche, files, and instances units require
the archeion and read that file at boot; if it has not appeared within a
short grace window the unit refuses to start, and
`journalctl -u kallip-instances` shows the path it waited for. An empty
token file fails every reader explicitly — delete the file to
re-provision rather than editing it by hand.

## Further options

The full option surface — per-service tuning, the web `runtimeConfig`,
`adminTokenFile` and `notifyTokenFile` — is described in the option
declarations in `nix/nixos-modules.nix`, with an option-level summary in
the polis and web NixOS module sections of
[container.md](reference/container.md).
