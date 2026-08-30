<script lang="ts">
  // Online management wrapper: resolves the RelayChannel for the given tagma,
  // creates an OnlineBackend, switches all stores to it, and renders the
  // management sub-page. The basePath comes from the path-builder so all
  // internal links stay within the /tagma/{tagmaId}/details/* tree.
  //
  // The stores share the RelayChannel with the chat — manage_result replies are
  // intercepted in RelayChannel.enqueue() and never reach the chat stream.

  import { onMount } from "svelte";
  import { channelsStore } from "../../lib/session/channels.svelte.ts";
  import { realtimeStore } from "../../lib/session/realtime.svelte.ts";
  import { OnlineBackend } from "../../lib/manage/backend.ts";
  import { manageChannelStalled } from "../../lib/manage/channelStalled.ts";
  import { budgetStore } from "../../lib/manage/budget.svelte.ts";
  import { agentsStore } from "../../lib/manage/agents.svelte.ts";
  import { profilesStore } from "../../lib/manage/profiles.svelte.ts";
  import { schedulesStore } from "../../lib/manage/schedules.svelte.ts";
  import { managementBackend } from "../../lib/manage/client.ts";
  import {
    tagmaChatPath,
    tagmaDetailsPath,
    type TagmaDetailsSection,
  } from "../../lib/shell/routes.ts";
  import Breadcrumbs from "../../components/Breadcrumbs.svelte";
  import OverviewPage from "./OverviewPage.svelte";
  import BudgetPage from "./BudgetPage.svelte";
  import AgentsPage from "./AgentsPage.svelte";
  import ProfilesPage from "./ProfilesPage.svelte";
  import SchedulesPage from "./SchedulesPage.svelte";
  import {
    manage_backend_failed,
    chat_channel_unavailable,
    manage_opening,
    common_retry,
    nav_breadcrumb_tagma,
    nav_breadcrumb_agents,
    nav_overview,
    nav_budget,
    nav_profiles,
    nav_schedules,
  } from "../../paraglide/messages.js";

  let {
    tagmaId,
    page,
  }: {
    tagmaId: string;
    page: TagmaDetailsSection;
  } = $props();

  const basePath = tagmaDetailsPath(tagmaId);

  const channelState = $derived(channelsStore.getTagmaChannelState(tagmaId));
  const conversationId = $derived(
    channelState.kind === "open" ||
      channelState.kind === "offline" ||
      channelState.kind === "error"
      ? (channelState.conversationId ?? null)
      : null,
  );

  // Can this channel still open on its own? absent to a confirmed-offline
  // peer and unavailable cannot (see channelStalled.ts); without this the
  // placeholder copy below would show forever.
  const stalled = $derived(
    !conversationId &&
      manageChannelStalled(
        channelState,
        realtimeStore.resolved && !realtimeStore.has(tagmaId),
      ),
  );
  let backendReady = $state(false);
  let error = $state<string | null>(null);

  $effect(() => {
    if (!conversationId) {
      backendReady = false;
      return;
    }
    const conv = channelsStore.get(conversationId);
    if (!conv || conv.kind !== "relay") {
      backendReady = false;
      return;
    }
    try {
      // conv.kind === "relay" narrows to RelayConversation
      const relayConv =
        conv as import("../../lib/session/conversation.svelte.ts").RelayConversation;
      const channel = relayConv.relayTransport.relayChannel;
      const backend = new OnlineBackend(channel);
      budgetStore.switchBackend(backend);
      agentsStore.switchBackend(backend);
      profilesStore.switchBackend(backend);
      schedulesStore.switchBackend(backend);
      backendReady = true;
      error = null;
    } catch (e) {
      console.error("[manage] backend wiring failed:", e);
      error = manage_backend_failed();
      backendReady = false;
    }
  });

  onMount(() => {
    return () => {
      try {
        const b = managementBackend();
        budgetStore.switchBackend(b);
        agentsStore.switchBackend(b);
        profilesStore.switchBackend(b);
        schedulesStore.switchBackend(b);
      } catch {
        /* no offline config */
      }
    };
  });

  // #4/#5 trail: the tagma segment links the tagma home surface (chat); the
  // tail is the current section (R3: current, no href).
  const sectionLabels: Record<TagmaDetailsSection, () => string> = {
    overview: nav_overview,
    budget: nav_budget,
    agents: nav_breadcrumb_agents,
    profiles: nav_profiles,
    schedules: nav_schedules,
  };
  const breadcrumbs = $derived([
    { label: nav_breadcrumb_tagma(), href: tagmaChatPath(tagmaId) },
    { label: sectionLabels[page](), current: true },
  ]);
</script>

<div class="h-full flex flex-col">
  <div class="px-6 pt-4 shrink-0">
    <Breadcrumbs segments={breadcrumbs} />
  </div>
  <div class="flex-1 min-h-0">
    {#if stalled}
      <!-- No conversation and none can come without a retry: absent to a
       presence-confirmed-offline peer, or an open-budget failure (mirror of
       the chat page's unavailable row). -->
      <div class="h-full grid place-items-center p-6">
        <div class="text-center flex flex-col gap-3 max-w-sm">
          <p class="text-sm text-error-500 dark:text-error-400">
            {chat_channel_unavailable()}
          </p>
          <button
            type="button"
            class="btn preset-tonal-surface self-center"
            onclick={() => channelsStore.retryTagma(tagmaId)}
          >
            {common_retry()}
          </button>
        </div>
      </div>
    {:else if !backendReady}
      <div class="h-full grid place-items-center p-6">
        <div class="text-center flex flex-col gap-3 max-w-sm">
          {#if error}
            <p class="text-error-500 dark:text-error-400 text-sm">{error}</p>
          {:else}
            <p class="text-sm opacity-60">{manage_opening()}</p>
          {/if}
        </div>
      </div>
    {:else if page === "overview"}
      <OverviewPage {basePath} />
    {:else if page === "budget"}
      <BudgetPage {basePath} />
    {:else if page === "agents"}
      <AgentsPage {tagmaId} />
    {:else if page === "profiles"}
      <ProfilesPage {basePath} />
    {:else if page === "schedules"}
      <SchedulesPage {basePath} />
    {/if}
  </div>
</div>
