// The online shell's confirm flow keys off the structured dangling list a
// 409 profiles save carries. These tests pin that the relay path
// reconstructs the full KallipError — not a flattened message-only one —
// and that the save-failure classification still routes a bare 409 (old
// backend) to the stale-backend branch.

import { assertEquals } from "@std/assert";
import { KallipError } from "@kallipai/kallip-common";
import type { RelayChannel } from "@kallipai/kallip-lesche-client";
import { OnlineBackend } from "./backend.ts";
import { classifySaveFailure } from "./profiles-view.ts";

function channelWith(body: unknown): RelayChannel {
  return {
    manage: () => Promise.resolve({ status: 409, body }),
  } as unknown as RelayChannel;
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
    const backend = new OnlineBackend(channelWith(DANGLING_BODY));
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
      channelWith({ error: { message: "config drops sets still bound" } }),
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
