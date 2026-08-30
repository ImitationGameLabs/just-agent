import {
  tagmaChatPath,
  tagmaDetailsPath,
  tagmaDetailsSectionPath,
} from "./routes.ts";

Deno.test("tagmaChatPath builds the tagma-centric chat route", () => {
  assertEquals(tagmaChatPath("abc"), "/tagma/abc/chat");
});

Deno.test("tagmaDetailsPath builds the manage details hub", () => {
  assertEquals(tagmaDetailsPath("abc"), "/tagma/abc/details");
});

Deno.test("tagmaDetailsSectionPath builds the details sections", () => {
  assertEquals(
    tagmaDetailsSectionPath("abc", "overview"),
    "/tagma/abc/details/overview",
  );
  assertEquals(
    tagmaDetailsSectionPath("abc", "schedules"),
    "/tagma/abc/details/schedules",
  );
});

