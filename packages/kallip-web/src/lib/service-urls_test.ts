import { assertEquals } from "@std/assert";
import { serviceUrl } from "./service-urls.ts";

const page = { protocol: "https:", hostname: "web.kallipai.com" };

Deno.test("derives https sibling subdomains from an https page", () => {
  assertEquals(
    serviceUrl("archeion", {}, page),
    "https://archeion.kallipai.com",
  );
  assertEquals(serviceUrl("lesche", {}, page), "https://lesche.kallipai.com");
  assertEquals(serviceUrl("files", {}, page), "https://files.kallipai.com");
  assertEquals(
    serviceUrl("instances", {}, page),
    "https://instances.kallipai.com/api/instances",
  );
});

Deno.test("subdomains follow the page protocol on plain http", () => {
  const http = { protocol: "http:", hostname: "web.kallipai.lan" };
  assertEquals(
    serviceUrl("archeion", {}, http),
    "http://archeion.kallipai.lan",
  );
  assertEquals(
    serviceUrl("instances", {}, http),
    "http://instances.kallipai.lan/api/instances",
  );
});

Deno.test("strips the web. prefix from the page hostname", () => {
  assertEquals(
    serviceUrl(
      "lesche",
      {},
      { protocol: "https:", hostname: "web.example.org" },
    ),
    "https://lesche.example.org",
  );
});

Deno.test("config.domain overrides the hostname-derived domain", () => {
  assertEquals(
    serviceUrl(
      "files",
      { domain: "kallipai.com" },
      {
        protocol: "http:",
        hostname: "localhost",
      },
    ),
    "http://files.kallipai.com",
  );
});

Deno.test("explicit service override wins over derivation", () => {
  assertEquals(
    serviceUrl(
      "archeion",
      { services: { archeion: "http://10.0.0.7:7100" } },
      page,
    ),
    "http://10.0.0.7:7100",
  );
});
