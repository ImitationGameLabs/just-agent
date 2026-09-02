// Chain pins for the wired trail table: a drill page's chain must extend
// its list page's chain verbatim and append. The operator-reported defect
// this locks: /rooms/:id showed [rooms -> name] while its own list page
// showed [tagmata -> rooms] -- the bar changed shape on every crumb
// follow. Same shim discipline as breadcrumbs_mobileback_test: the module
// under test pulls rune-bearing stores and compiled paraglide messages,
// so the passthrough shims precede the dynamic import.

declare global {
  function $state<T>(initial: T): T;
  function $state<T>(): T | undefined;
  function $derived<T>(expr: T): T;
}
(globalThis as Record<string, unknown>)["$state"] = (v: unknown) => v;
(globalThis as Record<string, unknown>)["$derived"] = (v: unknown) => v;

const { assertEquals } = await import("@std/assert");
const { matchTrail } = await import("./breadcrumbs.ts");
const { nav_rooms, nav_tagmata, room_label_fallback, settings_heading } =
  await import("../../paraglide/messages.js");

Deno.test("the rooms list keeps its manage-domain chain", () => {
  assertEquals(matchTrail("/rooms"), [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: nav_rooms(), current: true },
  ]);
});

Deno.test("the room drill extends the list chain with the room", () => {
  assertEquals(matchTrail("/rooms/r-1"), [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: nav_rooms(), href: "/rooms" },
    { label: room_label_fallback({ id: "r-1" }), current: true },
  ]);
});

Deno.test("the room settings drill keeps the full chain", () => {
  assertEquals(matchTrail("/rooms/r-1/settings"), [
    { label: nav_tagmata(), href: "/tagmata" },
    { label: nav_rooms(), href: "/rooms" },
    {
      label: room_label_fallback({ id: "r-1" }),
      href: "/rooms/r-1",
    },
    { label: settings_heading(), current: true },
  ]);
});

Deno.test("drill chains extend their list chain (shape continuity)", () => {
  const list = matchTrail("/rooms");
  const drill = matchTrail("/rooms/r-1");
  if (!list || !drill) throw new Error("the rooms chains must resolve");
  assertEquals(
    drill.slice(0, list.length).map((s) => s.label),
    list.map((s) => s.label),
  );
  // the list page's tail becomes a link in the drill chain
  assertEquals(drill[list.length - 1]?.href, "/rooms");
});
