// Types for the shared app shell. Kept in a plain `.ts` module (not inside the
// AppShell component) so consumers can import `NavItem` for type-only use.
import type { Component } from "svelte";

import {
  shell_connecting,
  shell_error,
  shell_live,
  shell_offline,
} from "../paraglide/messages.js";

// The indicator's visual tokens (dot classes + SR label) live HERE, not in
// AppShell: both the sidebar (AppShell) and hub rows (HubRow) render the
// same status dot, so the mapping must not fork.
export function navIndicatorDotClass(indicator: NavIndicator): string {
  switch (indicator) {
    case "live":
      return "bg-success-500";
    case "down":
      return "bg-surface-400-600";
    case "error":
      return "bg-error-500";
    // Unreachable at runtime: callers render a spinner for "pending"
    // before reaching here. Kept so the switch stays exhaustive.
    case "pending":
      return "bg-surface-400-600";
  }
}

// The dot itself is aria-hidden (decorative); this label carries the status
// to screen readers so an SR user learns the channel's liveness, not just
// its name.
export function navIndicatorLabel(indicator: NavIndicator): string {
  switch (indicator) {
    case "live":
      return shell_live();
    case "pending":
      return shell_connecting();
    case "down":
      return shell_offline();
    case "error":
      return shell_error();
  }
}

/** A small status indicator AppShell renders as a leading dot instead of an
 * icon (e.g. per-chat liveness in the sidebar). AppShell owns the visual
 * tokens; consumers map their domain state to this tri-state (+ error). */
export type NavIndicator = "live" | "pending" | "down" | "error";

// A single navigation entry. Exactly one leading mark: either an `icon`
// (a Svelte component rendered as `<Icon class="size-4" />`) or an
// `indicator` (a status dot). The discriminated union enforces mutual
// exclusivity at the type level; a third arm allows text-only entries.
export type NavItem =
  | { href: string; label: string; icon: Component; indicator?: never }
  | { href: string; label: string; icon?: never; indicator: NavIndicator }
  | { href: string; label: string; icon?: never; indicator?: never };
