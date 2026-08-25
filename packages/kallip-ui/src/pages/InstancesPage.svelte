<script lang="ts">
  // The unified instances page: one device per row -- the enrolled tagma's
  // identity joined with its host-side instance process by the slug
  // prefix convention -- under one header (daemon health inline + the
  // single "New tagma" action opening the two-path dialog). The page owns
  // every store call; the dialog and the rows stay presentational
  // (the CreateRoomDialog discipline). The AppShell expects the page root
  // to scroll itself (h-full overflow-y-auto).
  import { onMount } from "svelte";
  import {
    agoraBaseUrlOrFail,
    agoraSession,
    lescheBaseUrlOrFail,
  } from "../lib/session/agora.svelte";
  import { realtimeStore } from "../lib/session/realtime.svelte.ts";
  import { instancesStore } from "../lib/instances/instances.svelte.ts";
  import {
    CONNECT_TOKEN_KEY,
    INSTANCES_TOKEN_KEY,
    InstancesError,
    instanceSlugFor,
  } from "../lib/instances/client.ts";
  import { formatRemaining, isExpired } from "../lib/tagmata.svelte.ts";
  import ConfirmDialog from "../components/ConfirmDialog.svelte";
  import CreateInstanceDialog, {
    type AdvancedSpawnFields,
  } from "../components/instances/CreateInstanceDialog.svelte";
  import DeviceRow from "../components/instances/DeviceRow.svelte";
  import {
    common_copied,
    common_copy,
    manage_instances_create_failed,
    manage_instances_create_mint_failed,
    manage_instances_daemon_running,
    manage_instances_daemon_stopped,
    manage_instances_empty,
    manage_instances_error_bad_request,
    manage_instances_error_internal,
    manage_instances_error_invalid_spawn_input,
    manage_instances_error_not_found,
    manage_instances_error_not_running,
    manage_instances_error_slug_taken,
    manage_instances_error_spawn_timeout,
    manage_instances_error_workspace_overlap,
    manage_instances_forbidden,
    manage_instances_heading,
    manage_instances_host_forbidden,
    manage_instances_load_failed,
    manage_instances_loading,
    manage_instances_new,
    manage_instances_spawn_success,
    manage_instances_stop,
    manage_instances_stop_description,
    manage_instances_stop_title,
    manage_instances_title,
    manage_instances_token_apply,
    manage_instances_token_label,
    manage_instances_token_placeholder,
    manage_instances_token_rejected,
    manage_instances_unauthorized,
    manage_instances_session_required,
    manage_instances_unreachable,
    nav_chat,
    tagmata_expired_badge,
    tagmata_expires_in,
    tagmata_pending_badge,
    tagma_profile_unnamed,
  } from "../paraglide/messages.js";

  $effect(() => {
    instancesStore.startPolling(5000);
    return () => instancesStore.stopPolling();
  });

  // --- create dialog ----------------------------------------------------
  let createOpen = $state(false);
  let createBusy = $state(false);
  let createError = $state<string | null>(null);
  // The just-spawned success line lives here (not in the dialog): it must
  // outlive the dialog, which closes on success.
  let spawnResult = $state<{ slug: string; port: number } | null>(null);

  const canSpawn = $derived(
    instancesStore.capabilities?.includes("designated-user") ?? false,
  );

  // Service error codes to their localized line; the spawn paths and the
  // stop dialog share this mapping through faultLine.
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
    if (cause instanceof InstancesError) {
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

  // One-click (dialog path A): mint, then spawn with the relay env
  // pointing at this deployment so the tagma enrolls itself on first
  // start. On success close the dialog and surface the port line.
  async function onOneClick(opts: { workspace: string }): Promise<void> {
    createBusy = true;
    spawnResult = null;
    createError = null;
    try {
      const minted = await agoraSession.mintTagma();
      if (!minted) {
        createError = manage_instances_create_mint_failed();
        return;
      }
      const slug = instanceSlugFor(minted.id);
      const env = [
        `KALLIP_TAGMA_RELAY_AGORA_URL=${agoraBaseUrlOrFail()}`,
        `KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=${minted.code}`,
        `KALLIP_TAGMA_RELAY_LESCHE_URL=${lescheBaseUrlOrFail()}`,
      ];
      try {
        const result = await instancesStore.spawn({
          slug,
          workspace: opts.workspace,
          env,
        });
        spawnResult = { slug: result.slug, port: result.port };
        createOpen = false;
      } catch (cause) {
        // The minted code stays valid (the pending row shows its masked
        // form); the advanced path can redeem it by hand.
        console.error("[instances] one-click spawn failed:", cause);
        createError = manage_instances_create_failed();
      }
    } finally {
      createBusy = false;
    }
  }

  // Advanced (dialog path A's disclosure): assemble the daemon env
  // allowlist from the fixed optional fields -- no free-form KEY=VALUE
  // entry (the daemon validates keys).
  async function onSpawn(f: AdvancedSpawnFields): Promise<void> {
    createBusy = true;
    spawnResult = null;
    createError = null;
    const env: string[] = [];
    if (f.agoraUrl.trim()) {
      env.push("KALLIP_TAGMA_RELAY_AGORA_URL=" + f.agoraUrl.trim());
    }
    if (f.enrollmentCode.trim()) {
      env.push("KALLIP_TAGMA_RELAY_ENROLLMENT_CODE=" + f.enrollmentCode.trim());
    }
    if (f.lescheUrl.trim()) {
      env.push("KALLIP_TAGMA_RELAY_LESCHE_URL=" + f.lescheUrl.trim());
    }
    if (f.instanceToken.trim()) {
      env.push("KALLIP_AUTH_TOKEN=" + f.instanceToken.trim());
    }
    if (f.llmProvider.trim()) {
      env.push("KALLIP_LLM_PROVIDER=" + f.llmProvider.trim());
    }
    if (f.llmModel.trim()) {
      env.push("KALLIP_LLM_MODEL=" + f.llmModel.trim());
    }
    // The key variable name follows the provider choice.
    if (f.llmApiKey.trim()) {
      const keyVar =
        f.llmProvider.trim() === "openai-compatible"
          ? "KALLIP_LLM_OPENAI_COMPAT_API_KEY"
          : "KALLIP_LLM_DEEPSEEK_API_KEY";
      env.push(keyVar + "=" + f.llmApiKey.trim());
    }
    try {
      const result = await instancesStore.spawn({
        slug: f.slug,
        workspace: f.workspace,
        env,
      });
      spawnResult = { slug: result.slug, port: result.port };
      if (f.instanceToken.trim()) {
        sessionStorage.setItem(CONNECT_TOKEN_KEY, f.instanceToken.trim());
      }
      createOpen = false;
    } catch (cause) {
      createError = faultLine(cause);
    } finally {
      createBusy = false;
    }
  }

  // Local-agent (dialog path B): mint only; the dialog holds the
  // plaintext + QR. mintTagma self-reports failure (null + its own error
  // state), so map null to the dialog's error line here.
  async function onMint(): Promise<{ id: string; code: string } | null> {
    createBusy = true;
    createError = null;
    try {
      const minted = await agoraSession.mintTagma();
      if (!minted) createError = manage_instances_create_mint_failed();
      return minted;
    } finally {
      createBusy = false;
    }
  }

  // --- stop dialog (from the retired panel) -----------------------------
  let stopTarget = $state<string | null>(null);
  let stopBusy = $state(false);
  let stopError = $state<string | null>(null);

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

  // --- standalone-mode token entry (the 401 banner) ---------------------
  let tokenInput = $state("");
  let tokenRejected = $state(false);

  async function onTokenApply(event: SubmitEvent) {
    event.preventDefault();
    if (!tokenInput.trim()) return;
    sessionStorage.setItem(INSTANCES_TOKEN_KEY, tokenInput.trim());
    await instancesStore.refresh();
    tokenRejected = instancesStore.errorKind === "unauthorized";
  }

  const kindMessage = {
    unauthorized: manage_instances_unauthorized,
    forbidden: manage_instances_forbidden,
    unreachable: manage_instances_unreachable,
    other: manage_instances_load_failed,
  } as const;

  // --- the unified list ---------------------------------------------------
  // Join identity (enrolled tagmas) with processes by the one-click slug
  // prefix convention; unmatched entries fall to their own row shape.
  // Identity-only rows re-derive presence here the way TagmataSection
  // does ("checking" until realtime's snapshot resolves).
  const devices = $derived.by(() => {
    const bySlug = new Map(
      instancesStore.instances.map((i) => [i.slug, i] as const),
    );
    const matched = new Set<string>();
    const rows: {
      key: string;
      identity:
        | {
            tagmaId: string;
            label: string | null;
            presence: "checking" | "online" | "offline";
            status?: ReturnType<typeof realtimeStore.statusFor>;
          }
        | undefined;
      process:
        | {
            slug: string;
            workspace: string;
            running: boolean;
            port?: number;
          }
        | undefined;
    }[] = [];
    for (const c of agoraSession.enrolledCards) {
      const slug = instanceSlugFor(c.tagmaId);
      const inst = bySlug.get(slug);
      if (inst) matched.add(slug);
      rows.push({
        key: c.tagmaId,
        identity: {
          tagmaId: c.tagmaId,
          label: c.label,
          presence: realtimeStore.resolved
            ? realtimeStore.has(c.tagmaId)
              ? "online"
              : "offline"
            : "checking",
          status: realtimeStore.statusFor(c.tagmaId),
        },
        process: inst
          ? {
              slug: inst.slug,
              workspace: inst.workspace,
              running: inst.running,
              port: instancesStore.spawnedPorts[inst.slug],
            }
          : undefined,
      });
    }
    for (const inst of instancesStore.instances) {
      if (matched.has(inst.slug)) continue;
      rows.push({
        key: inst.slug,
        identity: undefined,
        process: {
          slug: inst.slug,
          workspace: inst.workspace,
          running: inst.running,
          port: instancesStore.spawnedPorts[inst.slug],
        },
      });
    }
    return rows;
  });

  const pending = $derived(agoraSession.pending);

  // Live "now" ticking once a minute so the pending rows' expiry lines
  // stay fresh without a re-fetch (EnrollmentCodeCard's granularity).
  let now = $state(Date.now());
  onMount(() => {
    const id = setInterval(() => {
      now = Date.now();
    }, 60_000);
    return () => clearInterval(id);
  });

  const empty = $derived(devices.length === 0 && pending.length === 0);

  function openCreate() {
    createError = null;
    createOpen = true;
  }
</script>

<svelte:head><title>{manage_instances_title()}</title></svelte:head>

<!-- Single scroll root (the AppShell overflow-hidden contract); the
     centered narrow column matches the other manage pages. -->
<div class="h-full overflow-y-auto">
  <div class="p-6 max-w-2xl mx-auto space-y-6">
    <h1 class="text-xl font-semibold hidden md:block">
      {manage_instances_heading()}
    </h1>

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

    {#if instancesStore.errorKind}
      {#if instancesStore.errorKind === "unauthorized" && instancesStore.errorCode === "admin_session_required"}
        <!-- Platform mode, cookie channel: no token to paste -- the
             admin login itself carries the right. -->
        <p class="text-error-500 dark:text-error-400 text-sm">
          {manage_instances_session_required()}
        </p>
      {:else if instancesStore.errorKind === "unauthorized"}
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
          {#if tokenRejected}
            <p class="text-xs text-error-500 dark:text-error-400">
              {manage_instances_token_rejected()}
            </p>
          {/if}
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
      <!-- Header row: daemon health inline (a dot + one line, not a
           ~90px card) + the single create action. -->
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <p class="text-sm flex items-center gap-2 min-w-0">
          {#if instancesStore.health}
            <span
              class="size-2 rounded-full shrink-0 {instancesStore.health.running
                ? 'bg-success-500'
                : 'bg-error-500'}"
              aria-hidden="true"
            ></span>
            <span
              class="truncate {instancesStore.health.running
                ? ''
                : 'text-error-500 dark:text-error-400'}"
            >
              {instancesStore.health.running
                ? manage_instances_daemon_running()
                : manage_instances_daemon_stopped()}
            </span>
            {#if instancesStore.health.detail}
              <span class="text-xs opacity-70 truncate">
                · {instancesStore.health.detail}</span
              >
            {/if}
          {/if}
        </p>
        <button
          type="button"
          class="btn preset-filled-primary-500 shrink-0"
          onclick={openCreate}
        >
          + {manage_instances_new()}
        </button>
      </div>

      <section
        class="card preset-tonal-surface divide-y divide-surface-200-800"
      >
        {#if empty}
          <!-- First-run empty state: promote the single primary action
               (the TagmataDashboard hero pattern). -->
          <div class="p-10 grid place-items-center gap-4">
            <p class="text-sm opacity-70">{manage_instances_empty()}</p>
            <button
              type="button"
              class="btn preset-filled-primary-500"
              onclick={openCreate}
            >
              + {manage_instances_new()}
            </button>
          </div>
        {:else}
          {#each pending as code (code.id)}
            <!-- A pending enrollment: masked code + expiry, copyable
                 only while the plaintext is still in hand. Rename and
                 revoke keep their full card on /tagmata. -->
            <div class="p-4 flex items-center gap-4">
              <span
                class="size-2 rounded-full shrink-0 bg-surface-300-700"
                aria-hidden="true"
              ></span>
              <div class="min-w-0 flex-1">
                <p class="font-medium truncate text-sm">
                  {code.label ?? tagma_profile_unnamed()}
                  <span
                    class="badge preset-filled-surface-500 text-xs ml-1 align-middle"
                    >{tagmata_pending_badge()}</span
                  >
                </p>
                <p class="text-xs opacity-70 truncate font-mono">{code.code}</p>
              </div>
              <div class="flex items-center gap-3 shrink-0">
                {#if isExpired(code.expiresAt)}
                  <span class="badge preset-filled-warning-500 text-xs"
                    >{tagmata_expired_badge()}</span
                  >
                {:else}
                  <span class="text-xs opacity-70 whitespace-nowrap">
                    {tagmata_expires_in({
                      duration: formatRemaining(
                        new Date(code.expiresAt).getTime() - now,
                      ),
                    })}
                  </span>
                {/if}
                {#if code.copyable}
                  <button
                    type="button"
                    class="btn btn-sm preset-tonal-surface"
                    onclick={() => agoraSession.copySecret(code.id, code.code)}
                  >
                    {agoraSession.copiedCodeId === code.id
                      ? common_copied()
                      : common_copy()}
                  </button>
                {/if}
              </div>
            </div>
          {/each}
          {#each devices as d (d.key)}
            <DeviceRow
              identity={d.identity}
              process={d.process}
              onStop={(slug) => {
                stopTarget = slug;
                stopError = null;
              }}
            />
          {/each}
        {/if}
      </section>
    {/if}

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

    <CreateInstanceDialog
      open={createOpen}
      busy={createBusy}
      error={createError}
      {canSpawn}
      {onOneClick}
      {onSpawn}
      {onMint}
      onCancel={() => (createOpen = false)}
    />
  </div>
</div>
