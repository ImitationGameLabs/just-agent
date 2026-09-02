// Tests for the mobile back-row route policy (lib/shell/breadcrumbs.ts
// mobileBack): conversations drill to the chats hub from either chat
// route family; the tagma details sections stay on the bottom bar
// (null -- the manage cell lights via RootLayout isActive); the agent
// detail and the tagma hub keep their trail drill; bar-cell destinations
// (/account, /tagmata) never grow a row.
//
// The module under test transitively imports rune-bearing stores and
// the compiled paraglide messages, so the $state / $derived passthrough
// shims are declared before the dynamic import (deno test runs the
// modules uncompiled) -- the directSessions_test pattern.

declare global {
  function $state<T>(initial: T): T;
  function $state<T>(): T | undefined;
  function $derived<T>(expr: T): T;
}
(globalThis as Record<string, unknown>)["$state"] = (v: unknown) => v;
(globalThis as Record<string, unknown>)["$derived"] = (v: unknown) => v;

const { assertEquals } = await import("@std/assert");
const { mobileBack } = await import("./breadcrumbs.ts");

Deno.test("the tagma chat drills back to the chats hub", () => {
  assertEquals(mobileBack("/tagma/t-1/chat")?.href, "/chats");
});

Deno.test("the relay chat keeps its chats-hub back target", () => {
  assertEquals(mobileBack("/chat/c-1")?.href, "/chats");
});

Deno.test("the tagma details sections keep the bottom bar", () => {
  assertEquals(mobileBack("/tagma/t-1/details/overview"), null);
  assertEquals(mobileBack("/tagma/t-1/details/budget"), null);
  assertEquals(mobileBack("/tagma/t-1/details/agents"), null);
});

Deno.test("the agent detail stays a drill to its agents section", () => {
  assertEquals(
    mobileBack("/tagma/t-1/details/agents/a-9")?.href,
    "/tagma/t-1/details/agents",
  );
});

Deno.test("the tagma hub keeps its registry drill", () => {
  assertEquals(mobileBack("/tagma/t-1")?.href, "/tagmata");
});

Deno.test("bar-cell destinations never grow a back row", () => {
  assertEquals(mobileBack("/account"), null);
  assertEquals(mobileBack("/tagmata"), null);
});
