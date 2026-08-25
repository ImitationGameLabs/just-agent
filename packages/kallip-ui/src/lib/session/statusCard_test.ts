// Tests for the status-card store's event wiring: attach() pulls the
// roster through the conversation backend once, nudge() re-pulls without
// waiting for the reconciliation poll (this is the relay `tagma_status` /
// direct-SSE event path the chat page feeds), overlapping nudges collapse
// into one request, and detach() clears rows and stops further pulls.
// Rune-bearing module under `deno test`: passthrough $state shim (the
// channels_openBudget_test pattern). The seam is OfflineBackend over a stub
// TagmaClient, so the real adapter runs and only the HTTP client is faked.
// The store's own reconciliation intervals (15s/30s) never fire inside a
// test; no `document` is installed, so startVisibleInterval degrades to a
// bare timer that detach() always clears.

declare global {
  function $state<T>(initial: T): T;
  function $state<T>(): T | undefined;
}

(globalThis as Record<string, unknown>)["$state"] = (v: unknown) => v;

import type {
  ListAgentsManagementResponse,
  TagmaClient,
  WireAgentManagementSummary,
} from "@kallipai/kallip-client";

const { assertEquals } = await import("@std/assert");
const { OfflineBackend } = await import("../manage/backend.ts");
const { statusCardStore } = await import("./statusCard.svelte.ts");

const root: WireAgentManagementSummary = {
  id: "root-1",
  workspace_root: "/w",
  state: "busy",
  created_by: null,
  role: "root",
  description: "",
  activity: "thinking",
  duty: "onduty",
  faulted_reason: null,
  conversation_id: null,
};

const sub: WireAgentManagementSummary = {
  ...root,
  id: "sub-1",
  created_by: "root-1",
  state: "idle",
  role: "worker",
  activity: "",
};

/** Only the calls the store makes from attach()/nudge(): the roster pull,
 * and the registry pull whose failure is a documented non-fatal path
 * (denominators stay null). getAgentStatus belongs to the slow context
 * poll and never runs inside a test. */
class StubClient {
  rosterCalls = 0;
  listAgents(): Promise<ListAgentsManagementResponse> {
    this.rosterCalls++;
    return Promise.resolve({ agents: [root, sub] });
  }
  getProfiles(): Promise<never> {
    return Promise.reject(new Error("registry unavailable in test"));
  }
}

const flush = () => new Promise((r) => setTimeout(r, 20));
const backend = (stub: StubClient) =>
  new OfflineBackend(stub as unknown as TagmaClient);

Deno.test("attach pulls the roster once through the backend", async () => {
  const stub = new StubClient();
  try {
    statusCardStore.attach(backend(stub));
    await flush();
    assertEquals(stub.rosterCalls, 1);
    assertEquals(statusCardStore.rootRow?.id, "root-1");
    assertEquals(statusCardStore.subRows.length, 1);
    assertEquals(statusCardStore.subRows[0]?.id, "sub-1");
  } finally {
    statusCardStore.detach();
  }
});

Deno.test(
  "nudge re-pulls the roster without waiting for the poll",
  async () => {
    const stub = new StubClient();
    try {
      statusCardStore.attach(backend(stub));
      await flush();
      assertEquals(stub.rosterCalls, 1);
      statusCardStore.nudge();
      await flush();
      assertEquals(stub.rosterCalls, 2);
    } finally {
      statusCardStore.detach();
    }
  },
);

Deno.test("overlapping nudges collapse into one request", async () => {
  const stub = new StubClient();
  try {
    statusCardStore.attach(backend(stub));
    await flush();
    assertEquals(stub.rosterCalls, 1);
    statusCardStore.nudge();
    statusCardStore.nudge(); // second lands while the first pull is in flight
    await flush();
    assertEquals(stub.rosterCalls, 2);
  } finally {
    statusCardStore.detach();
  }
});

Deno.test("detach clears rows and makes nudges no-ops", async () => {
  const stub = new StubClient();
  statusCardStore.attach(backend(stub));
  await flush();
  statusCardStore.detach();
  assertEquals(statusCardStore.rootRow, null);
  assertEquals(statusCardStore.subRows.length, 0);
  statusCardStore.nudge();
  await flush();
  assertEquals(stub.rosterCalls, 1);
});
