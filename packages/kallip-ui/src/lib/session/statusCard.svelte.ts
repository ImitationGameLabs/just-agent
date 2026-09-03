// Status-card rows: the per-agent list under the chat status bar. Roster
// refresh is event-nudged first -- the chat page routes every status-snapshot
// update (relay `tagma_status` push online, direct SSE drain offline) into
// nudge() -- with a slow reconciliation poll (15s) as the dropped-frame
// backstop; the per-agent context poll runs slower still (30s, occupancy is
// approximate). Both cadences pause while the tab is hidden, over whichever
// ManagementBackend the conversation provides (OnlineBackend on the relay
// channel, OfflineBackend direct). Context occupancy mirrors the detail
// page's approximation (turn tokens + pinned) with the registry profile's
// max window as denominator; faulted/parked agents get no context column --
// their status response is a 409 or meaningless.

import type { AgentState, WireParkedReason } from "@kallipai/kallip-client";
import { type ManagementBackend } from "../manage/backend.ts";
import { startVisibleInterval } from "../visibleInterval.ts";

/** One rendered row. `contextTokens` is null until the slow poll lands (or
 * forever, for faulted/parked agents). */
export interface StatusCardRow {
  readonly id: string;
  readonly state: AgentState;
  readonly role: string;
  readonly activity: string;
  /** Wire's free-form words for the agent; drives the row hover tooltip.
   * Empty when the wire does not set one. */
  readonly description: string;
  readonly contextTokens: number | null;
  /** Registry max window for the row's profile; null until the
   * attach-time registry pull lands (or the profile is unknown). */
  readonly contextWindow: number | null;
  /** Present only for parked rows; appended to the row's hover tooltip. */
  readonly parkedReason: WireParkedReason | null;
}

// Attention order: work states rise, terminal anomalies stay visible above
// idle (operator 04:36 ruling -- a fault must be seen), idle sinks into the
// fold. Keys are state + id only, never activity text, so a poll refresh
// re-renders in place instead of reshuffling rows.
const STATE_ORDER: Record<AgentState, number> = {
  busy: 0,
  waiting: 1,
  retrying: 2,
  faulted: 3,
  parked: 4,
  idle: 5,
};

class StatusCardStore {
  rootRow = $state<StatusCardRow | null>(null);
  subRows = $state<readonly StatusCardRow[]>([]);

  private backend: ManagementBackend | null = null;
  private rosterStop: (() => void) | null = null;
  private contextStop: (() => void) | null = null;
  private contexts = new Map<string, number>();
  private lastSubSignature = "";
  // Denominator data: the profile registry pulled once per attach (it
  // changes rarely and only takes effect on apply), and each agent's
  // active profile id captured by the slow context poll.
  private profileWindows = new Map<string, number>();
  private profileIds = new Map<string, string>();
  // Interval re-entry guards: both refreshes are async, and a slow network
  // must not stack a second concurrent round on a still-running one.
  private rosterRunning = false;
  private contextsRunning = false;
  /** Set once the first contexts round has been attempted (success or
   * not): the roster-triggered first pull must fire exactly once even
   * when every getAgentStatus fails, or the two recurse. */

  private contextsPrimed = false;
  /** Page unmount: stop the pollers, keep every cached row/number so a
   * return to the page paints instantly and only background-refreshes. */
  suspend(): void {
    this.rosterStop?.();
    this.contextStop?.();
    this.rosterStop = null;
    this.contextStop = null;
    this.backend = null;
  }

  /** Full reset (mode switch / logout): pollers and all cached data. */
  detach(): void {
    this.suspend();
    this.rootRow = null;
    this.subRows = [];
    this.contexts.clear();
    this.profileWindows.clear();
    this.profileIds.clear();
    this.lastSubSignature = "";
    this.contextsPrimed = false;
  }

  attach(backend: ManagementBackend): void {
    // Idempotent re-attach: returning to the page keeps the warm cache
    // (rows, contexts, windows) and only background-refreshes, so the
    // panel paints instantly from the previous visit's data.
    const resuming = this.backend === null && this.rootRow !== null;
    this.suspend();
    this.backend = backend;
    this.refreshRoster();
    this.rosterStop = startVisibleInterval(() => this.refreshRoster(), 15_000);
    this.contextStop = startVisibleInterval(
      () => this.refreshContexts(),
      30_000,
    );
    void this.refreshProfileWindows(backend);
    if (resuming) this.refreshContexts();
  }
  /** Event-driven roster refresh: the chat page routes every status-snapshot
   * update (the relay `tagma_status` push online, the direct SSE drain
   * offline) into this, so state flips paint at once instead of waiting for
   * the reconciliation poll. The re-entry guard collapses overlapping
   * nudges; the interval remains the dropped-frame backstop. */
  nudge(): void {
    void this.refreshRoster();
  }

  private async refreshRoster(): Promise<void> {
    if (this.rosterRunning) return;
    const backend = this.backend;
    if (!backend) return;
    this.rosterRunning = true;
    try {
      const resp = await backend.listAgents();
      if (this.backend !== backend) return; // detached mid-flight
      let root: StatusCardRow | undefined;
      const subs: StatusCardRow[] = [];
      for (const a of resp.agents) {
        const row: StatusCardRow = {
          id: a.id,
          state: a.state,
          role: a.role,
          activity: a.activity,
          description: a.description ?? "",
          contextTokens: this.contextOf(a),
          contextWindow: this.windowOf(a.id),
          parkedReason: a.parked_reason ?? null,
        };
        if (a.created_by === null) root = row;
        else subs.push(row);
      }
      this.rootRow = root ?? null;
      subs.sort(
        (a, b) =>
          STATE_ORDER[a.state] - STATE_ORDER[b.state] ||
          (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
      );
      // Keep the previous array (and row objects) when nothing observable
      // changed: Svelte skips the update, so DOM rows do not reshuffle.
      const signature = subs
        .map(
          (r) =>
            `${r.id}:${r.state}:${r.role}:${r.activity}:${r.contextTokens}:${r.description}:${r.contextWindow}`,
        )
        .join("|");
      if (signature !== this.lastSubSignature) {
        this.lastSubSignature = signature;
        this.subRows = subs;
      }
      // First roster with rows: pull contexts right away instead of
      // waiting for the first 30s tick (the numbers were invisible for
      // a full period otherwise).
      if ((root !== undefined || subs.length > 0) && !this.contextsPrimed) {
        void this.refreshContexts();
        this.contextsPrimed = true;
      }
    } catch {
      /* transient poll failure: keep the last roster */
    } finally {
      this.rosterRunning = false;
    }
  }

  /** Slow poll: context occupancy for live agents, one request each. */
  private async refreshContexts(): Promise<void> {
    if (this.contextsRunning) return;
    const backend = this.backend;
    if (!backend) return;
    this.contextsRunning = true;
    this.contextsPrimed = true;
    try {
      const targets = [
        ...(this.rootRow ? [this.rootRow] : []),
        ...this.subRows,
      ].filter((r) => r.state !== "faulted" && r.state !== "parked");
      // Parallel, not serial: N agents cost one RTT round, not N. The
      // per-agent catch keeps one dead agent from sinking the round.
      await Promise.all(
        targets.map(async (row) => {
          try {
            const status = await backend.getAgentStatus(row.id);
            if (this.backend !== backend) return;
            this.contexts.set(
              row.id,
              status.context.turn_tokens +
                status.context.pinned_items.reduce((sum, [, n]) => sum + n, 0),
            );
            const pid = status.profile?.profile_id;
            if (pid) this.profileIds.set(row.id, pid);
          } catch {
            /* agent gone or not yet responsive: leave the stale value */
          }
        }),
      );
      this.refreshRoster(); // re-merge context numbers into rows
    } finally {
      this.contextsRunning = false;
    }
  }

  private contextOf(a: { id: string; state: AgentState }): number | null {
    if (a.state === "faulted" || a.state === "parked") return null;
    return this.contexts.get(a.id) ?? null;
  }

  /** Registry pull for context denominators (fire-and-forget, best
   * effort: on failure every row shows "x / —" until the next attach). */
  private async refreshProfileWindows(
    backend: ManagementBackend,
  ): Promise<void> {
    try {
      const config = await backend.getProfiles();
      if (this.backend !== backend) return;
      for (const set of Object.values(config.sets)) {
        for (const p of set.profiles) {
          this.profileWindows.set(p.id, p.max_context_window);
        }
      }
      this.refreshRoster(); // re-merge windows into rows
    } catch {
      /* registry unavailable: denominators stay null */
    }
  }

  private windowOf(id: string): number | null {
    const pid = this.profileIds.get(id);
    return pid ? (this.profileWindows.get(pid) ?? null) : null;
  }
}

export const statusCardStore = new StatusCardStore();
