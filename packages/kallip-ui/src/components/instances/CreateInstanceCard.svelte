<script lang="ts">
  import {
    agoraBaseUrlOrFail,
    agoraSession,
    lescheBaseUrlOrFail,
  } from "../../lib/session/agora.svelte";
  import { instancesStore } from "../../lib/instances/instances.svelte.ts";
  import FormError from "../FormError.svelte";
  import {
    manage_instances_create_creating,
    common_create,
    manage_instances_create_failed,
    manage_instances_create_heading,
    manage_instances_create_mint_failed,
    manage_instances_create_workspace_label,
    manage_instances_create_workspace_placeholder,
    manage_instances_spawn_success,
    nav_chat,
  } from "../../paraglide/messages.js";

  // One-click create: mint an enrollment code on the agora, then spawn the
  // instance with the relay environment pointing at this deployment, so the
  // tagma enrolls itself on first start. The workspace is the only input --
  // the daemon requires an existing directory and cannot invent one.
  let workspace = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let spawned = $state<{ slug: string; port: number } | null>(null);

  const canSubmit = $derived(workspace.trim().length > 0 && !busy);

  async function create(e: Event) {
    e.preventDefault();
    if (!canSubmit) return;
    busy = true;
    error = null;
    spawned = null;
    try {
      const minted = await agoraSession.mintTagma();
      if (!minted) {
        // The registry section above carries the mint failure's own error
        // line; this hint keeps the create form from failing silently.
        error = manage_instances_create_mint_failed();
        return;
      }
      const slug = `tagma-${minted.id.slice(0, 8)}`;
      const env = [
        `KALLIP_TAGMA_RELAY_AGORA_URL=${agoraBaseUrlOrFail()}`,
        `KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=${minted.code}`,
        `KALLIP_TAGMA_RELAY_LESCHE_URL=${lescheBaseUrlOrFail()}`,
      ];
      try {
        const result = await instancesStore.spawn({
          slug,
          workspace: workspace.trim(),
          env,
        });
        spawned = { slug: result.slug, port: result.port };
        workspace = "";
      } catch (cause) {
        // The minted code stays valid (the pending card above shows it with
        // its expiry); the manual spawn form below can redeem it by hand.
        console.error("[instances] one-click spawn failed:", cause);
        error = manage_instances_create_failed();
      }
    } finally {
      busy = false;
    }
  }
</script>

<section
  class="space-y-4 p-4 bg-surface-100-900 border border-surface-200-800 rounded-xl"
>
  <h2 class="text-lg font-medium">{manage_instances_create_heading()}</h2>
  <form class="space-y-3" onsubmit={create}>
    <label class="block space-y-1">
      <span class="text-sm opacity-70">
        {manage_instances_create_workspace_label()}
        <span class="text-error-500 dark:text-error-400">*</span>
      </span>
      <input
        class="input"
        type="text"
        placeholder={manage_instances_create_workspace_placeholder()}
        bind:value={workspace}
        required
      />
    </label>
    {#if error}
      <FormError message={error} />
    {/if}
    {#if spawned}
      <p class="text-sm text-success-500 dark:text-success-400">
        {manage_instances_spawn_success({
          slug: spawned.slug,
          port: spawned.port,
        })}
        <a
          class="underline underline-offset-2 ml-1"
          href={"/connect?tagmaUrl=http://127.0.0.1:" + spawned.port}
          >{nav_chat()}</a
        >
      </p>
    {/if}
    <button
      type="submit"
      class="btn preset-filled-primary-500 w-full"
      disabled={!canSubmit}
    >
      {busy ? manage_instances_create_creating() : common_create()}
    </button>
  </form>
</section>
