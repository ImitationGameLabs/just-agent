// Daemon instances store: the read-only face of the offline home. Polls
// the daemon web proxy for the daemon's liveness and its instance list;
// spawn/stop mutations rethrow their classified errors for the calling
// surface (form, confirm dialog) to render; the list refreshes on success.

import {
  type DaemonHealth,
  type DaemonInstance,
  type DaemonSpawnInput,
  type DaemonSpawnResult,
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
  /** True once any refresh has succeeded; gates the first-frame loading
   * line so the 5s poll never flashes it over live data.
   */
  loaded = $state(false);
  /** The proxy's machine-readable error code (e.g. host_forbidden). */
  errorCode = $state<string | null>(null);
  /** The listen port of each instance spawned in this session, by slug:
   * the list wire has no port, so the Chat CTA needs this memory.
   */
  spawnedPorts = $state<Record<string, number>>({});

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
      this.errorCode = null;
      this.errorKind = null;
      this.loaded = true;
    } catch (cause) {
      if (cause instanceof DaemonWebError) {
        this.errorKind = cause.kind;
        this.errorCode = cause.code ?? null;
        this.error = cause.message;
      } else {
        this.errorKind = "other";
        this.error = manage_instances_load_failed();
      }
    } finally {
      this.isLoading = false;
    }
  }

  /** Spawn one instance; success records its port and refreshes the list.
   * Errors rethrow classified for the form to render.
   */
  async spawn(input: DaemonSpawnInput): Promise<DaemonSpawnResult> {
    const result = await this.client.spawn(input);
    this.spawnedPorts[input.slug] = result.port;
    await this.refresh();
    return result;
  }

  /** Stop one instance by slug; errors rethrow for the dialog to render. */
  async stop(slug: string): Promise<void> {
    await this.client.stop(slug);
    await this.refresh();
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
