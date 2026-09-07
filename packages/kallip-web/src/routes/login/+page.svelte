<script lang="ts">
  import { page } from "$app/state";
  import { LoginPage } from "@kallipai/kallip-ui";

  // Honor the `?next=` set by the auth gate so a deep link returns after login.
  const returnPath = $derived(
    new URLSearchParams(page.url.search).get("next") ?? undefined,
  );
  // Runtime deployment flag from /config.js (see app.d.ts): a self-hosted
  // deployment sets offlineLogin in its rewritten config.js and shows the
  // operator-key branch, a cloud deployment never does (two-way
  // information hiding). The universal dist defaults to hidden.
  const offlineLogin = window.KALLIP_CONFIG?.offlineLogin === true;
</script>

<LoginPage {returnPath} {offlineLogin} />
