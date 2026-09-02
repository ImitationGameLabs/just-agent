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
