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

/** The manage details hub: `/tagma/<uuid>/details` (redirects to overview). */
export function tagmaDetailsPath(tagmaId: string): string {
  return `/tagma/${tagmaId}/details`;
}
