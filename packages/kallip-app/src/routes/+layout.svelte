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
    initNotificationBackend,
    initInstances,
    initLesche,
    localStorageConfigStorage,
    type NavIcons,
  } from "@kallipai/kallip-ui";
  import { tauriNotificationBackend } from "../lib/tauri-notification.ts";
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
  // NOTE: Tauri swaps localStorageConfigStorage for a secure-storage adapter
  // once the plugin is wired. The WebAuthn passkey ceremony in this webview is
  // gated on Tauri webview origin support.
  initShell(goto);
  // Desktop-surface boundary: this app runs in the Tauri webview on the
  // user's own machine, so falling back to localhost direct ports is
  // the dev-local default here (VITE_*_URL overrides point it
  // elsewhere). This is not a browser fallback: the web app served
  // from a deployment derives its sibling-subdomain URLs from the
  // page location and never reaches these defaults.
  //
  // Service URLs on this surface: explicit VITE_*_URL build-time
  // overrides, then localhost direct ports.
  initArcheion(import.meta.env.VITE_ARCHEION_URL ?? "http://localhost:7100");
  initLesche(import.meta.env.VITE_LESCHE_URL ?? "http://localhost:7200");
  initFiles(import.meta.env.VITE_FILES_URL ?? "http://localhost:7400");
  initConfigStorage(localStorageConfigStorage);
  initNotificationBackend(tauriNotificationBackend);
  initInstances(
    import.meta.env.VITE_INSTANCES_URL ?? "http://localhost:7300/api/instances",
  );

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
  appKind="app"
  {icons}
>
  {@render children()}
</RootLayout>
