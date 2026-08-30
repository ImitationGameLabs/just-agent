import { assertEquals } from "@std/assert";
import { tagmaChatPath, tagmaDetailsPath } from "./routes.ts";

Deno.test("tagmaChatPath builds the tagma-centric chat route", () => {
  assertEquals(tagmaChatPath("abc"), "/tagma/abc/chat");
});

Deno.test("tagmaDetailsPath builds the manage details hub", () => {
  assertEquals(tagmaDetailsPath("abc"), "/tagma/abc/details");
});
