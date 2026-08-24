<script lang="ts">
  // Read-only instance face of the offline home: the daemon proxy supplies
  // list; the write face (spawn form, stop dialog, token entry) lives here.
  import { instancesStore } from "../../lib/daemon/instances.svelte.ts";
  import {
    CONNECT_TOKEN_KEY,
    DAEMON_TOKEN_KEY,
    DaemonWebError,
  } from "../../lib/daemon/client.ts";
  import ConfirmDialog from "../../components/ConfirmDialog.svelte";
  import FormError from "../../components/FormError.svelte";
  import {
    manage_instances_title,
    manage_instances_heading,
    manage_instances_daemon_health,
    manage_instances_daemon_running,
    manage_instances_daemon_stopped,
    manage_instances_running,
    manage_instances_stopped,
    manage_instances_empty,
    manage_instances_loading,
    manage_instances_unauthorized,
    manage_instances_token_label,
    manage_instances_token_placeholder,
    manage_instances_token_apply,
    manage_instances_forbidden,
    manage_instances_host_forbidden,
    manage_instances_unreachable,
    manage_instances_load_failed,
    manage_instances_spawn_heading,
    manage_instances_spawn_slug_label,
    manage_instances_spawn_slug_placeholder,
    manage_instances_spawn_workspace_label,
    manage_instances_spawn_workspace_placeholder,
    manage_instances_spawn_agora_label,
    manage_instances_spawn_enrollment_label,
    manage_instances_spawn_lesche_label,
    manage_instances_spawn_token_label,
    manage_instances_spawn_llm_provider_label,
    manage_instances_spawn_llm_model_label,
    manage_instances_spawn_llm_api_key_label,
    manage_instances_spawn_submit,
    manage_instances_spawn_success,
    nav_chat,
    manage_instances_stop,
    manage_instances_stop_title,
    manage_instances_stop_description,
    manage_instances_error_slug_taken,
    manage_instances_error_workspace_overlap,
    manage_instances_error_invalid_spawn_input,
    manage_instances_error_spawn_timeout,
    manage_instances_error_not_found,
    manage_instances_error_not_running,
    manage_instances_error_bad_request,
    manage_instances_error_internal,
  } from "../../paraglide/messages.js";

  $effect(() => {
    instancesStore.startPolling(5000);
    return () => instancesStore.stopPolling();
  });

  // --- spawn form ----------------------------------------------------
  let slug = $state("");
  let workspace = $state("");
  let agoraUrl = $state("");
  let enrollmentCode = $state("");
  let lescheUrl = $state("");
  let instanceToken = $state("");
  let llmProvider = $state("");
  let llmModel = $state("");
  let llmApiKey = $state("");
  let spawnBusy = $state(false);
  let spawnError = $state<string | null>(null);
  let spawnResult = $state<{ slug: string; port: number } | null>(null);

  // --- stop dialog ----------------------------------------------------
  let stopTarget = $state<string | null>(null);
  let stopBusy = $state(false);
  let stopError = $state<string | null>(null);

  // --- standalone-mode token entry (the 401 banner) ---------------------
  let tokenInput = $state("");

  // Daemon error codes to their localized line; the form and the stop
  // dialog share this mapping through faultLine.
  const codeMessage: Record<string, () => string> = {
    slug_taken: manage_instances_error_slug_taken,
    workspace_overlap: manage_instances_error_workspace_overlap,
    invalid_spawn_input: manage_instances_error_invalid_spawn_input,
    spawn_timeout: manage_instances_error_spawn_timeout,
    not_found: manage_instances_error_not_found,
    not_running: manage_instances_error_not_running,
    bad_request: manage_instances_error_bad_request,
    internal: manage_instances_error_internal,
  };

  function faultLine(cause: unknown): string {
    if (cause instanceof DaemonWebError) {
      const line = cause.code ? codeMessage[cause.code] : undefined;
      if (line) {
        return line();
      }
      if (cause.kind === "unauthorized") {
        return manage_instances_unauthorized();
      }
    }
    return manage_instances_error_internal();
  }

  // Assemble the daemon's env allowlist from the fixed optional fields —
  // no free-form KEY=VALUE entry (by design; the daemon validates keys).
  async function onSubmitSpawn(event: SubmitEvent) {
    event.preventDefault();
    if (spawnBusy) return;
    spawnBusy = true;
    spawnError = null;
    spawnResult = null;
    const env: string[] = [];
    if (agoraUrl.trim()) {
      env.push("KALLIP_TAGMA_RELAY_AGORA_URL=" + agoraUrl.trim());
    }
    if (enrollmentCode.trim()) {
      env.push("KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=" + enrollmentCode.trim());
    }
    if (lescheUrl.trim()) {
      env.push("KALLIP_TAGMA_RELAY_LESCHE_URL=" + lescheUrl.trim());
    }
    if (instanceToken.trim()) {
      env.push("KALLIP_AUTH_TOKEN=" + instanceToken.trim());
    }
    if (llmProvider.trim()) {
      env.push("KALLIP_LLM_PROVIDER=" + llmProvider.trim());
    }
    if (llmModel.trim()) {
      env.push("KALLIP_LLM_MODEL=" + llmModel.trim());
    }
    // The key variable name follows the provider choice.
    if (llmApiKey.trim()) {
      const keyVar =
        llmProvider.trim() === "openai-compatible"
          ? "KALLIP_LLM_OPENAI_COMPAT_API_KEY"
          : "KALLIP_LLM_DEEPSEEK_API_KEY";
      env.push(keyVar + "=" + llmApiKey.trim());
    }
    try {
      const result = await instancesStore.spawn({
        slug: slug.trim(),
        workspace: workspace.trim(),
        env,
      });
      spawnResult = { slug: result.slug, port: result.port };
      if (instanceToken.trim()) {
        sessionStorage.setItem(CONNECT_TOKEN_KEY, instanceToken.trim());
      }
      slug = "";
      workspace = "";
      agoraUrl = "";
      enrollmentCode = "";
      lescheUrl = "";
      instanceToken = "";
      llmProvider = "";
      llmModel = "";
      llmApiKey = "";
    } catch (cause) {
      spawnError = faultLine(cause);
    } finally {
      spawnBusy = false;
    }
  }

  async function onStopConfirmed() {
    if (!stopTarget || stopBusy) return;
    stopBusy = true;
    stopError = null;
    try {
      await instancesStore.stop(stopTarget);
      stopTarget = null;
    } catch (cause) {
      stopError = faultLine(cause);
    } finally {
      stopBusy = false;
    }
  }

  // Store the token and retry immediately; a still-standing 401 keeps
  // the banner, a green one dissolves into the normal page.
  function onTokenApply(event: SubmitEvent) {
    event.preventDefault();
    if (!tokenInput.trim()) return;
    sessionStorage.setItem(DAEMON_TOKEN_KEY, tokenInput.trim());
    instancesStore.refresh();
  }

  // One human line per classified failure kind (page-level banner). The
  // 401 branch carries the token entry; host_forbidden gets its own line.
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
      {#if instancesStore.errorKind === "unauthorized"}
        <form class="space-y-2" onsubmit={onTokenApply}>
          <p class="text-error-500 dark:text-error-400 text-sm">
            {manage_instances_unauthorized()}
          </p>
          <div class="flex gap-2">
            <input
              class="input"
              type="password"
              autocomplete="off"
              bind:value={tokenInput}
              placeholder={manage_instances_token_placeholder()}
              aria-label={manage_instances_token_label()}
            />
            <button type="submit" class="btn preset-filled-primary-500 shrink-0"
              >{manage_instances_token_apply()}</button
            >
          </div>
        </form>
      {:else}
        <p class="text-error-500 dark:text-error-400 text-sm">
          {#if instancesStore.errorKind === "forbidden" && instancesStore.errorCode === "host_forbidden"}
            {manage_instances_host_forbidden()}
          {:else}
            {kindMessage[instancesStore.errorKind]()}
          {/if}
        </p>
      {/if}
    {:else if !instancesStore.loaded}
      <p class="text-sm opacity-70">{manage_instances_loading()}</p>
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

      <section class="card preset-tonal-surface p-5 space-y-4">
        <h2 class="text-sm font-medium">{manage_instances_spawn_heading()}</h2>
        <form class="space-y-4" onsubmit={onSubmitSpawn}>
          <div class="grid gap-4 sm:grid-cols-2">
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_slug_label()} *
              </span>
              <input
                class="input"
                bind:value={slug}
                placeholder={manage_instances_spawn_slug_placeholder()}
                required
              />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_workspace_label()} *
              </span>
              <input
                class="input"
                bind:value={workspace}
                placeholder={manage_instances_spawn_workspace_placeholder()}
                required
              />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_agora_label()}
              </span>
              <input class="input" bind:value={agoraUrl} />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_enrollment_label()}
              </span>
              <input class="input" bind:value={enrollmentCode} />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_lesche_label()}
              </span>
              <input class="input" bind:value={lescheUrl} />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_token_label()}
              </span>
              <input class="input" type="password" bind:value={instanceToken} />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_llm_provider_label()}
              </span>
              <input
                class="input"
                bind:value={llmProvider}
                placeholder="deepseek | openai-compatible"
              />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_llm_model_label()}
              </span>
              <input class="input" bind:value={llmModel} />
            </label>
            <label class="block space-y-1">
              <span class="text-sm opacity-70">
                {manage_instances_spawn_llm_api_key_label()}
              </span>
              <input class="input" type="password" bind:value={llmApiKey} />
            </label>
          </div>
          <FormError message={spawnError} />
          {#if spawnResult}
            <p class="text-sm text-success-500 dark:text-success-400">
              {manage_instances_spawn_success({
                slug: spawnResult.slug,
                port: spawnResult.port,
              })}
              <a
                class="underline underline-offset-2 ml-1"
                href={"/connect?tagmaUrl=http://127.0.0.1:" + spawnResult.port}
                >{nav_chat()}</a
              >
            </p>
          {/if}
          <button
            type="submit"
            class="btn preset-filled-primary-500"
            disabled={spawnBusy || !slug.trim() || !workspace.trim()}
          >
            {spawnBusy ? "…" : manage_instances_spawn_submit()}
          </button>
        </form>
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
              <div class="flex items-center gap-3 shrink-0">
                {#if instancesStore.spawnedPorts[instance.slug]}
                  <a
                    class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-surface-500"
                    href={"/connect?tagmaUrl=http://127.0.0.1:" +
                      instancesStore.spawnedPorts[instance.slug]}
                    >{nav_chat()}</a
                  >
                {/if}
                {#if instance.running}
                  <button
                    type="button"
                    class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-error-500"
                    onclick={() => {
                      stopTarget = instance.slug;
                      stopError = null;
                    }}>{manage_instances_stop()}</button
                  >
                {/if}
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
            </div>
          {/each}
        {/if}
      </section>

      <ConfirmDialog
        open={stopTarget !== null}
        title={manage_instances_stop_title()}
        description={manage_instances_stop_description({
          slug: stopTarget ?? "",
        })}
        confirmLabel={manage_instances_stop()}
        busy={stopBusy}
        tone="danger"
        error={stopError}
        onConfirm={onStopConfirmed}
        onCancel={() => {
          stopTarget = null;
          stopError = null;
        }}
      />
    {/if}
  </div>
</div>
