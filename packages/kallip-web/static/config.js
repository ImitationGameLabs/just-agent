// Runtime deployment config for the web app, loaded by <script> in
// deno-lint-ignore-file no-window
// app.html before the bundle boots. Keep this file as the empty shell
// for same-origin deployments: every value then derives from the
// browser location (web.<domain> sibling subdomains, or plain-http
// direct ports). Deployments that need explicit values edit this file
// in place, or (NixOS module) get it rewritten from module options via
// a Caddy handle on /config.js.
window.KALLIP_CONFIG = {
  // domain: "example.com", // sibling-subdomain root (archeion.<domain> ...)
  // tlsOff: false, // true = plain-http direct ports instead of https subdomains
  // offlineLogin: false, // true = show the operator-key login branch
  // services: { archeion: "", lesche: "", files: "", instances: "" },
  // Uncomment only with real values: an empty string is not "unset" —
  // the ?? fallback chain never sees past it.
};
