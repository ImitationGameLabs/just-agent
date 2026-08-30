import { assert } from "@std/assert";

// Source-read pins for the shared chrome components (extracted in B2.5,
// consumed by pages from B3 on). These guard branch contracts a typecheck
// cannot see: which branch wins when several could match, that migrated
// conditions survive verbatim, and the order snippets render in. Same
// rationale as the shell routes_test pins; scoped read grants accordingly.

const BREADCRUMBS = new URL("./Breadcrumbs.svelte", import.meta.url);
const PAGE_HEADER = new URL("./PageHeader.svelte", import.meta.url);
const ROOM_PAGE = new URL(
  "../pages/RoomConversationPage.svelte",
  import.meta.url,
);

function source(url: URL): string {
  return new TextDecoder().decode(Deno.readFileSync(url));
}

Deno.test(
  "Breadcrumbs resolves the current segment before the href branch",
  { permissions: { read: [BREADCRUMBS] } },
  () => {
    const src = source(BREADCRUMBS);
    const current = src.indexOf("{#if seg.current}");
    const href = src.indexOf("{:else if seg.href}");
    assert(current !== -1, "the aria-current branch must exist");
    assert(href !== -1, "the href branch must exist");
    assert(
      current < href,
      "a current segment must win over its href (contract note in the type)",
    );
  },
);

Deno.test(
  "RoomConversationPage keeps its three badge conditions",
  { permissions: { read: [ROOM_PAGE] } },
  () => {
    const src = source(ROOM_PAGE);
    // The B2.5 header migration must not have dropped or reordered the
    // conditional badges the inline bar grew over time.
    const conditions = [
      '{#if room?.visibility === "public"}',
      "{#if conv?.roster}",
      "{#if conv.roster.is_creator}",
    ];
    let at = -1;
    for (const condition of conditions) {
      const next = src.indexOf(condition, at + 1);
      assert(next > at, `badge condition must survive in order: ${condition}`);
      at = next;
    }
  },
);

Deno.test(
  "PageHeader renders breadcrumbs, title, badges, actions in order",
  { permissions: { read: [PAGE_HEADER] } },
  () => {
    const src = source(PAGE_HEADER);
    // The bar is a single flex row; reordering the optional regions is a
    // visible layout change every consumer inherits silently.
    const markers = [
      "{#if breadcrumbs}",
      "{@render title()}",
      "{#if badges}",
      "{#if actions}",
    ];
    let at = -1;
    for (const marker of markers) {
      const next = src.indexOf(marker, at + 1);
      assert(next > at, `region must render in order: ${marker}`);
      at = next;
    }
  },
);
