// The online shell's confirm flow keys off the structured dangling list a
// 409 profiles save carries. These tests pin that the relay path
// reconstructs the full KallipError — not a flattened message-only one —
// and that the save-failure classification still routes a bare 409 (old
// backend) to the stale-backend branch.

import { assertEquals } from "@std/assert";
import { KallipError } from "@kallipai/kallip-common";
import {
  type ManageRestClient,
  ProjectionClient,
} from "@kallipai/kallip-lesche-client";
import { OnlineBackend } from "./backend.ts";

import { classifySaveFailure } from "./profiles-view.ts";
function restWith(body: unknown): ManageRestClient {
  return {
    manage: () => Promise.resolve({ status: 409, body }),
  } as unknown as ManageRestClient;
}

const PUT_BODY = {
  endpoints: {},
  sets: [],
  parking: [],
  default: "",
};

const DANGLING_BODY = {
  error: {
    message: "config drops sets still bound by agents: agent-x → 'alt'",
    dangling: ["agent-x → 'alt'"],
  },
};

Deno.test(
  "a relayed 409 with a dangling list reaches the confirm flow",
  async () => {
    const backend = new OnlineBackend(restWith(DANGLING_BODY), "t-a");
    let caught: unknown = null;
    await backend.updateProfiles({ ...PUT_BODY, force: false }).catch((e) => {
      caught = e;
    });
    assertEquals(caught instanceof KallipError, true);
    const err = caught as KallipError;
    assertEquals(err.api.status, 409);
    assertEquals(err.api.dangling, ["agent-x → 'alt'"]);
    // The store's catch classifies this as the confirm-flow branch.
    assertEquals(classifySaveFailure(err, false), "park-dangling");
  },
);

Deno.test(
  "a bare relayed 409 (old backend) still lands on the downgrade branch",
  async () => {
    const backend = new OnlineBackend(
      restWith({ error: { message: "config drops sets still bound" } }),
      "t-a",
    );
    let caught: unknown = null;
    await backend.updateProfiles({ ...PUT_BODY, force: true }).catch((e) => {
      caught = e;
    });
    assertEquals(caught instanceof KallipError, true);
    const err = caught as KallipError;
    assertEquals(err.api.status, 409);
    assertEquals(err.api.dangling, undefined);
    // With force and no structured list: the old-backend downgrade.
    assertEquals(classifySaveFailure(err, true), "stale-backend");
  },
);

Deno.test(
  "a plain-text relayed 403 folds into the same KallipError shape",
  async () => {
    // The proxy answers forbidden agents with a bare text body; the
    // string branch must yield the same KallipError shape the envelope
    // path produces.
    const rest = {
      manage: () => Promise.resolve({ status: 403, body: "not your tagma" }),
    } as unknown as ManageRestClient;
    const backend = new OnlineBackend(rest, "t-a");
    let caught: unknown = null;
    await backend.getBudget().catch((e) => {
      caught = e;
    });
    assertEquals(caught instanceof KallipError, true);
    const err = caught as KallipError;
    assertEquals(err.api.status, 403);
    assertEquals(err.api.message, "not your tagma");
  },
);

Deno.test(
  "the agent id is URL-encoded in the relayed manage path",
  async () => {
    // The backend path builders embed the agent id directly; pin that
    // what reaches the rest client is the encoded form, not the raw
    // id (a stray "/" or "?" would warp the frame path).
    const captured: Array<{ path: string }> = [];
    const rest = {
      manage: (_agent: string, _method: string, path: string) => {
        captured.push({ path });
        return Promise.resolve({ status: 200, body: {} });
      },
    } as unknown as ManageRestClient;
    const backend = new OnlineBackend(rest, "t-a");
    await backend.getAgentStatus("a/b c");
    assertEquals(captured.length, 1);
    assertEquals(captured[0]!.path, "/agents/a%2Fb%20c/status");
  },
);

// --- P2-c: the projection seam -------------------------------------------

// End-to-end across the seam: listAgents rides the projection GET, and a
// dirty frame pumped through the stubbed SSE stream reaches the feed
// subscriber -- the exact chain a real lesche drives (store -> SSE -> GET).
Deno.test(
  "projection-backed listAgents and a dirty frame reach the feed",
  async () => {
    const sse = 'data: {"tagma_id":"t-a","seq":9}\n\n';
    const seen: string[] = [];
    const real = globalThis.fetch;
    globalThis.fetch = ((url: string | URL | Request) => {
      const u = String(url);
      seen.push(u);
      if (u.endsWith("/projection/agents")) {
        return Promise.resolve(
          Response.json({
            stale: false,
            seq: 9,
            updated_at: 1,
            agents: [
              {
                id: "root",
                workspace_root: "/w",
                state: "idle",
                created_by: null,
                role: "",
                description: "",
                activity: "",
                duty: "onduty",
                faulted_reason: null,
                conversation_id: null,
              },
            ],
            status: {},
          }),
        );
      }
      return Promise.resolve(
        new Response(sse, {
          status: 200,
          headers: { "content-type": "text/event-stream" },
        }),
      );
    }) as typeof fetch;
    try {
      const backend = new OnlineBackend(
        {} as unknown as ManageRestClient, // the manage relay stays untouched
        "t-a",
        new ProjectionClient("http://lesche.test"),
      );
      const agents = await backend.listAgents();
      assertEquals(agents.agents[0]!.id, "root");
      assertEquals(
        seen.some((u) => u.endsWith("/projection/agents")),
        true,
      );

      // The feed: one dirty nudge reaches the subscriber, no polling.
      let nudges = 0;
      const stop = backend.projectionFeed!.subscribe(() => {
        nudges += 1;
      });
      await new Promise((r) => setTimeout(r, 60));
      stop();
      assertEquals(nudges >= 1, true);
    } finally {
      globalThis.fetch = real;
    }
  },
);
