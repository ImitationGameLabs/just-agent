<script lang="ts">
  // Read-only instance face of the offline home: the daemon proxy supplies
  // machine-level liveness and the instance list; spawn/stop join later.
  import { instancesStore } from "../../lib/daemon/instances.svelte.ts";
  import {
    manage_instances_title,
    manage_instances_heading,
    manage_instances_daemon_health,
    manage_instances_daemon_running,
    manage_instances_daemon_stopped,
    manage_instances_running,
    manage_instances_stopped,
    manage_instances_empty,
    manage_instances_unauthorized,
    manage_instances_forbidden,
    manage_instances_unreachable,
    manage_instances_load_failed,
  } from "../../paraglide/messages.js";

  $effect(() => {
    instancesStore.startPolling(5000);
    return () => instancesStore.stopPolling();
  });

  // One human line per classified failure kind (token UX lands with the
  // write batch; for now the 401 just explains itself).
  const kindMessage = {
    unauthorized: manage_instances_unauthorized,
    forbidden: manage_instances_forbidden,
    unreachable: manage_instances_unreachable,
    other: manage_instances_load_failed,
  } as const;
</script>

<svelte:head><title>{manage_instances_title()}</title></svelte:head>

<div class="h-full overflow-y-auto">
  <div class="p-6 max-w-2xl space-y-6">
    <!-- md+ keeps this h1; below md the shell top row carries the title. -->
    <h1 class="text-xl font-semibold hidden md:block">
      {manage_instances_heading()}
    </h1>

    {#if instancesStore.errorKind}
      <p class="text-error-500 dark:text-error-400 text-sm">
        {kindMessage[instancesStore.errorKind]()}
      </p>
    {:else}
      <section class="card preset-tonal-surface p-5 space-y-1">
        <h2 class="text-sm font-medium">{manage_instances_daemon_health()}</h2>
        {#if instancesStore.health}
          <p
            class="text-sm {instancesStore.health.running
              ? 'text-success-500 dark:text-success-400'
              : 'text-error-500 dark:text-error-400'}"
          >
            {#if instancesStore.health.running}
              {manage_instances_daemon_running()}
            {:else}
              {manage_instances_daemon_stopped()}
            {/if}
            {#if instancesStore.health.detail}
              <span class="text-xs opacity-70">
                · {instancesStore.health.detail}</span
              >
            {/if}
          </p>
        {/if}
      </section>

      <section
        class="card preset-tonal-surface divide-y divide-surface-200-800"
      >
        {#if instancesStore.instances.length === 0}
          <p class="p-5 text-sm opacity-70">{manage_instances_empty()}</p>
        {:else}
          {#each instancesStore.instances as instance (instance.slug)}
            <div class="p-5 flex items-center justify-between gap-4">
              <div class="min-w-0">
                <p class="font-medium truncate">{instance.slug}</p>
                <p class="text-xs opacity-70 truncate">{instance.workspace}</p>
              </div>
              <span
                class={instance.running
                  ? "text-sm text-success-500 dark:text-success-400"
                  : "text-sm opacity-70"}
              >
                {instance.running
                  ? manage_instances_running()
                  : manage_instances_stopped()}
              </span>
            </div>
          {/each}
        {/if}
      </section>
    {/if}
  </div>
</div>
