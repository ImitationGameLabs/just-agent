// Pure view helpers extracted from ProfilesPage: probe-report merging into
// keyed maps, draft-derived id lists, probe status labels/colors (i18n via
// paraglide — same-layer precedent as compute.ts), and the parked-live
// snapshot core. The page keeps the $state/$derived/store plumbing; these
// take the maps/values as parameters so tests can drive them directly
// (profiles-view_test.ts).

import type {
  AgentStatusResponse,
  ProfileConfig,
  ProfileModelProbeReport,
  ProfileProbeResponse,
  ProfileProbeStatus,
  ProfileProviderProbeReport,
} from "@kallipai/kallip-client";
import {
  manage_profiles_probe_models_one,
  manage_profiles_probe_models_other,
  manage_profiles_probe_status_invalid,
  manage_profiles_probe_status_ok,
  manage_profiles_probe_status_partial,
  manage_profiles_probe_status_unauthorized,
  manage_profiles_probe_status_unreachable,
} from "../../paraglide/messages.js";

/** `${setName}:${profileId}` — profile ids can repeat across sets. */
export function profileKey(setName: string, profileId: string): string {
  return `${setName}:${profileId}`;
}

/** Merge a probe response's provider-scope results into the page-held map. */
export function mergeProviderScope(
  reports: Map<string, ProfileProviderProbeReport>,
  resp: ProfileProbeResponse,
): void {
  for (const r of resp.results) reports.set(r.endpoint_id, r);
}

/** Merge a set-scoped response: sets[0] is the requested set. */
export function mergeProfileScope(
  setName: string,
  reports: Map<string, ProfileModelProbeReport>,
  resp: ProfileProbeResponse,
): void {
  const s = resp.sets[0];
  if (!s) return;
  for (const p of s.profiles) {
    reports.set(profileKey(setName, p.profile_id), p);
  }
}

/**
 * Merge an all-scope response: each reported set carries its name, so the
 * reports map keys straight off it — set name + profile id.
 */
export function mergeProfileScopeAll(
  reports: Map<string, ProfileModelProbeReport>,
  resp: ProfileProbeResponse,
): void {
  for (const s of resp.sets) {
    for (const p of s.profiles) {
      reports.set(profileKey(s.name, p.profile_id), p);
    }
  }
}

/** Drop the stored report for one profile (set-scoped key). */
export function clearProfileResult(
  reports: Map<string, ProfileModelProbeReport>,
  setName: string,
  profileId: string,
): void {
  reports.delete(profileKey(setName, profileId));
}

/** Endpoint (provider) ids of the draft config. */
export function providerIdsOf(
  draft: ProfileConfig | null | undefined,
): string[] {
  return Object.keys(draft?.endpoints ?? {});
}

/**
 * Every profile id visible in the draft — sets ∪ parking (the parking
 * dialog's new-mode duplicate check; advisory only, PUT is authoritative).
 */
export function occupiedIdsOf(
  draft: ProfileConfig | null | undefined,
): string[] {
  const ids = Object.values(draft?.sets ?? {}).flatMap((s) =>
    s.profiles.map((p) => p.id),
  );
  return [...ids, ...(draft?.parking ?? []).map((p) => p.id)];
}

export function probeStatusLabel(s: ProfileProbeStatus): string {
  switch (s) {
    case "ok":
      return manage_profiles_probe_status_ok();
    case "partial":
      return manage_profiles_probe_status_partial();
    case "unreachable":
      return manage_profiles_probe_status_unreachable();
    case "unauthorized":
      return manage_profiles_probe_status_unauthorized();
    case "invalid_config":
      return manage_profiles_probe_status_invalid();
  }
}

export const probeStatusColor: Record<ProfileProbeStatus, string> = {
  ok: "text-success-500 dark:text-success-400",
  partial: "text-warning-500 dark:text-warning-400",
  unreachable: "text-error-500 dark:text-error-400",
  unauthorized: "text-error-500 dark:text-error-400",
  invalid_config: "text-error-500 dark:text-error-400",
};

export function modelsCountLabel(count: number): string {
  return count === 1
    ? manage_profiles_probe_models_one({ count })
    : manage_profiles_probe_models_other({ count });
}

/**
 * Parked-live snapshot core: which parked ids some live agent still runs.
 * Takes the settled per-agent statuses (the page fetches them; per-agent
 * failures arrive as rejections and are skipped — one 409/404 must not void
 * the advisory snapshot). Null = nothing parked-live.
 */
export function parkedLiveSnapshot(
  parkedIds: string[],
  statuses: PromiseSettledResult<AgentStatusResponse>[],
): { agentCount: number; profileIds: string[] } | null {
  const parked = new Set(parkedIds);
  let agentCount = 0;
  const profileIds = new Set<string>();
  for (const s of statuses) {
    if (s.status !== "fulfilled") continue;
    const pid = s.value.profile?.profile_id;
    if (pid && parked.has(pid)) {
      agentCount++;
      profileIds.add(pid);
    }
  }
  return agentCount > 0 ? { agentCount, profileIds: [...profileIds] } : null;
}
