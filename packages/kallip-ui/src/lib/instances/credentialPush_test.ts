// Pure-logic tests for the one-click credential push: wire assembly, the
// eligibility gate, loop termination states, and probe verdict mapping.
// Time and transport are injected, so no real channel or clock is touched.

import { assertEquals } from "@std/assert";
import {
  buildPushConfig,
  ENROLL_PUSH_INTERVAL_MS,
  ENROLL_PUSH_WINDOW_MS,
  isLocked,
  probeRequestFor,
  probeVerdict,
  providerEndpointKey,
  pushCredentials,
  pushErrorKind,
  type PushPorts,
} from "./credentialPush.ts";
import { KallipError, TransportError } from "@kallipai/kallip-common";
import type {
  ProfileConfig,
  ProfileProbeResponse,
} from "@kallipai/kallip-client";

const live: ProfileConfig = {
  tiers: [
    {
      profiles: [
        {
          id: "p1",
          endpoint: "main",
          model: "deepseek-chat",
          max_context_window: 128_000,
        },
      ],
    },
  ],
  endpoints: {
    main: {
      id: "main",
      family: "deepseek",
      api_key: "sk-existing",
      base_url: null,
    },
  },
  parking: [],
};

const target = {
  endpointKey: "provider:tagma-ab12cd34",
  family: "deepseek",
  apiKey: "sk-vault-secret",
  baseUrl: null,
};

function instantPorts(
  overrides: Partial<PushPorts> = {},
): PushPorts & { puts: number; closed: boolean } {
  // Fake clock: sleeps advance it, so the deadline fold is deterministic.
  let clock = 0;
  const ports = {
    puts: 0,
    closed: false,
    fetchLive: () => Promise.resolve(live),
    put: (_body: ProfileConfig) => {
      ports.puts++;
      return Promise.resolve({});
    },
    probe: () =>
      Promise.resolve({
        results: [],
        tiers: [],
      }) as Promise<ProfileProbeResponse>,
    now: () => clock,
    sleep: (ms: number) => {
      clock += ms;
      return Promise.resolve();
    },
    close: () => {
      ports.closed = true;
    },
    ...overrides,
  };
  return ports as unknown as PushPorts & { puts: number; closed: boolean };
}

Deno.test("endpoint key stays within the TOML-safe charset", () => {
  const key = providerEndpointKey("tagma-ab12cd34");
  assertEquals(key, "provider:tagma-ab12cd34");
  assertEquals(/^[a-z0-9:_-]+$/.test(key), true);
});

Deno.test("eligibility: encrypted rows lock outside a passkey session", () => {
  assertEquals(isLocked({ mode: "encrypted" }, true), false);
  assertEquals(isLocked({ mode: "encrypted" }, false), true);
  // Plaintext never locks, whatever the session came through.
  assertEquals(isLocked({ mode: "plaintext" }, false), false);
});

Deno.test("wire assembly round-trips live rows and adds one endpoint", () => {
  const body = buildPushConfig(live, target);
  // Tiers pass through verbatim (positional rotation untouched).
  assertEquals(body.tiers, live.tiers);
  // The existing endpoint keeps by tri-state, ours carries the real key.
  assertEquals(body.endpoints["main"].api_key, null);
  assertEquals(body.endpoints["main"].base_url, null);
  assertEquals(body.endpoints[target.endpointKey].api_key, target.apiKey);
  assertEquals(body.endpoints[target.endpointKey].family, target.family);
  // Parking list replaces (the UI always sends it).
  assertEquals(body.parking, []);
});

Deno.test(
  "wire assembly over an empty config (fresh instance) adds only us",
  () => {
    const body = buildPushConfig(
      { tiers: [], endpoints: {}, parking: [] },
      target,
    );
    assertEquals(Object.keys(body.endpoints), [target.endpointKey]);
  },
);

Deno.test("re-pushing the same instance overwrites its entry in place", () => {
  const once = buildPushConfig(live, target);
  const twice = buildPushConfig(once, { ...target, apiKey: "sk-rotated" });
  assertEquals(twice.endpoints[target.endpointKey].api_key, "sk-rotated");
  assertEquals(Object.keys(twice.endpoints).length, 2);
});

Deno.test(
  "probe request references our endpoint inline with keep-nulls",
  () => {
    const req = probeRequestFor(target);
    assertEquals(req.endpoints.length, 1);
    assertEquals(req.endpoints[0], {
      id: target.endpointKey,
      family: target.family,
      api_key: null,
      base_url: target.baseUrl,
    });
    assertEquals(req.tiers.length, 0);
  },
);

Deno.test("probe verdict maps only our endpoint's ok to true", () => {
  const ok = {
    results: [{ endpoint_id: target.endpointKey, status: "ok" }],
    tiers: [],
  } as unknown as ProfileProbeResponse;
  const bad = {
    results: [{ endpoint_id: target.endpointKey, status: "unauthorized" }],
    tiers: [],
  } as unknown as ProfileProbeResponse;
  const other = {
    results: [{ endpoint_id: "elsewhere", status: "ok" }],
    tiers: [],
  } as unknown as ProfileProbeResponse;
  assertEquals(probeVerdict(ok, target.endpointKey), true);
  assertEquals(probeVerdict(bad, target.endpointKey), false);
  assertEquals(probeVerdict(other, target.endpointKey), false);
});

Deno.test("error classification: transient vs terminal", () => {
  assertEquals(pushErrorKind(new TransportError("ws died")), "retry");
  assertEquals(
    pushErrorKind(new KallipError({ status: 500, message: "boom" })),
    "retry",
  );
  assertEquals(
    pushErrorKind(new KallipError({ status: 429, message: "slow down" })),
    "retry",
  );
  assertEquals(pushErrorKind(new Error("crypto failed")), "retry");
  // A structured rejection (bad key shape) cannot succeed on retry.
  assertEquals(
    pushErrorKind(new KallipError({ status: 400, message: "bad api_key" })),
    "terminal",
  );
  assertEquals(
    pushErrorKind(new KallipError({ status: 401, message: "denied" })),
    "terminal",
  );
});

Deno.test(
  "push loop: first-attempt success reports pushed + probe verdict",
  async () => {
    const ports = instantPorts({
      probe: () =>
        Promise.resolve({
          results: [{ endpoint_id: target.endpointKey, status: "ok" }],
          tiers: [],
        }) as unknown as Promise<ProfileProbeResponse>,
    });
    const outcome = await pushCredentials(target, ports);
    assertEquals(outcome.state, "pushed");
    if (outcome.state === "pushed") assertEquals(outcome.probeOk, true);
    assertEquals(ports.puts, 1);
    assertEquals(ports.closed, true);
  },
);

Deno.test(
  "push loop: terminal 400 stops immediately with the message",
  async () => {
    let attempts = 0;
    const ports = instantPorts({
      put: () => {
        attempts++;
        return Promise.reject(
          new KallipError({
            status: 400,
            message: "api_key must not be empty",
          }),
        );
      },
    });
    const outcome = await pushCredentials(target, ports);
    assertEquals(outcome, {
      state: "failed",
      endpointKey: target.endpointKey,
      message: "api_key must not be empty",
    });
    assertEquals(attempts, 1);
    assertEquals(ports.closed, true);
  },
);

Deno.test(
  "push loop: transient failures retry until the window folds",
  async () => {
    // Fold check fires after the attempt whose next sleep would cross the
    // deadline, so total attempts = ceil(WINDOW / INTERVAL).
    const expectedAttempts = Math.ceil(
      ENROLL_PUSH_WINDOW_MS / ENROLL_PUSH_INTERVAL_MS,
    );
    let attempts = 0;
    const ports = instantPorts({
      put: () => {
        attempts++;
        return Promise.reject(new TransportError("not enrolled yet"));
      },
    });
    const outcome = await pushCredentials(target, ports);
    assertEquals(attempts, expectedAttempts);
    assertEquals(outcome.state, "unreachable");
    assertEquals(ports.closed, true);
  },
);

Deno.test("push loop: recovery inside the window still pushes", async () => {
  let attempts = 0;
  const ports = instantPorts({
    put: () => {
      attempts++;
      if (attempts < 3) return Promise.reject(new TransportError("soon"));
      return Promise.resolve({});
    },
  });
  const outcome = await pushCredentials(target, ports);
  assertEquals(outcome.state, "pushed");
  assertEquals(attempts, 3);
});

Deno.test(
  "push loop: a probe failure after a good PUT degrades probeOk only",
  async () => {
    const ports = instantPorts({
      probe: () => Promise.reject(new TransportError("probe dropped")),
    });
    const outcome = await pushCredentials(target, ports);
    assertEquals(outcome, {
      state: "pushed",
      endpointKey: target.endpointKey,
      probeOk: false,
    });
  },
);
