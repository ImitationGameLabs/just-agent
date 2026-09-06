# kallip-web

The kallipai web app: a SvelteKit SPA (adapter-static) built on the shared
`kallip-ui` package. Part of the JS/TS workspace under `packages/`; see
`docs/frontend-development.md` for the toolchain — everything runs through
`deno task`, never npm/npx.

## Commands

From this directory (`deno task` merges the root tasks with this
package's scripts and resolves the closest match):

- `deno task dev` — vite dev server (:5173, the `web.` edge in the dev Caddyfile)
- `deno task build` — production build into `build/` (adapter-static SPA)
- `deno task check` — svelte-check
- `deno task sync` — resolves to the root sync task, which runs this
  package's prepare hook: svelte-kit sync plus a paraglide recompile

## i18n

Messages live in `../kallip-ui/i18n/project.inlang/messages/<locale>/*.json`.
After adding or changing keys, regenerate the paraglide output from the repo
root with `deno task i18n` (the compiled output is gitignored). The two inlang
plugins are pinned as dev dependencies here and referenced from the project
settings by their `node_modules` paths (resolved relative to the
project.inlang directory, so three levels up reaches the repo root).

## Deployment

- **Dev**: the host vite dev server behind the dev Caddyfile (`web.<domain>`).
- **NixOS**: the flake's `packages.kallip-web-dist` builds the bundle (two
  derivations: a networked deps build and an offline vite build), and
  `services.kallipai.web` serves it through caddy with an index.html fallback.
  See `docs/reference/container.md`.
