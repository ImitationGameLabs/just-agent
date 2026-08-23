// Tests for DirectTransport.send's parked-409 auto-wake: the direct path's
// answer to "agent is parked; use POST /agents/{id}/wake" — wake, back off,
// re-send; never silently drop, never double-send (the 409 fires before the
// tagma's inbox push, so an unsent message never landed).

import { assertEquals, assertRejects } from "@std/assert";
import { KallipError } from "@kallipai/kallip-common";
import type { TagmaClient } from "@kallipai/kallip-client";
import { DirectTransport } from "./directTransport.ts";
import { LOCAL_OPERATOR_SENDER } from "../transcript.ts";

const PARKED_409 = () =>
  new KallipError({
    status: 409,
    message: "agent is parked; use POST /agents/{id}/wake to kick it awake",
  });

/** A TagmaClient whose postMessage walks a scripted outcome list ("ok"
 * resolves, an Error instance rejects with it), counting calls; wakeAgent
 * optionally rejects. The SSE stream stays empty — these tests exercise
 * send only. */
function sendClient(
  outcomes: ("ok" | Error)[],
  wakeFails = false,
): { client: TagmaClient; state: { wakeCalls: number; posts: number } } {
  const state = { wakeCalls: 0, posts: 0 };
  const client = {
    wakeAgent() {
      state.wakeCalls++;
      return wakeFails
        ? Promise.reject(new Error("wake refused"))
        : Promise.resolve();
    },
    async postMessage(_id: string, _text: string): Promise<void> {
      state.posts++;
      const next = outcomes.shift();
      if (next instanceof Error) throw next;
    },
    async *externalEventStream() {},
  } as unknown as TagmaClient;
  return { client, state };
}

Deno.test("send auto-wakes on a parked 409 and re-sends", async () => {
  const { client, state } = sendClient([PARKED_409(), "ok"]);
  const t = new DirectTransport(client, "root", LOCAL_OPERATOR_SENDER, [1]);
  await t.send("hello");
  assertEquals(state.posts, 2); // first attempt rejected, retry delivered
  assertEquals(state.wakeCalls, 1);
});

Deno.test("a failed wake call does not abort the retry loop", async () => {
  const { client, state } = sendClient([PARKED_409(), "ok"], true);
  const t = new DirectTransport(client, "root", LOCAL_OPERATOR_SENDER, [1]);
  await t.send("hello");
  assertEquals(state.posts, 2);
});

Deno.test("send exhausts retries and rethrows the original 409", async () => {
  const { client, state } = sendClient([
    PARKED_409(),
    PARKED_409(),
    PARKED_409(),
  ]);
  const t = new DirectTransport(client, "root", LOCAL_OPERATOR_SENDER, [1, 1]);
  const err = await assertRejects(() => t.send("hello"), KallipError);
  assertEquals(err.api.status, 409);
  assertEquals(state.posts, 3); // every attempt hit the parked guard
  assertEquals(state.wakeCalls, 1); // one wake attempt, not one per retry
});

Deno.test("send propagates non-parked errors without waking", async () => {
  const { client, state } = sendClient([new Error("network down")]);
  const t = new DirectTransport(client, "root", LOCAL_OPERATOR_SENDER, [1]);
  await assertRejects(() => t.send("hello"), Error, "network down");
  assertEquals(state.posts, 1);
  assertEquals(state.wakeCalls, 0);
});

Deno.test("a mid-retry non-parked error beats the parked 409", async () => {
  const gone = new KallipError({ status: 404, message: "agent not found" });
  const { client } = sendClient([PARKED_409(), gone]);
  const t = new DirectTransport(client, "root", LOCAL_OPERATOR_SENDER, [1]);
  const err = await assertRejects(() => t.send("hello"), KallipError);
  assertEquals(err.api.status, 404);
});

// --- the SSE retry loop (runMux) ---

/** One raw frame as the demux sees it. */
type RawFrame = { readonly event: string; readonly data: string };

/** A scripted stream attempt: an Error rejects the connect, a factory
 * yields the frames of one connection (mirroring the real client, which
 * reports liveness when the connection opens). */
type AttemptFactory = (
  signal: AbortSignal | undefined,
  onFrame: () => void,
) => AsyncIterable<RawFrame>;

function streamClient(script: (Error | AttemptFactory)[]): {
  client: TagmaClient;
  state: { connects: number };
} {
  const state = { connects: 0 };
  const client = {
    async *externalEventStream(
      _id: string,
      signal?: AbortSignal,
      onFrame?: () => void,
    ) {
      state.connects++;
      const next = script.shift();
      if (!next) throw new Error("script exhausted");
      if (next instanceof Error) throw next;
      yield* next(signal, onFrame!);
    },
  } as unknown as TagmaClient;
  return { client, state };
}

function statusFrame(n: number): RawFrame {
  return {
    event: "status",
    data: JSON.stringify({
      root_state: "idle",
      subagents_total: n,
      subagents_active: 0,
      token_budget: 0,
      token_consumed: 0,
    }),
  };
}

/** A healthy connection: yields the given frames, then holds open (like a
 * live SSE) until aborted. Ending the generator would end the stream
 * cleanly, which now triggers a reconnect. */
const liveStream = (...fs: RawFrame[]): AttemptFactory =>
  async function* (signal, onFrame) {
    onFrame(); // connection open
    for (const f of fs) yield f;
    await new Promise<never>((_, reject) => {
      const onAbort = () => reject(new Error("aborted"));
      if (signal?.aborted) onAbort();
      else signal?.addEventListener("abort", onAbort, { once: true });
    });
  };

/** A healthy keepalive-fed connection: yields the frame every few ms (so
 * the watchdog stays fed) until aborted. */
const ticking = (f: RawFrame): AttemptFactory =>
  async function* (sig, onFrame) {
    onFrame(); // connection open
    for (;;) {
      onFrame(); // every raw frame feeds the watchdog
      yield f;
      const aborted = await new Promise<boolean>((resolve) => {
        const t = setTimeout(() => resolve(false), 5);
        const onAbort = () => {
          clearTimeout(t);
          resolve(true);
        };
        if (sig?.aborted) onAbort();
        else sig?.addEventListener("abort", onAbort, { once: true });
      });
      if (aborted) return;
    }
  };

/** A connection that opens and then goes silent forever (a half-open
 * socket): never yields — next() parks until the abort rejects. Hand-rolled
 * as an async iterable because a yield-less async generator trips
 * require-yield by design. */
const silent: AttemptFactory = (signal, onFrame) => {
  onFrame(); // connection open
  return {
    [Symbol.asyncIterator]: () => ({
      next: (): Promise<IteratorResult<RawFrame>> =>
        new Promise<never>((_, reject) => {
          const onAbort = () => reject(new Error("aborted"));
          if (signal?.aborted) onAbort();
          else signal?.addEventListener("abort", onAbort, { once: true });
        }),
    }),
  };
};

const tick = (ms: number) => new Promise((r) => setTimeout(r, ms));

Deno.test("stream failure retries silently and resumes", async () => {
  const { client, state } = streamClient([
    new Error("net down"),
    liveStream(statusFrame(1), statusFrame(2)),
  ]);
  const states: string[] = [];
  const t = new DirectTransport(
    client,
    "root",
    LOCAL_OPERATOR_SENDER,
    [1],
    [1],
  );
  t.onState = (s) => states.push(s);
  const seen: number[] = [];
  const drain = (async () => {
    for await (const s of t.status()) seen.push(s.subagentsTotal);
  })();
  await tick(80);
  t.close();
  await drain;
  assertEquals(state.connects, 2);
  assertEquals(states, ["reconnecting", "resumed"]);
  assertEquals(seen, [1, 2]);
});

Deno.test("stream retries exhaust and fail the drains", async () => {
  const { client, state } = streamClient([
    new Error("a"),
    new Error("b"),
    new Error("c"),
  ]);
  const states: string[] = [];
  const t = new DirectTransport(
    client,
    "root",
    LOCAL_OPERATOR_SENDER,
    [1],
    [1, 1],
  );
  t.onState = (s) => states.push(s);
  let err: unknown = null;
  try {
    for await (const _ of t.status()) break;
  } catch (e) {
    err = e;
  }
  assertEquals((err as Error).message, "c");
  assertEquals(state.connects, 3);
  assertEquals(states, ["reconnecting", "reconnecting"]);
});

Deno.test(
  "watchdog tears down a silent half-open stream and reconnects",
  async () => {
    const { client, state } = streamClient([silent, ticking(statusFrame(7))]);
    const states: string[] = [];
    const t = new DirectTransport(
      client,
      "root",
      LOCAL_OPERATOR_SENDER,
      [1],
      [10_000],
      30,
    );
    t.onState = (s) => states.push(s);
    const seen: number[] = [];
    const drain = (async () => {
      for await (const s of t.status()) seen.push(s.subagentsTotal);
    })();
    await tick(120);
    t.close();
    await drain;
    assertEquals(state.connects, 2);
    assertEquals(states, ["resumed", "reconnecting", "resumed"]);
    assertEquals(seen.length >= 1 && seen.every((v) => v === 7), true);
  },
);

Deno.test(
  "close during reconnect backoff ends the drains cleanly",
  async () => {
    const { client, state } = streamClient([new Error("down")]);
    const states: string[] = [];
    const t = new DirectTransport(
      client,
      "root",
      LOCAL_OPERATOR_SENDER,
      [1],
      [60_000],
    );
    t.onState = (s) => states.push(s);
    let ended = false;
    const drain = (async () => {
      for await (const _ of t.status()) {
        /* wait */
      }
      ended = true;
    })();
    await tick(30); // first failure -> reconnecting -> sleeping 60s
    t.close(); // must kick the sleep
    await drain;
    assertEquals(ended, true);
    assertEquals(states, ["reconnecting"]);
    assertEquals(state.connects, 1);
  },
);

Deno.test("foreground return swaps the stream silently", async () => {
  const { client, state } = streamClient([silent, liveStream(statusFrame(3))]);
  const states: string[] = [];
  const t = new DirectTransport(
    client,
    "root",
    LOCAL_OPERATOR_SENDER,
    [1],
    [10_000],
  );
  t.onState = (s) => states.push(s);
  const drain = (async () => {
    for await (const _ of t.status()) {
      /* wait */
    }
  })();
  await tick(20); // connected and silent
  t.handleForegroundVisible(); // no DOM in Deno: treated as visible
  await tick(40); // swap -> frames flow
  t.close();
  await drain;
  assertEquals(state.connects, 2);
  assertEquals(states, ["resumed", "resumed"]); // no reconnecting flash
});
