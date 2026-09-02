<script lang="ts">
  import { onMount } from "svelte";
  import type { Snippet } from "svelte";
  import AppShell from "../../components/AppShell.svelte";
  import AccountMenu from "../../components/AccountMenu.svelte";
  import TagmaStatusHeader from "../../components/TagmaStatusHeader.svelte";
  import TagmaStatusLine from "../../components/TagmaStatusLine.svelte";
  import TagmaStatusPanel from "../../components/TagmaStatusPanel.svelte";
  import { classifyError } from "../errors.ts";
  import { agoraSession } from "../session/agora.svelte";
  import { channelsStore } from "../session/channels.svelte";
  import { statusCardStore } from "../session/statusCard.svelte.ts";
  import { roomsStore } from "../session/rooms.svelte";
  import { directSessionsStore } from "../session/directSessions.svelte";
  import { instancesStore } from "../instances/instances.svelte.ts";
  import { realtimeStore } from "../session/realtime.svelte";
  import { roomConversationsStore } from "../session/roomConversations.svelte";
  import { roomKey, tagmaKey, unreadStore } from "../session/unread.svelte.ts";
  import { notify, shouldNotifyRoom } from "../session/notify.ts";
  import { decodeB64 } from "@kallipai/kallip-common";
  import { decodeRoomMessage } from "../room-message.ts";
  import type { Envelope } from "@kallipai/kallip-lesche-client";
  import { connectDirect } from "../session/connect.ts";
  import { configStore } from "../config/config.svelte";
  import {
    navFor,
    pathMatches,
    tagmaNavIndicator,
    type NavIcons,
  } from "./links.ts";
  import { appGateDecision, isPublicRoute } from "./gate.ts";
  import { chatRoute } from "./chatRoute.ts";
  import { mobileBack } from "./breadcrumbs.ts";
  import { isOfflineOnlyShell, navigate, shellMode } from "./port.ts";
  import {
    account_menu,
    manage_agents_heading,
    manage_budget_heading,
    manage_overview_heading,
    manage_profiles_heading,
    manage_schedules_heading,
    nav_files,
    nav_home,
    nav_manage,
    room_label_fallback,
    settings_heading,
  } from "../../paraglide/messages.js";

  let {
    pathname,
    search,
    appKind,
    icons,
    children,
  }: {
    pathname: string;
    search: string;
    appKind: "app" | "web";
    icons: NavIcons;
    children: Snippet;
  } = $props();

  // The mode is the single source of "which product are we in", derived from
  // the shell identity (shellMode) rather than the persisted config: a
  // web/app shell is always online (a stored offline config is clamped away
  // -- its routes moved to kallip-direct); the direct shell is always
  // offline.
  const mode = $derived(shellMode());

  // Boot once the config has loaded. Two boot shapes:
  //   - offline-only shell: reconnect the tagma when credentials exist,
  //     never touch agora (no credentials = nothing to boot);
  //   - online (web/app, any stored mode): resolve the agora session so
  //     the gate reads a settled `user`.
  // onMount (not a reactive $effect) so this runs exactly once, with no
  // `booted` flag and no effect read-of-write hazard.
  onMount(() => {
    // Wire inbound envelopes: demux by recipient -- an envelope for an OPENED
    // room conversation goes to the room store (plaintext render); anything else
    // is the bilateral 1:1 path -> channelsStore. The room store's `get` is the
    // demux (rooms bypass the bilateral projector per the mellow-baking-taco
    // decision). Bound here (the shell, where both singletons are in scope)
    // rather than via a store-to-store import, keeping realtime decoupled from
    // both. Idempotent + safe to run once per mount.
    realtimeStore.setEnvelopeSink((env) => {
      if (roomConversationsStore.get(env.channel_id)) {
        roomConversationsStore.deliverLive(
          env.channel_id,
          env.ciphertext,
          env.sender,
        );
        // The transcript renders live; the unread store no-ops while the
        // room is being viewed and pull-counts when open-but-not-viewed.
        unreadStore.noteRoomActivity(env.channel_id);
        maybeNotifyRoom(env);
      } else if (roomsStore.has(env.channel_id)) {
        // A room envelope with no open conversation: the unread store
        // pull-counts it precisely (live room envelopes carry no seq).
        unreadStore.noteRoomActivity(env.channel_id);
        maybeNotifyRoom(env);
      } else {
        channelsStore.deliver(env);
      }
    });
    // Wire runtime signals (busy/idle presence, turn terminals, errors) into
    // the owning channel's transcript. Same shell-binding discipline.
    realtimeStore.setSignalSink((tagmaId, signal) =>
      channelsStore.deliverSignal(tagmaId, signal),
    );
    // Wire aggregate status snapshots (root state, subagent counts, budget) into
    // the owning channel's `statusSnapshot`, so the chat header reads one
    // uniform source (the direct path drains its own SSE status). Same
    // shell-binding discipline.
    realtimeStore.setStatusSink((tagmaId, snapshot) =>
      channelsStore.deliverStatus(tagmaId, snapshot),
    );
    // Wire the cached-status backfill: a freshly-opened relay channel seeds its
    // `statusSnapshot` from realtime's in-session cache so the header shows at
    // once (otherwise it waits for the next status push). Same shell-binding
    // discipline; keeps channels decoupled from realtime.
    channelsStore.setStatusBackfill((tagmaId) =>
      realtimeStore.statusFor(tagmaId),
    );

    // An offline-class channel-open failure (the KEX came back 503) means the
    // lesche just proved the tagma unreachable: retract the stale-online
    // presence entry so both the dot and the auto-open driver read the
    // truth. Bound here for the same decoupling reason as the backfill.
    channelsStore.setOfflineCorrection((tagmaId) =>
      realtimeStore.markOffline(tagmaId),
    );
    // Wire presence transitions to auto-connect: an offline -> online tagma is
    // opened on demand, with `refresh: true` so an ALREADY-open channel is
    // re-keyed too -- a restarted peer's fresh epoch cannot read our old
    // session key, and without the refresh it would silently drop our sends
    // (202-then-nothing). Same shell-binding discipline as the envelope sink.
    // NOTE: for never-opened tagmas this stays a pre-warm convenience (the
    // sidebar shows enrolled tagmas regardless -- it just makes the spinner
    // fleeting by opening channels the SSE knows are online); the refresh
    // leg, though, is load-bearing: it is the only path that heals a
    // restarted-peer channel.
    realtimeStore.setPresenceSink((tagmaId, online) => {
      if (!online) return;
      const tagma = agoraSession.tagmata.find(
        (t) => t.tagma_id === tagmaId && t.state === "enrolled",
      );
      // No budget reset here: an online transition cannot be told apart
      // from a flapping one, and resetting on every event would defeat
      // the failure budget (the auto path caps at six attempts per
      // session; the user-driven retry is the re-arm channel).
      if (tagma) void channelsStore.ensureOpen(tagma, { refresh: true });
    });
    // Wire room-membership-changed nudges into the room roster refresh: a
    // membership change repaints the member count / creator badge without
    // waiting for the room page's poll. Same shell-binding discipline.
    realtimeStore.setRoomMembershipChangedSink((roomId) => {
      void roomConversationsStore.refreshRoster(roomId);
    });
    // Wire room-member presence deltas into the room's live online-member set:
    // a peer's connect/disconnect mutates the set between roster re-fetches.
    realtimeStore.setRoomMemberPresenceSink((roomId, memberId, online) => {
      roomConversationsStore.applyMemberPresence(roomId, memberId, online);
    });
    // Wire read-cursor echoes into the unread store: another session's PUT
    // converges this session's badge in real time (multi-device read state).
    realtimeStore.setRoomReadCursorChangedSink((roomId, lastReadSeq) => {
      unreadStore.applyServerRead(roomId, lastReadSeq);
    });

    void configStore.ready.then(() => {
      const cfg = configStore.value;
      if (isOfflineOnlyShell()) {
        // Offline-only shell (kallip-direct): agora is unreachable by design.
        // Boot the local transport when connect credentials exist; without
        // them there is nothing to boot -- the gate parks the user on
        // /connect, whose submit writes cfg.offline for the next boot. Never
        // touches agoraSession.
        if (cfg?.offline) {
          connectDirect(cfg.offline)
            .then(({ transport, conversationId }) =>
              channelsStore.attachLocal(transport, conversationId),
            )
            .catch((e) => {
              channelsStore.localError = e;
            });
        }
      } else {
        // Resolve the session; the gate reads the settled `user`. The tagma
        // registry fetch + auto-open are driven by the user_id $effect below
        // (which also re-fires on re-login, unlike this one-shot onMount).
        void agoraSession.whoami();
      }
    });
    // One capability probe at boot (both modes): the tagmata page's
    // create card reads it -- hidden while the service is unreachable.
    // Offline-only shells skip it: no route there consumes capabilities, and
    // the instances service is not part of that shell's world.
    if (!isOfflineOnlyShell()) void instancesStore.fetchCapabilities();
  });

  // Load the tagma registry + auto-open channels for online tagmas. Keyed on
  // `user?.user_id` (a stable primitive, NOT the `user` object -- whoami
  // reassigns `user` to a fresh object on every fetch): fires at boot, on
  // re-login (a different user_id), and on a mode flip back to online (the
  // cookie survives offline mode, so user_id is stable but `mode` changes).
  // RootLayout.onMount runs once per SPA session, so a re-login would otherwise
  // never re-fetch the registry; this effect is what makes it happen.
  //
  // The post-refresh sweep opens channels for tagmas already showing online at
  // that moment -- it covers the boot ordering where the SSE presence snapshot
  // landed before the registry loaded (the presence sink misses those, since
  // the registry was empty when they fired). Live transitions and snapshots
  // arriving after the sweep are handled by the presence sink. `ensureOpen` is
  // idempotent, so a transition the sink already handled and the sweep both
  // touch is opened exactly once. NOTE: like the presence sink, this is now a
  // pre-warm convenience, not load-bearing for sidebar visibility (enrolled
  // tagmas always show; /tagma/{tagmaId}/chat opens on demand).
  $effect(() => {
    const uid = agoraSession.user?.user_id;
    if (mode !== "online" || !uid) return;
    void agoraSession.refreshTagmata().then(() => {
      if (!agoraSession.user) return; // logged out mid-flight: gate redirects.
      for (const t of agoraSession.tagmata) {
        if (t.state === "enrolled" && realtimeStore.has(t.tagma_id)) {
          void channelsStore.ensureOpen(t);
        }
      }
    });
  });

  // Load the rooms registry + invite inbox for the signed-in online user. A
  // sibling effect to the tagmata one (rooms are a separate concern; the tagma
  // effect ends in a channel auto-open sweep that is unrelated). Same keying
  // discipline: `user?.user_id` (a stable primitive), not the `user` object.
  $effect(() => {
    const uid = agoraSession.user?.user_id;
    if (mode !== "online" || !uid) return;
    void roomsStore.refresh();
  });

  // The direct-session poller (lesche T↔T): online-only, signed-in. The
  // store skips tagmas without an open channel, so this costs nothing
  // beyond the channels the boot sweep already opens. Same keying
  // discipline as the rooms effect: user_id + mode.
  $effect(() => {
    const uid = agoraSession.user?.user_id;
    if (mode !== "online" || !uid) {
      directSessionsStore.stop();
      return;
    }
    directSessionsStore.start();
    return () => directSessionsStore.stop();
  });

  // Load the signed-in user's passkeys (devices). Gated on `!passkeysLoaded` so
  // it cooperates with SettingsPage's own passkey-load effect (whichever fires
  // first loads; the other no-ops) -- two triggers with the SAME guard, not a
  // maintenance trap. Keyed on user_id; reset() clears passkeysLoaded on logout.
  $effect(() => {
    const uid = agoraSession.user?.user_id;
    if (mode !== "online" || !uid || agoraSession.passkeysLoaded) return;
    void agoraSession.refreshPasskeys();
  });

  // Run the realtime SSE feed (presence + envelope delivery) while signed-in in
  // online mode; tear it down otherwise. Keyed on `user?.user_id` (a stable
  // primitive), NOT the `user` object: whoami() reassigns `user` to a fresh
  // object on every fetch, so keying on the object would cycle the feed on each
  // re-fetch. user_id still changes on login-as-different-user / logout, so the
  // cleanup fires exactly when it should.
  $effect(() => {
    const uid = agoraSession.user?.user_id;
    if (mode === "online" && uid) {
      realtimeStore.start();
      return () => {
        realtimeStore.stop();
      };
    }
  });

  const decision = $derived(
    appGateDecision({
      loaded: configStore.loaded,
      mode,
      user: agoraSession.user,
      authError: agoraSession.authError,
      connected: channelsStore.localConnected,
      pathname,
      search,
      appKind,
    }),
  );

  // Act on a redirect decision. replaceState so the guarded URL never enters
  // history (Back returns to the pre-app referrer, not a redirect loop).
  $effect(() => {
    if (decision.kind === "redirect") {
      void navigate(decision.url, { replaceState: true });
    }
  });

  // The online sidebar lists EVERY enrolled tagma (not just open channels):
  // the indicator reflects the channel transport state, and the entry links to
  // the tagma-keyed route /tagma/{tagmaId}/chat which opens the channel on
  // demand.
  // Channel transport drives the dot; presence feeds its open and absent
  // arms: once it resolves without the peer the entry reads down (the same
  // safe-default policy as the /tagmata dashboard), so a never-online peer
  // cannot spin forever -- auto-open only fires for online tagmas.
  const tagmaNav = $derived(
    agoraSession.enrolledCards.map((c) => ({
      tagmaId: c.tagmaId,
      label: c.label,
      indicator: tagmaNavIndicator(
        channelsStore.getTagmaChannelState(c.tagmaId),
        realtimeStore.resolved && !realtimeStore.has(c.tagmaId),
      ),
      badge: unreadStore.countOf(tagmaKey(c.tagmaId)),
    })),
  );

  const links = $derived(
    navFor({
      mode,
      icons,
      tagmata: tagmaNav,
      rooms: roomsStore.rooms.map((r) => ({
        roomId: r.room_id,
        label: r.name || room_label_fallback({ id: r.room_id.slice(0, 8) }),
        badge: unreadStore.countOf(roomKey(r.room_id)),
      })),
      directs: directSessionsStore.list().map((s) => ({
        tagmaId: s.tagmaId,
        peerId: s.peerId,
        label: directSessionsStore.peerLabel(s.peerId, s.peerHandle),
      })),
      chatsBadge: unreadStore.total(),
    }),
  );

  // Manage-domain mapping: the tagma details sections (and the agent
  // detail beneath them) light the manage cell -- the bar cell and the
  // sidebar's manage item share this one predicate, mirroring the
  // mobileBack manage-domain rule in lib/shell/breadcrumbs.ts.
  function isActive(href: string): boolean {
    if (href === "/tagmata") {
      const segs = pathname.split("/").filter(Boolean);
      if (segs[0] === "tagma" && segs[2] === "details") return true;
    }
    return pathMatches(pathname, href);
  }

  // The rooms notification path (plan D4). The envelope demux is the only
  // point that sees room traffic for conversations nobody is looking at.
  // The floor is the unread store's watermark count: a viewed room never
  // notifies (its transcript is the delivery), an own echo never does, and
  // a zero count means nothing unread (the envelope-before-pull race then
  // suppresses -- the adjudicated conservative direction). The tag is the
  // conversation key so a room's burst stays one notification.
  function maybeNotifyRoom(env: Envelope): void {
    // Mirror the transcript's warn-drop (deliverLive/renderPublic): one
    // malformed payload must not throw inside the sink callback -- the
    // transcript side has already surfaced the decode failure.
    let decoded;
    try {
      decoded = decodeRoomMessage(decodeB64(env.ciphertext));
    } catch (e) {
      console.error("[room notify] decode failed:", e);
      return;
    }
    if (decoded.op !== "message") return; // warn-drop shape, matches the transcript
    const roomId = env.channel_id;
    const key = roomKey(roomId);
    if (
      !shouldNotifyRoom({
        unreadCount: unreadStore.countOf(key),
        viewing: unreadStore.isViewing(key),
        own: env.sender.id === agoraSession.participantId,
      })
    ) {
      return;
    }
    const row = roomsStore.rooms.find((r) => r.room_id === roomId);
    void notify({
      tag: key,
      title: row?.name || room_label_fallback({ id: roomId.slice(0, 8) }),
      body: decoded.text,
    });
  }

  // Offline error: the local conversation's transport-level error (mid-session
  // tagma failure) or localError (a boot-reconnect / mode-switch failure that
  // landed before a local conversation existed). The banner classifies it; the
  // full error is mirrored to the console.
  const offlineError = $derived(
    channelsStore.local?.error ?? channelsStore.localError,
  );
  const errorView = $derived(offlineError ? classifyError(offlineError) : null);
  $effect(() => {
    if (offlineError) console.error(offlineError);
  });
  // The small-screen back row's target. Offline content pages (any
  // /local/* below the home itself) swap the bottom bar for the row and
  // drill home; online deep pages drill the same way, with the target
  // derived from the trail table (mobileBack in lib/shell/breadcrumbs.ts).
  // Desktop is unaffected: the row renders only in the mobile shell.
  const back = $derived(
    mode === "offline"
      ? pathname.startsWith("/local/") && pathname !== "/local"
        ? { href: "/local", label: nav_home() }
        : null
      : mobileBack(pathname),
  );
  // The chat deep routes: their status line lives in the shell's mobile
  // top row, beside the back chevron -- one row, not page chrome plus
  // shell row. /chat/:id and /local/chat carry the store key directly;
  // the /tagma/:id/chat shape carries a tagma id, resolved through the
  // store (the pathname id is NOT the conversation id).
  const chatId = $derived.by(() => {
    const route = chatRoute(pathname);
    if (!route) return undefined;
    return route.kind === "conversation"
      ? route.conversationId
      : channelsStore.conversationIdForTagma(route.tagmaId);
  });
  // Mobile top-row titles: static i18n headings mapped by route (the
  // pages keep their own h1 for md+; see AppShell `title`). Covers
  // the manage hub and sub-pages plus the four bar-destination
  // pages, whose small-screen headings render here, not in-page.
  const mobileTitles: Record<string, () => string> = {
    "/local/manage": nav_manage,
    "/local/manage/overview": manage_overview_heading,
    "/local/manage/budget": manage_budget_heading,
    "/local/manage/agents": manage_agents_heading,
    "/local/manage/profiles": manage_profiles_heading,
    "/local/manage/schedules": manage_schedules_heading,
    "/tagmata": nav_manage,
    "/files": nav_files,
    "/settings": settings_heading,
    "/account": account_menu,
  };
  const mobileTitle = $derived(mobileTitles[pathname]?.());
  // Mobile status expansion owned by the shell's topPanel pair (chat only).
  let statusExpanded = $state(false);
  // Hoisted state survives navigation; reset on route change so returning
  // to chat starts collapsed like the old in-page header did.
  $effect(() => {
    pathname;
    statusExpanded = false;
  });
</script>

<!-- Sidebar footer entry; see AccountMenu for behavior. -->
{#snippet statusSnippet()}
  <AccountMenu />
{/snippet}

<!-- Mobile top row for the chat deep pages: a one-line status
     summary (TagmaStatusLine) rides beside the back chevron; the
     expanded half (budget + agent rows) renders as the shell's
     topPanel below the row, so the row stays one line tall in both
     states. -->
{#snippet topRowSnippet()}
  {#if chatId}
    <TagmaStatusLine
      status={channelsStore.get(chatId)?.statusSnapshot}
      expanded={statusExpanded}
      onToggle={() => (statusExpanded = !statusExpanded)}
    />
  {/if}
{/snippet}

{#snippet topPanelSnippet()}
  {#if chatId && statusExpanded}
    <TagmaStatusPanel
      status={channelsStore.get(chatId)?.statusSnapshot}
      agentRows={{
        rootRow: statusCardStore.rootRow,
        subRows: statusCardStore.subRows,
      }}
    />
  {/if}
{/snippet}

{#if decision.kind === "render" && isPublicRoute(pathname)}
  {@render children()}
{:else if decision.kind === "render"}
  <AppShell
    {links}
    {pathname}
    {isActive}
    {back}
    topRow={back && !mobileTitle ? topRowSnippet : undefined}
    topPanel={back && !mobileTitle ? topPanelSnippet : undefined}
    title={mobileTitle}
    error={errorView}
    status={statusSnippet}
  >
    {@render children()}
  </AppShell>
{:else}
  <!-- skeleton: config still loading (mode unknown) or whoami in flight (online,
       no error yet). An auth failure routes the user to /login (see
       appGateDecision), so this branch is only the brief resolving window.
       Never a protected AppShell, so no gated content flashes. -->
  <div class="p-4"><p class="opacity-60">Loading…</p></div>
{/if}
