/**
 * Single source for the shell breadcrumb trail: a route table keyed by
 * pathname pattern, rendered by the desktop shell's one chrome bar. A
 * shell page gets
 * its trail by adding a table entry, not by mounting a component -- so a new
 * page cannot forget the trail or drift from the chrome (the shell owns
 * divider, height, and segment style). A route with no entry renders no bar
 * at all, which keeps the offline /local/* tree bar-free; the one deliberate
 * tabled exception to the shell-only rule is the agent detail route below,
 * which retired its page-level trail in the same batch.
 *
 * Resolvers run inside the shell's $derived, so store reads (room names,
 * channel labels) stay reactive while a deep view loads. The matching engine
 * itself lives in trailMatch.ts, dependency-free and unit-tested there.
 */
import { agoraSession } from "../session/agora.svelte.ts";
import { channelsStore } from "../session/channels.svelte.ts";
import { roomsStore } from "../session/rooms.svelte.ts";
import { RelayConversation } from "../session/conversation.svelte.ts";
import { directSessionsStore } from "../session/directSessions.svelte.ts";
import {
  tagmaDetailsPath,
  tagmaDetailsSectionPath,
  type TagmaDetailsSection,
} from "./routes.ts";
import {
  account_menu,
  chat_title_local,
  nav_budget,
  nav_breadcrumb_agents,
  nav_breadcrumb_tagma,
  nav_chat,
  nav_chats,
  nav_overview,
  nav_profiles,
  nav_rooms,
  nav_schedules,
  nav_tagmata,
  room_label_fallback,
  settings_heading,
  tagma_fallback_label,
} from "../../paraglide/messages.js";
import {
  entry,
  matchTrail as matchTable,
  backFromTrail,
  type TrailEntry,
  type With,
} from "./trailMatch.ts";

// Re-exported so the shell (and any future consumer) can treat this module
// as the single import surface for the trail: the table plus the matcher.
export {
  type BreadcrumbSegment,
  type TrailEntry,
  type TrailParams,
  type With,
} from "./trailMatch.ts";

// The details sections beneath the tagma hub, labeled exactly as the
// OnlineManagePage headings (nav_breadcrumb_agents, not nav_agents: the
// trail word is "agents" in the breadcrumb register).
const sectionLabels: Record<TagmaDetailsSection, () => string> = {
  overview: nav_overview,
  budget: nav_budget,
  agents: nav_breadcrumb_agents,
  profiles: nav_profiles,
  schedules: nav_schedules,
};

export const trailTable: TrailEntry[] = [
  entry("/tagmata", () => [{ label: nav_tagmata(), current: true }]),
  entry("/tagma/:id", ({ id }: With<"id">) => [
    { label: nav_tagmata(), href: "/tagmata" },
    {
      label:
        agoraSession.enrolledCards.find((t) => t.tagmaId === id)?.label ??
        tagma_fallback_label({ id: id.slice(0, 8) }),
      current: true,
    },
  ]),
  entry("/tagma/:id/chat", ({ id }: With<"id">) => [
    { label: nav_breadcrumb_tagma(), href: tagmaDetailsPath(id) },
    { label: nav_chat(), current: true },
  ]),
  entry(
    "/tagma/:id/details/:section",
    ({ id, section }: With<"id" | "section">) => [
      { label: nav_breadcrumb_tagma(), href: tagmaDetailsPath(id) },
      { label: sectionLabels[section as TagmaDetailsSection](), current: true },
    ],
  ),
  entry(
    "/tagma/:id/details/agents/:agentId",
    ({ id, agentId }: With<"id" | "agentId">) => [
      { label: nav_breadcrumb_tagma(), href: tagmaDetailsPath(id) },
      {
        label: nav_breadcrumb_agents(),
        href: tagmaDetailsSectionPath(id, "agents"),
      },
      // The agent's role lives in the page's own backend (arch M2 keeps this
      // route off the global store), so the trail tail carries the id prefix
      // and the page header carries the role -- the same fallback branch the
      // page itself used before the trail moved here.
      { label: agentId.slice(0, 8), current: true },
    ],
  ),
  entry("/rooms", () => [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: nav_rooms(), current: true },
  ]),
  entry("/rooms/:id", ({ id }: With<"id">) => {
    const name = roomsStore.rooms.find((r) => r.room_id === id)?.name;
    return [
      { label: nav_rooms(), href: "/rooms" },
      {
        label: name || room_label_fallback({ id: id.slice(0, 8) }),
        current: true,
      },
    ];
  }),
  entry("/rooms/:id/settings", ({ id }: With<"id">) => {
    const name = roomsStore.rooms.find((r) => r.room_id === id)?.name;
    return [
      { label: nav_rooms(), href: "/rooms" },
      {
        label: name || room_label_fallback({ id: id.slice(0, 8) }),
        href: `/rooms/${id}`,
      },
      { label: settings_heading(), current: true },
    ];
  }),
  entry("/chat/:id", ({ id }: With<"id">) => {
    const conv = channelsStore.get(id);
    const label =
      conv instanceof RelayConversation && conv.label !== null
        ? conv.label
        : id === "local"
          ? chat_title_local()
          : id.slice(0, 8);
    return [
      { label: nav_tagmata(), href: "/tagmata" },
      { label, current: true },
    ];
  }),
  // The direct-session transcript: chat-domain (the entering section is the
  // chats hub, which the trail chain yields as the mobile back target).
  entry("/tagma/:id/direct/:peer", ({ peer }: With<"id" | "peer">) => [
    { label: nav_chats(), href: "/chats" },
    // `id` (the fetch-through daemon) is deliberately not in the label:
    // the peer is what the breadcrumb names.
    { label: directSessionsStore.peerLabel(peer), current: true },
  ]),
  entry("/settings", () => [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: settings_heading(), current: true },
  ]),
  entry("/account", () => [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: account_menu(), current: true },
  ]),
  // The matcher decodes captures once, aligned with the decoded params the
  // framework hands the page shells -- the resolver uses them as-is.
  entry("/user/:handle", ({ handle }: With<"handle">) => [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: handle, current: true },
  ]),
];

/** Match a pathname against the route table; the first entry whose pattern
 * matches segment-for-segment wins. Returns null for untabled routes (the
 * shell renders no bar there). */
export function matchTrail(pathname: string) {
  return matchTable(trailTable, pathname);
}

/** The small-screen back row's target: trail-derived (the deepest linked
 * segment is the parent, the same chain the desktop bar renders), with
 * the route policy the pure engine must not own. Conversations are the
 * one override: their mobile parent is the chats hub the bar cell lists
 * them under, not the manage registry the desktop chain names. /account
 * is excluded: it is a bar cell destination -- swapping the bar for a
 * back row there would strand the other cells. The hubs yield null by
 * construction (/tagmata is a pure tail, /chats is off-table) and keep
 * the bar. */
export function mobileBack(
  pathname: string,
): { href: string; label: string } | null {
  if (pathname === "/account") return null;
  const segs = pathname.split("/").filter(Boolean);
  // Conversations live in the chats domain wherever they route from: the
  // relay chat and the tagma chat both drill back to the chats hub.
  if (pathname === "/chat" || pathname.startsWith("/chat/")) {
    return { href: "/chats", label: nav_chats() };
  }
  if (segs.length === 3 && segs[0] === "tagma" && segs[2] === "chat") {
    return { href: "/chats", label: nav_chats() };
  }
  // The tagma details sections are manage-domain: they keep the bottom
  // bar (with the manage cell lit, see RootLayout isActive) instead of
  // a back row. The agent detail below them stays a drill (back = its
  // agents section), and so does the tagma hub itself.
  if (segs.length === 4 && segs[0] === "tagma" && segs[2] === "details") {
    return null;
  }
  return backFromTrail(matchTrail(pathname));
}
