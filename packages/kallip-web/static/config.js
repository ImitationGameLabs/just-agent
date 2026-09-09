// deno-lint-ignore-file no-window
// Factory-default runtime config for the web app, loaded by <script> in
// app.html before the bundle boots. This file ships the self-hosted
// default posture: offline login shows the operator-key branch, and
// every other value derives from the browser location (web.<domain>
// sibling subdomains, or plain-http direct ports). NixOS deployments
// override values through services.kallipai.web.runtimeConfig, which
// bakes over this file; non-NixOS deployments edit it in place.
// The file is served publicly, so anything placed here is readable by
// anyone who can reach the site — keep it to values the browser is
// meant to see; secrets belong in environment files or credential
// stores, never in this file.
window.KALLIP_CONFIG = {
  // domain: "example.com", // sibling-subdomain root (archeion.<domain> ...)
  // tlsOff: false, // true = plain-http direct ports instead of https subdomains
  offlineLogin: true, // true = show the operator-key login branch
  // services: { archeion: "", lesche: "", files: "", instances: "" },
  // Uncomment only with real values: an empty string is not "unset" —
  // the ?? fallback chain never sees past it.
};
