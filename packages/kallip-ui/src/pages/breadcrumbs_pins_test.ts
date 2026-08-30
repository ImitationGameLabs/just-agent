import { assert } from "@std/assert";

// Source-read pins for the C1 breadcrumb mounts (unified breadcrumbs plan).
// A typecheck cannot see that a page still mounts its trail, which segment is
// current, or that a href still comes from the path-builder; these pins guard
// exactly that, same rationale as chrome_pins_test and the retired shim
// source-read tests. Scoped read grants accordingly.

const TAGMATA_PAGE = new URL("./TagmataPage.svelte", import.meta.url);
const TAGMA_PROFILE_PAGE = new URL(
  "./TagmaProfilePage.svelte",
  import.meta.url,
);
const TAGMA_CHAT_PAGE = new URL("./TagmaChatPage.svelte", import.meta.url);
const ONLINE_MANAGE_PAGE = new URL(
  "./manage/OnlineManagePage.svelte",
  import.meta.url,
);

function source(url: URL): string {
  return new TextDecoder().decode(Deno.readFileSync(url));
}

Deno.test(
  "TagmataPage mounts a single current-segment trail (root page, R1a exception)",
  { permissions: { read: [TAGMATA_PAGE] } },
  () => {
    const src = source(TAGMATA_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("label: nav_tagmata(), current: true }"),
      "the tagmata segment itself must be the current tail (no href)",
    );
  },
);

Deno.test(
  "TagmaProfilePage mounts the Tagmata-root trail with a current name tail",
  { permissions: { read: [TAGMA_PROFILE_PAGE] } },
  () => {
    const src = source(TAGMA_PROFILE_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes('href: "/tagmata"'),
      "the root segment must link the tagmata top level",
    );
    assert(
      src.includes("label: displayName, current: true"),
      "the profile name must be the current tail",
    );
  },
);

Deno.test(
  "TagmaChatPage keeps its parent segment on the path-builder and a current chat tail",
  { permissions: { read: [TAGMA_CHAT_PAGE] } },
  () => {
    const src = source(TAGMA_CHAT_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("tagmaDetailsPath(tagmaId)"),
      "the tagma segment href must come from the path-builder",
    );
    assert(
      src.includes("label: nav_chat(), current: true"),
      "the chat segment must be the current tail",
    );
  },
);

Deno.test(
  "OnlineManagePage keeps its parent segment on the path-builder and a current section tail",
  { permissions: { read: [ONLINE_MANAGE_PAGE] } },
  () => {
    const src = source(ONLINE_MANAGE_PAGE);
    assert(src.includes("<Breadcrumbs"), "trail must be mounted");
    assert(
      src.includes("tagmaChatPath(tagmaId)"),
      "the tagma segment href must come from the path-builder",
    );
    assert(
      src.includes("sectionLabels[page](), current: true"),
      "the section label must be the current tail for every details section",
    );
  },
);
