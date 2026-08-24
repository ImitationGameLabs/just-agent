// Client for the daemon's local web proxy (kallip-daemon-web): the offline
// home reads machine-level instance state through its /api/daemon/* HTTP
// face. Deliberately separate from manage/backend.ts — the daemon proxy is a
// third backend with its own uniform {code, message} error shape, and
// instance lifecycle is not a tagma management concern.

/** One managed instance as the daemon reports it (wire mirror).
 */
export interface DaemonInstance {
  slug: string;
  instance_id: string;
  workspace: string;
  running: boolean;
  owner: number | null;
}

/** Liveness report: the daemon itself, or one instance by slug.
 */
export interface DaemonHealth {
  slug: string | null;
  running: boolean;
  detail: string | null;
}

/** The error kinds the offline instances page branches on.
 */
export type DaemonWebErrorKind =
  | "unauthorized"
  | "forbidden"
  | "unreachable"
  | "other";

/** One failed exchange, classified to the kind the page renders.
 */
export class DaemonWebError extends Error {
  constructor(
    readonly kind: DaemonWebErrorKind,
    message: string,
    readonly status?: number,
    options?: ErrorOptions,
  ) {
    super(message, options);
  }
}

export class DaemonWebClient {
  private readonly base: string;

  constructor(baseUrl = "/api/daemon") {
    this.base = baseUrl.replace(/\/+$/, "");
  }

  /** The daemon's own liveness (no slug).
   */
  health(): Promise<DaemonHealth> {
    return this.get<DaemonHealth>("/health");
  }

  /** Every instance the daemon sees, with its running bit.
   */
  async list(): Promise<DaemonInstance[]> {
    const body = await this.get<{ instances: DaemonInstance[] }>("/list");
    return body.instances;
  }

  private async get<T>(path: string): Promise<T> {
    let response: Response;
    try {
      response = await fetch(this.base + path, {
        headers: { accept: "application/json" },
      });
    } catch (cause) {
      throw new DaemonWebError(
        "unreachable",
        "daemon web proxy unreachable: " + path,
        undefined,
        { cause },
      );
    }
    if (response.ok) {
      return (await response.json()) as T;
    }
    throw this.fault(response.status, await faultMessage(response));
  }

  /** Map one non-2xx response onto the page's branch kinds.
   */
  private fault(status: number, message: string): DaemonWebError {
    switch (status) {
      case 401:
        return new DaemonWebError("unauthorized", message, status);
      case 403:
        return new DaemonWebError("forbidden", message, status);
      case 502:
      case 503:
      case 504:
        return new DaemonWebError("unreachable", message, status);
      default:
        return new DaemonWebError("other", message, status);
    }
  }
}

/** The fault body's human message, or the status line if it isn't JSON.
 */
async function faultMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { message?: string };
    return body.message ?? "http " + String(response.status);
  } catch {
    return "http " + String(response.status);
  }
}
