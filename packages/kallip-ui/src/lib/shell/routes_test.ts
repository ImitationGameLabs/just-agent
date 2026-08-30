import { assert, assertEquals } from "@std/assert";
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

const DETAILS_PAGES = [
  "overview",
  "budget",
  "agents",
  "profiles",
  "schedules",
].map((section) => ({
  section,
  pages: [
    new URL(
      `../../../../kallip-web/src/routes/tagma/[id]/details/${section}/+page.svelte`,
      import.meta.url,
    ),
    new URL(
      `../../../../kallip-app/src/routes/tagma/[id]/details/${section}/+page.svelte`,
      import.meta.url,
    ),
  ],
}));

Deno.test(
  "the details section pages render their own section",
  // Same rot-risk one tree over: a thin section page could copy-paste a
  // neighbor's page constant and still typecheck. Read all ten shells and
  // assert the section each one renders. Scoped read grant, same rationale.
  {
    permissions: {
      read: DETAILS_PAGES.flatMap(({ pages }) => [...pages]),
    },
  },
  () => {
    for (const { section, pages } of DETAILS_PAGES) {
      for (const page of pages) {
        const src = new TextDecoder().decode(Deno.readFileSync(page));
        assert(
          src.includes(`page="${section}"`),
          "section page must render its own section",
        );
        assert(
          src.includes("page.params.id"),
          "section page must forward the route param, not a literal",
        );
      }
    }
  },
);

const DETAILS_HUBS = [
  new URL(
    "../../../../kallip-web/src/routes/tagma/[id]/details/+page.ts",
    import.meta.url,
  ),
  new URL(
    "../../../../kallip-app/src/routes/tagma/[id]/details/+page.ts",
    import.meta.url,
  ),
];

Deno.test(
  "the details hub stays wired to the path builder",
  // The hub 301s to overview through the builder; pin the wiring so a
  // downgrade to 302 or a re-derived literal cannot slip in. Scoped read
  // grant, same rationale.
  { permissions: { read: DETAILS_HUBS } },
  () => {
    for (const hub of DETAILS_HUBS) {
      const src = new TextDecoder().decode(Deno.readFileSync(hub));
      assert(src.includes("redirect(301"), "hub must issue a permanent 301");
      assert(
        src.includes("tagmaDetailsSectionPath(params.id"),
        "hub must forward via the path builder, not a re-derived literal",
      );
      assert(
        src.includes('"overview"'),
        "hub must land on the overview section",
      );
    }
  },
);
