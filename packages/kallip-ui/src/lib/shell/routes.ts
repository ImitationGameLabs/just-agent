/**
 * Single source for the app's tagma-centric URLs (the path-builder from the
 * tagma-centric online IA plan). Every href/redirect must come from here, so
 * a future route rename is a one-line change instead of a repo-wide string
 * hunt. Route params are interpolated verbatim; callers pass real tagma ids.
 */

/** The tagma chat page: `/tagma/<uuid>/chat`. */
export function tagmaChatPath(tagmaId: string): string {
  return `/tagma/${tagmaId}/chat`;
}

/** The manage details hub: `/tagma/<uuid>/details`; the hub route is a
 * permanent redirect to the overview section. */
export function tagmaDetailsPath(tagmaId: string): string {
  return `/tagma/${tagmaId}/details`;
}

/** The manage details sections beneath the hub. */
export type TagmaDetailsSection =
  | "overview"
  | "budget"
  | "agents"
  | "profiles"
  | "schedules";

/** A manage details section: `/tagma/<uuid>/details/<section>`. */
export function tagmaDetailsSectionPath(
  tagmaId: string,
  section: TagmaDetailsSection,
): string {
  return `${tagmaDetailsPath(tagmaId)}/${section}`;
}
