<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import "../app.css";
  import {
    RootLayout,
    initShell,
    initArcheion,
    initConfigStorage,
    initFiles,
    initInstances,
    initLesche,
    localStorageConfigStorage,
    type NavIcons,
  } from "@kallipai/kallip-ui";
  import {
    Calendar,
    Cpu,
    Folder,
    House,
    LayoutGrid,
    MessageSquare,
    Settings,
    Users,
    Wallet,
  } from "@lucide/svelte";

  // Inject the app's navigation, archeion/lesche URLs, and storage backend into
  // kallip-ui. The shared <RootLayout> consumes these ports (it cannot import
  // $app/* or import.meta.env from inside the library package). Idempotent
  // setters; the root layout has a single instance so this runs once at boot.
  initShell(goto);
  // Service URLs resolve at runtime, in three layers: the deployment
  // config from /config.js (window.KALLIP_CONFIG, rewritten per
  // deployment by the NixOS module; empty shell by default), then the
  // build-time VITE_*_URL overrides, then derivation from the browser
  // location — the origin it is on names the deployment domain, so a
  // same-origin deployment (web.<domain> sibling subdomains, or the
  // plain-http direct-port shape) needs zero configuration.
  // The https shape is fronted by Caddy: the browser reaches
  // archeion/lesche at their *.<domain> subdomains. Any other protocol
  // means the plain-http shape: direct ports on the host.
  const config = window.KALLIP_CONFIG ?? {};
  const tlsOff = config.tlsOff ?? location.protocol !== "https:";
  const domain = (config.domain ?? location.hostname).replace(/^web\./, "");
  initArcheion(
    config.services?.archeion ??
      import.meta.env.VITE_ARCHEION_URL ??
      (tlsOff ? `http://${domain}:7100` : `https://archeion.${domain}`),
  );
  initLesche(
    config.services?.lesche ??
      import.meta.env.VITE_LESCHE_URL ??
      (tlsOff ? `http://${domain}:7200` : `https://lesche.${domain}`),
  );
  initFiles(
    config.services?.files ??
      import.meta.env.VITE_FILES_URL ??
      (tlsOff ? `http://${domain}:7400` : `https://files.${domain}`),
  );
  initInstances(
    config.services?.instances ??
      import.meta.env.VITE_INSTANCES_URL ??
      (tlsOff
        ? `http://${domain}:7300/api/instances`
        : `https://instances.${domain}/api/instances`),
  );
  initConfigStorage(localStorageConfigStorage);

  const icons: NavIcons = {
    chat: MessageSquare,
    tagmata: Cpu,
    rooms: Users,
    settings: Settings,
    manageOverview: LayoutGrid,
    home: House,
    manageBudget: Wallet,
    manageAgents: Users,
    manageProfiles: Settings,
    manageSchedules: Calendar,
    files: Folder,
  };

  let { children } = $props();
</script>

<RootLayout
  pathname={page.url.pathname}
  search={page.url.search}
  appKind="web"
  {icons}
>
  {@render children()}
</RootLayout>
