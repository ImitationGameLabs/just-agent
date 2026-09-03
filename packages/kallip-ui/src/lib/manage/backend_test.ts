// The online shell's confirm flow keys off the structured dangling list a
// 409 profiles save carries. These tests pin that the relay path
// reconstructs the full KallipError — not a flattened message-only one —
// and that the save-failure classification still routes a bare 409 (old
// backend) to the stale-backend branch.

import { assertEquals } from "@std/assert";
import { KallipError } from "@kallipai/kallip-common";
import type { ManageRestClient } from "@kallipai/kallip-lesche-client";
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
