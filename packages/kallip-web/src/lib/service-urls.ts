/**
 * Service URL derivation for the web app, in two layers:
 *
 * 1. Explicit override — `config.services[name]` when the runtime
 *    config carries a value for the service.
 * 2. Derivation — the sibling subdomain of the deployment domain,
 *    following the page's own protocol and port: an https page on the
 *    default port reaches `https://archeion.<domain>`, while a page on
 *    a non-default port (the dev edge on :8080, say) reaches
 *    `http://archeion.<domain>:8080` — the edge listens there, so the
 *    sibling routes carry it too.
 *
 * The deployment domain is the config's `domain` when set (taken
 * verbatim, `web.` prefix included), otherwise the page's own hostname
 * with the `web.` prefix stripped — the app is served at
 * `web.<domain>`, and its origin names the deployment.
 */

export type ServiceName = "archeion" | "lesche" | "files" | "instances";

/** The subset of `window.KALLIP_CONFIG` this derivation consumes. */
export interface DerivationConfig {
  domain?: string;
  services?: Partial<Record<ServiceName, string>>;
}

/** The parts of the browser location the derivation reads. */
export interface PageLocation {
  protocol: string;
  hostname: string;
  port?: string;
}

/** Derive the URL for one backend service from the config and the page. */
export function serviceUrl(
  name: ServiceName,
  config: DerivationConfig,
  page: PageLocation,
): string {
  const override = config.services?.[name];
  if (override !== undefined) {
    return override;
  }
  const domain = config.domain ?? page.hostname.replace(/^web\./, "");
  const path = name === "instances" ? "/api/instances" : "";
  const defaultPort = page.protocol === "https:" ? "443" : "80";
  const port = page.port && page.port !== defaultPort ? `:${page.port}` : "";
  return `${page.protocol}//${name}.${domain}${port}${path}`;
}
