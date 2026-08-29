<script lang="ts">
  import type {
    AgentStatusResponse,
    ProfileConfig,
  } from "@kallipai/kallip-client";
  import { agentStateLabel } from "../../lib/agentState.ts";
  import { MoreVertical, Pencil, Trash } from "@lucide/svelte";
  import { Menu, Portal } from "@skeletonlabs/skeleton-svelte";
  import { managementBackend } from "../../lib/manage/client.ts";
  import { KallipError } from "@kallipai/kallip-common";
  import { agentsStore } from "../../lib/manage/agents.svelte.ts";
  import { navigate } from "../../lib/shell/port.ts";
  import { TONAL_ICON_SURF } from "../../lib/classes.ts";
  import { startVisibleInterval } from "../../lib/visibleInterval.ts";
  import ConfirmDialog from "../../components/ConfirmDialog.svelte";
  import AgentIdentityDialog from "../../components/manage/AgentIdentityDialog.svelte";
  import CopyButton from "../../components/CopyButton.svelte";
  import StateDot from "../../components/manage/StateDot.svelte";
  import CurrentProfileCard from "../../components/manage/CurrentProfileCard.svelte";
  import AgentStatusCard from "../../components/manage/AgentStatusCard.svelte";
  import RetryListCard from "../../components/manage/RetryListCard.svelte";

  import {
    common_edit,
    common_remove,
    manage_agent_title,
    manage_agent_status_failed,
    manage_agent_back,
    manage_agent_role,
    manage_agent_duty,
    manage_agent_duty_team,
    manage_agent_identity_actions_aria,
    manage_agent_duty_onduty,
    manage_agent_duty_team_onduty,
    manage_agent_duty_team_offduty,
    manage_agent_duty_team_note,
    manage_agent_duty_offduty,
    manage_agent_created_by,
    manage_agent_workspace,
    manage_agent_description,
    manage_agent_toggle_duty,
    manage_agent_interrupt,
    manage_agent_remove_agent,
    manage_agent_remove_agent_desc_short,
  } from "../../paraglide/messages.js";
  let { id, basePath = "/local/manage" }: { id: string; basePath?: string } =
    $props();

  let status = $state<AgentStatusResponse | null>(null);
  let profileConfig = $state<ProfileConfig | null>(null);
  let statusError = $state<string | null>(null);
  let isLoading = $state(false);
  let stopPoll: (() => void) | null = null;

  let showRemoveDialog = $state(false);
  let showIdentityDialog = $state(false);

  async function fetchStatus() {
    isLoading = true;
    statusError = null;
    try {
      status = await managementBackend().getAgentStatus(id);
    } catch (e) {
      if (e instanceof KallipError) statusError = e.message;
      else {
        console.error("[agent status] fetch failed:", e);
        statusError = manage_agent_status_failed();
      }
    } finally {
      isLoading = false;
    }
  }

  $effect(() => {
    // Fetched directly (not via profilesStore) on purpose: the store's
    // refresh() clobbers its draft, which would discard unsaved edits
    // made on the profiles page.
    managementBackend()
      .getProfiles()
      .then((cfg) => {
        profileConfig = cfg;
      })
      .catch(() => {});
    fetchStatus();
    agentsStore.refresh();
    stopPoll = startVisibleInterval(fetchStatus, 5000);
    return () => {
      if (stopPoll) stopPoll();
    };
  });

  const agent = $derived(agentsStore.agents.find((a) => a.id === id));
  // Window occupancy approximation: conversation + pinned turns. This
  // understates what the runtime actually composes (its request estimate
  // also covers the system prompt and tools); cumulative counters are
  // lifetime totals -- useless against a window.
  const windowTokens = $derived(
    status
      ? status.context.turn_tokens +
          status.context.pinned_items.reduce((sum, [, n]) => sum + n, 0)
      : 0,
  );
  const contextWindow = $derived.by(() => {
    const pid = status?.profile?.profile_id;
    if (!pid || !profileConfig) return null;
    for (const set of Object.values(profileConfig.sets)) {
      for (const p of set.profiles) {
        if (p.id === pid) return p.max_context_window;
      }
    }
    return null;
  });

  async function onSaveIdentity(role: string, description: string) {
    await agentsStore.updateMetadata(id, { role, description }).catch(() => {});
    showIdentityDialog = false;
  }
  async function onConfirmRemove() {
    await agentsStore.remove(id).catch(() => {});
    showRemoveDialog = false;
    navigate("/local/manage/agents");
  }
</script>

<svelte:head
  ><title>{manage_agent_title({ id: agent?.role || id })}</title></svelte:head
>

<div class="h-full overflow-y-auto">
  <div class="p-6 max-w-2xl space-y-6">
    <div>
      <div class="flex items-center gap-3">
        <a
          href={`${basePath}/agents`}
          class="btn btn-sm preset-outlined-surface-500"
          >{manage_agent_back()}</a
        >
        {#if agent}
          <h1 class="text-xl font-semibold truncate min-w-0">
            {agent.role || "—"}
          </h1>
          <StateDot state={agent.state} />
          <span class="font-medium">{agentStateLabel(agent.state)}</span>
          {#if agent.activity}
            <span class="opacity-60 text-sm">· {agent.activity}</span>
          {/if}
        {:else}
          <h1 class="text-xl font-semibold font-mono truncate min-w-0">{id}</h1>
        {/if}
      </div>
      {#if agent}
        <div class="flex items-start gap-1 mt-1 group">
          <p class="font-mono text-xs opacity-60 break-all select-text min-w-0">
            {id}
          </p>
          <CopyButton getText={() => id} />
        </div>
      {/if}
    </div>

    {#if statusError}
      <p class="text-error-500 dark:text-error-400 text-sm">{statusError}</p>
    {/if}

    {#if agent}
      <section class="card preset-tonal-surface p-5 space-y-3">
        <div class="flex justify-end">
          <Menu
            positioning={{ placement: "bottom-end" }}
            onSelect={(e) => {
              if (e.value === "edit") showIdentityDialog = true;
              else if (e.value === "remove") showRemoveDialog = true;
            }}
          >
            <Menu.Trigger
              class="size-10 {TONAL_ICON_SURF} shrink-0"
              aria-label={manage_agent_identity_actions_aria()}
            >
              <MoreVertical class="size-4" />
            </Menu.Trigger>
            <Portal>
              <Menu.Positioner>
                <Menu.Content
                  class="card preset-tonal-surface p-1 min-w-[8rem]"
                >
                  <Menu.Item
                    value="edit"
                    class="flex items-center gap-2 px-3 py-2 rounded-base text-sm cursor-pointer hover:preset-filled-surface-500"
                  >
                    <Pencil class="size-4" />
                    {common_edit()}
                  </Menu.Item>
                  <Menu.Item
                    value="remove"
                    class="flex items-center gap-2 px-3 py-2 rounded-base text-sm text-error-500 dark:text-error-400 cursor-pointer hover:preset-filled-error-500"
                  >
                    <Trash class="size-4" />
                    {manage_agent_remove_agent()}
                  </Menu.Item>
                </Menu.Content>
              </Menu.Positioner>
            </Portal>
          </Menu>
        </div>
        <div class="grid grid-cols-2 gap-3 text-sm">
          <div>
            <span class="opacity-60 text-xs uppercase tracking-wide block"
              >{manage_agent_role()}</span
            >
            <span>{agent.role || "—"}</span>
          </div>
          <div>
            <span class="opacity-60 text-xs uppercase tracking-wide block"
              >{agent.created_by === null
                ? manage_agent_duty_team()
                : manage_agent_duty()}</span
            >
            <span
              >{agent.duty === "onduty"
                ? agent.created_by === null
                  ? manage_agent_duty_team_onduty()
                  : manage_agent_duty_onduty()
                : agent.created_by === null
                  ? manage_agent_duty_team_offduty()
                  : manage_agent_duty_offduty()}</span
            >
            {#if agent.created_by === null}
              <p class="opacity-50 text-xs">{manage_agent_duty_team_note()}</p>
            {/if}
          </div>
          <div>
            <span class="opacity-60 text-xs uppercase tracking-wide block"
              >{manage_agent_created_by()}</span
            >
            <span class="font-mono text-xs" title={agent.created_by ?? "root"}
              >{agent.created_by ?? "root"}</span
            >
          </div>
          <div>
            <span class="opacity-60 text-xs uppercase tracking-wide block"
              >{manage_agent_workspace()}</span
            >
            <span
              class="font-mono text-xs truncate block"
              title={agent.workspace_root}>{agent.workspace_root}</span
            >
          </div>
        </div>
        <div>
          <span class="opacity-60 text-xs uppercase tracking-wide block"
            >{manage_agent_description()}</span
          >
          <span class="text-sm">{agent.description || "—"}</span>
        </div>
      </section>
    {/if}
    {#if status?.profile}
      <CurrentProfileCard profile={status.profile} />
    {/if}

    {#if status}
      <AgentStatusCard {status} {windowTokens} {contextWindow} />
      {#if status.recent_retries.length > 0}
        <RetryListCard {status} />
      {/if}
    {/if}

    {#if agent}
      <section class="flex flex-wrap gap-2">
        {#if agent.state === "busy"}
          <button
            class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-surface-500"
            disabled={agentsStore.isInFlight(agent.id)}
            onclick={() => agentsStore.interrupt(agent.id).catch(() => {})}
            >{manage_agent_interrupt()}</button
          >
        {/if}
        <button
          class="btn btn-sm preset-outlined-surface-500 hover:preset-filled-surface-500"
          disabled={agentsStore.isInFlight(agent.id)}
          onclick={() => agentsStore.toggleDuty(agent.id).catch(() => {})}
          >{manage_agent_toggle_duty()}</button
        >
      </section>
    {/if}
  </div>
</div>

<ConfirmDialog
  busy={showRemoveDialog && agentsStore.isInFlight(id)}
  open={showRemoveDialog}
  title={manage_agent_remove_agent()}
  description={manage_agent_remove_agent_desc_short()}
  confirmLabel={common_remove()}
  tone="danger"
  onConfirm={onConfirmRemove}
  onCancel={() => (showRemoveDialog = false)}
/>
<AgentIdentityDialog
  open={showIdentityDialog}
  role={agent?.role ?? ""}
  description={agent?.description ?? ""}
  busy={agentsStore.isInFlight(id)}
  onSave={onSaveIdentity}
  onCancel={() => (showIdentityDialog = false)}
/>
