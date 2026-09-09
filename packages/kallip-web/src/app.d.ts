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
     * optional, and each layers differently: services.* URLs and the
     * domain override location-based derivation; offlineLogin
     * defaults to true when unset (the factory file ships it as true).
     */
    KALLIP_CONFIG?: {
      /** Sibling-subdomain root (archeion.<domain> ...). */
      domain?: string;
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
