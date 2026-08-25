// Locks the tagma->instance slug convention: the one-click spawn names the
// instance and the devices-list join looks it up, so a silent change to the
// prefix shape would break the join with no signal. These cases pin the
// mapping (and its deliberate first-8-chars collision semantics).
import { instanceSlugFor } from "./client.ts";
import { assertEquals } from "@std/assert";

Deno.test("instanceSlugFor derives the one-click spawn slug", () => {
  assertEquals(instanceSlugFor("abcdefgh-1234-xyz"), "tagma-abcdefgh");
});

Deno.test(
  "ids differing only past the first 8 chars map to the same slug",
  () => {
    // Deliberate: the convention trades a rare collision for a short, readable
    // slug -- the join treats such identities as one device.
    assertEquals(
      instanceSlugFor("abcdefgh-AAAA"),
      instanceSlugFor("abcdefgh-BBBB"),
    );
  },
);
