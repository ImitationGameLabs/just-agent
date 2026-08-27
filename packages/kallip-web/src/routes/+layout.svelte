<script lang="ts">
  import { goto } from "$app/navigation";
  import { page } from "$app/state";
  import "../app.css";
  import {
    RootLayout,
    initShell,
    initAgora,
    initConfigStorage,
    initInstances,
    initLesche,
    localStorageConfigStorage,
    type NavIcons,
  } from "@kallipai/kallip-ui";
  import {
    Calendar,
    Cpu,
    House,
    LayoutGrid,
    MessageSquare,
    Settings,
    Users,
    Wallet,
  } from "@lucide/svelte";

  // Inject the app's navigation, agora/lesche URLs, and storage backend into
  // kallip-ui. The shared <RootLayout> consumes these ports (it cannot import
  // $app/* or import.meta.env from inside the library package). Idempotent
  // setters; the root layout has a single instance so this runs once at boot.
  initShell(goto);
  // The https shape is fronted by Caddy: the browser reaches agora/lesche at
  // their *.<devDomain> subdomains. KALLIP_TLS=off (injected alongside the
  // domain by vite.config.ts) selects the plain-http shape: direct ports on
  // the host. Explicit VITE_*_URL values still win in either shape.
  const tlsOff = import.meta.env.KALLIP_TLS === "off";
  const devDomain = import.meta.env.KALLIP_DOMAIN ?? "kallipai.com";
  initAgora(
    import.meta.env.VITE_AGORA_URL ??
      (tlsOff ? `http://${devDomain}:7100` : `https://agora.${devDomain}`),
  );
  initLesche(
    import.meta.env.VITE_LESCHE_URL ??
      (tlsOff ? `http://${devDomain}:7200` : `https://lesche.${devDomain}`),
  );
  initInstances(
    import.meta.env.VITE_INSTANCES_URL ??
      (tlsOff
        ? `http://${devDomain}:7300/api/instances`
        : `https://instances.${devDomain}/api/instances`),
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
