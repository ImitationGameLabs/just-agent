// Daemon instances store: the read-only face of the offline home. Polls
// the daemon web proxy for the daemon's liveness and its instance list;
// mutations (spawn, stop) join this store in a later batch.

import {
  type DaemonHealth,
  type DaemonInstance,
  DaemonWebClient,
  DaemonWebError,
  type DaemonWebErrorKind,
} from "./client.ts";
import { manage_instances_load_failed } from "../../paraglide/messages.js";

class InstancesStore {
  private readonly client = new DaemonWebClient();
  private pollHandle: ReturnType<typeof setInterval> | null = null;

  health = $state<DaemonHealth | null>(null);
  instances = $state<DaemonInstance[]>([]);
  isLoading = $state(false);
  error = $state<string | null>(null);
  errorKind = $state<DaemonWebErrorKind | null>(null);

  /** Fetch both faces once; a classified error lands in error/errorKind. */
  async refresh(): Promise<void> {
    this.isLoading = true;
    try {
      const [health, instances] = await Promise.all([
        this.client.health(),
        this.client.list(),
      ]);
      this.health = health;
      this.instances = instances;
      this.error = null;
      this.errorKind = null;
    } catch (cause) {
      if (cause instanceof DaemonWebError) {
        this.errorKind = cause.kind;
        this.error = cause.message;
      } else {
        this.errorKind = "other";
        this.error = manage_instances_load_failed();
      }
    } finally {
      this.isLoading = false;
    }
  }

  startPolling(intervalMs = 5000): void {
    this.stopPolling();
    this.refresh();
    this.pollHandle = setInterval(() => {
      if (!this.isLoading) this.refresh();
    }, intervalMs);
  }

  stopPolling(): void {
    if (this.pollHandle !== null) {
      clearInterval(this.pollHandle);
      this.pollHandle = null;
    }
  }
}

export const instancesStore = new InstancesStore();
