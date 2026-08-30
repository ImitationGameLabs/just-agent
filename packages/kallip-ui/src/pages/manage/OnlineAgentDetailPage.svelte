<script lang="ts">
  // Online agent detail: resolves the RelayChannel for the given tagma and
  // constructs a dedicated OnlineBackend for this page. Unlike
  // OnlineManagePage this deliberately does NOT switch the global stores --
  // the page must render self-sufficiently from the route's tagmaId
  // (arch M2), because a deep link carries no guarantee that the manage hub
  // ran first. The placeholder rows mirror OnlineManagePage's channel
  // states; the wiring pattern is shared with it for the same reason.
  import { channelsStore } from "../../lib/session/channels.svelte.ts";
  import { realtimeStore } from "../../lib/session/realtime.svelte.ts";
  import { OnlineBackend } from "../../lib/manage/backend.ts";
  import { manageChannelStalled } from "../../lib/manage/channelStalled.ts";
  import AgentDetailPage from "./AgentDetailPage.svelte";
  import {
    tagmaChatPath,
    tagmaDetailsPath,
    tagmaDetailsSectionPath,
  } from "../../lib/shell/routes.ts";
  import {
    chat_channel_unavailable,
    common_retry,
    manage_agents_details,
    manage_opening,
    nav_breadcrumb_agents,
    nav_breadcrumb_tagma,
  } from "../../paraglide/messages.js";

  let { tagmaId, agentId }: { tagmaId: string; agentId: string } = $props();

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
  let backend = $state<OnlineBackend | null>(null);

  $effect(() => {
    if (!conversationId) {
      backend = null;
      return;
    }
    const conv = channelsStore.get(conversationId);
    if (!conv || conv.kind !== "relay") {
      backend = null;
      return;
    }
    try {
      // conv.kind === "relay" narrows to RelayConversation
      const relayConv =
        conv as import("../../lib/session/conversation.svelte.ts").RelayConversation;
      backend = new OnlineBackend(relayConv.relayTransport.relayChannel);
    } catch (e) {
      console.error("[agent detail] backend wiring failed:", e);
      backend = null;
    }
  });

  const breadcrumbs = $derived([
    { label: nav_breadcrumb_tagma(), href: tagmaChatPath(tagmaId) },
    { label: manage_agents_details(), href: tagmaDetailsPath(tagmaId) },
    {
      label: nav_breadcrumb_agents(),
      href: tagmaDetailsSectionPath(tagmaId, "agents"),
    },
  ]);
</script>

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
{:else if !backend}
  <div class="h-full grid place-items-center p-6">
    <div class="text-center flex flex-col gap-3 max-w-sm">
      <p class="text-sm opacity-60">{manage_opening()}</p>
    </div>
  </div>
{:else}
  <AgentDetailPage
    id={agentId}
    basePath={tagmaDetailsPath(tagmaId)}
    {backend}
    {breadcrumbs}
  />
{/if}
