// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
  namespace App {
    // interface Error {}
    // interface Locals {}
    // interface PageData {}
    // interface PageState {}
    // interface Platform {}
  }
  interface Window {
    /**
     * Runtime deployment config, loaded from /config.js before the app
     * bundle (see static/config.js for the factory default; the NixOS
     * module bakes over it from runtimeConfig). Every field is
     * optional, and each layers differently: services.* URLs fall
     * through to the build-time VITE_*_URL overrides and then to
     * location-based derivation; domain and tlsOff fall through to
     * location derivation only; offlineLogin defaults to true when
     * unset (the factory file ships it as true).
     */
    KALLIP_CONFIG?: {
      /** Sibling-subdomain root (archeion.<domain> ...). */
      domain?: string;
      /** True = plain-http direct ports instead of https subdomains. */
      tlsOff?: boolean;
      /** True = show the operator-key login branch. */
      offlineLogin?: boolean;
      /** Full URL overrides, one per backend service. */
      services?: {
        archeion?: string;
        lesche?: string;
        files?: string;
        instances?: string;
      };
    };
  }
}

export {};
