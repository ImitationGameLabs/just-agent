// Client for the daemon's local web proxy (kallip-daemon-web): the offline
// home reads machine-level instance state through its /api/daemon/* HTTP
// face. Deliberately separate from manage/backend.ts — the daemon proxy is a
// third backend with its own uniform {code, message} error shape, and
// instance lifecycle is not a tagma management concern.
/** sessionStorage key holding the standalone-mode bearer token; when set,
 * every request carries it as the authorization header.
 */
export const DAEMON_TOKEN_KEY = "kallip:daemon-token";

/** One-shot handoff key: the spawn form parks the new instance's operator
 * token here when the user provided one, and /connect picks it up exactly
 * once (read + remove) so the token never rides the URL.
 */
export const CONNECT_TOKEN_KEY = "kallip:connect-token";

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

/** Spawn request: the daemon's allowlisted env pairs ride as KEY=VALUE.
 */
export interface DaemonSpawnInput {
  slug: string;
  workspace: string;
  env: string[];
}

/** One freshly launched instance (the proxy unwraps the wire payload).
 */
export interface DaemonSpawnResult {
  slug: string;
  pid: number;
  port: number;
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
    readonly code?: string,
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

  /** Launch one instance; the response carries its listen port.
   */
  spawn(input: DaemonSpawnInput): Promise<DaemonSpawnResult> {
    return this.post<DaemonSpawnResult>("/spawn", input);
  }

  /** Stop one instance by slug.
   */
  stop(slug: string): Promise<{ slug: string }> {
    return this.post<{ slug: string }>("/stop", { slug });
  }

  private async request<T>(path: string, init?: RequestInit): Promise<T> {
    let response: Response;
    try {
      response = await fetch(this.base + path, {
        ...init,
        headers: { ...this.authHeaders(), ...init?.headers },
      });
    } catch (cause) {
      throw new DaemonWebError(
        "unreachable",
        "daemon web proxy unreachable: " + path,
        undefined,
        undefined,
        { cause },
      );
    }
    if (response.ok) {
      return (await response.json()) as T;
    }
    throw await this.fault(response);
  }

  private get<T>(path: string): Promise<T> {
    return this.request<T>(path);
  }

  private post<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>(path, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  /** Headers every exchange carries; a stored token rides along.
   */
  private authHeaders(): Record<string, string> {
    const headers: Record<string, string> = { accept: "application/json" };
    const token = sessionStorage.getItem(DAEMON_TOKEN_KEY);
    if (token) {
      headers.authorization = "Bearer " + token;
    }
    return headers;
  }

  /** Map one non-2xx response onto the page's branch kinds, keeping the
   * proxy's machine-readable code for message selection.
   */
  private async fault(response: Response): Promise<DaemonWebError> {
    const { code, message } = await faultBody(response);
    switch (response.status) {
      case 401:
        return new DaemonWebError(
          "unauthorized",
          message,
          response.status,
          code,
        );
      case 403:
        return new DaemonWebError("forbidden", message, response.status, code);
      case 502:
      case 503:
      case 504:
        return new DaemonWebError(
          "unreachable",
          message,
          response.status,
          code,
        );
      default:
        return new DaemonWebError("other", message, response.status, code);
    }
  }
}

/** The fault body's code and human message, or the status line when the
 * body isn't JSON.
 */
async function faultBody(
  response: Response,
): Promise<{ code?: string; message: string }> {
  try {
    const body = (await response.json()) as { code?: string; message?: string };
    return {
      code: body.code,
      message: body.message ?? "http " + String(response.status),
    };
  } catch {
    return { message: "http " + String(response.status) };
  }
}
