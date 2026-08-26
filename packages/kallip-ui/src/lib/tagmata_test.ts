import { assertEquals } from "@std/assert";
import { joinDeviceRows } from "./tagmata.svelte.ts";
import type { TagmaProcessLike } from "./tagmata.svelte.ts";

// The panel join is a pure projection; these tests pin the key preference
// (process-reported tagma_id first, one-click slug convention as fallback)
// and the unmatched-halves shapes the card list renders.
const presence = () => "checking" as const;
const status = () => undefined;
const ports = {};
const slugFor = (id: string) => `tagma-${id.slice(0, 8)}`;

function proc(
  p: Partial<TagmaProcessLike> & { slug: string },
): TagmaProcessLike {
  return { workspace: "/w", running: true, ...p };
}

Deno.test("joinDeviceRows merges on the process-reported tagma_id", () => {
  const rows = joinDeviceRows(
    [
      {
        tagmaId: "tid-full-uuid",
        label: null,
        createdAt: "2026-08-26T00:00:00Z",
      },
    ],
    [proc({ slug: "team", tagma_id: "tid-full-uuid" })],
    ports,
    slugFor,
    presence,
    status,
  );
  assertEquals(rows.length, 1);
  assertEquals(rows[0].tagma?.tagmaId, "tid-full-uuid");
  assertEquals(rows[0].process?.slug, "team");
});

Deno.test("joinDeviceRows falls back to the slug convention", () => {
  // A one-click-era process (pre-field daemon) whose slug encodes the id.
  const rows = joinDeviceRows(
    [
      {
        tagmaId: "abcdefgh-1234",
        label: null,
        createdAt: "2026-08-26T00:00:00Z",
      },
    ],
    [proc({ slug: "tagma-abcdefgh" })],
    ports,
    slugFor,
    presence,
    status,
  );
  assertEquals(rows.length, 1);
  assertEquals(rows[0].process?.slug, "tagma-abcdefgh");
});

Deno.test("joinDeviceRows keeps unmatched halves as their own rows", () => {
  const rows = joinDeviceRows(
    [
      {
        tagmaId: "enrolled-only",
        label: null,
        createdAt: "2026-08-26T00:00:00Z",
      },
    ],
    [proc({ slug: "local-only" })],
    ports,
    slugFor,
    presence,
    status,
  );
  assertEquals(rows.length, 2);
  assertEquals(rows[0].tagma?.tagmaId, "enrolled-only");
  assertEquals(rows[0].process, undefined);
  assertEquals(rows[1].key, "local-only");
  assertEquals(rows[1].process?.slug, "local-only");
});

Deno.test(
  "joinDeviceRows claims a process once even on colliding identities",
  () => {
    const rows = joinDeviceRows(
      [
        { tagmaId: "tid-a", label: null, createdAt: "2026-08-26T00:00:00Z" },
        { tagmaId: "tid-b", label: null, createdAt: "2026-08-26T00:00:00Z" },
      ],
      [proc({ slug: "team", tagma_id: "tid-a" })],
      ports,
      slugFor,
      presence,
      status,
    );
    assertEquals(rows.length, 2);
    assertEquals(rows[0].process?.slug, "team");
    assertEquals(rows[1].tagma?.tagmaId, "tid-b");
    assertEquals(rows[1].process, undefined);
  },
);
