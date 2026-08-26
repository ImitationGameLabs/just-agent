<script lang="ts">
  // The provider-vault section: the user's stored API keys, plus the add
  // form. Prop-driven like PasskeyManager -- the owning page maps the agora
  // store into these props and wires the mutations (which throw on failure;
  // the store never blanks the list). Encryption affordances are gated by
  // `canFlip` (the page passes the passkey-session marker).
  import type { ProviderSummary } from "@kallipai/kallip-agora-client";
  import {
    settings_providers,
    settings_provider_none,
    settings_provider_add,
    settings_provider_intro,
    common_loading,
  } from "../../paraglide/messages.js";
  import ProviderVaultCard from "./ProviderVaultCard.svelte";
  import ProviderVaultForm from "./ProviderVaultForm.svelte";

  let {
    entries,
    phase,
    error = null,
    canFlip = false,
    onRename,
    onFlip,
    onDelete,
    onCreate,
    onCopyKey,
  }: {
    entries: ProviderSummary[];
    // "loading" until the first list fetch lands; "error" keeps the list.
    phase: "loading" | "loaded" | "error";
    error?: string | null;
    canFlip?: boolean;
    onRename?: (id: string, name: string) => Promise<void> | void;
    onFlip?: (entry: ProviderSummary) => Promise<void> | void;
    onDelete?: (id: string) => Promise<void> | void;
    onCreate?: (req: {
      name: string;
      provider: string;
      base_url: string | null;
      key_material: string;
      mode: "plaintext" | "encrypted";
    }) => Promise<boolean> | boolean | void;
    onCopyKey?: (entry: ProviderSummary) => Promise<string | null>;
  } = $props();

  let formOpen = $state(false);
</script>

<section class="space-y-3">
  <div class="flex items-center justify-between gap-2">
    <h2 class="text-sm font-medium uppercase opacity-60 tracking-wide">
      {settings_providers()}
    </h2>
    <button
      class="btn btn-sm preset-tonal-surface"
      onclick={() => (formOpen = !formOpen)}
    >
      {formOpen ? "–" : settings_provider_add()}
    </button>
  </div>

  <p class="text-xs opacity-60">{settings_provider_intro()}</p>

  {#if error}
    <div class="text-xs text-error-600 dark:text-error-500">{error}</div>
  {/if}

  {#if phase === "loading"}
    <p class="text-sm opacity-60">{common_loading()}</p>
  {:else if entries.length === 0}
    <p class="text-sm opacity-60">{settings_provider_none()}</p>
  {:else}
    <ul class="space-y-2">
      {#each entries as entry (entry.id)}
        <ProviderVaultCard
          {entry}
          {canFlip}
          {onRename}
          {onFlip}
          {onDelete}
          {onCopyKey}
        />
      {/each}
    </ul>
  {/if}

  <ProviderVaultForm
    open={formOpen}
    {canFlip}
    {onCreate}
    onClosed={() => (formOpen = false)}
  />
</section>
